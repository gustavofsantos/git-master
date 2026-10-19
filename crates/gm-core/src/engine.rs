use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gm_schema::{
    Actor, Blocker, Command, CommandKind, CommitFile, Event, History, HookFailure, HookScope,
    OperationId, OperationState, WorktreeEntry, WorktreeInspection,
};

use crate::GitPort;
use crate::commit_detail::{DetailJob, DetailReport};
use crate::commit_diff::{StackJob, StackReport};
use crate::history::{Canceller, HistoryJob, HistoryOutcome, HistoryReport, HistoryStream};
use crate::hook::{self, ApprovalStore, Fingerprint, HookOutcome, HookRecord, Plan, Report, Step};
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
        }
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
        if matches!(
            command.kind,
            CommandKind::LoadHistory | CommandKind::LoadHistoryPage { .. }
        ) {
            let after = match &command.kind {
                CommandKind::LoadHistoryPage { after } => Some(after.clone()),
                _ => None,
            };
            match self.start_history(id, &command.actor, &command.kind, after) {
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
        if let CommandKind::LoadCommitStack { oid, files } = &command.kind {
            match self.start_stack(id, &command.actor, &command.kind, oid, files) {
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
        if let CommandKind::LoadCommitDetail { oid } = &command.kind {
            match self.start_detail(id, &command.actor, &command.kind, oid) {
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
            CommandKind::LoadHistory | CommandKind::LoadHistoryPage { .. } => {
                Err("history is started by handle".to_owned())
            }
            CommandKind::LoadCommitDetail { .. } | CommandKind::LoadCommitStack { .. } => {
                Err("commit reads are started by handle".to_owned())
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
                // Approval is the human's decision (docs/00); an agent may only propose.
                if !matches!(actor, Actor::Human) {
                    return Err("only a human can approve a hook".to_owned());
                }
                let root = self.open_root()?;
                let (branch, _) = self.current(&root)?;
                let hooks = self.list_hooks(&root, branch.as_deref())?;
                let hook = hooks
                    .iter()
                    .find(|h| h.scope == HookScope::Repo && &h.name == name)
                    .ok_or_else(|| format!("no repository hook named {name}"))?;
                if &hook.command_hash != command_hash {
                    return Err(format!(
                        "the command of {name} changed since the hooks were listed; list them again"
                    ));
                }
                self.approvals
                    .approve(&root.to_string_lossy(), name, command_hash)?;
                let hooks = self.list_hooks(&root, branch.as_deref())?;
                Ok(vec![Event::HooksListed { id, hooks }])
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
            // The worktree commands that `execute` does not list above.
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

    fn list_worktrees(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let worktrees = self.git.worktrees(&root).map_err(|e| e.0)?;
        Ok(vec![Event::WorktreesLoaded { id, worktrees }])
    }

    /// The branch list, with the configured user email so the UI can tell
    /// which branches are the user's own.
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
    /// and the prune pair.
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
            _ => Err("not a worktree management command".to_owned()),
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
        self.root = Some(root);
        Ok(events)
    }

    fn open_root(&self) -> Result<PathBuf, String> {
        self.root
            .clone()
            .ok_or_else(|| "no repository is open".to_owned())
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
        self.log.push(LogEntry {
            id: op.id,
            actor: op.command.actor.clone(),
            kind: op.command.kind.clone(),
            state: op.state,
        });
        events.push(Event::OperationChanged {
            id: op.id,
            actor: op.command.actor.clone(),
            state: op.state,
            detail,
        });
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
