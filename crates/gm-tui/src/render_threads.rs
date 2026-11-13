//! Draws the threads in the diff and the comment box (D236). What a card holds comes from
//! `gm_view::review` (its parts, badge, chips, reply heads, toggles and resolve label), the
//! same model the app's diff pane draws; this module only puts it in terminal cells.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use gm_schema::ReviewComment;
use gm_view::compose;
use gm_view::review::{
    self, AGENT_WRITTEN, Badge, Chips, Heading, NEEDS_ANSWER, Part, made_label, reply_head,
    resolve_label,
};

use crate::app::{Action, Hit};
use crate::render::{BOLD, DIM, ON, Strip};
use crate::threads::{Composer, Folds, TYPES};

/// Columns before a card's edge: the line number and the sign of a diff line.
pub const CARD_INDENT: u16 = 8;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_secs()).ok())
        .unwrap_or(0)
}

fn badge_word(badge: Badge) -> &'static str {
    match badge {
        Badge::Open => "Open",
        Badge::Handled => "Handled",
        Badge::Resolved => "Resolved",
        Badge::Outdated => "Outdated",
    }
}

fn badge_color(badge: Badge) -> Color {
    match badge {
        Badge::Open => Color::Yellow,
        Badge::Handled => Color::Cyan,
        Badge::Resolved => Color::Green,
        Badge::Outdated => Color::Magenta,
    }
}

/// The words beside a card's status: who wrote it, its label, kind and severity, whether it
/// waits for an answer, its decorations.
fn chip_words(chips: &Chips) -> Vec<String> {
    let mut words = Vec::new();
    if chips.agent {
        words.push(AGENT_WRITTEN.to_owned());
    }
    words.extend(chips.label.map(|label| label.as_str().to_owned()));
    words.extend(chips.kind.iter().map(|word| (*word).to_owned()));
    if chips.needs_answer {
        words.push(NEEDS_ANSWER.to_owned());
    }
    words.extend(chips.decorations.iter().cloned());
    words
}

/// One row of a card: the edge, then the part.
pub fn card_line(
    comment: Option<&ReviewComment>,
    id: i64,
    part: &Part,
    area: Rect,
    folds: &Folds,
    hits: &mut Vec<Hit>,
) -> Line<'static> {
    let badge = comment.map_or(Badge::Open, review::badge);
    let edge = Style::new().fg(badge_color(badge));
    let mut strip = Strip::new(area);
    strip
        .text(" ".repeat(usize::from(CARD_INDENT)), Style::new())
        .text("│ ", edge);
    match part {
        Part::Head {
            badge,
            lines,
            created_at,
            replyable,
            chips,
        } => {
            strip
                .text(badge_word(*badge), edge.add_modifier(Modifier::BOLD))
                .text(
                    format!(" · {lines} · {}", made_label(now_secs(), *created_at)),
                    DIM,
                );
            for word in chip_words(chips) {
                strip.text(format!(" · {word}"), Style::new().fg(Color::Cyan));
            }
            strip.text("  ", Style::new());
            if *replyable {
                strip
                    .button("[reply]", Style::new().fg(Color::Blue), Action::Reply(id))
                    .text(" ", Style::new());
            }
            let resolved = *badge == Badge::Resolved;
            let label = format!("[{}]", resolve_label(resolved, chips.note).to_lowercase());
            let action = if resolved {
                Action::Reopen(id)
            } else {
                Action::Resolve(id)
            };
            strip
                .button(label, Style::new().fg(Color::Green), action)
                .text(" ", Style::new());
            if resolved {
                strip
                    .button("[fold]", DIM, Action::OpenThread(id))
                    .text(" ", Style::new());
            }
            let delete = if folds.delete.is_armed(id, Instant::now()) {
                "[delete?]"
            } else {
                "[delete]"
            };
            strip.button(delete, Style::new().fg(Color::Red), Action::Delete(id));
        }
        Part::Text(text) => {
            strip.text(text.clone(), Style::new());
        }
        Part::Folded(summary) => {
            hits.push(Hit {
                area,
                action: Action::OpenThread(id),
            });
            strip.text(format!("▸ {summary}"), DIM);
        }
        Part::Reply {
            agent,
            author,
            created_at,
        } => {
            strip.text(reply_head(author, *created_at, now_secs()), BOLD);
            if *agent {
                strip.text(format!(" · {AGENT_WRITTEN}"), Style::new().fg(Color::Cyan));
            }
        }
        Part::Toggle(toggle) => {
            strip.button(
                format!("[{}]", toggle.label()),
                DIM,
                Action::ExpandThread(id),
            );
        }
    }
    let (spans, buttons) = strip.into_parts();
    hits.extend(buttons);
    Line::from(spans)
}

