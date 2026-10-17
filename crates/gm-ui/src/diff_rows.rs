//! Pure model behind the diff pane: the flat rows of its list, the cursor moves
//! over them, and the line selection that stage and unstage act on (D24).
//! Nothing here touches GPUI, so every rule is unit-tested on its own.

use std::collections::BTreeSet;

use gm_core::stage_math::{pair_hunk, pair_warning};
use gm_schema::{DiffSide, FileDiff, Hunk, LineKind};

/// Which comparison a hunk belongs to. Unstaged sorts first, as it is shown
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Unstaged,
    Staged,
}

impl Side {
    /// The side of the diff this list side is, for actions and commands.
    pub fn diff_side(self) -> DiffSide {
        match self {
            Side::Unstaged => DiffSide::Unstaged,
            Side::Staged => DiffSide::Staged,
        }
    }
}

/// What a diff list is for. The Working Copy diff stages and discards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The Working Copy: stage, unstage and discard.
    Working,
    /// A commit's diff (H4): it only shows. No control acts on the repository.
    ReadOnly,
}

/// The controls a mode shows and accepts. A control that is off is not drawn,
/// and its key or click does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    /// The "Staged | Unstaged" switch in the file header.
    pub side_selector: bool,
    /// The checkbox of each changed line, and the keys that select lines.
    pub line_selection: bool,
    /// The chunk buttons in each hunk header, and the keys that act on a hunk.
    pub chunk_actions: bool,
}

/// The controls of `mode`.
pub fn controls(mode: Mode) -> Controls {
    match mode {
        Mode::Working => Controls {
            side_selector: true,
            line_selection: true,
            chunk_actions: true,
        },
        Mode::ReadOnly => Controls {
            side_selector: false,
            line_selection: false,
            chunk_actions: false,
        },
    }
}

/// The side the pane opens on: unstaged, unless the file has only staged
/// changes.
pub fn default_side(unstaged: &[Hunk], staged: &[Hunk]) -> Side {
    if unstaged.is_empty() && !staged.is_empty() {
        Side::Staged
    } else {
        Side::Unstaged
    }
}

/// A changed line: side, index of its hunk in that side's diff, index of the
/// line in `Hunk::lines`. Only added and removed lines are ever keys.
pub type LineKey = (Side, usize, usize);

/// One row of the virtualized list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    HunkHeader {
        side: Side,
        hunk_index: usize,
    },
    Line {
        side: Side,
        hunk_index: usize,
        line_index: usize,
    },
}

impl Row {
    /// The side and hunk a row belongs to.
    pub fn hunk(self) -> (Side, usize) {
        match self {
            Row::HunkHeader { side, hunk_index }
            | Row::Line {
                side, hunk_index, ..
            } => (side, hunk_index),
        }
    }
}

/// The hunks of both sides, borrowed from the diffs the pane holds.
#[derive(Clone, Copy, Debug)]
pub struct Diffs<'a> {
    pub unstaged: &'a [Hunk],
    pub staged: &'a [Hunk],
}

/// The hunks of a side's diff; empty when the side has no diff.
pub fn hunks_of(diff: Option<&FileDiff>) -> &[Hunk] {
    diff.map_or(&[], |diff| diff.hunks.as_slice())
}

fn is_change(kind: LineKind) -> bool {
    kind != LineKind::Context
}

impl<'a> Diffs<'a> {
    pub fn hunks(self, side: Side) -> &'a [Hunk] {
        match side {
            Side::Unstaged => self.unstaged,
            Side::Staged => self.staged,
        }
    }

    pub fn hunk(self, side: Side, hunk_index: usize) -> Option<&'a Hunk> {
        self.hunks(side).get(hunk_index)
    }

    /// Whether `key` names an added or removed line.
    pub fn is_changed(self, key: LineKey) -> bool {
        let (side, hunk_index, line_index) = key;
        self.hunk(side, hunk_index)
            .and_then(|hunk| hunk.lines.get(line_index))
            .is_some_and(|line| is_change(line.kind))
    }
}

