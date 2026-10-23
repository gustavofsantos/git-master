//! Workflow scripts (design/workflow-scripts.md) as the front-ends see them: the files the engine
//! found, how a run moves, and what a run's journal holds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One workflow file the engine found, with what loading it gave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInfo {
    /// The workflow's `name`, or the file stem when the file does not load.
    pub name: String,
    /// Absolute path of the file.
    pub path: String,
    /// The trigger's `on`, such as `manual`; empty when the file does not load.
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

/// Where a run is. `Running` while the script works; the others wait on something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Running,
    /// Waiting for an event or a timeout (`ctx.wait`).
    Waiting,
    /// Paused at an approval (`ctx.approve`, or an `apply` that asks) until the human answers.
    AwaitingApproval,
    Done,
    Failed,
    /// `ctx.block` ended the run.
    Blocked,
    Cancelled,
    /// The app stopped while the run was in progress; the next open marks it (D100).
    Interrupted,
}

/// How a run ended, when it ended by its own script.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEnd {
    Done,
    Failed,
    Blocked,
    Cancelled,
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
    /// The state the run is in, while it has not ended. Always `None` for a script run, which
    /// has no states.
    pub current_state: Option<String>,
}

/// One `ctx` call of a run, as the host recorded it: observed by the host, not reported by the
/// script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowCallRecord {
    /// 1, 2, 3… in the order the script made the calls.
    pub seq: u32,
    /// The name without `ctx.`, such as `branch.create`.
    pub func: String,
    /// `read`, `propose`, `apply`, `pause` or `helper`.
    pub level: String,
    /// The arguments as the host received them, after the catalog's checks.
    pub args: serde_json::Value,
    /// What the call answered; `None` until it ends, and for a call a restart found unfinished.
    pub result: Option<serde_json::Value>,
    /// `done`, `failed`, `conflict`, … or `interrupted`; `None` until the call ends.
    pub outcome: Option<String>,
    /// The reason of an outcome other than `done`, when there is one.
    pub message: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub started_at: i64,
    /// How long the call took, once it ended.
    pub duration_ms: Option<i64>,
}

/// Everything about one run, for the run view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunDetail {
    pub summary: WorkflowRunSummary,
    /// The script text as the run saw it, even if the file changed after.
    pub definition: String,
    /// The `trigger` value the run got.
    pub trigger: serde_json::Value,
    /// The `input` value the run got: `inputs`, `trigger` and `repo`. A value over 64 KiB is
    /// kept as its size, its sha256 and its first 4 KiB, as the calls are.
    pub input: serde_json::Value,
    /// Every `ctx` call in the order it was made.
    pub calls: Vec<WorkflowCallRecord>,
}

/// A dry run of a script's text: the calls it would make, read without performing any of them.
/// Nothing is written and no run is recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDryRun {
    /// Every problem of the text, one per line as `line N: message`, when it does not load. Then
    /// `calls` is empty and `stop` is empty.
    pub error: Option<String>,
    /// Declared inputs the request did not give. Each is `""` in the walk.
    pub missing_inputs: Vec<String>,
    /// Every `ctx` call the walk made, in order.
    pub calls: Vec<WorkflowDryCall>,
    /// Why the walk stopped, in words: `returned: <value>`, `ended: blocked: <reason>`,
    /// `ended: failed: <reason>` or `ended: cancelled`. Empty when `error` is set.
    pub stop: String,
}

/// One problem of a script's text, as the editor shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDiagnostic {
    /// 1-based; 0 when the problem has no single line.
    pub line: u32,
    pub message: String,
}

/// One `ctx` call of a dry run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDryCall {
    /// The name without `ctx.`, such as `exec`.
    pub func: String,
    /// `read`, `propose`, `apply`, `pause` or `helper`; empty for a call the catalog does not know.
    pub level: String,
    /// The arguments the call was made with, after the catalog's checks.
    pub args: serde_json::Value,
    /// The answer was computed as the host computes it (`now`, `random`, `notify`); nothing ran
    /// in any case.
    pub executed: bool,
    /// The answer was simulated, because the call would reach the engine; nothing ran.
    pub assumed: bool,
    /// `done` for a simulated or computed answer, `rejected` when the arguments are wrong,
    /// `blocked` or `failed` for a call that ends the run.
    pub outcome: String,
    /// What the dry run says about the call, in words, such as `simulated: nothing is applied`.
    pub note: String,
}
