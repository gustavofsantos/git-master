//! Pure line-selection math behind stage and unstage (D24).
//!
//! Content is split into lines on `\n` only, so a `\r` stays part of its line
//! and CRLF survives. Only the last line of a file may lack its terminator; a
//! `DiffLine` marks that line with `no_newline`.

use gm_schema::{DiffLine, Hunk, LineKind};

const MISMATCH: &str = "hunk does not match the index";

/// Whether `content` ends with `\n`. Empty content has no lines and no newline.
pub fn has_final_newline(content: &[u8]) -> bool {
    content.last() == Some(&b'\n')
}

/// Splits `content` into lines without their `\n`. Empty content has no lines.
/// Pair the result with `has_final_newline` to rebuild the exact bytes.
pub fn split_lines(content: &[u8]) -> Vec<Vec<u8>> {
    if content.is_empty() {
        return Vec::new();
    }
    let body = content.strip_suffix(b"\n").unwrap_or(content);
    body.split(|&byte| byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect()
}

/// Joins lines with `\n`. The last line gets a terminator only when
/// `final_newline` is set, so `join_lines(&split_lines(c), has_final_newline(c))`
/// returns `c` for any bytes `c`.
pub fn join_lines(lines: &[Vec<u8>], final_newline: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        out.extend_from_slice(line);
        if i + 1 < lines.len() || final_newline {
            out.push(b'\n');
        }
    }
    out
}

/// Stages the selected added and removed lines of `hunk` onto `index`.
///
/// `index` is the current index blob (the old side of an unstaged diff) and
/// `hunk` is one hunk of that diff. Context lines are kept. A removed line is
/// dropped when selected and kept otherwise. An added line is inserted when
/// selected and skipped otherwise. Lines outside the hunk are untouched.
///
/// # Errors
///
/// Fails when a selected index is out of range, repeated, or a context line,
/// or when the hunk's old side does not match `index`.
pub fn stage_selection(index: &[u8], hunk: &Hunk, selected: &[u32]) -> Result<Vec<u8>, String> {
    rewrite(index, hunk, selected, Side::Old)
}

/// Stages every added and removed line of `hunk` onto `index`.
///
/// # Errors
///
/// Fails when the hunk's old side does not match `index`.
pub fn stage_whole_hunk(index: &[u8], hunk: &Hunk) -> Result<Vec<u8>, String> {
    stage_selection(index, hunk, &change_indices(hunk, |_| true))
}

/// Reverts the selected changes of `hunk` in `index`.
///
/// `hunk` is one hunk of the staged diff (HEAD against index) and `index` is
/// the current index blob, its new side. A selected added line is removed from
/// the index. A selected removed line is re-inserted at its place. Unselected
/// changes stay.
///
/// # Errors
///
/// Fails when a selected index is out of range, repeated, or a context line,
/// or when the hunk's new side does not match `index`.
pub fn unstage_selection(index: &[u8], hunk: &Hunk, selected: &[u32]) -> Result<Vec<u8>, String> {
    rewrite(index, hunk, selected, Side::New)
}

/// Pairs `(removed, added)` of adjacent `-`/`+` lines inside one change block
/// where exactly one of the two is selected. The i-th removed line pairs with
/// the i-th added line of the block. The result is ordered by removed index.
pub fn pair_warning(hunk: &Hunk, selected: &[u32]) -> Vec<(u32, u32)> {
    let is_selected = |i: usize| u32::try_from(i).is_ok_and(|i| selected.contains(&i));
    let lines = &hunk.lines;
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        let removed_start = i;
        let added_start = run_end(lines, removed_start, LineKind::Removed);
        let added_end = run_end(lines, added_start, LineKind::Added);
        let pairable = (added_start - removed_start).min(added_end - added_start);
        for k in 0..pairable {
            let removed = removed_start + k;
            let added = added_start + k;
            if is_selected(removed) != is_selected(added) {
                pairs.push((index_u32(removed), index_u32(added)));
            }
        }
        i = added_end;
    }
    pairs
}

