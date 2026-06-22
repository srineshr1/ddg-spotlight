//! Application state and pure state transitions (kept UI/IO-free for testing).

use crate::ddg::{self, SearchResult};
use crate::local::{LocalEntry, LocalKind};

/// Maximum number of autocomplete suggestions kept for the dropdown.
pub const MAX_SUGGESTIONS: usize = 6;

/// Search mode, selected by a leading sigil in the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Default: web search via DuckDuckGo.
    Web,
    /// `/` sigil: search local folders.
    Folder,
    /// `@` sigil: search local files.
    File,
}

impl Mode {
    /// The local-search kind for this mode, if it is a local mode.
    pub fn local_kind(self) -> Option<LocalKind> {
        match self {
            Mode::File => Some(LocalKind::Files),
            Mode::Folder => Some(LocalKind::Dirs),
            Mode::Web => None,
        }
    }

    /// Whether this is a local (filesystem) mode.
    pub fn is_local(self) -> bool {
        !matches!(self, Mode::Web)
    }
}

/// Parse the leading sigil of a query into a [`Mode`] and the remaining term.
/// `@` → files, `/` → folders, anything else → web.
pub fn parse_query(query: &str) -> (Mode, &str) {
    match query.as_bytes().first() {
        Some(b'@') => (Mode::File, query[1..].trim_start()),
        Some(b'/') => (Mode::Folder, query[1..].trim_start()),
        _ => (Mode::Web, query),
    }
}

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
    /// Current query text (including any leading mode sigil).
    pub query: String,
    /// Caret position as a byte index into `query` (always on a char boundary).
    pub cursor: usize,
    /// Generation counter; bumped on every query change.
    pub generation: u64,
    /// Current search mode (derived from the query sigil).
    pub mode: Mode,
    /// Latest web result shown.
    pub result: SearchResult,
    /// Local file/folder results (meaningful in File/Folder mode).
    pub local_results: Vec<LocalEntry>,
    /// Current status.
    pub status: Status,
    /// Selected index into the active results list (web links or local results).
    pub selected: Option<usize>,
    /// Set to true when the app should exit.
    pub should_quit: bool,
    /// Live autocomplete suggestions shown in the dropdown below the bar.
    pub suggestions: Vec<String>,
    /// Highlighted suggestion index, if any (meaningful while typing in web mode).
    pub suggestion_selected: Option<usize>,
    /// Whether the web results panel is visible (true after Enter in web mode).
    pub results_visible: bool,
}

impl Default for App {
    fn default() -> Self {
        App {
            query: String::new(),
            cursor: 0,
            generation: 0,
            mode: Mode::Web,
            result: SearchResult::default(),
            local_results: Vec::new(),
            status: Status::Idle,
            selected: None,
            should_quit: false,
            suggestions: Vec::new(),
            suggestion_selected: None,
            results_visible: false,
        }
    }
}

impl App {
    pub fn new() -> Self {
        App::default()
    }

    /// The query with its mode sigil stripped (what actually gets searched).
    pub fn term(&self) -> &str {
        parse_query(&self.query).1
    }

    /// Insert a character at the caret and advance the caret.
    pub fn push_char(&mut self, c: char) {
        self.query.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.on_query_changed();
    }

