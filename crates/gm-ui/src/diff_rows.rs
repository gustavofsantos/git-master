//! Pure model behind the diff pane: the flat rows of its list, the cursor moves
//! over them, and the line selection that stage and unstage act on (D24).
//! Nothing here touches GPUI, so every rule is unit-tested on its own.

use std::collections::BTreeSet;
use std::ops::Range;

use gm_core::stage_math::{pair_hunk, pair_warning};
use gm_schema::{Change, DiffSide, FileDiff, Hunk, LineKind};

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
#[allow(clippy::struct_excessive_bools)] // one bool per control of the mode
pub struct Controls {
    /// The "Staged | Unstaged" switch in the file header.
    pub side_selector: bool,
    /// The checkbox of each changed line, and the keys that select lines.
    pub line_selection: bool,
    /// The chunk buttons in each hunk header, and the keys that act on a hunk.
    pub chunk_actions: bool,
    /// The "Unified | Split" switch in the file header (D39).
    pub layout_selector: bool,
}

/// The controls of `mode`.
pub fn controls(mode: Mode) -> Controls {
    match mode {
        Mode::Working => Controls {
            side_selector: true,
            line_selection: true,
            chunk_actions: true,
            layout_selector: true,
        },
        Mode::ReadOnly => Controls {
            side_selector: false,
            line_selection: false,
            chunk_actions: false,
            layout_selector: true,
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

/// One row of the virtualized list. The last three exist only in a stacked
/// diff of a commit (H4c): the header of a file, a notice in place of a file's
/// rows, and the count of files past the limit.
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
    FileHeader {
        file: usize,
    },
    Notice {
        file: usize,
        notice: Notice,
    },
    MoreFiles {
        count: usize,
    },
}

impl Row {
    /// The side and hunk a row belongs to; `None` for the stack's own rows.
    pub fn hunk(self) -> Option<(Side, usize)> {
        match self {
            Row::HunkHeader { side, hunk_index }
            | Row::Line {
                side, hunk_index, ..
            } => Some((side, hunk_index)),
            Row::FileHeader { .. } | Row::Notice { .. } | Row::MoreFiles { .. } => None,
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
pub enum Layout {
    #[default]
    Unified,
    Split,
}

/// One side of a split row: a line of a hunk, keyed like a unified row. A
/// context line is a cell on both sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub key: LineKey,
    /// The line is unterminated (`\ No newline at end of file`).
    pub no_newline: bool,
}

/// One row of the split list: a hunk header across both sides, or a pair of
/// cells. An empty side is `None`: it is drawn as a plain panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    HunkHeader {
        side: Side,
        hunk_index: usize,
    },
    Pair {
        old: Option<Cell>,
        new: Option<Cell>,
    },
    /// A stack's file header, notice or count, across both sides.
    Banner(Row),
}

/// The split rows of one side, and for each unified row the split row that
/// holds it, so the cursor and scrolling can follow the unified rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplitRows {
    pub rows: Vec<SplitRow>,
    pub of_unified: Vec<usize>,
}

/// The split rows of one side, built from the same hunks as `build_rows`. The
/// pairing is `pair_hunk`'s (D24 uses the same pairs for its warning).
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

/// Changed lines of one file a stacked diff shows before it says how many are
/// left (H4c). Keeps a huge file from making the list slow to build.
pub const FILE_LINE_LIMIT: usize = 5_000;

/// Why a file of a stack has no rows of its own, or that some of its lines
/// are not shown. Copied into a row, so it holds no text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A binary file: git gives no text for it.
    Binary,
    /// A rename with no text changes (the path is the file header's).
    Renamed,
    /// A mode change with no text changes.
    ModeOnly,
    /// An added or deleted file with no lines.
    Empty,
    /// This many lines of the file are past `FILE_LINE_LIMIT`.
    MoreLines(usize),
}

/// The text of a notice, shown in place of a file's rows or after its lines.
#[must_use]
pub fn notice_text(notice: Notice) -> String {
    match notice {
        Notice::Binary => "Binary file: its contents are not shown.".to_owned(),
        Notice::Renamed => "Renamed, with no text changes.".to_owned(),
        Notice::ModeOnly => "Mode change only: no text changes.".to_owned(),
        Notice::Empty => "Empty file: no lines.".to_owned(),
        Notice::MoreLines(count) => format!("and {count} more lines, not shown."),
    }
}

