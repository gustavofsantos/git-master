//! Manual workflows (docs/10) as the front-ends see them: the files the engine
//! found, and how a run moves from one state to the next.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One workflow file the engine found, with what loading it gave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInfo {
    /// The workflow's `name`, or the file stem when the file does not parse.
    pub name: String,
    /// Absolute path of the file.
    pub path: String,
    /// The trigger's `on`, such as `manual`; empty when the file does not parse.
    pub trigger: String,
    /// The key binding of a manual workflow, when it has one.
    pub key: Option<String>,
    /// The declared inputs, in file order.
    pub inputs: Vec<WorkflowInput>,
    /// sha256 of the file, lowercase hex.
    pub hash: String,
    /// The first error, as `line N: message`; `None` when the workflow loads.
    pub error: Option<String>,
    /// Every warning of a loaded workflow, as `line N: message`.
    pub warnings: Vec<String>,
    /// The file as read, for the editor; empty when it could not be read.
    pub source: String,
}

/// One declared input of a workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInput {
    pub name: String,
    /// The question the composer asks for this input.
    pub prompt: Option<String>,
}

/// Where a run is. `Running` while a state works; the others wait on something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Running,
    /// Waiting for a condition the engine checks again later.
    Waiting,
    /// Paused at an `approve` state until the human answers.
    AwaitingApproval,
    Done,
    Failed,
    /// A `before.*` run vetoed the operation it guards.
    Blocked,
    Cancelled,
}

/// How a run ended, when it ended by its own workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEnd {
    Done,
    Failed,
    Blocked,
}

/// One run as the history lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunSummary {
    pub run: i64,
    pub workflow: String,
    pub status: WorkflowRunStatus,
    /// `human`, `event:<name>` or `agent:<client>`.
    pub started_by: String,
    /// Milliseconds since the Unix epoch.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
    /// The state the run is in, while it has not ended.
    pub current_state: Option<String>,
}

/// One visit of a state, as the run log recorded it: observed by the engine,
/// not reported by the step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStepRecord {
    /// Path of the state (`each/plan` inside a `for_each`).
    pub state: String,
    /// 1 for the first visit of the state in the run, then 2, 3…
    pub visit: u32,
    /// The action the state ran; `None` for a wait or a `for_each`.
    pub action: Option<String>,
    /// What the step received, with every `${...}` already rendered. `None`
    /// for runs recorded before inputs were kept.
    pub input: Option<serde_json::Value>,
    /// What the step answered; `None` while it has not answered.
    pub output: Option<serde_json::Value>,
    /// The output was too large and only its head, size and hash are kept.
    pub truncated: bool,
    /// The outcome it ended with, and the state or `end.*` the run went to.
    pub outcome: Option<String>,
    pub next: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// Everything about one run, for the run view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunDetail {
    pub summary: WorkflowRunSummary,
    /// The workflow file as it was when the run started.
    pub definition: String,
    /// `inputs`, `trigger` and `repo` as the run saw them; a large
    /// `repo.staged_diff` is cut to its head.
    pub inputs: serde_json::Value,
    pub trigger: serde_json::Value,
    pub repo: serde_json::Value,
    /// Every step in the order it ran.
    pub steps: Vec<WorkflowStepRecord>,
}

/// A dry run of a workflow's text: the steps it would take, read without
/// performing any of them. Nothing is written and no run is recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDryRun {
    /// Every error of the text, one per line as `line N: error: message`, when
    /// it does not load. Then `steps` and `stop` are empty.
    pub error: Option<String>,
    /// Declared inputs the request did not give. Each is `""` in the walk.
    pub missing_inputs: Vec<String>,
    /// Every state the walk visited, in order. Steps that are not run are
    /// marked `assumed`.
    pub steps: Vec<WorkflowDryStep>,
    /// Why the walk stopped, in words: `end.done`, `end.failed: <reason>`,
    /// `would wait for <event>`, `` `<action>` is not available yet``, a
    /// machine error, or `stopped after 200 steps`. Empty when `error` is set.
    pub stop: String,
}

/// One state of a dry run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDryStep {
    /// Path of the state (`each/plan` inside a `for_each`).
    pub state: String,
    pub action: String,
    /// `read`, `propose`, `apply` or `approve`: the action's effect level.
    pub level: String,
    /// What the step would receive, with every `${...}` rendered. For `exec`,
    /// this is the argv and stdin it would send.
    pub input: serde_json::Value,
    /// The answer was computed as the driver computes it (`noop`, `set`);
    /// nothing ran in any case.
    pub executed: bool,
    /// The outcome and output were assumed, because the step was not run.
    pub assumed: bool,
    pub outcome: String,
    pub output: serde_json::Value,
}
