//! The Workflows page's model (design/screens.md, D233): which script is open, the text of its
//! file as the last listing read it, the diagnostics of that text, its dry run, and the runs of
//! the open script. The page shows the file and never writes it: the person edits it in their
//! own editor, and each listing brings the new text. Plain
//! data and pure functions; the screen is in `shell_workflows`, and the engine's answers reach
//! this model through `Model::apply`.

use std::collections::BTreeSet;

use gm_schema::{
    CommandKind, OperationId, OperationState, WorkflowCallRecord, WorkflowDiagnostic,
    WorkflowDryRun, WorkflowInfo, WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary,
};
use serde_json::{Value, json};

/// How many runs the Runs tab asks the engine for.
pub const RUN_LIMIT: u32 = 50;

/// How many lines of a JSON block show before "Show all".
pub const SHOWN_LINES: usize = 6;

/// The two tabs of an open script: its text, and the runs of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Script,
    Runs,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Script, Tab::Runs];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Tab::Script => "Script",
            Tab::Runs => "Runs",
        }
    }
}

/// The review line of the open workflow (`WF3b`, D101): whether its version is reviewed, and whether
/// its event runs apply without asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewLine {
    Unreviewed,
    Reviewed { auto_approve: bool },
}

/// The review line of the workflow `info`. `None` when there is no such workflow, or its file does
/// not load: there is no version to review then.
#[must_use]
pub fn review_line(info: Option<&WorkflowInfo>) -> Option<ReviewLine> {
    let info = info?;
    // A built-in workflow is reviewed by the app, not by the person (D164(2)): its switch is its
    // only control.
    if info.error.is_some() || info.builtin() {
        return None;
    }
    Some(if info.reviewed {
        ReviewLine::Reviewed {
            auto_approve: info.auto_approve,
        }
    } else {
        ReviewLine::Unreviewed
    })
}

/// What the review line says.
#[must_use]
pub fn review_text(line: &ReviewLine) -> &'static str {
    match line {
        ReviewLine::Unreviewed => "Not reviewed: review this version before its runs.",
        ReviewLine::Reviewed {
            auto_approve: false,
        } => "Reviewed",
        ReviewLine::Reviewed { auto_approve: true } => "Reviewed: event runs apply without asking",
    }
}

/// The title a row of the list shows: a built-in workflow's title, else its name (`WF4b`).
#[must_use]
pub fn row_title(info: &WorkflowInfo) -> &str {
    info.title.as_deref().unwrap_or(&info.name)
}

/// What a click on "Enabled in this repository" does (`WF4b`, D164(1)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchClick {
    /// Sends this command at once: switching off needs no dialog, nor does a user workflow on.
    Send(Box<CommandKind>),
    /// Opens the Switch On dialog, which shows the built-in script and asks about the grant.
    Ask,
}

/// What clicking the switch of `info` does: switching a built-in on asks first, the rest are sent.
#[must_use]
pub fn switch_click(info: &WorkflowInfo) -> SwitchClick {
    if !info.enabled && info.builtin() {
        return SwitchClick::Ask;
    }
    SwitchClick::Send(Box::new(CommandKind::SetWorkflowEnabled {
        name: info.name.clone(),
        enabled: !info.enabled,
    }))
}

/// The words beside the switch: whether the workflow runs here, and for a built-in what to do.
#[must_use]
pub fn switch_text(info: &WorkflowInfo) -> &'static str {
    match (info.enabled, info.builtin()) {
        (true, _) => "Runs in this repository.",
        (false, true) => "Off here. Switch it on to run it in this repository.",
        (false, false) => "Off here. It does not run in this repository.",
    }
}

/// The command Copy to Edit sends for `info`: a built-in workflow only (`WF4b`, D164(4)).
#[must_use]
pub fn copy_request(info: &WorkflowInfo) -> Option<CommandKind> {
    info.builtin().then(|| CommandKind::CopyBuiltinWorkflow {
        name: info.name.clone(),
    })
}

/// The run controls the open workflow offers (`WF3b`): Approve and Reject for a run that waits for
/// an approval, and Cancel for any run that has not ended.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunControls {
    /// The run that waits, and what it asks.
    pub approve: Option<(i64, String)>,
    /// The run to stop.
    pub cancel: Option<i64>,
}

/// The run controls, from the run that waits for an approval and the run that is active.
#[must_use]
pub fn run_controls(awaiting: Option<(i64, &str)>, active: Option<i64>) -> RunControls {
    RunControls {
        approve: awaiting.map(|(run, summary)| (run, summary.to_owned())),
        cancel: active,
    }
}

/// The last dry run answered: the text it walked and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DryRunAnswer {
    source: String,
    result: WorkflowDryRun,
}

/// The workflow open on the page: its file and the text the latest listing read from it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    path: String,
    name: String,
    /// A built-in workflow: it has no file, and only Copy to Edit makes one (`WF4b`).
    builtin: bool,
    /// The file as the latest listing read it.
    text: String,
    /// The hash of that text; a delete expects it.
    hash: String,
}