/// The notice of a file that has no text rows of its own, or `None` when its
/// hunks are shown. A binary file is a notice whatever its hunks.
#[must_use]
pub fn file_notice(change: Change, renamed: bool, binary: bool, has_hunks: bool) -> Option<Notice> {
    if binary {
        return Some(Notice::Binary);
    }
    if has_hunks {
        return None;
    }
    Some(if renamed {
        Notice::Renamed
    } else if matches!(change, Change::Added | Change::Deleted) {
        Notice::Empty
    } else {
        Notice::ModeOnly
    })
}

/// One file of a stack: the range of the stack's hunks that are its own, and
/// the notice that replaces its rows when it has no text (see `file_notice`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFile {
    pub hunks: Range<usize>,
    pub notice: Option<Notice>,
}

/// The rows of a stacked diff of one commit (H4c), in both layouts, and the
/// row where each file starts in each. `hunks` of the stack are every file's
/// hunks in order, so a hunk key is the same as in the single-side rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stacked {
    pub rows: Vec<Row>,
    pub split: Vec<SplitRow>,
    pub unified_starts: Vec<usize>,
    pub split_starts: Vec<usize>,
}

/// Joins the hunks of several files into one list, and gives each file's
/// range in it. The order of the files is kept.
#[must_use]
pub fn concat_hunks(diffs: Vec<FileDiff>) -> (Vec<Hunk>, Vec<Range<usize>>) {
    let mut hunks = Vec::new();
    let mut ranges = Vec::with_capacity(diffs.len());
    for diff in diffs {
        let start = hunks.len();
        hunks.extend(diff.hunks);
        ranges.push(start..hunks.len());
    }
    (hunks, ranges)
}

/// Builds the rows of a stack: per file a header, then its notice or its hunks
/// in both layouts, with at most `FILE_LINE_LIMIT` lines a file. `more_files`
/// past the listed files adds a last row that counts them.
#[must_use]
pub fn build_stack(hunks: &[Hunk], files: &[StackFile], more_files: usize) -> Stacked {
    let mut out = Stacked::default();
    for (file, spec) in files.iter().enumerate() {
        out.unified_starts.push(out.rows.len());
        out.split_starts.push(out.split.len());
        let header = Row::FileHeader { file };
        out.rows.push(header);
        out.split.push(SplitRow::Banner(header));
        if let Some(notice) = spec.notice {
            let row = Row::Notice { file, notice };
            out.rows.push(row);
            out.split.push(SplitRow::Banner(row));
        }
        let own = &hunks[spec.hunks.clone()];
        let total: usize = own.iter().map(|hunk| hunk.lines.len()).sum();
        let mut shown = 0;
        for (offset, hunk) in own.iter().enumerate() {
            if shown >= FILE_LINE_LIMIT {
                break;
            }
            let hunk_index = spec.hunks.start + offset;
            push_unified_hunk(&mut out.rows, hunk_index, hunk, &mut shown);
            push_split_hunk(&mut out.split, hunk_index, hunk, &mut shown);
        }
        if shown < total {
            let row = Row::Notice {
                file,
                notice: Notice::MoreLines(total - shown),
            };
            out.rows.push(row);
            out.split.push(SplitRow::Banner(row));
        }
    }
    if more_files > 0 {
        let row = Row::MoreFiles { count: more_files };
        out.rows.push(row);
        out.split.push(SplitRow::Banner(row));
    }
    out
}

/// A hunk's header and lines, as `build_rows` lists them, until the file's limit.
fn push_unified_hunk(rows: &mut Vec<Row>, hunk_index: usize, hunk: &Hunk, shown: &mut usize) {
    rows.push(Row::HunkHeader {
        side: Side::Unstaged,
        hunk_index,
    });
    for line_index in 0..hunk.lines.len() {
        if *shown >= FILE_LINE_LIMIT {
            return;
        }
        *shown += 1;
        rows.push(Row::Line {
            side: Side::Unstaged,
            hunk_index,
            line_index,
        });
    }
}

/// A hunk's header and split pairs, as `split_rows` pairs them, until the
/// file's limit. A context line counts once, a changed pair counts each side.
fn push_split_hunk(rows: &mut Vec<SplitRow>, hunk_index: usize, hunk: &Hunk, shown: &mut usize) {
    rows.push(SplitRow::HunkHeader {
        side: Side::Unstaged,
        hunk_index,
    });
    let cell = |index: Option<u32>| {
        let line_index = usize::try_from(index?).ok()?;
        let line = hunk.lines.get(line_index)?;
        Some(Cell {
            key: (Side::Unstaged, hunk_index, line_index),
            no_newline: line.no_newline,
        })
    };
    for pair in pair_hunk(hunk) {
        if *shown >= FILE_LINE_LIMIT {
            return;
        }
        let context = pair.old.is_some() && pair.old == pair.new;
        *shown += if context {
            1
        } else {
            usize::from(pair.old.is_some()) + usize::from(pair.new.is_some())
        };
        rows.push(SplitRow::Pair {
            old: cell(pair.old),
            new: cell(pair.new),
        });
    }
}

