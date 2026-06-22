//! Rendering — a transparent-margin overlay with a centered card that grows
//! from a search bar to a suggestion dropdown / results panel. The card content
//! depends on the active [`Mode`]: web (suggestions + ranked links) or local
//! (file / folder results).
//!
//! The margins are left as the terminal's default background (transparent under
//! a translucent Alacritty window) while the card paints an explicit background
//! (opaque), so the desktop shows through, undimmed, around the card.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Mode, Status};
use crate::theme::Theme;

// Nerd Font glyphs for each mode.
const ICON_WEB: &str = "\u{f002}"; // magnifier
const ICON_FILE: &str = "\u{f15b}"; // file
const ICON_FOLDER: &str = "\u{f07b}"; // folder

const FOOTER_WEB: &str =
    "\u{2191}\u{2193} select  \u{00b7}  Enter open  \u{00b7}  1-9 quick  \u{00b7}  Esc back";
const FOOTER_LOCAL: &str = "\u{2191}\u{2193} select  \u{00b7}  Enter open  \u{00b7}  Esc close";

/// Fraction of the screen width the card occupies (numerator/denominator).
const WIDTH_NUM: u16 = 3;
const WIDTH_DEN: u16 = 5;
const MIN_CARD_WIDTH: u16 = 40;
const MAX_CARD_WIDTH: u16 = 100;
/// Max wrapped answer lines kept in the answer block.
const MAX_ANSWER_LINES: u16 = 4;
/// Max local result rows the card grows to show (more scroll within the list).
const MAX_LOCAL_ROWS: u16 = 12;
/// Columns the mode icon + its trailing spaces occupy ("{glyph}  ").
const ICON_COLS: u16 = 3;
/// Width of the "Web | Files | Folder" tab indicator.
const MODE_TABS_WIDTH: u16 = 20;

/// Compute the card width for a given screen width.
pub fn card_width(screen_w: u16) -> u16 {
    let target = screen_w.saturating_mul(WIDTH_NUM) / WIDTH_DEN;
    let clamped = target.clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH);
    clamped.min(screen_w.saturating_sub(2)).max(1)
}

/// Inner text width available inside the card (borders + 1col padding each side).
fn inner_text_width(card_w: u16) -> u16 {
    card_w.saturating_sub(4)
}

/// Number of lines the answer block needs for the given inner width (0 if none).
fn answer_height(r: &crate::ddg::SearchResult, width: u16) -> u16 {
    if r.heading.is_empty() && r.answer_text.is_empty() {
        return 0;
    }
    let mut h: u16 = 0;
    if !r.heading.is_empty() {
        h += 1;
    }
    if !r.answer_text.is_empty() {
        h += wrap_count(&r.answer_text, width as usize).min(MAX_ANSWER_LINES);
    }
    if !r.source.is_empty() {
        h += 1;
    }
    h + 1 // trailing spacer between answer and links
}

/// Height of the web results body (answer block + links, or a status line).
fn results_height(app: &App, width: u16) -> u16 {
    if let Status::Error(_) = app.status {
        return 1;
    }
    let mut h = answer_height(&app.result, width);
    h += (app.result.links.len() as u16).saturating_mul(2); // 2 lines per link
    h.max(1)
}

/// Desired inner (content) height of the card for the current state.
fn inner_height(app: &App, width: u16, max_inner: u16) -> u16 {
    let mut h: u16 = 1; // the search input row
    if app.mode.is_local() {
        let rows = (app.local_results.len() as u16).clamp(1, MAX_LOCAL_ROWS);
        h = h.saturating_add(rows).saturating_add(1); // results + footer
    } else if app.results_visible {
        h = h.saturating_add(1) // blank separator under the input
            .saturating_add(results_height(app, width))
            .saturating_add(1); // footer hint
    } else if app.status == Status::Searching {
        h = h.saturating_add(1);
    } else if !app.suggestions.is_empty() {
        h = h.saturating_add(app.suggestions.len() as u16);
    }
    h.clamp(1, max_inner.max(1))
}

