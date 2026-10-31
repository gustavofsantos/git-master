//! Review comments (B4, D97): the anchor of a comment and where it is in a
//! file now. Pure: the file text comes in as a string, so the tests need no
//! repository. A match is exact, as whole lines, and only a unique one counts;
//! anything else is outdated, never a guess.

use std::ops::Range;

use gm_schema::{ReviewAnchor, ReviewChange, ReviewLocation};

/// The most lines one comment may cover.
pub const MAX_SELECTION: usize = 200;
/// The lines kept above and below the selection, for display.
pub const CONTEXT_LINES: usize = 3;

/// The lines of `text`, without their line endings. A final line break does
/// not start another line, so `"a\n"` is one line, and a CRLF file reads the
/// same as its LF copy.
fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// The anchor of the lines `start_line` to `end_line` (1-based, inclusive) of
/// `file_text`. Refused: a selection outside the file, one longer than
/// `MAX_SELECTION` lines, and one with only blank lines.
pub fn anchor_for(file_text: &str, start_line: u32, end_line: u32) -> Result<ReviewAnchor, String> {
    let lines = lines_of(file_text);
    let total = lines.len();
    let outside =
        || format!("lines {start_line}-{end_line} are not in the file, which has {total} lines");
    let first = usize::try_from(start_line).map_err(|_| outside())?;
    let last = usize::try_from(end_line).map_err(|_| outside())?;
    if first == 0 || last < first || last > total {
        return Err(outside());
    }
    if last - first + 1 > MAX_SELECTION {
        return Err(format!(
            "a comment covers at most {MAX_SELECTION} lines, this one covers {}",
            last - first + 1
        ));
    }
    let selected = &lines[first - 1..last];
    if selected.iter().all(|line| line.trim().is_empty()) {
        return Err("the selection has only blank lines".to_owned());
    }
    let above = (first - 1).saturating_sub(CONTEXT_LINES);
    let below = (last + CONTEXT_LINES).min(total);
    Ok(ReviewAnchor {
        text: selected.join("\n"),
        before: lines[above..first - 1]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
        after: lines[last..below]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
        start_line,
    })
}

/// Where `anchor` is in `file_text` now. The commented lines must appear
/// exactly once as a run of whole lines; no match, or more than one, is `None`
/// (outdated). The context lines and the recorded start line never decide.
pub fn locate(file_text: &str, anchor: &ReviewAnchor) -> Option<ReviewLocation> {
    if anchor.text.trim().is_empty() {
        return None;
    }
    let needle: Vec<&str> = anchor.text.split('\n').collect();
    let lines = lines_of(file_text);
    let mut found = None;
    for (at, window) in lines.windows(needle.len()).enumerate() {
        if window == needle.as_slice() {
            if found.is_some() {
                return None;
            }
            found = Some(at);
        }
    }
    let at = found?;
    Some(ReviewLocation {
        start_line: line_number(at)?,
        end_line: line_number(at + needle.len() - 1)?,
    })
}

/// The lines the anchor covered in `file_text`, as a snapshot of the file holds
/// them. The recorded start line is tried first, because the snapshot was taken
/// from the same file; when the text is not there, the anchor must be found
/// exactly once, as `locate` requires. Empty when neither holds.
pub fn snapshot_region(file_text: &str, anchor: &ReviewAnchor) -> Vec<String> {
    let lines = lines_of(file_text);
    snapshot_range(file_text, anchor)
        .map(|range| owned(&lines[range]))
        .unwrap_or_default()
}

/// Where `snapshot_region` finds the anchor, as 0-based indexes of the lines
/// of `file_text`.
fn snapshot_range(file_text: &str, anchor: &ReviewAnchor) -> Option<Range<usize>> {
    let lines = lines_of(file_text);
    let needle: Vec<&str> = anchor.text.split('\n').collect();
    let recorded = usize::try_from(anchor.start_line)
        .ok()
        .and_then(|line| line.checked_sub(1))
        .filter(|&at| lines.get(at..at + needle.len()) == Some(needle.as_slice()));
    let at = recorded.or_else(|| {
        locate(file_text, anchor).and_then(|found| {
            usize::try_from(found.start_line)
                .ok()
                .and_then(|line| line.checked_sub(1))
        })
    })?;
    Some(at..at + needle.len())
}

/// One hunk of a zero-context diff, as its header `@@ -old_start,old_lines
/// +new_start,new_lines @@` gives it. A side with no lines starts after the
/// line it names, as Git writes it: `-4,0` is an insertion after line 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

