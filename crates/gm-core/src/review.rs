//! Review comments (B4, D97): the anchor of a comment and where it is in a
//! file now. Pure: the file text comes in as a string, so the tests need no
//! repository. A match is exact, as whole lines, and only a unique one counts;
//! anything else is outdated, never a guess.

use gm_schema::{ReviewAnchor, ReviewLocation};

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
    });
    match at {
        Some(at) => lines[at..at + needle.len()]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
        None => Vec::new(),
    }
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