/// Compute the centered card rectangle for the current state.
pub fn compute_card_rect(area: Rect, app: &App) -> Rect {
    let w = card_width(area.width);
    let top_margin = (area.height / 4).max(1);
    // Leave room for the top margin, a 1-row bottom margin, and the 2 borders.
    let max_inner = area.height.saturating_sub(top_margin + 1 + 2);
    let inner_h = inner_height(app, inner_text_width(w), max_inner);
    let outer_h = (inner_h + 2).min(area.height.saturating_sub(top_margin).max(3));
    let x = area.width.saturating_sub(w) / 2;
    Rect {
        x,
        y: area.y + top_margin,
        width: w,
        height: outer_h,
    }
}

pub fn render(f: &mut Frame, app: &App, theme: &Theme) {
    let area = f.area();
    // Transparent margins: clear everything to the terminal default (Reset) bg.
    f.render_widget(Clear, area);

    if area.width < 12 || area.height < 5 {
        render_search_field(f, Rect { height: 1, ..area }, app, theme);
        return;
    }

    let card = compute_card_rect(area, app);
    render_card(f, card, app, theme);
}

fn render_card(f: &mut Frame, card: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(theme.foreground))
        // Explicit background => opaque card over the transparent margins.
        .style(Style::default().bg(theme.background));
    let inner = block.inner(card);
    f.render_widget(block, card);

    let content = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };
    if content.width == 0 || content.height == 0 {
        return;
    }

    let input_area = Rect {
        height: 1,
        ..content
    };
    render_search_field(f, input_area, app, theme);

    if content.height <= 1 {
        return;
    }
    let rest = Rect {
        y: content.y + 1,
        height: content.height - 1,
        ..content
    };

    if app.mode.is_local() {
        render_local(f, rest, app, theme);
    } else if app.results_visible {
        render_results(f, rest, app, theme);
    } else if app.status == Status::Searching {
        let muted = Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::ITALIC);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("Searching\u{2026}", muted))),
            rest,
        );
    } else if !app.suggestions.is_empty() {
        render_suggestions(f, rest, app, theme);
    }
}

/// Glyph for the current mode.
fn mode_glyph(mode: Mode) -> &'static str {
    match mode {
        Mode::Web => ICON_WEB,
        Mode::File => ICON_FILE,
        Mode::Folder => ICON_FOLDER,
    }
}

/// Placeholder shown when the (sigil-stripped) term is empty.
fn placeholder(mode: Mode) -> &'static str {
    match mode {
        Mode::Web => "Search the web\u{2026}",
        Mode::File => "Search files\u{2026}",
        Mode::Folder => "Search folders\u{2026}",
    }
}

fn render_search_field(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    use ratatui::layout::Alignment;
    let accent = Style::default().fg(theme.accent);
    let muted = Style::default().fg(theme.muted);
    let bold = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);

    // Right-aligned mode indicator: Web | Files | Folder (active highlighted).
    let tabs_w = MODE_TABS_WIDTH.min(area.width);
    if tabs_w > 0 {
        let tabs_area = Rect {
            x: area.x + area.width - tabs_w,
            y: area.y,
            width: tabs_w,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(mode_tabs(app.mode, theme)).alignment(Alignment::Right),
            tabs_area,
        );
    }

    // Left input: the mode icon + the (sigil-stripped) term, leaving room for
    // the tabs. The leading sigil is shown as the icon, not as typed text.
    let input_w = area.width.saturating_sub(tabs_w + 2).max(1);
    let icon = Span::styled(format!("{}  ", mode_glyph(app.mode)), accent);
    let term = app.term();
    let line = if term.is_empty() {
        Line::from(vec![
            icon,
            Span::styled(placeholder(app.mode), muted.add_modifier(Modifier::ITALIC)),
        ])
    } else {
        Line::from(vec![icon, Span::styled(term.to_string(), bold)])
    };
    let input_area = Rect {
        width: input_w,
        ..area
    };
    f.render_widget(Paragraph::new(line), input_area);

    // Real terminal cursor (a blinking block, set in `Tui::new`) at the caret:
    // on the first placeholder letter when empty, else just after the term.
    let caret = ICON_COLS + term.chars().count() as u16;
    let cx = area.x + caret.min(input_w.saturating_sub(1));
    f.set_cursor_position((cx, area.y));
}

