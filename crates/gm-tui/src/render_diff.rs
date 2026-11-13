//! Draws the **Diff** tab (D236): a header with the path, `+a −r` and the hunk at the cursor;
//! each hunk's bar with Git's function context and its **[stage]** or **[unstage]** button;
//! each line with its number, its sign and its code in the terminal's colors.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use gm_schema::{DiffLine, LineKind, SyntaxRole};
use gm_view::diff_rows::{Notice, Row, no_newline_mark, notice_text};

use crate::app::{Action, App, Context, Focus, Hit};
use crate::diff::{DiffView, LineColors};
use crate::render::{BOLD, DIM, ON, Strip};

/// Columns of the line number.
const NUMBER_WIDTH: usize = 5;

pub fn draw_diff(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Main;
    let context = app.context;
    let Some(view) = app.diff.as_mut() else {
        let text = format!("No {} changes.", context.title().to_lowercase());
        frame.render_widget(Paragraph::new(text).style(DIM), area);
        return;
    };
    let header_area = Rect { height: 1, ..area };
    if view.anchor.is_some() {
        let mut strip = Strip::new(header_area);
        selection_header(view, &mut strip);
        let (spans, buttons) = strip.into_parts();
        frame.render_widget(Paragraph::new(Line::from(spans)), header_area);
        app.hits.extend(buttons);
    } else {
        frame.render_widget(Paragraph::new(header(view)), header_area);
    }
    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    view.body_top = body.y;
    if let Some(message) = message(view) {
        let text = Paragraph::new(Line::styled(format!(" {message}"), DIM));
        frame.render_widget(
            text,
            Rect {
                y: body.y + 1,
                height: 1,
                ..body
            },
        );
        return;
    }
    let height = usize::from(body.height);
    view.keep_cursor_visible(height);
    let width = usize::from(body.width);
    let mut lines = Vec::with_capacity(height);
    for (index, row) in view.rows.iter().enumerate().skip(view.scroll).take(height) {
        let y = body.y + u16::try_from(index - view.scroll).unwrap_or(u16::MAX);
        let row_area = Rect {
            y,
            height: 1,
            ..body
        };
        app.hits.push(Hit {
            area: row_area,
            action: Action::DiffRow(index),
        });
        let mut line = match *row {
            Row::HunkHeader { hunk_index, .. } => {
                hunk_bar(view, hunk_index, row_area, &mut app.hits)
            }
            Row::Line {
                hunk_index,
                line_index,
                ..
            } => match view.line(hunk_index, line_index) {
                Some(line) => code_line(
                    line,
                    view.colors.get(&(hunk_index, line_index)),
                    width,
                    view.is_selected(index),
                ),
                None => Line::default(),
            },
            _ => Line::default(),
        };
        if focused && index == view.cursor {
            line = line.patch_style(ON);
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), body);
}

fn header(view: &DiffView) -> Line<'static> {
    let mut spans = vec![Span::styled(view.path.clone(), BOLD)];
    if view.diff.is_some() {
        let (added, removed) = view.counts();
        spans.push(Span::styled(
            format!("  +{added}"),
            Style::new().fg(Color::Green),
        ));
        spans.push(Span::styled(
            format!(" −{removed}"),
            Style::new().fg(Color::Red),
        ));
        let hunks = view.hunks().len();
        if let Some(hunk) = view.cursor_hunk() {
            spans.push(Span::styled(format!(" · hunk {}/{hunks}", hunk + 1), DIM));
        }
    }
    spans.push(Span::styled(
        format!(" · {}", view.context.title().to_lowercase()),
        DIM,
    ));
    Line::from(spans)
}

/// The header while lines are selected: the range and the buttons that act on it.
fn selection_header(view: &DiffView, strip: &mut Strip<'static>) {
    strip.text(view.path.clone(), BOLD);
    if let Some(label) = view.selection_label() {
        strip.text(format!(" · {label}  "), Style::new().fg(Color::Blue));
    }
    let (label, color) = match view.context {
        Context::Unstaged => ("[stage lines]", Color::Green),
        Context::Staged => ("[unstage lines]", Color::Yellow),
    };
    strip
        .button(label, Style::new().fg(color), Action::StageSelection)
        .text(" ", Style::new())
        .button("[clear]", DIM, Action::ClearSelection);
}

/// What shows in place of the lines, if anything.
fn message(view: &DiffView) -> Option<String> {
    if let Some(error) = &view.error {
        return Some(error.clone());
    }
    let Some(diff) = view.diff.as_ref() else {
        return Some("Reading the diff…".to_owned());
    };
    if diff.binary {
        return Some(notice_text(Notice::Binary));
    }
    if diff.hunks.is_empty() {
        return Some("No text changes: a rename or a mode change only.".to_owned());
    }
    None
}