/// The strip above a file's rows: its outdated threads, or the threads outside its hunks.
pub fn heading_line(heading: &Heading, area: Rect, hits: &mut Vec<Hit>) -> Line<'static> {
    match heading {
        Heading::Outdated { count, open } => {
            let mut strip = Strip::new(area);
            let mark = if *open { "▾" } else { "▸" };
            let noun = if *count == 1 { "thread" } else { "threads" };
            strip.button(
                format!(" {mark} {count} outdated {noun}"),
                Style::new().fg(Color::Magenta),
                Action::ToggleOutdated,
            );
            let (spans, buttons) = strip.into_parts();
            hits.extend(buttons);
            Line::from(spans)
        }
        Heading::Outside => Line::styled(" Threads on lines this diff does not show:", DIM),
    }
}

/// The lines of the box's text, wrapped to `width`, with the cursor at its end.
fn text_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::from(vec![
            Span::styled("█", Style::new().fg(Color::Blue)),
            Span::styled(" Write the comment.", DIM),
        ])];
    }
    let mut lines: Vec<Line> = review::wrap(text, width.max(1))
        .into_iter()
        .map(Line::raw)
        .collect();
    if let Some(last) = lines.last_mut() {
        last.push_span(Span::styled("█", Style::new().fg(Color::Blue)));
    }
    lines
}

/// Draws the comment box at `top` in `pane`, or as low as it fits.
pub fn draw_composer(
    frame: &mut Frame,
    composer: &Composer,
    pane: Rect,
    top: u16,
    hits: &mut Vec<Hit>,
) {
    let mut own = Vec::new();
    let width = pane.width.saturating_sub(CARD_INDENT).max(20);
    let inner_width = usize::from(width.saturating_sub(2));
    let text = text_lines(&composer.text, inner_width);
    let rows = text.len().min(8)
        + 1
        + usize::from(composer.typed())
        + usize::from(composer.error.is_some() || composer.discarding);
    let height = u16::try_from(rows + 2).unwrap_or(u16::MAX).min(pane.height);
    let bottom = pane.y + pane.height;
    let y = if top + height <= bottom {
        top
    } else {
        bottom.saturating_sub(height)
    };
    let area = Rect::new(pane.x + pane.width - width, y, width, height);
    let block = Block::bordered()
        .title(format!(" {} ", compose::title(&composer.target)))
        .border_style(Style::new().fg(Color::Blue));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    let mut y = inner.y;
    if composer.typed() {
        let mut strip = Strip::new(Rect::new(inner.x, y, inner.width, 1));
        strip.text("type ", DIM);
        for (index, (word, _)) in TYPES.iter().enumerate() {
            let style = if index == composer.kind % TYPES.len() {
                ON
            } else {
                DIM
            };
            strip
                .button(format!(" {word} "), style, Action::CommentType(index))
                .text(" ", Style::new());
        }
        strip.text(" Tab", DIM);
        let (spans, buttons) = strip.into_parts();
        own.extend(buttons);
        lines.push(Line::from(spans));
        y += 1;
    }
    let shown = text.len().saturating_sub(8);
    for line in text.into_iter().skip(shown) {
        lines.push(line);
        y += 1;
    }
    if let Some(error) = &composer.error {
        lines.push(Line::styled(error.clone(), Style::new().fg(Color::Red)));
        y += 1;
    } else if composer.discarding {
        lines.push(Line::styled(
            "Esc again discards the text.",
            Style::new().fg(Color::Yellow),
        ));
        y += 1;
    }
    let mut strip = Strip::new(Rect::new(inner.x, y, inner.width, 1));
    let save = format!("[{}]", compose::save_label(&composer.target).to_lowercase());
    let save_style = if composer.may_save() {
        Style::new().fg(Color::Green)
    } else {
        DIM
    };
    strip
        .button(save, save_style, Action::SaveComment)
        .text(" Enter  ", DIM)
        .button("[cancel]", DIM, Action::CancelComment)
        .text(" Esc  · new line Alt+Enter", DIM);
    let (spans, buttons) = strip.into_parts();
    own.extend(buttons);
    lines.push(Line::from(spans));
    frame.render_widget(Paragraph::new(lines), inner);
    // The box takes the clicks over it: only its own buttons act there.
    hits.retain(|hit| !area.intersects(hit.area));
    hits.extend(own);
}