/// The index of the file whose rows hold row `top`: the last file that starts
/// at or before it. Zero when there are no files.
#[must_use]
pub fn file_at_row(starts: &[usize], top: usize) -> usize {
    starts
        .partition_point(|&start| start <= top)
        .saturating_sub(1)
}

/// The file a step from the top row goes to. It wraps: forward from the last
/// file goes to the first, and backward from the first file's header goes to
/// the last. Forward goes to the file after the one at the top. Backward goes
/// to the top file's own header when the top is below it, otherwise to the file
/// before. `None` only when there are no files.
#[must_use]
pub fn step_from_top(starts: &[usize], top: usize, forward: bool) -> Option<usize> {
    if starts.is_empty() {
        return None;
    }
    let current = file_at_row(starts, top);
    if forward {
        Some((current + 1) % starts.len())
    } else if top > starts[current] {
        Some(current)
    } else {
        Some(current.checked_sub(1).unwrap_or(starts.len() - 1))
    }
}

/// The blank rows to add after the last row of a stacked list, so that the
/// last file's header can scroll to the top of the page. `tail_rows` counts the
/// rows from the last file's header to the end (the header included) and
/// `visible_rows` the rows a page shows. A list is scrolled at most to its end,
/// so the rows past the end must make up `visible_rows` below the header.
/// Padding is not a row of the diff: the cursor, the hits and the file starts
/// never see it.
#[must_use]
pub fn scroll_padding(tail_rows: usize, visible_rows: usize) -> usize {
    visible_rows.saturating_sub(tail_rows)
}

/// Whether a split cell holds the cursor's line. A context line is one line on
/// both sides, so both of its cells hold the cursor; a changed line is one cell.
#[must_use]
pub fn split_cell_on_cursor(cell: Option<Cell>, cursor: Option<LineKey>) -> bool {
    matches!((cell, cursor), (Some(cell), Some(key)) if cell.key == key)
}

/// The mark git writes after a line that has no newline at the end of its
/// file (`no_newline`), or nothing for a line that has one.
#[must_use]
pub fn no_newline_mark(no_newline: bool) -> &'static str {
    if no_newline {
        "\\ No newline at end of file"
    } else {
        ""
    }
}

/// Columns a tab takes up, counted from the column it starts at. Used for the
/// width of a line, which bounds the horizontal scroll of the split view.
pub const TAB_STOP: usize = 4;

/// The number of columns `text` occupies: wide characters take two and
/// combining marks none. A tab goes to the next multiple of `TAB_STOP`.
pub fn display_columns(text: &str) -> usize {
    let mut column = 0;
    for ch in text.chars() {
        column += char_columns(ch, column);
    }
    column
}

/// `text` without its leading characters that fit in `columns`. A character
/// that starts before `columns` and ends after it is kept, so a cut never
/// splits a character.
pub fn skip_columns(text: &str, columns: usize) -> &str {
    let mut column = 0;
    for (at, ch) in text.char_indices() {
        let end = column + char_columns(ch, column);
        if end > columns {
            return &text[at..];
        }
        column = end;
    }
    ""
}

/// The most columns a line of `side` takes, which is how far its cells can
/// scroll.
pub fn max_scroll_columns(diffs: Diffs<'_>, side: Side) -> usize {
    diffs
        .hunks(side)
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| display_columns(&line.text))
        .max()
        .unwrap_or(0)
}

