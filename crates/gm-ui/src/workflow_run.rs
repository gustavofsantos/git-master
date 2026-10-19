//! The window's view of the manual workflows: the files the engine found, and
//! the runs it has been told about, newest last. No GPUI in here. The composer
//! rule for a suggested message is a plain function, shared with the hooks.

use std::collections::BTreeMap;

use gm_schema::{WorkflowEnd, WorkflowInfo, WorkflowRunStatus};

use crate::hook_run::Fill;

/// How many runs the window keeps. Older ones are forgotten.
const KEPT_RUNS: usize = 10;

/// What the window knows of one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunView {
    /// The workflow's name, as the run reported it.
    pub workflow: String,
    pub status: WorkflowRunStatus,
    /// The state the run is in, when it has one.
    pub state: Option<String>,
    /// The last notice the run's `notify` steps gave.
    pub notice: Option<String>,
    /// What the run waits to be approved, while it waits.
    pub approval: Option<String>,
    /// How the run ended, with the reason when there is one.
    pub finished: Option<(WorkflowEnd, Option<String>)>,
}

impl RunView {
    fn new(workflow: String, status: WorkflowRunStatus) -> Self {
        Self {
            workflow,
            status,
            state: None,
            notice: None,
            approval: None,
            finished: None,
        }
    }

    /// Whether the run has not ended yet. A status that ends a run counts as
    /// ended even before its `WorkflowRunFinished` arrives.
    fn is_active(&self) -> bool {
        self.finished.is_none()
            && !matches!(
                self.status,
                WorkflowRunStatus::Done
                    | WorkflowRunStatus::Failed
                    | WorkflowRunStatus::Blocked
                    | WorkflowRunStatus::Cancelled
            )
    }
}

/// The workflow state the window keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workflows {
    /// Every workflow the engine listed, or `None` until the first listing.
    pub listed: Option<Vec<WorkflowInfo>>,
    /// Each directory the listing did not read, as `<path>: <reason>`.
    pub ignored: Vec<String>,
    /// The last runs, keyed by run id. Ids grow, so the last key is the newest.
    pub runs: BTreeMap<i64, RunView>,
}

impl Workflows {
    /// Forgets everything that belonged to the repository being left.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn listed(&mut self, workflows: Vec<WorkflowInfo>, ignored: Vec<String>) {
        self.listed = Some(workflows);
        self.ignored = ignored;
    }

    /// A run moved. Leaving `AwaitingApproval` clears the approval it waited on.
    pub fn run_changed(
        &mut self,
        run: i64,
        workflow: String,
        status: WorkflowRunStatus,
        state: Option<String>,
    ) {
        let view = self.entry(run, String::new());
        view.workflow = workflow;
        view.status = status;
        view.state = state;
        if status != WorkflowRunStatus::AwaitingApproval {
            view.approval = None;
        }
    }

    /// The run waits at an `approve` state; `summary` is what it asks.
    pub fn approval_requested(&mut self, run: i64, state: String, summary: String) {
        let view = self.entry(run, String::new());
        view.status = WorkflowRunStatus::AwaitingApproval;
        view.state = Some(state);
        view.approval = Some(summary);
    }

    /// A `notify` step gave a line for the user.
    pub fn notice(&mut self, run: i64, text: String) {
        self.entry(run, String::new()).notice = Some(text);
    }

    /// The run ended. Its approval, if it waited for one, is gone.
    pub fn finished(&mut self, run: i64, end: WorkflowEnd, reason: Option<String>) {
        let view = self.entry(run, String::new());
        view.status = match end {
            WorkflowEnd::Done => WorkflowRunStatus::Done,
            WorkflowEnd::Failed => WorkflowRunStatus::Failed,
            WorkflowEnd::Blocked => WorkflowRunStatus::Blocked,
        };
        view.approval = None;
        view.finished = Some((end, reason));
    }

    /// The view of `run`, made when it is new, with `workflow` as its name.
    /// Keeps only the newest runs.
    fn entry(&mut self, run: i64, workflow: String) -> &mut RunView {
        self.runs
            .entry(run)
            .or_insert_with(|| RunView::new(workflow, WorkflowRunStatus::Running));
        while self.runs.len() > KEPT_RUNS {
            self.runs.pop_first();
        }
        self.runs.get_mut(&run).expect("the run was just inserted")
    }

    /// The newest run that waits for an approval, with what it asks.
    #[must_use]
    pub fn awaiting_approval(&self) -> Option<(i64, &str)> {
        self.runs
            .iter()
            .rev()
            .find_map(|(run, view)| view.approval.as_deref().map(|summary| (*run, summary)))
    }

    /// The newest run that has not ended.
    #[must_use]
    pub fn active(&self) -> Option<i64> {
        self.runs
            .iter()
            .rev()
            .find(|(_, view)| view.is_active())
            .map(|(run, _)| *run)
    }

    /// The one line the composer shows: the active run, or else the newest one.
    #[must_use]
    pub fn status_line(&self) -> Option<String> {
        let run = self
            .active()
            .or_else(|| self.runs.keys().next_back().copied())?;
        let view = self.runs.get(&run)?;
        let head = status_text(view);
        Some(match &view.notice {
            Some(notice) => format!("{head} - {notice}"),
            None => head,
        })
    }
}

