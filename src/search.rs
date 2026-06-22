//! Debounced background search and autocomplete.
//!
//! Two workers run on separate threads:
//! - `SearchWorker`: full search — Instant Answer + ranked web links (Enter).
//! - `SuggestionWorker`: autocomplete suggestions as the user types.
//!
//! Both use generation-based staleness so fast typing never shows stale results.

use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::ddg::{self, SearchResult};
use crate::local::{self, LocalEntry, LocalKind};

/// A request sent to the worker thread.
pub struct Job {
    pub generation: u64,
    pub query: String,
}

/// A response sent back from the worker thread.
pub struct Outcome {
    pub generation: u64,
    #[allow(dead_code)]
    pub query: String,
    pub result: Result<SearchResult, String>,
}

/// Handle to the background search worker.
pub struct SearchWorker {
    job_tx: Sender<Job>,
    outcome_rx: Receiver<Outcome>,
}

impl SearchWorker {
    /// Spawn the worker using the real network fetch with a shared client.
    pub fn spawn() -> SearchWorker {
        let client = ddg::build_client();
        Self::spawn_with(move |q| ddg::fetch_all(&client, q))
    }

    /// Spawn the worker with a custom fetch function (used by tests).
    pub fn spawn_with<F>(fetch: F) -> SearchWorker
    where
        F: Fn(&str) -> Result<SearchResult, String> + Send + 'static,
    {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel::<Outcome>();

        thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let result = fetch(&job.query);
                // If the UI has gone away, stop.
                if outcome_tx
                    .send(Outcome {
                        generation: job.generation,
                        query: job.query,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        SearchWorker { job_tx, outcome_rx }
    }

    /// Dispatch a query to the worker.
    pub fn dispatch(&self, generation: u64, query: String) {
        let _ = self.job_tx.send(Job { generation, query });
    }

    /// Non-blocking drain of the most recent available outcome, if any.
    /// Returns the latest outcome (older queued ones are discarded).
    pub fn try_recv_latest(&self) -> Option<Outcome> {
        let mut latest = None;
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            latest = Some(outcome);
        }
        latest
    }
}

/// Handle to the background autocomplete/suggestion worker.
pub struct SuggestionWorker {
    tx: Sender<String>,
    rx: Receiver<Vec<String>>,
}

impl SuggestionWorker {
    pub fn spawn() -> Self {
        let client = ddg::build_client();
        Self::spawn_with(move |q| ddg::suggest(&client, q))
    }

    pub fn spawn_with<F>(fetch: F) -> Self
    where
        F: Fn(&str) -> Vec<String> + Send + 'static,
    {
        let (tx, job_rx) = std::sync::mpsc::channel::<String>();
        let (result_tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        thread::spawn(move || {
            while let Ok(query) = job_rx.recv() {
                let suggestions = fetch(&query);
                if result_tx.send(suggestions).is_err() {
                    break;
                }
            }
        });
        SuggestionWorker { tx, rx }
    }

    pub fn suggest(&self, query: String) {
        let _ = self.tx.send(query);
    }

    /// Non-blocking drain: returns the latest batch, discarding older.
    pub fn try_recv_latest(&self) -> Option<Vec<String>> {
        let mut latest = None;
        while let Ok(suggestions) = self.rx.try_recv() {
            latest = Some(suggestions);
        }
        latest
    }
}

/// Tracks pending input and decides when a debounced query is ready to fire.
pub struct Debouncer {
    delay: Duration,
    /// The query awaiting dispatch, with the instant it was last edited.
    pending: Option<(String, Instant)>,
    /// The last query we actually dispatched (to avoid duplicate dispatches).
    last_dispatched: Option<String>,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Debouncer {
            delay,
            pending: None,
            last_dispatched: None,
        }
    }

    /// Record that the query changed at `now`.
    pub fn on_change(&mut self, query: String, now: Instant) {
        // No-op if it matches what we already dispatched and nothing is pending.
        self.pending = Some((query, now));
    }