fn char_columns(ch: char, column: usize) -> usize {
    if ch == '\t' {
        TAB_STOP - column % TAB_STOP
    } else {
        unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
    }
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
                layout_selector: true,
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
                layout_selector: true,
            }
        );
    }

    #[test]
    fn a_split_cell_is_on_the_cursor_only_when_its_line_is() {
        // ctx, -a, +b, ctx: the removed and added lines share one split row.
        let unstaged = vec![simple("h0")];
        let diffs = Diffs {
            unstaged: &unstaged,
            staged: &[],
        };
        let split = split_rows(diffs, Side::Unstaged);
        let removed = (Side::Unstaged, 0, 1);
        let added = (Side::Unstaged, 0, 2);
        let (old, new) = split
            .rows
            .iter()
            .find_map(|row| match row {
                SplitRow::Pair { old, new } if old.map(|c| c.key) == Some(removed) => {
                    Some((*old, *new))
                }
                _ => None,
            })
            .expect("the row of the removed line");
        assert_eq!(new.map(|c| c.key), Some(added));
        assert!(split_cell_on_cursor(old, Some(removed)));
        assert!(!split_cell_on_cursor(new, Some(removed)));
        assert!(split_cell_on_cursor(new, Some(added)));
        assert!(!split_cell_on_cursor(old, Some(added)));
        // A context line is one line on both sides, so both cells are on the cursor.
        let context = (Side::Unstaged, 0, 0);
        let (old, new) = split
            .rows
            .iter()
            .find_map(|row| match row {
                SplitRow::Pair { old, new } if old.map(|c| c.key) == Some(context) => {
                    Some((*old, *new))
                }
                _ => None,
            })
            .expect("the context row");
        assert!(split_cell_on_cursor(old, Some(context)));
        assert!(split_cell_on_cursor(new, Some(context)));
        assert!(!split_cell_on_cursor(None, Some(removed)));
        assert!(!split_cell_on_cursor(old, None));
    }

    #[test]
    fn selecting_through_split_cells_selects_the_same_lines_as_the_unified_rows() {
        // Two hunks, each with a -/+ pair and a surplus +: the cells must cover
        // every changed line exactly as the unified rows do.
        let unstaged = vec![simple("h0"), simple("h1")];
        let diffs = Diffs {
            unstaged: &unstaged,
            staged: &[],
        };
        let unified: Vec<LineKey> = build_rows(diffs, Side::Unstaged)
            .into_iter()
            .filter_map(|row| match row {
                Row::Line {
                    side,
                    hunk_index,
                    line_index,
                } => Some((side, hunk_index, line_index)),
                Row::HunkHeader { .. }
                | Row::FileHeader { .. }
                | Row::Notice { .. }
                | Row::MoreFiles { .. } => None,
            })
            .filter(|key| diffs.is_changed(*key))
            .collect();
        let mut from_split: Vec<LineKey> = split_rows(diffs, Side::Unstaged)
            .rows
            .iter()
            .flat_map(|row| match row {
                SplitRow::Pair { old, new } => [*old, *new],
                SplitRow::HunkHeader { .. } | SplitRow::Banner(_) => [None, None],
            })
            .flatten()
            .map(|cell| cell.key)
            .filter(|key| diffs.is_changed(*key))
            .collect();
        from_split.sort_unstable();
        from_split.dedup();
        let mut unified_sorted = unified.clone();
        unified_sorted.sort_unstable();
        assert_eq!(from_split, unified_sorted);

        let mut by_split = Selection::default();
        let mut by_unified = Selection::default();
        for key in &from_split {
            by_split.toggle_line(diffs, *key);
        }
        for key in &unified {
            by_unified.toggle_line(diffs, *key);
        }
        assert_eq!(
            by_split.selected_lines_by_hunk(Side::Unstaged),
            by_unified.selected_lines_by_hunk(Side::Unstaged)
        );
    }

    #[test]
    fn a_line_without_its_newline_gets_git_s_marker_in_both_layouts() {
        assert_eq!(no_newline_mark(true), "\\ No newline at end of file");
        assert_eq!(no_newline_mark(false), "");
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
            Some((Side::Staged, 3))
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

    #[test]
    fn display_columns_count_wide_characters_twice_and_marks_once() {
        assert_eq!(display_columns("abc"), 3);
        assert_eq!(display_columns("日本"), 4);
        assert_eq!(display_columns("e\u{301}"), 1);
        assert_eq!(display_columns("\t"), TAB_STOP);
        assert_eq!(display_columns("a\tb"), TAB_STOP + 1);
    }

    #[test]
    fn skip_columns_drops_whole_characters_from_the_left() {
        assert_eq!(skip_columns("abcdef", 0), "abcdef");
        assert_eq!(skip_columns("abcdef", 2), "cdef");
        assert_eq!(skip_columns("日本語", 2), "本語");
        assert_eq!(skip_columns("abc", 9), "");
        assert_eq!(skip_columns("a\tb", 1), "\tb");
    }

    #[test]
    fn the_scroll_limit_is_the_widest_line_of_the_side() {
        let long = hunk(
            "h",
            vec![
                line(LineKind::Added, "0123456789"),
                line(LineKind::Context, "x"),
            ],
        );
        let short = hunk("i", vec![line(LineKind::Removed, "ab")]);
        let unstaged = [short, long];
        assert_eq!(
            max_scroll_columns(diffs(&unstaged, &[]), Side::Unstaged),
            10
        );
        assert_eq!(max_scroll_columns(diffs(&[], &[]), Side::Unstaged), 0);
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

/// The split rows of commits read with real git: an addition, a deletion and a
/// rename with an edit (the H4b-A cases the split view must pair correctly).
#[cfg(test)]
mod real_git {
    use super::*;
    use gm_core::GitPort;
    use gm_core::git::SubprocessGit;
    use gm_schema::{DiffSide, FileDiff, StageTarget};
    use std::fmt::Write as _;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::AtomicBool;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn commit(dir: &Path, message: &str) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// The file diff of `path` in `oid`, as the engine reads it, and its split
    /// rows on the unstaged side (the side a commit diff shows).
    /// The diff of `path` in `oid`, from the stack of the whole commit (H4c).
    fn read(dir: &Path, oid: &str, path: &str) -> (FileDiff, SplitRows) {
        let stop = AtomicBool::new(false);
        let detail = SubprocessGit::default().commit_detail(dir, oid).unwrap()(&stop).unwrap();
        let stack = SubprocessGit::default()
            .commit_stack(dir, oid, &detail.files)
            .unwrap();
        let diff = stack(&stop)
            .unwrap()
            .into_iter()
            .find(|diff| diff.path == path)
            .unwrap();
        let split = split_rows(
            Diffs {
                unstaged: &diff.hunks,
                staged: &[],
            },
            Side::Unstaged,
        );
        (diff, split)
    }

    /// A repository with `one/two/three` committed and `ONE/TWO/three/four` in
    /// the working copy: a -/+ pair, another -/+ pair and a surplus +.
    fn working_copy_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\n").unwrap();
        commit(dir.path(), "init");
        std::fs::write(dir.path().join("f.txt"), "ONE\nTWO\nthree\nfour\n").unwrap();
        dir
    }

    /// The index blob of `path`, as git stores it.
    fn index_blob(dir: &Path, path: &str) -> Vec<u8> {
        Command::new("git")
            .args(["show", &format!(":{path}")])
            .current_dir(dir)
            .output()
            .unwrap()
            .stdout
    }

    /// The keys of the unstaged lines whose text is in `texts`, found by walking
    /// `keys_of`, which lists the lines in some layout's order.
    fn keys_with_text(diff: &FileDiff, texts: &[&str], keys: &[LineKey]) -> Vec<LineKey> {
        let mut found: Vec<LineKey> = keys
            .iter()
            .copied()
            .filter(|&(_, hunk, line)| texts.contains(&diff.hunks[hunk].lines[line].text.as_str()))
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    /// Stages the lines `keys` select, the way the pane's Stage Selected Lines
    /// does (D24). Returns the index blob after it.
    fn stage_keys(dir: &Path, keys: &[LineKey]) -> Vec<u8> {
        let diff = SubprocessGit::default()
            .diff(dir, "f.txt", DiffSide::Unstaged)
            .unwrap();
        let diffs = Diffs {
            unstaged: &diff.hunks,
            staged: &[],
        };
        let mut selection = Selection::default();
        for &key in keys {
            selection.toggle_line(diffs, key);
        }
        for (hunk_index, lines) in selection.selected_lines_by_hunk(Side::Unstaged) {
            SubprocessGit::default()
                .stage(
                    dir,
                    &StageTarget::Lines {
                        path: "f.txt".into(),
                        hunk: diff.hunks[hunk_index].id.clone(),
                        lines,
                    },
                )
                .unwrap();
        }
        index_blob(dir, "f.txt")
    }

    #[test]
    fn staging_through_split_cells_writes_the_index_the_unified_rows_write() {
        let wanted = ["two", "TWO", "four"];
        let (by_hand, by_split_hand) = (working_copy_repo(), working_copy_repo());
        let diff = SubprocessGit::default()
            .diff(by_hand.path(), "f.txt", DiffSide::Unstaged)
            .unwrap();
        // The unified rows: every changed line, in row order.
        let unified: Vec<LineKey> = build_rows(
            Diffs {
                unstaged: &diff.hunks,
                staged: &[],
            },
            Side::Unstaged,
        )
        .into_iter()
        .filter_map(|row| match row {
            Row::Line {
                side,
                hunk_index,
                line_index,
            } => Some((side, hunk_index, line_index)),
            Row::HunkHeader { .. }
            | Row::FileHeader { .. }
            | Row::Notice { .. }
            | Row::MoreFiles { .. } => None,
        })
        .collect();
        // The split cells: each cell names its own line, the same key.
        let split: Vec<LineKey> = split_rows(
            Diffs {
                unstaged: &diff.hunks,
                staged: &[],
            },
            Side::Unstaged,
        )
        .rows
        .iter()
        .flat_map(|row| match row {
            SplitRow::Pair { old, new } => [*old, *new],
            SplitRow::HunkHeader { .. } | SplitRow::Banner(_) => [None, None],
        })
        .flatten()
        .map(|cell| cell.key)
        .collect();
        let by_unified = keys_with_text(&diff, &wanted, &unified);
        let by_split = keys_with_text(&diff, &wanted, &split);
        assert_eq!(by_split, by_unified);
        assert_eq!(by_unified.len(), 3);

        let through_unified = stage_keys(by_hand.path(), &by_unified);
        let through_split = stage_keys(by_split_hand.path(), &by_split);
        assert_eq!(through_split, through_unified);
        assert_eq!(through_split, b"one\nTWO\nthree\nfour\n");
    }

    /// The kind of the line a cell names.
    fn kind_of(diff: &FileDiff, cell: Cell) -> LineKind {
        diff.hunks[cell.key.1].lines[cell.key.2].kind
    }

    fn pairs(split: &SplitRows) -> Vec<(Option<Cell>, Option<Cell>)> {
        split
            .rows
            .iter()
            .filter_map(|row| match row {
                SplitRow::Pair { old, new } => Some((*old, *new)),
                SplitRow::HunkHeader { .. } | SplitRow::Banner(_) => None,
            })
            .collect()
    }

    #[test]
    fn an_added_file_has_an_empty_old_side_on_every_row() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        commit(dir.path(), "root");
        std::fs::write(dir.path().join("new.txt"), "x\ny\n").unwrap();
        let oid = commit(dir.path(), "add");

        let (diff, split) = read(dir.path(), &oid, "new.txt");
        assert_eq!(split.rows.len(), 1 + diff.hunks[0].lines.len());
        for (old, new) in pairs(&split) {
            assert_eq!(old, None);
            assert_eq!(kind_of(&diff, new.unwrap()), LineKind::Added);
        }
    }

    #[test]
    fn a_deleted_file_has_an_empty_new_side_on_every_row() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("gone.txt"), "x\ny\n").unwrap();
        commit(dir.path(), "root");
        std::fs::remove_file(dir.path().join("gone.txt")).unwrap();
        let oid = commit(dir.path(), "delete");

        let (diff, split) = read(dir.path(), &oid, "gone.txt");
        assert_eq!(pairs(&split).len(), 2);
        for (old, new) in pairs(&split) {
            assert_eq!(new, None);
            assert_eq!(kind_of(&diff, old.unwrap()), LineKind::Removed);
        }
    }

    #[test]
    fn a_rename_with_an_edit_pairs_the_changed_line_across_both_sides() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let mut body = String::new();
        for n in 1..=10 {
            writeln!(body, "line {n}").unwrap();
        }
        std::fs::write(dir.path().join("old.txt"), &body).unwrap();
        commit(dir.path(), "root");
        git(dir.path(), &["mv", "old.txt", "new.txt"]);
        std::fs::write(
            dir.path().join("new.txt"),
            body.replace("line 5\n", "five\n"),
        )
        .unwrap();
        let oid = commit(dir.path(), "rename and edit");

        let (diff, split) = read(dir.path(), &oid, "new.txt");
        assert_eq!(diff.old_path.as_deref(), Some("old.txt"));
        let both_changed: Vec<_> = pairs(&split)
            .into_iter()
            .filter_map(|(old, new)| Some((old?, new?)))
            .filter(|(old, _)| kind_of(&diff, *old) != LineKind::Context)
            .collect();
        assert_eq!(both_changed.len(), 1);
        let (old, new) = both_changed[0];
        assert_eq!(kind_of(&diff, old), LineKind::Removed);
        assert_eq!(kind_of(&diff, new), LineKind::Added);
        // Every changed line sits in exactly one cell of its own side.
        let changed_old = pairs(&split)
            .iter()
            .filter(|(old, _)| old.is_some_and(|c| kind_of(&diff, c) == LineKind::Removed))
            .count();
        let changed_new = pairs(&split)
            .iter()
            .filter(|(_, new)| new.is_some_and(|c| kind_of(&diff, c) == LineKind::Added))
            .count();
        let total = |kind| {
            diff.hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind == kind)
                .count()
        };
        assert_eq!(changed_old, total(LineKind::Removed));
        assert_eq!(changed_new, total(LineKind::Added));
    }
}