/// The Workflows page's state. Reset when the repository changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowsPage {
    open: Option<Open>,
    tab: Tab,
    /// The problems of the text `checked_source` holds, as the engine last reported them.
    diagnostics: Vec<WorkflowDiagnostic>,
    /// The text the last accepted check answered for; `None` until one did.
    checked_source: Option<String>,
    /// The check the engine is computing: its operation and the text it checks.
    checking: Option<(OperationId, String)>,
    /// The text of a check request not yet sent; `track_check` takes it.
    pending_check: Option<String>,
    /// Whether the delete confirmation is showing.
    asking_delete: bool,
    /// The runs of the open workflow, once read.
    runs: Option<Vec<WorkflowRunSummary>>,
    runs_failed: bool,
    runs_op: Option<OperationId>,
    /// The run whose detail is shown, and the read of it in flight.
    run: Option<i64>,
    detail: Option<WorkflowRunDetail>,
    run_op: Option<OperationId>,
    /// The JSON blocks the user expanded with "Show all", by `expand_key`.
    expanded: BTreeSet<String>,
    /// The dry run asked for and not sent yet: the text it walks.
    pending_dry_run: Option<String>,
    /// The dry run the engine is computing: its operation and the text it walks.
    dry_running: Option<(OperationId, String)>,
    /// The last dry run answered.
    dry_run: Option<DryRunAnswer>,
}

/// What the window's action rules need to know of the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageState<'a> {
    /// Whether the Workflows page is the main screen.
    pub shown: bool,
    /// The name of the open workflow.
    pub name: Option<&'a str>,
    /// The path of the open workflow's file, which the person can open in an editor. `None` for
    /// a built-in workflow, which has none.
    pub file: Option<&'a str>,
}

