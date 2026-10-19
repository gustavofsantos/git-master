//! The pure transition function of a workflow run (docs/10, D78, D80):
//! given the definition, the run so far and the outcome of the current
//! state, it returns the next run, the transition to record and the effect
//! the engine must carry out. No I/O; the engine owns effects and storage.

use std::collections::BTreeMap;

use serde_json::Value;

use super::catalog::Level;
use super::model::{Approval, Definition, End};

/// Hard cap of transitions per run (D78).
pub const MAX_TRANSITIONS: u32 = 1000;

/// What the run reads besides state outputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    pub inputs: Value,
    pub trigger: Value,
    pub repo: Value,
}

/// A run in progress or ended. Plain data, so the engine can store it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub position: Position,
    /// Latest output of each top-level state, by id.
    pub outputs: BTreeMap<String, Value>,
    /// Times each guarded or limited edge was taken.
    pub edge_uses: BTreeMap<EdgeKey, u32>,
    pub transitions: u32,
    /// Output of the latest `set`, returned by a `transform.*` run.
    pub last_set: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Position {
    /// Running or waiting in a top-level state; `inner` while inside a
    /// `for_each`.
    At {
        state: String,
        inner: Option<Inner>,
    },
    Ended {
        end: End,
        reason: Option<String>,
    },
}

/// Where a `for_each` stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Inner {
    /// Index of the current item.
    pub index: usize,
    /// The items, rendered once when the `for_each` was entered.
    pub items: Vec<Value>,
    /// Current state of the nested machine.
    pub state: String,
    /// Outputs of the nested states for the current item.
    pub outputs: BTreeMap<String, Value>,
}

/// One edge: the state (`each/plan` for a nested one), the outcome, and the
/// index of the transition in that outcome's list.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeKey {
    pub state: String,
    pub outcome: String,
    pub index: usize,
}

/// What the engine must do next.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Dispatch the action of `state` with the rendered input.
    Run {
        state: String,
        action: String,
        input: Value,
        level: Level,
        approval: Approval,
    },
    /// Park the run until `event` (or the timeout).
    Wait {
        state: String,
        event: String,
        timeout: Option<std::time::Duration>,
    },
    /// The run ended. `value` is the last `set` of a transform run.
    Finish {
        end: End,
        reason: Option<String>,
        value: Option<Value>,
    },
}

/// The transition to append to the run's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub from: String,
    pub outcome: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub run: Run,
    pub record: Record,
    pub effect: Effect,
}

/// Misuse of the function, not a failure of the run: a failing expression
/// ends the run in `end.failed` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineError(pub String);

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MachineError {}

/// Enters the start state.
pub fn start(_definition: &Definition, _context: &Context) -> Result<(Run, Effect), MachineError> {
    todo!("WF0: machine::start")
}

/// Applies the outcome of the current state and enters the next one.
pub fn next(
    _definition: &Definition,
    _context: &Context,
    _run: &Run,
    _outcome: &str,
    _output: Value,
) -> Result<Step, MachineError> {
    todo!("WF0: machine::next")
}
