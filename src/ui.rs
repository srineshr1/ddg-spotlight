//! Rendering — Spotlight-style: a centered rounded "card" with a search field
//! (magnifier glyph + placeholder) and a full-width highlighted results list.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Status};
use crate::theme::Theme;

/// Magnifier glyph shown at the start of the search field (nerd-font search
/// icon, present in the Omarchy default fonts).
const SEARCH_ICON: &str = "\u{f002}";
/// Left padding (in spaces) applied to every list row so the full-width
/// selection bar has a little breathing room on the left, matching Spotlight.
const ROW_PAD: &str = "  ";

fn pad(s: impl Into<String>) -> String {
    format!("{ROW_PAD}{}", s.into())
}

fn build_items(app: &App, theme: &Theme) -> (Vec<ListItem<'static>>, usize) {
    let muted = Style::default().fg(theme.muted);
    let accent = Style::default().fg(theme.accent);
    let base = Style::default().fg(theme.foreground);
    let error = Style::default().fg(theme.error);

    let mut items: Vec<ListItem<'static>> = Vec::new();

    match &app.status {
        Status::Idle => {}
        Status::Searching => {
            items.push(ListItem::new(Line::from(Span::styled(
                pad("Searching…"),
                muted,
            ))));
        }
        Status::Error(e) => {
            items.push(ListItem::new(Line::from(Span::styled(
                pad(e.clone()),
                error,
            ))));
        }
        Status::Done if app.result.is_empty() => {
            items.push(ListItem::new(Line::from(Span::styled(
                pad("No instant answer — Enter for full search…"),
                muted.add_modifier(Modifier::ITALIC),
            ))));
        }
        Status::Done => {
            if !app.result.heading.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    pad(app.result.heading.clone()),
                    accent.add_modifier(Modifier::BOLD),
                ))));
            }
            if !app.result.answer_text.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    pad(app.result.answer_text.clone()),
                    base,
                ))));
            }
            if !app.result.source.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    pad(format!("— {}", app.result.source)),
                    muted.add_modifier(Modifier::ITALIC),
                ))));
            }
            if !app.result.related.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    String::new(),
                    muted,
                )))); // spacer
            }

            let link_offset = items.len();
            for topic in &app.result.related {
                items.push(ListItem::new(Line::from(Span::styled(
                    pad(topic.label.clone()),
                    base,
                ))));
            }
            return (items, link_offset);
        }
    }

    let len = items.len();
    (items, len)
}

/// Compute the centered card and the input/results regions inside it.
///
/// Returns `(card, input, results)` where `card` is the bordered box, `input`
/// is the single-row search field, and `results` is the list area below the
/// separator. The separator occupies the row directly under `input`.
pub fn compute_layout(area: Rect) -> (Rect, Rect, Rect) {
    // Card size: a comfortable centered box, clamped to the terminal.
    let w = (area.width as f32 * 0.6) as u16;
    let w = w.clamp(40, area.width);
    let h = (area.height as f32 * 0.6) as u16;
    let h = h.clamp(6, area.height);

    let x = area.x + (area.width.saturating_sub(w)) / 2;
    // Sit a bit above vertical center, like Spotlight.
    // let top = (area.height as f32 * 0.18) as u16;
    // let top = top.min(area.height.saturating_sub(h));
    // let y = area.y + top;

    // let card = Rect {
    //     x,
    //     y,
    //     width: w,
    //     height: h,
    // };

    // Inside the border, with a little horizontal/vertical padding.
    let inner = card.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let [input, _sep, results] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search field
            Constraint::Length(1), // separator
            Constraint::Min(0),    // results
        ])
        .areas(inner);

    (card, input, results)
// }

pub fn render(f: &mut Frame, app: &App, theme: &Theme) {
    let area = f.area();
    let (card, input, results) = compute_layout(area);
    //
    let bg = ratatui::style::Color::Black;
    let accent = Style::default().fg(theme.accent).bg(bg);
    let muted = Style::default().fg(theme.muted).bg(bg);
    let bold = Style::default()
        .fg(theme.foreground)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    // --- Card: rounded border, filled background (floats over the dimmed desktop) ---
    let card_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent).bg(bg))
        .style(Style::default().bg(bg));
    f.render_widget(card_block, card);

    // --- Search field: magnifier glyph + query (or muted placeholder) + cursor ---
    let icon = Span::styled(format!("{SEARCH_ICON}  "), accent);
    let cursor = Span::styled("▏", accent);
    let line = if app.query.is_empty() {
        Line::from(vec![
            icon,
            Span::styled("Search…", muted.add_modifier(Modifier::ITALIC)),
            cursor,
        ])
    } else {
        Line::from(vec![icon, Span::styled(app.query.clone(), bold), cursor])
    };
    f.render_widget(Paragraph::new(line), input);

    // --- Separator under the search field ---
    let sep = Rect {
        x: input.x,
        y: input.y + 1,
        width: input.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(sep.width as usize),
            muted,
        ))),
        sep,
    );

    // --- Results ---
    if results.height == 0 || results.width < 2 {
        return;
    }

    let (list_items, link_offset) = build_items(app, theme);
    let list_selected = app.selected.map(|i| i + link_offset);

    let list = List::new(list_items)
        .highlight_style(
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD),
        )
        // No arrow marker — the full-width background bar marks the selection.
        .highlight_symbol("");

    let mut state = ListState::default();
    state.select(list_selected);
    f.render_stateful_widget(list, results, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddg::{SearchResult, Topic};

    #[test]
    fn layout_stacks_inside_card() {
        let area = Rect::new(0, 0, 120, 40);
        let (card, input, results) = compute_layout(area);
        // Input sits above the results.
        assert!(input.y < results.y);
        // Both regions live inside the card border.
        assert!(input.x >= card.x && input.x < card.x + card.width);
        assert!(input.y > card.y);
        assert!(results.y + results.height <= card.y + card.height);
        // Card is centered and reasonably sized.
        assert!(card.width >= 40);
        assert!(card.width <= 120);
    }

    #[test]
    fn build_idle_empty() {
        let (items, off) = build_items(&App::new(), &Theme::default());
        assert_eq!(items.len(), 0);
        assert_eq!(off, 0);
    }

    #[test]
    fn build_done_topics() {
        let mut app = App::new();
        app.push_char('r');
        let r = SearchResult {
            heading: "H".into(),
            answer_text: "A".into(),
            source: "S".into(),
            related: vec![
                Topic {
                    label: "l1".into(),
                    url: "u1".into(),
                },
                Topic {
                    label: "l2".into(),
                    url: "u2".into(),
                },
            ],
            ..Default::default()
        };
        app.apply_outcome(1, Ok(r));
        let (items, off) = build_items(&app, &Theme::default());
        // heading, answer, source, spacer, l1, l2 = 6
        assert_eq!(items.len(), 6);
        assert_eq!(off, 4);
    }
}
