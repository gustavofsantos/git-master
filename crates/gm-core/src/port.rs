use std::path::Path;
use std::sync::{Arc, OnceLock};

use gm_schema::{
    BranchEntry, DiffSide, FileDiff, History, HookScope, HunkId, PruneCandidate, RepoStatus,
    StageTarget, WorktreeEntry, WorktreeMode,
};

use crate::command_log::CommandLog;
use crate::hook::ConfigValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// What the core needs from Git. Adapters implement it; the core tests with fakes.
pub trait GitPort: Send {
    /// The working tree root that contains `path`.
    fn resolve_root(&self, path: &Path) -> Result<std::path::PathBuf, GitError>;

    /// Never takes `index.lock`: an agent may be running `git` in the same repo.
    fn status(&self, root: &Path) -> Result<RepoStatus, GitError>;

    /// Diff of one path. An untracked path diffs as all-added.
    fn diff(&self, _root: &Path, _path: &str, _side: DiffSide) -> Result<FileDiff, GitError> {
        Err(GitError("diff is not supported by this adapter".into()))
    }

    /// Moves `target` into the index. A stale hunk id is an error, never a
    /// guess.
    fn stage(&self, _root: &Path, _target: &StageTarget) -> Result<(), GitError> {
        Err(GitError("stage is not supported by this adapter".into()))
    }

    /// Moves `target` out of the index (staged diff, HEAD against index).
    fn unstage(&self, _root: &Path, _target: &StageTarget) -> Result<(), GitError> {
        Err(GitError("unstage is not supported by this adapter".into()))
    }

    /// Reverts the unstaged hunk `hunk` of `path` in the working tree. The
    /// index is not touched. A stale id is an error, never a guess.
    fn discard_hunk(&self, _root: &Path, _path: &str, _hunk: &HunkId) -> Result<(), GitError> {
        Err(GitError("discard is not supported by this adapter".into()))
    }

    /// Commits the index; returns the new commit id.
    fn commit(&self, _root: &Path, _message: &str) -> Result<String, GitError> {
        Err(GitError("commit is not supported by this adapter".into()))
    }

    /// Local branches first, then remote ones, each alphabetically. Never
    /// records anything.
    fn branches(&self, _root: &Path) -> Result<Vec<BranchEntry>, GitError> {
        Err(GitError("branches is not supported by this adapter".into()))
    }

    /// The worktrees of the repository, the main checkout first. Never
    /// records anything.
    fn worktrees(&self, _root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
        Err(GitError(
            "worktrees is not supported by this adapter".into(),
        ))
    }

    /// The first `HISTORY_LIMIT` commits of HEAD, newest first. Never records
    /// anything (D28).
    fn history(&self, _root: &Path) -> Result<History, GitError> {
        Err(GitError("history is not supported by this adapter".into()))
    }

    /// Creates a linked worktree at `path` (relative paths are taken from
    /// `root`) that checks out `mode`. Refuses a path that already exists.
    /// Git's own refusal is the error.
    fn create_worktree(
        &self,
        _root: &Path,
        _path: &str,
        _mode: &WorktreeMode,
    ) -> Result<(), GitError> {
        Err(GitError(
            "create worktree is not supported by this adapter".into(),
        ))
    }

    /// Commits of HEAD in the worktree at `path` that no other local branch or
    /// remote has, or that are ahead of the upstream: whichever count is higher.
    /// `branch` is the worktree's branch, `None` when detached. Zero on an unborn
    /// HEAD. Never records anything.
    fn unpushed_commits(&self, _path: &Path, _branch: Option<&str>) -> Result<u32, GitError> {
        Err(GitError(
            "unpushed commits is not supported by this adapter".into(),
        ))
    }

    /// Removes the linked worktree at `path` and its folder. `force` is 0 (Git
    /// refuses changes), 1 (`--force`) or 2 (`--force --force`, which also
    /// removes a locked worktree). Git's own refusal is the error.
    fn remove_worktree(&self, _root: &Path, _path: &str, _force: u8) -> Result<(), GitError> {
        Err(GitError(
            "remove worktree is not supported by this adapter".into(),
        ))
    }

    /// Locks the worktree at `path`; an empty `reason` locks it without one.
    fn lock_worktree(&self, _root: &Path, _path: &str, _reason: &str) -> Result<(), GitError> {
        Err(GitError(
            "lock worktree is not supported by this adapter".into(),
        ))
    }

    /// Unlocks the worktree at `path`.
    fn unlock_worktree(&self, _root: &Path, _path: &str) -> Result<(), GitError> {
        Err(GitError(
            "unlock worktree is not supported by this adapter".into(),
        ))
    }

    /// The admin records `git worktree prune` would remove, from its dry run.
    /// Never records anything.
    fn prunable(&self, _root: &Path) -> Result<Vec<PruneCandidate>, GitError> {
        Err(GitError("prunable is not supported by this adapter".into()))
    }

    /// Removes the admin records of the worktrees whose folders are gone.
    fn prune_worktrees(&self, _root: &Path) -> Result<(), GitError> {
        Err(GitError(
            "prune worktrees is not supported by this adapter".into(),
        ))
    }

    /// Switches HEAD to `branch`. A remote-only branch gets a local tracking
    /// branch of the same short name. Git's own refusal is the error.
    fn checkout(&self, _root: &Path, _branch: &str) -> Result<(), GitError> {
        Err(GitError("checkout is not supported by this adapter".into()))
    }

    /// The `gitmaster.prepare-commit.*` keys of one scope, in config order.
    /// `User` is global config only; `Repo` is the repository's own config.
    /// Never records anything.
    fn hook_config(&self, _root: &Path, _scope: HookScope) -> Result<Vec<ConfigValue>, GitError> {
        Err(GitError(
            "hook config is not supported by this adapter".into(),
        ))
    }

    /// The staged diff, as `git diff --cached` prints it. Never records anything.
    fn staged_diff(&self, _root: &Path) -> Result<String, GitError> {
        Err(GitError(
            "staged diff is not supported by this adapter".into(),
        ))
    }

    /// The login-shell PATH, which git and the hook scripts run with. Empty
    /// until it is read; an adapter without one keeps the process's PATH.
    fn login_path(&self) -> Arc<OnceLock<String>> {
        Arc::default()
    }

    /// The log that hook steps and git writes are recorded in, when the
    /// adapter keeps one. Reads are never recorded.
    fn command_log(&self) -> Option<CommandLog> {
        None
    }
}