#[cfg(test)]
mod stack_tests {
    use gm_schema::{DiffLine, HunkId};

    use super::*;

    fn hunk(lines: usize) -> Hunk {
        Hunk {
            id: HunkId(String::new()),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 0,
            header: String::new(),
            lines: (0..lines)
                .map(|n| DiffLine {
                    kind: LineKind::Added,
                    old_no: None,
                    new_no: Some(u32::try_from(n + 1).unwrap()),
                    text: format!("line {n}"),
                    no_newline: false,
                })
                .collect(),
        }
    }

    fn file(hunks: Range<usize>) -> StackFile {
        StackFile {
            hunks,
            notice: None,
        }
    }

    #[test]
    fn a_file_with_text_has_no_notice_and_an_empty_one_says_why() {
        assert_eq!(file_notice(Change::Modified, false, false, true), None);
        assert_eq!(
            file_notice(Change::Added, false, false, false),
            Some(Notice::Empty)
        );
        assert_eq!(
            file_notice(Change::Deleted, false, false, false),
            Some(Notice::Empty)
        );
        assert_eq!(
            file_notice(Change::Modified, false, false, false),
            Some(Notice::ModeOnly)
        );
        assert_eq!(
            file_notice(Change::Renamed, true, false, false),
            Some(Notice::Renamed)
        );
    }