    /// If the debounce delay has elapsed for the pending query (and it differs
    /// from the last dispatched one), return it and mark it dispatched.
    pub fn take_ready(&mut self, now: Instant) -> Option<String> {
        let (query, changed_at) = self.pending.as_ref()?;
        if now.duration_since(*changed_at) < self.delay {
            return None;
        }
        if self.last_dispatched.as_deref() == Some(query.as_str()) {
            self.pending = None;
            return None;
        }
        let query = query.clone();
        self.last_dispatched = Some(query.clone());
        self.pending = None;
        Some(query)
    }
}

/// Maximum number of local results returned per query.
const MAX_LOCAL_RESULTS: usize = 50;

/// A local file/folder search request.
pub struct LocalJob {
    pub generation: u64,
    pub kind: LocalKind,
    pub term: String,
}

/// A local file/folder search response.
pub struct LocalOutcome {
    pub generation: u64,
    pub results: Vec<LocalEntry>,
}

/// Background worker for local file/folder search. Builds the filesystem index
/// lazily on the first request (so web-only sessions never pay for it), then
/// fuzzy-filters it in-memory on every subsequent request — effectively instant.
pub struct LocalSearchWorker {
    job_tx: Sender<LocalJob>,
    outcome_rx: Receiver<LocalOutcome>,
}

impl LocalSearchWorker {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<LocalJob>();
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel::<LocalOutcome>();

        thread::spawn(move || {
            let mut index: Option<local::Index> = None;
            while let Ok(job) = job_rx.recv() {
                let index = index.get_or_insert_with(|| local::build_index(&local::home_dir()));
                let results = local::search(index, job.kind, &job.term, MAX_LOCAL_RESULTS);
                if outcome_tx
                    .send(LocalOutcome {
                        generation: job.generation,
                        results,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        LocalSearchWorker { job_tx, outcome_rx }
    }

    /// Dispatch a local search request.
    pub fn dispatch(&self, generation: u64, kind: LocalKind, term: String) {
        let _ = self.job_tx.send(LocalJob {
            generation,
            kind,
            term,
        });
    }

    /// Non-blocking drain of the most recent local outcome, discarding older.
    pub fn try_recv_latest(&self) -> Option<LocalOutcome> {
        let mut latest = None;
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            latest = Some(outcome);
        }
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_waits_for_delay() {
        let mut d = Debouncer::new(Duration::from_millis(300));
        let t0 = Instant::now();
        d.on_change("ru".into(), t0);
        assert_eq!(d.take_ready(t0), None, "should not fire immediately");
        let later = t0 + Duration::from_millis(301);
        assert_eq!(d.take_ready(later), Some("ru".to_string()));
    }

    #[test]
    fn debounce_latest_wins() {
        let mut d = Debouncer::new(Duration::from_millis(300));
        let t0 = Instant::now();
        d.on_change("r".into(), t0);
        d.on_change("ru".into(), t0 + Duration::from_millis(100));
        d.on_change("rust".into(), t0 + Duration::from_millis(200));
        // Only the last query should fire, once enough time passes from it.
        let fire_at = t0 + Duration::from_millis(200 + 301);
        assert_eq!(d.take_ready(fire_at), Some("rust".to_string()));
        // Nothing pending afterwards.
        assert_eq!(d.take_ready(fire_at + Duration::from_secs(1)), None);
    }

    #[test]
    fn does_not_redispatch_same_query() {
        let mut d = Debouncer::new(Duration::from_millis(10));
        let t0 = Instant::now();
        d.on_change("rust".into(), t0);
        let t1 = t0 + Duration::from_millis(11);
        assert_eq!(d.take_ready(t1), Some("rust".to_string()));
        // Re-entering the same text should not dispatch again.
        d.on_change("rust".into(), t1);
        let t2 = t1 + Duration::from_millis(20);
        assert_eq!(d.take_ready(t2), None);
    }

    #[test]
    fn worker_roundtrip_and_latest_wins() {
        // Fetch echoes the query length as a fake heading.
        let worker = SearchWorker::spawn_with(|q| {
            Ok(SearchResult {
                heading: q.to_string(),
                ..Default::default()
            })
        });
        worker.dispatch(1, "a".into());
        worker.dispatch(2, "ab".into());
        worker.dispatch(3, "abc".into());

        // Give the worker time to process all jobs.
        thread::sleep(Duration::from_millis(100));
        let latest = worker.try_recv_latest().expect("an outcome");
        assert_eq!(latest.generation, 3);
        assert_eq!(latest.result.unwrap().heading, "abc");
    }
}
