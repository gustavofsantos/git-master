use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diff::{DiffSide, FileDiff, StageTarget};
use crate::status::RepoStatus;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandKind {
    OpenRepo { path: String },
    RefreshStatus,
    LoadDiff { path: String, side: DiffSide },
    Stage { target: StageTarget },
    Unstage { target: StageTarget },
    Commit { message: String },
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