    #[test]
    fn a_binary_file_is_a_notice_whatever_its_hunks() {
        assert_eq!(
            file_notice(Change::Modified, false, true, false),
            Some(Notice::Binary)
        );
        assert_eq!(
            file_notice(Change::Modified, false, true, true),
            Some(Notice::Binary)
        );
    }

    #[test]
    fn the_notice_texts_name_the_case_and_the_count() {
        assert!(notice_text(Notice::Binary).starts_with("Binary file"));
        assert_eq!(
            notice_text(Notice::MoreLines(3)),
            "and 3 more lines, not shown."
        );
    }

    #[test]
    fn a_stack_lists_each_files_header_then_its_hunks_in_order() {
        let hunks = [hunk(2), hunk(1)];
        let files = [file(0..1), file(1..2)];
        let stacked = build_stack(&hunks, &files, 0);
        assert_eq!(
            stacked.rows,
            [
                Row::FileHeader { file: 0 },
                Row::HunkHeader {
                    side: Side::Unstaged,
                    hunk_index: 0
                },
                Row::Line {
                    side: Side::Unstaged,
                    hunk_index: 0,
                    line_index: 0
                },
                Row::Line {
                    side: Side::Unstaged,
                    hunk_index: 0,
                    line_index: 1
                },
                Row::FileHeader { file: 1 },
                Row::HunkHeader {
                    side: Side::Unstaged,
                    hunk_index: 1
                },
                Row::Line {
                    side: Side::Unstaged,
                    hunk_index: 1,
                    line_index: 0
                },
            ]
        );
        assert_eq!(stacked.unified_starts, [0, 4]);
    }