/// What the status of one run says, with its workflow's name in front.
fn status_text(view: &RunView) -> String {
    let name = if view.workflow.is_empty() {
        "workflow"
    } else {
        view.workflow.as_str()
    };
    if let Some((end, reason)) = &view.finished {
        let verb = match end {
            WorkflowEnd::Done => return format!("{name}: done"),
            WorkflowEnd::Failed => "failed",
            WorkflowEnd::Blocked => "blocked",
        };
        return match reason {
            Some(reason) => format!("{name}: {verb}: {reason}"),
            None => format!("{name}: {verb}"),
        };
    }
    if let Some(summary) = &view.approval {
        return format!("{name}: waiting for approval: {summary}");
    }
    match view.status {
        WorkflowRunStatus::Running => match &view.state {
            Some(state) => format!("{name}: running {state}"),
            None => format!("{name}: running"),
        },
        WorkflowRunStatus::Waiting => format!("{name}: waiting"),
        WorkflowRunStatus::AwaitingApproval => format!("{name}: waiting for approval"),
        WorkflowRunStatus::Done => format!("{name}: done"),
        WorkflowRunStatus::Failed => format!("{name}: failed"),
        WorkflowRunStatus::Blocked => format!("{name}: blocked"),
        WorkflowRunStatus::Cancelled => format!("{name}: cancelled"),
    }
}

