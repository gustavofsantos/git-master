//! Keeps the status fresh without a refresh button. A watcher thread turns file
//! and git-dir changes of the open repository into `RefreshStatus` commands
//! dispatched through the engine with `Actor::Human`. It never blocks the UI:
//! the UI only receives `Notice`s and may send a focus `request`.
//!
//! Sources of change:
//! - the git dir (and the common dir of a linked worktree), non-recursively,
//!   plus `refs/` recursively; only `git_metadata_matters` paths count;
//! - the worktree registry, `worktrees/` in the common dir, non-recursively:
//!   a worktree added or removed from any shell changes its direct entries
//!   (not the files inside them, which belong to the other worktrees);
//! - the thread folder, `gitmaster/` in the common dir, non-recursively: only
//!   `threads.db` and `threads.db-wal` count, and they raise `ThreadsChanged`
//!   through the engine after a quiet period (design/threads.md);
//! - the refs, `refs/` and `packed-refs` in the git dir or the common dir: after
//!   a quiet period the engine compares the branch tips and fires `refs.changed`
//!   (`WF3a`);
//! - the workflow folders the last listing named (`WorkflowsListed.directories`), each
//!   non-recursively: a `.lua` file written, renamed or removed there by another program, such as
//!   the person's editor, lists the workflows again after a quiet period (D233);
//! - the worktree: the root and each directory holding tracked files
//!   (`set::watch_dirs`); changes whose paths `check-ignore` reports as ignored
//!   are dropped, so `target/` and `node_modules/` cost nothing;
//! - window focus, through `WatchHandle::request`.
//!
//! The watch set is rebuilt when `index` or `HEAD` changes. Past
//! `MAX_WATCHED_DIRS`, or when the OS refuses a watch, the watcher drops its
//! worktree watches and sends `Notice::Degraded` once: refresh then happens
//! on focus only.

mod coalesce;
mod filter;
mod probe;
mod set;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_channel::Receiver as AsyncReceiver;
use gm_schema::{Actor, CommandKind, Event, OperationId, RepoStatus};
use notify::{Event as FsEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::EngineHandle;
use coalesce::Coalescer;
pub use set::MAX_WATCHED_DIRS;

/// The quiet period a change waits for.
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// The quiet period a ref change waits for, before the engine checks the branch tips (`WF3a`).
pub const REFS_QUIET: Duration = Duration::from_millis(500);
/// The longest a change waits, however long the stream of changes.
pub const MAX_WAIT: Duration = Duration::from_secs(1);
/// How often a due refresh is re-checked while the engine is busy.
const BUSY_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchConfig {
    pub max_dirs: usize,
    pub debounce: Duration,
    pub max_wait: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            max_dirs: MAX_WATCHED_DIRS,
            debounce: DEBOUNCE,
            max_wait: MAX_WAIT,
        }
    }
}

/// What the watcher reports to the front-end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The repository is watched, with this many worktree directories.
    Watching { dirs: usize },
    /// Watching is off for this repository; refresh happens on focus only.
    Degraded { reason: String },
    /// A refresh was dispatched and is running under this id.
    Refreshing(OperationId),
}

enum Input {
    Fs(notify::Result<FsEvent>),
    /// Boxed: a thread event is large, and every input pays for the largest variant.
    Engine(Box<Event>),
    Focus,
    Shutdown,
}

/// The notices of one watcher, read by the front-end.
pub struct Notices(AsyncReceiver<Notice>);

impl Notices {
    /// The next notice, or `None` once the watcher has stopped.
    pub async fn next(&self) -> Option<Notice> {
        self.0.recv().await.ok()
    }

    /// Like `next`, for threads that are not async.
    pub fn next_blocking(&self) -> Option<Notice> {
        self.0.recv_blocking().ok()
    }
}

/// Owns the watcher thread. Dropping it stops watching.
pub struct WatchHandle {
    tx: mpsc::Sender<Input>,
}

impl WatchHandle {
    /// The window gained focus: refresh, as any other change would.
    pub fn request(&self) {
        let _ = self.tx.send(Input::Focus);
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Input::Shutdown);
    }
}

