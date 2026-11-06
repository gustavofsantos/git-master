//! Workspaces (D174, D183): what the engine's workspace commands ask for and answer with, and the
//! rows `gitmaster workspace` prints. docs/guide/workspaces.md, "Create a workspace" and "Remove a
//! workspace"; docs/reference/cli.md, "Workspaces".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a workspace is kept or removed when the app closes (D174 (3)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Durable,
    Ephemeral,
}

impl WorkspaceKind {
    /// The word the Git key `gitmaster.workspace.kind` holds for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// One workspace under the root, as `gitmaster workspace list` prints it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceRow {
    /// The folder name, which is the name the commands take.
    pub name: String,
    pub kind: WorkspaceKind,
    /// The absolute path of the workspace folder.
    pub path: String,
    /// How many members the workspace's `.gitmodules` lists.
    pub members: usize,
}

/// A member asked for in a new workspace: a repository and the branch the member is checked out
/// on (D178 (1)). `repo` is a folder of the repository, the main checkout or any worktree of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberRequest {
    pub repo: String,
    pub branch: String,
}

/// What happens to a member's branch when the workspace is created (D178 (2)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BranchHow {
    /// The local branch exists and is checked out in the new worktree.
    CheckOut,
    /// A new local branch is created to track the remote branch `remote`.
    TrackRemote { remote: String },
    /// A new local branch is created from `base`, a commit-ish.
    CreateFrom { base: String },
}

/// One member of a planned workspace: the repository, the member's folder name (its path in the
/// workspace), the branch and what happens to the branch. Nothing is written for it yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberPlan {
    /// The main checkout of the repository the member is a worktree of.
    pub repo: String,
    /// The folder name of the member, relative to the workspace.
    pub member: String,
    pub branch: String,
    pub how: BranchHow,
}

/// A reason a workspace cannot be created as planned (D178 (3)). `repo` is the main checkout the
/// problem is about, `None` for a problem between two members; `path` is the folder of the
/// worktree that holds the branch, when another worktree does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceProblem {
    pub repo: Option<String>,
    pub message: String,
    pub path: Option<String>,
}

/// The kind of a thing that stops a workspace from being removed (D174 (3), D31).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemovalBlockerKind {
    /// The workspace contains the directory the command runs in, or the window shows it.
    Current,
    /// A member's repository is gone, so Git cannot remove its worktree.
    MemberRepositoryMissing,
    /// The workspace's own repository has changes other than its members' commits.
    WorkspaceChanges,
    /// A member has staged, unstaged or untracked files.
    MemberDirty,
    /// A member is on a detached HEAD that no local or remote branch contains.
    MemberDetached,
}

/// One thing that blocks a workspace's removal. `overridable` blockers go when the removal is
/// forced; the others always refuse the removal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemovalBlocker {
    pub kind: RemovalBlockerKind,
    /// The member's folder name, for a blocker about one member.
    pub member: Option<String>,
    /// The reason, in words for a person, without the member's name when `member` gives it.
    pub detail: String,
    pub overridable: bool,
}
