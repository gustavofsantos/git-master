//! Fits a long branch name into a row of fixed width. The name is split in two:
//! a head, which shrinks and ends in an ellipsis when the row is narrow, and a
//! tail of the last `TAIL_CHARS` characters, which never shrinks. A ticket name
//! like `feature/PROJ-12345-some-very-long-description-of-the-ticket` comes out as
//! `feature/PROJ-12345-some-very-lo…of-the-ticket`: the prefix and the
//! distinguishing end both stay readable.
//!
//! Why a split and not a computed string: GPUI cannot measure text cheaply in a
//! render, so a character budget would be a guess at the row's width. The head
//! takes whatever width the row really has, and the tail is a fixed count of
//! characters that is always shown. The pure functions here are tested without
//! a window; `name_label` is the thin element that draws the split.

use std::ops::Range;

use gpui::{Div, FontWeight, HighlightStyle, StyledText, div, prelude::*, px};

use crate::theme;

/// Characters of a name that always show, after the ellipsis of the head.
pub const TAIL_CHARS: usize = 16;

/// Names longer than this may be cut in the narrowest row, the sidebar, so they
/// get the full name on hover. A shorter name fits every row the app draws.
pub const HOVER_CHARS: usize = 24;

/// True when `name` is long enough to be cut somewhere, so it needs a tooltip.
#[must_use]
pub fn needs_hover(name: &str) -> bool {
    name.chars().count() > HOVER_CHARS
}

/// The byte index where the tail starts. A name of `tail_chars` characters or
/// fewer has no tail, so the index is its length and the head holds all of it.
/// The index is always on a character boundary.
#[must_use]
pub fn tail_start(name: &str, tail_chars: usize) -> usize {
    let count = name.chars().count();
    if count <= tail_chars {
        return name.len();
    }
    name.char_indices()
        .nth(count - tail_chars)
        .map_or(name.len(), |(at, _)| at)
}

/// The matches that fall inside `from..to`, moved so that they index the text of
/// that span. A match that straddles the edge is cut at it.
#[must_use]
pub fn clip_ranges(ranges: &[Range<usize>], from: usize, to: usize) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(from);
            let end = range.end.min(to);
            (start < end).then(|| start - from..end - from)
        })
        .collect()
}

/// A name in a row whose width is the caller's: the head shrinks with an
/// ellipsis, the tail stays whole. `matches` are highlighted in the accent
/// colour, as the palette does. The result grows to take the row's free space,
/// so a sibling placed after it stays at the row's end.
pub fn name_label(name: &str, matches: &[Range<usize>]) -> Div {
    let at = tail_start(name, TAIL_CHARS);
    let head = name_text(&name[..at], &clip_ranges(matches, 0, at));
    let tail = name_text(&name[at..], &clip_ranges(matches, at, name.len()));
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .child(
            div()
                .flex_shrink()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(head),
        )
        .child(div().flex_none().whitespace_nowrap().child(tail))
}

/// A text run with its matches highlighted. Empty text draws nothing.
fn name_text(text: &str, matches: &[Range<usize>]) -> StyledText {
    let highlight = HighlightStyle {
        color: Some(theme::accent()),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..HighlightStyle::default()
    };
    StyledText::new(text.to_owned())
        .with_highlights(matches.iter().map(|range| (range.clone(), highlight)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One match, as the list the functions take.
    fn one(range: Range<usize>) -> Vec<Range<usize>> {
        vec![range]
    }

    /// The part of `name` before and after the tail boundary, for readable asserts.
    fn split(name: &str, tail: usize) -> (&str, &str) {
        let at = tail_start(name, tail);
        (&name[..at], &name[at..])
    }

    #[test]
    fn a_short_name_is_all_head() {
        assert_eq!(split("main", TAIL_CHARS), ("main", ""));
        assert_eq!(split("", TAIL_CHARS), ("", ""));
    }

    #[test]
    fn a_name_exactly_the_tail_long_is_all_head() {
        let name = "abcdefghijklmnop";
        assert_eq!(name.chars().count(), TAIL_CHARS);
        assert_eq!(split(name, TAIL_CHARS), (name, ""));
    }

    #[test]
    fn a_long_name_keeps_its_last_tail_characters() {
        let name = "feature/PROJ-12345-some-very-long-description-of-the-ticket-with-more-words";
        let (head, tail) = split(name, TAIL_CHARS);
        assert_eq!(tail, "-with-more-words");
        assert_eq!(format!("{head}{tail}"), name);
        assert!(head.starts_with("feature/PROJ-12345"));
    }

    #[test]
    fn a_split_never_cuts_a_multibyte_character() {
        // "ç", "ã" and "ú" are two bytes each: a byte cut would panic on slicing.
        let name = "feature/ação-ç-revisão-ñandú-ünïcode-ticket-três-último";
        for tail in 0..=name.chars().count() + 1 {
            let at = tail_start(name, tail);
            assert!(
                name.is_char_boundary(at),
                "tail {tail} cut inside a character"
            );
            // A name no longer than the tail has no tail at all.
            let kept = name[at..].chars().count();
            let expected = if name.chars().count() > tail { tail } else { 0 };
            assert_eq!(kept, expected);
        }
    }

    #[test]
    fn a_name_gets_a_hover_only_past_the_hover_length() {
        assert!(!needs_hover("main"));
        assert!(!needs_hover(&"a".repeat(HOVER_CHARS)));
        assert!(needs_hover(&"a".repeat(HOVER_CHARS + 1)));
        // Multibyte characters count once each, not per byte.
        assert!(!needs_hover(&"ç".repeat(HOVER_CHARS)));
    }

    #[test]
    fn a_tail_of_zero_leaves_the_whole_name_as_head() {
        assert_eq!(split("feature/x", 0), ("feature/x", ""));
    }

    #[test]
    fn a_tail_of_one_is_the_last_character() {
        assert_eq!(split("feature/x", 1), ("feature/", "x"));
    }

    #[test]
    fn a_match_inside_the_head_stays_in_the_head() {
        assert_eq!(clip_ranges(&one(2..5), 0, 10), vec![2..5]);
    }

    #[test]
    fn a_match_in_the_tail_moves_to_index_zero_of_the_tail() {
        assert_eq!(clip_ranges(&one(12..15), 10, 20), vec![2..5]);
    }

    #[test]
    fn a_match_straddling_the_split_is_cut_at_it_on_both_sides() {
        let ranges = one(8..12);
        assert_eq!(clip_ranges(&ranges, 0, 10), vec![8..10]);
        assert_eq!(clip_ranges(&ranges, 10, 20), vec![0..2]);
    }

    #[test]
    fn a_match_outside_the_span_is_dropped() {
        assert!(clip_ranges(&one(1..3), 10, 20).is_empty());
        assert!(clip_ranges(&one(25..30), 10, 20).is_empty());
    }

    #[test]
    fn an_empty_span_keeps_no_match() {
        assert!(clip_ranges(&one(0..4), 3, 3).is_empty());
    }
}
