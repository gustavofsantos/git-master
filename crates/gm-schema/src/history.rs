use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::status::Change;

/// How many commits one page of the history holds (H1, H2).
pub const HISTORY_PAGE: usize = 200;

/// What a commit row shows of one ref that points at it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RefLabel {
    /// The short name: `HEAD`, `main`, `origin/main` or `v1.0`.
    pub name: String,
    pub kind: RefKind,
}

/// The kinds of ref label. Git's decoration does not say whether a branch is
/// local or remote, so a branch name covers both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// The `HEAD` marker, listed before the branch it points at.
    Head,
    Branch,
    Tag,
}

/// One commit as the history list shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitSummary {
    /// Full object id.
    pub oid: String,
    /// Full object ids of the parents, first parent first. Empty for a root
    /// commit. Feeds the graph lanes (H5).
    pub parents: Vec<String>,
    /// The abbreviated id Git prints for the commit.
    pub short: String,
    /// The first line of the message, as Git gives it (`%s`).
    pub subject: String,
    /// The refs that point at this commit, HEAD first.
    pub refs: Vec<RefLabel>,
    /// Author time, in seconds since the Unix epoch.
    pub author_time: i64,
}

/// The answer to `LoadHistory`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct History {
    /// Newest first, at most `HISTORY_PAGE`.
    pub commits: Vec<CommitSummary>,
    /// HEAD points at a branch that has no commits yet; the list is empty.
    pub unborn: bool,
    /// Commits of HEAD follow this page; ask for the next one after the last.
    pub more: bool,
}

/// How many changed files a commit's detail lists. The rest are counted in
/// `CommitDetail::more_files` (H3).
pub const DETAIL_FILE_LIMIT: usize = 200;

/// One commit as the detail panel shows it (H3). Read with `git show`, never
/// logged (D28).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitDetail {
    /// Full object id.
    pub oid: String,
    /// The abbreviated id Git prints for the commit.
    pub short: String,
    /// The first line of the message (`%s`).
    pub subject: String,
    /// The message after the subject and its blank line, paragraphs kept.
    /// Empty when the message has no body.
    pub body: String,
    pub author_name: String,
    pub author_email: String,
    /// The author date as Git prints it, with the author's own offset (`%ai`).
    pub author_date: String,
    /// The parents, abbreviated, first parent first. Empty for a root commit.
    pub parents: Vec<String>,
    /// The files changed against the first parent (against the empty tree for
    /// a root commit), in Git's order, at most `DETAIL_FILE_LIMIT`.
    pub files: Vec<CommitFile>,
    /// Changed files past the limit. They are counted, not listed.
    pub more_files: usize,
}

/// One file changed by a commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitFile {
    /// The path after the commit (the only path, unless renamed).
    pub path: String,
    /// The path before the commit, for a rename.
    pub old_path: Option<String>,
    pub change: Change,
    /// Lines added; zero for a binary file.
    pub added: u32,
    /// Lines removed; zero for a binary file.
    pub deleted: u32,
    /// Git reports no line counts for a binary file.
    pub binary: bool,
}
