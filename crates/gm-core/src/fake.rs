//! In-memory `GitPort` for tests of the engine and the front-ends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gm_schema::{
    BranchEntry, CommitDetail, CommitSummary, History, RepoStatus, WorktreeEntry, WorktreeMode,
};

use crate::history::{Canceller, HistoryPage, HistoryStream};
use crate::port::DetailRead;
use crate::{GitError, GitPort};
use std::sync::atomic::{AtomicBool, Ordering};

/// Answers `resolve_root` with its own root, and `status`, `branches`,
/// `worktrees` and `checkout` with shared, replaceable results, so a test can change what the
/// next call sees.
#[derive(Clone)]
pub struct FakeGit {
    root: PathBuf,
    status: Arc<Mutex<Result<RepoStatus, GitError>>>,
    branches: Arc<Mutex<Result<Vec<BranchEntry>, GitError>>>,
    checkout: Arc<Mutex<Result<(), GitError>>>,
    worktrees: Arc<Mutex<Result<Vec<WorktreeEntry>, GitError>>>,
    create: Arc<Mutex<Result<(), GitError>>>,
    created: Arc<Mutex<Vec<(String, WorktreeMode)>>>,
    history: Arc<Mutex<Result<History, GitError>>>,
    detail: Arc<Mutex<Result<CommitDetail, GitError>>>,
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>, status: RepoStatus) -> Self {
        Self {
            root: root.into(),
            status: Arc::new(Mutex::new(Ok(status))),
            branches: Arc::new(Mutex::new(Ok(Vec::new()))),
            checkout: Arc::new(Mutex::new(Ok(()))),
            worktrees: Arc::new(Mutex::new(Ok(Vec::new()))),
            create: Arc::new(Mutex::new(Ok(()))),
            created: Arc::new(Mutex::new(Vec::new())),
            history: Arc::new(Mutex::new(Ok(History::default()))),
            detail: Arc::new(Mutex::new(Ok(CommitDetail::default()))),
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
    /// If another thread panicked while holding the worktrees lock.
    pub fn set_worktrees(&self, worktrees: Result<Vec<WorktreeEntry>, GitError>) {
        *self.worktrees.lock().unwrap() = worktrees;
    }

    /// What the next commit detail read answers.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the detail lock.
    pub fn set_detail(&self, detail: Result<CommitDetail, GitError>) {
        *self.detail.lock().unwrap() = detail;
    }

    /// The commits the next history walk yields. `History::more` is ignored.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the history lock.
    pub fn set_history(&self, history: Result<History, GitError>) {
        *self.history.lock().unwrap() = history;
    }

    /// The result of the next `create_worktree`.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the create lock.
    pub fn set_create(&self, result: Result<(), GitError>) {
        *self.create.lock().unwrap() = result;
    }

    /// The `create_worktree` calls so far, in order: the path and the mode.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the created lock.
    pub fn created(&self) -> Vec<(String, WorktreeMode)> {
        self.created.lock().unwrap().clone()
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

    fn worktrees(&self, _root: &Path) -> Result<Vec<WorktreeEntry>, GitError> {
        self.worktrees.lock().unwrap().clone()
    }

    fn create_worktree(
        &self,
        _root: &Path,
        path: &str,
        mode: &WorktreeMode,
    ) -> Result<(), GitError> {
        self.created
            .lock()
            .unwrap()
            .push((path.to_owned(), mode.clone()));
        self.create.lock().unwrap().clone()
    }

    fn checkout(&self, _root: &Path, _branch: &str) -> Result<(), GitError> {
        self.checkout.lock().unwrap().clone()
    }

    fn commit_detail(&self, _root: &Path, _oid: &str) -> Result<DetailRead, GitError> {
        let detail = self.detail.lock().unwrap().clone();
        Ok(Box::new(move |stop: &AtomicBool| {
            if stop.load(Ordering::SeqCst) {
                return Err(GitError("the commit read was superseded".into()));
            }
            detail
        }))
    }

    fn history_stream(&self, _root: &Path) -> Result<Option<Box<dyn HistoryStream>>, GitError> {
        let history = self.history.lock().unwrap().clone()?;
        if history.unborn {
            return Ok(None);
        }
        Ok(Some(Box::new(FakeWalk {
            commits: history.commits.into(),
        })))
    }
}

/// A walk over the commits a test set, yielded as the engine reads them.
struct FakeWalk {
    commits: std::collections::VecDeque<CommitSummary>,
}

impl HistoryStream for FakeWalk {
    fn next_page(&mut self, limit: usize) -> Result<HistoryPage, GitError> {
        let take = limit.min(self.commits.len());
        let commits: Vec<CommitSummary> = self.commits.drain(..take).collect();
        Ok(HistoryPage {
            commits,
            done: self.commits.is_empty(),
            unborn: false,
        })
    }

    fn skip_past(&mut self, oid: &str) -> Result<(), GitError> {
        while let Some(commit) = self.commits.pop_front() {
            if commit.oid == oid {
                return Ok(());
            }
        }
        Err(GitError(format!("commit {oid} is not in the history")))
    }

    fn canceller(&self) -> Canceller {
        std::sync::Arc::new(|| {})
    }
}
