//! The **Diff** tab's model (D236): the diff of one file on one side, its rows, the cursor
//! over them and the colors of its code. The rows come from `gm_view::diff_rows`, the same
//! model the app's diff pane uses.

use std::collections::{BTreeSet, HashMap};

use gm_schema::{DiffLine, FileDiff, Hunk, LineKind, ReviewComment, SyntaxRole};
use gm_view::diff_rows::{self, Diffs, Row, Side};
use gm_view::review::{self, Entry, Listing, Opened};

use crate::app::Context;

/// A diff larger than this many lines is not colored: the text to parse would be large and
/// the colors are a help, not the content.
pub const COLOR_LINE_LIMIT: usize = 20_000;

/// The side of the diff a context shows.
pub fn side_of(context: Context) -> Side {
    match context {
        Context::Unstaged => Side::Unstaged,
        Context::Staged => Side::Staged,
    }
}

/// The colors of one line: byte ranges of its text and their roles.
pub type LineColors = Vec<(usize, usize, SyntaxRole)>;

/// The text to color for a diff, and where each line of it starts.
#[derive(Debug, PartialEq, Eq)]
pub struct ColorJob {
    /// The view the colors are for; a newer view drops an older job's answer.
    pub generation: u64,
    pub path: String,
    /// The new side of every hunk, context and added lines, one after another.
    pub text: String,
    /// For each line in `text`: its hunk, its index in the hunk and its byte offset.
    pub lines: Vec<(usize, usize, usize)>,
}

/// The diff of the selected file, as the **Diff** tab shows it.
#[derive(Debug)]
pub struct DiffView {
    pub path: String,
    pub context: Context,
    /// `None` until the engine answers.
    pub diff: Option<FileDiff>,
    /// Why the diff could not be read.
    pub error: Option<String>,
    pub rows: Vec<Row>,
    /// What is drawn: the rows with the threads under their lines, as the app's diff lays
    /// them out (`gm_view::review::notes`).
    pub listing: Listing,
    /// Working-tree lines an open thread covers; their gutter is marked.
    pub marked: BTreeSet<u32>,
    /// Index into `rows`.
    pub cursor: usize,
    /// The first entry of `listing` on screen.
    pub scroll: usize,
    /// The next draw brings the cursor on screen; the wheel clears it.
    pub follow_cursor: bool,
    /// The colors of the lines, by hunk and line index; empty until they are computed.
    pub colors: HashMap<(usize, usize), LineColors>,
    pub generation: u64,
    /// The other end of the line selection; the cursor is this end. `None`: no selection.
    pub anchor: Option<usize>,
    /// The screen row of the first diff row, as the last draw placed it; a drag reads rows
    /// against it.
    pub body_top: u16,
}

impl DiffView {
    pub fn new(path: String, context: Context, generation: u64) -> Self {
        Self {
            path,
            context,
            diff: None,
            error: None,
            rows: Vec::new(),
            listing: Listing::default(),
            marked: BTreeSet::new(),
            cursor: 0,
            scroll: 0,
            follow_cursor: true,
            colors: HashMap::new(),
            generation,
            anchor: None,
            body_top: 0,
        }
    }

    /// Takes in the engine's diff. The cursor stays on the same row number, so a refresh
    /// after an edit keeps the place; the colors are dropped until the new ones arrive.
    pub fn load(&mut self, diff: FileDiff) {
        let side = side_of(self.context);
        let (unstaged, staged) = match side {
            Side::Unstaged => (diff.hunks.as_slice(), &[][..]),
            Side::Staged => (&[][..], diff.hunks.as_slice()),
        };
        self.rows = diff_rows::build_rows(Diffs { unstaged, staged }, side);
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.anchor = self
            .anchor
            .map(|anchor| anchor.min(self.rows.len().saturating_sub(1)));
        if self.diff.is_none() {
            self.cursor = self.first_line().unwrap_or(0);
        }
        self.diff = Some(diff);
        self.error = None;
        self.colors.clear();
    }