impl LineHunk {
    /// The lines this hunk replaces, as 0-based indexes of the old file.
    fn old_side(&self) -> Range<usize> {
        span(self.old_start, self.old_lines)
    }

    /// The lines it puts in their place, as 0-based indexes of the new file.
    fn new_side(&self) -> Range<usize> {
        span(self.new_start, self.new_lines)
    }
}

/// The 0-based range of a hunk side that starts at the 1-based `start` and has
/// `count` lines; an empty side sits after line `start`.
fn span(start: u32, count: u32) -> Range<usize> {
    let start = usize::try_from(start).unwrap_or(usize::MAX);
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    let at = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    at..at.saturating_add(count)
}

/// The anchored lines of a comment in its "before" snapshot and the same
/// region in its "after" one (D140). The region is found in `before_text` by
/// `snapshot_region`'s rule, then mapped through `hunks`, the zero-context diff
/// of the file between the two snapshots, in order: a hunk that overlaps the
/// region or touches its edges (it ends on the line above, starts on the line
/// below, or inserts right before or after) widens it to cover the hunk's new
/// side, and hunks above it shift it. `changed` is whether any hunk overlapped
/// or touched it. `after_text` is `None` when the file is gone from the "after"
/// snapshot: the after side is then empty and changed. `None` when the anchor
/// is not in `before_text`.
pub fn review_change(
    before_text: &str,
    after_text: Option<&str>,
    anchor: &ReviewAnchor,
    hunks: &[LineHunk],
) -> Option<ReviewChange> {
    let before_lines = lines_of(before_text);
    let region = snapshot_range(before_text, anchor)?;
    let before = owned(&before_lines[region.clone()]);
    let Some(after_text) = after_text else {
        return Some(ReviewChange {
            before,
            after: Vec::new(),
            changed: true,
        });
    };
    let (range, changed) = after_range(&region, hunks);
    let after_lines = lines_of(after_text);
    let end = range.end.min(after_lines.len());
    let start = range.start.min(end);
    Some(ReviewChange {
        before,
        after: owned(&after_lines[start..end]),
        changed,
    })
}

/// Where the old lines `region` are in the new file, through `hunks`, and
/// whether a hunk overlapped or touched them. See `review_change`.
fn after_range(region: &Range<usize>, hunks: &[LineHunk]) -> (Range<usize>, bool) {
    let touching: Vec<&LineHunk> = hunks
        .iter()
        .filter(|hunk| {
            let old = hunk.old_side();
            old.start <= region.end && old.end >= region.start
        })
        .collect();
    let mut start = map_boundary(region.start, hunks, false);
    let mut end = map_boundary(region.end, hunks, true);
    for hunk in &touching {
        start = start.min(hunk.new_side().start);
        end = end.max(hunk.new_side().end);
    }
    (start..end.max(start), !touching.is_empty())
}

/// Where the boundary before old line `at` (0-based) is in the new file: moved
/// by every hunk wholly before it. Inside a hunk, it is the start of the hunk's
/// new side, or its end when `upper`.
fn map_boundary(at: usize, hunks: &[LineHunk], upper: bool) -> usize {
    let mut shift: isize = 0;
    for hunk in hunks {
        let (old, new) = (hunk.old_side(), hunk.new_side());
        if old.start < at && at < old.end {
            return if upper { new.end } else { new.start };
        }
        if old.end <= at && old.start < at {
            shift += signed(new.len()) - signed(old.len());
        }
    }
    at.checked_add_signed(shift).unwrap_or(0)
}

