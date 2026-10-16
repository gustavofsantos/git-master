//! In-memory `GitPort` for tests of the engine and the front-ends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gm_schema::RepoStatus;

use crate::{GitError, GitPort};

/// Answers `resolve_root` with its own root and `status` with a shared,
/// replaceable status, so a test can change what the next refresh sees.
#[derive(Clone)]
pub struct FakeGit {
    root: PathBuf,
    status: Arc<Mutex<Result<RepoStatus, GitError>>>,
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>, status: RepoStatus) -> Self {
        Self {
            root: root.into(),
            status: Arc::new(Mutex::new(Ok(status))),
        }
    }

    /// # Panics
    ///
    /// If another thread panicked while holding the status lock.
    pub fn set_status(&self, status: Result<RepoStatus, GitError>) {
        *self.status.lock().unwrap() = status;
    }
}

impl GitPort for FakeGit {
    fn resolve_root(&self, path: &Path) -> Result<PathBuf, GitError> {
        if path.starts_with(&self.root) {
            Ok(self.root.clone())
        } else {
            Err(GitError(format!(
                "not a git repository: {}",
                path.display()
            )))
        }
    }

    fn status(&self, _root: &Path) -> Result<RepoStatus, GitError> {
        self.status.lock().unwrap().clone()
    }
}
