use std::path::Path;

use gm_schema::{DiffSide, FileDiff, RepoStatus, StageTarget};

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

    /// Commits the index; returns the new commit id.
    fn commit(&self, _root: &Path, _message: &str) -> Result<String, GitError> {
        Err(GitError("commit is not supported by this adapter".into()))
    }
}
