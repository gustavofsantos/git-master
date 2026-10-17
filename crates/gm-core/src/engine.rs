use std::path::PathBuf;

use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

use crate::GitPort;

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
}

struct Op<'a> {
    id: OperationId,
    command: &'a Command,
    state: OperationState,
}

impl Engine {
    pub fn new(git: Box<dyn GitPort>) -> Self {
        Self {
            git,
            root: None,
            log: Vec::new(),
        }
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// Runs one command to a terminal state and returns the events, in order.
    pub fn handle(&mut self, id: OperationId, command: &Command) -> Vec<Event> {
        let mut events = Vec::new();
        let mut op = Op {
            id,
            command,
            state: OperationState::Requested,
        };
        self.record(&op, None, &mut events);
        // Nothing gates or needs approval yet; the states are walked anyway so
        // hooks (slice 2) and agent approval (slice 7) slot in without a new path.
        self.advance(&mut op, OperationState::Gating, &mut events);
        self.advance(&mut op, OperationState::Executing, &mut events);
        match self.execute(id, &command.kind) {
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

    fn execute(&mut self, id: OperationId, kind: &CommandKind) -> Result<Vec<Event>, String> {
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
        }
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