/// Starts watching the repository the engine has open. `login_path` is the
/// PATH of the user's shell, as the git adapter uses it.
///
/// # Panics
///
/// If the OS cannot start the watcher thread.
pub fn spawn(engine: EngineHandle, login_path: Arc<OnceLock<String>>) -> (WatchHandle, Notices) {
    spawn_with(engine, login_path, WatchConfig::default())
}

/// Like `spawn`, with the limits given by the caller (tests use small ones).
///
/// # Panics
///
/// If the OS cannot start the watcher thread.
pub fn spawn_with(
    engine: EngineHandle,
    login_path: Arc<OnceLock<String>>,
    config: WatchConfig,
) -> (WatchHandle, Notices) {
    let (tx, rx) = mpsc::channel();
    let (notices_tx, notices) = async_channel::unbounded();
    let events = engine.subscribe();
    let forward = tx.clone();
    std::thread::Builder::new()
        .name("gm-watch-events".into())
        .spawn(move || {
            while let Ok(event) = events.recv_blocking() {
                if forward.send(Input::Engine(Box::new(event))).is_err() {
                    break;
                }
            }
        })
        .expect("spawn the watch event thread");
    let worker = Worker {
        engine,
        login_path,
        config,
        input: tx.clone(),
        notices: notices_tx,
        coalescer: Coalescer::new(config.debounce, config.max_wait),
        root: None,
        git: None,
        dirs: None,
        os: None,
        degraded: false,
        registry: false,
        threads_dir: false,
        threads_first: None,
        threads_last: None,
        refs_first: None,
        refs_last: None,
        workflow_wanted: Vec::new(),
        workflow_dirs: BTreeSet::new(),
        workflows_first: None,
        workflows_last: None,
    };
    std::thread::Builder::new()
        .name("gm-watch".into())
        .spawn(move || worker.run(&rx))
        .expect("spawn the watcher thread");
    (WatchHandle { tx }, Notices(notices))
}

/// State of the watcher thread. Only that thread touches it.
struct Worker {
    engine: EngineHandle,
    login_path: Arc<OnceLock<String>>,
    config: WatchConfig,
    input: mpsc::Sender<Input>,
    notices: async_channel::Sender<Notice>,
    coalescer: Coalescer,
    root: Option<PathBuf>,
    /// The git dir and common dir of the open repository.
    git: Option<probe::GitDirs>,
    /// The notify watcher, when the worktree is watched.
    os: Option<RecommendedWatcher>,
    /// The worktree directories `os` watches, absolute.
    dirs: Option<BTreeSet<PathBuf>>,
    /// Set once the degraded notice went out for the open repository.
    degraded: bool,
    /// Whether the worktree registry is watched. It is created by the first
    /// linked worktree, so it may be watched only after it appears.
    registry: bool,
    /// Whether the thread folder `gitmaster/` is watched. A capture creates it,
    /// so it may be watched only after it appears.
    threads_dir: bool,
    /// When the first thread database change of the pending burst came.
    threads_first: Option<Instant>,
    /// When the last thread database change of the pending burst came.
    threads_last: Option<Instant>,
    /// When the first ref change of the pending burst came.
    refs_first: Option<Instant>,
    /// When the last ref change of the pending burst came.
    refs_last: Option<Instant>,
    /// The folders workflows are read from, as the last listing named them, existing or not.
    workflow_wanted: Vec<PathBuf>,
    /// Those of `workflow_wanted` that `os` watches.
    workflow_dirs: BTreeSet<PathBuf>,
    /// When the first workflow file change of the pending burst came.
    workflows_first: Option<Instant>,
    /// When the last workflow file change of the pending burst came.
    workflows_last: Option<Instant>,
}

