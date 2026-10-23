use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a moved file goes (D109). A branch gets a commit; a worktree gets the
/// change on its disk, unstaged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MoveTarget {
    /// A local branch. `create` makes it from the current HEAD; without it the
    /// branch must exist.
    Branch { name: String, create: bool },
    /// The linked worktree whose folder is `path`, as `git worktree list` names it.
    Worktree { path: String },
}

/// Why a move is refused. The engine lists every blocker that applies, in
/// `order_blockers` order, so the human sees all of them at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MoveBlocker {
    /// The path has no change in the Working Copy.
    NothingToMove,
    /// The path names a folder, as an untracked folder does in the status. A
    /// folder cannot be moved as one in v1; its files move one at a time.
    IsFolder,
    /// The path is conflicted in the source.
    Conflicted,
    /// The target is the branch of the window.
    SameBranch,
    /// The target is the worktree of the window.
    SameWorktree,
    /// `create` is set but the branch already exists.
    BranchExists,
    /// `create` is not set and there is no such local branch.
    BranchNotFound,
    /// The branch name breaks a rule of Git; `reason` is the sentence the user reads.
    InvalidName { reason: String },
    /// The branch is checked out in another worktree at `path`.
    BranchCheckedOutElsewhere { path: String },
    /// No linked worktree has that folder.
    WorktreeNotFound,
    /// The worktree's folder is gone, or Git would prune it.
    WorktreeMissing,
    /// A symbolic link, or a path that leaves the worktree.
    UnsafePath,
    /// The installed Git is older than the floor for this feature (2.38).
    GitTooOld { found: String },
    /// The three-way merge of the file has conflicts; `paths` lists them.
    Conflict { paths: Vec<String> },
    /// A binary file, and the target's copy differs from the base.
    BinaryDiffers,
    /// A new file over an existing file that has other content.
    AddOverExisting,
}

/// The answer to `PreviewMoveChanges`: what would stop the move. An empty list
/// means the move would work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MovePreview {
    pub blockers: Vec<MoveBlocker>,
}

/// What a move did (D109). A branch target names the commit it made; `kept`
/// says why the source file stayed, when it did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveOutcome {
    /// The commit on the target branch; `None` for a worktree target.
    pub commit: Option<String>,
    /// The reason the source file was kept, when it was not discarded.
    pub kept: Option<String>,
}
