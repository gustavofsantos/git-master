use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gm_schema::{
    Actor, Command, CommandKind, Event, HookFailure, HookScope, OperationId, OperationState,
};

use crate::GitPort;
use crate::hook::{self, ApprovalStore, Fingerprint, HookOutcome, HookRecord, Plan, Report, Step};

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
            CommandKind::OpenRepo { path } => {
                let root = self.git.resolve_root(path.as_ref()).map_err(|e| e.0)?;
                let status = self.git.status(&root).map_err(|e| e.0)?;
                self.root = Some(root);
                Ok(vec![Event::StatusLoaded { id, status }])
            }
            CommandKind::RefreshStatus => {
                let root = self.open_root()?;
                let status = self.git.status(&root).map_err(|e| e.0)?;
                Ok(vec![Event::StatusLoaded { id, status }])
            }
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
            CommandKind::ListBranches => {
                let root = self.open_root()?;
                let branches = self.git.branches(&root).map_err(|e| e.0)?;
                Ok(vec![Event::BranchesLoaded { id, branches }])
            }
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
            cancel: Arc::clone(&cancel),
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

    fn open_root(&self) -> Result<PathBuf, String> {
        self.root
            .clone()
            .ok_or_else(|| "no repository is open".to_owned())
    }

    /// A mutation ends by showing the new state, so no front-end has to ask.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitError;
    use crate::fake::FakeGit;
    use gm_schema::RepoStatus;
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
        }));
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

    #[test]
    fn an_agent_cannot_approve_a_hook() {
        let mut engine = engine();
        let command = Command {
            actor: Actor::Agent {
                name: "claude".into(),
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