/// The right-aligned mode indicator: `Web | Files | Folder`, active highlighted.
fn mode_tabs(mode: Mode, theme: &Theme) -> Line<'static> {
    let active = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(theme.muted);
    let seg = |m: Mode, label: &'static str| {
        Span::styled(label, if m == mode { active } else { inactive })
    };
    Line::from(vec![
        seg(Mode::Web, "Web"),
        Span::styled(" | ", inactive),
        seg(Mode::File, "Files"),
        Span::styled(" | ", inactive),
        seg(Mode::Folder, "Folder"),
    ])
}

fn selection_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.selection_fg)
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD)
}

fn render_suggestions(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let base = Style::default().fg(theme.foreground);
    let items: Vec<ListItem> = app
        .suggestions
        .iter()
        .map(|s| {
            ListItem::new(Line::from(Span::styled(
                format!("  {}", truncate(s, area.width.saturating_sub(2) as usize)),
                base,
            )))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol("");

    let mut state = ListState::default();
    state.select(app.suggestion_selected);
    f.render_stateful_widget(list, area, &mut state);
}

/// Render the local file/folder results (or a status line) + footer.
fn render_local(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let muted = Style::default().fg(theme.muted);
    let bold = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);

    let footer_h: u16 = if area.height >= 3 { 1 } else { 0 };
    if footer_h == 1 {
        let footer = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(FOOTER_LOCAL, muted))),
            footer,
        );
    }
    let body = Rect {
        height: area.height - footer_h,
        ..area
    };
    if body.height == 0 {
        return;
    }

    if app.local_results.is_empty() {
        let msg = if app.status == Status::Searching {
            "Searching files\u{2026}"
        } else if app.term().is_empty() {
            "Type to search\u{2026}"
        } else {
            "No matches"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                muted.add_modifier(Modifier::ITALIC),
            ))),
            body,
        );
        return;
    }

    let width = body.width as usize;
    let items: Vec<ListItem> = app
        .local_results
        .iter()
        .map(|e| {
            // " name        ~/parent" on one line.
            let name = truncate(&e.name, width.saturating_sub(2));
            let used = name.chars().count() + 3;
            let parent = truncate(&e.parent, width.saturating_sub(used + 2));
            ListItem::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(name, bold),
                Span::raw("  "),
                Span::styled(parent, muted),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol("");
    let mut state = ListState::default();
    state.select(app.selected);
    f.render_stateful_widget(list, body, &mut state);
}

fn render_results(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let muted = Style::default().fg(theme.muted);
    let error = Style::default().fg(theme.error);

    let footer_h: u16 = if area.height >= 3 { 1 } else { 0 };
    if footer_h == 1 {
        let footer = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(FOOTER_WEB, muted))),
            footer,
        );
    }
    let body = Rect {
        height: area.height - footer_h,
        ..area
    };
    if body.height == 0 {
        return;
    }

    if let Status::Error(e) = &app.status {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(format!("error: {e}"), error))),
            body,
        );
        return;
    }

    let answer_lines = build_answer_lines(&app.result, theme, body.width as usize);
    let ah = (answer_lines.len() as u16).min(body.height);
    if ah > 0 {
        let answer_area = Rect { height: ah, ..body };
        f.render_widget(Paragraph::new(answer_lines), answer_area);
    }

    let links_area = Rect {
        y: body.y + ah,
        height: body.height.saturating_sub(ah),
        ..body
    };
    if links_area.height == 0 {
        return;
    }

    if app.result.links.is_empty() {
        if app.result.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "No results \u{2014} press Enter to open DuckDuckGo",
                    muted.add_modifier(Modifier::ITALIC),
                ))),
                links_area,
            );
        }
        return;
    }

    let items = build_link_items(app, theme, links_area.width as usize);
    let list = List::new(items)
        .highlight_style(selection_style(theme))
        .highlight_symbol("");
    let mut state = ListState::default();
    state.select(app.selected);
    f.render_stateful_widget(list, links_area, &mut state);
}

