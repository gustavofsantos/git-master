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
    /// The file as read, for the Script tab; empty when it could not be read.
    pub source: String,
    /// Whether the person reviewed this version of the text (D101).
    #[serde(default)]
    pub reviewed: bool,
    /// Whether event runs of this reviewed version apply without asking (the grant of D101).
    #[serde(default)]
    pub auto_approve: bool,
    /// Whether the workflow runs in this repository: a user workflow is on unless it was switched
    /// off here, a built-in one is off unless it was switched on here (`WF4b`, D164(1)).
    #[serde(default)]
    pub enabled: bool,
    /// The `description` of the workflow's `M.meta`: one line of what it does. `None` when it has
    /// none or the file does not load. The Run review picker shows it (D219 (1)).
    #[serde(default)]
    pub description: Option<String>,
    /// The display title of a built-in workflow, such as "Restack children when parent moves"
    /// (`WF4b`, D164(2)). `None` for a workflow the person wrote: only built-ins carry one.
    #[serde(default)]
    pub title: Option<String>,
    /// Where the file is read from (D219 (2)).
    #[serde(default)]
    pub location: WorkflowLocation,
    /// Whether the workflow is a review, from `review = true` in `M.meta` (D219 (1)). A file that
    /// does not load is a workflow.
    #[serde(default)]
    pub role: WorkflowRole,
}

impl WorkflowInfo {
    /// Whether the workflow ships with the app (`WF4b`, D164(2)): it has a title and no file. Its
    /// `source` is read-only.
    #[must_use]
    pub fn builtin(&self) -> bool {
        self.title.is_some()
    }
}

/// Where a workflow file is read from (D219 (2)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowLocation {
    /// The user's workflows folder, which runs in every repository.
    #[default]
    User,
    /// `.git/gitmaster/workflows/` of the repository. Its worktrees share it.
    Repository,
    /// Ships with the app.
    BuiltIn,
}

impl WorkflowLocation {
    /// The label the front ends show: `user`, `repository` or `built-in`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Repository => "repository",
            Self::BuiltIn => "built-in",
        }
    }
}

/// Whether a workflow runs as a plain workflow or as a review (D219 (1)). An enum rather than a
/// flag: `WorkflowInfo` already holds three flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowRole {
    /// A workflow, which runs on its trigger or when you run it.
    #[default]
    Workflow,
    /// A review: `review = true` in `M.meta`. It runs only when you pick it in **Run review…**.
    Review,
}

/// The workflows that are reviews, in the order given: the list the **Run review…** picker shows
/// (D219 (1)). Pure: it reads the listing and nothing else.
#[must_use]
pub fn review_workflows(workflows: &[WorkflowInfo]) -> Vec<&WorkflowInfo> {
    workflows
        .iter()
        .filter(|workflow| workflow.role == WorkflowRole::Review)
        .collect()
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

/// One problem of a script's text, as the Script tab shows it.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, review: bool) -> WorkflowInfo {
        WorkflowInfo {
            name: name.to_owned(),
            path: format!("/w/{name}.lua"),
            trigger: "manual".to_owned(),
            key: None,
            inputs: Vec::new(),
            hash: String::new(),
            error: None,
            warnings: Vec::new(),
            source: String::new(),
            reviewed: false,
            auto_approve: false,
            enabled: true,
            title: None,
            location: WorkflowLocation::User,
            role: if review {
                WorkflowRole::Review
            } else {
                WorkflowRole::Workflow
            },
            description: None,
        }
    }

    #[test]
    fn the_picker_lists_only_the_reviews_in_listing_order() {
        let listing = [
            info("tests", false),
            info("security", true),
            info("style", false),
            info("perf", true),
        ];
        let names: Vec<&str> = review_workflows(&listing)
            .iter()
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(names, ["security", "perf"]);
    }

    #[test]
    fn a_location_has_the_label_the_front_ends_show() {
        assert_eq!(WorkflowLocation::User.label(), "user");
        assert_eq!(WorkflowLocation::Repository.label(), "repository");
        assert_eq!(WorkflowLocation::BuiltIn.label(), "built-in");
        let wire = serde_json::to_value(WorkflowLocation::BuiltIn).unwrap();
        assert_eq!(wire, "built-in");
    }
}
