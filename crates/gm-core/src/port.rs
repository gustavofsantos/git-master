use std::path::Path;

use gm_schema::RepoStatus;

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
}
