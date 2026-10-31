//! The Markdown of a prompt or a reply, drawn for the Threads detail. `markdown.rs`
//! reads the text into blocks; this file draws them with GPUI and keeps the parse of
//! each text, so a row drawn again is not parsed again.
//!
//! Inline styles are highlight runs of one `StyledText` per paragraph, so the text
//! wraps as prose does. Inline code keeps the body font with a code background, since
//! a highlight cannot change the font family. A redaction marker is a muted run on a
//! selection-coloured background, the chip the plain text paints.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use gm_schema::HighlightSpan;
use gpui::{
    Div, FontStyle, FontWeight, HighlightStyle, IntoElement, StrikethroughStyle, StyledText,
    UnderlineStyle, div, prelude::*, px, rems,
};

use crate::icon::{self, Icon};
use crate::markdown::{self, Block, Inline, Item};
use crate::theme;
use crate::threads;

/// How many parsed texts are kept. A thread's rows are far fewer; past this the cache starts again.
const CACHE_ENTRIES: usize = 256;
/// The width of a list marker's column, wide enough for `10.`.
const MARKER_WIDTH: f32 = 22.;
/// The width of the bar down the side of a quote.
const QUOTE_BAR: f32 = 2.;

/// The parsed texts of the thread shown, keyed by a hash of each text. It is emptied when
/// the thread changes, since its texts belong to that thread.
#[derive(Default)]
pub(crate) struct MarkdownCache {
    thread: Option<String>,
    parsed: HashMap<u64, Rc<Parsed>>,
}

/// A text read for drawing: its blocks, and how many lines past the first ones were left out.
pub(crate) struct Parsed {
    blocks: Vec<Block>,
    hidden: usize,
}

impl MarkdownCache {
    /// Keeps the cache for `uid` and drops it for any other thread.
    pub(crate) fn for_thread(&mut self, uid: &str) {
        if self.thread.as_deref() != Some(uid) {
            self.thread = Some(uid.to_owned());
            self.parsed.clear();
        }
    }

    /// The parse of `text` with its first `max_lines` lines read; parsed once, then kept.
    pub(crate) fn get(&mut self, text: &str, max_lines: usize) -> Rc<Parsed> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(parsed) = self.parsed.get(&key) {
            return Rc::clone(parsed);
        }
        if self.parsed.len() >= CACHE_ENTRIES {
            self.parsed.clear();
        }
        let (shown, hidden) = threads::head(text, max_lines);
        let mut blocks = markdown::parse(shown);
        markdown::colour(&mut blocks);
        let parsed = Rc::new(Parsed { blocks, hidden });
        self.parsed.insert(key, Rc::clone(&parsed));
        parsed
    }
}

/// The blocks of a parsed text, one below the other, with a note of the lines left out.
pub(crate) fn document(parsed: &Parsed) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(0.))
        .children(parsed.blocks.iter().map(block_view))
        .children((parsed.hidden > 0).then(|| {
            div()
                .text_xs()
                .text_color(theme::muted())
                .child(format!("… {} more lines", parsed.hidden))
        }))
}

/// One block: a paragraph, a heading, a code block, a quote, a list, a rule or a table.
fn block_view(block: &Block) -> gpui::AnyElement {
    match block {
        Block::Paragraph(inlines) => inline_text(inlines).into_any_element(),
        Block::Heading { level, inlines } => div()
            .font_weight(FontWeight::BOLD)
            .text_size(rems(heading_scale(*level)))
            .child(inline_text(inlines))
            .into_any_element(),
        Block::Code { text, spans, .. } => code_block(text, spans).into_any_element(),
        Block::Quote(inner) => div()
            .flex()
            .flex_row()
            .gap_2()
            .min_w(px(0.))
            .child(
                div()
                    .flex_none()
                    .w(px(QUOTE_BAR))
                    .rounded_sm()
                    .bg(theme::muted()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    .children(inner.iter().map(block_view)),
            )
            .into_any_element(),
        Block::List {
            ordered,
            start,
            items,
        } => list_view(*ordered, *start, items).into_any_element(),
        Block::Rule => div()
            .my_1()
            .h(px(1.))
            .w_full()
            .bg(theme::hairline())
            .into_any_element(),
        Block::Table { head, rows } => table_view(head, rows).into_any_element(),
    }
}

/// A heading's size as a multiple of the body text: h1 is 1.4, h2 1.25, h3 1.1, the rest the body size.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.4,
        2 => 1.25,
        3 => 1.1,
        _ => 1.,
    }
}

