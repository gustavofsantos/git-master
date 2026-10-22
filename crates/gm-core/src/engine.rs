use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gm_schema::{
    Actor, Blocker, BranchFailure, Command, CommandKind, CommitFile, DirListing, Event,
    GrepOptions, History, HookFailure, HookScope, MERGED_LIST_CHANGED, MergedBranch, OperationId,
    OperationState, REFLOG_PAGE, STASH_LIST_CHANGED, WorktreeEntry, WorktreeInspection,
};

use crate::GitPort;
use crate::branch;
use crate::commit_detail::{DetailJob, DetailReport};
use crate::commit_diff::{StackJob, StackReport};
use crate::files::{
    Building, FilesSession, IndexJob, IndexReport, ReadJob, ReadReport, SearchJob, SearchReport,
    TextJob, TextReport,
};
use crate::grep::GrepRequest;
use crate::history::{Canceller, HistoryJob, HistoryOutcome, HistoryReport, HistoryStream};
use crate::hook::{self, ApprovalStore, Fingerprint, HookOutcome, HookRecord, Plan, Report, Step};
use crate::reflog;
use crate::remote::{RemoteJob, RemoteReport};
use crate::workflow;
use crate::workflow::driver::{ExecJob, ExecReport};
use crate::workflow::store::{RETAINED_RUNS, Store};
use crate::workflow::templates;
use crate::workflow_session::{self, Actions, Session};
use crate::worktree;

/// One state change of one operation. The log is audit, and the base for undo
/// later; it is not replayed to rebuild state (docs/06).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub id: OperationId,
    pub actor: Actor,
    pub kind: CommandKind,
    pub state: OperationState,
}

/// Synchronous and deterministic: the same commands over the same repository
/// give the same events and the same log. Threads live in `runner`.
pub struct Engine {
    git: Box<dyn GitPort>,
    root: Option<PathBuf>,
    log: Vec<LogEntry>,
    approvals: ApprovalStore,
    /// Hook runs that started and have not answered yet, by run id.
    runs: BTreeMap<OperationId, ActiveRun>,
    /// Runs accepted but not yet handed to a thread; see `take_jobs`.
    jobs: Vec<Plan>,
    hook_log: Vec<HookRecord>,
    /// The walk the history list reads, while a repository is open.
    history: Option<HistorySession>,
    /// History pages accepted since the last call; see `take_history_jobs`.
    history_jobs: Vec<HistoryJob>,
    /// The commit detail read in flight, if any. A new request supersedes it.
    detail: Option<ActiveDetail>,
    /// Detail reads accepted since the last call; see `take_detail_jobs`.
    detail_jobs: Vec<DetailJob>,
    /// The commit file diff read in flight, if any. A new request supersedes it.
    stack: Option<ActiveDetail>,
    /// Commit stack reads accepted since the last call; see `take_stack_jobs`.
    stack_jobs: Vec<StackJob>,
    /// The file list of the open repository (N1, N2).
    files: Option<FilesSession>,
    /// Reads of the file list accepted since the last call; see `take_index_jobs`.
    index_jobs: Vec<IndexJob>,
    /// The file search in flight, if any. A new request supersedes it.
    files_search: Option<ActiveDetail>,
    /// The query of `files_search` while it waits for the first file list.
    files_waiting: Option<String>,
    /// File searches accepted since the last call; see `take_search_jobs`.
    search_jobs: Vec<SearchJob>,
    /// The file read in flight, if any. A new request supersedes it.
    files_read: Option<ActiveDetail>,
    /// File reads accepted since the last call; see `take_read_jobs`.
    read_jobs: Vec<ReadJob>,
    /// The text search in flight, if any. A new request supersedes it.
    text_search: Option<ActiveDetail>,
    /// Text searches accepted since the last call; see `take_text_jobs`.
    text_jobs: Vec<TextJob>,
    /// The remote operation in flight, if any. Only one runs at a time.
    remote: Option<ActiveRemote>,
    /// Remote operations accepted since the last call; see `take_remote_jobs`.
    remote_jobs: Vec<RemoteJob>,
    /// The user's workflow directory, read before `gitmaster.workflowPath`.
    workflow_user_dir: Option<PathBuf>,
    /// The folder a deleted workflow file is moved to; see `delete_workflow_file`.
    workflow_trash_dir: Option<PathBuf>,
    /// The workflow runs of the open repository; see `workflow_session`.
    workflow: Option<Session>,
}

/// The walk behind the history list: the stream while no job reads it, how to
/// stop its git process, and the page request in flight.
struct HistorySession {
    stream: Option<Box<dyn HistoryStream>>,
    stop: Canceller,
    /// The last commit the list holds. A page after it continues the walk.
    last: Option<String>,
    active: Option<ActiveHistory>,
}

/// A page request that has not answered yet.
struct ActiveHistory {
    id: OperationId,
    actor: Actor,
    kind: CommandKind,
}

/// A commit read (the detail, or one file of it) that has not answered yet.
/// `stop` is shared with its job: setting it ends the read.
struct ActiveDetail {
    op: ActiveHistory,
    stop: Arc<AtomicBool>,
}

/// A fetch, pull or push with lease that has not answered yet, and the
/// repository it runs in.
struct ActiveRemote {
    op: ActiveHistory,
    root: PathBuf,
}

/// What the engine keeps of a `RunPrepareCommit` until it answers.
struct ActiveRun {
    command: Command,
    root: PathBuf,
    fingerprint: Fingerprint,
    cancel: Arc<AtomicBool>,
    /// Command and command hash of each step, by hook name.
    scripts: BTreeMap<String, (String, String)>,
}

impl ActiveRun {
    /// The command and its hash of the step `hook`; empty when unknown.
    fn script(&self, hook: &str) -> (String, String) {
        self.scripts.get(hook).cloned().unwrap_or_default()
    }
}

/// A run the engine accepted: its record, and the plan for its thread.
struct Accepted {
    active: ActiveRun,
    plan: Plan,
}

struct Op<'a> {
    id: OperationId,
    command: &'a Command,
    state: OperationState,
}

impl Engine {
    /// An engine whose approvals are not saved anywhere: repository hooks
    /// never get approved.
    pub fn new(git: Box<dyn GitPort>) -> Self {
        Self::with_approvals(git, ApprovalStore::unavailable())
    }

    pub fn with_approvals(git: Box<dyn GitPort>, approvals: ApprovalStore) -> Self {
        Self {
            git,
            root: None,
            log: Vec::new(),
            approvals,
            runs: BTreeMap::new(),
            jobs: Vec::new(),
            hook_log: Vec::new(),
            history: None,
            history_jobs: Vec::new(),
            detail: None,
            detail_jobs: Vec::new(),
            stack: None,
            stack_jobs: Vec::new(),
            files: None,
            index_jobs: Vec::new(),
            files_search: None,
            files_waiting: None,
            search_jobs: Vec::new(),
            files_read: None,
            read_jobs: Vec::new(),
            text_search: None,
            text_jobs: Vec::new(),
            remote: None,
            remote_jobs: Vec::new(),
            workflow_user_dir: crate::workflow::loader::default_user_dir(),
            workflow_trash_dir: crate::hook::approval::default_file()
                .and_then(|file| file.parent().map(|data| data.join("workflow-trash"))),
            workflow: None,
        }
    }

    /// Sets the directory workflows are read from before `gitmaster.workflowPath`.
    /// `None` reads none. The default is the user's config directory.
    pub fn set_workflow_user_dir(&mut self, dir: Option<PathBuf>) {
        self.workflow_user_dir = dir;
    }