/// Which comparison a rewrite works on. The blob being rewritten is the old
/// side for staging and the new side for unstaging.
#[derive(Clone, Copy)]
enum Side {
    Old,
    New,
}

impl Side {
    /// The change kind present in the blob being rewritten.
    fn changed_kind(self) -> LineKind {
        match self {
            Side::Old => LineKind::Removed,
            Side::New => LineKind::Added,
        }
    }

    /// First line and line count of this side's span in the hunk header.
    fn span(self, hunk: &Hunk) -> (u32, u32) {
        match self {
            Side::Old => (hunk.old_start, hunk.old_lines),
            Side::New => (hunk.new_start, hunk.new_lines),
        }
    }
}

/// Rewrites `blob` so the hunk's selected changes are applied on `side`.
fn rewrite(blob: &[u8], hunk: &Hunk, selected: &[u32], side: Side) -> Result<Vec<u8>, String> {
    let mask = selection_mask(hunk, selected)?;
    let lines = split_lines(blob);
    let final_newline = has_final_newline(blob);
    let (start, count) = side.span(hunk);
    let (start, count) = (to_usize(start), to_usize(count));

    // A zero-length span names the line it follows (0 for the start of the
    // file), so it inserts after `start` lines instead of replacing one.
    let first = if count == 0 {
        start
    } else {
        start.checked_sub(1).ok_or(MISMATCH)?
    };

    let span_lines: Vec<&DiffLine> = hunk
        .lines
        .iter()
        .filter(|line| line.kind == LineKind::Context || line.kind == side.changed_kind())
        .collect();
    if span_lines.len() != count {
        return Err(MISMATCH.to_owned());
    }
    for (offset, line) in span_lines.iter().enumerate() {
        let pos = first + offset;
        let matches = lines.get(pos).is_some_and(|text| {
            text.as_slice() == line.text.as_bytes()
                && line.no_newline == no_newline_at(pos, lines.len(), final_newline)
        });
        if !matches {
            return Err(MISMATCH.to_owned());
        }
    }

    let mut out: Vec<(Vec<u8>, bool)> = Vec::with_capacity(lines.len());
    for (pos, text) in lines.iter().enumerate().take(first) {
        out.push((text.clone(), no_newline_at(pos, lines.len(), final_newline)));
    }
    for (i, line) in hunk.lines.iter().enumerate() {
        let keep = match line.kind {
            LineKind::Context => true,
            kind if kind == side.changed_kind() => !mask[i],
            _ => mask[i],
        };
        if keep {
            out.push((line.text.as_bytes().to_vec(), line.no_newline));
        }
    }
    for (pos, text) in lines.iter().enumerate().skip(first + count) {
        out.push((text.clone(), no_newline_at(pos, lines.len(), final_newline)));
    }

    let final_newline = out.last().is_some_and(|(_, no_newline)| !no_newline);
    let texts: Vec<Vec<u8>> = out.into_iter().map(|(text, _)| text).collect();
    Ok(join_lines(&texts, final_newline))
}

/// Whether the line at `pos` of a blob lacks its terminator.
fn no_newline_at(pos: usize, len: usize, final_newline: bool) -> bool {
    pos + 1 == len && !final_newline
}

/// One flag per hunk line, set for each selected index. Rejects indices that
/// are out of range, repeated, or point at a context line.
fn selection_mask(hunk: &Hunk, selected: &[u32]) -> Result<Vec<bool>, String> {
    let mut mask = vec![false; hunk.lines.len()];
    for &index in selected {
        let i = to_usize(index);
        let Some(line) = hunk.lines.get(i) else {
            return Err(format!(
                "selected line {index} is out of range for a hunk of {} lines",
                hunk.lines.len()
            ));
        };
        if line.kind == LineKind::Context {
            return Err(format!("selected line {index} is a context line"));
        }
        if mask[i] {
            return Err(format!("selected line {index} is repeated"));
        }
        mask[i] = true;
    }
    Ok(mask)
}