    /// Delete the character before the caret (Backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.prev_boundary(self.cursor);
        self.query.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.on_query_changed();
    }

    /// Delete the character at the caret (Delete / forward-delete).
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.query.len() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.query.replace_range(self.cursor..end, "");
        self.on_query_changed();
    }

    /// Delete the word before the caret (Ctrl-Backspace / Ctrl-W).
    pub fn delete_word_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let head = &self.query[..self.cursor];
        let trimmed = head.trim_end_matches(char::is_whitespace);
        let start = trimmed.trim_end_matches(|c: char| !c.is_whitespace()).len();
        self.query.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.on_query_changed();
    }

    /// Clear the entire query.
    pub fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.cursor = 0;
            self.on_query_changed();
        }
    }

    /// Replace the query wholesale (e.g. after accepting a suggestion); the
    /// caret moves to the end.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.cursor = self.query.len();
        self.on_query_changed();
    }

    /// Switch mode while preserving the typed term (adds/removes the sigil).
    pub fn set_mode(&mut self, mode: Mode) {
        let term = self.term().to_string();
        self.query = match mode {
            Mode::Web => term,
            Mode::File => format!("@{term}"),
            Mode::Folder => format!("/{term}"),
        };
        self.cursor = self.query.len();
        self.on_query_changed();
    }

    // --- Caret movement (no query change, so no generation bump / re-search) ---

    /// Move the caret one character left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_boundary(self.cursor);
        }
    }

    /// Move the caret one character right.
    pub fn move_right(&mut self) {
        if self.cursor < self.query.len() {
            self.cursor = self.next_boundary(self.cursor);
        }
    }

    /// Move the caret to the start of the query.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move the caret to the end of the query.
    pub fn move_end(&mut self) {
        self.cursor = self.query.len();
    }

    /// Move the caret to the start of the previous word.
    pub fn move_word_left(&mut self) {
        let head = &self.query[..self.cursor];
        let trimmed = head.trim_end_matches(char::is_whitespace);
        self.cursor = trimmed.trim_end_matches(|c: char| !c.is_whitespace()).len();
    }

    /// Move the caret to the end of the next word.
    pub fn move_word_right(&mut self) {
        let tail = &self.query[self.cursor..];
        let ws = tail.len() - tail.trim_start_matches(char::is_whitespace).len();
        let rest = &tail[ws..];
        let word = rest.len() - rest.trim_start_matches(|c: char| !c.is_whitespace()).len();
        self.cursor += ws + word;
    }

    /// Byte index of the char boundary just before `i` (`i` must be > 0).
    fn prev_boundary(&self, i: usize) -> usize {
        self.query[..i]
            .char_indices()
            .next_back()
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    /// Byte index of the char boundary just after `i` (`i` must be < len).
    fn next_boundary(&self, i: usize) -> usize {
        self.query[i..]
            .chars()
            .next()
            .map(|c| i + c.len_utf8())
            .unwrap_or(i)
    }

    /// Caret column within the displayed (sigil-stripped) term, in characters.
    /// Used to position the block cursor in the search field.
    pub fn caret_col(&self) -> usize {
        let (_, term) = parse_query(&self.query);
        let term_start = self.query.len() - term.len();
        let c = self.cursor.clamp(term_start, self.query.len());
        self.query[term_start..c].chars().count()
    }

    /// Set the selected list index (used by mouse clicks), clamped to the list.
    pub fn select(&mut self, idx: usize) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        self.selected = Some(idx.min(len - 1));
    }

    fn on_query_changed(&mut self) {
        self.generation += 1;
        self.mode = parse_query(&self.query).0;
        self.selected = None;
        self.suggestions.clear();
        self.suggestion_selected = None;
        self.local_results.clear();
        self.results_visible = false;
        self.result = SearchResult::default();
        self.status = Status::Idle;
    }

    /// Apply a web result from the worker if it matches the current generation.
    pub fn apply_outcome(&mut self, generation: u64, result: Result<SearchResult, String>) {
        if generation != self.generation {
            return; // stale
        }
        self.results_visible = true;
        self.suggestions.clear();
        self.suggestion_selected = None;
        match result {
            Ok(r) => {
                // Highlight the first link by default, Google-style.
                self.selected = (!r.links.is_empty()).then_some(0);
                self.result = r;
                self.status = Status::Done;
            }
            Err(e) => {
                self.status = Status::Error(e);
            }
        }
    }

    /// Apply local file/folder results if they match the current generation.
    pub fn apply_local(&mut self, generation: u64, results: Vec<LocalEntry>) {
        if generation != self.generation {
            return; // stale
        }
        self.local_results = results;
        self.selected = (!self.local_results.is_empty()).then_some(0);
        self.status = Status::Done;
    }

    /// Store live autocomplete suggestions (capped), resetting the highlight.
    pub fn set_suggestions(&mut self, suggestions: Vec<String>) {
        self.suggestions = suggestions.into_iter().take(MAX_SUGGESTIONS).collect();
        self.suggestion_selected = None;
    }

    /// The currently highlighted suggestion text, if any.
    pub fn selected_suggestion(&self) -> Option<String> {
        self.suggestion_selected
            .and_then(|i| self.suggestions.get(i).cloned())
    }

    /// Move the suggestion highlight down.
    pub fn suggestion_next(&mut self) {
        let len = self.suggestions.len();
        if len == 0 {
            return;
        }
        self.suggestion_selected = Some(match self.suggestion_selected {
            Some(i) if i + 1 < len => i + 1,
            Some(i) => i,
            None => 0,
        });
    }

    /// Move the suggestion highlight up (off the top clears the highlight).
    pub fn suggestion_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        match self.suggestion_selected {
            Some(0) | None => self.suggestion_selected = None,
            Some(i) => self.suggestion_selected = Some(i - 1),
        }
    }

    /// Dismiss the results panel and return to typing (keeps the query).
    pub fn dismiss_results(&mut self) {
        self.results_visible = false;
        self.selected = None;
    }

    /// Length of the list currently being navigated with Up/Down.
    fn active_len(&self) -> usize {
        if self.mode.is_local() {
            self.local_results.len()
        } else if self.results_visible {
            self.result.links.len()
        } else {
            0
        }
    }

    /// Move selection down through the active list (clamped at the bottom).
    pub fn select_next(&mut self) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) if i + 1 < len => i + 1,
            Some(i) => i,
            None => 0,
        });
    }

    /// Move selection up through the active list (clamped at the top).
    pub fn select_prev(&mut self) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) if i > 0 => i - 1,
            _ => 0,
        });
    }

    /// URL for the web link at a 0-based index (used by the number keys 1-9).
    pub fn link_url_at(&self, idx: usize) -> Option<String> {
        self.result.links.get(idx).map(|l| l.url.clone())
    }

    /// The selected local entry's path, if any (Enter in File/Folder mode).
    pub fn selected_local_path(&self) -> Option<String> {
        self.selected
            .and_then(|i| self.local_results.get(i))
            .map(|e| e.path.clone())
    }

    /// Determine the URL to open on Enter in web mode: the selected link, or the
    /// full DuckDuckGo results page for the current query. Returns None if
    /// there's nothing to open (empty query and no selection).
    pub fn open_url(&self) -> Option<String> {
        if let Some(i) = self.selected {
            if let Some(link) = self.result.links.get(i) {
                return Some(link.url.clone());
            }
        }
        let q = self.term().trim();
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

    fn result_with_links(n: usize) -> SearchResult {
        SearchResult {
            links: (0..n)
                .map(|i| crate::ddg::WebLink {
                    title: format!("t{i}"),
                    url: format!("https://e{i}.example"),
                    snippet: format!("snippet {i}"),
                    domain: format!("e{i}.example"),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn local_entries(n: usize) -> Vec<LocalEntry> {
        (0..n)
            .map(|i| LocalEntry {
                path: format!("/home/u/f{i}"),
                name: format!("f{i}"),
                parent: "~".to_string(),
            })
            .collect()
    }

    #[test]
    fn parse_query_detects_sigils() {
        assert_eq!(parse_query("rust async"), (Mode::Web, "rust async"));
        assert_eq!(parse_query("@report"), (Mode::File, "report"));
        assert_eq!(parse_query("/projects"), (Mode::Folder, "projects"));
        // Sigil with trailing space is trimmed; bare sigil yields an empty term.
        assert_eq!(parse_query("@  notes"), (Mode::File, "notes"));
        assert_eq!(parse_query("/"), (Mode::Folder, ""));
        assert_eq!(parse_query("@"), (Mode::File, ""));
    }

    #[test]
    fn typing_sigil_sets_mode_and_term() {
        let mut app = App::new();
        app.push_char('@');
        app.push_char('d');
        app.push_char('o');
        app.push_char('c');
        assert_eq!(app.mode, Mode::File);
        assert_eq!(app.term(), "doc");
    }

    #[test]
    fn switching_sigil_updates_mode() {
        let mut app = App::new();
        app.set_query("/downloads".into());
        assert_eq!(app.mode, Mode::Folder);
        app.set_query("rust".into());
        assert_eq!(app.mode, Mode::Web);
    }

    #[test]
    fn typing_bumps_generation() {
        let mut app = App::new();
        app.push_char('r');
        assert_eq!(app.query, "r");
        assert_eq!(app.generation, 1);
        assert_eq!(app.status, Status::Idle);
        assert!(!app.results_visible);
    }

    #[test]
    fn typing_clears_results_and_suggestions() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_links(2)));
        assert!(app.results_visible);
        app.push_char('b'); // typing again hides results
        assert!(!app.results_visible);
        assert!(app.suggestions.is_empty());
        assert!(app.result.links.is_empty());
    }

    #[test]
    fn apply_local_highlights_first() {
        let mut app = App::new();
        app.push_char('@');
        app.apply_local(app.generation, local_entries(3));
        assert_eq!(app.local_results.len(), 3);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.status, Status::Done);
    }

    #[test]
    fn stale_local_outcome_is_ignored() {
        let mut app = App::new();
        app.push_char('@'); // gen 1
        app.push_char('a'); // gen 2
        app.apply_local(1, local_entries(3));
        assert!(app.local_results.is_empty());
    }

    #[test]
    fn local_navigation_and_open() {
        let mut app = App::new();
        app.push_char('@');
        app.apply_local(app.generation, local_entries(3));
        assert_eq!(app.selected, Some(0));
        app.select_next();
        assert_eq!(app.selected, Some(1));
        app.select_prev();
        app.select_prev(); // clamp at top
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.selected_local_path(), Some("/home/u/f0".to_string()));
    }

    #[test]
    fn clearing_query_returns_to_idle() {
        let mut app = App::new();
        app.push_char('r');
        app.backspace();
        assert_eq!(app.query, "");
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.mode, Mode::Web);
    }

    #[test]
    fn stale_outcome_is_ignored() {
        let mut app = App::new();
        app.push_char('a'); // gen 1
        app.push_char('b'); // gen 2
        app.apply_outcome(1, Ok(result_with_links(2)));
        assert!(!app.results_visible);
        assert!(app.result.links.is_empty());
    }

    #[test]
    fn matching_outcome_highlights_first_link() {
        let mut app = App::new();
        app.push_char('a'); // gen 1
        app.apply_outcome(1, Ok(result_with_links(2)));
        assert_eq!(app.status, Status::Done);
        assert!(app.results_visible);
        assert_eq!(app.result.links.len(), 2);
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn link_selection_clamps_at_bounds() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_links(2)));
        assert_eq!(app.selected, Some(0));
        app.select_next();
        assert_eq!(app.selected, Some(1));
        app.select_next();
        assert_eq!(app.selected, Some(1));
        app.select_prev();
        assert_eq!(app.selected, Some(0));
        app.select_prev();
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn open_url_uses_selected_link() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_links(2)));
        app.select_next();
        assert_eq!(app.open_url(), Some("https://e1.example".to_string()));
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

    #[test]
    fn suggestions_store_cap_and_navigate() {
        let mut app = App::new();
        let many: Vec<String> = (0..10).map(|i| format!("s{i}")).collect();
        app.set_suggestions(many);
        assert_eq!(app.suggestions.len(), MAX_SUGGESTIONS);
        assert_eq!(app.suggestion_selected, None);
        assert_eq!(app.selected_suggestion(), None);

        app.suggestion_next();
        assert_eq!(app.suggestion_selected, Some(0));
        assert_eq!(app.selected_suggestion(), Some("s0".to_string()));
        app.suggestion_prev();
        assert_eq!(app.suggestion_selected, None);
    }

    #[test]
    fn set_query_replaces_and_resets() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(1, Ok(result_with_links(2)));
        app.set_query("rustlang".into());
        assert_eq!(app.query, "rustlang");
        assert!(!app.results_visible);
        assert!(app.suggestions.is_empty());
        assert!(app.result.links.is_empty());
    }

    #[test]
    fn dismiss_results_keeps_query() {
        let mut app = App::new();
        app.push_char('r');
        app.apply_outcome(1, Ok(result_with_links(2)));
        assert!(app.results_visible);
        app.dismiss_results();
        assert!(!app.results_visible);
        assert_eq!(app.selected, None);
        assert_eq!(app.query, "r");
    }

    #[test]
    fn caret_insert_in_middle() {
        let mut app = App::new();
        for c in "rust".chars() {
            app.push_char(c);
        }
        assert_eq!(app.cursor, 4);
        app.move_left();
        app.move_left();
        assert_eq!(app.cursor, 2);
        app.push_char('X'); // "ruXst"
        assert_eq!(app.query, "ruXst");
        assert_eq!(app.cursor, 3);
        app.move_home();
        assert_eq!(app.cursor, 0);
        app.move_end();
        assert_eq!(app.cursor, app.query.len());
    }

    #[test]
    fn delete_forward_removes_char_at_caret() {
        let mut app = App::new();
        for c in "abc".chars() {
            app.push_char(c);
        }
        app.move_home();
        app.delete_forward(); // removes 'a'
        assert_eq!(app.query, "bc");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn delete_word_back_removes_previous_word() {
        let mut app = App::new();
        for c in "rust async".chars() {
            app.push_char(c);
        }
        app.delete_word_back();
        assert_eq!(app.query, "rust ");
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn word_movement() {
        let mut app = App::new();
        for c in "foo bar".chars() {
            app.push_char(c);
        }
        app.move_word_left(); // start of "bar"
        assert_eq!(app.cursor, 4);
        app.move_word_left(); // start of "foo"
        assert_eq!(app.cursor, 0);
        app.move_word_right(); // end of "foo"
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn caret_col_accounts_for_sigil() {
        let mut app = App::new();
        for c in "@doc".chars() {
            app.push_char(c);
        }
        assert_eq!(app.caret_col(), 3); // after "doc"
        app.move_home(); // before the '@'
        assert_eq!(app.caret_col(), 0); // clamped to term start
    }

    #[test]
    fn set_mode_preserves_term() {
        let mut app = App::new();
        for c in "notes".chars() {
            app.push_char(c);
        }
        app.set_mode(Mode::File);
        assert_eq!(app.query, "@notes");
        assert_eq!(app.mode, Mode::File);
        assert_eq!(app.term(), "notes");
        assert_eq!(app.cursor, app.query.len());
        app.set_mode(Mode::Web);
        assert_eq!(app.query, "notes");
        assert_eq!(app.mode, Mode::Web);
    }

    #[test]
    fn select_clamps_to_list() {
        let mut app = App::new();
        app.push_char('a');
        app.apply_outcome(app.generation, Ok(result_with_links(3)));
        app.select(2);
        assert_eq!(app.selected, Some(2));
        app.select(99);
        assert_eq!(app.selected, Some(2));
    }
}