    /// Sets the folder deleted workflow files are moved to. `None` refuses
    /// deletes. The default is `workflow-trash` in the app's data directory.
    pub fn set_workflow_trash_dir(&mut self, dir: Option<PathBuf>) {
        self.workflow_trash_dir = dir;
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// One record per hook step that ended, in the order they ended.
    pub fn hook_log(&self) -> &[HookRecord] {
        &self.hook_log
    }

    /// The hook runs accepted since the last call. The caller executes each
    /// `Plan` on its own thread and passes every `Report` back to `hook_report`.
    pub fn take_jobs(&mut self) -> Vec<Plan> {
        std::mem::take(&mut self.jobs)
    }

    /// The history pages accepted since the last call. The caller runs each on
    /// a thread and passes its report to `history_report`.
    pub fn take_history_jobs(&mut self) -> Vec<HistoryJob> {
        std::mem::take(&mut self.history_jobs)
    }

    /// The reads of the file list accepted since the last call. The caller runs
    /// each on a thread and passes its report to `index_report`.
    pub fn take_index_jobs(&mut self) -> Vec<IndexJob> {
        std::mem::take(&mut self.index_jobs)
    }

    /// The file searches accepted since the last call. The caller runs each on
    /// a thread and passes its report to `search_report`.
    pub fn take_search_jobs(&mut self) -> Vec<SearchJob> {
        std::mem::take(&mut self.search_jobs)
    }

    /// The text searches accepted since the last call. The caller runs each on a
    /// thread and passes its report to `text_report`.
    pub fn take_text_jobs(&mut self) -> Vec<TextJob> {
        std::mem::take(&mut self.text_jobs)
    }

    /// The file reads accepted since the last call. The caller runs each on a
    /// thread and passes its report to `read_report`.
    pub fn take_read_jobs(&mut self) -> Vec<ReadJob> {
        std::mem::take(&mut self.read_jobs)
    }

    /// The commit detail reads accepted since the last call. The caller runs
    /// each on a thread and passes its report to `detail_report`.
    pub fn take_detail_jobs(&mut self) -> Vec<DetailJob> {
        std::mem::take(&mut self.detail_jobs)
    }

    /// The file diff reads accepted since the last call. The caller runs each on a
    /// thread and passes its report to `stack_report`.
    pub fn take_stack_jobs(&mut self) -> Vec<StackJob> {
        std::mem::take(&mut self.stack_jobs)
    }

    /// The remote operations accepted since the last call. The caller runs each
    /// on a thread and passes its report to `remote_report`.
    pub fn take_remote_jobs(&mut self) -> Vec<RemoteJob> {
        std::mem::take(&mut self.remote_jobs)
    }

    /// The `exec` steps of workflow runs started since the last call. The caller
    /// runs each on a thread and passes its report to `workflow_report`.
    pub fn take_workflow_jobs(&mut self) -> Vec<ExecJob> {
        self.workflow
            .as_mut()
            .map(|session| std::mem::take(&mut session.jobs))
            .unwrap_or_default()
    }

    /// Turns the answer of an `exec` step into the run's next events. A report
    /// for a run that was cancelled is dropped.
    pub fn workflow_report(&mut self, report: ExecReport) -> Vec<Event> {
        let now = now_ms();
        let Some(session) = self.workflow.as_mut() else {
            return Vec::new();
        };
        let origin = session.origin(report.run);
        let root = session.root.clone();
        let mut actions = Actions::new(self.git.as_ref(), &root, &mut self.log, origin);
        let outcome = session.driver.exec_report(report, &mut actions, now);
        let wrote = actions.wrote();
        let events = session.absorb(outcome);
        self.workflow_events(origin, &root, events, wrote)
    }

    /// Turns the answer of a remote operation into events: the operation ends
    /// (with Git's text on failure, or the success message), then the status and
    /// the branches are read again, since ahead and behind may have moved. A report
    /// for an operation that is not in flight is dropped.
    pub fn remote_report(&mut self, report: RemoteReport) -> Vec<Event> {
        let active = match self.remote.take() {
            Some(active) if active.op.id == report.id => active,
            other => {
                self.remote = other;
                return Vec::new();
            }
        };
        let (state, detail) = match report.outcome {
            Ok(()) => (
                OperationState::Done,
                Some(success_message(&active.op.kind).to_owned()),
            ),
            Err(message) => (OperationState::Failed, Some(message)),
        };
        let mut events = Vec::new();
        // The status is read for the repository the operation ran in, and only while
        // it is still the open one. A read that fails leaves the last shown state.
        if self.root.as_deref() == Some(active.root.as_path()) {
            if let Ok(status) = self.git.status(&active.root) {
                events.push(Event::StatusLoaded {
                    id: report.id,
                    status,
                });
            }
            if let Ok(branches) = self.git.branches(&active.root) {
                let user_email = self.git.user_email(&active.root).unwrap_or_default();
                events.push(Event::BranchesLoaded {
                    id: report.id,
                    branches,
                    user_email,
                });
            }
        }
        // The end comes last: a status clears the error line, and a failure must stay on it.
        events.push(self.finish_op(&active.op, state, detail));
        events
    }

    /// Turns the answer of a file diff read into events. A report for a read that
    /// was superseded, or for a repository that was left, is dropped.
    pub fn stack_report(&mut self, report: StackReport) -> Vec<Event> {
        match self.stack.take() {
            Some(active) if active.op.id == report.id => {
                let (state, detail) = match &report.outcome {
                    Ok(_) => (OperationState::Done, None),
                    Err(message) => (OperationState::Failed, Some(message.clone())),
                };
                let mut events = vec![self.finish_op(&active.op, state, detail)];
                if let Ok(diffs) = report.outcome {
                    events.push(Event::CommitStackLoaded {
                        id: report.id,
                        diffs,
                    });
                }
                events
            }
            other => {
                self.stack = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a read of the file list into events: `FilesIndexed`,
    /// and the search that was waiting for the list starts. A report for a read
    /// that was replaced, or for a repository that was left, is dropped.
    pub fn index_report(&mut self, report: IndexReport) -> Vec<Event> {
        let Some(session) = self.files.as_mut() else {
            return Vec::new();
        };
        let building = match session.building.take() {
            Some(building) if building.token == report.token => building,
            other => {
                session.building = other;
                return Vec::new();
            }
        };
        match report.outcome {
            Ok(index) => {
                let index = Arc::new(index);
                session.index = Some(Arc::clone(&index));
                let files = index.len();
                if let (Some(active), Some(query)) = (&self.files_search, self.files_waiting.take())
                {
                    self.search_jobs.push(SearchJob {
                        id: active.op.id,
                        index,
                        query,
                        indexing: false,
                        stop: Arc::clone(&active.stop),
                    });
                }
                vec![Event::FilesIndexed {
                    id: building.id,
                    files,
                }]
            }
            // A search that waited for the list fails with Git's message. With
            // none waiting the failure is silent: the next request reads again.
            Err(message) => {
                if self.files_waiting.take().is_none() {
                    return Vec::new();
                }
                match self.files_search.take() {
                    Some(active) => {
                        vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                    }
                    None => Vec::new(),
                }
            }
        }
    }

    /// Turns the answer of a file search into events. A report for a search
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn search_report(&mut self, report: SearchReport) -> Vec<Event> {
        match self.files_search.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Some(search) => vec![
                    self.finish_op(&active.op, OperationState::Done, None),
                    Event::FilesFound {
                        id: report.id,
                        search,
                    },
                ],
                None => vec![self.finish_op(&active.op, OperationState::Cancelled, None)],
            },
            other => {
                self.files_search = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a text search into events. A report for a search
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn text_report(&mut self, report: TextReport) -> Vec<Event> {
        match self.text_search.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Ok(result) => vec![
                    self.finish_op(&active.op, OperationState::Done, None),
                    Event::TextFound {
                        id: report.id,
                        result,
                    },
                ],
                Err(message) => {
                    vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                }
            },
            other => {
                self.text_search = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a file read into events. A report for a read that
    /// was superseded, or for a repository that was left, is dropped.
    pub fn read_report(&mut self, report: ReadReport) -> Vec<Event> {
        match self.files_read.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Ok(file) => vec![
                    self.finish_op(&active.op, OperationState::Done, None),
                    Event::FileRead {
                        id: report.id,
                        file,
                    },
                ],
                Err(message) => {
                    vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                }
            },
            other => {
                self.files_read = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a commit detail read into events. A report for a
    /// read that was superseded, or for a repository that was left, is dropped.
    pub fn detail_report(&mut self, report: DetailReport) -> Vec<Event> {
        match self.detail.take() {
            Some(active) if active.op.id == report.id => {
                let state = if report.outcome.is_ok() {
                    OperationState::Done
                } else {
                    OperationState::Failed
                };
                let mut events = Vec::new();
                match report.outcome {
                    Ok(detail) => {
                        events.push(self.finish_op(&active.op, state, None));
                        events.push(Event::CommitDetailLoaded {
                            id: report.id,
                            detail,
                        });
                    }
                    Err(message) => {
                        events.push(self.finish_op(&active.op, state, Some(message)));
                    }
                }
                events
            }
            other => {
                self.detail = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a history page into events. A report for a request
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn history_report(&mut self, report: HistoryReport) -> Vec<Event> {
        let Some(mut session) = self.history.take() else {
            return Vec::new();
        };
        let active = match session.active.take() {
            Some(active) if active.id == report.id => active,
            other => {
                session.active = other;
                self.history = Some(session);
                return Vec::new();
            }
        };
        let mut events = Vec::new();
        match report.outcome {
            HistoryOutcome::Page { stream, page } => {
                if let Some(last) = page.commits.last() {
                    session.last = Some(last.oid.clone());
                }
                session.stream = stream;
                let history = History {
                    commits: page.commits,
                    unborn: page.unborn,
                    more: !page.done,
                };
                events.push(self.finish_op(&active, OperationState::Done, None));
                events.push(Event::HistoryLoaded {
                    id: report.id,
                    history,
                });
            }
            HistoryOutcome::Failed(message) => {
                // The walk is gone; the cursor stays, so a later page resumes after it.
                session.stream = None;
                events.push(self.finish_op(&active, OperationState::Failed, Some(message)));
            }
        }
        self.history = Some(session);
        events
    }

    /// Turns one report of a hook run into events, and records the run's
    /// state when it ends. A report for a run that already ended is ignored.
    pub fn hook_report(&mut self, report: Report) -> Vec<Event> {
        let mut events = Vec::new();
        match report {
            Report::Started {
                run,
                hook,
                step,
                of,
            } => {
                if self.runs.contains_key(&run) {
                    events.push(Event::HookStarted {
                        id: run,
                        hook,
                        step,
                        of,
                    });
                }
            }
            Report::StepDone { run, hook } => {
                if let Some(active) = self.runs.get(&run) {
                    let (command, command_hash) = active.script(&hook);
                    self.hook_log.push(HookRecord {
                        run,
                        hook,
                        command,
                        command_hash,
                        exit_code: Some(0),
                        outcome: HookOutcome::Ok,
                    });
                }
            }
            Report::Finished { run, message } => {
                let Some(active) = self.runs.remove(&run) else {
                    return events;
                };
                let repo_changed = self.repo_changed(&active);
                let mut op = Op {
                    id: run,
                    command: &active.command,
                    state: OperationState::Executing,
                };
                events.push(Event::HookFinished {
                    id: run,
                    message,
                    repo_changed,
                });
                self.advance(&mut op, OperationState::Done, &mut events);
            }
            Report::Failed { run, hook, failure } => {
                let Some(active) = self.runs.remove(&run) else {
                    return events;
                };
                let summary = hook::run::failure_summary(&hook, &failure);
                self.hook_log.push(HookRecord {
                    run,
                    command: active.script(&hook).0,
                    command_hash: active.script(&hook).1,
                    hook: hook.clone(),
                    exit_code: match &failure {
                        HookFailure::Exited { code, .. } => *code,
                        _ => None,
                    },
                    outcome: HookOutcome::Failed(summary.clone()),
                });
                events.push(Event::HookFailed {
                    id: run,
                    hook,
                    failure,
                });
                let mut op = Op {
                    id: run,
                    command: &active.command,
                    state: OperationState::Executing,
                };
                self.advance_with(&mut op, OperationState::Failed, Some(summary), &mut events);
            }
            Report::Cancelled { run, hook } => {
                let Some(active) = self.runs.remove(&run) else {
                    return events;
                };
                self.hook_log.push(HookRecord {
                    run,
                    command: active.script(&hook).0,
                    command_hash: active.script(&hook).1,
                    hook: hook.clone(),
                    exit_code: None,
                    outcome: HookOutcome::Cancelled,
                });
                events.push(Event::HookCancelled { id: run, hook });
                let mut op = Op {
                    id: run,
                    command: &active.command,
                    state: OperationState::Executing,
                };
                self.advance(&mut op, OperationState::Cancelled, &mut events);
            }
        }
        events
    }

    /// Starts the read a command asks for when it is one of the reads that run
    /// on a thread and answer later: the history, a commit, a stack of file
    /// diffs, a file search or a file. `None` for any other command. A read in
    /// flight of the same kind is superseded; the result carries the events of
    /// that cancellation.
    fn start_read(
        &mut self,
        id: OperationId,
        command: &Command,
    ) -> Option<Result<Vec<Event>, String>> {
        let (actor, kind) = (&command.actor, &command.kind);
        Some(match kind {
            CommandKind::LoadHistory => self.start_history(id, actor, kind, None),
            CommandKind::LoadHistoryPage { after } => {
                self.start_history(id, actor, kind, Some(after.clone()))
            }
            CommandKind::LoadCommitStack { oid, files } => {
                self.start_stack(id, actor, kind, oid, files)
            }
            CommandKind::LoadCommitDetail { oid } => self.start_detail(id, actor, kind, oid),
            CommandKind::SearchFiles { query } => self.start_files_search(id, actor, kind, query),
            CommandKind::ReadFile { path } => self.start_file_read(id, actor, kind, path),
            CommandKind::SearchText {
                query,
                options,
                paths,
            } => self.start_text_search(id, actor, kind, query, *options, paths),
            _ => return None,
        })
    }

    /// Runs one command to a terminal state and returns the events, in order.
    /// A `RunPrepareCommit` returns at `Executing`; it ends in `hook_report`.
    pub fn handle(&mut self, id: OperationId, command: &Command) -> Vec<Event> {
        let mut events = Vec::new();
        let mut op = Op {
            id,
            command,
            state: OperationState::Requested,
        };
        self.record(&op, None, &mut events);
        self.advance(&mut op, OperationState::Gating, &mut events);
        if let Some(started) = self.start_read(id, command) {
            match started {
                Ok(superseded) => {
                    events.extend(superseded);
                    self.advance(&mut op, OperationState::Executing, &mut events);
                }
                Err(message) => {
                    self.advance_with(&mut op, OperationState::Failed, Some(message), &mut events);
                }
            }
            return events;
        }
        if matches!(
            command.kind,
            CommandKind::Fetch | CommandKind::Pull | CommandKind::PushWithLease
        ) {
            match self.start_remote(id, &command.actor, &command.kind) {
                Ok(()) => self.advance(&mut op, OperationState::Executing, &mut events),
                Err(message) => {
                    self.advance_with(&mut op, OperationState::Failed, Some(message), &mut events);
                }
            }
            return events;
        }
        if let CommandKind::RunPrepareCommit { message } = &command.kind {
            match self.prepare_run(id, command, message) {
                Ok(accepted) => {
                    self.advance(&mut op, OperationState::Executing, &mut events);
                    self.runs.insert(id, accepted.active);
                    self.jobs.push(accepted.plan);
                }
                Err(message) => {
                    self.advance_with(&mut op, OperationState::Failed, Some(message), &mut events);
                }
            }
            return events;
        }
        // Gating has nothing to check for the other commands; the states are walked
        // so that every operation has the same shape in the log.
        self.advance(&mut op, OperationState::Executing, &mut events);
        match self.execute(id, &command.actor, &command.kind) {
            Ok(produced) => {
                events.extend(produced);
                self.advance(&mut op, OperationState::Done, &mut events);
            }
            Err(message) => {
                self.advance_with(&mut op, OperationState::Failed, Some(message), &mut events);
            }
        }
        events
    }

    fn execute(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::OpenRepo { path } => self.open_repo(id, path),
            CommandKind::RefreshStatus => {
                let root = self.open_root()?;
                let status = self.git.status(&root).map_err(|e| e.0)?;
                self.loaded(id, &root, status)
            }
            CommandKind::ListWorktrees => self.list_worktrees(id),
            CommandKind::LoadDiff { path, side } => {
                let root = self.open_root()?;
                let diff = self.git.diff(&root, path, *side).map_err(|e| e.0)?;
                Ok(vec![Event::DiffLoaded {
                    id,
                    side: *side,
                    diff,
                }])
            }
            CommandKind::Stage { target } => {
                let root = self.open_root()?;
                self.git.stage(&root, target).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::Unstage { target } => {
                let root = self.open_root()?;
                self.git.unstage(&root, target).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::DiscardHunk { path, hunk } => {
                let root = self.open_root()?;
                self.git.discard_hunk(&root, path, hunk).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::DiscardFile { .. } => self.discard_file(id, actor, kind),
            CommandKind::Commit { message } => {
                if message.trim().is_empty() {
                    return Err("the commit message is empty".to_owned());
                }
                let root = self.open_root()?;
                let oid = self.git.commit(&root, message).map_err(|e| e.0)?;
                let mut events = vec![Event::Committed { id, oid }];
                events.extend(self.reload_status(id, &root)?);
                Ok(events)
            }
            CommandKind::ListBranches => self.list_branches(id),
            CommandKind::IndexFiles => {
                self.start_index(id)?;
                Ok(Vec::new())
            }
            CommandKind::ListDirectory { dir } => self.list_directory(id, dir),
            CommandKind::SearchFiles { .. }
            | CommandKind::ReadFile { .. }
            | CommandKind::SearchText { .. } => Err("file reads are started by handle".to_owned()),
            CommandKind::LoadHistory | CommandKind::LoadHistoryPage { .. } => {
                Err("history is started by handle".to_owned())
            }
            CommandKind::LoadCommitDetail { .. } | CommandKind::LoadCommitStack { .. } => {
                Err("commit reads are started by handle".to_owned())
            }
            CommandKind::Fetch | CommandKind::Pull | CommandKind::PushWithLease => {
                Err("remote operations are started by handle".to_owned())
            }
            CommandKind::CreateWorktree { .. } => self.add_worktree(id, actor, kind),
            CommandKind::Checkout { branch } => {
                let root = self.open_root()?;
                self.git.checkout(&root, branch).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::ListHooks => {
                let root = self.open_root()?;
                let (branch, _) = self.current(&root)?;
                let hooks = self.list_hooks(&root, branch.as_deref())?;
                Ok(vec![Event::HooksListed { id, hooks }])
            }
            CommandKind::ApproveHook { name, command_hash } => {
                self.approve_hook(id, actor, name, command_hash)
            }
            CommandKind::RunPrepareCommit { .. } => {
                Err("a prepare-commit run is started by handle, not execute".to_owned())
            }
            CommandKind::CancelHook { run } => match self.runs.get(run) {
                Some(active) => {
                    active.cancel.store(true, Ordering::SeqCst);
                    Ok(Vec::new())
                }
                None => Err(format!("no hook run {} is active", run.0)),
            },
            CommandKind::StageAll => self.stage_all(id),
            CommandKind::ListWorkflows
            | CommandKind::RunWorkflow { .. }
            | CommandKind::ApproveWorkflowStep { .. }
            | CommandKind::CancelWorkflowRun { .. }
            | CommandKind::CreateWorkflowFromTemplate { .. }
            | CommandKind::ListWorkflowRuns { .. }
            | CommandKind::LoadWorkflowRun { .. }
            | CommandKind::DryRunWorkflow { .. }
            | CommandKind::SaveWorkflowFile { .. }
            | CommandKind::DeleteWorkflowFile { .. } => self.workflow_command(id, actor, kind),
            // The worktree and branch commands that `execute` does not list above.
            _ => self.manage_worktree(id, actor, kind),
        }
    }

    /// Checks a run and captures what it needs. Quick git reads only; the
    /// hooks themselves run later, on their own thread.
    fn prepare_run(
        &self,
        id: OperationId,
        command: &Command,
        message: &str,
    ) -> Result<Accepted, String> {
        let root = self.open_root()?;
        let (branch, head) = self.current(&root)?;
        let hooks = self.list_hooks(&root, branch.as_deref())?;
        let chain = hook::chain(&hooks)?;
        if chain.steps.is_empty() {
            return Err(match chain.awaiting_approval.first() {
                Some(name) => format!("hook {name} needs approval: list the hooks and approve it"),
                None => "no prepare-commit hook applies to this repository".to_owned(),
            });
        }
        let scripts = chain
            .steps
            .iter()
            .map(|hook| {
                (
                    hook.name.clone(),
                    (hook.command.clone(), hook.command_hash.clone()),
                )
            })
            .collect();
        let steps: Vec<Step> = chain
            .steps
            .into_iter()
            .map(|hook| Step {
                name: hook.name.clone(),
                command: hook.command.clone(),
                timeout: Duration::from_millis(hook.timeout_ms),
            })
            .collect();
        let staged_diff = self.git.staged_diff(&root).map_err(|e| e.0)?;
        let fingerprint = hook::fingerprint(head.as_deref(), &staged_diff);
        let cancel = Arc::new(AtomicBool::new(false));
        let plan = Plan {
            run: id,
            root: root.clone(),
            branch,
            head,
            staged_diff,
            message: message.to_owned(),
            steps,
            login_path: self.git.login_path(),
            cancel: Arc::clone(&cancel),
            log: self.git.command_log(),
        };
        let active = ActiveRun {
            command: command.clone(),
            root,
            fingerprint,
            cancel,
            scripts,
        };
        Ok(Accepted { active, plan })
    }

    fn list_hooks(
        &self,
        root: &std::path::Path,
        branch: Option<&str>,
    ) -> Result<Vec<gm_schema::HookInfo>, String> {
        hook::list(self.git.as_ref(), root, branch, &self.approvals).map_err(|e| e.0)
    }

    /// Branch name and HEAD commit, both `None` when they do not apply.
    fn current(&self, root: &std::path::Path) -> Result<(Option<String>, Option<String>), String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        Ok((status.branch.head, status.branch.oid))
    }

    /// Whether HEAD or the staged diff moved since the run started. When the
    /// repository cannot be read now, it counts as changed.
    fn repo_changed(&self, active: &ActiveRun) -> bool {
        let Ok((_, head)) = self.current(&active.root) else {
            return true;
        };
        let Ok(staged) = self.git.staged_diff(&active.root) else {
            return true;
        };
        hook::fingerprint(head.as_deref(), &staged) != active.fingerprint
    }

    /// Starts a page request. It continues the walk when the walk stands right
    /// after `after` (the list's last commit); otherwise it starts a walk from
    /// HEAD and reads past `after`, which is keyset resume and not `--skip`.
    /// A request in flight is superseded: its walk is stopped and it is
    /// cancelled. Returns the events of that cancellation.
    fn start_history(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        after: Option<String>,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let continues = match (&self.history, &after) {
            (Some(session), Some(after)) => {
                session.active.is_none()
                    && session.stream.is_some()
                    && session.last.as_deref() == Some(after.as_str())
            }
            _ => false,
        };
        let active = ActiveHistory {
            id,
            actor: actor.clone(),
            kind: kind.clone(),
        };
        if continues {
            let session = self.history.as_mut().expect("a session continues");
            let stream = session.stream.take();
            session.active = Some(active);
            self.history_jobs.push(HistoryJob {
                id,
                stream,
                after: None,
            });
            return Ok(Vec::new());
        }
        // The new walk starts before the old one is stopped, so a failure keeps the old one.
        let stream = self.git.history_stream(&root).map_err(|e| e.0)?;
        let stop: Canceller = match &stream {
            Some(stream) => stream.canceller(),
            None => std::sync::Arc::new(|| {}),
        };
        let superseded = self.drop_history();
        self.history = Some(HistorySession {
            stream: None,
            stop,
            last: None,
            active: Some(active),
        });
        self.history_jobs.push(HistoryJob { id, stream, after });
        Ok(superseded)
    }

    /// Checks a fetch, pull or push with lease and queues its run. The run starts
    /// on a thread after this returns, so the operation is `Executing` until its
    /// report. Refused for an agent, and while another remote operation runs.
    fn start_remote(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<(), String> {
        let what = match kind {
            CommandKind::Fetch => "fetch",
            CommandKind::Pull => "pull",
            _ => "push with lease",
        };
        only_a_human(actor, what)?;
        if self.remote.is_some() {
            return Err("a remote operation is already running".to_owned());
        }
        let root = self.open_root()?;
        let run = match kind {
            CommandKind::Fetch => self.git.fetch(&root),
            CommandKind::Pull => self.git.pull(&root),
            _ => self.git.push_with_lease(&root),
        }
        .map_err(|e| e.0)?;
        self.remote = Some(ActiveRemote {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            root,
        });
        self.remote_jobs.push(RemoteJob { id, run });
        Ok(())
    }

    /// Starts the read of commit `oid`. A read in flight is superseded: it is
    /// stopped and cancelled. Returns the events of that cancellation.
    fn start_detail(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        oid: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let read = self.git.commit_detail(&root, oid).map_err(|e| e.0)?;
        // The read is built before the old one is stopped, so a refusal keeps the old one.
        // A file diff of the commit list before this one is no longer wanted either.
        let mut superseded = self.drop_detail();
        superseded.extend(self.drop_stack());
        let stop = Arc::new(AtomicBool::new(false));
        self.detail = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.detail_jobs.push(DetailJob { id, read, stop });
        Ok(superseded)
    }

    /// Starts the diffs of the files of commit `oid` (H4c). A read in flight is
    /// superseded. Returns the events of that cancellation.
    fn start_stack(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        oid: &str,
        files: &[CommitFile],
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let read = self.git.commit_stack(&root, oid, files).map_err(|e| e.0)?;
        let superseded = self.drop_stack();
        let stop = Arc::new(AtomicBool::new(false));
        self.stack = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.stack_jobs.push(StackJob { id, read, stop });
        Ok(superseded)
    }

    /// Ends the stack read: it is stopped, and a request still in flight is cancelled.
    fn drop_stack(&mut self) -> Vec<Event> {
        let Some(active) = self.stack.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// The file list of the open repository, started afresh when `root` is not
    /// the repository it was made for.
    fn files_session(&mut self, root: &Path) -> &mut FilesSession {
        if !matches!(&self.files, Some(session) if session.root == root) {
            self.drop_files();
        }
        self.files
            .get_or_insert_with(|| FilesSession::new(root.to_path_buf()))
    }

    /// Reads the file list, unless a read is already in flight. `id` is the
    /// command that asked; `FilesIndexed` carries it.
    fn start_index(&mut self, id: OperationId) -> Result<(), String> {
        let root = self.open_root()?;
        if self.files_session(&root).building.is_some() {
            return Ok(());
        }
        let read = self.git.file_list(&root).map_err(|e| e.0)?;
        let stop = Arc::new(AtomicBool::new(false));
        let session = self.files_session(&root);
        let token = session.next_token();
        session.building = Some(Building {
            token,
            id,
            stop: Arc::clone(&stop),
        });
        self.index_jobs.push(IndexJob { token, read, stop });
        Ok(())
    }

    /// The first read of the list, when none was made and none is in flight.
    fn ensure_index(&mut self, id: OperationId) -> Result<(), String> {
        let root = self.open_root()?;
        let session = self.files_session(&root);
        if session.index.is_none() && session.building.is_none() {
            self.start_index(id)?;
        }
        Ok(())
    }

    /// One folder of the file tree, from the list in memory. With no list yet
    /// the listing is empty and says it is being read.
    fn list_directory(&mut self, id: OperationId, dir: &str) -> Result<Vec<Event>, String> {
        self.ensure_index(id)?;
        let root = self.open_root()?;
        let session = self.files_session(&root);
        let indexing = session.indexing();
        let listing = match &session.index {
            Some(index) => index.list_dir(dir, indexing),
            None => DirListing {
                dir: dir.trim_matches('/').to_owned(),
                indexing,
                ..DirListing::default()
            },
        };
        Ok(vec![Event::DirectoryListed { id, listing }])
    }

    /// Starts a search of the file list. A search in flight is superseded:
    /// stopped and cancelled. With no list yet the search waits for the first
    /// read. Returns the events of the cancellation.
    fn start_files_search(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        query: &str,
    ) -> Result<Vec<Event>, String> {
        self.ensure_index(id)?;
        let root = self.open_root()?;
        let session = self.files_session(&root);
        let index = session.index.clone();
        let indexing = session.indexing();
        let superseded = self.drop_files_search();
        let stop = Arc::new(AtomicBool::new(false));
        self.files_search = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        match index {
            Some(index) => self.search_jobs.push(SearchJob {
                id,
                index,
                query: query.to_owned(),
                indexing,
                stop,
            }),
            None => self.files_waiting = Some(query.to_owned()),
        }
        Ok(superseded)
    }

    /// Starts the read of file `path` for the viewer. A read in flight is
    /// superseded. Returns the events of the cancellation.
    fn start_file_read(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        path: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let superseded = self.drop_files_read();
        let stop = Arc::new(AtomicBool::new(false));
        self.files_read = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.read_jobs.push(ReadJob {
            id,
            root,
            path: path.to_owned(),
            stop,
        });
        Ok(superseded)
    }

    /// Starts a text search of the worktree. A search in flight is superseded:
    /// stopped and cancelled. Returns the events of the cancellation. A text
    /// Git cannot take is refused here, before anything is cancelled.
    fn start_text_search(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        query: &str,
        options: GrepOptions,
        paths: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let request = GrepRequest::new(query, options, paths)?;
        let read = self.git.grep_text(&root, &request).map_err(|e| e.0)?;
        let superseded = self.drop_text_search();
        let stop = Arc::new(AtomicBool::new(false));
        self.text_search = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.text_jobs.push(TextJob { id, read, stop });
        Ok(superseded)
    }

    /// Ends the text search: it is stopped, and a request in flight is cancelled.
    fn drop_text_search(&mut self) -> Vec<Event> {
        let Some(active) = self.text_search.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the file search: it is stopped, and a request in flight is cancelled.
    fn drop_files_search(&mut self) -> Vec<Event> {
        self.files_waiting = None;
        let Some(active) = self.files_search.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the file read: it is stopped, and a request in flight is cancelled.
    fn drop_files_read(&mut self) -> Vec<Event> {
        let Some(active) = self.files_read.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Forgets the file list and ends everything that reads it or the files.
    fn drop_files(&mut self) -> Vec<Event> {
        if let Some(building) = self.files.take().and_then(|session| session.building) {
            building.stop.store(true, Ordering::SeqCst);
        }
        let mut events = self.drop_files_search();
        events.extend(self.drop_files_read());
        events.extend(self.drop_text_search());
        events
    }

    /// Ends the commit detail read: it is stopped, and a request still in flight
    /// is cancelled.
    fn drop_detail(&mut self) -> Vec<Event> {
        let Some(active) = self.detail.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the history walk: its git process is stopped, and a page request
    /// still in flight is cancelled.
    fn drop_history(&mut self) -> Vec<Event> {
        let Some(session) = self.history.take() else {
            return Vec::new();
        };
        (session.stop)();
        match session.active {
            Some(active) => vec![self.finish_op(&active, OperationState::Cancelled, None)],
            None => Vec::new(),
        }
    }

    /// Records the end of a page request, as the log records every operation.
    fn finish_op(
        &mut self,
        active: &ActiveHistory,
        state: OperationState,
        detail: Option<String>,
    ) -> Event {
        self.log.push(LogEntry {
            id: active.id,
            actor: active.actor.clone(),
            kind: active.kind.clone(),
            state,
        });
        Event::OperationChanged {
            id: active.id,
            actor: active.actor.clone(),
            state,
            detail,
        }
    }

    /// The branch, HEAD and stash commands: create and delete a branch, check out a
    /// commit, read the reflog (B3) and the stashes (B2). Human only where a command
    /// writes.
    fn branch_command(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::CreateBranch {
                name,
                start_point,
                checkout,
            } => self.create_branch(id, actor, name, start_point.as_deref(), *checkout),
            CommandKind::DeleteBranch { name, force } => {
                self.delete_branch(id, actor, name, *force)
            }
            CommandKind::ListMergedBranches => self.list_merged(id),
            CommandKind::DeleteMergedBranches { branches } => {
                self.delete_merged(id, actor, branches)
            }
            CommandKind::CheckoutCommit { oid } => self.checkout_commit(id, actor, oid),
            CommandKind::LoadReflog { skip } => self.load_reflog(id, *skip),
            CommandKind::ListStashes => self.list_stashes(id),
            CommandKind::CreateStash { .. }
            | CommandKind::ApplyStash { .. }
            | CommandKind::PopStash { .. }
            | CommandKind::DropStash { .. } => self.stash_command(id, actor, kind),
            _ => Err("not a worktree or branch command".to_owned()),
        }
    }

    /// A new local branch, checked out or not. The status, the worktrees and the
    /// branch list follow, as after a checkout. Human only.
    fn create_branch(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        start_point: Option<&str>,
        checkout: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "create a branch")?;
        let root = self.open_root()?;
        if let Some(problem) = branch::name_problem(name) {
            return Err(problem.to_owned());
        }
        if let Some(problem) = start_point.and_then(branch::start_point_problem) {
            return Err(problem.to_owned());
        }
        self.git
            .create_branch(&root, name, start_point, checkout)
            .map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    /// Deletes a local branch, safely (`-d`) unless `force` (`-D`). A branch that a
    /// worktree has checked out is refused here, naming the worktree. Human only.
    fn delete_branch(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        force: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete a branch")?;
        let root = self.open_root()?;
        if let Some(problem) = branch::name_problem(name) {
            return Err(problem.to_owned());
        }
        let status = self.git.status(&root).map_err(|e| e.0)?;
        let checked_out_at = if status.branch.head.as_deref() == Some(name) {
            Some((root.to_string_lossy().into_owned(), true))
        } else {
            let worktrees = self.git.worktrees(&root).map_err(|e| e.0)?;
            worktrees
                .iter()
                .find(|entry| entry.branch.as_deref() == Some(name))
                .map(|entry| (entry.path.clone(), same_folder(&root, &entry.path)))
        };
        if let Some(refusal) = branch::delete_refusal(
            name,
            checked_out_at
                .as_ref()
                .map(|(path, open)| (path.as_str(), *open)),
        ) {
            return Err(refusal);
        }
        self.git
            .delete_branch(&root, name, force)
            .map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    /// The merged-branch list: the base the check used and the branches Git
    /// would delete. Reads only.
    fn list_merged(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let plan = self.merged_plan(&root)?;
        Ok(vec![Event::MergedBranchesListed {
            id,
            base: plan.base,
            branches: plan.branches,
        }])
    }

    /// Deletes the branches the human confirmed, with `git branch -d`, if the
    /// merged list still holds exactly those. Each branch is tried on its own: a
    /// refusal is reported and the others still go. Human only.
    fn delete_merged(
        &self,
        id: OperationId,
        actor: &Actor,
        names: &[String],
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete merged branches")?;
        let root = self.open_root()?;
        let mut now: Vec<String> = self
            .merged_plan(&root)?
            .branches
            .into_iter()
            .map(|branch| branch.name)
            .collect();
        let mut asked = names.to_vec();
        now.sort();
        asked.sort();
        if now != asked {
            return Err(MERGED_LIST_CHANGED.to_owned());
        }
        let mut deleted = Vec::new();
        let mut failures = Vec::new();
        for name in names {
            match self.git.delete_branch(&root, name, false) {
                Ok(()) => deleted.push(name.clone()),
                Err(e) => failures.push(BranchFailure {
                    name: name.clone(),
                    message: e.0,
                }),
            }
        }
        let mut events = self.branches_changed(id, &root)?;
        events.push(Event::MergedBranchesDeleted {
            id,
            deleted,
            failures,
        });
        Ok(events)
    }

    /// The base and the branches merged into it that may be deleted now. Git
    /// lists local branches only; the rules are `branch::merge_base` and
    /// `branch::is_merged_candidate`.
    fn merged_plan(&self, root: &Path) -> Result<MergedPlan, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        let current = status.branch.head.clone();
        let locals: Vec<String> = self
            .git
            .branches(root)
            .map_err(|e| e.0)?
            .into_iter()
            .filter(|entry| !entry.is_remote)
            .map(|entry| entry.name)
            .collect();
        let local_refs: Vec<&str> = locals.iter().map(String::as_str).collect();
        let origin = self
            .git
            .origin_head(root)
            .map_err(|e| e.0)?
            .as_deref()
            .and_then(branch::origin_head_branch)
            .map(str::to_owned);
        let Some(base) = branch::merge_base(origin.as_deref(), &local_refs, current.as_deref())
        else {
            return Ok(MergedPlan {
                base: None,
                branches: Vec::new(),
            });
        };
        let elsewhere: Vec<String> = self
            .git
            .worktrees(root)
            .map_err(|e| e.0)?
            .into_iter()
            .filter_map(|entry| entry.branch)
            .collect();
        let elsewhere_refs: Vec<&str> = elsewhere.iter().map(String::as_str).collect();
        let branches = self
            .git
            .merged_branches(root, &base)
            .map_err(|e| e.0)?
            .into_iter()
            .filter(|merged| local_refs.contains(&merged.name.as_str()))
            .filter(|merged| {
                branch::is_merged_candidate(
                    &merged.name,
                    &base,
                    current.as_deref(),
                    &elsewhere_refs,
                )
            })
            .collect();
        Ok(MergedPlan {
            base: Some(base),
            branches,
        })
    }

    /// The status, the worktrees and the branch list, after a branch changed.
    fn branches_changed(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        let mut events = self.loaded(id, root, status)?;
        let branches = self.git.branches(root).map_err(|e| e.0)?;
        let user_email = self.git.user_email(root).map_err(|e| e.0)?;
        events.push(Event::BranchesLoaded {
            id,
            branches,
            user_email,
        });
        Ok(events)
    }

    fn list_worktrees(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let worktrees = self.git.worktrees(&root).map_err(|e| e.0)?;
        Ok(vec![Event::WorktreesLoaded { id, worktrees }])
    }

    /// The branch list, with the configured user email so the UI can tell
    /// which branches are the user's own.
    /// One page of the reflog of HEAD (B3, D95). One entry more than a page is
    /// read, so `has_more` is known without a second run. Reads only.
    fn load_reflog(&self, id: OperationId, skip: u32) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let mut entries = self
            .git
            .reflog(&root, skip, REFLOG_PAGE + 1)
            .map_err(|e| e.0)?;
        let has_more = entries.len() > REFLOG_PAGE;
        entries.truncate(REFLOG_PAGE);
        Ok(vec![Event::ReflogLoaded {
            id,
            skip,
            entries,
            has_more,
        }])
    }

    /// Detaches HEAD at a commit, as a reflog entry is checked out (B3, D95). The
    /// id is checked before Git runs. The status and the branch list follow, as
    /// after a branch change. Human only.
    fn checkout_commit(
        &self,
        id: OperationId,
        actor: &Actor,
        oid: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "check out a commit")?;
        if let Some(problem) = reflog::oid_problem(oid) {
            return Err(problem.to_owned());
        }
        let root = self.open_root()?;
        self.git.checkout_commit(&root, oid).map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    fn list_branches(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let branches = self.git.branches(&root).map_err(|e| e.0)?;
        let user_email = self.git.user_email(&root).map_err(|e| e.0)?;
        Ok(vec![Event::BranchesLoaded {
            id,
            branches,
            user_email,
        }])
    }

    /// A new linked worktree; the list and the status follow, as after a checkout.
    fn add_worktree(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        let CommandKind::CreateWorktree { path, mode } = kind else {
            return Err("not a worktree command".to_owned());
        };
        // Worktrees have no agent path yet (D31, Q12).
        if !matches!(actor, Actor::Human) {
            return Err("only a human can create a worktree".to_owned());
        }
        let root = self.open_root()?;
        self.git
            .create_worktree(&root, path, mode)
            .map_err(|e| e.0)?;
        let status = self.git.status(&root).map_err(|e| e.0)?;
        self.loaded(id, &root, status)
    }

    /// The worktree commands after the listing: inspect, remove, lock, unlock,
    /// and the prune pair. Any other kind is passed on to `branch_command`.
    fn manage_worktree(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::InspectWorktree { path } => {
                let root = self.open_root()?;
                let inspection = self.inspect(&root, path)?;
                Ok(vec![Event::WorktreeInspected { id, inspection }])
            }
            CommandKind::RemoveWorktree { path, force } => {
                self.remove_worktree(id, actor, path, *force)
            }
            CommandKind::LockWorktree { path, reason } => {
                only_a_human(actor, "lock a worktree")?;
                let root = self.open_root()?;
                self.listed(&root, path)?;
                self.git
                    .lock_worktree(&root, path, reason)
                    .map_err(|e| e.0)?;
                self.reloaded(id, &root)
            }
            CommandKind::UnlockWorktree { path } => {
                only_a_human(actor, "unlock a worktree")?;
                let root = self.open_root()?;
                self.listed(&root, path)?;
                self.git.unlock_worktree(&root, path).map_err(|e| e.0)?;
                self.reloaded(id, &root)
            }
            CommandKind::ListPrunable => {
                let root = self.open_root()?;
                let candidates = self.git.prunable(&root).map_err(|e| e.0)?;
                Ok(vec![Event::PrunableListed { id, candidates }])
            }
            CommandKind::PruneWorktrees { names } => self.prune(id, actor, names),
            _ => self.branch_command(id, actor, kind),
        }
    }

    /// The listed worktree at `path`; any other path is refused before Git runs.
    fn listed(&self, root: &Path, path: &str) -> Result<WorktreeEntry, String> {
        self.git
            .worktrees(root)
            .map_err(|e| e.0)?
            .into_iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| format!("{path} is not a worktree of this repository"))
    }

    /// What removing the worktree at `path` would lose or refuse. Reads only;
    /// the same state gives the same inspection, with no timestamp in it.
    fn inspect(&self, root: &Path, path: &str) -> Result<WorktreeInspection, String> {
        let entry = self.listed(root, path)?;
        let folder = Path::new(path);
        let missing = !folder.is_dir();
        let (mut staged, mut unstaged, mut untracked, mut unpushed) = (0, 0, 0, 0);
        if !missing {
            let status = self.git.status(folder).map_err(|e| e.0)?;
            for change in &status.entries {
                staged += u32::from(change.staged.is_some());
                unstaged += u32::from(change.unstaged.is_some());
                untracked += u32::from(change.untracked);
            }
            unpushed = self
                .git
                .unpushed_commits(folder, entry.branch.as_deref())
                .map_err(|e| e.0)?;
        }
        let blockers = worktree::blockers([
            (entry.is_main, Blocker::Main),
            (
                self.root
                    .as_deref()
                    .is_some_and(|open| same_folder(open, path)),
                Blocker::Current,
            ),
            (missing, Blocker::Missing),
            (entry.locked.is_some(), Blocker::Locked),
            (staged + unstaged + untracked > 0, Blocker::Dirty),
            (unpushed > 0, Blocker::Unpushed),
        ]);
        Ok(WorktreeInspection {
            path: path.to_owned(),
            branch: entry.branch.clone(),
            name: worktree::name_of(path, entry.branch.as_deref()),
            locked: entry.locked.clone(),
            missing,
            staged,
            unstaged,
            untracked,
            unpushed,
            blockers,
        })
    }

    /// Removes a worktree after the rule of D31 checks it again, now: the
    /// dialog's earlier inspection is not trusted.
    fn remove_worktree(
        &self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        force: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "remove a worktree")?;
        let root = self.open_root()?;
        let inspection = self.inspect(&root, path)?;
        inspection.check_removal(force)?;
        // A locked worktree needs Git's second `--force`.
        let level = if inspection.blockers.is_empty() {
            0
        } else if inspection.locked.is_some() {
            2
        } else {
            1
        };
        self.git
            .remove_worktree(&root, path, level)
            .map_err(|e| e.0)?;
        self.reloaded(id, &root)
    }

    /// Removes the admin records named in `names`, if the dry run still lists
    /// exactly those.
    fn prune(
        &self,
        id: OperationId,
        actor: &Actor,
        names: &[String],
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "prune worktrees")?;
        let root = self.open_root()?;
        let mut now: Vec<String> = self
            .git
            .prunable(&root)
            .map_err(|e| e.0)?
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        let mut expected = names.to_vec();
        now.sort();
        expected.sort();
        if now != expected {
            return Err(
                "the worktrees to prune changed since they were listed; list them again".to_owned(),
            );
        }
        self.git.prune_worktrees(&root).map_err(|e| e.0)?;
        self.reloaded(id, &root)
    }

    /// The status and the worktree list, after a change to the worktrees.
    fn reloaded(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        self.loaded(id, root, status)
    }

    /// Opens the repository at `path`. The history walk of the old one ends.
    fn open_repo(&mut self, id: OperationId, path: &str) -> Result<Vec<Event>, String> {
        let root = self.git.resolve_root(path.as_ref()).map_err(|e| e.0)?;
        let status = self.git.status(&root).map_err(|e| e.0)?;
        let mut events = self.loaded(id, &root, status)?;
        events.extend(self.drop_history());
        events.extend(self.drop_detail());
        events.extend(self.drop_stack());
        events.extend(self.drop_files());
        self.root = Some(root);
        Ok(events)
    }

    fn open_root(&self) -> Result<PathBuf, String> {
        self.root
            .clone()
            .ok_or_else(|| "no repository is open".to_owned())
    }

    /// The stash list, newest first. Reads only.
    fn list_stashes(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let stashes = self.git.stashes(&root).map_err(|e| e.0)?;
        Ok(vec![Event::StashesListed { id, stashes }])
    }

    /// The stash writes (B2, D94). Human only. An apply, a pop or a drop first
    /// checks that `stash@{index}` still is the stash the human listed, so a
    /// stale index never acts on another stash.
    fn stash_command(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::CreateStash {
                message,
                include_untracked,
            } => {
                only_a_human(actor, "create a stash")?;
                let root = self.open_root()?;
                let message = message
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty());
                self.git
                    .create_stash(&root, message, *include_untracked)
                    .map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::ApplyStash { index, oid } => {
                only_a_human(actor, "apply a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.apply_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::PopStash { index, oid } => {
                only_a_human(actor, "pop a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.pop_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::DropStash { index, oid } => {
                only_a_human(actor, "drop a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.drop_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            _ => Err("not a stash command".to_owned()),
        }
    }

    /// Refuses unless `stash@{index}` still is the stash `oid` the human listed.
    fn ensure_stash_listed(&self, root: &Path, index: u32, oid: &str) -> Result<(), String> {
        match self.git.resolve_stash(root, index).map_err(|e| e.0)? {
            Some(now) if now == oid => Ok(()),
            _ => Err(STASH_LIST_CHANGED.to_owned()),
        }
    }

    /// The status and the stash list after a stash write, so no front-end asks.
    fn stashes_changed(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let mut events = self.reload_status(id, root)?;
        let stashes = self.git.stashes(root).map_err(|e| e.0)?;
        events.push(Event::StashesListed { id, stashes });
        Ok(events)
    }

    /// Discards every change of one file (B1, D93). Human only: the loss is
    /// permanent. The status follows, as after a stage.
    fn discard_file(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        let CommandKind::DiscardFile {
            path,
            include_staged,
        } = kind
        else {
            return Err("not a discard command".to_owned());
        };
        only_a_human(actor, "discard changes")?;
        let root = self.open_root()?;
        self.git
            .discard_file(&root, path, *include_staged)
            .map_err(|e| e.0)?;
        self.reload_status(id, &root)
    }

    /// A mutation ends by showing the new state, so no front-end has to ask.
    /// The status and the worktree list of one read, so both reach the UI together.
    fn loaded(
        &self,
        id: OperationId,
        root: &std::path::Path,
        status: gm_schema::RepoStatus,
    ) -> Result<Vec<Event>, String> {
        let worktrees = self.git.worktrees(root).map_err(|e| e.0)?;
        Ok(vec![
            Event::StatusLoaded { id, status },
            Event::WorktreesLoaded { id, worktrees },
        ])
    }

    fn reload_status(&self, id: OperationId, root: &std::path::Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        Ok(vec![Event::StatusLoaded { id, status }])
    }

    /// Approves the repository hook `name` at `command_hash`. Human only.
    fn approve_hook(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        command_hash: &str,
    ) -> Result<Vec<Event>, String> {
        // Approval is the human's decision (docs/00); an agent may only propose.
        if !matches!(actor, Actor::Human) {
            return Err("only a human can approve a hook".to_owned());
        }
        let root = self.open_root()?;
        let (branch, _) = self.current(&root)?;
        let hooks = self.list_hooks(&root, branch.as_deref())?;
        let hook = hooks
            .iter()
            .find(|h| h.scope == HookScope::Repo && h.name == name)
            .ok_or_else(|| format!("no repository hook named {name}"))?;
        if hook.command_hash != command_hash {
            return Err(format!(
                "the command of {name} changed since the hooks were listed; list them again"
            ));
        }
        self.approvals
            .approve(&root.to_string_lossy(), name, command_hash)?;
        let hooks = self.list_hooks(&root, branch.as_deref())?;
        Ok(vec![Event::HooksListed { id, hooks }])
    }

    /// Stages every change of the working tree, then reads the status again.
    fn stage_all(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        self.git.stage_all(&root).map_err(|e| e.0)?;
        self.reload_status(id, &root)
    }

    /// The workflow commands: the listing, and a run's start, approval and cancel.
    fn workflow_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::RunWorkflow { name, inputs } => self.run_workflow(id, actor, name, inputs),
            CommandKind::ApproveWorkflowStep { run, approved } => {
                self.approve_workflow_step(id, actor, *run, *approved)
            }
            CommandKind::CancelWorkflowRun { run } => self.cancel_workflow_run(id, actor, *run),
            CommandKind::ListWorkflows => self.list_workflows(id),
            CommandKind::CreateWorkflowFromTemplate { template } => {
                self.create_workflow_from_template(id, actor, template)
            }
            CommandKind::ListWorkflowRuns { workflow, limit } => {
                self.list_workflow_runs(id, workflow, *limit)
            }
            CommandKind::LoadWorkflowRun { run } => self.load_workflow_run(id, *run),
            CommandKind::DryRunWorkflow { source, inputs } => {
                self.dry_run_workflow(id, source, inputs)
            }
            CommandKind::SaveWorkflowFile {
                path,
                source,
                expected_hash,
            } => self.save_workflow_file(id, actor, path, source, expected_hash.as_deref()),
            CommandKind::DeleteWorkflowFile {
                path,
                expected_hash,
            } => self.delete_workflow_file(id, actor, path, expected_hash),
            _ => Err("not a workflow command".to_owned()),
        }
    }

    /// Opens the workflow session of the open repository, unless it is open, and
    /// resumes the unfinished runs its database holds (D80). Its events come first.
    fn ensure_workflows(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        if self
            .workflow
            .as_ref()
            .is_some_and(|session| session.root == root)
        {
            return Ok(Vec::new());
        }
        let common = self.git.common_dir(&root).map_err(|e| e.0)?;
        let store = Store::open(&common.join("gitmaster").join("state.db")).map_err(|e| e.0)?;
        let mut session = Session::new(root.clone(), store, id);
        session.driver.set_login_path(self.git.login_path());
        let mut actions = Actions::new(self.git.as_ref(), &root, &mut self.log, id);
        let outcome = session.driver.resume(&mut actions, now_ms());
        let wrote = actions.wrote();
        let events = session.absorb(outcome);
        self.workflow = Some(session);
        Ok(self.workflow_events(id, &root, events, wrote))
    }

    /// The workflow files as the front-ends list them. Reads only.
    fn list_workflows(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let mut events = self.ensure_workflows(id)?;
        let root = self.open_root()?;
        events.push(self.workflows_listed(id, &root)?);
        Ok(events)
    }

    /// The listing of the workflow files, as `ListWorkflows` answers it.
    fn workflows_listed(&self, id: OperationId, root: &Path) -> Result<Event, String> {
        let loaded =
            workflow_session::load(self.git.as_ref(), root, self.workflow_user_dir.as_deref())?;
        Ok(Event::WorkflowsListed {
            id,
            workflows: loaded
                .entries
                .iter()
                .map(workflow_session::workflow_info)
                .collect(),
            ignored: workflow_session::ignored_lines(&loaded.ignored),
            user_dir: self
                .workflow_user_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
            templates: templates::all()
                .iter()
                .map(|template| template.name.to_owned())
                .collect(),
        })
    }

    /// The newest `limit` runs of `workflow` in the open repository. Reads only.
    fn list_workflow_runs(
        &mut self,
        id: OperationId,
        workflow: &str,
        limit: u32,
    ) -> Result<Vec<Event>, String> {
        let mut events = self.ensure_workflows(id)?;
        let session = self
            .workflow
            .as_ref()
            .ok_or_else(|| "the workflow session is not open".to_owned())?;
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let rows = session
            .driver
            .store()
            .runs(workflow, limit.min(RETAINED_RUNS))
            .map_err(|e| e.0)?;
        events.push(Event::WorkflowRunsListed {
            id,
            workflow: workflow.to_owned(),
            runs: rows.iter().map(workflow_session::run_summary).collect(),
        });
        Ok(events)
    }

    /// One run with every step it took. Reads only.
    fn load_workflow_run(&mut self, id: OperationId, run: i64) -> Result<Vec<Event>, String> {
        let mut events = self.ensure_workflows(id)?;
        let session = self
            .workflow
            .as_ref()
            .ok_or_else(|| "the workflow session is not open".to_owned())?;
        let detail = workflow_session::run_detail(session.driver.store(), run)?;
        events.push(Event::WorkflowRunLoaded {
            id,
            detail: Box::new(detail),
        });
        Ok(events)
    }

    /// Dry run of the workflow text `source` (the editor's, not a saved file):
    /// walks it without running a step. Reads only. The workflow session is not
    /// opened, so nothing is resumed and the store is not touched.
    fn dry_run_workflow(
        &self,
        id: OperationId,
        source: &str,
        inputs: &serde_json::Value,
    ) -> Result<Vec<Event>, String> {
        let dry_run = match workflow::load(source) {
            Ok((definition, _)) => {
                let root = self.open_root()?;
                workflow_session::dry_run(self.git.as_ref(), &root, &definition, inputs)
            }
            Err(diagnostics) => workflow_session::dry_run_error(&diagnostics),
        };
        Ok(vec![Event::WorkflowDryRun {
            id,
            dry_run: Box::new(dry_run),
        }])
    }

    /// Writes the workflow file at `path`, which must be in a workflows directory
    /// (never a repository). With `expected_hash` the file on disk must still hash
    /// to it; without, the file must not exist. Human only. It writes no Git
    /// state, so it is not in the command log.
    fn save_workflow_file(
        &mut self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        source: &str,
        expected_hash: Option<&str>,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "save a workflow file")?;
        let root = self.open_root()?;
        let dirs = workflow_session::workflow_dirs(
            self.git.as_ref(),
            &root,
            self.workflow_user_dir.as_deref(),
        )?;
        if let Some(user) = self.workflow_user_dir.as_deref()
            && Path::new(path).parent() == Some(user)
            && !user.exists()
        {
            std::fs::create_dir_all(user)
                .map_err(|e| format!("could not create {}: {e}", user.display()))?;
        }
        let file = workflow_session::workflow_file(&root, &dirs, path)?;
        if u64::try_from(source.len()).unwrap_or(u64::MAX) > crate::workflow::loader::MAX_FILE {
            return Err("the workflow is larger than 256 KiB".to_owned());
        }
        match expected_hash {
            Some(hash) => check_hash(&file, hash)?,
            None if std::fs::symlink_metadata(&file).is_ok() => {
                return Err(format!("a file already exists at {}", file.display()));
            }
            None => {}
        }
        workflow_session::write_atomically(&file, source)?;
        Ok(vec![
            Event::WorkflowFileSaved {
                id,
                path: path.to_owned(),
                hash: crate::workflow::loader::sha256_hex(source.as_bytes()),
            },
            self.workflows_listed(id, &root)?,
        ])
    }

    /// Moves the workflow file at `path` to the trash folder, when it still hashes
    /// to `expected_hash`. Human only. Like a save, it is not in the command log.
    fn delete_workflow_file(
        &mut self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        expected_hash: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete a workflow file")?;
        let root = self.open_root()?;
        let dirs = workflow_session::workflow_dirs(
            self.git.as_ref(),
            &root,
            self.workflow_user_dir.as_deref(),
        )?;
        let file = workflow_session::workflow_file(&root, &dirs, path)?;
        check_hash(&file, expected_hash)?;
        let trash = self
            .workflow_trash_dir
            .as_deref()
            .ok_or_else(|| "there is no trash folder for workflow files".to_owned())?;
        let trashed_to = workflow_session::trash_file(&file, trash, now_ms())?;
        Ok(vec![
            Event::WorkflowFileDeleted {
                id,
                path: path.to_owned(),
                trashed_to: trashed_to.display().to_string(),
            },
            self.workflows_listed(id, &root)?,
        ])
    }

    /// Copies a built-in template into the user's workflows directory. It writes
    /// no Git state, so it is not in the command log. Human only.
    fn create_workflow_from_template(
        &self,
        id: OperationId,
        actor: &Actor,
        template_name: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "create a workflow from a template")?;
        let template = templates::all()
            .iter()
            .find(|template| template.name == template_name)
            .ok_or_else(|| format!("no template named `{template_name}`"))?;
        let dir = self
            .workflow_user_dir
            .as_deref()
            .ok_or_else(|| "there is no user workflows directory (HOME is not set)".to_owned())?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let created = templates::create(dir, template)?;
        Ok(vec![Event::WorkflowCreated {
            id,
            path: created.path.display().to_string(),
            name: created.name,
        }])
    }

    /// Starts a manual workflow run for a human. The run's own events follow;
    /// the operation ends once it has started.
    fn run_workflow(
        &mut self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<Vec<Event>, String> {
        if !matches!(actor, Actor::Human) {
            return Err("agents run workflows through MCP in a later slice".to_owned());
        }
        let mut events = self.ensure_workflows(id)?;
        let root = self.open_root()?;
        let loaded =
            workflow_session::load(self.git.as_ref(), &root, self.workflow_user_dir.as_deref())?;
        let entry = loaded
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("no workflow named `{name}`"))?;
        let repo = workflow_session::repo_snapshot(self.git.as_ref(), &root);
        let inputs = serde_json::to_value(inputs).map_err(|e| e.to_string())?;
        let session = self
            .workflow
            .as_mut()
            .ok_or_else(|| "the workflow session is not open".to_owned())?;
        let mut actions = Actions::new(self.git.as_ref(), &root, &mut self.log, id);
        let (run, outcome) = session
            .driver
            .start(entry, inputs, repo, &mut actions, now_ms())?;
        let wrote = actions.wrote();
        session.started(run, id);
        events.extend(session.absorb(outcome));
        Ok(self.workflow_events(id, &root, events, wrote))
    }

    /// The human's answer to the `approve` step a run waits at.
    fn approve_workflow_step(
        &mut self,
        id: OperationId,
        actor: &Actor,
        run: i64,
        approved: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "approve a workflow step")?;
        let mut events = self.ensure_workflows(id)?;
        let root = self.open_root()?;
        let session = self
            .workflow
            .as_mut()
            .ok_or_else(|| format!("run {run} is not awaiting approval"))?;
        let mut actions = Actions::new(self.git.as_ref(), &root, &mut self.log, id);
        let outcome = session
            .driver
            .approve(run, approved, &mut actions, now_ms())?;
        let wrote = actions.wrote();
        events.extend(session.absorb(outcome));
        Ok(self.workflow_events(id, &root, events, wrote))
    }

    /// Stops a workflow run; its `exec` step, if any, is killed.
    fn cancel_workflow_run(
        &mut self,
        id: OperationId,
        actor: &Actor,
        run: i64,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "cancel a workflow run")?;
        let mut events = self.ensure_workflows(id)?;
        let session = self
            .workflow
            .as_mut()
            .ok_or_else(|| format!("run {run} is not active"))?;
        let outcome = session.driver.cancel(run, now_ms())?;
        events.extend(session.absorb(outcome));
        Ok(events)
    }

    /// The events of a workflow call. Once Git was written, the status is read
    /// again, so the window shows the change.
    fn workflow_events(
        &self,
        id: OperationId,
        root: &Path,
        mut events: Vec<Event>,
        wrote: bool,
    ) -> Vec<Event> {
        if wrote && let Ok(status) = self.git.status(root) {
            events.push(Event::StatusLoaded { id, status });
        }
        events
    }

    fn advance(&mut self, op: &mut Op, to: OperationState, events: &mut Vec<Event>) {
        self.advance_with(op, to, None, events);
    }

    fn advance_with(
        &mut self,
        op: &mut Op,
        to: OperationState,
        detail: Option<String>,
        events: &mut Vec<Event>,
    ) {
        op.state = op
            .state
            .move_to(to)
            .expect("the engine only takes legal transitions");
        self.record(op, detail, events);
    }

    fn record(&mut self, op: &Op, detail: Option<String>, events: &mut Vec<Event>) {
        // A workflow listing or run read is a read (D28), as is the stash list, and a
        // template copy or a workflow file save or delete writes no Git state: none
        // is logged.
        if !matches!(
            op.command.kind,
            CommandKind::ListStashes
                | CommandKind::ListWorkflows
                | CommandKind::LoadReflog { .. }
                | CommandKind::ListWorkflowRuns { .. }
                | CommandKind::LoadWorkflowRun { .. }
                | CommandKind::DryRunWorkflow { .. }
                | CommandKind::CreateWorkflowFromTemplate { .. }
                | CommandKind::SaveWorkflowFile { .. }
                | CommandKind::DeleteWorkflowFile { .. }
        ) {
            self.log.push(LogEntry {
                id: op.id,
                actor: op.command.actor.clone(),
                kind: op.command.kind.clone(),
                state: op.state,
            });
        }
        events.push(Event::OperationChanged {
            id: op.id,
            actor: op.command.actor.clone(),
            state: op.state,
            detail,
        });
    }
}

/// The base the merged check used, and the branches it would delete.
struct MergedPlan {
    base: Option<String>,
    branches: Vec<MergedBranch>,
}

/// What a finished remote operation says in its `Done` event.
fn success_message(kind: &CommandKind) -> &'static str {
    match kind {
        CommandKind::Fetch => "Fetched",
        CommandKind::Pull => "Pulled",
        _ => "Pushed",
    }
}

/// Milliseconds since the Unix epoch: the clock of workflow runs.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// What a save or delete answers when the file moved on since it was read.
const CHANGED_ON_DISK: &str = "the file changed on disk; reload it";

/// Refuses unless the file at `file` still hashes to `expected`.
fn check_hash(file: &Path, expected: &str) -> Result<(), String> {
    match workflow_session::file_hash(file) {
        Some(actual) if actual == expected => Ok(()),
        _ => Err(CHANGED_ON_DISK.to_owned()),
    }
}

/// Worktree changes are human decisions (D31, Q12): an agent may not make one.
fn only_a_human(actor: &Actor, what: &str) -> Result<(), String> {
    if matches!(actor, Actor::Human) {
        Ok(())
    } else {
        Err(format!("only a human can {what}"))
    }
}

/// Whether two paths name the same folder, through symbolic links when both exist.
fn same_folder(open: &Path, path: &str) -> bool {
    let path = Path::new(path);
    match (std::fs::canonicalize(open), std::fs::canonicalize(path)) {
        (Ok(a), Ok(b)) => a == b,
        _ => open == path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitError;
    use crate::fake::FakeGit;
    use gm_schema::{RepoStatus, WorktreeMode};
    use proptest::prelude::*;

    fn engine() -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        Engine::new(Box::new(FakeGit::new("/r", status)))
    }

    fn command(kind: CommandKind) -> Command {
        Command {
            actor: Actor::Human,
            kind,
        }
    }

    fn open() -> Command {
        command(CommandKind::OpenRepo { path: "/r".into() })
    }

    fn states(events: &[Event]) -> Vec<OperationState> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::OperationChanged { state, .. } => Some(*state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn open_walks_to_done_and_loads_status() {
        let events = engine().handle(OperationId(1), &open());
        assert_eq!(
            states(&events),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing,
                OperationState::Done
            ]
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { .. }))
        );
    }

    #[test]
    fn refresh_without_a_repo_fails_with_a_reason() {
        let events = engine().handle(OperationId(1), &command(CommandKind::RefreshStatus));
        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(detail.as_deref(), Some("no repository is open"));
    }

    #[test]
    fn list_branches_emits_the_branches_it_read() {
        let branches = vec![gm_schema::BranchEntry {
            name: "main".into(),
            is_remote: false,
            is_current: true,
            upstream: None,
            author_email: None,
        }];
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_branches(Ok(branches.clone()));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::ListBranches));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(events.contains(&Event::BranchesLoaded {
            id: OperationId(2),
            branches,
            user_email: None,
        }));
    }

    fn commit_of(oid: &str) -> gm_schema::CommitSummary {
        gm_schema::CommitSummary {
            oid: oid.into(),
            parents: Vec::new(),
            short: oid.chars().take(7).collect(),
            subject: oid.into(),
            refs: Vec::new(),
            author_time: 0,
        }
    }

    /// An engine over a fake repository whose HEAD has `oids`, newest first.
    fn history_engine(oids: &[&str]) -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_history(Ok(gm_schema::History {
            commits: oids.iter().map(|oid| commit_of(oid)).collect(),
            unborn: false,
            more: false,
        }));
        Engine::new(Box::new(git))
    }

    /// Runs every history job the engine accepted, as the runner's threads do.
    fn run_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_history_jobs()
            .into_iter()
            .flat_map(|job| engine.history_report(job.run()))
            .collect()
    }

    #[test]
    fn a_history_load_answers_once_its_page_is_read() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        let started = engine.handle(OperationId(2), &command(CommandKind::LoadHistory));
        assert_eq!(
            states(&started),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing
            ]
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::HistoryLoaded { .. }))
        );

        let events = run_jobs(&mut engine);

        let Some(Event::HistoryLoaded { id, history }) = events
            .iter()
            .find(|e| matches!(e, Event::HistoryLoaded { .. }))
        else {
            panic!("the page is not answered, got {events:?}");
        };
        assert_eq!(*id, OperationId(2));
        assert_eq!(history.commits.len(), 3);
        assert!(!history.more);
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    #[test]
    fn a_superseded_history_load_is_dropped_and_cancelled() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(CommandKind::LoadHistory));
        let superseding = engine.handle(OperationId(3), &command(CommandKind::LoadHistory));
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let jobs = engine.take_history_jobs();
        assert_eq!(jobs.len(), 2);
        let mut jobs = jobs.into_iter();
        let stale = engine.history_report(jobs.next().unwrap().run());
        assert!(
            stale.is_empty(),
            "the superseded page must not show: {stale:?}"
        );
        let current = engine.history_report(jobs.next().unwrap().run());
        assert!(matches!(
            current.first(),
            Some(Event::OperationChanged {
                id: OperationId(3),
                state: OperationState::Done,
                ..
            })
        ));
        assert!(current.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_page_after_the_last_commit_continues_the_walk() {
        let oids = ["c", "b", "a"];
        let mut engine = history_engine(&oids);
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(CommandKind::LoadHistory));
        run_jobs(&mut engine);

        // The first page held every commit, so the next one is the walk's end.
        let next = engine.handle(
            OperationId(3),
            &command(CommandKind::LoadHistoryPage { after: "a".into() }),
        );
        assert!(next.contains(&Event::OperationChanged {
            id: OperationId(3),
            actor: Actor::Human,
            state: OperationState::Executing,
            detail: None,
        }));
        let events = run_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded { id: OperationId(3), history } if history.commits.is_empty()
        )));
    }

    #[test]
    fn a_page_for_an_unknown_cursor_restarts_the_walk_past_it() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        // No page was read, so the cursor "b" matches no walk: the walk restarts and reads past it.
        engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistoryPage { after: "b".into() }),
        );
        let events = run_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded { id: OperationId(2), history }
                if history.commits.len() == 1 && history.commits[0].oid == "a"
        )));
    }

    fn detail_engine() -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_detail(Ok(gm_schema::CommitDetail {
            oid: "a".repeat(40),
            subject: "the subject".into(),
            ..gm_schema::CommitDetail::default()
        }));
        Engine::new(Box::new(git))
    }

    fn load_detail(oid: &str) -> CommandKind {
        CommandKind::LoadCommitDetail { oid: oid.into() }
    }

    /// Runs every detail job the engine accepted, as the runner's threads do.
    fn run_detail_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_detail_jobs()
            .into_iter()
            .flat_map(|job| engine.detail_report(job.run()))
            .collect()
    }

    #[test]
    fn a_commit_detail_answers_once_it_is_read() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        let started = engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        assert_eq!(
            states(&started),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing
            ]
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::CommitDetailLoaded { .. }))
        );

        let events = run_detail_jobs(&mut engine);

        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitDetailLoaded { id: OperationId(2), detail } if detail.subject == "the subject"
        )));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    fn stack_engine() -> (Engine, FakeGit) {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_stack(Ok(vec![gm_schema::FileDiff {
            path: "a.txt".into(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        }]));
        (Engine::new(Box::new(git.clone())), git)
    }

    /// A stack request for commit `oid` that lists one file, `path`.
    fn load_file(oid: &str, path: &str) -> CommandKind {
        CommandKind::LoadCommitStack {
            oid: oid.into(),
            files: vec![CommitFile {
                path: path.into(),
                old_path: None,
                change: gm_schema::Change::Modified,
                added: 1,
                deleted: 1,
                binary: false,
            }],
        }
    }

    /// Runs every stack job the engine accepted, as the runner's threads do.
    fn run_stack_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_stack_jobs()
            .into_iter()
            .flat_map(|job| engine.stack_report(job.run()))
            .collect()
    }

    #[test]
    fn a_commit_stack_answers_once_it_is_read() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        let started = engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::CommitStackLoaded { .. }))
        );

        let events = run_stack_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitStackLoaded { id: OperationId(2), diffs } if diffs[0].path == "a.txt"
        )));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    #[test]
    fn a_superseded_commit_stack_is_cancelled_stopped_and_dropped() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let superseding = engine.handle(
            OperationId(3),
            &command(load_file(&"a".repeat(40), "b.txt")),
        );
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let mut jobs = engine.take_stack_jobs().into_iter();
        let stale = jobs.next().unwrap();
        assert!(stale.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.stack_report(stale.run()).is_empty());

        let current = jobs.next().unwrap();
        assert!(!current.stop.load(std::sync::atomic::Ordering::SeqCst));
        let events = engine.stack_report(current.run());
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitStackLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_new_commit_detail_ends_the_stack_read_in_flight() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let job = engine.take_stack_jobs().pop().unwrap();

        let events = engine.handle(OperationId(3), &command(load_detail(&"b".repeat(40))));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(job.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.stack_report(job.run()).is_empty());
    }

    #[test]
    fn a_commit_stack_writes_nothing_and_stages_nothing() {
        let (mut engine, git) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        run_stack_jobs(&mut engine);

        assert_eq!(git.writes(), Vec::<&str>::new());
        assert!(!engine.log().iter().any(|entry| matches!(
            entry.kind,
            CommandKind::Stage { .. } | CommandKind::Unstage { .. }
        )));
    }

    #[test]
    fn a_failed_commit_stack_fails_with_gits_message() {
        let (mut engine, git) = stack_engine();
        git.set_stack(Err(GitError("fatal: bad object".into())));
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let events = run_stack_jobs(&mut engine);
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: Some("fatal: bad object".into()),
        }));
    }
    #[test]
    fn a_superseded_commit_detail_is_cancelled_stopped_and_dropped() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        let superseding = engine.handle(OperationId(3), &command(load_detail(&"b".repeat(40))));
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let mut jobs = engine.take_detail_jobs().into_iter();
        let stale = jobs.next().unwrap();
        assert!(
            stale.stop.load(std::sync::atomic::Ordering::SeqCst),
            "the stale read must be stopped"
        );
        let stale_events = engine.detail_report(stale.run());
        assert!(
            stale_events.is_empty(),
            "the superseded answer must not show: {stale_events:?}"
        );

        let current = jobs.next().unwrap();
        assert!(!current.stop.load(std::sync::atomic::Ordering::SeqCst));
        let events = engine.detail_report(current.run());
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitDetailLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_failed_commit_detail_fails_with_gits_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_detail(Err(GitError("fatal: bad object".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));

        let events = run_detail_jobs(&mut engine);

        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: Some("fatal: bad object".into()),
        }));
    }

    #[test]
    fn leaving_the_repository_cancels_the_commit_detail() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        let job = engine.take_detail_jobs().pop().unwrap();
        let reopened = engine.handle(OperationId(3), &open());
        assert!(reopened.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(job.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.detail_report(job.run()).is_empty());
    }

    #[test]
    fn leaving_the_repository_cancels_the_history_load() {
        let mut engine = history_engine(&["c"]);
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(CommandKind::LoadHistory));
        let reopened = engine.handle(OperationId(3), &open());
        assert!(reopened.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(run_jobs(&mut engine).is_empty());
    }

    fn worktree(path: &str, branch: &str) -> gm_schema::WorktreeEntry {
        gm_schema::WorktreeEntry {
            path: path.into(),
            head: Some("abc123".into()),
            branch: Some(branch.into()),
            detached: false,
            bare: false,
            is_main: path == "/r",
            locked: None,
            prunable: None,
        }
    }

    #[test]
    fn list_worktrees_emits_the_worktrees_it_read() {
        let worktrees = vec![worktree("/r", "main"), worktree("/r-feature", "feature")];
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Ok(worktrees.clone()));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::ListWorktrees));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(events.contains(&Event::WorktreesLoaded {
            id: OperationId(2),
            worktrees,
        }));
    }

    #[test]
    fn refresh_follows_the_status_with_the_worktrees() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Ok(vec![worktree("/r", "main")]));
        let mut engine = Engine::new(Box::new(git));

        let opened = engine.handle(OperationId(1), &open());
        assert!(
            opened
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(1)))
        );

        let refreshed = engine.handle(OperationId(2), &command(CommandKind::RefreshStatus));
        assert_eq!(states(&refreshed).last(), Some(&OperationState::Done));
        assert!(
            refreshed
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { id, .. } if *id == OperationId(2)))
        );
        assert!(
            refreshed
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_failed_worktree_list_fails_the_refresh() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Err(GitError("worktree list broke".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::RefreshStatus));

        assert_eq!(states(&events).last(), Some(&OperationState::Failed));
    }

    #[test]
    fn checkout_reloads_status_so_the_ui_repaints() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let kind = CommandKind::Checkout {
            branch: "feature".into(),
        };

        let events = engine.handle(OperationId(2), &command(kind));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_refused_checkout_fails_with_the_git_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_checkout(Err(GitError("local changes would be overwritten".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let kind = CommandKind::Checkout {
            branch: "feature".into(),
        };
        let events = engine.handle(OperationId(2), &command(kind));

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("local changes would be overwritten")
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { .. }))
        );
    }

    #[test]
    fn opening_outside_the_repo_fails() {
        let bad = command(CommandKind::OpenRepo {
            path: "/elsewhere".into(),
        });
        let events = engine().handle(OperationId(1), &bad);
        assert_eq!(states(&events).last(), Some(&OperationState::Failed));
    }

    fn create(path: &str, mode: WorktreeMode) -> CommandKind {
        CommandKind::CreateWorktree {
            path: path.into(),
            mode,
        }
    }

    #[test]
    fn create_worktree_hands_the_path_and_mode_to_git_and_reloads_the_list() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        let mut engine = Engine::new(Box::new(git.clone()));
        engine.handle(OperationId(1), &open());
        let mode = WorktreeMode::New {
            branch: "feature".into(),
            base: "main".into(),
        };

        let events = engine.handle(OperationId(2), &command(create("/r-feature", mode.clone())));

        assert_eq!(git.created(), vec![("/r-feature".to_owned(), mode)]);
        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_refused_create_worktree_fails_with_the_git_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_create(Err(GitError(
            "'main' is already checked out at '/r'".into(),
        )));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(
            OperationId(2),
            &command(create(
                "/r-main",
                WorktreeMode::Existing {
                    branch: "main".into(),
                },
            )),
        );

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("'main' is already checked out at '/r'")
        );
    }

    #[test]
    fn an_agent_cannot_create_a_worktree() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: create(
                "/r-agent",
                WorktreeMode::Detached {
                    commit: "HEAD".into(),
                },
            ),
        };

        let events = engine.handle(OperationId(2), &command);

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("only a human can create a worktree")
        );
    }

    #[test]
    fn an_agent_cannot_approve_a_hook() {
        let mut engine = engine();
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::ApproveHook {
                name: "team".into(),
                command_hash: "0".repeat(64),
            },
        };

        let events = engine.handle(OperationId(1), &command);

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(detail.as_deref(), Some("only a human can approve a hook"));
    }

    #[test]
    fn an_agent_cannot_run_a_workflow_yet() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::RunWorkflow {
                name: "suggest".into(),
                inputs: BTreeMap::new(),
            },
        };

        let events = engine.handle(OperationId(2), &command);

        assert_eq!(
            failure(&events).as_deref(),
            Some("agents run workflows through MCP in a later slice")
        );
        assert!(engine.take_workflow_jobs().is_empty());
    }

    fn failure(events: &[Event]) -> Option<String> {
        events.iter().find_map(|e| match e {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
    }

    #[test]
    fn an_agent_cannot_create_or_delete_a_branch() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let create = Command {
            actor: agent.clone(),
            kind: CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: None,
                checkout: false,
            },
        };
        let events = engine.handle(OperationId(2), &create);
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can create a branch")
        );
        let delete = Command {
            actor: agent,
            kind: CommandKind::DeleteBranch {
                name: "topic".into(),
                force: true,
            },
        };
        let events = engine.handle(OperationId(3), &delete);
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can delete a branch")
        );
    }

    fn stash(index: u32, oid: &str, message: &str) -> gm_schema::StashEntry {
        gm_schema::StashEntry {
            index,
            oid: oid.into(),
            message: message.into(),
            branch: Some("main".into()),
            time: 0,
        }
    }

    /// An engine over a fake whose stash list holds one stash, `a` at index 0.
    fn stash_engine() -> (Engine, FakeGit) {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        fake.set_stashes(Ok(vec![stash(0, &"a".repeat(40), "On main: x")]));
        let mut engine = Engine::new(Box::new(fake.clone()));
        engine.handle(OperationId(1), &open());
        (engine, fake)
    }

    #[test]
    fn an_agent_cannot_change_a_stash_and_a_human_can() {
        let (mut engine, fake) = stash_engine();
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let oid = "a".repeat(40);
        let writes = [
            (
                CommandKind::CreateStash {
                    message: None,
                    include_untracked: true,
                },
                "only a human can create a stash",
            ),
            (
                CommandKind::ApplyStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can apply a stash",
            ),
            (
                CommandKind::PopStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can pop a stash",
            ),
            (
                CommandKind::DropStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can drop a stash",
            ),
        ];
        for (n, (kind, refused)) in writes.iter().enumerate() {
            let events = engine.handle(
                OperationId(2 + n as u64),
                &Command {
                    actor: agent.clone(),
                    kind: kind.clone(),
                },
            );
            assert_eq!(failure(&events).as_deref(), Some(*refused));
        }
        assert!(fake.writes().is_empty());

        let events = engine.handle(
            OperationId(10),
            &command(CommandKind::DropStash { index: 0, oid }),
        );
        assert_eq!(failure(&events), None);
        assert_eq!(fake.writes(), ["drop_stash"]);
        let listed = events.iter().find_map(|e| match e {
            Event::StashesListed { stashes, .. } => Some(stashes.len()),
            _ => None,
        });
        assert_eq!(listed, Some(1));
    }

    #[test]
    fn an_agent_may_list_the_stashes() {
        let (mut engine, fake) = stash_engine();
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: Actor::Agent {
                    name: "agent".into(),
                },
                kind: CommandKind::ListStashes,
            },
        );
        assert_eq!(failure(&events), None);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::StashesListed { stashes, .. } if stashes.len() == 1
        )));
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn a_stale_stash_is_refused_before_git_writes() {
        let (mut engine, fake) = stash_engine();
        // The list changed: `stash@{0}` is now another stash.
        fake.set_stashes(Ok(vec![stash(0, &"b".repeat(40), "On main: y")]));
        let stale = "a".repeat(40);
        for (n, kind) in [
            CommandKind::ApplyStash {
                index: 0,
                oid: stale.clone(),
            },
            CommandKind::PopStash {
                index: 0,
                oid: stale.clone(),
            },
            CommandKind::DropStash {
                index: 0,
                oid: stale.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let events = engine.handle(OperationId(2 + n as u64), &command(kind));
            assert_eq!(failure(&events).as_deref(), Some(STASH_LIST_CHANGED));
        }
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn stash_writes_are_logged_and_the_list_is_not() {
        let (mut engine, _fake) = stash_engine();
        engine.handle(OperationId(2), &command(CommandKind::ListStashes));
        assert!(
            engine
                .log()
                .iter()
                .all(|entry| entry.kind != CommandKind::ListStashes)
        );
        engine.handle(
            OperationId(3),
            &command(CommandKind::CreateStash {
                message: Some("note".into()),
                include_untracked: false,
            }),
        );
        assert!(engine.log().iter().any(|entry| {
            entry.id == OperationId(3) && matches!(entry.kind, CommandKind::CreateStash { .. })
        }));
    }

    #[test]
    fn an_agent_cannot_discard_a_file_and_a_human_can() {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        let mut engine = Engine::new(Box::new(fake.clone()));
        engine.handle(OperationId(1), &open());
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let discard = CommandKind::DiscardFile {
            path: "f.txt".into(),
            include_staged: true,
        };
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: agent,
                kind: discard.clone(),
            },
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can discard changes")
        );
        assert!(fake.writes().is_empty());
        let events = engine.handle(OperationId(3), &command(discard));
        assert_eq!(failure(&events), None);
        assert_eq!(fake.writes(), ["discard_file"]);
    }

    #[test]
    fn an_invalid_branch_name_is_refused_before_git_runs() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::CreateBranch {
                name: "a b".into(),
                start_point: None,
                checkout: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("A branch name cannot contain spaces.")
        );
        let events = engine.handle(
            OperationId(3),
            &command(CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: Some("--all".into()),
                checkout: true,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Choose a branch, tag or commit to start from.")
        );
        let events = engine.handle(
            OperationId(4),
            &command(CommandKind::DeleteBranch {
                name: "x..y".into(),
                force: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("A branch name cannot contain '..'.")
        );
    }

    #[test]
    fn the_checked_out_branch_is_refused_before_git_runs() {
        let status = RepoStatus {
            root: "/r".into(),
            branch: gm_schema::BranchInfo {
                head: Some("main".into()),
                ..gm_schema::BranchInfo::default()
            },
            ..RepoStatus::default()
        };
        let mut engine = Engine::new(Box::new(FakeGit::new("/r", status)));
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::DeleteBranch {
                name: "main".into(),
                force: true,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Cannot delete the checked-out branch main.")
        );
    }

    #[test]
    fn a_branch_checked_out_in_another_worktree_is_refused_with_its_path() {
        let status = RepoStatus {
            root: "/r".into(),
            branch: gm_schema::BranchInfo {
                head: Some("main".into()),
                ..gm_schema::BranchInfo::default()
            },
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        fake.set_worktrees(Ok(vec![
            worktree("/r", "main"),
            worktree("/r-feature", "feature"),
        ]));
        let mut engine = Engine::new(Box::new(fake));
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::DeleteBranch {
                name: "feature".into(),
                force: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Cannot delete feature: it is checked out in the worktree at /r-feature.")
        );
    }

    fn any_kind() -> impl Strategy<Value = CommandKind> {
        prop_oneof![
            Just(CommandKind::RefreshStatus),
            prop_oneof![Just("/r"), Just("/r/sub"), Just("/x")]
                .prop_map(|p| CommandKind::OpenRepo { path: p.into() }),
        ]
    }

    fn run(kinds: &[CommandKind]) -> (Vec<LogEntry>, Vec<Event>) {
        let mut engine = engine();
        let mut all = Vec::new();
        for (n, kind) in kinds.iter().enumerate() {
            all.extend(engine.handle(OperationId(n as u64 + 1), &command(kind.clone())));
        }
        (engine.log().to_vec(), all)
    }

    proptest! {
        #[test]
        fn same_commands_give_the_same_log(kinds in proptest::collection::vec(any_kind(), 0..12)) {
            prop_assert_eq!(run(&kinds), run(&kinds));
        }

        #[test]
        fn every_operation_ends_in_one_terminal_state(kinds in proptest::collection::vec(any_kind(), 1..12)) {
            let (log, _) = run(&kinds);
            for n in 1..=kinds.len() as u64 {
                let own: Vec<_> = log.iter().filter(|e| e.id == OperationId(n)).collect();
                for pair in own.windows(2) {
                    prop_assert!(pair[0].state.can_move_to(pair[1].state));
                }
                prop_assert!(own.last().unwrap().state.is_terminal());
                prop_assert_eq!(own.iter().filter(|e| e.state.is_terminal()).count(), 1);
            }
        }
    }
}
