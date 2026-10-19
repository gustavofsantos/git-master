//! The remote operation the window waits for (fetch, pull, push with lease):
//! from the moment its command is sent until the engine answers. The toolbar
//! shows it as busy, the rules refuse another one while it runs, and its answer
//! goes to the status bar. Pure state with no GPUI in it: the view feeds it the
//! events and reads it back.

use gm_schema::{OperationId, OperationState};

use crate::action_registry::Action;

/// How a remote operation ended, as the status bar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The success message the engine sent, such as "Fetched".
    Done(String),
    /// Git's own text, or a fallback when the engine sent none.
    Failed(String),
}

/// The remote operation in flight, if any, and the command that started it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteOp {
    in_flight: Option<(OperationId, Action)>,
}

impl RemoteOp {
    /// Records that `action` was sent as command `id`, which is now in flight.
    pub fn begin(&mut self, id: OperationId, action: Action) {
        self.in_flight = Some((id, action));
    }

    /// The action in flight, if any.
    #[must_use]
    pub fn in_flight(&self) -> Option<Action> {
        self.in_flight.map(|(_, action)| action)
    }

    /// Whether the operation `id` is the one in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.in_flight.is_some_and(|(owner, _)| owner == id)
    }

    /// Ends the operation when `id` is the one in flight and `state` is final,
    /// and says how it ended. The states before the end change nothing.
    pub fn settle(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<String>,
    ) -> Option<Outcome> {
        if !self.owns(id) || !state.is_terminal() {
            return None;
        }
        self.in_flight = None;
        Some(if state == OperationState::Done {
            Outcome::Done(detail.unwrap_or_default())
        } else {
            Outcome::Failed(detail.unwrap_or_else(|| "the operation failed".to_owned()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operation_is_in_flight_from_its_command_until_its_end() {
        let mut remote = RemoteOp::default();
        assert_eq!(remote.in_flight(), None);
        remote.begin(OperationId(4), Action::Pull);
        assert_eq!(remote.in_flight(), Some(Action::Pull));
        assert!(remote.owns(OperationId(4)));
        assert!(!remote.owns(OperationId(5)));

        assert_eq!(
            remote.settle(
                OperationId(4),
                OperationState::Done,
                Some("Pulled".to_owned())
            ),
            Some(Outcome::Done("Pulled".to_owned()))
        );
        assert_eq!(remote.in_flight(), None);
    }

    #[test]
    fn states_before_the_end_and_other_operations_change_nothing() {
        let mut remote = RemoteOp::default();
        remote.begin(OperationId(4), Action::Fetch);
        assert_eq!(
            remote.settle(OperationId(4), OperationState::Executing, None),
            None
        );
        assert_eq!(
            remote.settle(
                OperationId(9),
                OperationState::Done,
                Some("Fetched".to_owned())
            ),
            None
        );
        assert_eq!(remote.in_flight(), Some(Action::Fetch));
    }

    #[test]
    fn a_failure_keeps_gits_text_and_has_a_fallback_without_it() {
        let mut remote = RemoteOp::default();
        remote.begin(OperationId(4), Action::PushWithLease);
        assert_eq!(
            remote.settle(
                OperationId(4),
                OperationState::Failed,
                Some("! [rejected] (stale info)".to_owned())
            ),
            Some(Outcome::Failed("! [rejected] (stale info)".to_owned()))
        );
        remote.begin(OperationId(5), Action::Fetch);
        assert_eq!(
            remote.settle(OperationId(5), OperationState::Cancelled, None),
            Some(Outcome::Failed("the operation failed".to_owned()))
        );
    }
}
