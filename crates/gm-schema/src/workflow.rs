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
