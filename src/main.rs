//! ddg-spotlight: a Spotlight-style DuckDuckGo web search TUI for Omarchy.

mod app;
mod ddg;
mod search;
mod theme;
mod ui;

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::search::{Debouncer, SearchWorker};
use crate::theme::Theme;

/// Debounce delay between the last keystroke and firing a query.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Event poll tick — also drives debounce/worker polling.
const TICK: Duration = Duration::from_millis(50);

fn main() -> io::Result<()> {
    // Debug helper: `ddg-spotlight --query "rust"` prints a normalized result.
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--query" || flag == "-q" {
            let q: String = args.collect::<Vec<_>>().join(" ");
            match ddg::fetch(&q) {
                Ok(r) => {
                    println!("heading: {}", r.heading);
                    println!("answer : {}", r.answer_text);
                    println!("source : {}", r.source);
                    println!("related: {} topics", r.related.len());
                    for t in r.related.iter().take(10) {
                        println!("  - {} ({})", t.label, t.url);
                    }
                }
                Err(e) => eprintln!("error: {e}"),
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
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: &Theme,
) -> io::Result<()> {
    let mut app = App::new();
    let worker = SearchWorker::spawn();
    let mut debouncer = Debouncer::new(DEBOUNCE);

    loop {
        terminal.draw(|f| ui::render(f, &app, theme))?;

        // 1) Drain any completed searches and apply the latest matching one.
        if let Some(outcome) = worker.try_recv_latest() {
            app.apply_outcome(outcome.generation, outcome.result);
        }

        // 2) Fire a debounced query if one is ready.
        if let Some(query) = debouncer.take_ready(Instant::now()) {
            if !query.trim().is_empty() {
                worker.dispatch(app.generation, query);
            }
        }

        // 3) Handle input (with a timeout so we keep ticking).
        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => app.should_quit = true,
                    KeyCode::Char('c') if ctrl => app.should_quit = true,
                    KeyCode::Char('u') if ctrl => {
                        app.clear_query();
                        debouncer.on_change(app.query.clone(), Instant::now());
                    }
                    KeyCode::Enter => {
                        if let Some(url) = app.open_url() {
                            open_in_browser(&url);
                            // Spotlight-style: opening dismisses the launcher.
                            app.should_quit = true;
                        }
                    }
                    KeyCode::Down => app.select_next(),
                    KeyCode::Up => app.select_prev(),
                    KeyCode::Backspace => {
                        app.backspace();
                        debouncer.on_change(app.query.clone(), Instant::now());
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

/// Open a URL in the user's default browser via xdg-open (detached).
fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
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
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Tui { terminal })
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
