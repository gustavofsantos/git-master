//! What the companion knows of the repository, as the engine last reported it (D241): the
//! status, the line counts, the review threads, the branch's changes against its base, and
//! the diffs of the files a screen unfolded, with the colors of their code.

use std::collections::HashMap;

use gm_schema::{
    ChangeStats, DiffSide, FileDiff, HighlightSpan, LineKind, RepoStatus, ReviewComment,
    RevisionRange, SyntaxRole,
};
use gm_view::diff_rows::{self, Diffs, Row, Side};

/// A diff larger than this many lines is not colored.
pub const COLOR_LINE_LIMIT: usize = 20_000;

/// The colors of one line: byte ranges of its text and their roles.
pub type LineColors = Vec<(usize, usize, SyntaxRole)>;

/// Where a file's changes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Untracked,
    Unstaged,
    Staged,
    /// The branch's commits against its base (D237).
    Branch,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Untracked => "Untracked files",
            Group::Unstaged => "Unstaged changes",
            Group::Staged => "Staged changes",
            Group::Branch => "Branch changes",
        }
    }

    /// The side `gm_view` reads its diff as: the working tree for untracked and unstaged
    /// files, the index for staged ones; the branch's diff is read like a commit's.
    pub fn side(self) -> Side {
        match self {
            Group::Untracked | Group::Unstaged => Side::Unstaged,
            Group::Staged | Group::Branch => Side::Staged,
        }
    }

    /// The engine's side for `LoadDiff`; `None` for the branch, read in its range.
    pub fn diff_side(self) -> Option<DiffSide> {
        match self {
            Group::Untracked | Group::Unstaged => Some(DiffSide::Unstaged),
            Group::Staged => Some(DiffSide::Staged),
            Group::Branch => None,
        }
    }

    /// Whether its changes can be staged (`Some(true)`) or unstaged (`Some(false)`).
    pub fn stages(self) -> Option<bool> {
        match self {
            Group::Untracked | Group::Unstaged => Some(true),
            Group::Staged => Some(false),
            Group::Branch => None,
        }
    }
}

/// The diff of one file of one group, its rows as `gm_view` lays them out, and its colors.
#[derive(Clone, Debug, Default)]
pub struct FileData {
    pub diff: Option<FileDiff>,
    pub rows: Vec<Row>,
    pub colors: HashMap<(usize, usize), LineColors>,
    /// Why the diff could not be read.
    pub error: Option<String>,
}

impl FileData {
    pub fn load(&mut self, diff: FileDiff, side: Side) {
        let hunks = diff.hunks.as_slice();
        let diffs = match side {
            Side::Unstaged => Diffs {
                unstaged: hunks,
                staged: &[],
            },
            Side::Staged => Diffs {
                unstaged: &[],
                staged: hunks,
            },
        };
        self.rows = diff_rows::build_rows(diffs, side);
        self.diff = Some(diff);
        self.error = None;
        self.colors.clear();
    }

    pub fn diffs(&self, side: Side) -> Diffs<'_> {
        let hunks = self
            .diff
            .as_ref()
            .map_or(&[][..], |diff| diff.hunks.as_slice());
        match side {
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
}

/// The repository as last reported.
#[derive(Debug, Default)]
pub struct Repo {
    pub status: RepoStatus,
    pub stats: ChangeStats,
    pub comments: Vec<ReviewComment>,
    pub range: Option<RevisionRange>,
    pub range_error: Option<String>,
    /// The diffs read, by group and path.
    pub files: HashMap<(Group, String), FileData>,
}

impl Repo {
    pub fn comment(&self, id: i64) -> Option<&ReviewComment> {
        self.comments.iter().find(|comment| comment.id == id)
    }
}

/// The text to color for a file's diff, and where each line of it starts.
#[derive(Debug, PartialEq, Eq)]
pub struct ColorJob {
    pub group: Group,
    pub path: String,
    /// The new side of every hunk, context and added lines, one after another.
    pub text: String,
    /// For each line in `text`: its hunk, its index in the hunk and its byte offset.
    pub lines: Vec<(usize, usize, usize)>,
}

/// The text to color for `diff`, or `None` when it is too large or has nothing to color.
pub fn color_job(group: Group, diff: &FileDiff) -> Option<ColorJob> {
    let count: usize = diff.hunks.iter().map(|hunk| hunk.lines.len()).sum();
    if count == 0 || count > COLOR_LINE_LIMIT {
        return None;
    }
    let mut text = String::new();
    let mut lines = Vec::new();
    for (hunk_index, hunk) in diff.hunks.iter().enumerate() {
        for (line_index, line) in hunk.lines.iter().enumerate() {
            if line.kind != LineKind::Removed {
                lines.push((hunk_index, line_index, text.len()));
                text.push_str(&line.text);
                text.push('\n');
            }
        }
    }
    Some(ColorJob {
        group,
        path: diff.path.clone(),
        text,
        lines,
    })
}

/// The spans of a whole text cut at each line, as ranges of the line's own text.
pub fn split_colors(
    job: &ColorJob,
    spans: &[HighlightSpan],
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
    use gm_schema::{DiffLine, Hunk, HunkId};

    #[test]
    fn colors_cover_the_new_side_and_split_per_line() {
        let line = |kind, text: &str| DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.to_owned(),
            no_newline: false,
        };
        let diff = FileDiff {
            path: "a.rs".to_owned(),
            old_path: None,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h".to_owned()),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 2,
                header: String::new(),
                lines: vec![
                    line(LineKind::Context, "fn a() {"),
                    line(LineKind::Removed, "old"),
                    line(LineKind::Added, "    new();"),
                ],
            }],
        };
        let job = color_job(Group::Unstaged, &diff).unwrap();
        assert_eq!(job.text, "fn a() {\n    new();\n");
        let spans = [
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
        ];
        let colors = split_colors(&job, &spans);
        assert_eq!(colors[&(0, 0)], [(0, 2, SyntaxRole::Keyword)]);
        assert_eq!(colors[&(0, 2)], [(4, 7, SyntaxRole::Function)]);
    }
}