/// The items of a list: a bullet, a number or a check box, then the item's blocks.
fn list_view(ordered: bool, start: u64, items: &[Item]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(0.))
        .children(items.iter().enumerate().map(|(index, item)| {
            let marker = match item.checked {
                Some(true) => {
                    icon::filled(Icon::CheckSquare, icon::SMALL, theme::muted()).into_any_element()
                }
                Some(false) => {
                    icon::view(Icon::Square, icon::SMALL, theme::muted()).into_any_element()
                }
                None if ordered => {
                    let number = start.saturating_add(u64::try_from(index).unwrap_or(0));
                    format!("{number}.").into_any_element()
                }
                None => "•".into_any_element(),
            };
            div()
                .flex()
                .flex_row()
                .gap_1()
                .min_w(px(0.))
                .child(
                    div()
                        .flex_none()
                        .w(px(MARKER_WIDTH))
                        .text_color(theme::muted())
                        .child(marker),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .flex_1()
                        .min_w(px(0.))
                        .children(item.blocks.iter().map(block_view)),
                )
        }))
}

/// A code block on the code background, in the code font. It wraps; it does not scroll sideways.
fn code_block(text: &str, spans: &[HighlightSpan]) -> Div {
    let highlights: Vec<(Range<usize>, HighlightStyle)> = spans
        .iter()
        .filter(|span| {
            span.start < span.end
                && span.end <= text.len()
                && text.is_char_boundary(span.start)
                && text.is_char_boundary(span.end)
        })
        .map(|span| {
            (
                span.start..span.end,
                HighlightStyle {
                    color: Some(theme::syntax(span.role)),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect();
    div()
        .p_2()
        .rounded_sm()
        .bg(theme::editor())
        .font_family(theme::code_font())
        .text_xs()
        .min_w(px(0.))
        .child(StyledText::new(text.to_owned()).with_highlights(highlights))
}

/// A table as a grid: a head row in bold when the table has one, then the rows, each cell wrapping.
fn table_view(head: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) -> Div {
    let mut lines = Vec::new();
    if !head.is_empty() {
        lines.push(table_row(head, true, false));
    }
    for (index, row) in rows.iter().enumerate() {
        lines.push(table_row(row, false, index > 0 || !head.is_empty()));
    }
    div()
        .flex()
        .flex_col()
        .min_w(px(0.))
        .rounded_sm()
        .border_1()
        .border_color(theme::hairline())
        .children(lines)
}

fn table_row(cells: &[Vec<Inline>], head: bool, ruled: bool) -> Div {
    div()
        .flex()
        .flex_row()
        .min_w(px(0.))
        .when(ruled, |row| {
            row.border_t_1().border_color(theme::hairline())
        })
        .children(cells.iter().map(|cell| {
            div()
                .flex_1()
                .min_w(px(0.))
                .px_2()
                .py_1()
                .when(head, |cell| cell.font_weight(FontWeight::BOLD))
                .child(inline_text(cell))
        }))
}

/// The inlines of a paragraph as one text, its styles as highlight runs.
fn inline_text(inlines: &[Inline]) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    push_inlines(inlines, Style::default(), &mut text, &mut highlights);
    StyledText::new(text).with_highlights(highlights)
}

/// The styles a run of text is under: the spans it sits in, as a set of flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Style(u8);

impl Style {
    const BOLD: Self = Self(1);
    const ITALIC: Self = Self(1 << 1);
    const STRIKE: Self = Self(1 << 2);
    const CODE: Self = Self(1 << 3);
    const LINK: Self = Self(1 << 4);
    const REDACTED: Self = Self(1 << 5);

    /// These styles and `other` together.
    fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The highlight of a run under these styles, or `None` when it is plain.
    fn highlight(self) -> Option<HighlightStyle> {
        if self == Self::default() {
            return None;
        }
        let mut style = HighlightStyle::default();
        if self.has(Self::BOLD) {
            style.font_weight = Some(FontWeight::BOLD);
        }
        if self.has(Self::ITALIC) {
            style.font_style = Some(FontStyle::Italic);
        }
        if self.has(Self::STRIKE) {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
        }
        if self.has(Self::CODE) {
            style.background_color = Some(theme::selection());
        }
        if self.has(Self::LINK) {
            style.color = Some(theme::accent());
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            });
        }
        if self.has(Self::REDACTED) {
            style.color = Some(theme::muted());
            style.background_color = Some(theme::selected_row());
        }
        Some(style)
    }
}

/// Appends `inlines` to `text`, and a highlight for each run that has a style.
fn push_inlines(
    inlines: &[Inline],
    style: Style,
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, HighlightStyle)>,
) {
    for inline in inlines {
        match inline {
            Inline::Text(part) => push_run(part, style, text, highlights),
            Inline::Code(part) => push_run(part, style.with(Style::CODE), text, highlights),
            Inline::Strong(inner) => {
                push_inlines(inner, style.with(Style::BOLD), text, highlights);
            }
            Inline::Emph(inner) => {
                push_inlines(inner, style.with(Style::ITALIC), text, highlights);
            }
            Inline::Strike(inner) => {
                push_inlines(inner, style.with(Style::STRIKE), text, highlights);
            }
            Inline::Link { text: inner, .. } => {
                push_inlines(inner, style.with(Style::LINK), text, highlights);
            }
            Inline::SoftBreak => text.push(' '),
            Inline::HardBreak => text.push('\n'),
            Inline::Redacted(name) => {
                let marker = format!("[REDACTED:{name}]");
                push_run(&marker, style.with(Style::REDACTED), text, highlights);
            }
        }
    }
}