/// What a suggested message does: it goes into an empty composer, and otherwise
/// it is offered as a suggestion, so typed text is never overwritten.
#[must_use]
pub fn fill_for_message(typed: &str, message: &str) -> Fill {
    if typed.trim().is_empty() {
        Fill::Apply(message.to_owned())
    } else {
        Fill::Suggest(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::WorkflowRunStatus as S;

    fn info(name: &str, trigger: &str, error: Option<&str>) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.yml"),
            trigger: trigger.into(),
            key: None,
            inputs: Vec::new(),
            hash: "h".into(),
            error: error.map(str::to_owned),
            warnings: Vec::new(),
        }
    }

    fn started(workflows: &mut Workflows, run: i64, name: &str) {
        workflows.run_changed(run, name.into(), S::Running, Some("ask".into()));
    }

    #[test]
    fn the_listing_keeps_the_workflows_and_what_was_ignored() {
        let mut workflows = Workflows::default();
        assert_eq!(workflows.listed, None);
        workflows.listed(vec![info("a", "manual", None)], vec!["/x: no file".into()]);
        assert_eq!(workflows.listed.as_ref().map(Vec::len), Some(1));
        assert_eq!(workflows.ignored, ["/x: no file"]);
    }

    #[test]
    fn a_running_step_shows_its_state_and_a_finished_run_says_done() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "suggest-commit-message");
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("suggest-commit-message: running ask")
        );
        workflows.finished(1, WorkflowEnd::Done, None);
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("suggest-commit-message: done")
        );
        assert_eq!(workflows.active(), None);
    }

    #[test]
    fn a_failure_or_a_block_keeps_its_reason() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "w");
        workflows.finished(1, WorkflowEnd::Failed, Some("exec exited with 2".into()));
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("w: failed: exec exited with 2")
        );
        started(&mut workflows, 2, "w");
        workflows.finished(2, WorkflowEnd::Blocked, None);
        assert_eq!(workflows.status_line().as_deref(), Some("w: blocked"));
    }

    #[test]
    fn an_approval_request_is_shown_and_is_the_awaiting_one() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 3, "branch-from-ticket");
        workflows.approval_requested(3, "confirm".into(), "Create branch x?".into());
        assert_eq!(workflows.awaiting_approval(), Some((3, "Create branch x?")));
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("branch-from-ticket: waiting for approval: Create branch x?")
        );
    }

    #[test]
    fn leaving_the_approval_state_clears_the_approval() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 3, "w");
        workflows.approval_requested(3, "confirm".into(), "Ok?".into());
        workflows.run_changed(3, "w".into(), S::Running, Some("next".into()));
        assert_eq!(workflows.awaiting_approval(), None);
        assert_eq!(workflows.status_line().as_deref(), Some("w: running next"));
    }

    #[test]
    fn the_newest_run_waiting_for_approval_is_the_one_named() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "a");
        started(&mut workflows, 2, "b");
        workflows.approval_requested(1, "s".into(), "first?".into());
        workflows.approval_requested(2, "s".into(), "second?".into());
        assert_eq!(workflows.awaiting_approval(), Some((2, "second?")));
    }

    #[test]
    fn active_is_the_newest_run_that_has_not_ended() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "a");
        started(&mut workflows, 2, "b");
        workflows.finished(2, WorkflowEnd::Done, None);
        assert_eq!(workflows.active(), Some(1));
    }

    #[test]
    fn a_cancelled_status_ends_a_run_even_before_it_is_finished() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "a");
        workflows.run_changed(1, "a".into(), S::Cancelled, None);
        assert_eq!(workflows.active(), None);
        assert_eq!(workflows.status_line().as_deref(), Some("a: cancelled"));
    }

    #[test]
    fn a_notice_is_shown_after_the_status() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "w");
        workflows.notice(1, "Branch created".into());
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("w: running ask - Branch created")
        );
    }

    #[test]
    fn a_status_without_a_name_falls_back_to_a_plain_word() {
        let mut workflows = Workflows::default();
        workflows.notice(9, "hello".into());
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("workflow: running - hello")
        );
    }

    #[test]
    fn no_run_means_no_line() {
        assert_eq!(Workflows::default().status_line(), None);
    }

    #[test]
    fn only_the_last_ten_runs_are_kept() {
        let mut workflows = Workflows::default();
        for run in 1..=12 {
            started(&mut workflows, run, "w");
        }
        assert_eq!(workflows.runs.len(), 10);
        assert_eq!(workflows.runs.keys().next(), Some(&3));
    }

    #[test]
    fn a_reset_forgets_the_listing_and_the_runs() {
        let mut workflows = Workflows::default();
        workflows.listed(Vec::new(), Vec::new());
        started(&mut workflows, 1, "w");
        workflows.reset();
        assert_eq!(workflows, Workflows::default());
    }

    #[test]
    fn a_message_goes_into_an_empty_composer_and_is_suggested_otherwise() {
        assert_eq!(
            fill_for_message("", "feat: x"),
            Fill::Apply("feat: x".into())
        );
        assert_eq!(
            fill_for_message("  \n", "feat: x"),
            Fill::Apply("feat: x".into())
        );
        assert_eq!(
            fill_for_message("draft", "feat: x"),
            Fill::Suggest("feat: x".into())
        );
    }
}
