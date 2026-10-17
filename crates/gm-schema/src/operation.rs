use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diff::{DiffSide, FileDiff, HunkId, StageTarget};
use crate::hook::{HookFailure, HookInfo};
use crate::status::{BranchEntry, RepoStatus, WorktreeEntry};

/// Sequential, so the operation log is deterministic.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct OperationId(pub u64);

/// Who asked. A human skips approval; an agent does not (once there is
/// anything to approve).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Actor {
    Human,
    Agent { name: String },
}

/// What a new worktree checks out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeMode {
    /// An existing local branch. Git refuses one that another worktree has.
    Existing { branch: String },
    /// A new branch `branch`, created at `base` (any commit-ish).
    New { branch: String, base: String },
    /// A detached HEAD at `commit` (any commit-ish); no branch is created.
    Detached { commit: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandKind {
    OpenRepo {
        path: String,
    },
    RefreshStatus,
    LoadDiff {
        path: String,
        side: DiffSide,
    },
    Stage {
        target: StageTarget,
    },
    Unstage {
        target: StageTarget,
    },
    /// Reverts one unstaged hunk in the working tree. Destructive: the
    /// hunk's changes are lost, and the id must name a current unstaged hunk.
    DiscardHunk {
        path: String,
        hunk: HunkId,
    },
    Commit {
        message: String,
    },
    ListBranches,
    /// A local branch, or a remote branch such as `origin/feature` (a local
    /// tracking branch is created for it when none exists).
    Checkout {
        branch: String,
    },
    /// Lists the worktrees of the repository, the main checkout first. Never
    /// records anything.
    ListWorktrees,
    /// Creates a linked worktree at `path`, which must not exist yet. Human
    /// only, for now (D31: no agent path for worktrees).
    CreateWorktree {
        path: String,
        mode: WorktreeMode,
    },
    /// Lists the hooks read from config, with their state (docs/03).
    ListHooks,
    /// Approves the repository hook `name` whose command still hashes to
    /// `command_hash`, as the last `HooksListed` showed it.
    ApproveHook {
        name: String,
        command_hash: String,
    },
    /// Runs the `prepare-commit` chain on `message`. Asynchronous: the hook
    /// events for the run carry the run's own operation id.
    RunPrepareCommit {
        message: String,
    },
    /// Stops the `RunPrepareCommit` operation `run`, if it is still running.
    CancelHook {
        run: OperationId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Command {
    pub actor: Actor,
    pub kind: CommandKind,
}

/// `Requested → Gating → AwaitingApproval → Executing → Done | Failed |
/// Vetoed | Cancelled` (docs/06, rule 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Requested,
    Gating,
    AwaitingApproval,
    Executing,
    Done,
    Failed,
    Vetoed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: OperationState,
    pub to: OperationState,
}

impl OperationState {
    pub const ALL: [Self; 8] = [
        Self::Requested,
        Self::Gating,
        Self::AwaitingApproval,
        Self::Executing,
        Self::Done,
        Self::Failed,
        Self::Vetoed,
        Self::Cancelled,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Vetoed | Self::Cancelled
        )
    }

    pub fn can_move_to(self, to: Self) -> bool {
        use OperationState::{
            AwaitingApproval, Cancelled, Done, Executing, Failed, Gating, Requested, Vetoed,
        };
        matches!(
            (self, to),
            (Requested, Gating | Cancelled)
                | (
                    Gating,
                    AwaitingApproval | Executing | Vetoed | Failed | Cancelled
                )
                | (AwaitingApproval, Executing | Vetoed | Cancelled)
                | (Executing, Done | Failed | Cancelled)
        )
    }

    pub fn move_to(self, to: Self) -> Result<Self, IllegalTransition> {
        if self.can_move_to(to) {
            Ok(to)
        } else {
            Err(IllegalTransition { from: self, to })
        }
    }
}

/// What front-ends subscribe to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    OperationChanged {
        id: OperationId,
        actor: Actor,
        state: OperationState,
        /// Failure reason, when `state` is `failed`.
        detail: Option<String>,
    },
    StatusLoaded {
        id: OperationId,
        status: RepoStatus,
    },
    DiffLoaded {
        id: OperationId,
        side: DiffSide,
        diff: FileDiff,
    },
    Committed {
        id: OperationId,
        oid: String,
    },
    BranchesLoaded {
        id: OperationId,
        branches: Vec<BranchEntry>,
    },
    /// Answers `ListWorktrees`, and follows each `RefreshStatus` and
    /// `OpenRepo`, so the worktree list is as fresh as the status.
    WorktreesLoaded {
        id: OperationId,
        worktrees: Vec<WorktreeEntry>,
    },
    HooksListed {
        id: OperationId,
        hooks: Vec<HookInfo>,
    },
    /// A chain step is about to run; `step` counts from 1 of `of`.
    HookStarted {
        id: OperationId,
        hook: String,
        step: u32,
        of: u32,
    },
    /// The whole chain answered; `message` is the last hook's output.
    /// `repo_changed` is true when HEAD or the staged diff differs from the
    /// fingerprint taken when the run started: the message is then a
    /// suggestion for the new state, not for the state the user saw.
    HookFinished {
        id: OperationId,
        message: String,
        repo_changed: bool,
    },
    HookFailed {
        id: OperationId,
        hook: String,
        failure: HookFailure,
    },
    HookCancelled {
        id: OperationId,
        hook: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn checkout_command_wire_format_is_tagged() {
        let command = CommandKind::Checkout {
            branch: "origin/feature".into(),
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["type"], "checkout");
        assert_eq!(json["branch"], "origin/feature");
        let list = serde_json::to_value(CommandKind::ListBranches).unwrap();
        assert_eq!(list["type"], "list_branches");
    }

    #[test]
    fn branches_loaded_wire_format_is_tagged() {
        let event = Event::BranchesLoaded {
            id: OperationId(2),
            branches: vec![BranchEntry {
                name: "main".into(),
                is_remote: false,
                is_current: true,
                upstream: Some("origin/main".into()),
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "branches_loaded");
        assert_eq!(json["branches"][0]["is_current"], true);
        assert_eq!(json["branches"][0]["upstream"], "origin/main");
    }

    fn any_state() -> impl Strategy<Value = OperationState> {
        proptest::sample::select(OperationState::ALL.to_vec())
    }

    proptest! {
        #[test]
        fn terminal_states_never_move(from in any_state(), to in any_state()) {
            if from.is_terminal() {
                prop_assert!(!from.can_move_to(to));
            }
        }

        #[test]
        fn nothing_moves_back_to_requested(from in any_state()) {
            prop_assert!(!from.can_move_to(OperationState::Requested));
        }
    }

    #[test]
    fn approval_cannot_be_skipped_into_done() {
        assert!(!OperationState::Gating.can_move_to(OperationState::Done));
        assert!(!OperationState::AwaitingApproval.can_move_to(OperationState::Done));
    }

    #[test]
    fn event_wire_format_is_tagged() {
        let event = Event::OperationChanged {
            id: OperationId(1),
            actor: Actor::Agent {
                name: "claude".into(),
            },
            state: OperationState::AwaitingApproval,
            detail: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "operation_changed");
        assert_eq!(json["actor"]["type"], "agent");
        assert_eq!(json["state"], "awaiting_approval");
    }
}
