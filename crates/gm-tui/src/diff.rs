//! The **Diff** tab's model (D236): the diff of one file on one side, its rows, the cursor
//! over them and the colors of its code. The rows come from `gm_view::diff_rows`, the same
//! model the app's diff pane uses.

use std::collections::HashMap;

use gm_schema::{DiffLine, FileDiff, Hunk, LineKind, SyntaxRole};
use gm_view::diff_rows::{self, Diffs, Row, Side};

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
    /// Index into `rows`.
    pub cursor: usize,
    /// The first row on screen.
    pub scroll: usize,
    /// The next draw brings the cursor on screen; the wheel clears it.
    pub follow_cursor: bool,
    /// The colors of the lines, by hunk and line index; empty until they are computed.
    pub colors: HashMap<(usize, usize), LineColors>,
    pub generation: u64,
}

impl DiffView {
    pub fn new(path: String, context: Context, generation: u64) -> Self {
        Self {
            path,
            context,
            diff: None,
            error: None,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow_cursor: true,
            colors: HashMap::new(),
            generation,
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
        if self.diff.is_none() {
            self.cursor = self.first_line().unwrap_or(0);
        }
        self.diff = Some(diff);
        self.error = None;
        self.colors.clear();
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

    /// Moves `scroll` so the cursor is on screen in `height` rows.
    pub fn keep_cursor_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(height));
        if !self.follow_cursor {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }

    pub fn scroll_by(&mut self, by: isize, height: usize) {
        let bottom = self.rows.len().saturating_sub(height);
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