impl WorkflowsPage {
    /// Forgets everything: the repository being left had its own workflows.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.path.as_str())
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.name.as_str())
    }

    /// The text of the open file; empty when no workflow is open.
    #[must_use]
    pub fn text(&self) -> &str {
        self.open.as_ref().map_or("", |open| open.text.as_str())
    }

    #[must_use]
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Every problem the last check of the text found, by line. Empty when the text loads.
    #[must_use]
    pub fn diagnostics(&self) -> &[WorkflowDiagnostic] {
        &self.diagnostics
    }

    /// Whether a check answered for the text as it was last checked: before the first answer,
    /// the Script tab says it is checking.
    #[must_use]
    pub fn check_answered(&self) -> bool {
        self.checked_source.is_some()
    }

    #[must_use]
    pub fn asking_delete(&self) -> bool {
        self.asking_delete
    }

    /// The runs read for the open workflow; `None` until the first answer.
    #[must_use]
    pub fn runs(&self) -> Option<&[WorkflowRunSummary]> {
        self.runs.as_deref()
    }

    #[must_use]
    pub fn runs_failed(&self) -> bool {
        self.runs_failed
    }

    #[must_use]
    pub fn selected_run(&self) -> Option<i64> {
        self.run
    }

    #[must_use]
    pub fn detail(&self) -> Option<&WorkflowRunDetail> {
        self.detail.as_ref()
    }

    #[must_use]
    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }

    /// Shows the JSON block `key` in full, or folds it back.
    pub fn toggle_expanded(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
    }

    /// What the window's action rules read of the page; `shown` says whether
    /// the page is the main screen.
    #[must_use]
    pub fn state(&self, shown: bool) -> PageState<'_> {
        PageState {
            shown,
            name: self.name(),
            file: self
                .open
                .as_ref()
                .filter(|open| !open.builtin)
                .map(|open| open.path.as_str()),
        }
    }

    /// Asks for a dry run of the file's text. The request is tracked by `track_dry_run` once the
    /// window has sent it.
    pub fn dry_run_request(&mut self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        self.pending_dry_run = Some(open.text.clone());
        Some(CommandKind::DryRunWorkflow {
            source: open.text.clone(),
            inputs: json!({}),
        })
    }

    /// Records the operation of the dry run the last request sent.
    pub fn track_dry_run(&mut self, id: OperationId) {
        if let Some(source) = self.pending_dry_run.take() {
            self.dry_running = Some((id, source));
        }
    }

    /// The dry run answer, when it answers the one in flight. Another
    /// operation's answer is dropped.
    pub fn dry_run_answered(&mut self, id: OperationId, result: WorkflowDryRun) {
        match self.dry_running.take() {
            Some((op, source)) if op == id => {
                self.dry_run = Some(DryRunAnswer { source, result });
            }
            other => self.dry_running = other,
        }
    }

    /// The last dry run, when it walked the file's text as it is now.
    #[must_use]
    pub fn dry_run_result(&self) -> Option<&WorkflowDryRun> {
        let answer = self.dry_run.as_ref()?;
        (self.open.as_ref().map(|open| open.text.as_str()) == Some(answer.source.as_str()))
            .then_some(&answer.result)
    }

    /// Whether a dry run answered before the file changed.
    #[must_use]
    pub fn dry_run_stale(&self) -> bool {
        self.dry_run.is_some() && self.dry_run_result().is_none()
    }

    #[must_use]
    pub fn dry_run_pending(&self) -> bool {
        self.dry_running.is_some()
    }

    /// Closes the dry run panel.
    pub fn dismiss_dry_run(&mut self) {
        self.dry_run = None;
    }

    /// Follows the engine's listing. The open file takes the listed hash and source, so an
    /// edit made in the person's editor shows. A file the listing no longer has closes.
    pub fn listed(&mut self, workflows: &[WorkflowInfo]) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(info) = workflows.iter().find(|info| info.path == open.path) else {
            self.close();
            return;
        };
        open.name.clone_from(&info.name);
        open.hash.clone_from(&info.hash);
        open.text.clone_from(&info.source);
    }

    /// Opens the workflow at `path` from the listing. `false` when the listing has no such file.
    pub fn select(&mut self, path: &str, workflows: &[WorkflowInfo]) -> bool {
        if self.path() == Some(path) {
            return true;
        }
        let Some(info) = workflows.iter().find(|info| info.path == path) else {
            return false;
        };
        self.open_file(info);
        true
    }

    /// Asks for a check of the file's text, unless the last accepted check or one in flight already
    /// answered for this text. The request is tracked by `track_check` once the window has
    /// sent it.
    pub fn check_request(&mut self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        if self.checked_source.as_deref() == Some(open.text.as_str()) {
            return None;
        }
        if self
            .checking
            .as_ref()
            .is_some_and(|(_, text)| *text == open.text)
        {
            return None;
        }
        self.pending_check = Some(open.text.clone());
        Some(CommandKind::CheckWorkflowScript {
            source: open.text.clone(),
        })
    }

    /// Records the operation of the check the last request sent. An earlier check in flight
    /// is superseded: its answer is dropped when it comes.
    pub fn track_check(&mut self, id: OperationId) {
        if let Some(source) = self.pending_check.take() {
            self.checking = Some((id, source));
        }
    }

    /// The answer to a check. It is kept only when it answers the check in flight and the
    /// file still holds the text that check read; an answer for an older text is dropped.
    pub fn checked(&mut self, id: OperationId, diagnostics: Vec<WorkflowDiagnostic>) {
        let Some((op, source)) = self.checking.take() else {
            return;
        };
        if op != id {
            self.checking = Some((op, source));
            return;
        }
        if self.text() != source {
            return;
        }
        let mut diagnostics = diagnostics;
        diagnostics.sort_by_key(|diagnostic| diagnostic.line);
        self.diagnostics = diagnostics;
        self.checked_source = Some(source);
    }

    /// Shows `tab`. The Runs tab asks for the open workflow's runs; the request
    /// comes back for the window to send and to track.
    pub fn show_tab(&mut self, tab: Tab) -> Option<CommandKind> {
        self.tab = tab;
        match tab {
            Tab::Runs => self.runs_request(),
            Tab::Script => None,
        }
    }

    /// The request for the open workflow's newest runs.
    #[must_use]
    pub fn runs_request(&self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        Some(CommandKind::ListWorkflowRuns {
            workflow: open.name.clone(),
            limit: RUN_LIMIT,
        })
    }

    /// Records the operation that answers the runs request.
    pub fn track_runs(&mut self, id: OperationId) {
        self.runs_op = Some(id);
        self.runs_failed = false;
    }

    /// The runs answer, when it is the request in flight for the open workflow.
    pub fn runs_listed(&mut self, id: OperationId, workflow: &str, runs: Vec<WorkflowRunSummary>) {
        if self.runs_op != Some(id) || self.name() != Some(workflow) {
            return;
        }
        self.runs_op = None;
        self.runs = Some(runs);
    }

    /// Selects a run of the Runs list and returns the read of its detail.
    pub fn open_run(&mut self, run: i64) -> Option<CommandKind> {
        self.open.as_ref()?;
        self.run = Some(run);
        self.detail = None;
        Some(CommandKind::LoadWorkflowRun { run })
    }

    /// Records the operation that answers a run's detail read.
    pub fn track_run(&mut self, id: OperationId) {
        self.run_op = Some(id);
    }

    /// The detail answer, when it is the read in flight for the selected run.
    pub fn run_loaded(&mut self, id: OperationId, detail: WorkflowRunDetail) {
        if self.run_op != Some(id) || self.run != Some(detail.summary.run) {
            return;
        }
        self.run_op = None;
        self.detail = Some(detail);
    }

    /// The read to send again when the selected run ended, so its steps are complete.
    #[must_use]
    pub fn refresh_run(&self, run: i64) -> Option<CommandKind> {
        (self.run == Some(run)).then_some(CommandKind::LoadWorkflowRun { run })
    }

    /// An operation of the page ended: a read that failed stops being waited for.
    pub fn operation_ended(&mut self, id: OperationId, state: OperationState) {
        if !state.is_terminal() {
            return;
        }
        if self.runs_op == Some(id) {
            self.runs_op = None;
            self.runs_failed = state == OperationState::Failed;
        }
        if self.run_op == Some(id) {
            self.run_op = None;
        }
        // A done dry run may still have its answer to come, so only a failure clears it.
        if state != OperationState::Done && self.dry_running.as_ref().map(|(op, _)| *op) == Some(id)
        {
            self.dry_running = None;
        }
        if state != OperationState::Done && self.checking.as_ref().map(|(op, _)| *op) == Some(id) {
            self.checking = None;
        }
    }

    /// The delete confirmation opens for the open workflow.
    pub fn ask_delete(&mut self) {
        if self.open.as_ref().is_some_and(|open| !open.builtin) {
            self.asking_delete = true;
        }
    }

    /// Whether the open workflow is a built-in one: it has no file to open or delete.
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.builtin)
    }

    /// Closes the delete confirmation without deleting.
    pub fn cancel_delete(&mut self) {
        self.asking_delete = false;
    }

    /// The request that moves the open file to the trash, expecting the hash the listing
    /// last read. Closes the confirmation.
    pub fn delete_request(&mut self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        if open.builtin {
            return None;
        }
        let expected_hash = open.hash.clone();
        let path = open.path.clone();
        self.asking_delete = false;
        Some(CommandKind::DeleteWorkflowFile {
            path,
            expected_hash,
        })
    }

    /// The engine moved `path` to the trash: a page on it closes.
    pub fn deleted(&mut self, path: &str) {
        if self.path() == Some(path) {
            self.close();
        }
    }

    fn open_file(&mut self, info: &WorkflowInfo) {
        let tab = self.tab;
        *self = Self::default();
        self.tab = tab;
        self.open = Some(Open {
            path: info.path.clone(),
            name: info.name.clone(),
            builtin: info.builtin(),
            text: info.source.clone(),
            hash: info.hash.clone(),
        });
    }

    /// Closes the open workflow and keeps the tab.
    fn close(&mut self) {
        let tab = self.tab;
        *self = Self::default();
        self.tab = tab;
    }
}

