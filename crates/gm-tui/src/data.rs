//! What the companion knows of the repository, as the engine last reported it (D241): the
//! status, the line counts, the review threads, the branch's changes against its base, and
//! the diffs of the files a screen unfolded, with the colors of their code.

use std::collections::HashMap;

use gm_core::syntax::language_for_path;
use gm_schema::{
    BlameEntry, ChangeStats, CommitDetail, DiffSide, FileBody, FileDiff, FileView, HighlightSpan,
    History, LineKind, RepoStatus, ReviewComment, RevisionRange, StackSnapshot, StashEntry,
    SyntaxRole,
};
use gm_view::diff_rows::{self, Diffs, Row, Side};
use gm_view::provenance_view::ProvenancePanel;

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
    /// The commit the Diff screen shows (`Repo::commit`).
    Commit,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Untracked => "Untracked files",
            Group::Unstaged => "Unstaged changes",
            Group::Staged => "Staged changes",
            Group::Branch => "Branch changes",
            Group::Commit => "Commit",
        }
    }

    /// The side `gm_view` reads its diff as: the working tree for untracked and unstaged
    /// files, the index for staged ones; the branch's diff is read like a commit's.
    pub fn side(self) -> Side {
        match self {
            Group::Untracked | Group::Unstaged => Side::Unstaged,
            Group::Staged | Group::Branch | Group::Commit => Side::Staged,
        }
    }

    /// The engine's side for `LoadDiff`; `None` for the branch, read in its range.
    pub fn diff_side(self) -> Option<DiffSide> {
        match self {
            Group::Untracked | Group::Unstaged => Some(DiffSide::Unstaged),
            Group::Staged => Some(DiffSide::Staged),
            Group::Branch | Group::Commit => None,
        }
    }

    /// Whether its changes can be staged (`Some(true)`) or unstaged (`Some(false)`).
    pub fn stages(self) -> Option<bool> {
        match self {
            Group::Untracked | Group::Unstaged => Some(true),
            Group::Staged => Some(false),
            Group::Branch | Group::Commit => None,
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
    /// The branch's own commits, `base..tip` of `range`, newest first.
    pub commits: Option<History>,
    pub stashes: Vec<StashEntry>,
    pub stacks: StackSnapshot,
    /// The commit the Diff screen shows, with its files.
    pub commit: Option<CommitDetail>,
    /// The file the File screen shows.
    pub file: Option<FileText>,
    /// The file the Blame screen shows, and who last changed each of its lines.
    pub blame: Option<(String, Vec<BlameEntry>)>,
    /// The colors of the blamed lines, by index (D251 (4)).
    pub blame_colors: HashMap<usize, LineColors>,
    /// The question the Origin screen asks and its answer, as the app's panel keeps them.
    pub origin: ProvenancePanel,
}

/// A file of the working tree, cut into lines, with the colors of its code.
#[derive(Debug, Default)]
pub struct FileText {
    pub path: String,
    pub lines: Vec<String>,
    /// The colors of each line, by line index.
    pub colors: HashMap<usize, LineColors>,
    /// Why there are no lines to show: binary, missing, a folder, a link.
    pub note: Option<String>,
    /// The text is the first part of a larger file.
    pub truncated: bool,
}

impl FileText {
    pub fn from_view(view: FileView) -> Self {
        let mut file = FileText {
            path: view.path,
            truncated: view.truncated,
            ..FileText::default()
        };
        match view.body {
            FileBody::Text { text } => file.lines = text.lines().map(str::to_owned).collect(),
            FileBody::Binary => {
                file.note = Some("Binary file: its contents are not shown.".to_owned());
            }
            FileBody::Missing => {
                file.note = Some("There is no such file in the working tree.".to_owned());
            }
            FileBody::Symlink { target } => {
                file.note = Some(format!("A symbolic link to {target}."));
            }
            FileBody::Directory => file.note = Some("This path is a folder.".to_owned()),
        }
        file
    }

    /// Takes the colors of the whole text, cut at each line.
    pub fn colored(&mut self, spans: &[HighlightSpan]) {
        let mut starts = Vec::with_capacity(self.lines.len());
        let mut at = 0;
        for line in &self.lines {
            starts.push(at);
            at += line.len() + 1;
        }
        self.colors.clear();
        for (index, line) in self.lines.iter().enumerate() {
            let (start, end) = (starts[index], starts[index] + line.len());
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
                self.colors.insert(index, own);
            }
        }
    }
}

impl Repo {
    pub fn comment(&self, id: i64) -> Option<&ReviewComment> {
        self.comments.iter().find(|comment| comment.id == id)
    }
}