/// The rows of one side: per hunk its header and all of its lines. The other
/// side is not listed; the pane shows one side at a time.
pub fn build_rows(diffs: Diffs<'_>, side: Side) -> Vec<Row> {
    let mut rows = Vec::new();
    for (hunk_index, hunk) in diffs.hunks(side).iter().enumerate() {
        rows.push(Row::HunkHeader { side, hunk_index });
        rows.extend((0..hunk.lines.len()).map(|line_index| Row::Line {
            side,
            hunk_index,
            line_index,
        }));
    }
    rows
}

/// How a diff is laid out (D39). Unified lists each hunk's lines one after the
/// other; Split puts the old and the new side of each row next to each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)] // wired into the pane by the next commit
pub enum Layout {
    #[default]
    Unified,
    Split,
}

/// One side of a split row: a line of a hunk, keyed like a unified row. A
/// context line is a cell on both sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // wired into the pane by the next commit
pub struct Cell {
    pub key: LineKey,
    /// The line is unterminated (`\ No newline at end of file`).
    pub no_newline: bool,
}

/// One row of the split list: a hunk header across both sides, or a pair of
/// cells. An empty side is `None`: it is drawn as a plain panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // wired into the pane by the next commit
pub enum SplitRow {
    HunkHeader {
        side: Side,
        hunk_index: usize,
    },
    Pair {
        old: Option<Cell>,
        new: Option<Cell>,
    },
}

/// The split rows of one side, and for each unified row the split row that
/// holds it, so the cursor and scrolling can follow the unified rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)] // wired into the pane by the next commit
pub struct SplitRows {
    pub rows: Vec<SplitRow>,
    pub of_unified: Vec<usize>,
}

/// The split rows of one side, built from the same hunks as `build_rows`. The
/// pairing is `pair_hunk`'s (D24 uses the same pairs for its warning).
#[allow(dead_code)] // wired into the pane by the next commit
pub fn split_rows(diffs: Diffs<'_>, side: Side) -> SplitRows {
    let mut out = SplitRows::default();
    for (hunk_index, hunk) in diffs.hunks(side).iter().enumerate() {
        let header = out.rows.len();
        out.rows.push(SplitRow::HunkHeader { side, hunk_index });
        out.of_unified.push(header);
        // The split row holding each line of the hunk, in line order.
        let mut landed = vec![header; hunk.lines.len()];
        for pair in pair_hunk(hunk) {
            let at = out.rows.len();
            let cell = |index: Option<u32>| {
                let line_index = usize::try_from(index?).ok()?;
                let line = hunk.lines.get(line_index)?;
                Some(Cell {
                    key: (side, hunk_index, line_index),
                    no_newline: line.no_newline,
                })
            };
            for index in [pair.old, pair.new].into_iter().flatten() {
                if let Some(slot) = usize::try_from(index).ok().and_then(|i| landed.get_mut(i)) {
                    *slot = at;
                }
            }
            out.rows.push(SplitRow::Pair {
                old: cell(pair.old),
                new: cell(pair.new),
            });
        }
        out.of_unified.extend(landed);
    }
    out
}

/// The key of the row when it is an added or removed line.
pub fn changed_key(row: Row, diffs: Diffs<'_>) -> Option<LineKey> {
    match row {
        Row::Line {
            side,
            hunk_index,
            line_index,
        } if diffs.is_changed((side, hunk_index, line_index)) => {
            Some((side, hunk_index, line_index))
        }
        _ => None,
    }
}

/// The first hunk header strictly after `cursor`.
pub fn next_hunk_header(rows: &[Row], cursor: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .skip(cursor.saturating_add(1))
        .find(|(_, row)| matches!(row, Row::HunkHeader { .. }))
        .map(|(index, _)| index)
}

/// The last hunk header strictly before `cursor`.
pub fn prev_hunk_header(rows: &[Row], cursor: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .take(cursor)
        .rev()
        .find(|(_, row)| matches!(row, Row::HunkHeader { .. }))
        .map(|(index, _)| index)
}