fn hunk_bar(view: &DiffView, hunk_index: usize, area: Rect, hits: &mut Vec<Hit>) -> Line<'static> {
    let Some(hunk) = view.hunk(hunk_index) else {
        return Line::default();
    };
    let mut strip = Strip::new(area);
    strip.text("@@ ", Style::new().fg(Color::Blue));
    let title = if hunk.header.trim().is_empty() {
        format!("line {}", hunk.new_start)
    } else {
        hunk.header.trim().to_owned()
    };
    strip.text(title, BOLD).text("  ", Style::new());
    let (label, color) = match view.context {
        Context::Unstaged => ("[stage]", Color::Green),
        Context::Staged => ("[unstage]", Color::Yellow),
    };
    strip.button(label, Style::new().fg(color), Action::StageHunk(hunk_index));
    let (spans, buttons) = strip.into_parts();
    hits.extend(buttons);
    Line::from(spans)
}

fn code_line(
    line: &DiffLine,
    colors: Option<&LineColors>,
    width: usize,
    selected: bool,
) -> Line<'static> {
    let (number, sign, sign_style, text_style) = match line.kind {
        LineKind::Added => (line.new_no, "+", Style::new().fg(Color::Green), None),
        LineKind::Removed => (
            None,
            "−",
            Style::new().fg(Color::Red),
            Some(Style::new().fg(Color::Red)),
        ),
        LineKind::Context => (line.new_no, " ", DIM, None),
    };
    let number = number.map_or_else(
        || " ".repeat(NUMBER_WIDTH),
        |no| format!("{no:>NUMBER_WIDTH$}"),
    );
    let mut spans = if selected {
        // The selection's mark takes the first column of the number.
        let rest: String = number.chars().skip(1).collect();
        vec![
            Span::styled("▌", Style::new().fg(Color::Blue)),
            Span::styled(rest, sign_style.add_modifier(Modifier::BOLD)),
        ]
    } else {
        vec![Span::styled(number, sign_style)]
    };
    spans.push(Span::styled(format!(" {sign} "), sign_style));
    let room = width.saturating_sub(NUMBER_WIDTH + 3);
    spans.extend(code_spans(&line.text, colors, text_style, room));
    if line.no_newline {
        spans.push(Span::styled(format!("  {}", no_newline_mark(true)), DIM));
    }
    Line::from(spans)
}

/// The spans of a line's code: its colored ranges in their role's color, the rest plain;
/// tabs as spaces; cut at `room` columns. `whole` colors the whole line instead.
pub fn code_spans(
    text: &str,
    colors: Option<&LineColors>,
    whole: Option<Style>,
    room: usize,
) -> Vec<Span<'static>> {
    let mut pieces: Vec<(&str, Style)> = Vec::new();
    match (whole, colors) {
        (Some(_), _) | (None, None) => pieces.push((text, whole.unwrap_or_default())),
        (None, Some(colors)) => {
            let mut at = 0;
            for &(start, end, role) in colors {
                let valid = start >= at
                    && end <= text.len()
                    && start < end
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end);
                if !valid {
                    continue;
                }
                if start > at {
                    pieces.push((&text[at..start], Style::new()));
                }
                pieces.push((&text[start..end], role_style(role)));
                at = end;
            }
            pieces.push((&text[at..], Style::new()));
        }
    }
    let mut left = room;
    let mut spans = Vec::new();
    for (piece, style) in pieces {
        if left == 0 {
            break;
        }
        let expanded: String = piece.replace('\t', "    ").chars().take(left).collect();
        left -= expanded.chars().count();
        if !expanded.is_empty() {
            spans.push(Span::styled(expanded, style));
        }
    }
    spans
}

/// The terminal color of a role (D236 (4)).
pub fn role_style(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Comment => Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SyntaxRole::String => Style::new().fg(Color::Green),
        SyntaxRole::Number | SyntaxRole::Constant | SyntaxRole::Attribute => {
            Style::new().fg(Color::Yellow)
        }
        SyntaxRole::Keyword => Style::new().fg(Color::Magenta),
        SyntaxRole::Function => Style::new().fg(Color::Blue),
        SyntaxRole::Type => Style::new().fg(Color::Cyan),
        SyntaxRole::Variable
        | SyntaxRole::Property
        | SyntaxRole::Operator
        | SyntaxRole::Punctuation => Style::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_cut_into_colored_and_plain_pieces_with_tabs_as_spaces() {
        let colors = vec![(0, 2, SyntaxRole::Keyword), (3, 4, SyntaxRole::Function)];
        let spans = code_spans("fn a\t() {}", Some(&colors), None, 80);
        let shown: Vec<(&str, Style)> = spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style))
            .collect();
        assert_eq!(
            shown,
            [
                ("fn", role_style(SyntaxRole::Keyword)),
                (" ", Style::new()),
                ("a", role_style(SyntaxRole::Function)),
                ("    () {}", Style::new()),
            ]
        );
    }

    #[test]
    fn a_long_line_is_cut_at_the_room_left() {
        let spans = code_spans("abcdefghij", None, None, 4);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "abcd");
    }

    #[test]
    fn a_range_off_a_character_boundary_is_left_plain() {
        let colors = vec![(1, 2, SyntaxRole::String)];
        let spans = code_spans("é", Some(&colors), None, 10);
        assert_eq!(spans[0].content, "é");
        assert_eq!(spans[0].style, Style::new());
    }
}