fn signed(count: usize) -> isize {
    isize::try_from(count).unwrap_or(isize::MAX)
}

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// The lines `anchor` covers in `file_text` now: the run `locate` finds, which
/// must be unique. Empty when the anchor is outdated, so the After side shows
/// nothing rather than a guess.
pub fn current_region(file_text: &str, anchor: &ReviewAnchor) -> Vec<String> {
    let Some(found) = locate(file_text, anchor) else {
        return Vec::new();
    };
    let count = anchor.text.split('\n').count();
    usize::try_from(found.start_line)
        .ok()
        .and_then(|line| line.checked_sub(1))
        .and_then(|at| {
            lines_of(file_text)
                .get(at..at + count)
                .map(<[&str]>::to_vec)
        })
        .unwrap_or_default()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// The 1-based line number of the 0-based index `at`.
fn line_number(at: usize) -> Option<u32> {
    u32::try_from(at + 1).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn file(lines: &[&str]) -> String {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    fn anchored(text: &str, start: u32, end: u32) -> ReviewAnchor {
        anchor_for(text, start, end).expect("a valid selection")
    }

    #[test]
    fn the_anchor_holds_the_selection_and_its_context() {
        let text = file(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
        let anchor = anchored(&text, 4, 5);
        assert_eq!(anchor.text, "d\ne");
        assert_eq!(anchor.before, vec!["a", "b", "c"]);
        assert_eq!(anchor.after, vec!["f", "g", "h"]);
        assert_eq!(anchor.start_line, 4);
    }

    #[test]
    fn context_is_cut_at_the_file_edges() {
        let text = file(&["a", "b", "c"]);
        let first = anchored(&text, 1, 1);
        assert!(first.before.is_empty());
        assert_eq!(first.after, vec!["b", "c"]);
        let last = anchored(&text, 3, 3);
        assert_eq!(last.before, vec!["a", "b"]);
        assert!(last.after.is_empty());
    }

    #[test]
    fn a_selection_outside_the_file_is_refused() {
        let text = file(&["a", "b"]);
        assert!(anchor_for(&text, 0, 1).is_err());
        assert!(anchor_for(&text, 2, 1).is_err());
        assert!(anchor_for(&text, 1, 3).is_err());
        assert!(anchor_for("", 1, 1).is_err());
        assert!(anchor_for(&text, 2, 2).is_ok());
    }

    #[test]
    fn a_selection_of_more_than_the_limit_is_refused() {
        let lines: Vec<String> = (0..=MAX_SELECTION).map(|n| format!("line {n}")).collect();
        let text = format!("{}\n", lines.join("\n"));
        assert!(anchor_for(&text, 1, u32::try_from(MAX_SELECTION).unwrap()).is_ok());
        let too_many = u32::try_from(MAX_SELECTION + 1).unwrap();
        assert!(anchor_for(&text, 1, too_many).is_err());
    }

    #[test]
    fn a_selection_of_blank_lines_is_refused() {
        let text = file(&["a", "", "   ", "b"]);
        assert!(anchor_for(&text, 2, 3).is_err());
        assert!(anchor_for(&text, 2, 2).is_err());
        assert!(anchor_for(&text, 1, 3).is_ok());
    }

    #[test]
    fn the_anchor_locates_at_its_own_range() {
        let text = file(&["one", "two", "three"]);
        let anchor = anchored(&text, 2, 3);
        assert_eq!(
            locate(&text, &anchor),
            Some(ReviewLocation {
                start_line: 2,
                end_line: 3
            })
        );
    }

    #[test]
    fn a_match_is_on_whole_lines_only() {
        let anchor = anchored(&file(&["abc"]), 1, 1);
        assert_eq!(locate(&file(&["xabc", "abcd"]), &anchor), None);
        assert_eq!(
            locate(&file(&["xabc", "abc"]), &anchor),
            Some(ReviewLocation {
                start_line: 2,
                end_line: 2
            })
        );
    }

    #[test]
    fn a_text_that_is_not_in_the_file_is_outdated() {
        let anchor = anchored(&file(&["one", "two"]), 1, 2);
        assert_eq!(locate(&file(&["one", "three"]), &anchor), None);
    }

    #[test]
    fn a_text_that_appears_twice_is_outdated_whatever_the_context() {
        let anchor = anchored(&file(&["x", "}", "y", "}", "z"]), 2, 2);
        assert_eq!(anchor.before, vec!["x"]);
        assert_eq!(locate(&file(&["x", "}", "y", "}", "z"]), &anchor), None);
    }

    #[test]
    fn overlapping_occurrences_count_as_several() {
        let anchor = anchored(&file(&["a", "a"]), 1, 2);
        assert_eq!(anchor.text, "a\na");
        assert_eq!(locate(&file(&["a", "a", "a"]), &anchor), None);
    }

    #[test]
    fn crlf_and_lf_files_are_the_same_text() {
        let lf = file(&["one", "two", "three"]);
        let crlf = "one\r\ntwo\r\nthree\r\n";
        let anchor = anchored(&lf, 2, 2);
        assert_eq!(anchor.text, "two");
        assert_eq!(
            locate(crlf, &anchored(crlf, 2, 2)),
            Some(ReviewLocation {
                start_line: 2,
                end_line: 2
            })
        );
        assert_eq!(
            locate(crlf, &anchor),
            Some(ReviewLocation {
                start_line: 2,
                end_line: 2
            })
        );
    }

    #[test]
    fn a_file_without_a_final_line_break_has_the_same_lines() {
        let with = file(&["one", "two"]);
        let without = "one\ntwo";
        let anchor = anchored(without, 2, 2);
        assert_eq!(anchor.text, "two");
        assert_eq!(anchor, anchored(&with, 2, 2));
        assert_eq!(
            locate(without, &anchor),
            Some(ReviewLocation {
                start_line: 2,
                end_line: 2
            })
        );
    }

    #[test]
    fn a_trailing_empty_line_is_a_line() {
        let text = "one\n\n";
        assert!(anchor_for(text, 2, 2).is_err());
        assert!(anchor_for(text, 1, 2).is_ok());
        assert!(anchor_for("\n", 1, 1).is_err());
    }

    #[test]
    fn an_empty_or_blank_anchor_locates_nothing() {
        let anchor = ReviewAnchor {
            text: String::new(),
            before: Vec::new(),
            after: Vec::new(),
            start_line: 1,
        };
        assert_eq!(locate(&file(&["", "x"]), &anchor), None);
    }

    #[test]
    fn the_snapshot_region_is_the_commented_lines_of_the_snapshot() {
        let text = file(&["x", "}", "y", "}", "z"]);
        let anchor = anchored(&text, 4, 4);
        assert_eq!(snapshot_region(&text, &anchor), vec!["}"]);
        // A recorded line that is ambiguous is not guessed at.
        assert!(snapshot_region(&file(&["new", "x", "}", "y", "}", "z"]), &anchor).is_empty());
        assert!(snapshot_region(&file(&["gone"]), &anchor).is_empty());
    }

    #[test]
    fn the_snapshot_region_prefers_the_recorded_line_when_it_still_matches() {
        let anchor = anchored(&file(&["x", "b", "y", "b"]), 4, 4);
        let snapshot = file(&["b", "x", "b", "b"]);
        assert_eq!(snapshot_region(&snapshot, &anchor), vec!["b"]);
    }

    #[test]
    fn the_snapshot_region_falls_back_to_a_unique_match() {
        let anchor = anchored(&file(&["a", "b", "c"]), 2, 3);
        let moved = file(&["n", "n", "n", "b", "c"]);
        assert_eq!(snapshot_region(&moved, &anchor), vec!["b", "c"]);
    }

    #[test]
    fn the_current_region_is_the_commented_lines_when_they_are_found_once() {
        let anchor = anchored(&file(&["a", "b", "c"]), 2, 3);
        assert_eq!(
            current_region(&file(&["n", "b", "c"]), &anchor),
            vec!["b", "c"]
        );
        assert!(current_region(&file(&["b", "c", "x", "b", "c"]), &anchor).is_empty());
        assert!(current_region(&file(&["b", "x"]), &anchor).is_empty());
    }

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> LineHunk {
        LineHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        }
    }

    fn strings(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    /// The change of the anchor on lines 3-4 ("c", "d") of "a".."g", once the
    /// file became `after` through `hunks`.
    fn change_of(after: &[&str], hunks: &[LineHunk]) -> ReviewChange {
        let before = file(&["a", "b", "c", "d", "e", "f", "g"]);
        let anchor = anchored(&before, 3, 4);
        review_change(&before, Some(&file(after)), &anchor, hunks).expect("a change")
    }

    #[test]
    fn with_no_hunks_the_after_side_is_the_before_side() {
        let change = change_of(&["a", "b", "c", "d", "e", "f", "g"], &[]);
        assert_eq!(change.before, strings(&["c", "d"]));
        assert_eq!(change.after, strings(&["c", "d"]));
        assert!(!change.changed);
    }

    #[test]
    fn an_edit_inside_the_region_shows_on_the_after_side() {
        let change = change_of(&["a", "b", "C", "d", "e", "f", "g"], &[hunk(3, 1, 3, 1)]);
        assert_eq!(change.before, strings(&["c", "d"]));
        assert_eq!(change.after, strings(&["C", "d"]));
        assert!(change.changed);
    }

    #[test]
    fn an_edit_that_crosses_an_edge_widens_the_region_to_cover_it() {
        // "d" and "e" became "D", "E", "x".
        let change = change_of(
            &["a", "b", "c", "D", "E", "x", "f", "g"],
            &[hunk(4, 2, 4, 3)],
        );
        assert_eq!(change.after, strings(&["c", "D", "E", "x"]));
        assert!(change.changed);
        // "a" to "c" became "z": the region starts where the hunk does.
        let change = change_of(&["z", "d", "e", "f", "g"], &[hunk(1, 3, 1, 1)]);
        assert_eq!(change.after, strings(&["z", "d"]));
    }

    #[test]
    fn edits_on_the_lines_next_to_the_region_touch_it() {
        let above = change_of(&["a", "B", "c", "d", "e", "f", "g"], &[hunk(2, 1, 2, 1)]);
        assert_eq!(above.after, strings(&["B", "c", "d"]));
        assert!(above.changed);
        let below = change_of(&["a", "b", "c", "d", "E", "f", "g"], &[hunk(5, 1, 5, 1)]);
        assert_eq!(below.after, strings(&["c", "d", "E"]));
        assert!(below.changed);
    }

    #[test]
    fn insertions_right_before_or_after_the_region_touch_it() {
        let before = change_of(
            &["a", "b", "new", "c", "d", "e", "f", "g"],
            &[hunk(2, 0, 3, 1)],
        );
        assert_eq!(before.after, strings(&["new", "c", "d"]));
        assert!(before.changed);
        let after = change_of(
            &["a", "b", "c", "d", "new", "e", "f", "g"],
            &[hunk(4, 0, 5, 1)],
        );
        assert_eq!(after.after, strings(&["c", "d", "new"]));
        assert!(after.changed);
    }

    #[test]
    fn edits_away_from_the_region_move_it_or_leave_it() {
        let above = change_of(
            &["x", "y", "A", "b", "c", "d", "e", "f"],
            &[hunk(1, 1, 1, 3), hunk(7, 1, 8, 0)],
        );
        assert_eq!(above.after, strings(&["c", "d"]));
        assert!(!above.changed);
        let below = change_of(&["a", "b", "c", "d", "e", "F", "g"], &[hunk(6, 1, 6, 1)]);
        assert_eq!(below.after, strings(&["c", "d"]));
        assert!(!below.changed);
    }

    #[test]
    fn a_deleted_region_has_an_empty_after_side() {
        let change = change_of(&["a", "b", "e", "f", "g"], &[hunk(3, 2, 2, 0)]);
        assert!(change.after.is_empty());
        assert!(change.changed);
    }

    #[test]
    fn a_file_gone_from_the_after_snapshot_has_an_empty_after_side() {
        let before = file(&["a", "b", "c"]);
        let anchor = anchored(&before, 2, 2);
        let change = review_change(&before, None, &anchor, &[]).expect("a change");
        assert_eq!(change.before, strings(&["b"]));
        assert!(change.after.is_empty());
        assert!(change.changed);
    }

    #[test]
    fn an_anchor_missing_from_the_before_snapshot_has_no_change() {
        let anchor = anchored(&file(&["a", "b"]), 2, 2);
        assert_eq!(
            review_change(&file(&["x"]), Some(&file(&["x"])), &anchor, &[]),
            None
        );
    }

    /// Lines that are unique in a file: each starts with its own index.
    fn unique_lines(words: &[String]) -> Vec<String> {
        words
            .iter()
            .enumerate()
            .map(|(n, word)| format!("{n}: {word}"))
            .collect()
    }

    proptest! {
        #[test]
        fn an_anchor_locates_at_its_own_range_when_its_text_is_unique(
            words in prop::collection::vec("[a-z ]{0,12}", 1..60),
            pick in (0usize..60, 0usize..60),
            crlf in any::<bool>(),
            trailing in any::<bool>(),
        ) {
            let lines = unique_lines(&words);
            let mut text = lines.join(if crlf { "\r\n" } else { "\n" });
            if trailing {
                text.push_str(if crlf { "\r\n" } else { "\n" });
            }
            let first = pick.0 % lines.len();
            let last = first + pick.1 % (lines.len() - first);
            let (start, end) = (first + 1, last + 1);
            let range = (u32::try_from(start).unwrap(), u32::try_from(end).unwrap());
            match anchor_for(&text, range.0, range.1) {
                // A selection of blank lines is refused; the lines here are never blank.
                Ok(anchor) => {
                    prop_assert_eq!(
                        locate(&text, &anchor),
                        Some(ReviewLocation { start_line: range.0, end_line: range.1 })
                    );
                }
                Err(_) => prop_assert!(lines[first..=last].iter().all(|l| l.trim().is_empty())),
            }
        }

        #[test]
        fn inserting_lines_moves_the_location_by_the_inserted_count(
            words in prop::collection::vec("[a-z]{1,8}", 2..40),
            pick in (0usize..40, 0usize..40),
            inserted in 1usize..6,
            at in 0usize..40,
        ) {
            let lines = unique_lines(&words);
            let first = pick.0 % lines.len();
            let last = first + pick.1 % (lines.len() - first);
            let text = file(&lines.iter().map(String::as_str).collect::<Vec<_>>());
            let anchor = anchored(&text, u32::try_from(first + 1).unwrap(), u32::try_from(last + 1).unwrap());
            // Insert before the range: it moves down. After it: it stays. Inside it: the text is broken.
            let at = at % (lines.len() + 1);
            let mut edited = lines.clone();
            for n in 0..inserted {
                edited.insert(at, format!("inserted {n}"));
            }
            let text = file(&edited.iter().map(String::as_str).collect::<Vec<_>>());
            let shift = u32::try_from(inserted).unwrap();
            let start = u32::try_from(first + 1).unwrap();
            let end = u32::try_from(last + 1).unwrap();
            let moved = locate(&text, &anchor);
            if at <= first {
                prop_assert_eq!(moved, Some(ReviewLocation {
                    start_line: start + shift,
                    end_line: end + shift,
                }));
            } else if at > last {
                prop_assert_eq!(moved, Some(ReviewLocation { start_line: start, end_line: end }));
            } else {
                prop_assert_eq!(moved, None);
            }
        }

        #[test]
        fn one_edit_maps_the_region_to_the_lines_it_became(
            words in prop::collection::vec("[a-z]{1,8}", 1..40),
            pick in (0usize..40, 0usize..40),
            edit in (0usize..41, 0usize..41),
            inserted in 0usize..5,
        ) {
            let lines = unique_lines(&words);
            let first = pick.0 % lines.len();
            let last = first + pick.1 % (lines.len() - first);
            let text = file(&lines.iter().map(String::as_str).collect::<Vec<_>>());
            let anchor = anchored(&text, u32::try_from(first + 1).unwrap(), u32::try_from(last + 1).unwrap());
            // Replace the lines [from, to) with `inserted` new ones, as one hunk.
            let from = edit.0 % (lines.len() + 1);
            let to = from + edit.1 % (lines.len() - from + 1);
            let mut edited = lines[..from].to_vec();
            edited.extend((0..inserted).map(|n| format!("new {n}")));
            edited.extend_from_slice(&lines[to..]);
            let after = file(&edited.iter().map(String::as_str).collect::<Vec<_>>());
            let side = |at: usize, count: usize| {
                let start = if count == 0 { at } else { at + 1 };
                u32::try_from(start).unwrap()
            };
            let hunks = if to == from && inserted == 0 {
                Vec::new()
            } else {
                vec![LineHunk {
                    old_start: side(from, to - from),
                    old_lines: u32::try_from(to - from).unwrap(),
                    new_start: side(from, inserted),
                    new_lines: u32::try_from(inserted).unwrap(),
                }]
            };
            let change = review_change(&text, Some(&after), &anchor, &hunks).unwrap();
            prop_assert_eq!(&change.before, &lines[first..=last].to_vec());
            let (start, end) = (first, last + 1);
            if hunks.is_empty() || to < start || from > end {
                // Away from the region: the same lines, wherever they moved.
                prop_assert!(!change.changed);
                prop_assert_eq!(&change.after, &change.before);
            } else {
                // Overlapping or touching: the region and the edit, as they are now.
                prop_assert!(change.changed);
                let lo = start.min(from);
                let hi = end.max(to) + inserted - (to - from);
                prop_assert_eq!(&change.after, &edited[lo..hi].to_vec());
            }
        }

        #[test]
        fn duplicating_the_commented_lines_makes_the_anchor_outdated(
            words in prop::collection::vec("[a-z]{1,8}", 2..40),
            pick in (0usize..40, 0usize..40),
        ) {
            let lines = unique_lines(&words);
            let first = pick.0 % lines.len();
            let last = first + pick.1 % (lines.len() - first);
            let text = file(&lines.iter().map(String::as_str).collect::<Vec<_>>());
            let anchor = anchored(&text, u32::try_from(first + 1).unwrap(), u32::try_from(last + 1).unwrap());
            let mut edited = lines.clone();
            let copy: Vec<String> = lines[first..=last].to_vec();
            let at = (last + 1).min(edited.len());
            for (n, line) in copy.into_iter().enumerate() {
                edited.insert(at + n, line);
            }
            let text = file(&edited.iter().map(String::as_str).collect::<Vec<_>>());
            prop_assert_eq!(locate(&text, &anchor), None);
        }
    }
}