/// The first changed line strictly after `cursor`.
pub fn next_changed_line(rows: &[Row], diffs: Diffs<'_>, cursor: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .skip(cursor.saturating_add(1))
        .find(|(_, row)| changed_key(**row, diffs).is_some())
        .map(|(index, _)| index)
}

/// The last changed line strictly before `cursor`.
pub fn prev_changed_line(rows: &[Row], diffs: Diffs<'_>, cursor: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .take(cursor)
        .rev()
        .find(|(_, row)| changed_key(**row, diffs).is_some())
        .map(|(index, _)| index)
}

/// The changed lines in the rows from `a` to `b`, both included, in either
/// order.
pub fn changed_keys_between(
    rows: &[Row],
    diffs: Diffs<'_>,
    a: usize,
    b: usize,
) -> BTreeSet<LineKey> {
    let (lo, hi) = (a.min(b), a.max(b));
    rows.iter()
        .enumerate()
        .skip(lo)
        .take(hi.saturating_sub(lo).saturating_add(1))
        .filter_map(|(_, row)| changed_key(*row, diffs))
        .collect()
}

/// Every changed line of one hunk.
fn changed_keys_of_hunk(diffs: Diffs<'_>, side: Side, hunk_index: usize) -> BTreeSet<LineKey> {
    diffs
        .hunk(side, hunk_index)
        .map(|hunk| {
            hunk.lines
                .iter()
                .enumerate()
                .filter(|(_, line)| is_change(line.kind))
                .map(|(line_index, _)| (side, hunk_index, line_index))
                .collect()
        })
        .unwrap_or_default()
}

/// The set of selected changed lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    keys: BTreeSet<LineKey>,
}

impl Selection {
    pub fn contains(&self, key: LineKey) -> bool {
        self.keys.contains(&key)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// Selects or deselects one line. A context line is never selected.
    pub fn toggle_line(&mut self, diffs: Diffs<'_>, key: LineKey) {
        if !diffs.is_changed(key) {
            return;
        }
        if !self.keys.remove(&key) {
            self.keys.insert(key);
        }
    }

    /// Selects every changed line of the hunk, or deselects them all when
    /// they are all selected already. Only the tests drive it for now.
    #[cfg(test)]
    pub fn toggle_hunk(&mut self, diffs: Diffs<'_>, side: Side, hunk_index: usize) {
        let keys = changed_keys_of_hunk(diffs, side, hunk_index);
        if keys.is_empty() {
            return;
        }
        if keys.iter().all(|key| self.keys.contains(key)) {
            for key in &keys {
                self.keys.remove(key);
            }
        } else {
            self.keys.extend(keys);
        }
    }

    /// The selection of visual mode: the lines selected before it started,
    /// plus the changed lines of the visual range.
    pub fn with_range(base: &Selection, range: BTreeSet<LineKey>) -> Selection {
        let mut keys = base.keys.clone();
        keys.extend(range);
        Selection { keys }
    }

    /// The selected line indices of one side, grouped by hunk in hunk order.
    /// Each line list is ascending, as `stage_math` expects.
    pub fn selected_lines_by_hunk(&self, side: Side) -> Vec<(usize, Vec<u32>)> {
        let mut groups: Vec<(usize, Vec<u32>)> = Vec::new();
        for &(key_side, hunk_index, line_index) in &self.keys {
            if key_side != side {
                continue;
            }
            let line = index_u32(line_index);
            match groups.last_mut() {
                Some((last, lines)) if *last == hunk_index => lines.push(line),
                _ => groups.push((hunk_index, vec![line])),
            }
        }
        groups
    }

    /// Keeps the lines whose hunk still exists under the same id in `new`,
    /// moved to the hunk's new index. Everything else is dropped.
    pub fn retain_present(&self, old: Diffs<'_>, new: Diffs<'_>) -> Selection {
        let mut kept = Selection::default();
        for &(side, hunk_index, line_index) in &self.keys {
            let Some(id) = old.hunk(side, hunk_index).map(|hunk| &hunk.id) else {
                continue;
            };
            let Some(new_index) = new.hunks(side).iter().position(|hunk| &hunk.id == id) else {
                continue;
            };
            let key = (side, new_index, line_index);
            if new.is_changed(key) {
                kept.keys.insert(key);
            }
        }
        kept
    }
}

/// The `(removed, added)` pairs split by the selection on one side, with the
/// hunk index of each pair.
pub fn split_pairs(
    diffs: Diffs<'_>,
    selection: &Selection,
    side: Side,
) -> Vec<(usize, (u32, u32))> {
    let mut pairs = Vec::new();
    for (hunk_index, lines) in selection.selected_lines_by_hunk(side) {
        if let Some(hunk) = diffs.hunk(side, hunk_index) {
            pairs.extend(
                pair_warning(hunk, &lines)
                    .into_iter()
                    .map(|pair| (hunk_index, pair)),
            );
        }
    }
    pairs
}

/// What an apply key does to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Stage,
    Unstage,
}

