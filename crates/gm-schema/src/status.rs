use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What happened to a path on one side (index or working tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BranchInfo {
    /// Branch name; `None` when HEAD is detached.
    pub head: Option<String>,
    /// Commit HEAD points at; `None` on an unborn branch.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// A branch as the branch list shows it. Remote branches are named as Git
/// names them, `origin/feature`; `origin/HEAD` is not listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BranchEntry {
    pub name: String,
    pub is_remote: bool,
    /// The branch HEAD is on. Never set for a remote branch.
    pub is_current: bool,
    /// Full name of the upstream of a local branch, e.g. `origin/main`.
    pub upstream: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusEntry {
    pub path: String,
    /// Source path of a rename or copy.
    pub orig_path: Option<String>,
    /// Staged change (index against HEAD).
    pub staged: Option<Change>,
    /// Unstaged change (working tree against index).
    pub unstaged: Option<Change>,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoStatus {
    /// Absolute path of the working tree root.
    pub root: String,
    pub branch: BranchInfo,
    pub entries: Vec<StatusEntry>,
}

/// A worktree as `git worktree list` reports it. The first entry is the main
/// checkout (or the bare repository itself).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeEntry {
    /// Absolute path of the worktree; for a bare entry, the repository path.
    pub path: String,
    /// Commit HEAD points at; `None` for a bare entry.
    pub head: Option<String>,
    /// Short branch name; `None` when detached or bare.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// The first entry: the main checkout.
    pub is_main: bool,
    /// Why the worktree is locked. `Some("")` when it is locked without a reason.
    pub locked: Option<String>,
    /// Why git may prune the worktree. `Some("")` when no reason was given.
    pub prunable: Option<String>,
}
