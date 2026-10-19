//! The typed model of one workflow file: a bounded state machine (docs/10,
//! D78). Plain data, built by `parse` and read by `validate` and `machine`.

use std::time::Duration;

use serde_json::Value;

/// One workflow file. Holds only the definition (D79): nothing about runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub schema: u32,
    pub name: String,
    pub description: Option<String>,
    pub concurrency: Concurrency,
    pub trigger: Trigger,
    /// Declared inputs, in file order.
    pub inputs: Vec<Input>,
    /// The state a run starts in; the first state when the file sets none.
    pub start: String,
    pub states: Vec<State>,
}

impl Definition {
    /// The top-level state with this id.
    #[must_use]
    pub fn state(&self, id: &str) -> Option<&State> {
        self.states.iter().find(|s| s.id == id)
    }
}

/// What a new trigger does while a run of the same workflow is active (D65).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Concurrency {
    /// At most one pending run; triggers coalesce.
    #[default]
    Queue,
    /// Cancel the active run if it has not reached an apply state yet.
    Replace,
    /// Drop the trigger.
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// `manual`, an engine event such as `commit.created`, or a phase such as
    /// `before.restack` or `transform.commit_message` (D67, D75).
    pub on: String,
    /// The raw `match` value, such as `branch:feat/*`.
    pub matcher: Option<String>,
    /// Optional key binding of a manual workflow, such as `cmd-shift-m`.
    pub key: Option<String>,
}

impl Trigger {
    /// A `before.*` phase: the run can veto by reaching `end.blocked`.
    #[must_use]
    pub fn is_before(&self) -> bool {
        self.on.starts_with("before.")
    }

    /// A `transform.*` phase: the run returns a value through `set`.
    #[must_use]
    pub fn is_transform(&self) -> bool {
        self.on.starts_with("transform.")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    /// Only `string` in WF0.
    pub kind: String,
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub id: String,
    /// 1-based line of the state's `id` in the file, for diagnostics.
    pub line: usize,
    pub kind: StateKind,
    /// Transitions by outcome, in file order. An outcome missing here takes
    /// the short form (D78): `done` goes to the next state, `failed` and
    /// `interrupted` go to `end.failed`.
    pub on: Vec<OutcomeRule>,
}

impl State {
    /// The rule for this outcome, if the file wrote one.
    #[must_use]
    pub fn rule(&self, outcome: &str) -> Option<&OutcomeRule> {
        self.on.iter().find(|r| r.outcome == outcome)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StateKind {
    /// Runs one catalog action.
    Action {
        action: String,
        /// The `with` table, strings still holding their `${...}` templates.
        with: Value,
        approval: Approval,
    },
    /// Pauses until an engine event (D78).
    Wait {
        event: String,
        timeout: Option<Duration>,
    },
    /// Runs `states` once per item of `items`, in order. Depth 1 only.
    ForEach {
        /// The template that yields the list, such as `${states.roots.output.roots}`.
        items: String,
        /// The name the item takes in expressions.
        item: String,
        states: Vec<State>,
    },
}

/// Who approves an apply state of an event-triggered run (D64).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Approval {
    #[default]
    Ask,
    Auto,
}

/// `on.<outcome>`: the transitions tried in order for that outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeRule {
    pub outcome: String,
    pub transitions: Vec<Transition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub to: Target,
    /// The `if` condition source, such as `${states.plan.output.branch_count} == 0`.
    pub guard: Option<String>,
    /// How many times per run this edge may be taken.
    pub limit: Option<u32>,
    /// Why the run was blocked; only on `end.blocked`.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    State(String),
    End(End),
}

impl Target {
    /// Reads `end.done`, `end.failed`, `end.blocked` or a state id.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "end.done" => Self::End(End::Done),
            "end.failed" => Self::End(End::Failed),
            "end.blocked" => Self::End(End::Blocked),
            id => Self::State(id.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Done,
    Failed,
    /// Only in a `before.*` phase: the operation is vetoed.
    Blocked,
}