/// Build the answer-block lines (heading, wrapped abstract, source, spacer).
fn build_answer_lines(
    r: &crate::ddg::SearchResult,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let accent_bold = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let base = Style::default().fg(theme.foreground);
    let muted_italic = Style::default()
        .fg(theme.muted)
        .add_modifier(Modifier::ITALIC);

    let mut lines: Vec<Line<'static>> = Vec::new();
    if r.heading.is_empty() && r.answer_text.is_empty() {
        return lines;
    }
    if !r.heading.is_empty() {
        lines.push(Line::from(Span::styled(truncate(&r.heading, width), accent_bold)));
    }
    if !r.answer_text.is_empty() {
        for l in wrap_lines(&r.answer_text, width)
            .into_iter()
            .take(MAX_ANSWER_LINES as usize)
        {
            lines.push(Line::from(Span::styled(l, base)));
        }
    }
    if !r.source.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("\u{2014} {}", r.source),
            muted_italic,
        )));
    }
    lines.push(Line::from(String::new())); // spacer
    lines
}

/// Build the selectable link list items (2 lines each: title; domain + snippet).
fn build_link_items(app: &App, theme: &Theme, width: usize) -> Vec<ListItem<'static>> {
    let num_style = Style::default().fg(theme.muted);
    let title_style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);
    let domain_style = Style::default().fg(theme.accent);
    let snippet_style = Style::default().fg(theme.muted);

    app.result
        .links
        .iter()
        .enumerate()
        .map(|(i, link)| {
            let num = i + 1;
            let title = truncate(&link.title, width.saturating_sub(4));
            let title_line = Line::from(vec![
                Span::styled(format!("{num}. "), num_style),
                Span::styled(title, title_style),
            ]);

            let domain = truncate(&link.domain, width.saturating_sub(3));
            let mut meta_spans = vec![Span::raw("   "), Span::styled(domain, domain_style)];
            if !link.snippet.is_empty() {
                let used = link.domain.chars().count() + 6;
                let snippet = truncate(&link.snippet, width.saturating_sub(used));
                meta_spans.push(Span::styled(format!("  {snippet}"), snippet_style));
            }
            ListItem::new(vec![title_line, Line::from(meta_spans)])
        })
        .collect()
}

/// Greedy word-wrap into lines no wider than `width` characters.
fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_count(text: &str, width: usize) -> u16 {
    wrap_lines(text, width).len() as u16
}

