//! In-memory `GitPort` for tests of the engine and the front-ends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gm_schema::{
    BranchEntry, CommitDetail, CommitFile, CommitSummary, FileDiff, History, RepoStatus,
    WorktreeEntry, WorktreeMode,
};

use crate::file_index::FileIndex;
use crate::history::{Canceller, HistoryPage, HistoryStream};
use crate::port::{DetailRead, FileListRead, StackRead};
use crate::{GitError, GitPort};
use std::sync::atomic::{AtomicBool, Ordering};

/// Answers `resolve_root` with its own root, and `status`, `branches`,
/// `user_email`, `worktrees` and `checkout` with shared, replaceable results, so a test can
/// change what the next call sees.
#[derive(Clone)]
pub struct FakeGit {
    root: PathBuf,
    status: Arc<Mutex<Result<RepoStatus, GitError>>>,
    branches: Arc<Mutex<Result<Vec<BranchEntry>, GitError>>>,
    user_email: Arc<Mutex<Result<Option<String>, GitError>>>,
    checkout: Arc<Mutex<Result<(), GitError>>>,
    worktrees: Arc<Mutex<Result<Vec<WorktreeEntry>, GitError>>>,
    create: Arc<Mutex<Result<(), GitError>>>,
    created: Arc<Mutex<Vec<(String, WorktreeMode)>>>,
    history: Arc<Mutex<Result<History, GitError>>>,
    detail: Arc<Mutex<Result<CommitDetail, GitError>>>,
    stack: Arc<Mutex<Result<Vec<FileDiff>, GitError>>>,
    files: Arc<Mutex<Result<Vec<String>, GitError>>>,
    /// How many times the file list was asked for.
    file_reads: Arc<Mutex<usize>>,
    /// Every write git was asked for (stage, unstage, discard, commit), by name.
    writes: Arc<Mutex<Vec<&'static str>>>,
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>, status: RepoStatus) -> Self {
        Self {
            root: root.into(),
            status: Arc::new(Mutex::new(Ok(status))),
            branches: Arc::new(Mutex::new(Ok(Vec::new()))),
            user_email: Arc::new(Mutex::new(Ok(None))),
            checkout: Arc::new(Mutex::new(Ok(()))),
            worktrees: Arc::new(Mutex::new(Ok(Vec::new()))),
            create: Arc::new(Mutex::new(Ok(()))),
            created: Arc::new(Mutex::new(Vec::new())),
            history: Arc::new(Mutex::new(Ok(History::default()))),
            detail: Arc::new(Mutex::new(Ok(CommitDetail::default()))),
            stack: Arc::new(Mutex::new(Ok(Vec::new()))),
            files: Arc::new(Mutex::new(Ok(Vec::new()))),
            file_reads: Arc::new(Mutex::new(0)),
            writes: Arc::new(Mutex::new(Vec::new())),
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
    /// If another thread panicked while holding the user email lock.
    pub fn set_user_email(&self, email: Result<Option<String>, GitError>) {
        *self.user_email.lock().unwrap() = email;
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

    /// What the next file list read answers: the paths of the worktree.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the files lock.
    pub fn set_files(&self, files: Result<Vec<String>, GitError>) {
        *self.files.lock().unwrap() = files;
    }

    /// How many file list reads were started.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the file reads lock.
    #[must_use]
    pub fn file_reads(&self) -> usize {
        *self.file_reads.lock().unwrap()
    }

    /// What the next commit stack read answers: one diff per listed file.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the stack lock.
    pub fn set_stack(&self, diffs: Result<Vec<FileDiff>, GitError>) {
        *self.stack.lock().unwrap() = diffs;
    }

    /// The writes git was asked for, in order. Empty for a read-only session.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the writes lock.
    pub fn writes(&self) -> Vec<&'static str> {
        self.writes.lock().unwrap().clone()
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
    fn stage(&self, _root: &Path, _target: &gm_schema::StageTarget) -> Result<(), GitError> {
        self.writes.lock().unwrap().push("stage");
        Err(GitError("stage is not supported by this adapter".into()))
    }

    fn unstage(&self, _root: &Path, _target: &gm_schema::StageTarget) -> Result<(), GitError> {
        self.writes.lock().unwrap().push("unstage");
        Err(GitError("unstage is not supported by this adapter".into()))
    }

    fn discard_hunk(
        &self,
        _root: &Path,
        _path: &str,
        _hunk: &gm_schema::HunkId,
    ) -> Result<(), GitError> {
        self.writes.lock().unwrap().push("discard");
        Err(GitError("discard is not supported by this adapter".into()))
    }

    fn commit(&self, _root: &Path, _message: &str) -> Result<String, GitError> {
        self.writes.lock().unwrap().push("commit");
        Err(GitError("commit is not supported by this adapter".into()))
    }

    fn stage_all(&self, _root: &Path) -> Result<(), GitError> {
        self.writes.lock().unwrap().push("stage_all");
        Ok(())
    }

    /// The `.git` folder under the fake root, as a normal checkout has it.
    fn common_dir(&self, _root: &Path) -> Result<PathBuf, GitError> {
        Ok(self.root.join(".git"))
    }

    /// No configuration: every key is unset.
    fn config_values(&self, _root: &Path, _key: &str) -> Result<Vec<String>, GitError> {
        Ok(Vec::new())
    }

    fn commit_stack(
        &self,
        _root: &Path,
        _oid: &str,
        _files: &[CommitFile],
    ) -> Result<StackRead, GitError> {
        let diffs = self.stack.lock().unwrap().clone();
        Ok(Box::new(move |stop: &AtomicBool| {
            if stop.load(Ordering::SeqCst) {
                return Err(GitError("the commit read was superseded".into()));
            }
            diffs
        }))
    }

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

    fn user_email(&self, _root: &Path) -> Result<Option<String>, GitError> {
        self.user_email.lock().unwrap().clone()
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

    fn file_list(&self, _root: &Path) -> Result<FileListRead, GitError> {
        *self.file_reads.lock().unwrap() += 1;
        let files = self.files.lock().unwrap().clone();
        Ok(Box::new(move |stop: &AtomicBool| {
            if stop.load(Ordering::SeqCst) {
                return Err(GitError("the file list read was superseded".into()));
            }
            files.map(FileIndex::from_paths)
        }))
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