/// What a press of an apply key does: apply now, or only show the warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Apply,
    Warn,
}

/// The two-press rule for a selection that splits a pair. A press with no split
/// applies at once. A press that splits warns and remembers the action; the
/// same action pressed again applies. Returns the outcome and the new pending
/// action.
pub fn press(pending: Option<Apply>, action: Apply, splits: bool) -> (Press, Option<Apply>) {
    if !splits {
        return (Press::Apply, None);
    }
    if pending == Some(action) {
        (Press::Apply, None)
    } else {
        (Press::Warn, Some(action))
    }
}

/// The changed lines of one hunk, and how many of them are selected.
pub fn hunk_selection_count(
    diffs: Diffs<'_>,
    selection: &Selection,
    side: Side,
    hunk_index: usize,
) -> (usize, usize) {
    let total = changed_keys_of_hunk(diffs, side, hunk_index);
    let selected = total.iter().filter(|key| selection.contains(**key)).count();
    (selected, total.len())
}

/// Added and removed line counts over a side's hunks.
pub fn change_counts(hunks: &[Hunk]) -> (usize, usize) {
    hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .fold((0, 0), |(added, removed), line| match line.kind {
            LineKind::Added => (added + 1, removed),
            LineKind::Removed => (added, removed + 1),
            LineKind::Context => (added, removed),
        })
}

/// The header line of a hunk, as shown in the list.
pub fn hunk_header_text(hunk: &Hunk) -> String {
    let mut text = format!(
        "@@ -{},{} +{},{} @@",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    );
    if !hunk.header.is_empty() {
        text.push(' ');
        text.push_str(&hunk.header);
    }
    text
}

