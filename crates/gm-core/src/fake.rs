//! In-memory `GitPort` for tests of the engine and the front-ends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gm_schema::{BranchEntry, RepoStatus};

use crate::{GitError, GitPort};

/// Answers `resolve_root` with its own root, and `status`, `branches` and
/// `checkout` with shared, replaceable results, so a test can change what the
/// next call sees.
#[derive(Clone)]
pub struct FakeGit {
    root: PathBuf,
    status: Arc<Mutex<Result<RepoStatus, GitError>>>,
    branches: Arc<Mutex<Result<Vec<BranchEntry>, GitError>>>,
    checkout: Arc<Mutex<Result<(), GitError>>>,
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>, status: RepoStatus) -> Self {
        Self {
            root: root.into(),
            status: Arc::new(Mutex::new(Ok(status))),
            branches: Arc::new(Mutex::new(Ok(Vec::new()))),
            checkout: Arc::new(Mutex::new(Ok(()))),
        }
    }

    /// # Panics
    ///
    /// If another thread panicked while holding the status lock.
    pub fn set_status(&self, status: Result<RepoStatus, GitError>) {
        *self.status.lock().unwrap() = status;
    }

    /// # Panics
    ///
    /// If another thread panicked while holding the branches lock.
    pub fn set_branches(&self, branches: Result<Vec<BranchEntry>, GitError>) {
        *self.branches.lock().unwrap() = branches;
    }

    /// # Panics
    ///
    /// If another thread panicked while holding the checkout lock.
    pub fn set_checkout(&self, result: Result<(), GitError>) {
        *self.checkout.lock().unwrap() = result;
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

    fn branches(&self, _root: &Path) -> Result<Vec<BranchEntry>, GitError> {
        self.branches.lock().unwrap().clone()
    }

    fn checkout(&self, _root: &Path, _branch: &str) -> Result<(), GitError> {
        self.checkout.lock().unwrap().clone()
    }
}
