use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many commits of HEAD the history lists (H1). Paging comes in H2.
pub const HISTORY_LIMIT: usize = 200;

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
    /// Newest first, at most `HISTORY_LIMIT`.
    pub commits: Vec<CommitSummary>,
    /// HEAD points at a branch that has no commits yet; the list is empty.
    pub unborn: bool,
}
