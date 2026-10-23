// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area_math.rs, MIT, same author.
// Copied into gm-ui from the b-text spike (removed from the tree; see git history: git show 7932c61:spikes/b-text/FINDINGS.md).
//! The pure arithmetic of the multi-line text area: visual lines, heights,
//! scrolling and clicks. Nothing here touches gpui, so the parts that cause
//! jitter when wrong (height clamp, scroll to caret) are unit tested.

use std::ops::Range;

/// One logical line (text between newlines) as the text shaper reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalSpan {
    /// Byte offset of the line in the whole text.
    pub start: usize,
    /// Byte length of the line, without its newline.
    pub len: usize,
    /// Byte offsets, relative to `start`, where each soft-wrapped
    /// continuation row begins.
    pub wraps: Vec<usize>,
}

/// One row on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualLine {
    /// First byte offset shown on the row.
    pub start: usize,
    /// One past the last byte shown on the row. For the last row of a logical
    /// line this is the offset of its newline (or the end of the text).
    pub end: usize,
    /// Index of the logical line the row belongs to.
    pub logical: usize,
    /// Whether this is the last row of its logical line.
    pub last: bool,
}

impl VisualLine {
    /// The furthest offset a pointer or Up/Down may land on. A caret at the
    /// end of a soft-wrapped row would be the start of the next row, so such
    /// a row stops one character short.
    pub fn caret_limit(&self, text: &str) -> usize {
        if self.last {
            return self.end;
        }
        let shown = text.get(self.start..self.end).unwrap_or("");
        let width = shown.chars().next_back().map_or(0, char::len_utf8);
        self.end - width
    }
}

/// Flattens logical lines into rows. A wrap offset that does not advance, or
/// that lies past the line, is ignored so a misbehaving shaper cannot produce
/// an empty or inverted row.
pub fn build_visual_lines(spans: &[LogicalSpan]) -> Vec<VisualLine> {
    let mut rows = Vec::with_capacity(spans.len());
    for (logical, span) in spans.iter().enumerate() {
        let mut row_start = 0;
        for &wrap in &span.wraps {
            if wrap > row_start && wrap <= span.len {
                rows.push(VisualLine {
                    start: span.start + row_start,
                    end: span.start + wrap,
                    logical,
                    last: false,
                });
                row_start = wrap;
            }
        }
        rows.push(VisualLine {
            start: span.start + row_start,
            end: span.start + span.len,
            logical,
            last: true,
        });
    }
    rows
}

/// The row that shows `offset`: the last row starting at or before it, so a
/// caret on a soft-wrap boundary sits at the start of the lower row.
pub fn row_for_offset(rows: &[VisualLine], offset: usize) -> usize {
    rows.partition_point(|row| row.start <= offset)
        .saturating_sub(1)
}

/// The height of `rows` rows. Row counts are far below f32's exact range.
#[allow(clippy::cast_precision_loss)]
pub fn rows_height(rows: usize, line_height: f32) -> f32 {
    rows as f32 * line_height
}

/// The row under a vertical position measured from the top of the content.
/// Positions above or below the content clamp to the first or last row.
pub fn row_at_y(y: f32, line_height: f32, rows: usize) -> usize {
    if rows == 0 || line_height <= 0.0 || y.is_nan() || y <= 0.0 {
        return 0;
    }
    // Truncation is the floor of a non-negative number.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let row = (y / line_height) as usize;
    row.min(rows - 1)
}

/// The row one step up (`up`) or down, or `None` at the edge, where the caret
/// goes to the start or end of the document instead.
pub fn target_row(row: usize, up: bool, rows: usize) -> Option<usize> {
    if up {
        row.checked_sub(1)
    } else {
        (row + 1 < rows).then_some(row + 1)
    }
}

/// How many rows the box shows: the content rows clamped to `min..=max`.
/// A `min` of 0 counts as 1 and a `max` below `min` counts as `min`.
pub fn visible_rows(content_rows: usize, min: usize, max: usize) -> usize {
    let min = min.max(1);
    let max = max.max(min);
    content_rows.clamp(min, max)
}

