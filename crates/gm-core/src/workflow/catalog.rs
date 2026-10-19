//! The actions, wait events and triggers a workflow may name, with each
//! action's effect level and declared outcomes (docs/10, D63, D78). Static
//! in WF0; JSON Schema of inputs and outputs comes with WF1.

/// Effect level of an action (D20, D64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Read,
    Propose,
    Apply,
    /// `approve`: pauses for the human.
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub name: &'static str,
    pub level: Level,
    /// Every outcome the action can end with, including `done` and `failed`
    /// (and `interrupted` for apply actions, D80).
    pub outcomes: &'static [&'static str],
    /// The trigger phase the action is limited to, such as `transform` for
    /// `set`; `None` when it is valid everywhere.
    pub phase: Option<&'static str>,
}

/// Outcomes with a default transition when the file writes none (D78):
/// `done` goes to the next state; `failed` and `interrupted` to `end.failed`.
pub const DEFAULTED: &[&str] = &["done", "failed", "interrupted"];

/// The action with this name.
pub fn action(_name: &str) -> Option<&'static ActionSpec> {
    todo!("WF0: catalog::action")
}

/// Outcomes of a wait state on this event, without `timeout` (which exists
/// only when the state sets one).
pub fn wait_outcomes(_event: &str) -> Option<&'static [&'static str]> {
    todo!("WF0: catalog::wait_outcomes")
}

/// Whether a workflow may use this as `trigger.on`.
pub fn is_known_trigger(_on: &str) -> bool {
    todo!("WF0: catalog::is_known_trigger")
}