    #[test]
    fn the_split_stack_starts_each_file_at_its_own_banner() {
        let hunks = [hunk(2), hunk(3)];
        let files = [file(0..1), file(1..2)];
        let stacked = build_stack(&hunks, &files, 0);
        assert_eq!(stacked.split_starts.len(), 2);
        let start = stacked.split_starts[1];
        assert_eq!(
            stacked.split[start],
            SplitRow::Banner(Row::FileHeader { file: 1 })
        );
        assert!(matches!(
            stacked.split[start + 1],
            SplitRow::HunkHeader { hunk_index: 1, .. }
        ));
    }

    #[test]
    fn a_notice_file_has_its_header_and_the_notice_and_no_hunks() {
        let hunks: [Hunk; 0] = [];
        let files = [StackFile {
            hunks: 0..0,
            notice: Some(Notice::Binary),
        }];
        let stacked = build_stack(&hunks, &files, 0);
        assert_eq!(
            stacked.rows,
            [
                Row::FileHeader { file: 0 },
                Row::Notice {
                    file: 0,
                    notice: Notice::Binary
                },
            ]
        );
    }

    #[test]
    fn files_past_the_listed_ones_are_counted_in_one_last_row() {
        let stacked = build_stack(&[], &[], 37);
        assert_eq!(stacked.rows, [Row::MoreFiles { count: 37 }]);
        assert_eq!(
            stacked.split,
            [SplitRow::Banner(Row::MoreFiles { count: 37 })]
        );
    }

