//! ddg-spotlight: a Spotlight-style DuckDuckGo web search TUI for Omarchy.

mod app;
mod ddg;
mod local;
mod search;
mod theme;
mod ui;

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::{App, Mode, Status};
use crate::search::{Debouncer, LocalSearchWorker, SearchWorker, SuggestionWorker};
use crate::theme::Theme;

/// Debounce delay between the last keystroke and firing a query.
const DEBOUNCE: Duration = Duration::from_millis(120);
/// Event poll tick — also drives debounce/worker polling.
const TICK: Duration = Duration::from_millis(50);

fn main() -> io::Result<()> {
    // Debug helper: `ddg-spotlight --query "rust"` prints a normalized result.
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--query" || flag == "-q" {
            let q: String = args.collect::<Vec<_>>().join(" ");
            let client = ddg::build_client();
            match ddg::fetch_all(&client, &q) {
                Ok(r) => {
                    println!("heading: {}", r.heading);
                    println!("answer : {}", r.answer_text);
                    println!("source : {}", r.source);
                    println!("links  : {}", r.links.len());
                    for (i, l) in r.links.iter().take(10).enumerate() {
                        println!("  {}. {} [{}]", i + 1, l.title, l.domain);
                        println!("     {}", l.url);
                        if !l.snippet.is_empty() {
                            println!("     {}", l.snippet);
                        }
                    }
                    println!("related: {} topics", r.related.len());
                }
                Err(e) => eprintln!("error: {e}"),
            }
            return Ok(());
        }
        if flag == "--files" || flag == "--folders" {
            let term = args.collect::<Vec<_>>().join(" ");
            let kind = if flag == "--files" {
                local::LocalKind::Files
            } else {
                local::LocalKind::Dirs
            };
            let index = local::build_index(&local::home_dir());
            let results = local::search(&index, kind, &term, 20);
            println!(
                "indexed {} files / {} dirs; {} matches for {:?}:",
                index.files.len(),
                index.dirs.len(),
                results.len(),
                term
            );
            for e in &results {
                println!("  {}/{}", e.parent, e.name);
            }
            return Ok(());
        }
    }

    let mut tui = Tui::new()?;
    let theme = Theme::load();
    let res = run(&mut tui.terminal, &theme);
    // Tui's Drop restores the terminal regardless of how run() exited.
    drop(tui);
    res
}