fn index_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{DiffLine, HunkId};
    use proptest::prelude::*;

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.to_owned(),
            no_newline: false,
        }
    }

    fn hunk(id: &str, lines: Vec<DiffLine>) -> Hunk {
        let old = lines.iter().filter(|l| l.kind != LineKind::Added).count();
        let new = lines.iter().filter(|l| l.kind != LineKind::Removed).count();
        Hunk {
            id: HunkId(id.to_owned()),
            old_start: 1,
            old_lines: u32::try_from(old).unwrap(),
            new_start: 1,
            new_lines: u32::try_from(new).unwrap(),
            header: String::new(),
            lines,
        }
    }

    #[test]
    fn a_read_only_diff_has_no_chunk_actions_no_line_selection_and_no_side_switch() {
        assert_eq!(
            controls(Mode::ReadOnly),
            Controls {
                side_selector: false,
                line_selection: false,
                chunk_actions: false,
            }
        );
    }

    #[test]
    fn the_working_copy_shows_every_control() {
        assert_eq!(
            controls(Mode::Working),
            Controls {
                side_selector: true,
                line_selection: true,
                chunk_actions: true,
            }
        );
    }

    /// `ctx, -a, +b, ctx`: lines 1 and 2 are changes.
    fn simple(id: &str) -> Hunk {
        hunk(
            id,
            vec![
                line(LineKind::Context, "c0"),
                line(LineKind::Removed, "a"),
                line(LineKind::Added, "b"),
                line(LineKind::Context, "c1"),
            ],
        )
    }

    fn diffs<'a>(unstaged: &'a [Hunk], staged: &'a [Hunk]) -> Diffs<'a> {
        Diffs { unstaged, staged }
    }

    #[test]
    fn no_hunks_gives_no_rows() {
        assert!(build_rows(diffs(&[], &[]), Side::Unstaged).is_empty());
        assert!(build_rows(diffs(&[], &[]), Side::Staged).is_empty());
    }

    #[test]
    fn rows_list_only_the_chosen_side() {
        let unstaged = [simple("u0")];
        let staged = [simple("s0")];
        let rows = build_rows(diffs(&unstaged, &staged), Side::Staged);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[0],
            Row::HunkHeader {
                side: Side::Staged,
                hunk_index: 0
            }
        );
        assert_eq!(
            rows[2],
            Row::Line {
                side: Side::Staged,
                hunk_index: 0,
                line_index: 1
            }
        );
    }

    #[test]
    fn the_pane_opens_on_staged_only_when_a_file_has_only_staged_changes() {
        let hunk_list = [simple("h")];
        assert_eq!(default_side(&hunk_list, &[]), Side::Unstaged);
        assert_eq!(default_side(&hunk_list, &hunk_list), Side::Unstaged);
        assert_eq!(default_side(&[], &hunk_list), Side::Staged);
        assert_eq!(default_side(&[], &[]), Side::Unstaged);
    }

    #[test]
    fn a_row_names_its_side_and_hunk() {
        assert_eq!(
            Row::Line {
                side: Side::Staged,
                hunk_index: 3,
                line_index: 0
            }
            .hunk(),
            (Side::Staged, 3)
        );
    }

    #[test]
    fn hunk_header_moves_skip_other_rows() {
        let unstaged = [simple("u0"), simple("u1")];
        let rows = build_rows(diffs(&unstaged, &[]), Side::Unstaged);
        // 0 header, 1-4 lines, 5 header, 6-9 lines
        assert_eq!(next_hunk_header(&rows, 0), Some(5));
        assert_eq!(next_hunk_header(&rows, 1), Some(5));
        assert_eq!(next_hunk_header(&rows, 4), Some(5));
        assert_eq!(next_hunk_header(&rows, 5), None);
        assert_eq!(prev_hunk_header(&rows, 9), Some(5));
        assert_eq!(prev_hunk_header(&rows, 5), Some(0));
        assert_eq!(prev_hunk_header(&rows, 0), None);
    }

    #[test]
    fn changed_line_moves_skip_context_and_headers() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let rows = build_rows(d, Side::Unstaged);
        // changed lines are rows 2 and 3; context rows 1 and 4
        assert_eq!(next_changed_line(&rows, d, 0), Some(2));
        assert_eq!(next_changed_line(&rows, d, 1), Some(2));
        assert_eq!(next_changed_line(&rows, d, 2), Some(3));
        assert_eq!(next_changed_line(&rows, d, 3), None);
        assert_eq!(prev_changed_line(&rows, d, 4), Some(3));
        assert_eq!(prev_changed_line(&rows, d, 3), Some(2));
        assert_eq!(prev_changed_line(&rows, d, 2), None);
    }

    #[test]
    fn changed_key_of_a_context_line_is_none() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let context = Row::Line {
            side: Side::Unstaged,
            hunk_index: 0,
            line_index: 0,
        };
        let removed = Row::Line {
            side: Side::Unstaged,
            hunk_index: 0,
            line_index: 1,
        };
        assert_eq!(changed_key(context, d), None);
        assert_eq!(changed_key(removed, d), Some((Side::Unstaged, 0, 1)));
    }

    #[test]
    fn toggle_line_ignores_context_lines() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let mut sel = Selection::default();
        sel.toggle_line(d, (Side::Unstaged, 0, 0));
        assert!(sel.is_empty());
        sel.toggle_line(d, (Side::Unstaged, 0, 1));
        assert!(sel.contains((Side::Unstaged, 0, 1)));
        sel.toggle_line(d, (Side::Unstaged, 0, 1));
        assert!(sel.is_empty());
    }

    #[test]
    fn toggle_hunk_selects_then_clears_all_changed_lines() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let mut sel = Selection::default();
        sel.toggle_hunk(d, Side::Unstaged, 0);
        assert_eq!(sel.len(), 2);
        assert!(sel.contains((Side::Unstaged, 0, 1)));
        assert!(sel.contains((Side::Unstaged, 0, 2)));
        sel.toggle_hunk(d, Side::Unstaged, 0);
        assert!(sel.is_empty());
    }

    #[test]
    fn toggle_hunk_with_partial_selection_selects_the_rest() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let mut sel = Selection::default();
        sel.toggle_line(d, (Side::Unstaged, 0, 1));
        sel.toggle_hunk(d, Side::Unstaged, 0);
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn clear_empties_the_selection() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let mut sel = Selection::default();
        sel.toggle_hunk(d, Side::Unstaged, 0);
        sel.clear();
        assert!(sel.is_empty());
    }

    #[test]
    fn selected_lines_are_grouped_by_hunk_and_side() {
        let unstaged = [simple("u0"), simple("u1")];
        let staged = [simple("s0")];
        let d = diffs(&unstaged, &staged);
        let mut sel = Selection::default();
        sel.toggle_line(d, (Side::Unstaged, 1, 2));
        sel.toggle_line(d, (Side::Unstaged, 0, 1));
        sel.toggle_line(d, (Side::Staged, 0, 2));
        assert_eq!(
            sel.selected_lines_by_hunk(Side::Unstaged),
            vec![(0, vec![1]), (1, vec![2])]
        );
        assert_eq!(sel.selected_lines_by_hunk(Side::Staged), vec![(0, vec![2])]);
    }

    #[test]
    fn retain_keeps_lines_of_hunks_still_present_at_their_new_index() {
        let old_u = [simple("a"), simple("b")];
        let d_old = diffs(&old_u, &[]);
        let mut sel = Selection::default();
        sel.toggle_line(d_old, (Side::Unstaged, 1, 1));
        sel.toggle_line(d_old, (Side::Unstaged, 0, 2));

        // hunk "b" moves to index 0 and hunk "a" disappears
        let new_u = [simple("b")];
        let kept = sel.retain_present(d_old, diffs(&new_u, &[]));
        assert_eq!(kept.len(), 1);
        assert!(kept.contains((Side::Unstaged, 0, 1)));
    }

    #[test]
    fn retain_drops_a_hunk_whose_content_changed() {
        let old_u = [simple("a")];
        let d_old = diffs(&old_u, &[]);
        let mut sel = Selection::default();
        sel.toggle_hunk(d_old, Side::Unstaged, 0);

        let new_u = [simple("a2")];
        let kept = sel.retain_present(d_old, diffs(&new_u, &[]));
        assert!(kept.is_empty());
    }

    #[test]
    fn retain_drops_a_line_that_is_no_longer_a_change() {
        let old_u = [simple("a")];
        let d_old = diffs(&old_u, &[]);
        let mut sel = Selection::default();
        sel.toggle_line(d_old, (Side::Unstaged, 0, 1));

        let same_id_no_change = [hunk("a", vec![line(LineKind::Context, "x")])];
        let kept = sel.retain_present(d_old, diffs(&same_id_no_change, &[]));
        assert!(kept.is_empty());
    }

    #[test]
    fn changed_range_is_inclusive_in_either_order() {
        let unstaged = [simple("u0"), simple("u1")];
        let d = diffs(&unstaged, &[]);
        let rows = build_rows(d, Side::Unstaged);
        let forward = changed_keys_between(&rows, d, 2, 9);
        let backward = changed_keys_between(&rows, d, 9, 2);
        assert_eq!(forward, backward);
        assert_eq!(forward.len(), 4);
        assert!(forward.contains(&(Side::Unstaged, 0, 1)));
        assert!(forward.contains(&(Side::Unstaged, 1, 2)));
    }

    #[test]
    fn visual_selection_is_base_plus_range() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let rows = build_rows(d, Side::Unstaged);
        let mut base = Selection::default();
        base.toggle_line(d, (Side::Unstaged, 0, 2));
        let range = changed_keys_between(&rows, d, 2, 3);
        let visual = Selection::with_range(&base, range);
        assert_eq!(visual.len(), 2);
    }

    #[test]
    fn a_split_pair_is_reported_once_per_side() {
        let unstaged = [simple("u0")];
        let d = diffs(&unstaged, &[]);
        let mut sel = Selection::default();
        sel.toggle_line(d, (Side::Unstaged, 0, 1));
        assert_eq!(split_pairs(d, &sel, Side::Unstaged), vec![(0, (1, 2))]);
        assert!(split_pairs(d, &sel, Side::Staged).is_empty());
        sel.toggle_line(d, (Side::Unstaged, 0, 2));
        assert!(split_pairs(d, &sel, Side::Unstaged).is_empty());
    }

    #[test]
    fn press_applies_when_nothing_splits() {
        assert_eq!(press(None, Apply::Stage, false), (Press::Apply, None));
        assert_eq!(
            press(Some(Apply::Unstage), Apply::Stage, false),
            (Press::Apply, None)
        );
    }

    #[test]
    fn press_warns_first_and_applies_on_the_second_press() {
        let (first, pending) = press(None, Apply::Stage, true);
        assert_eq!(first, Press::Warn);
        assert_eq!(pending, Some(Apply::Stage));
        let (second, pending) = press(pending, Apply::Stage, true);
        assert_eq!(second, Press::Apply);
        assert_eq!(pending, None);
    }

    #[test]
    fn press_of_another_action_warns_again() {
        let (outcome, pending) = press(Some(Apply::Stage), Apply::Unstage, true);
        assert_eq!(outcome, Press::Warn);
        assert_eq!(pending, Some(Apply::Unstage));
    }

    #[test]
    fn hunk_header_text_has_both_spans_and_the_context() {
        let mut h = simple("h");
        h.old_start = 4;
        h.old_lines = 3;
        h.new_start = 5;
        h.new_lines = 2;
        assert_eq!(hunk_header_text(&h), "@@ -4,3 +5,2 @@");
        h.header = "fn main()".to_owned();
        assert_eq!(hunk_header_text(&h), "@@ -4,3 +5,2 @@ fn main()");
    }

    #[test]
    fn hunks_of_none_is_empty() {
        assert!(hunks_of(None).is_empty());
    }

    fn cell(side: Side, hunk_index: usize, line_index: usize) -> Cell {
        Cell {
            key: (side, hunk_index, line_index),
            no_newline: false,
        }
    }

    #[test]
    fn the_layout_starts_unified() {
        assert_eq!(Layout::default(), Layout::Unified);
    }

    #[test]
    fn split_pairs_a_replacement_line_by_line_and_marks_the_empty_side() {
        // 0 ctx, 1 -a, 2 -b, 3 +A, 4 +B, 5 +C, 6 ctx
        let hunk = hunk(
            "h",
            vec![
                line(LineKind::Context, "c0"),
                line(LineKind::Removed, "a"),
                line(LineKind::Removed, "b"),
                line(LineKind::Added, "A"),
                line(LineKind::Added, "B"),
                line(LineKind::Added, "C"),
                line(LineKind::Context, "c1"),
            ],
        );
        let unstaged = [hunk];
        let split = split_rows(diffs(&unstaged, &[]), Side::Unstaged);
        let s = Side::Unstaged;
        assert_eq!(
            split.rows,
            vec![
                SplitRow::HunkHeader {
                    side: s,
                    hunk_index: 0
                },
                SplitRow::Pair {
                    old: Some(cell(s, 0, 0)),
                    new: Some(cell(s, 0, 0))
                },
                SplitRow::Pair {
                    old: Some(cell(s, 0, 1)),
                    new: Some(cell(s, 0, 3))
                },
                SplitRow::Pair {
                    old: Some(cell(s, 0, 2)),
                    new: Some(cell(s, 0, 4))
                },
                SplitRow::Pair {
                    old: None,
                    new: Some(cell(s, 0, 5))
                },
                SplitRow::Pair {
                    old: Some(cell(s, 0, 6)),
                    new: Some(cell(s, 0, 6))
                },
            ]
        );
    }

    #[test]
    fn split_maps_every_unified_row_to_the_split_row_that_holds_it() {
        // Unified: header, ctx c0, -a, +b, ctx c1. Split: header, c0, a|b, c1.
        let unstaged = [simple("h")];
        let split = split_rows(diffs(&unstaged, &[]), Side::Unstaged);
        assert_eq!(split.of_unified, vec![0, 1, 2, 2, 3]);
        assert_eq!(split.rows.len(), 4);
    }

    #[test]
    fn a_missing_final_newline_is_a_flag_of_its_cell() {
        let mut last = line(LineKind::Added, "tail");
        last.no_newline = true;
        let unstaged = [hunk("h", vec![line(LineKind::Removed, "x"), last])];
        let split = split_rows(diffs(&unstaged, &[]), Side::Unstaged);
        assert_eq!(
            split.rows[1],
            SplitRow::Pair {
                old: Some(Cell {
                    key: (Side::Unstaged, 0, 0),
                    no_newline: false
                }),
                new: Some(Cell {
                    key: (Side::Unstaged, 0, 1),
                    no_newline: true
                }),
            }
        );
    }

    #[test]
    fn a_read_only_diff_has_the_same_split_rows_as_its_unified_rows() {
        let unstaged = [simple("h")];
        let split = split_rows(diffs(&unstaged, &[]), Side::Unstaged);
        assert_eq!(
            build_rows(diffs(&unstaged, &[]), Side::Unstaged).len(),
            split.of_unified.len()
        );
    }

    fn kind_strategy() -> impl Strategy<Value = LineKind> {
        prop::sample::select(vec![LineKind::Context, LineKind::Added, LineKind::Removed])
    }

    proptest! {
        #[test]
        fn every_changed_line_is_in_exactly_one_cell_and_every_unified_row_lands(
            kinds in prop::collection::vec(kind_strategy(), 0..16),
        ) {
            let lines: Vec<DiffLine> = kinds
                .iter()
                .enumerate()
                .map(|(i, kind)| line(*kind, &format!("l{i}")))
                .collect();
            let unstaged = [hunk("h", lines)];
            let d = diffs(&unstaged, &[]);
            let split = split_rows(d, Side::Unstaged);
            let mut old_hits = vec![0u32; kinds.len()];
            let mut new_hits = vec![0u32; kinds.len()];
            for row in &split.rows {
                if let SplitRow::Pair { old, new } = row {
                    if let Some(cell) = old {
                        old_hits[cell.key.2] += 1;
                    }
                    if let Some(cell) = new {
                        new_hits[cell.key.2] += 1;
                    }
                }
            }
            for (i, kind) in kinds.iter().enumerate() {
                let (want_old, want_new) = match kind {
                    LineKind::Context => (1, 1),
                    LineKind::Removed => (1, 0),
                    LineKind::Added => (0, 1),
                };
                prop_assert_eq!(old_hits[i], want_old);
                prop_assert_eq!(new_hits[i], want_new);
            }
            // Each unified row lands in a split row that holds it.
            let unified = build_rows(d, Side::Unstaged);
            prop_assert_eq!(unified.len(), split.of_unified.len());
            for (u, row) in unified.iter().enumerate() {
                let at = split.of_unified[u];
                let holds = match (row, split.rows[at]) {
                    (Row::HunkHeader { .. }, SplitRow::HunkHeader { .. }) => true,
                    (Row::Line { line_index, .. }, SplitRow::Pair { old, new }) => {
                        [old, new].into_iter().flatten().any(|c| c.key.2 == *line_index)
                    }
                    _ => false,
                };
                prop_assert!(holds);
            }
        }
    }
}
