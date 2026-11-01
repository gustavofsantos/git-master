//! The operation the window waits on for its workflow gates (`WF3c`, D157). No GPUI in here: the
//! model follows the engine's `GateStarted` and the operation's state, and each surface asks
//! whether the operation it shows is the one that waits.

use gm_schema::{OperationId, OperationState};

/// What a surface shows while its operation waits for the gates.
pub(crate) const WAITING: &str = "Waiting for workflow gates…";

/// The link that cancels the waiting operation, shown beside `WAITING`.
pub(crate) const CANCEL: &str = "Cancel gates";

/// The gated operation that waits now, and the gate that runs for it. One at a time (D157).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GateWait {
    waiting: Option<(OperationId, String)>,
}

impl GateWait {
    /// A gate of `id` started: the operation waits, behind the gate `workflow`.
    pub(crate) fn started(&mut self, id: OperationId, workflow: &str) {
        self.waiting = Some((id, workflow.to_owned()));
    }

    /// The operation `id` changed to `state`. Any state but `Gating` ends its wait: its gates
    /// passed, or it was vetoed, failed or cancelled.
    pub(crate) fn changed(&mut self, id: OperationId, state: OperationState) {
        if state != OperationState::Gating && self.waits(id) {
            self.waiting = None;
        }
    }

    /// Whether `id` waits for its gates now.
    pub(crate) fn waits(&self, id: OperationId) -> bool {
        self.waiting
            .as_ref()
            .is_some_and(|(waiting, _)| *waiting == id)
    }

    /// The operation that waits, if any.
    pub(crate) fn current(&self) -> Option<OperationId> {
        self.waiting.as_ref().map(|(id, _)| *id)
    }

    /// The operation Cancel stops: the one that waits now. `None` once it moved on, so no surface
    /// offers Cancel for an operation that cannot be stopped.
    pub(crate) fn cancel_target(&self) -> Option<OperationId> {
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_makes_its_operation_wait_until_it_moves_on() {
        let mut wait = GateWait::default();
        assert_eq!(wait.current(), None);
        wait.started(OperationId(4), "freeze");
        assert!(wait.waits(OperationId(4)));
        assert!(!wait.waits(OperationId(5)));
        wait.changed(OperationId(4), OperationState::Executing);
        assert_eq!(wait.current(), None);
    }

    #[test]
    fn a_veto_or_a_cancel_ends_the_wait_too() {
        for state in [
            OperationState::Vetoed,
            OperationState::Cancelled,
            OperationState::Failed,
        ] {
            let mut wait = GateWait::default();
            wait.started(OperationId(4), "freeze");
            wait.changed(OperationId(4), state);
            assert!(!wait.waits(OperationId(4)), "{state:?}");
        }
    }

    #[test]
    fn cancel_is_offered_only_while_the_operation_waits() {
        let mut wait = GateWait::default();
        assert_eq!(wait.cancel_target(), None);
        wait.started(OperationId(4), "freeze");
        assert_eq!(wait.cancel_target(), Some(OperationId(4)));
        wait.changed(OperationId(4), OperationState::Cancelled);
        assert_eq!(wait.cancel_target(), None);
    }

    #[test]
    fn the_next_gate_and_other_operations_do_not_end_the_wait() {
        let mut wait = GateWait::default();
        wait.started(OperationId(4), "first");
        wait.changed(OperationId(4), OperationState::Gating);
        wait.changed(OperationId(9), OperationState::Done);
        assert!(wait.waits(OperationId(4)));
        wait.started(OperationId(4), "second");
        assert!(wait.waits(OperationId(4)));
    }
}