fn push_run(
    part: &str,
    style: Style,
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, HighlightStyle)>,
) {
    if part.is_empty() {
        return;
    }
    let start = text.len();
    text.push_str(part);
    if let Some(highlight) = style.highlight() {
        highlights.push((start..text.len(), highlight));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_scale_down_to_the_body_size() {
        assert!(heading_scale(1) > heading_scale(2));
        assert!(heading_scale(2) > heading_scale(3));
        assert!((heading_scale(4) - 1.).abs() < f32::EPSILON);
        assert!((heading_scale(6) - 1.).abs() < f32::EPSILON);
    }

    #[test]
    fn plain_runs_have_no_highlight() {
        assert!(Style::default().highlight().is_none());
        assert!(Style::BOLD.with(Style::LINK).highlight().is_some());
    }

    #[test]
    fn runs_join_their_text_with_ordered_highlights() {
        let inlines = vec![
            Inline::Text("a ".to_owned()),
            Inline::Strong(vec![Inline::Text("bold".to_owned())]),
            Inline::SoftBreak,
            Inline::Redacted("k".to_owned()),
        ];
        let mut text = String::new();
        let mut highlights = Vec::new();
        push_inlines(&inlines, Style::default(), &mut text, &mut highlights);
        assert_eq!(text, "a bold [REDACTED:k]");
        let ranges: Vec<Range<usize>> = highlights.into_iter().map(|(range, _)| range).collect();
        assert_eq!(ranges, vec![2..6, 7..19]);
    }

    #[test]
    fn cache_keeps_one_parse_per_text_and_resets_per_thread() {
        let mut cache = MarkdownCache::default();
        cache.for_thread("t1");
        let first = cache.get("hello", 400);
        let again = cache.get("hello", 400);
        assert!(Rc::ptr_eq(&first, &again));
        cache.for_thread("t2");
        assert!(cache.parsed.is_empty());
    }

    #[test]
    fn cache_starts_again_past_its_size() {
        let mut cache = MarkdownCache::default();
        cache.for_thread("t1");
        for index in 0..=CACHE_ENTRIES {
            cache.get(&format!("text {index}"), 400);
        }
        assert!(cache.parsed.len() <= CACHE_ENTRIES);
    }

    #[test]
    fn a_long_text_keeps_its_first_lines_and_counts_the_rest() {
        let mut cache = MarkdownCache::default();
        cache.for_thread("t1");
        let text = (0..10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = cache.get(&text, 4);
        assert_eq!(parsed.hidden, 6);
    }
}