/// Where a line's colors are kept: a diff's hunk and line, or a line's index and 0.
pub type LineKey = (usize, usize);

/// What a color job colors (D251 (2)). A new job stops only the job of the same target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColorTarget {
    /// One side of a file's diff: the old (context and removed lines) or the new (context
    /// and added lines). One text holding both sides would not parse.
    Diff {
        group: Group,
        path: String,
        old: bool,
    },
    /// The lines the Blame screen shows.
    Blame,
    /// A thread's Before or After lines.
    Thread { id: i64, after: bool },
    /// One side of an agent edit's patch.
    Patch { edit: i64, old: bool },
}

/// A text to color, what it colors, and where each of its lines starts.
#[derive(Debug, PartialEq, Eq)]
pub struct ColorJob {
    pub target: ColorTarget,
    /// The file the text comes from, which names its language.
    pub path: String,
    pub text: String,
    /// For each line in `text`: where its colors go (`None` for a line that is there only
    /// for the parse) and its byte offset.
    pub lines: Vec<(Option<LineKey>, usize)>,
}

impl ColorJob {
    pub fn new(target: ColorTarget, path: &str) -> Self {
        Self {
            target,
            path: path.to_owned(),
            text: String::new(),
            lines: Vec::new(),
        }
    }

    pub fn push(&mut self, key: Option<LineKey>, line: &str) {
        self.lines.push((key, self.text.len()));
        self.text.push_str(line);
        self.text.push('\n');
    }

    /// The job, or `None` when it has nothing to color, is too large or has no grammar.
    pub fn finish(self) -> Option<Self> {
        let empty = self.lines.iter().all(|(key, _)| key.is_none());
        let fits = self.lines.len() <= COLOR_LINE_LIMIT;
        (!empty && fits && language_for_path(&self.path).is_some()).then_some(self)
    }
}

/// The texts to color for `diff`: its new side, and its old side when it removes lines.
pub fn diff_color_jobs(group: Group, diff: &FileDiff) -> Vec<ColorJob> {
    let count: usize = diff.hunks.iter().map(|hunk| hunk.lines.len()).sum();
    if count > COLOR_LINE_LIMIT {
        return Vec::new();
    }
    [false, true]
        .into_iter()
        .filter_map(|old| {
            let target = ColorTarget::Diff {
                group,
                path: diff.path.clone(),
                old,
            };
            let mut job = ColorJob::new(target, &diff.path);
            for (hunk_index, hunk) in diff.hunks.iter().enumerate() {
                for (line_index, line) in hunk.lines.iter().enumerate() {
                    let key = Some((hunk_index, line_index));
                    match (line.kind, old) {
                        (LineKind::Context, _) => job.push(key.filter(|_| !old), &line.text),
                        (LineKind::Added, false) | (LineKind::Removed, true) => {
                            job.push(key, &line.text);
                        }
                        _ => {}
                    }
                }
            }
            job.finish()
        })
        .collect()
}

/// The text to color for the Blame screen's lines: the file as blamed, line by line.
pub fn blame_color_job(path: &str, lines: &[BlameEntry]) -> Option<ColorJob> {
    let mut job = ColorJob::new(ColorTarget::Blame, path);
    for (index, line) in lines.iter().enumerate() {
        job.push(Some((index, 0)), &line.text);
    }
    job.finish()
}

/// The spans of a whole text cut at each line, as ranges of the line's own text.
pub fn split_colors(job: &ColorJob, spans: &[HighlightSpan]) -> HashMap<LineKey, LineColors> {
    let mut colors: HashMap<LineKey, LineColors> = HashMap::new();
    for (index, &(key, start)) in job.lines.iter().enumerate() {
        let Some(key) = key else {
            continue;
        };
        let end = job
            .lines
            .get(index + 1)
            .map_or(job.text.len(), |next| next.1)
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
            colors.insert(key, own);
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{DiffLine, Hunk, HunkId};

    #[test]
    fn colors_cover_both_sides_and_split_per_line() {
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
        let jobs = diff_color_jobs(Group::Unstaged, &diff);
        let [job, old] = jobs.as_slice() else {
            panic!("two sides: {jobs:?}");
        };
        assert_eq!(job.text, "fn a() {\n    new();\n");
        // The old side keeps its context for the parse but colors only the removed line.
        assert_eq!(old.text, "fn a() {\nold\n");
        assert_eq!(old.lines, [(None, 0), (Some((0, 1)), 9)]);
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
        let colors = split_colors(job, &spans);
        assert_eq!(colors[&(0, 0)], [(0, 2, SyntaxRole::Keyword)]);
        assert_eq!(colors[&(0, 2)], [(4, 7, SyntaxRole::Function)]);
    }
}
