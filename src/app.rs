//! Application state and pure state transitions (kept UI/IO-free for testing).

use crate::ddg::{self, SearchResult};

/// Current search/network status, surfaced in the footer/status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nothing typed yet.
    Idle,
    /// A query is in flight.
    Searching,
    /// A successful result is shown.
    Done,
    /// An error occurred.
    Error(String),
}

/// The whole application state.
pub struct App {
    /// Current query text.
    pub query: String,
    /// Generation counter; bumped on every query change.
    pub generation: u64,
    /// Latest result shown.
    pub result: SearchResult,
    /// Current status.
    pub status: Status,
    /// Selected related-topic index, if any.
    pub selected: Option<usize>,
    /// Set to true when the app should exit.
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        App {
            query: String::new(),
            generation: 0,
            result: SearchResult::default(),
            status: Status::Idle,
            selected: None,
            should_quit: false,
        }
    }
}

impl App {
    pub fn new() -> Self {
        App::default()
    }

    /// Insert a character at the end of the query and bump the generation.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.on_query_changed();
    }

    /// Remove the last character of the query.
    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.on_query_changed();
        }
    }

    /// Clear the entire query.
    pub fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.on_query_changed();
        }
    }

    fn on_query_changed(&mut self) {
        self.generation += 1;
        self.selected = None;
        if self.query.trim().is_empty() {
            self.result = SearchResult::default();
            self.status = Status::Idle;
        } else {
            self.status = Status::Searching;
        }
    }

    /// Apply a result from the worker if it matches the current generation.
    pub fn apply_outcome(&mut self, generation: u64, result: Result<SearchResult, String>) {
        if generation != self.generation {
            return; // stale
        }
        match result {
            Ok(r) => {
                self.result = r;
                self.status = Status::Done;
                self.selected = None;
            }
            Err(e) => {
                self.status = Status::Error(e);
            }
        }
    }

    /// Move selection down through the related-topics list.
    pub fn select_next(&mut self) {
        let len = self.result.related.len();
        if len == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) if i + 1 < len => i + 1,
            Some(i) => i, // stop at bottom
            None => 0,
        });
    }

    /// Move selection up through the related-topics list.
    pub fn select_prev(&mut self) {
        let len = self.result.related.len();
        if len == 0 {
            return;
        }
        match self.selected {
            Some(0) | None => self.selected = None, // back above the list
            Some(i) => self.selected = Some(i - 1),
        }
    }

    /// Determine the URL to open on Enter: the selected topic, or the full
    /// DuckDuckGo results page for the current query. Returns None if there's
    /// nothing to open (empty query and no selection).
    pub fn open_url(&self) -> Option<String> {
        if let Some(i) = self.selected {
            if let Some(topic) = self.result.related.get(i) {
                return Some(topic.url.clone());
            }
        }
        let q = self.query.trim();
        if q.is_empty() {
            None
        } else {
            Some(ddg::results_url(q))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddg::Topic;

    fn result_with_topics(n: usize) -> SearchResult {
        SearchResult {
            related: (0..n)
                .map(|i| Topic {
                    label: format!("t{i}"),
                    url: format!("https://e{i}.example"),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn typing_bumps_generation_and_sets_searching() {
        let mut app = App::new();
        app.push_char('r');
        assert_eq!(app.query, "r");
        assert_eq!(app.generation, 1);
        assert_eq!(app.status, Status::Searching);
    }

    #[test]
    fn clearing_query_returns_to_idle() {
        let mut app = App::new();
        app.push_char('r');
        app.backspace();
        assert_eq!(app.query, "");
        assert_eq!(app.status, Status::Idle);
    }

    #[test]
    fn stale_outcome_is_ignored() {
        let mut app = App::new();
        app.push_char('a'); // gen 1
        app.push_char('b'); // gen 2
        app.apply_outcome(1, Ok(result_with_topics(2)));
        // gen 1 is stale; status stays Searching, no result applied.
        assert_eq!(app.status, Status::Searching);
        assert!(app.result.related.is_empty());
    }

    #[test]
    fn matching_outcome_is_applied() {
        let mut app = App::new();
        app.push_char('a'); // gen 1
        app.apply_outcome(1, Ok(result_with_topics(2)));
        assert_eq!(app.status, Status::Done);
        assert_eq!(app.result.related.len(), 2);
    }

    #[test]
    fn selection_clamps_at_bounds() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_topics(2)));
        assert_eq!(app.selected, None);
        app.select_next(); // -> 0
        assert_eq!(app.selected, Some(0));
        app.select_next(); // -> 1
        app.select_next(); // stays at 1 (bottom)
        assert_eq!(app.selected, Some(1));
        app.select_prev(); // -> 0
        assert_eq!(app.selected, Some(0));
        app.select_prev(); // -> None (above list)
        assert_eq!(app.selected, None);
    }

    #[test]
    fn open_url_uses_selected_topic() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_topics(2)));
        app.select_next();
        assert_eq!(app.open_url(), Some("https://e0.example".to_string()));
    }

    #[test]
    fn open_url_falls_back_to_results_page() {
        let mut app = App::new();
        for c in "rust lang".chars() {
            app.push_char(c);
        }
        assert_eq!(
            app.open_url(),
            Some("https://duckduckgo.com/?q=rust%20lang".to_string())
        );
    }

    #[test]
    fn open_url_none_when_empty() {
        let app = App::new();
        assert_eq!(app.open_url(), None);
    }
}