/// The furthest the content can scroll: its overflow past the viewport.
pub fn max_scroll(content_height: f32, viewport_height: f32) -> f32 {
    (content_height - viewport_height).max(0.0)
}

/// `scroll` kept inside `0..=max_scroll`.
pub fn clamp_scroll(scroll: f32, content_height: f32, viewport_height: f32) -> f32 {
    scroll
        .max(0.0)
        .min(max_scroll(content_height, viewport_height))
}

/// The smallest scroll change that shows the caret row completely. It moves
/// only when the caret is out of view, so typing inside the box never scrolls.
pub fn scroll_to_caret(
    scroll: f32,
    caret_top: f32,
    line_height: f32,
    viewport_height: f32,
    content_height: f32,
) -> f32 {
    let wanted = if viewport_height < line_height || caret_top < scroll {
        caret_top
    } else if caret_top + line_height > scroll + viewport_height {
        caret_top + line_height - viewport_height
    } else {
        scroll
    };
    clamp_scroll(wanted, content_height, viewport_height)
}

/// What a press means, from the number of rapid clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    Caret,
    Word,
    Line,
}

/// Single click places the caret, double selects the word, triple the line;
/// a fourth click starts the cycle again.
pub fn click_kind(click_count: usize) -> ClickKind {
    match click_count.saturating_sub(1) % 3 {
        0 => ClickKind::Caret,
        1 => ClickKind::Word,
        _ => ClickKind::Line,
    }
}

/// The selected part of one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionSegment {
    pub row: usize,
    pub from: usize,
    pub to: usize,
    /// The selection continues past the row's newline, so the highlight is
    /// extended a little to look continuous.
    pub newline: bool,
}

/// Splits a selection into per-row segments.
pub fn selection_segments(
    rows: &[VisualLine],
    selection: &Range<usize>,
    text_len: usize,
) -> Vec<SelectionSegment> {
    if selection.is_empty() {
        return Vec::new();
    }
    rows.iter()
        .enumerate()
        .filter_map(|(row, line)| {
            let from = selection.start.max(line.start);
            let to = selection.end.min(line.end);
            let newline = line.last
                && line.end < text_len
                && selection.start <= line.end
                && selection.end > line.end;
            (from < to || newline).then_some(SelectionSegment {
                row,
                from,
                to: to.max(from),
                newline,
            })
        })
        .collect()
}

/// Converts a UTF-16 offset, the unit the platform IME speaks, into a byte
/// offset. An offset inside a surrogate pair rounds up past that character.
pub fn offset_from_utf16(content: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in content.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += ch.len_utf16();
        utf8 += ch.len_utf8();
    }
    utf8
}