    /// The hunks of the side shown, as `gm_view` reads them.
    pub fn diffs(&self) -> Diffs<'_> {
        let hunks = self.hunks();
        match side_of(self.context) {
            Side::Unstaged => Diffs {
                unstaged: hunks,
                staged: &[],
            },
            Side::Staged => Diffs {
                unstaged: &[],
                staged: hunks,
            },
        }
    }

    /// Lays the rows out again with the threads of `comments` under their lines.
    pub fn relayout(&mut self, comments: &[ReviewComment], opened: Opened<'_>) {
        let diffs = self.diffs();
        let split = diff_rows::split_rows(diffs, side_of(self.context));
        let notes = review::notes(&self.rows, diffs, &split, comments, &self.path, opened);
        self.listing = notes.unified;
        self.marked = notes.marked;
    }

    /// The entry of row `row` in the listing.
    pub fn entry_of(&self, row: usize) -> usize {
        self.listing.at.get(row).copied().unwrap_or(row)
    }

    /// The row an entry belongs to: its own for a diff row, else the diff row above it.
    pub fn row_of_entry(&self, entry: usize) -> Option<usize> {
        self.listing.entries[..=entry.min(self.listing.entries.len().checked_sub(1)?)]
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Diff(row) => Some(*row),
                _ => None,
            })
    }

    /// The thread drawn first under row `row`, if any.
    pub fn thread_under(&self, row: usize) -> Option<i64> {
        let at = self.listing.at.get(row)?;
        match self.listing.entries.get(at + 1)? {
            Entry::Card { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// Moves the cursor to the next (`forward`) or the previous row with a thread under it.
    pub fn jump_thread(&mut self, forward: bool) {
        let has_thread = |row: &usize| self.thread_under(*row).is_some();
        let found = if forward {
            (self.cursor + 1..self.rows.len()).find(has_thread)
        } else {
            (0..self.cursor).rev().find(has_thread)
        };
        if let Some(row) = found {
            self.cursor = row;
            self.follow_cursor = true;
        }
    }

    pub fn hunks(&self) -> &[Hunk] {
        self.diff.as_ref().map_or(&[], |diff| diff.hunks.as_slice())
    }

    pub fn hunk(&self, index: usize) -> Option<&Hunk> {
        self.hunks().get(index)
    }

    pub fn line(&self, hunk: usize, line: usize) -> Option<&DiffLine> {
        self.hunk(hunk)?.lines.get(line)
    }

    /// The hunk the cursor is in, if any.
    pub fn cursor_hunk(&self) -> Option<usize> {
        self.rows.get(self.cursor)?.hunk().map(|(_, hunk)| hunk)
    }

    /// Lines added and removed over the whole diff.
    pub fn counts(&self) -> (usize, usize) {
        self.hunks()
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .fold((0, 0), |(added, removed), line| match line.kind {
                LineKind::Added => (added + 1, removed),
                LineKind::Removed => (added, removed + 1),
                LineKind::Context => (added, removed),
            })
    }

    fn first_line(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Line { .. }))
    }

    /// Moves the cursor `by` rows, stopping at either end.
    pub fn move_cursor(&mut self, by: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(by).min(last);
        self.follow_cursor = true;
    }

    /// Moves the cursor to the header of the next (`forward`) or the previous hunk. Going
    /// back from inside a hunk goes to its own header first.
    pub fn jump_hunk(&mut self, forward: bool) {
        let is_header = |row: &Row| matches!(row, Row::HunkHeader { .. });
        let found = if forward {
            self.rows
                .iter()
                .enumerate()
                .skip(self.cursor + 1)
                .find(|(_, row)| is_header(row))
        } else {
            self.rows
                .iter()
                .enumerate()
                .take(self.cursor)
                .rev()
                .find(|(_, row)| is_header(row))
        };
        if let Some((index, _)) = found {
            self.cursor = index;
            self.follow_cursor = true;
        }
    }

    /// Starts a selection at the cursor, or ends the one there is.
    pub fn toggle_selection(&mut self) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    /// Puts the cursor at the other end of the selection.
    pub fn swap_ends(&mut self) {
        if let Some(anchor) = self.anchor.as_mut() {
            std::mem::swap(anchor, &mut self.cursor);
            self.follow_cursor = true;
        }
    }

    /// The rows the selection covers, first and last; `None` without a selection.
    pub fn selected_rows(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn is_selected(&self, row: usize) -> bool {
        self.selected_rows()
            .is_some_and(|(first, last)| (first..=last).contains(&row))
    }

    /// The selected lines that stage: for each hunk the selection touches, the indices of its
    /// added and removed lines in the selection. A hunk with only context selected is left
    /// out.
    pub fn selected_changes(&self) -> Vec<(usize, Vec<u32>)> {
        let Some((first, last)) = self.selected_rows() else {
            return Vec::new();
        };
        let mut changes: Vec<(usize, Vec<u32>)> = Vec::new();
        for row in &self.rows[first..=last] {
            let Row::Line {
                hunk_index,
                line_index,
                ..
            } = *row
            else {
                continue;
            };
            let is_change = self
                .line(hunk_index, line_index)
                .is_some_and(|line| line.kind != LineKind::Context);
            let Ok(index) = u32::try_from(line_index) else {
                continue;
            };
            if !is_change {
                continue;
            }
            match changes.last_mut() {
                Some((hunk, lines)) if *hunk == hunk_index => lines.push(index),
                _ => changes.push((hunk_index, vec![index])),
            }
        }
        changes
    }

    /// The selection as the header names it: the first and last line numbers in the file
    /// and how many lines it holds, such as `44–48 · 5 lines`.
    pub fn selection_label(&self) -> Option<String> {
        let (first, last) = self.selected_rows()?;
        let numbers: Vec<u32> = self.rows[first..=last]
            .iter()
            .filter_map(|row| match *row {
                Row::Line {
                    hunk_index,
                    line_index,
                    ..
                } => self
                    .line(hunk_index, line_index)
                    .and_then(|line| line.new_no.or(line.old_no)),
                _ => None,
            })
            .collect();
        let count = self.rows[first..=last]
            .iter()
            .filter(|row| matches!(row, Row::Line { .. }))
            .count();
        let noun = if count == 1 { "line" } else { "lines" };
        Some(match (numbers.first(), numbers.last()) {
            (Some(start), Some(end)) if start != end => format!("{start}–{end} · {count} {noun}"),
            (Some(start), _) => format!("{start} · {count} {noun}"),
            _ => format!("{count} {noun}"),
        })
    }

    /// Moves `scroll` so the cursor is on screen in `height` rows.
    pub fn keep_cursor_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        let entries = self.listing.entries.len();
        self.scroll = self.scroll.min(entries.saturating_sub(height));
        if !self.follow_cursor {
            return;
        }
        let at = self.entry_of(self.cursor);
        if at < self.scroll {
            self.scroll = at;
        } else if at >= self.scroll + height {
            self.scroll = at + 1 - height;
        }
    }

    pub fn scroll_by(&mut self, by: isize, height: usize) {
        let bottom = self.listing.entries.len().saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(by).min(bottom);
        self.follow_cursor = false;
    }

    /// The text to color, or `None` for a diff too large to color or with nothing to color.
    pub fn color_job(&self) -> Option<ColorJob> {
        let line_count: usize = self.hunks().iter().map(|hunk| hunk.lines.len()).sum();
        if line_count == 0 || line_count > COLOR_LINE_LIMIT {
            return None;
        }
        let mut text = String::new();
        let mut lines = Vec::new();
        for (hunk_index, hunk) in self.hunks().iter().enumerate() {
            for (line_index, line) in hunk.lines.iter().enumerate() {
                if line.kind != LineKind::Removed {
                    lines.push((hunk_index, line_index, text.len()));
                    text.push_str(&line.text);
                    text.push('\n');
                }
            }
        }
        Some(ColorJob {
            generation: self.generation,
            path: self.path.clone(),
            text,
            lines,
        })
    }

    /// Takes in the colors of `job`'s text, as byte ranges of the whole text, and splits
    /// them over its lines.
    pub fn take_colors(&mut self, job: &ColorJob, spans: &[gm_schema::HighlightSpan]) {
        if job.generation != self.generation {
            return;
        }
        self.colors = split_colors(job, spans);
    }
}