/// The text of one diagnostic under the script: `line N: message`.
#[must_use]
pub fn diagnostic_text(diagnostic: &WorkflowDiagnostic) -> String {
    if diagnostic.line == 0 {
        diagnostic.message.clone()
    } else {
        format!("line {}: {}", diagnostic.line, diagnostic.message)
    }
}

/// The text of a run's status, as the Runs tab shows it.
#[must_use]
pub fn status_label(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "Running",
        WorkflowRunStatus::Waiting => "Waiting",
        WorkflowRunStatus::AwaitingApproval => "Awaiting approval",
        WorkflowRunStatus::Done => "Done",
        WorkflowRunStatus::Failed => "Failed",
        WorkflowRunStatus::Blocked => "Blocked",
        WorkflowRunStatus::Cancelled => "Cancelled",
        WorkflowRunStatus::Interrupted => "Interrupted",
    }
}

/// A span in milliseconds, as a short word: `850 ms`, `3.2 s`, `2 min 4 s`.
#[must_use]
pub fn duration_label(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        // Tenths of a second, rounded, without floating point.
        let tenths = (ms + 50) / 100;
        format!("{}.{} s", tenths / 10, tenths % 10)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

/// How long a run ran, when it has ended.
#[must_use]
pub fn run_duration(run: &WorkflowRunSummary) -> Option<i64> {
    run.ended_at.map(|ended| ended - run.started_at)
}

/// How long a step took, when it has ended.
#[must_use]
pub fn step_duration(step: &StepRecord) -> Option<i64> {
    step.ended_at.map(|ended| ended - step.started_at)
}

/// Pretty JSON for a context value or a step's input or output.
#[must_use]
pub fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// The part of a JSON block the page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub text: String,
    /// How many lines are folded away; zero when the block is whole.
    pub hidden: usize,
}

/// A block's text, folded to `SHOWN_LINES` lines unless it is `expanded` or
/// already that short.
#[must_use]
pub fn preview(text: &str, expanded: bool) -> Preview {
    let lines: Vec<&str> = text.lines().collect();
    if expanded || lines.len() <= SHOWN_LINES {
        return Preview {
            text: text.to_owned(),
            hidden: 0,
        };
    }
    Preview {
        text: lines[..SHOWN_LINES].join("\n"),
        hidden: lines.len() - SHOWN_LINES,
    }
}

/// The key of one JSON block of a run's timeline, for `toggle_expanded`.
#[must_use]
pub fn expand_key(run: i64, step: usize, part: &str) -> String {
    format!("run-{run}-step-{step}-{part}")
}

/// The key of a run's context block, for `toggle_expanded`.
#[must_use]
pub fn context_key(run: i64, part: &str) -> String {
    format!("run-{run}-{part}")
}

