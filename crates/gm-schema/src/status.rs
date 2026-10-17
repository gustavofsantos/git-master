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

/// Why `RemoveWorktree` refuses a worktree (D31). The engine derives them from
/// the facts of one inspection; the order here is the order they are listed in.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    /// The main checkout. Never removed.
    Main,
    /// The worktree the window is open in. Never removed from its own window.
    Current,
    /// The folder is gone; prune the worktree instead.
    Missing,
    /// Locked with `git worktree lock`. Removed only with force.
    Locked,
    /// Staged, unstaged or untracked changes. Removed only with force.
    Dirty,
    /// Commits that no other branch or remote has. Removed only with force.
    Unpushed,
}

impl Blocker {
    /// A blocker that force does not lift.
    #[must_use]
    pub fn is_hard(self) -> bool {
        matches!(self, Blocker::Main | Blocker::Current | Blocker::Missing)
    }
}

/// What removing one worktree would lose or refuse, read without changing
/// anything. The same repository state gives the same inspection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeInspection {
    /// The worktree's path, as `git worktree list` gave it.
    pub path: String,
    /// The short branch name; `None` when detached.
    pub branch: Option<String>,
    /// The name shown for the worktree: the branch, or the folder's name
    /// when detached.
    pub name: String,
    /// Why it is locked; `Some("")` when no reason was given.
    pub locked: Option<String>,
    /// The folder is gone.
    pub missing: bool,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    /// Commits of HEAD that no other branch or remote has, or that are ahead
    /// of the upstream, whichever is more.
    pub unpushed: u32,
    /// Every blocker that applies, in `Blocker` order.
    pub blockers: Vec<Blocker>,
}

/// One entry `git worktree prune` would remove: its admin name under
/// `.git/worktrees` and the reason Git gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PruneCandidate {
    pub name: String,
    pub reason: String,
}

impl Blocker {
    /// The sentence that refuses a hard blocker.
    fn refusal(self) -> &'static str {
        match self {
            Blocker::Main => "The main checkout cannot be removed.",
            Blocker::Current => {
                "This window is open in this worktree. Open the main checkout to remove it."
            }
            Blocker::Missing => "Its folder is missing. Prune the worktree instead.",
            Blocker::Locked | Blocker::Dirty | Blocker::Unpushed => "",
        }
    }
}

impl WorktreeInspection {
    /// Whether the removal may go ahead (D31). A hard blocker always refuses.
    /// A soft one (locked, dirty, unpushed) refuses unless `force` is set, which
    /// is the user's confirmation. The message says why.
    pub fn check_removal(&self, force: bool) -> Result<(), String> {
        if let Some(hard) = self.blockers.iter().copied().find(|b| b.is_hard()) {
            return Err(hard.refusal().to_owned());
        }
        if self.blockers.is_empty() || force {
            return Ok(());
        }
        let reasons: Vec<String> = self.blockers.iter().map(|b| self.describe(*b)).collect();
        Err(format!(
            "{} cannot be removed. {} Confirm the removal to remove it anyway.",
            self.name,
            reasons.join(" ")
        ))
    }

    /// One blocker as a sentence for the user.
    #[must_use]
    pub fn describe(&self, blocker: Blocker) -> String {
        match blocker {
            Blocker::Main => "This is the main checkout.".to_owned(),
            Blocker::Current => "This window is open in this worktree.".to_owned(),
            Blocker::Missing => "Its folder is missing.".to_owned(),
            Blocker::Locked => match self.locked.as_deref() {
                Some(reason) if !reason.is_empty() => format!("It is locked: {reason}."),
                _ => "It is locked.".to_owned(),
            },
            Blocker::Dirty => format!(
                "It has uncommitted changes: {} staged, {} unstaged, {} untracked.",
                self.staged, self.unstaged, self.untracked
            ),
            Blocker::Unpushed => {
                let plural = if self.unpushed == 1 { "" } else { "s" };
                format!(
                    "{} commit{plural} that no other branch or remote has.",
                    self.unpushed
                )
            }
        }
    }
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