/// Indices of the added and removed lines of `hunk` that satisfy `keep`.
fn change_indices(hunk: &Hunk, keep: impl Fn(usize) -> bool) -> Vec<u32> {
    hunk.lines
        .iter()
        .enumerate()
        .filter(|(i, line)| line.kind != LineKind::Context && keep(*i))
        .map(|(i, _)| index_u32(i))
        .collect()
}

/// Index of the first line at or after `start` that is not of `kind`.
fn run_end(lines: &[DiffLine], start: usize, kind: LineKind) -> usize {
    start
        + lines
            .iter()
            .skip(start)
            .take_while(|line| line.kind == kind)
            .count()
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

fn index_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::HunkId;
    use proptest::prelude::*;

    fn dl(kind: LineKind, text: &str, no_newline: bool) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.to_owned(),
            no_newline,
        }
    }

    fn ctx(text: &str) -> DiffLine {
        dl(LineKind::Context, text, false)
    }

    fn rem(text: &str) -> DiffLine {
        dl(LineKind::Removed, text, false)
    }

    fn add(text: &str) -> DiffLine {
        dl(LineKind::Added, text, false)
    }

    fn hunk_of(lines: Vec<DiffLine>) -> Hunk {
        let old_lines = lines.iter().filter(|l| l.kind != LineKind::Added).count();
        let new_lines = lines.iter().filter(|l| l.kind != LineKind::Removed).count();
        let span = |n: u32| u32::from(n != 0);
        let old_lines = u32::try_from(old_lines).unwrap_or(u32::MAX);
        let new_lines = u32::try_from(new_lines).unwrap_or(u32::MAX);
        Hunk {
            id: HunkId("test".to_owned()),
            old_start: span(old_lines),
            old_lines,
            new_start: span(new_lines),
            new_lines,
            header: String::new(),
            lines,
        }
    }

    fn all_changes(hunk: &Hunk) -> Vec<u32> {
        change_indices(hunk, |_| true)
    }

    /// Lines of `content` as text, for readable assertions.
    fn text(content: &[u8]) -> String {
        String::from_utf8_lossy(content).into_owned()
    }

    #[test]
    fn split_and_join_round_trip_exact_bytes() {
        let cases: [&[u8]; 9] = [
            b"",
            b"\n",
            b"\n\n",
            b"a",
            b"a\n",
            b"a\n\n",
            b"a\r\nb\r\n",
            b"a\r\nb",
            b"\r",
        ];
        for content in cases {
            let lines = split_lines(content);
            assert_eq!(
                join_lines(&lines, has_final_newline(content)),
                content,
                "round trip of {content:?}"
            );
        }
    }

    #[test]
    fn split_drops_only_the_final_terminator() {
        assert!(split_lines(b"").is_empty());
        assert_eq!(split_lines(b"\n"), vec![b"".to_vec()]);
        assert_eq!(split_lines(b"a\n\n"), vec![b"a".to_vec(), b"".to_vec()]);
        assert_eq!(
            split_lines(b"a\r\nb\r\n"),
            vec![b"a\r".to_vec(), b"b\r".to_vec()]
        );
    }

    #[test]
    fn stage_selected_added_line_only() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B"), add("X"), ctx("c")]);
        let out = stage_selection(b"a\nb\nc\n", &hunk, &[2]).unwrap();
        assert_eq!(out, b"a\nb\nB\nc\n");
    }

    #[test]
    fn stage_selected_removed_line_only() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B"), add("X"), ctx("c")]);
        let out = stage_selection(b"a\nb\nc\n", &hunk, &[1]).unwrap();
        assert_eq!(out, b"a\nc\n");
    }

    #[test]
    fn stage_nothing_returns_index_byte_identical() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B"), ctx("c")]);
        let index = b"a\nb\nc\n";
        assert_eq!(stage_selection(index, &hunk, &[]).unwrap(), index);
    }

    #[test]
    fn stage_everything_equals_applying_the_hunk() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B"), add("X"), ctx("c")]);
        let out = stage_selection(b"a\nb\nc\n", &hunk, &all_changes(&hunk)).unwrap();
        assert_eq!(out, b"a\nB\nX\nc\n");
        assert_eq!(stage_whole_hunk(b"a\nb\nc\n", &hunk).unwrap(), out);
    }

    #[test]
    fn stage_inserts_into_empty_index_with_zero_old_span() {
        let hunk = Hunk {
            id: HunkId("new".to_owned()),
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: 2,
            header: String::new(),
            lines: vec![add("x"), add("y")],
        };
        assert_eq!(stage_selection(b"", &hunk, &[0]).unwrap(), b"x\n");
        assert_eq!(stage_whole_hunk(b"", &hunk).unwrap(), b"x\ny\n");
    }

    #[test]
    fn stage_inserts_after_a_zero_length_span_in_the_middle() {
        let hunk = Hunk {
            id: HunkId("ins".to_owned()),
            old_start: 2,
            old_lines: 0,
            new_start: 3,
            new_lines: 1,
            header: String::new(),
            lines: vec![add("new")],
        };
        assert_eq!(
            stage_whole_hunk(b"a\nb\nc\n", &hunk).unwrap(),
            b"a\nb\nnew\nc\n"
        );
    }

    #[test]
    fn stage_rejects_index_mismatch() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B")]);
        let err = stage_selection(b"a\nz\n", &hunk, &[]).unwrap_err();
        assert_eq!(err, MISMATCH);
        let err = stage_selection(b"", &hunk, &[]).unwrap_err();
        assert_eq!(err, MISMATCH);
    }

    #[test]
    fn selection_errors_name_the_problem() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B")]);
        let index = b"a\nb\n";
        let out_of_range = stage_selection(index, &hunk, &[3]).unwrap_err();
        assert!(out_of_range.contains("out of range"), "{out_of_range}");
        let context = stage_selection(index, &hunk, &[0]).unwrap_err();
        assert!(context.contains("context line"), "{context}");
        let repeated = stage_selection(index, &hunk, &[1, 1]).unwrap_err();
        assert!(repeated.contains("repeated"), "{repeated}");
    }

    #[test]
    fn stage_respects_lines_outside_the_hunk() {
        let hunk = Hunk {
            id: HunkId("mid".to_owned()),
            old_start: 2,
            old_lines: 1,
            new_start: 2,
            new_lines: 2,
            header: String::new(),
            lines: vec![rem("b"), add("B"), add("B2")],
        };
        let out = stage_whole_hunk(b"a\nb\nc\nd\n", &hunk).unwrap();
        assert_eq!(out, b"a\nB\nB2\nc\nd\n");
    }

    #[test]
    fn crlf_lines_keep_their_carriage_return() {
        let hunk = hunk_of(vec![ctx("a\r"), rem("b\r"), add("B\r")]);
        let index = b"a\r\nb\r\n";
        assert_eq!(stage_selection(index, &hunk, &[]).unwrap(), index);
        assert_eq!(stage_whole_hunk(index, &hunk).unwrap(), b"a\r\nB\r\n");
        assert_eq!(
            stage_selection(index, &hunk, &[2]).unwrap(),
            b"a\r\nb\r\nB\r\n"
        );
    }

    #[test]
    fn staging_only_the_added_last_line_of_a_file_without_final_newline() {
        // Index "a" (no final newline) becomes "a\nb" (no final newline).
        let hunk = Hunk {
            id: HunkId("eof".to_owned()),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            header: String::new(),
            lines: vec![
                dl(LineKind::Removed, "a", true),
                add("a"),
                dl(LineKind::Added, "b", true),
            ],
        };
        // Selecting only the added `b`: the removed `a` stays, the added `a` is
        // skipped, and the result ends where `b` ends.
        assert_eq!(stage_selection(b"a", &hunk, &[2]).unwrap(), b"a\nb");
        // Selecting nothing keeps the index byte-identical.
        assert_eq!(stage_selection(b"a", &hunk, &[]).unwrap(), b"a");
        // Selecting everything gives the new file.
        assert_eq!(stage_whole_hunk(b"a", &hunk).unwrap(), b"a\nb");
    }

    #[test]
    fn staging_a_removed_unterminated_line_drops_the_final_newline() {
        // Index "a\nb" (no final newline), new "a\nb\nc\n": the old `b` is replaced.
        let hunk = Hunk {
            id: HunkId("tail".to_owned()),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 3,
            header: String::new(),
            lines: vec![
                ctx("a"),
                dl(LineKind::Removed, "b", true),
                add("b"),
                add("c"),
            ],
        };
        // Removed `b` kept, added `b` skipped, added `c` taken: `c` is last and terminated.
        assert_eq!(stage_selection(b"a\nb", &hunk, &[3]).unwrap(), b"a\nb\nc\n");
        // Removed `b` and added `b` taken, `c` skipped: the last line is `b`, terminated.
        assert_eq!(stage_selection(b"a\nb", &hunk, &[1, 2]).unwrap(), b"a\nb\n");
        // Removed `b` dropped only and nothing added: `a` is the last line, terminated.
        assert_eq!(stage_selection(b"a\nb", &hunk, &[1]).unwrap(), b"a\n");
    }

    #[test]
    fn context_line_flagged_as_unterminated_must_be_last_in_index() {
        let hunk = Hunk {
            id: HunkId("flag".to_owned()),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            header: String::new(),
            lines: vec![ctx("a"), dl(LineKind::Context, "b", true)],
        };
        assert_eq!(stage_whole_hunk(b"a\nb", &hunk).unwrap(), b"a\nb");
        // Same hunk against an index that does end with a newline: mismatch.
        assert_eq!(stage_whole_hunk(b"a\nb\n", &hunk).unwrap_err(), MISMATCH);
    }

    #[test]
    fn unstage_reverts_selected_added_line() {
        // HEAD "a\nb\n", index "a\nB\n": staged change is b -> B.
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B")]);
        assert_eq!(unstage_selection(b"a\nB\n", &hunk, &[2]).unwrap(), b"a\n");
    }

    #[test]
    fn unstage_reinserts_selected_removed_line() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B")]);
        assert_eq!(
            unstage_selection(b"a\nB\n", &hunk, &[1]).unwrap(),
            b"a\nb\nB\n"
        );
    }

    #[test]
    fn unstage_everything_gives_head_and_nothing_gives_index() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B"), ctx("c")]);
        let index = b"a\nB\nc\n";
        assert_eq!(
            unstage_selection(index, &hunk, &all_changes(&hunk)).unwrap(),
            b"a\nb\nc\n"
        );
        assert_eq!(unstage_selection(index, &hunk, &[]).unwrap(), index);
    }

    #[test]
    fn unstage_rejects_new_side_mismatch() {
        let hunk = hunk_of(vec![ctx("a"), rem("b"), add("B")]);
        assert_eq!(
            unstage_selection(b"a\nb\n", &hunk, &[1]).unwrap_err(),
            MISMATCH
        );
    }

    #[test]
    fn pair_warning_reports_half_selected_replacements() {
        let hunk = hunk_of(vec![
            rem("x"),
            rem("y"),
            add("X"),
            add("Y"),
            add("Z"),
            ctx("c"),
            rem("q"),
            add("Q"),
        ]);
        assert!(pair_warning(&hunk, &[]).is_empty());
        assert_eq!(pair_warning(&hunk, &[0]), vec![(0, 2)]);
        assert_eq!(pair_warning(&hunk, &[2, 3]), vec![(0, 2), (1, 3)]);
        assert!(pair_warning(&hunk, &[0, 2]).is_empty());
        assert_eq!(pair_warning(&hunk, &[7]), vec![(6, 7)]);
        assert!(pair_warning(&hunk, &[0, 1, 2, 3, 4, 5, 6, 7]).is_empty());
    }

    fn line_strategy() -> impl Strategy<Value = Vec<u8>> {
        prop::sample::select(vec![
            b"a".to_vec(),
            b"b".to_vec(),
            b"c".to_vec(),
            b"a\r".to_vec(),
        ])
    }

    fn content_strategy() -> impl Strategy<Value = Vec<u8>> {
        (prop::collection::vec(line_strategy(), 0..6), any::<bool>())
            .prop_map(|(lines, final_newline)| join_lines(&lines, final_newline))
    }

    /// One line of a blob for the test differ: its text and whether it is
    /// unterminated, which is how git tells two same-text lines apart.
    #[derive(Clone, PartialEq, Eq)]
    struct Row {
        text: Vec<u8>,
        no_newline: bool,
    }

    fn rows(content: &[u8]) -> Vec<Row> {
        let lines = split_lines(content);
        let len = lines.len();
        let final_newline = has_final_newline(content);
        lines
            .into_iter()
            .enumerate()
            .map(|(pos, text)| Row {
                text,
                no_newline: no_newline_at(pos, len, final_newline),
            })
            .collect()
    }

    /// A single hunk covering the whole file, from a plain LCS line diff.
    fn single_hunk(old: &[u8], new: &[u8]) -> Hunk {
        let a = rows(old);
        let b = rows(new);
        let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i][j] = if a[i] == b[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }

        let (mut i, mut j) = (0, 0);
        let mut lines = Vec::new();
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                lines.push(dl(LineKind::Context, &text(&a[i].text), a[i].no_newline));
                i += 1;
                j += 1;
            } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
                lines.push(dl(LineKind::Removed, &text(&a[i].text), a[i].no_newline));
                i += 1;
            } else {
                lines.push(dl(LineKind::Added, &text(&b[j].text), b[j].no_newline));
                j += 1;
            }
        }
        hunk_of(lines)
    }

    /// Changed line indices of `hunk` for which `keep` holds.
    fn subset(hunk: &Hunk, keep: impl Fn(usize) -> bool) -> Vec<u32> {
        change_indices(hunk, keep)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn staged_plus_unstaged_equals_total_change(
            old in content_strategy(),
            new in content_strategy(),
            mask in prop::collection::vec(any::<bool>(), 0..40),
        ) {
            let hunk = single_hunk(&old, &new);
            let keep = |i: usize| mask.get(i).copied().unwrap_or(false);
            let selected = subset(&hunk, keep);
            let complement = subset(&hunk, |i| !keep(i));

            // Nothing selected is the identity; everything selected applies the hunk.
            prop_assert_eq!(stage_selection(&old, &hunk, &[]).unwrap(), old.clone());
            prop_assert_eq!(stage_whole_hunk(&old, &hunk).unwrap(), new.clone());

            // Staging S gives A. Unstaging S from the new side gives the same
            // index as staging the complement from the old side.
            let staged = stage_selection(&old, &hunk, &selected).unwrap();
            let unstaged_from_new = unstage_selection(&new, &hunk, &selected).unwrap();
            let staged_complement = stage_selection(&old, &hunk, &complement).unwrap();
            prop_assert_eq!(unstaged_from_new, staged_complement);

            // Reverting every staged change of A returns the old file.
            let staged_hunk = single_hunk(&old, &staged);
            let reverted = unstage_selection(&staged, &staged_hunk, &all_changes(&staged_hunk)).unwrap();
            prop_assert_eq!(reverted, old.clone());

            // Staging the remaining unstaged diff of A completes the change.
            let unstaged_hunk = single_hunk(&staged, &new);
            prop_assert_eq!(stage_whole_hunk(&staged, &unstaged_hunk).unwrap(), new.clone());
        }
    }
}