/// The three blocks of a run's context, in the order the page shows them. The input value holds
/// `inputs` and `repo`.
#[must_use]
pub fn context_blocks(detail: &WorkflowRunDetail) -> [(&'static str, String); 3] {
    static NOTHING: Value = Value::Null;
    [
        (
            "Inputs",
            pretty_json(detail.input.get("inputs").unwrap_or(&NOTHING)),
        ),
        ("Trigger", pretty_json(&detail.trigger)),
        (
            "Repository",
            pretty_json(detail.input.get("repo").unwrap_or(&NOTHING)),
        ),
    ]
}

/// One `ctx` call of a run, as the run view shows it: the call as the host recorded it.
#[derive(Clone, Debug, PartialEq)]
pub struct StepRecord {
    /// The call's number in the run, from 1.
    pub seq: u32,
    /// The function without `ctx.`, such as `branch.create`.
    pub func: String,
    /// `read`, `propose`, `apply`, `pause` or `helper`; empty when the catalog does not know it.
    pub level: String,
    /// The arguments the call received.
    pub input: Value,
    /// What the call answered; `None` until it ends.
    pub output: Option<Value>,
    pub outcome: Option<String>,
    /// The reason of an outcome other than `done`, when there is one.
    pub message: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// The calls of a run, in the order they were made.
#[must_use]
pub fn step_rows(detail: &WorkflowRunDetail) -> Vec<StepRecord> {
    detail.calls.iter().map(step_record).collect()
}

fn step_record(call: &WorkflowCallRecord) -> StepRecord {
    StepRecord {
        seq: call.seq,
        func: call.func.clone(),
        level: call.level.clone(),
        input: call.args.clone(),
        output: call.result.clone(),
        outcome: call.outcome.clone(),
        message: call.message.clone(),
        started_at: call.started_at,
        ended_at: call.duration_ms.map(|ms| call.started_at + ms),
    }
}

/// The step's level and outcome in one line, such as `apply · done` or `read · failed: no repo`.
#[must_use]
pub fn step_summary(step: &StepRecord) -> String {
    let outcome = outcome_line(step);
    if step.level.is_empty() {
        outcome
    } else {
        format!("{} · {outcome}", step.level)
    }
}

/// The step's outcome, with its reason when there is one, or a line saying the call has not
/// answered.
#[must_use]
pub fn outcome_line(step: &StepRecord) -> String {
    match (&step.outcome, &step.message) {
        (Some(outcome), Some(message)) => format!("{outcome}: {message}"),
        (Some(outcome), None) => outcome.clone(),
        (None, _) => "no outcome yet".to_owned(),
    }
}

/// The text of the Input block: the arguments the call received.
#[must_use]
pub fn input_text(step: &StepRecord) -> String {
    pretty_json(&step.input)
}

/// The text of the Output block, or a line saying the step has not answered.
#[must_use]
pub fn output_text(step: &StepRecord) -> String {
    match &step.output {
        Some(value) => pretty_json(value),
        None => "No output yet".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::WorkflowRunSummary;
    use serde_json::json;

    const SAMPLE: &str = r#"local M = {}

M.meta = {
  name = "ticket-branch",
  description = "Create a branch from a ticket",
  inputs = { { name = "ticket", prompt = "Ticket key" } },
  trigger = { on = "manual" },
}

function M.execute(ctx, input)
  local name = ctx.exec({ argv = { "ticket-branch-name", input.inputs.ticket } })
  ctx.branch.create({ name = name.text, checkout = true })
end

return M
"#;

    fn info(path: &str, name: &str, source: &str, hash: &str) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: path.into(),
            trigger: "manual".into(),
            key: None,
            inputs: Vec::new(),
            hash: hash.into(),
            error: None,
            warnings: Vec::new(),
            source: source.into(),
            reviewed: false,
            auto_approve: false,
            enabled: false,
            title: None,
            location: gm_schema::WorkflowLocation::User,
            role: gm_schema::WorkflowRole::Workflow,
            description: None,
        }
    }

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    fn summary(run: i64, status: WorkflowRunStatus) -> WorkflowRunSummary {
        WorkflowRunSummary {
            run,
            workflow: "ticket-branch".into(),
            status,
            started_by: "human".into(),
            started_at: 1_000,
            ended_at: Some(4_500),
            end_reason: None,
            current_state: None,
        }
    }

    fn opened() -> (WorkflowsPage, Vec<WorkflowInfo>) {
        let listed = vec![info("/w/ticket.lua", "ticket-branch", SAMPLE, "h1")];
        let mut page = WorkflowsPage::default();
        assert!(page.select("/w/ticket.lua", &listed));
        (page, listed)
    }

    #[test]
    fn a_selected_file_loads_clean_with_its_hash_and_no_diagnostics() {
        let (page, _) = opened();
        assert_eq!(page.name(), Some("ticket-branch"));
        assert_eq!(page.text(), SAMPLE);
        assert!(page.diagnostics().is_empty());
        assert_eq!(page.state(true).file, Some("/w/ticket.lua"));
    }

    /// The file changed outside the app: the next listing reads `text` from it.
    fn edit(page: &mut WorkflowsPage, text: &str) {
        let path = page.path().expect("a workflow is open").to_owned();
        let name = page.name().expect("a workflow is open").to_owned();
        page.listed(&[info(&path, &name, text, "edited")]);
    }

    #[test]
    fn a_listing_with_a_new_text_shows_the_outside_edit_and_a_delete_expects_its_hash() {
        let (mut page, _) = opened();
        page.listed(&[info("/w/ticket.lua", "renamed", "return {}\n", "h9")]);
        assert_eq!(page.text(), "return {}\n");
        assert_eq!(page.name(), Some("renamed"));
        page.ask_delete();
        assert_eq!(
            page.delete_request(),
            Some(CommandKind::DeleteWorkflowFile {
                path: "/w/ticket.lua".into(),
                expected_hash: "h9".into(),
            })
        );
    }

    #[test]
    fn the_page_names_the_file_of_a_user_workflow_and_none_for_a_built_in() {
        let (page, _) = opened();
        assert_eq!(page.state(true).file, Some("/w/ticket.lua"));
        assert_eq!(WorkflowsPage::default().state(true).file, None);
        let mut built_in = info("builtin:x", "x", SAMPLE, "h");
        built_in.title = Some("X".into());
        let mut page = WorkflowsPage::default();
        assert!(page.select("builtin:x", std::slice::from_ref(&built_in)));
        assert_eq!(page.state(true).file, None);
    }

    #[test]
    fn a_file_the_listing_no_longer_has_closes() {
        let (mut page, _) = opened();
        page.listed(&[]);
        assert_eq!(page.path(), None);
        assert_eq!(page.text(), "");
    }

    #[test]
    fn the_delete_confirmation_sends_the_listed_hash_and_closes() {
        let (mut page, _) = opened();
        page.cancel_delete();
        assert!(!page.asking_delete());
        page.ask_delete();
        assert!(page.asking_delete());
        let kind = page.delete_request().expect("delete sends");
        assert_eq!(
            kind,
            CommandKind::DeleteWorkflowFile {
                path: "/w/ticket.lua".into(),
                expected_hash: "h1".into(),
            }
        );
        assert!(!page.asking_delete());
        page.deleted("/w/ticket.lua");
        assert_eq!(page.path(), None);
    }

    #[test]
    fn a_page_with_no_workflow_asks_for_nothing() {
        let mut page = WorkflowsPage::default();
        page.ask_delete();
        assert!(!page.asking_delete());
        assert_eq!(page.state(true).file, None);
        assert_eq!(page.delete_request(), None);
        assert_eq!(page.show_tab(Tab::Runs), None);
    }

    #[test]
    fn the_runs_tab_asks_for_fifty_runs_of_the_open_workflow() {
        let (mut page, _) = opened();
        assert_eq!(
            page.show_tab(Tab::Runs),
            Some(CommandKind::ListWorkflowRuns {
                workflow: "ticket-branch".into(),
                limit: 50,
            })
        );
        assert_eq!(page.tab(), Tab::Runs);
        assert_eq!(page.show_tab(Tab::Script), None);
    }

    #[test]
    fn runs_answer_only_the_request_in_flight_for_the_same_workflow() {
        let (mut page, _) = opened();
        page.track_runs(id(10));
        page.runs_listed(
            id(9),
            "ticket-branch",
            vec![summary(1, WorkflowRunStatus::Done)],
        );
        assert_eq!(page.runs(), None, "a stale answer is dropped");
        page.runs_listed(id(10), "other", vec![]);
        assert_eq!(page.runs(), None, "another workflow's answer is dropped");
        page.runs_listed(
            id(10),
            "ticket-branch",
            vec![summary(1, WorkflowRunStatus::Done)],
        );
        assert_eq!(page.runs().map(<[WorkflowRunSummary]>::len), Some(1));
    }

    #[test]
    fn a_failed_runs_read_says_so() {
        let (mut page, _) = opened();
        page.track_runs(id(11));
        page.operation_ended(id(11), OperationState::Failed);
        assert!(page.runs_failed());
        assert_eq!(page.runs(), None);
    }

    #[test]
    fn a_run_loads_into_the_detail_and_a_finished_run_asks_again() {
        let (mut page, _) = opened();
        assert_eq!(
            page.open_run(3),
            Some(CommandKind::LoadWorkflowRun { run: 3 })
        );
        assert_eq!(page.selected_run(), Some(3));
        page.track_run(id(20));
        let detail = WorkflowRunDetail {
            summary: summary(3, WorkflowRunStatus::Done),
            definition: String::new(),
            input: json!({"inputs": json!({}), "repo": json!({})}),
            trigger: json!({}),
            calls: Vec::new(),
        };
        page.run_loaded(id(21), detail.clone());
        assert!(
            page.detail().is_none(),
            "another operation's answer is dropped"
        );
        page.run_loaded(id(20), detail);
        assert_eq!(page.detail().map(|d| d.summary.run), Some(3));
        assert_eq!(
            page.refresh_run(3),
            Some(CommandKind::LoadWorkflowRun { run: 3 })
        );
        assert_eq!(page.refresh_run(4), None);
    }

    #[test]
    fn choosing_another_run_drops_the_old_detail() {
        let (mut page, _) = opened();
        page.open_run(1);
        page.track_run(id(1));
        page.run_loaded(
            id(1),
            WorkflowRunDetail {
                summary: summary(1, WorkflowRunStatus::Done),
                definition: String::new(),
                input: json!({"inputs": json!(null), "repo": json!(null)}),
                trigger: json!(null),
                calls: Vec::new(),
            },
        );
        page.open_run(2);
        assert!(page.detail().is_none());
    }

    #[test]
    fn expanded_blocks_toggle_by_key() {
        let mut page = WorkflowsPage::default();
        let key = expand_key(1, 0, "input");
        assert!(!page.is_expanded(&key));
        page.toggle_expanded(&key);
        assert!(page.is_expanded(&key));
        page.toggle_expanded(&key);
        assert!(!page.is_expanded(&key));
    }

    #[test]
    fn a_reset_forgets_the_open_workflow_and_its_runs() {
        let (mut page, _) = opened();
        page.track_runs(id(1));
        page.reset();
        assert_eq!(page, WorkflowsPage::default());
    }

    #[test]
    fn durations_read_as_ms_seconds_or_minutes() {
        assert_eq!(duration_label(850), "850 ms");
        assert_eq!(duration_label(3_200), "3.2 s");
        assert_eq!(duration_label(124_000), "2 min 4 s");
        assert_eq!(duration_label(-5), "0 ms");
    }

    #[test]
    fn a_run_and_a_step_last_from_start_to_end() {
        let run = summary(1, WorkflowRunStatus::Done);
        assert_eq!(run_duration(&run), Some(3_500));
        let mut open = run;
        open.ended_at = None;
        assert_eq!(run_duration(&open), None);
    }

    #[test]
    fn statuses_have_their_words() {
        assert_eq!(
            status_label(WorkflowRunStatus::AwaitingApproval),
            "Awaiting approval"
        );
        assert_eq!(status_label(WorkflowRunStatus::Cancelled), "Cancelled");
        assert_eq!(status_label(WorkflowRunStatus::Blocked), "Blocked");
    }

    #[test]
    fn a_long_block_folds_to_six_lines_and_says_how_many_hide() {
        let text = (1..=9)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let folded = preview(&text, false);
        assert_eq!(folded.hidden, 3);
        assert_eq!(folded.text.lines().count(), 6);
        assert_eq!(
            preview(&text, true),
            Preview {
                text: text.clone(),
                hidden: 0
            }
        );
        assert_eq!(preview("a\nb", false).hidden, 0);
    }

    #[test]
    fn json_is_pretty_printed() {
        let text = pretty_json(&json!({"a": 1}));
        assert!(text.contains('\n'));
        assert!(text.contains("\"a\": 1"));
    }

    fn call(seq: u32, func: &str) -> WorkflowCallRecord {
        WorkflowCallRecord {
            seq,
            func: func.into(),
            level: "read".into(),
            args: json!({"argv": ["git", "status"]}),
            result: Some(json!({"text": "x"})),
            outcome: Some("done".into()),
            message: None,
            started_at: 100,
            duration_ms: Some(20),
        }
    }

    #[test]
    fn a_step_shows_its_level_outcome_and_reason() {
        let mut failed = call(2, "branch.create");
        failed.level = "apply".into();
        failed.outcome = Some("conflict".into());
        failed.message = Some("the name is taken".into());
        let step = step_record(&failed);
        assert_eq!(step.func, "branch.create");
        assert_eq!(step_summary(&step), "apply · conflict: the name is taken");
        assert_eq!(step_duration(&step), Some(20));
        assert!(input_text(&step).contains("\"argv\""));
        assert!(output_text(&step).contains("\"text\": \"x\""));

        let mut running = call(3, "exec");
        running.level = String::new();
        running.outcome = None;
        running.result = None;
        running.duration_ms = None;
        let step = step_record(&running);
        assert_eq!(step_summary(&step), "no outcome yet");
        assert_eq!(output_text(&step), "No output yet");
        assert_eq!(step_duration(&step), None);
    }

    #[test]
    fn the_steps_keep_the_order_of_the_calls() {
        let detail = WorkflowRunDetail {
            summary: summary(1, WorkflowRunStatus::Done),
            definition: String::new(),
            input: json!({}),
            trigger: json!({}),
            calls: vec![call(1, "exec"), call(2, "notify")],
        };
        let steps = step_rows(&detail);
        let funcs: Vec<(u32, &str)> = steps.iter().map(|s| (s.seq, s.func.as_str())).collect();
        assert_eq!(funcs, [(1, "exec"), (2, "notify")]);
    }

    #[test]
    fn the_context_shows_inputs_trigger_and_repo_in_that_order() {
        let detail = WorkflowRunDetail {
            summary: summary(1, WorkflowRunStatus::Done),
            definition: String::new(),
            input: json!({"inputs": json!({"ticket": "ENG-1"}), "repo": json!({"branch": "main"})}),
            trigger: json!({"on": "manual"}),
            calls: Vec::new(),
        };
        let blocks = context_blocks(&detail);
        let labels: Vec<&str> = blocks.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["Inputs", "Trigger", "Repository"]);
        assert!(blocks[0].1.contains("ENG-1"));
    }

    /// A script that is not the open file's text: the dry run walks whatever the file holds.
    const FLOW: &str = "return {}\n-- a second line\n";

    fn page_with(source: &str) -> WorkflowsPage {
        let listed = vec![info("/w/flow.lua", "flow", source, "h1")];
        let mut page = WorkflowsPage::default();
        assert!(page.select("/w/flow.lua", &listed));
        page
    }

    fn answer() -> WorkflowDryRun {
        WorkflowDryRun {
            error: None,
            missing_inputs: Vec::new(),
            calls: Vec::new(),
            stop: "end.done".into(),
        }
    }

    #[test]
    fn a_dry_run_answer_is_kept_for_its_request_and_stale_after_an_edit() {
        let mut page = page_with(FLOW);
        let _ = page.dry_run_request();
        page.track_dry_run(id(5));
        assert!(page.dry_run_pending());
        page.dry_run_answered(id(4), answer());
        assert_eq!(
            page.dry_run_result(),
            None,
            "another operation's answer is dropped"
        );
        assert!(page.dry_run_pending());
        page.dry_run_answered(id(5), answer());
        assert_eq!(page.dry_run_result(), Some(&answer()));
        assert!(!page.dry_run_pending());
        assert!(!page.dry_run_stale());

        edit(&mut page, &format!("{FLOW}# note\n"));
        assert_eq!(page.dry_run_result(), None);
        assert!(page.dry_run_stale());
        page.dismiss_dry_run();
        assert!(!page.dry_run_stale());
    }

    fn problem(line: u32, message: &str) -> WorkflowDiagnostic {
        WorkflowDiagnostic {
            line,
            message: message.into(),
        }
    }

    #[test]
    fn a_check_request_asks_once_per_text() {
        let mut page = page_with(FLOW);
        assert!(page.check_request().is_some(), "a new file is checked");
        page.track_check(id(30));
        assert_eq!(page.check_request(), None, "the same text is in flight");

        page.checked(id(30), vec![problem(2, "bad")]);
        assert_eq!(page.diagnostics(), [problem(2, "bad")]);
        assert_eq!(
            page.check_request(),
            None,
            "the answered text is not asked again"
        );

        edit(&mut page, &format!("{FLOW}x\n"));
        assert_eq!(
            page.check_request(),
            Some(CommandKind::CheckWorkflowScript {
                source: format!("{FLOW}x\n"),
            })
        );
    }

    #[test]
    fn a_check_answer_for_an_older_edit_is_dropped() {
        let mut page = page_with(FLOW);
        let _ = page.check_request();
        page.track_check(id(40));
        edit(&mut page, "return 1\n");
        let _ = page.check_request();
        page.track_check(id(41));

        page.checked(id(40), vec![problem(1, "old")]);
        assert!(
            page.diagnostics().is_empty(),
            "a superseded answer is dropped"
        );
        page.checked(id(41), vec![problem(1, "new")]);
        assert_eq!(page.diagnostics(), [problem(1, "new")]);
    }

    #[test]
    fn a_check_answer_for_text_the_file_no_longer_holds_is_dropped() {
        let mut page = page_with(FLOW);
        let _ = page.check_request();
        page.track_check(id(50));
        edit(&mut page, "changed while checking\n");
        page.checked(id(50), vec![problem(3, "stale")]);
        assert!(page.diagnostics().is_empty());
        assert!(
            page.check_request().is_some(),
            "the text is checked again after the answer"
        );
    }

    #[test]
    fn a_check_answers_its_problems_by_line_and_a_failed_check_is_no_longer_in_flight() {
        let mut page = page_with(FLOW);
        let _ = page.check_request();
        page.track_check(id(60));
        page.checked(id(60), vec![problem(9, "b"), problem(2, "a")]);
        let lines: Vec<u32> = page.diagnostics().iter().map(|d| d.line).collect();
        assert_eq!(lines, [2, 9]);

        edit(&mut page, "another\n");
        let _ = page.check_request();
        page.track_check(id(61));
        page.operation_ended(id(61), OperationState::Failed);
        assert!(
            page.check_request().is_some(),
            "a failed check can be asked again"
        );
    }

    #[test]
    fn a_failed_dry_run_is_no_longer_pending() {
        let mut page = page_with(FLOW);
        let _ = page.dry_run_request();
        page.track_dry_run(id(7));
        page.operation_ended(id(7), OperationState::Failed);
        assert!(!page.dry_run_pending());
    }

    #[test]
    fn a_workflow_that_does_not_load_has_no_review_line() {
        assert_eq!(review_line(None), None);
        let mut broken = info("/w/a.lua", "a", "", "h");
        broken.error = Some("line 1: bad".into());
        assert_eq!(review_line(Some(&broken)), None);
    }

    #[test]
    fn a_version_is_unreviewed_until_the_listing_says_it_is_reviewed() {
        let mut loaded = info("/w/a.lua", "a", "", "h");
        assert_eq!(review_line(Some(&loaded)), Some(ReviewLine::Unreviewed));
        loaded.reviewed = true;
        assert_eq!(
            review_line(Some(&loaded)),
            Some(ReviewLine::Reviewed {
                auto_approve: false
            })
        );
        loaded.auto_approve = true;
        assert_eq!(
            review_line(Some(&loaded)),
            Some(ReviewLine::Reviewed { auto_approve: true })
        );
    }

    #[test]
    fn the_review_line_has_its_words() {
        assert_eq!(
            review_text(&ReviewLine::Unreviewed),
            "Not reviewed: review this version before its runs."
        );
        assert_eq!(
            review_text(&ReviewLine::Reviewed {
                auto_approve: false
            }),
            "Reviewed"
        );
        assert_eq!(
            review_text(&ReviewLine::Reviewed { auto_approve: true }),
            "Reviewed: event runs apply without asking"
        );
    }

    #[test]
    fn approve_and_reject_follow_a_run_that_waits_and_cancel_follows_any_active_run() {
        assert_eq!(run_controls(None, None), RunControls::default());
        assert_eq!(
            run_controls(None, Some(3)),
            RunControls {
                approve: None,
                cancel: Some(3)
            }
        );
        assert_eq!(
            run_controls(Some((4, "push the branch")), Some(4)),
            RunControls {
                approve: Some((4, "push the branch".into())),
                cancel: Some(4)
            }
        );
    }

    fn built_in(name: &str, title: &str, enabled: bool) -> WorkflowInfo {
        WorkflowInfo {
            trigger: "stack.parent_moved".into(),
            title: Some(title.into()),
            enabled,
            reviewed: true,
            path: format!("builtin:{name}"),
            ..info(&format!("builtin:{name}"), name, SAMPLE, "shipped")
        }
    }

    #[test]
    fn a_built_in_shows_its_title_and_a_user_workflow_its_name() {
        let shipped = built_in(
            "restack-stale-after-fetch",
            "Restack stale stacks after fetch",
            false,
        );
        assert_eq!(row_title(&shipped), "Restack stale stacks after fetch");
        assert_eq!(row_title(&info("/w/x.lua", "mine", SAMPLE, "h")), "mine");
    }

    #[test]
    fn switching_a_built_in_on_asks_and_switching_off_is_sent_at_once() {
        let off = built_in("restack-stale-after-fetch", "t", false);
        assert_eq!(switch_click(&off), SwitchClick::Ask);
        let on = built_in("restack-stale-after-fetch", "t", true);
        assert_eq!(
            switch_click(&on),
            SwitchClick::Send(Box::new(CommandKind::SetWorkflowEnabled {
                name: "restack-stale-after-fetch".into(),
                enabled: false,
            }))
        );
    }

    #[test]
    fn a_user_workflow_switches_at_once_both_ways() {
        let mut mine = info("/w/mine.lua", "mine", SAMPLE, "h");
        mine.enabled = true;
        assert_eq!(
            switch_click(&mine),
            SwitchClick::Send(Box::new(CommandKind::SetWorkflowEnabled {
                name: "mine".into(),
                enabled: false,
            }))
        );
        mine.enabled = false;
        assert_eq!(
            switch_click(&mine),
            SwitchClick::Send(Box::new(CommandKind::SetWorkflowEnabled {
                name: "mine".into(),
                enabled: true,
            }))
        );
    }

    #[test]
    fn the_switch_says_whether_the_workflow_runs_here() {
        let mut mine = info("/w/mine.lua", "mine", SAMPLE, "h");
        mine.enabled = true;
        assert_eq!(switch_text(&mine), "Runs in this repository.");
        mine.enabled = false;
        assert_eq!(
            switch_text(&mine),
            "Off here. It does not run in this repository."
        );
        let off = built_in("b", "t", false);
        assert_eq!(
            switch_text(&off),
            "Off here. Switch it on to run it in this repository."
        );
    }

    #[test]
    fn copy_to_edit_is_for_built_ins_only() {
        assert_eq!(
            copy_request(&built_in("restack-stale-after-fetch", "t", false)),
            Some(CommandKind::CopyBuiltinWorkflow {
                name: "restack-stale-after-fetch".into(),
            })
        );
        assert_eq!(
            copy_request(&info("/w/mine.lua", "mine", SAMPLE, "h")),
            None
        );
    }

    #[test]
    fn a_built_in_is_never_reviewed_by_hand_and_has_no_file_to_open_or_delete() {
        let shipped = built_in("restack-stale-after-fetch", "t", false);
        assert_eq!(review_line(Some(&shipped)), None);

        let mut page = WorkflowsPage::default();
        assert!(page.select(&shipped.path, std::slice::from_ref(&shipped)));
        assert!(page.is_builtin());
        assert_eq!(page.state(true).file, None);
        page.ask_delete();
        assert!(!page.asking_delete());
        assert_eq!(page.delete_request(), None);
    }

    #[test]
    fn a_user_file_opens_in_an_editor_and_deletes() {
        let (mut page, _) = opened();
        assert!(!page.is_builtin());
        assert!(page.state(true).file.is_some());
        page.ask_delete();
        assert!(page.asking_delete());
    }
}