/// The spans of a whole text cut at each line, as ranges of the line's own text.
pub fn split_colors(
    job: &ColorJob,
    spans: &[gm_schema::HighlightSpan],
) -> HashMap<(usize, usize), LineColors> {
    let mut colors: HashMap<(usize, usize), LineColors> = HashMap::new();
    for (index, &(hunk, line, start)) in job.lines.iter().enumerate() {
        let end = job
            .lines
            .get(index + 1)
            .map_or(job.text.len(), |next| next.2)
            .saturating_sub(1);
        let own: LineColors = spans
            .iter()
            .filter(|span| span.start < end && span.end > start)
            .map(|span| {
                (
                    span.start.max(start) - start,
                    span.end.min(end) - start,
                    span.role,
                )
            })
            .collect();
        if !own.is_empty() {
            colors.insert((hunk, line), own);
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{HighlightSpan, HunkId};

    fn line(kind: LineKind, text: &str, old_no: Option<u32>, new_no: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_no,
            new_no,
            text: text.to_owned(),
            no_newline: false,
        }
    }

    fn hunk(id: &str, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            id: HunkId(id.to_owned()),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 0,
            header: String::new(),
            lines,
        }
    }

    fn view() -> DiffView {
        let mut view = DiffView::new("a.rs".to_owned(), Context::Unstaged, 7);
        view.load(FileDiff {
            path: "a.rs".to_owned(),
            old_path: None,
            binary: false,
            hunks: vec![
                hunk(
                    "h1",
                    vec![
                        line(LineKind::Context, "fn a() {", Some(1), Some(1)),
                        line(LineKind::Removed, "    old();", Some(2), None),
                        line(LineKind::Added, "    new();", None, Some(2)),
                    ],
                ),
                hunk(
                    "h2",
                    vec![line(LineKind::Added, "fn b() {}", None, Some(9))],
                ),
            ],
        });
        let none = BTreeSet::new();
        view.relayout(
            &[],
            Opened {
                outdated: false,
                comments: &none,
                threads: &none,
            },
        );
        view
    }

    #[test]
    fn a_new_diff_puts_the_cursor_on_its_first_line_and_counts_its_changes() {
        let view = view();
        assert_eq!(view.rows.len(), 6);
        assert_eq!(view.cursor, 1);
        assert_eq!(view.cursor_hunk(), Some(0));
        assert_eq!(view.counts(), (2, 1));
    }

    #[test]
    fn hunk_jumps_go_between_headers() {
        let mut view = view();
        view.jump_hunk(true);
        assert_eq!(view.cursor, 4);
        assert_eq!(view.cursor_hunk(), Some(1));
        view.jump_hunk(true);
        assert_eq!(view.cursor, 4);
        view.move_cursor(1);
        view.jump_hunk(false);
        assert_eq!(view.cursor, 4);
        view.jump_hunk(false);
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn a_refreshed_diff_keeps_the_cursor_row() {
        let mut view = view();
        view.move_cursor(2);
        let diff = view.diff.clone().unwrap();
        view.load(diff);
        assert_eq!(view.cursor, 3);
    }

    #[test]
    fn the_color_text_holds_the_new_side_and_colors_split_per_line() {
        let view = view();
        let job = view.color_job().unwrap();
        assert_eq!(job.text, "fn a() {\n    new();\nfn b() {}\n");
        assert_eq!(job.lines, [(0, 0, 0), (0, 2, 9), (1, 0, 20)]);
        let spans = [
            // `fn` on the first line, `new` on the second, a span over two lines.
            HighlightSpan {
                start: 0,
                end: 2,
                role: SyntaxRole::Keyword,
            },
            HighlightSpan {
                start: 13,
                end: 16,
                role: SyntaxRole::Function,
            },
            HighlightSpan {
                start: 15,
                end: 22,
                role: SyntaxRole::Comment,
            },
        ];
        let colors = split_colors(&job, &spans);
        assert_eq!(colors[&(0, 0)], [(0, 2, SyntaxRole::Keyword)]);
        assert_eq!(
            colors[&(0, 2)],
            [(4, 7, SyntaxRole::Function), (6, 10, SyntaxRole::Comment)]
        );
        assert_eq!(colors[&(1, 0)], [(0, 2, SyntaxRole::Comment)]);
    }

    #[test]
    fn a_selection_over_two_hunks_stages_the_changed_lines_of_each() {
        let mut view = view();
        assert_eq!(view.cursor, 1); // the context line of the first hunk
        view.toggle_selection();
        view.move_cursor(4); // the added line of the second hunk
        assert_eq!(view.selected_rows(), Some((1, 5)));
        assert_eq!(view.selected_changes(), [(0, vec![1, 2]), (1, vec![0])]);
        assert_eq!(view.selection_label().as_deref(), Some("1–9 · 4 lines"));
        view.swap_ends();
        assert_eq!(view.cursor, 1);
        assert_eq!(view.anchor, Some(5));
        view.toggle_selection();
        assert_eq!(view.selected_rows(), None);
        assert!(view.selected_changes().is_empty());
    }

    #[test]
    fn a_selection_of_context_only_stages_nothing() {
        let mut view = view();
        view.toggle_selection();
        assert_eq!(view.selected_changes(), []);
        assert_eq!(view.selection_label().as_deref(), Some("1 · 1 line"));
    }

    #[test]
    fn colors_of_an_older_view_are_dropped() {
        let mut view = view();
        let mut job = view.color_job().unwrap();
        job.generation = 6;
        let span = HighlightSpan {
            start: 0,
            end: 2,
            role: SyntaxRole::Keyword,
        };
        view.take_colors(&job, &[span]);
        assert!(view.colors.is_empty());
    }
}