pub fn offset_to_utf16(content: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in content.chars() {
        if utf8 >= offset {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, len: usize, wraps: &[usize]) -> LogicalSpan {
        LogicalSpan {
            start,
            len,
            wraps: wraps.to_vec(),
        }
    }

    // "hello world\n\nend": line 0 wraps at 6, line 1 is empty, line 2 is "end".
    fn sample() -> Vec<VisualLine> {
        build_visual_lines(&[span(0, 11, &[6]), span(12, 0, &[]), span(13, 3, &[])])
    }

    #[test]
    fn rows_follow_wraps_and_newlines() {
        let rows = sample();
        assert_eq!(rows.len(), 4);
        assert_eq!((rows[0].start, rows[0].end, rows[0].last), (0, 6, false));
        assert_eq!((rows[1].start, rows[1].end, rows[1].last), (6, 11, true));
        assert_eq!((rows[2].start, rows[2].end), (12, 12));
        assert_eq!((rows[3].start, rows[3].end), (13, 16));
        assert_eq!(rows[3].logical, 2);
    }

    #[test]
    fn bad_wrap_offsets_are_ignored() {
        let rows = build_visual_lines(&[span(0, 5, &[0, 3, 3, 9])]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].start, rows[0].end), (0, 3));
        assert_eq!((rows[1].start, rows[1].end), (3, 5));
    }

    #[test]
    fn offset_on_a_wrap_boundary_belongs_to_the_lower_row() {
        let rows = sample();
        assert_eq!(row_for_offset(&rows, 0), 0);
        assert_eq!(row_for_offset(&rows, 5), 0);
        assert_eq!(row_for_offset(&rows, 6), 1);
        assert_eq!(row_for_offset(&rows, 11), 1);
        assert_eq!(row_for_offset(&rows, 12), 2);
        assert_eq!(row_for_offset(&rows, 16), 3);
        assert_eq!(row_for_offset(&rows, 99), 3);
        assert_eq!(row_for_offset(&[], 4), 0);
    }

    #[test]
    fn caret_limit_stops_short_of_a_soft_wrap() {
        let text = "hello world\n\nend";
        let rows = sample();
        assert_eq!(rows[0].caret_limit(text), 5);
        assert_eq!(rows[1].caret_limit(text), 11);
        assert_eq!(rows[3].caret_limit(text), 16);
        // A multi-byte last character is stepped over whole.
        let wide = "aé b";
        let row = build_visual_lines(&[span(0, wide.len(), &[3])])[0];
        assert_eq!(row.caret_limit(wide), 1);
    }

    #[test]
    fn row_at_y_clamps_to_the_content() {
        assert_eq!(row_at_y(-5.0, 20.0, 4), 0);
        assert_eq!(row_at_y(0.0, 20.0, 4), 0);
        assert_eq!(row_at_y(19.9, 20.0, 4), 0);
        assert_eq!(row_at_y(20.0, 20.0, 4), 1);
        assert_eq!(row_at_y(79.0, 20.0, 4), 3);
        assert_eq!(row_at_y(500.0, 20.0, 4), 3);
        assert_eq!(row_at_y(30.0, 20.0, 0), 0);
        assert_eq!(row_at_y(f32::NAN, 20.0, 4), 0);
        assert_eq!(row_at_y(30.0, 0.0, 4), 0);
    }

    #[test]
    fn target_row_stops_at_the_edges() {
        assert_eq!(target_row(0, true, 3), None);
        assert_eq!(target_row(2, true, 3), Some(1));
        assert_eq!(target_row(2, false, 3), None);
        assert_eq!(target_row(0, false, 3), Some(1));
        assert_eq!(target_row(0, false, 1), None);
    }

    #[test]
    fn visible_rows_clamps_between_min_and_max() {
        assert_eq!(visible_rows(0, 3, 10), 3);
        assert_eq!(visible_rows(1, 3, 10), 3);
        assert_eq!(visible_rows(3, 3, 10), 3);
        assert_eq!(visible_rows(4, 3, 10), 4);
        assert_eq!(visible_rows(10, 3, 10), 10);
        assert_eq!(visible_rows(11, 3, 10), 10);
        assert_eq!(visible_rows(500, 3, 10), 10);
    }

    #[test]
    fn visible_rows_survives_nonsense_limits() {
        assert_eq!(visible_rows(0, 0, 0), 1);
        assert_eq!(visible_rows(7, 0, 0), 1);
        // max below min: min wins, the box never shrinks under it.
        assert_eq!(visible_rows(2, 4, 2), 4);
        assert_eq!(visible_rows(9, 4, 2), 4);
        assert_eq!(visible_rows(1, 2, 2), 2);
    }

    #[test]
    fn height_is_stable_while_typing_inside_the_clamp() {
        // The box height is a pure function of rows; adding rows below min
        // and past max never changes it, so no jitter at the edges.
        let h = |rows| visible_rows(rows, 2, 8);
        assert_eq!(h(1), h(2));
        assert_eq!(h(8), h(9));
        assert_eq!(h(9), h(40));
        assert_eq!(h(3), 3);
    }

    #[test]
    fn scroll_is_clamped_to_the_overflow() {
        // 12 rows of 20px in a 5-row viewport.
        assert_eq!(max_scroll(240.0, 100.0), 140.0);
        assert_eq!(max_scroll(80.0, 100.0), 0.0);
        assert_eq!(clamp_scroll(-10.0, 240.0, 100.0), 0.0);
        assert_eq!(clamp_scroll(50.0, 240.0, 100.0), 50.0);
        assert_eq!(clamp_scroll(999.0, 240.0, 100.0), 140.0);
        assert_eq!(clamp_scroll(30.0, 80.0, 100.0), 0.0);
    }

    #[test]
    fn caret_inside_the_viewport_never_scrolls() {
        for row in 0..5_usize {
            let top = rows_height(row, 20.0);
            assert_eq!(scroll_to_caret(0.0, top, 20.0, 100.0, 240.0), 0.0);
        }
        assert_eq!(scroll_to_caret(40.0, 60.0, 20.0, 100.0, 240.0), 40.0);
        assert_eq!(scroll_to_caret(40.0, 120.0, 20.0, 100.0, 240.0), 40.0);
    }

    #[test]
    fn caret_below_scrolls_just_enough() {
        // Row 5 starts at 100, one past the 5-row viewport.
        assert_eq!(scroll_to_caret(0.0, 100.0, 20.0, 100.0, 240.0), 20.0);
        assert_eq!(scroll_to_caret(0.0, 220.0, 20.0, 100.0, 240.0), 140.0);
    }

    #[test]
    fn caret_above_scrolls_just_enough() {
        assert_eq!(scroll_to_caret(60.0, 40.0, 20.0, 100.0, 240.0), 40.0);
        assert_eq!(scroll_to_caret(140.0, 0.0, 20.0, 100.0, 240.0), 0.0);
    }

    #[test]
    fn scroll_returns_to_zero_when_content_shrinks() {
        // Text deleted: 3 rows left in a 5-row box, caret on the last.
        assert_eq!(scroll_to_caret(90.0, 40.0, 20.0, 100.0, 60.0), 0.0);
    }

    #[test]
    fn scroll_to_caret_handles_a_viewport_smaller_than_a_row() {
        assert_eq!(scroll_to_caret(0.0, 40.0, 20.0, 10.0, 100.0), 40.0);
    }

    #[test]
    fn clicks_cycle_caret_word_line() {
        assert_eq!(click_kind(0), ClickKind::Caret);
        assert_eq!(click_kind(1), ClickKind::Caret);
        assert_eq!(click_kind(2), ClickKind::Word);
        assert_eq!(click_kind(3), ClickKind::Line);
        assert_eq!(click_kind(4), ClickKind::Caret);
        assert_eq!(click_kind(5), ClickKind::Word);
    }

    #[test]
    fn selection_spanning_rows_gets_one_segment_per_row() {
        let rows = sample();
        let text_len = 16;
        let segments = selection_segments(&rows, &(3..14), text_len);
        let shape: Vec<_> = segments
            .iter()
            .map(|s| (s.row, s.from, s.to, s.newline))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, 3, 6, false),
                (1, 6, 11, true),
                (2, 12, 12, true),
                (3, 13, 14, false),
            ]
        );
    }

    #[test]
    fn selection_ending_at_a_newline_does_not_extend_it() {
        let rows = sample();
        let segments = selection_segments(&rows, &(6..11), 16);
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].newline);
        assert!(selection_segments(&rows, &(4..4), 16).is_empty());
    }

    #[test]
    fn selection_at_the_end_of_text_has_no_newline() {
        let rows = sample();
        let segments = selection_segments(&rows, &(13..16), 16);
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].newline);
    }

    // "a" is 1 byte and 1 UTF-16 unit; "é" is 2 and 1; the emoji is 4 and 2.
    const MIXED: &str = "aé😀b";

    #[test]
    fn utf16_offsets_convert_both_ways() {
        for (utf8, utf16) in [(0, 0), (1, 1), (3, 2), (7, 4), (8, 5)] {
            assert_eq!(offset_to_utf16(MIXED, utf8), utf16);
            assert_eq!(offset_from_utf16(MIXED, utf16), utf8);
        }
    }
}