    #[test]
    fn a_file_past_the_line_limit_shows_its_first_lines_and_says_how_many_are_left() {
        let hunks = [hunk(FILE_LINE_LIMIT + 1000)];
        let files = [file(0..1)];
        let stacked = build_stack(&hunks, &files, 0);
        let lines = stacked
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Line { .. }))
            .count();
        assert_eq!(lines, FILE_LINE_LIMIT);
        assert_eq!(
            stacked.rows.last(),
            Some(&Row::Notice {
                file: 0,
                notice: Notice::MoreLines(1000)
            })
        );
    }

    #[test]
    fn a_split_file_past_the_line_limit_says_how_many_lines_are_left() {
        let hunks = [hunk(FILE_LINE_LIMIT + 10)];
        let files = [file(0..1)];
        let stacked = build_stack(&hunks, &files, 0);
        assert_eq!(
            stacked.split.last(),
            Some(&SplitRow::Banner(Row::Notice {
                file: 0,
                notice: Notice::MoreLines(10)
            }))
        );
    }

    #[test]
    fn a_file_inside_the_limit_has_no_more_lines_notice() {
        let stacked = build_stack(&[hunk(3)], &[file(0..1)], 0);
        assert!(
            !stacked
                .rows
                .iter()
                .any(|row| matches!(row, Row::Notice { .. }))
        );
    }

    #[test]
    fn concat_keeps_the_order_and_gives_each_file_its_range() {
        let diff = |n: usize| FileDiff {
            path: String::new(),
            old_path: None,
            binary: false,
            hunks: (0..n).map(|_| hunk(1)).collect(),
        };
        let (all, ranges) = concat_hunks(vec![diff(2), diff(0), diff(1)]);
        assert_eq!(all.len(), 3);
        assert_eq!(ranges, [0..2, 2..2, 2..3]);
    }

    #[test]
    fn the_file_at_a_row_is_the_last_file_that_starts_at_or_before_it() {
        let starts = [0, 4, 9];
        assert_eq!(file_at_row(&starts, 0), 0);
        assert_eq!(file_at_row(&starts, 3), 0);
        assert_eq!(file_at_row(&starts, 4), 1);
        assert_eq!(file_at_row(&starts, 100), 2);
        assert_eq!(file_at_row(&[], 5), 0);
    }

    #[test]
    fn forward_goes_to_the_next_file_and_wraps_from_the_last_to_the_first() {
        let starts = [0, 4, 9];
        assert_eq!(step_from_top(&starts, 0, true), Some(1));
        assert_eq!(step_from_top(&starts, 6, true), Some(2));
        assert_eq!(step_from_top(&starts, 9, true), Some(0));
        assert_eq!(step_from_top(&starts, 12, true), Some(0));
        assert_eq!(step_from_top(&[], 0, true), None);
    }

    #[test]
    fn backward_goes_to_the_files_header_first_and_then_to_the_one_before() {
        let starts = [0, 4, 9];
        // Inside file 1, below its header: back to its header.
        assert_eq!(step_from_top(&starts, 6, false), Some(1));
        // At file 1's header: back to file 0.
        assert_eq!(step_from_top(&starts, 4, false), Some(0));
        // At the first file's header: wraps to the last file.
        assert_eq!(step_from_top(&starts, 0, false), Some(2));
        // Inside the first file, below its header: its header comes first.
        assert_eq!(step_from_top(&starts, 2, false), Some(0));
    }

    #[test]
    fn a_single_file_wraps_onto_itself() {
        assert_eq!(step_from_top(&[0], 0, true), Some(0));
        assert_eq!(step_from_top(&[0], 0, false), Some(0));
    }
    #[test]
    fn the_padding_lets_the_last_header_reach_the_top_for_any_tail() {
        // A short last file needs the rest of a page below it; a long one needs none.
        assert_eq!(scroll_padding(3, 20), 17);
        assert_eq!(scroll_padding(20, 20), 0);
        assert_eq!(scroll_padding(100, 20), 0);
        assert_eq!(scroll_padding(1, 1), 0);
        assert_eq!(scroll_padding(5, 0), 0);
        for tail in 1..=60 {
            for visible in 1..=40 {
                let rows = 200 + tail;
                let total = rows + scroll_padding(tail, visible);
                // The scroll reaches `total - visible`; the last header is at `rows - tail`.
                assert!(
                    total - visible >= rows - tail,
                    "tail {tail}, visible {visible}"
                );
            }
        }
    }
}
