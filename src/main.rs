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
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
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

    loop {
        terminal.draw(|f| ui::render(f, &app, theme))?;

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
            if let Event::Key(key) = event::read()? {
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
                    KeyCode::Backspace => {
                        app.backspace();
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
                    KeyCode::Char(c) => {
                        app.push_char(c);
                        debouncer.on_change(app.query.clone(), Instant::now());
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Open a URL, file, or folder with the user's default handler via xdg-open
/// (detached).
fn xdg_open(target: &str) {
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

/// RAII guard that sets up the alternate screen + raw mode and restores the
/// terminal on drop, even if the app panics or returns an error.
struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Mouse capture stops the terminal from drawing drag-selections over the
        // overlay; the blinking block cursor replaces the drawn caret.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            SetCursorStyle::BlinkingBlock
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Tui { terminal })
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            SetCursorStyle::DefaultUserShape,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