impl Worker {
    fn run(mut self, rx: &mpsc::Receiver<Input>) {
        loop {
            let now = Instant::now();
            let due = self
                .coalescer
                .due()
                .into_iter()
                .chain(self.threads_due())
                .chain(self.refs_due())
                .chain(self.workflows_due())
                .min();
            let timeout = due.map(|due| if due <= now { BUSY_POLL } else { due - now });
            let first = match timeout {
                Some(timeout) => rx.recv_timeout(timeout),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let mut batch = Vec::new();
            match first {
                Ok(input) => batch.push(input),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            batch.extend(rx.try_iter());
            if self.absorb(batch) {
                return;
            }
            let now = Instant::now();
            if self.threads_due().is_some_and(|due| due <= now) {
                self.threads_first = None;
                self.threads_last = None;
                self.engine.threads_changed();
            }
            if self.refs_due().is_some_and(|due| due <= now) {
                self.refs_first = None;
                self.refs_last = None;
                self.engine.refs_changed();
            }
            if self.workflows_due().is_some_and(|due| due <= now) {
                self.workflows_first = None;
                self.workflows_last = None;
                // The listing reads the files again; the window shows the new text (D233).
                if self.root.is_some() {
                    self.engine
                        .dispatch(Actor::Human, CommandKind::ListWorkflows);
                }
            }
            // Without an open repository there is nothing to refresh; the request waits.
            if self.root.is_some() && self.coalescer.should_start(now, self.engine.is_busy()) {
                let id = self
                    .engine
                    .dispatch(Actor::Human, CommandKind::RefreshStatus);
                self.coalescer.started(id);
                let _ = self.notices.try_send(Notice::Refreshing(id));
            }
        }
    }

    /// Applies a batch in arrival order. Returns true on shutdown.
    fn absorb(&mut self, batch: Vec<Input>) -> bool {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut rescan = false;
        for input in batch {
            match input {
                Input::Shutdown => return true,
                Input::Focus => {
                    self.flush_paths(&mut paths, &mut rescan);
                    self.coalescer.request(Instant::now());
                }
                Input::Fs(Err(_)) => rescan = true,
                Input::Fs(Ok(event)) => {
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    rescan |= event.need_rescan();
                    paths.extend(event.paths);
                }
                Input::Engine(event) => {
                    self.flush_paths(&mut paths, &mut rescan);
                    self.on_engine_event(*event);
                }
            }
        }
        self.flush_paths(&mut paths, &mut rescan);
        false
    }

    fn on_engine_event(&mut self, event: Event) {
        let now = Instant::now();
        match event {
            Event::StatusLoaded { id, status } => {
                // An open writes nothing, so it opens no quiet period.
                if !self.on_status(&status) {
                    self.coalescer.status_loaded(id, now);
                }
            }
            Event::OperationChanged { id, state, .. } if state.is_terminal() => {
                self.coalescer.finished(id, now);
            }
            Event::WorkflowsListed { directories, .. } => {
                self.workflow_wanted = directories.into_iter().map(PathBuf::from).collect();
                self.sync_workflow_dirs();
            }
            _ => {}
        }
    }

    /// Whether the status belongs to another repository, which is then opened.
    fn on_status(&mut self, status: &RepoStatus) -> bool {
        if self.root.as_deref() == Some(Path::new(&status.root)) {
            return false;
        }
        self.open(Path::new(&status.root));
        true
    }

    /// Replaces the watches with those of a newly opened repository.
    fn open(&mut self, root: &Path) {
        self.os = None;
        self.dirs = None;
        self.git = None;
        self.degraded = false;
        self.registry = false;
        self.threads_dir = false;
        self.threads_first = None;
        self.threads_last = None;
        self.refs_first = None;
        self.refs_last = None;
        self.workflow_dirs.clear();
        self.workflows_first = None;
        self.workflows_last = None;
        self.root = Some(root.to_path_buf());
        match self.install(root) {
            Ok(dirs) => {
                let _ = self.notices.try_send(Notice::Watching { dirs });
            }
            Err(reason) => self.degrade(reason),
        }
    }

    fn install(&mut self, root: &Path) -> Result<usize, String> {
        let git = probe::git_dirs(root, &self.login_path)?;
        let mut os = self.new_watcher()?;
        let mut watch = |path: &Path, mode: RecursiveMode| {
            os.watch(path, mode)
                .map_err(|e| format!("cannot watch {}: {e}", path.display()))
        };
        watch(&git.git_dir, RecursiveMode::NonRecursive)?;
        if git.common_dir != git.git_dir {
            watch(&git.common_dir, RecursiveMode::NonRecursive)?;
        }
        let refs = git.common_dir.join("refs");
        if refs.is_dir() {
            watch(&refs, RecursiveMode::Recursive)?;
        }
        let registry = git.common_dir.join("worktrees");
        let registered = registry.is_dir();
        if registered {
            watch(&registry, RecursiveMode::NonRecursive)?;
        }
        self.registry = registered;
        let threads = git.common_dir.join("gitmaster");
        let threads_dir = threads.is_dir();
        if threads_dir {
            watch(&threads, RecursiveMode::NonRecursive)?;
        }
        self.threads_dir = threads_dir;
        self.git = Some(git);
        self.os = Some(os);
        self.sync_worktree(root)
    }

    /// Watches the worktree registry once git has created it. A failure is
    /// not fatal: the next refresh still reads the list, and focus refreshes it.
    fn watch_registry(&mut self, common_dir: &Path) {
        let Some(os) = self.os.as_mut() else {
            return;
        };
        if os
            .watch(&common_dir.join("worktrees"), RecursiveMode::NonRecursive)
            .is_ok()
        {
            self.registry = true;
        }
    }

    /// Watches the thread folder once a capture has created it. A failure is not
    /// fatal: the next capture is seen when the window is focused.
    fn watch_threads(&mut self, common_dir: &Path) {
        let Some(os) = self.os.as_mut() else {
            return;
        };
        if os
            .watch(&common_dir.join("gitmaster"), RecursiveMode::NonRecursive)
            .is_ok()
        {
            self.threads_dir = true;
        }
    }

    /// Brings the workflow watches in line with the folders of the last listing that exist. A
    /// folder that cannot be watched is left out: its files are read at the next listing.
    fn sync_workflow_dirs(&mut self) {
        let Some(os) = self.os.as_mut() else {
            self.workflow_dirs.clear();
            return;
        };
        let wanted: BTreeSet<PathBuf> = self
            .workflow_wanted
            .iter()
            .filter(|dir| dir.is_dir())
            .cloned()
            .collect();
        for dir in self.workflow_dirs.difference(&wanted) {
            let _ = os.unwatch(dir);
        }
        let mut watched = BTreeSet::new();
        for dir in wanted {
            if self.workflow_dirs.contains(&dir)
                || os.watch(&dir, RecursiveMode::NonRecursive).is_ok()
            {
                watched.insert(dir);
            }
        }
        self.workflow_dirs = watched;
    }

    /// When the pending workflow file change lists the workflows again, if there is one.
    fn workflows_due(&self) -> Option<Instant> {
        let first = self.workflows_first?;
        let last = self.workflows_last?;
        Some((last + DEBOUNCE).min(first + MAX_WAIT))
    }

    /// Notes a change of the thread database. The event goes out after a quiet
    /// period, or once `MAX_WAIT` has passed since the burst began.
    fn note_threads_change(&mut self, now: Instant) {
        self.threads_first.get_or_insert(now);
        self.threads_last = Some(now);
    }

    /// When the pending thread change is announced, if there is one.
    fn threads_due(&self) -> Option<Instant> {
        let first = self.threads_first?;
        let last = self.threads_last?;
        Some((last + DEBOUNCE).min(first + MAX_WAIT))
    }

    /// Notes a change of a ref. The check goes out after a quiet period, or once `MAX_WAIT` has
    /// passed since the burst began.
    fn note_refs_change(&mut self, now: Instant) {
        self.refs_first.get_or_insert(now);
        self.refs_last = Some(now);
    }

    /// When the pending ref change is checked, if there is one.
    fn refs_due(&self) -> Option<Instant> {
        let first = self.refs_first?;
        let last = self.refs_last?;
        Some((last + REFS_QUIET).min(first + MAX_WAIT))
    }

    /// Brings the worktree watches in line with the tracked set. Returns the
    /// number of directories watched.
    fn sync_worktree(&mut self, root: &Path) -> Result<usize, String> {
        let listing = probe::tracked_listing(root, &self.login_path)?;
        let wanted: BTreeSet<PathBuf> = set::watch_dirs(&listing, self.config.max_dirs)
            .map_err(|e| format!("more than {} directories to watch", e.limit))?
            .into_iter()
            .map(|rel| root.join(rel))
            .collect();
        let current = self.dirs.take().unwrap_or_default();
        let Some(os) = self.os.as_mut() else {
            return Err("the watcher is not running".to_owned());
        };
        for dir in current.difference(&wanted) {
            // The directory may be gone already; that drops its watch too.
            let _ = os.unwatch(dir);
        }
        for dir in wanted.difference(&current) {
            os.watch(dir, RecursiveMode::NonRecursive)
                .map_err(|e| format!("cannot watch {}: {e}", dir.display()))?;
        }
        let count = wanted.len();
        self.dirs = Some(wanted);
        Ok(count)
    }

    fn new_watcher(&self) -> Result<RecommendedWatcher, String> {
        let input = self.input.clone();
        notify::recommended_watcher(move |event| {
            let _ = input.send(Input::Fs(event));
        })
        .map_err(|e| format!("cannot start the file watcher: {e}"))
    }

    fn degrade(&mut self, reason: String) {
        self.os = None;
        self.dirs = None;
        self.workflow_dirs.clear();
        if !self.degraded {
            self.degraded = true;
            let _ = self.notices.try_send(Notice::Degraded { reason });
        }
    }

    /// Turns the paths of a batch into at most one change. Checks the ignore
    /// rules in one git call for the worktree paths.
    fn flush_paths(&mut self, paths: &mut Vec<PathBuf>, rescan: &mut bool) {
        let now = Instant::now();
        // Events were lost: the watch set and the status may both be stale.
        let mut tracked_set_changed = std::mem::take(rescan);
        let mut matters = tracked_set_changed;
        if paths.is_empty() && !matters {
            return;
        }
        let Some(root) = self.root.clone() else {
            paths.clear();
            return;
        };
        let (git_dir, common_dir) = match &self.git {
            Some(git) => (git.git_dir.clone(), git.common_dir.clone()),
            None => (root.join(".git"), root.join(".git")),
        };
        let mut worktree: BTreeSet<String> = BTreeSet::new();
        let mut registry_appeared = false;
        let mut threads_touched = false;
        let mut threads_folder_appeared = false;
        let mut refs_touched = false;
        let mut workflows_touched = false;
        for path in paths.drain(..) {
            workflows_touched |= filter::workflow_changed(&path, &self.workflow_wanted);
            if let Ok(rel) = path
                .strip_prefix(&git_dir)
                .or_else(|_| path.strip_prefix(&common_dir))
            {
                if filter::git_metadata_matters(rel) {
                    matters = true;
                    tracked_set_changed |= filter::changes_tracked_set(rel);
                }
                registry_appeared |= !self.registry && rel == Path::new("worktrees");
                threads_touched |= filter::threads_database_changed(rel);
                refs_touched |= filter::refs_changed(rel);
                threads_folder_appeared |= !self.threads_dir && filter::threads_dir_appeared(rel);
            } else if let Ok(rel) = path.strip_prefix(&root)
                && !filter::is_inside_dot_git(rel)
            {
                worktree.insert(rel.to_string_lossy().into_owned());
            }
        }
        if !worktree.is_empty() {
            let candidates: Vec<String> = worktree.into_iter().collect();
            // If the check fails, the change counts: a refresh is cheap, a stale view is not.
            let ignored = probe::ignored(&root, &self.login_path, &candidates).unwrap_or_default();
            matters |= candidates.iter().any(|rel| !ignored.contains(rel));
        }
        if registry_appeared {
            self.watch_registry(&common_dir);
        }
        if threads_folder_appeared {
            self.watch_threads(&common_dir);
        }
        if threads_touched {
            self.note_threads_change(now);
        }
        if refs_touched {
            self.note_refs_change(now);
        }
        if workflows_touched {
            self.workflows_first.get_or_insert(now);
            self.workflows_last = Some(now);
        }
        if tracked_set_changed && let Err(reason) = self.sync_worktree(&root) {
            self.degrade(reason);
        }
        if matters {
            self.coalescer.change(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let config = WatchConfig::default();
        assert_eq!(config.max_dirs, 5_000);
        assert_eq!(config.debounce, Duration::from_millis(150));
    }
}