/// Truncate to `width` characters, appending an ellipsis when shortened.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "\u{2026}".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddg::{SearchResult, WebLink};
    use crate::local::LocalEntry;

    fn app_with_links(n: usize) -> App {
        let mut app = App::new();
        for c in "rust".chars() {
            app.push_char(c);
        }
        let links = (0..n)
            .map(|i| WebLink {
                title: format!("Title {i}"),
                url: format!("https://e{i}.example"),
                snippet: format!("snippet {i}"),
                domain: format!("e{i}.example"),
            })
            .collect();
        app.apply_outcome(
            app.generation,
            Ok(SearchResult {
                links,
                ..Default::default()
            }),
        );
        app
    }

    fn app_with_local(n: usize) -> App {
        let mut app = App::new();
        app.push_char('@');
        let entries = (0..n)
            .map(|i| LocalEntry {
                path: format!("/home/u/f{i}.txt"),
                name: format!("f{i}.txt"),
                parent: "~/docs".to_string(),
            })
            .collect();
        app.apply_local(app.generation, entries);
        app
    }

    #[test]
    fn card_is_horizontally_centered() {
        let area = Rect::new(0, 0, 120, 40);
        let app = App::new();
        let card = compute_card_rect(area, &app);
        let expected_x = (area.width - card.width) / 2;
        assert_eq!(card.x, expected_x);
        assert!(card.width >= MIN_CARD_WIDTH);
    }

    #[test]
    fn idle_card_is_small() {
        let area = Rect::new(0, 0, 120, 40);
        let app = App::new();
        let card = compute_card_rect(area, &app);
        assert_eq!(card.height, 3);
    }

    #[test]
    fn card_grows_with_suggestions() {
        let area = Rect::new(0, 0, 120, 40);
        let mut app = App::new();
        app.push_char('r');
        app.set_suggestions(vec!["rust".into(), "ruby".into(), "rsync".into()]);
        let card = compute_card_rect(area, &app);
        assert_eq!(card.height, 6);
    }

    #[test]
    fn card_grows_taller_for_results() {
        let area = Rect::new(0, 0, 120, 40);
        let idle = compute_card_rect(area, &App::new());
        let with_results = compute_card_rect(area, &app_with_links(5));
        assert!(with_results.height > idle.height);
        assert!(with_results.y + with_results.height <= area.height);
    }

    #[test]
    fn card_grows_for_local_results() {
        let area = Rect::new(0, 0, 120, 40);
        let idle = compute_card_rect(area, &App::new());
        let local = compute_card_rect(area, &app_with_local(5));
        // input(1) + 5 results + footer(1) + borders(2) = 9.
        assert_eq!(local.height, 9);
        assert!(local.height > idle.height);
    }

    #[test]
    fn card_fits_in_small_screen() {
        let area = Rect::new(0, 0, 30, 12);
        let card = compute_card_rect(area, &app_with_links(20));
        assert!(card.x + card.width <= area.width);
        assert!(card.y + card.height <= area.height);
    }

    #[test]
    fn wrap_lines_wraps_on_width() {
        let lines = wrap_lines("the quick brown fox jumps", 9);
        assert!(lines.iter().all(|l| l.chars().count() <= 9));
        assert!(lines.len() >= 3);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell\u{2026}");
        assert_eq!(truncate("x", 0), "");
    }

    #[test]
    fn link_items_have_two_lines_each() {
        let app = app_with_links(3);
        let items = build_link_items(&app, &Theme::default(), 60);
        assert_eq!(items.len(), 3);
        for item in &items {
            assert_eq!(item.height(), 2);
        }
    }

    #[test]
    fn answer_lines_include_heading_and_source() {
        let r = SearchResult {
            heading: "Rust".into(),
            answer_text: "A systems language.".into(),
            source: "Wikipedia".into(),
            ..Default::default()
        };
        let lines = build_answer_lines(&r, &Theme::default(), 60);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn no_answer_yields_no_lines() {
        let lines = build_answer_lines(&SearchResult::default(), &Theme::default(), 60);
        assert!(lines.is_empty());
    }

    #[test]
    fn mode_glyph_differs_per_mode() {
        assert_eq!(mode_glyph(Mode::Web), ICON_WEB);
        assert_eq!(mode_glyph(Mode::File), ICON_FILE);
        assert_eq!(mode_glyph(Mode::Folder), ICON_FOLDER);
    }

    #[test]
    fn mode_tabs_have_three_segments() {
        // Web | Files | Folder = 3 labels + 2 separators.
        let line = mode_tabs(Mode::File, &Theme::default());
        assert_eq!(line.spans.len(), 5);
        assert_eq!(line.spans[0].content, "Web");
        assert_eq!(line.spans[2].content, "Files");
        assert_eq!(line.spans[4].content, "Folder");
    }
}