/// Main event/render loop.
fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, theme: &Theme) -> io::Result<()> {
    let mut app = App::new();
    let search_worker = SearchWorker::spawn();
    let suggest_worker = SuggestionWorker::spawn();
    let local_worker = LocalSearchWorker::spawn();
    let mut debouncer = Debouncer::new(DEBOUNCE);
    let mut clicks = ui::ClickMap::default();

    loop {
        terminal.draw(|f| {
            clicks = ui::render(f, &app, theme);
        })?;

        // 1) Drain web autocomplete suggestions (web mode only).
        if let Some(suggestions) = suggest_worker.try_recv_latest() {
            if app.mode == Mode::Web && !app.results_visible {
                app.set_suggestions(suggestions);
            }
        }

        // 2) Drain web search results.
        if let Some(outcome) = search_worker.try_recv_latest() {
            app.apply_outcome(outcome.generation, outcome.result);
        }

        // 3) Drain local file/folder results.
        if let Some(outcome) = local_worker.try_recv_latest() {
            app.apply_local(outcome.generation, outcome.results);
        }

        // 4) Fire a debounced query: web autocomplete or live local search.
        if debouncer.take_ready(Instant::now()).is_some() {
            match app.mode.local_kind() {
                Some(kind) => {
                    app.status = Status::Searching;
                    local_worker.dispatch(app.generation, kind, app.term().to_string());
                }
                None => {
                    let term = app.term().to_string();
                    if !term.trim().is_empty() && !app.results_visible {
                        suggest_worker.suggest(term);
                    }
                }
            }
        }

        // 5) Handle input.
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    match key.code {
                        KeyCode::Esc => {
                            if app.results_visible {
                                app.dismiss_results();
                            } else {
                                app.should_quit = true;
                            }
                        }
                        KeyCode::Char('c') if ctrl => app.should_quit = true,
                        KeyCode::Char('u') if ctrl => {
                            app.clear_query();
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        KeyCode::Enter => {
                            if app.mode.is_local() {
                                // Open the highlighted file/folder.
                                if let Some(path) = app.selected_local_path() {
                                    xdg_open(&path);
                                    app.should_quit = true;
                                }
                            } else if app.results_visible {
                                if let Some(url) = app.open_url() {
                                    xdg_open(&url);
                                    app.should_quit = true;
                                }
                            } else {
                                // Accept a highlighted suggestion into the query first.
                                if let Some(s) = app.selected_suggestion() {
                                    app.set_query(s);
                                }
                                if !app.term().trim().is_empty() {
                                    app.status = Status::Searching;
                                    search_worker.dispatch(app.generation, app.term().to_string());
                                }
                            }
                        }
                        KeyCode::Tab => {
                            if app.mode.is_local() || app.results_visible {
                                app.select_next();
                            } else if !app.suggestions.is_empty() {
                                let pick = app
                                    .selected_suggestion()
                                    .or_else(|| app.suggestions.first().cloned());
                                if let Some(s) = pick {
                                    app.set_query(s);
                                    debouncer.on_change(app.query.clone(), Instant::now());
                                }
                            }
                        }
                        KeyCode::Down => {
                            if app.mode.is_local() || app.results_visible {
                                app.select_next();
                            } else {
                                app.suggestion_next();
                            }
                        }
                        KeyCode::Up => {
                            if app.mode.is_local() || app.results_visible {
                                app.select_prev();
                            } else {
                                app.suggestion_prev();
                            }
                        }
                        // --- Caret movement (left/right within the text) ---
                        KeyCode::Left if ctrl => app.move_word_left(),
                        KeyCode::Left => app.move_left(),
                        KeyCode::Right if ctrl => app.move_word_right(),
                        KeyCode::Right => app.move_right(),
                        KeyCode::Home => app.move_home(),
                        KeyCode::End => app.move_end(),
                        KeyCode::Char('a') if ctrl => app.move_home(),
                        KeyCode::Char('e') if ctrl => app.move_end(),
                        // --- Editing ---
                        KeyCode::Backspace if ctrl => {
                            app.delete_word_back();
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        KeyCode::Char('w') if ctrl => {
                            app.delete_word_back();
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        KeyCode::Backspace => {
                            app.backspace();
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        KeyCode::Delete => {
                            app.delete_forward();
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        // Number keys 1-9 open the matching web result directly. In
                        // local mode digits are part of the search term, so this only
                        // applies when web results are shown.
                        KeyCode::Char(c)
                            if app.results_visible && c.is_ascii_digit() && c != '0' =>
                        {
                            let idx = (c as u8 - b'1') as usize;
                            if let Some(url) = app.link_url_at(idx) {
                                xdg_open(&url);
                                app.should_quit = true;
                            }
                        }
                        KeyCode::Char(c) if !ctrl => {
                            app.push_char(c);
                            debouncer.on_change(app.query.clone(), Instant::now());
                        }
                        _ => {}
                    }
                }
                Event::Mouse(me) => {
                    handle_mouse(me, &clicks, &mut app, &search_worker, &mut debouncer);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Open a URL, file, or folder with the user's default handler via xdg-open.
///
/// The child is fully detached (its own process group + null stdio) so it
/// survives the launcher exiting — otherwise the SIGHUP sent when this window
/// closes would kill the browser/file-manager before it finishes launching.
fn xdg_open(target: &str) {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let _ = std::process::Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

/// Route a mouse event: left-click opens a result / switches mode, the wheel
/// moves the selection.
fn handle_mouse(
    me: MouseEvent,
    clicks: &ui::ClickMap,
    app: &mut App,
    search_worker: &SearchWorker,
    debouncer: &mut Debouncer,
) {
    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // A mode tab?
            if let Some(mode) = clicks.tab_hit(me.column, me.row) {
                if app.mode != mode {
                    app.set_mode(mode);
                    debouncer.on_change(app.query.clone(), Instant::now());
                }
                return;
            }
            // A result row?
            if let Some((kind, idx)) = clicks.list_hit(me.column, me.row) {
                match kind {
                    ui::HitKind::Link => {
                        app.select(idx);
                        if let Some(url) = app.open_url() {
                            xdg_open(&url);
                            app.should_quit = true;
                        }
                    }
                    ui::HitKind::Local => {
                        app.select(idx);
                        if let Some(path) = app.selected_local_path() {
                            xdg_open(&path);
                            app.should_quit = true;
                        }
                    }
                    ui::HitKind::Suggestion => {
                        if let Some(s) = app.suggestions.get(idx).cloned() {
                            app.set_query(s);
                            app.status = Status::Searching;
                            search_worker.dispatch(app.generation, app.term().to_string());
                        }
                    }
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if app.mode.is_local() || app.results_visible {
                app.select_next();
            } else {
                app.suggestion_next();
            }
        }
        MouseEventKind::ScrollUp => {
            if app.mode.is_local() || app.results_visible {
                app.select_prev();
            } else {
                app.suggestion_prev();
            }
        }
        _ => {}
    }
}

/// RAII guard that sets up the alternate screen + raw mode and restores the
/// terminal on drop, even if the app panics or returns an error.
struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    keyboard_enhanced: bool,
}

impl Tui {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        // Kitty keyboard protocol (when supported) makes Ctrl-Backspace and
        // other modified keys reliably distinguishable from plain ones.
        let keyboard_enhanced = matches!(supports_keyboard_enhancement(), Ok(true));
        let mut stdout = io::stdout();
        // Mouse capture stops the terminal from drawing drag-selections over the
        // overlay (plain clicks go to the app; Shift-drag still selects); the
        // blinking block cursor replaces the drawn caret.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            SetCursorStyle::BlinkingBlock
        )?;
        if keyboard_enhanced {
            let _ = execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Tui {
            terminal,
            keyboard_enhanced,
        })
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.keyboard_enhanced {
            let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            self.terminal.backend_mut(),
            SetCursorStyle::DefaultUserShape,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
