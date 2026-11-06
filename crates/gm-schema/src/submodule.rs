use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many commits one `LoadSubmodule` answers (D175). When the submodule
/// gained more, the answer says `more`.
pub const SUBMODULE_COMMIT_LIMIT: usize = 200;

/// One commit a submodule gained over the commit the repository records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmoduleCommit {
    pub oid: String,
    /// The first line of the message; empty when the message is empty.
    pub subject: String,
    pub author_name: String,
    /// The author time, in Unix seconds.
    pub author_time: i64,
}

/// The state of one submodule as its row shows it (D175), read without
/// changing anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmoduleDetail {
    /// The path as the request gave it, relative to the repository root.
    pub path: String,
    /// The branch the submodule has checked out; `None` when detached or when
    /// the folder is not checked out.
    pub branch: Option<String>,
    /// HEAD of the submodule is not on a branch. `false` when the folder is not
    /// checked out.
    pub detached: bool,
    /// The commit the submodule's HEAD is at; `None` when the folder is not
    /// checked out or the submodule has no commit yet.
    pub head: Option<String>,
    /// The commit the repository's index records for the path.
    pub recorded: Option<String>,
    /// Whether the folder holds a checkout of the submodule.
    pub checked_out: bool,
    /// Commits of `head` the repository does not record (`recorded..head`),
    /// newest first, at most `SUBMODULE_COMMIT_LIMIT`.
    pub commits: Vec<SubmoduleCommit>,
    /// More commits than `commits` holds follow.
    pub more: bool,
    /// Commits the repository records that the submodule's HEAD does not have
    /// (`head..recorded`): the submodule is behind. Counted, not listed.
    pub missing_commits: usize,
    /// The main checkout of the submodule's repository: the repository a linked
    /// worktree was made from, or the submodule's own folder when its Git
    /// directory is its own. `None` when the folder is not checked out.
    #[serde(default)]
    pub repo: Option<String>,
}
