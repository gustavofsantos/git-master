//! The window's view of the manual workflows: the files the engine found, and
//! the runs it has been told about, newest last. No GPUI in here. The composer
//! rule for a suggested message is a plain function, shared with the transforms.

use std::collections::BTreeMap;

use gm_schema::{
    OperationId, RestackBlock, RestackPlan, WorkflowEnd, WorkflowInfo, WorkflowRunStatus,
};

use crate::suggestion::Fill;
use crate::workflow_keys::KeyPlan;

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
    /// The restack the approval would move, when it is an apply of a restack (`WF4c`).
    pub approval_plan: Option<RestackPlan>,
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
            approval_plan: None,
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

/// A version of a script the person is asked to review (D101): the window shows `source`, and
/// confirming marks `hash` as reviewed, with the grant the person chose when the trigger is not
/// `manual`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRequest {
    pub name: String,
    pub hash: String,
    pub source: String,
    /// The inputs of the run that waits for the review.
    pub inputs: BTreeMap<String, String>,
    /// The workflow's trigger `on`: `manual`, an event such as `commit.created`, or a phase such
    /// as `before.commit`.
    pub on: String,
    /// Whether confirming runs the script too, as a manual run does. Event and phase workflows
    /// are only marked reviewed: their runs come from their events (`WF3b`).
    pub run: bool,
    /// The dialog switches the workflow on in this repository (`WF4b`): confirming sends
    /// `SetWorkflowEnabled`, and the grant when it changes.
    pub switch: bool,
    /// The grant the version has now, so the dialog starts from it and a change is sent.
    pub granted: bool,
}

/// An event run that waits for the review of its version (`WF3b`, D155(3)): the window shows a
/// notice with Review and Dismiss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WantedRun {
    pub hash: String,
    /// The event that asked, such as `commit.created`, or a transform's `transform.commit_message`.
    pub trigger: String,
}

/// A run stopped on a restack conflict (`WF4c`): the window shows a notice with **Resolve conflict**
/// until the run ends. `plan` is the restack after the apply, with the blocked branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestackConflictNotice {
    pub run: i64,
    pub workflow: String,
    pub plan: RestackPlan,
}

impl RestackConflictNotice {
    /// The branch the notice names: the first one the plan blocks on a conflict.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        conflict_branch(&self.plan)
    }
}

/// The first branch of `plan` that is blocked on a conflict. Pure.
#[must_use]
pub fn conflict_branch(plan: &RestackPlan) -> Option<&str> {
    plan.branches
        .iter()
        .find(|branch| matches!(branch.block, Some(RestackBlock::Conflict { .. })))
        .map(|branch| branch.name.as_str())
}

/// The line of the notice: the workflow's name and the branch it stopped on. Pure.
#[must_use]
pub fn restack_conflict_text(workflow: &str, branch: &str) -> String {
    format!("{workflow} stopped on a restack conflict in {branch}")
}

/// A gate that is not reviewed vetoed the operation `op` (`WF3b`, D157(2)): the veto's line gets a
/// Review link for this version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateReview {
    pub op: OperationId,
    pub workflow: String,
    pub hash: String,
}

/// The workflow state the window keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workflows {
    /// The version waiting for the person's review, until they answer.
    pub review: Option<ReviewRequest>,
    /// The event runs that wait for their version's review, by workflow name (`WF3b`).
    pub wanted: BTreeMap<String, WantedRun>,
    /// The gate that vetoed the last operation for want of a review, if any.
    pub gate_review: Option<GateReview>,
    /// Every workflow the engine listed, or `None` until the first listing.
    pub listed: Option<Vec<WorkflowInfo>>,
    /// Each directory the listing did not read, as `<path>: <reason>`.
    pub ignored: Vec<String>,
    /// The user's workflows directory, when the engine knows where it is.
    pub user_dir: Option<String>,
    /// The built-in templates `CreateWorkflowFromTemplate` takes, by name.
    pub templates: Vec<String>,
    /// The file the last `WorkflowCreated` named, until a run starts.
    pub created: Option<String>,
    /// The file a template just created, until the listing has it and the page opens it.
    pub fresh: Option<String>,
    /// The shortcuts of the manual workflows: the ones bound and the ones left out.
    pub keys: KeyPlan,
    /// The last runs, keyed by run id. Ids grow, so the last key is the newest.
    pub runs: BTreeMap<i64, RunView>,
    /// The run that stopped on a restack conflict, until the run ends (`WF4c`).
    pub restack_conflict: Option<RestackConflictNotice>,
}

impl Workflows {
    /// Forgets everything that belonged to the repository being left.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn listed(
        &mut self,
        workflows: Vec<WorkflowInfo>,
        ignored: Vec<String>,
        user_dir: Option<String>,
        templates: Vec<String>,
    ) {
        self.listed = Some(workflows);
        self.ignored = ignored;
        self.user_dir = user_dir;
        self.templates = templates;
    }

    /// The engine asks for a review before it runs `name`.
    pub fn review_requested(
        &mut self,
        name: String,
        hash: String,
        source: String,
        inputs: BTreeMap<String, String>,
    ) {
        self.review = Some(ReviewRequest {
            name,
            hash,
            source,
            inputs,
            on: "manual".to_owned(),
            run: true,
            switch: false,
            granted: false,
        });
    }

    /// An event run of `workflow` waits for the review of `hash`. The latest request of a workflow
    /// is the one kept (`WF3b`).
    pub fn wants_review(&mut self, workflow: String, hash: String, trigger: String) {
        self.wanted.insert(workflow, WantedRun { hash, trigger });
    }

    /// The request of `workflow` ended: reviewed and started, dismissed or stale.
    pub fn request_dropped(&mut self, workflow: &str) {
        self.wanted.remove(workflow);
    }

    /// A gate of `op` is not reviewed: its veto gets the Review link for `workflow` at `hash`.
    pub fn gate_unreviewed(&mut self, op: OperationId, workflow: String, hash: String) {
        self.gate_review = Some(GateReview { op, workflow, hash });
    }

    /// An operation changed. The gate's Review link belongs to its own veto only, so any other
    /// operation's change drops it.
    pub fn operation_changed(&mut self, op: OperationId) {
        if self.gate_review.as_ref().is_some_and(|gate| gate.op != op) {
            self.gate_review = None;
        }
    }

    /// The newest run of `workflow` that waits for an approval, with what it asks.
    #[must_use]
    pub fn awaiting_approval_of(&self, workflow: &str) -> Option<(i64, &str)> {
        self.runs
            .iter()
            .rev()
            .filter(|(_, view)| view.workflow == workflow)
            .find_map(|(run, view)| view.approval.as_deref().map(|summary| (*run, summary)))
    }

    /// The newest run of `workflow` that has not ended.
    #[must_use]
    pub fn active_of(&self, workflow: &str) -> Option<i64> {
        self.runs
            .iter()
            .rev()
            .find(|(_, view)| view.workflow == workflow && view.is_active())
            .map(|(run, _)| *run)
    }

    /// The person answered the review (or closed it).
    pub fn review_answered(&mut self) {
        self.review = None;
    }

    /// A template was copied to `path`. The status says so until a run starts.
    pub fn created(&mut self, path: String) {
        self.fresh = Some(path.clone());
        self.created = Some(path);
    }

    /// The name the window shows for `workflow`: a built-in's title, else its name (`WF4c`).
    #[must_use]
    pub fn display_name(&self, workflow: &str) -> String {
        self.listed
            .as_deref()
            .and_then(|listed| listed.iter().find(|info| info.name == workflow))
            .and_then(|info| info.title.clone())
            .unwrap_or_else(|| workflow.to_owned())
    }

    /// Why the shortcut of the workflow `name` is not bound, when it is not.
    #[must_use]
    pub fn key_warning(&self, name: &str) -> Option<&str> {
        self.keys
            .warnings
            .iter()
            .find(|warning| warning.workflow == name)
            .map(|warning| warning.message.as_str())
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
            view.approval_plan = None;
        }
    }

    /// The run waits at an `approve` state; `summary` is what it asks, and `plan` is the restack
    /// the apply would move, when it is one (`WF4c`).
    pub fn approval_requested(
        &mut self,
        run: i64,
        state: String,
        summary: String,
        plan: Option<RestackPlan>,
    ) {
        let view = self.entry(run, String::new());
        view.status = WorkflowRunStatus::AwaitingApproval;
        view.state = Some(state);
        view.approval = Some(summary);
        view.approval_plan = plan;
    }

    /// The plan of the approval `run` waits for, when that approval is a restack's.
    #[must_use]
    pub fn approval_plan(&self, run: i64) -> Option<&RestackPlan> {
        self.runs.get(&run)?.approval_plan.as_ref()
    }

    /// A run of a workflow stopped on a restack conflict: the notice names it until the run ends.
    pub fn restack_conflicted(&mut self, run: i64, workflow: String, plan: RestackPlan) {
        self.restack_conflict = Some(RestackConflictNotice {
            run,
            workflow,
            plan,
        });
    }

    /// The person dismissed the notice.
    pub fn dismiss_restack_conflict(&mut self) {
        self.restack_conflict = None;
    }

    /// A `notify` step gave a line for the user.
    pub fn notice(&mut self, run: i64, text: String) {
        self.entry(run, String::new()).notice = Some(text);
    }

    /// The run ended. Its approval, if it waited for one, is gone, and so is its restack notice.
    pub fn finished(&mut self, run: i64, end: WorkflowEnd, reason: Option<String>) {
        if self
            .restack_conflict
            .as_ref()
            .is_some_and(|notice| notice.run == run)
        {
            self.restack_conflict = None;
        }
        let view = self.entry(run, String::new());
        view.status = match end {
            WorkflowEnd::Done => WorkflowRunStatus::Done,
            WorkflowEnd::Failed => WorkflowRunStatus::Failed,
            WorkflowEnd::Blocked => WorkflowRunStatus::Blocked,
            WorkflowEnd::Cancelled => WorkflowRunStatus::Cancelled,
        };
        view.approval = None;
        view.approval_plan = None;
        view.finished = Some((end, reason));
    }

    /// The view of `run`, made when it is new, with `workflow` as its name.
    /// Keeps only the newest runs.
    fn entry(&mut self, run: i64, workflow: String) -> &mut RunView {
        if !self.runs.contains_key(&run) {
            // A run that starts replaces the note about the template just created.
            self.created = None;
        }
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

    /// The one line the composer shows. An active run comes first; then the
    /// note about a template just created; then the first workflow that does not
    /// load; then the newest run.
    #[must_use]
    pub fn status_line(&self) -> Option<String> {
        if let Some(run) = self.active() {
            return self.run_line(run);
        }
        if let Some(path) = &self.created {
            return Some(format!("Created {path}; edit it, then Reload Workflows"));
        }
        if let Some(line) = self.broken_line() {
            return Some(line);
        }
        let run = self.runs.keys().next_back().copied()?;
        self.run_line(run)
    }

    fn run_line(&self, run: i64) -> Option<String> {
        let view = self.runs.get(&run)?;
        let head = status_text(view);
        Some(match &view.notice {
            Some(notice) => format!("{head} - {notice}"),
            None => head,
        })
    }

    /// The first listed workflow that does not load, with how many more do not.
    fn broken_line(&self) -> Option<String> {
        let listed = self.listed.as_deref()?;
        let mut broken = listed.iter().filter_map(|workflow| {
            workflow
                .error
                .as_deref()
                .map(|error| (&workflow.name, error))
        });
        let (name, error) = broken.next()?;
        let more = broken.count();
        Some(match more {
            0 => format!("{name} does not load: {error}"),
            _ => format!("{name} does not load: {error} (+{more} more)"),
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
            WorkflowEnd::Cancelled => "cancelled",
        };
        return match reason {
            Some(reason) => format!("{name}: {verb}: {reason}"),
            None => format!("{name}: {verb}"),
        };
    }
    // A restack's approval says what it moves; the Workflows page lists the branches (`WF4c`).
    if let Some(plan) = &view.approval_plan {
        return format!(
            "{name}: waiting for approval: {}",
            crate::restack_dialog::approval_headline(plan)
        );
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
        WorkflowRunStatus::Interrupted => format!("{name}: interrupted"),
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
    use gm_schema::{PlannedBranch, WorkflowRunStatus as S};

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
            source: String::new(),
            reviewed: false,
            auto_approve: false,
            enabled: false,
            title: None,
            location: gm_schema::WorkflowLocation::User,
            role: gm_schema::WorkflowRole::Workflow,
        }
    }

    fn started(workflows: &mut Workflows, run: i64, name: &str) {
        workflows.run_changed(run, name.into(), S::Running, Some("ask".into()));
    }

    #[test]
    fn the_listing_keeps_the_workflows_and_what_was_ignored() {
        let mut workflows = Workflows::default();
        assert_eq!(workflows.listed, None);
        workflows.listed(
            vec![info("a", "manual", None)],
            vec!["/x: no file".into()],
            Some("/home/u/.config/gitmaster/workflows".into()),
            vec!["commit-message".into()],
        );
        assert_eq!(workflows.listed.as_ref().map(Vec::len), Some(1));
        assert_eq!(workflows.ignored, ["/x: no file"]);
        assert_eq!(
            workflows.user_dir.as_deref(),
            Some("/home/u/.config/gitmaster/workflows")
        );
        assert_eq!(workflows.templates, ["commit-message"]);
    }

    #[test]
    fn a_key_the_plan_left_out_has_its_warning_on_its_workflow() {
        let workflows = Workflows {
            keys: crate::workflow_keys::plan(
                &[("flow".to_owned(), "secondary-s".to_owned())],
                &[("ctrl-s".to_owned(), "Save Workflow".to_owned())],
                false,
            ),
            ..Workflows::default()
        };
        assert_eq!(
            workflows.key_warning("flow"),
            Some("key secondary-s is already used by Save Workflow")
        );
        assert_eq!(workflows.key_warning("other"), None);
    }

    #[test]
    fn a_created_file_is_opened_once() {
        let mut workflows = Workflows::default();
        workflows.created("/w/new.lua".into());
        assert_eq!(workflows.fresh.as_deref(), Some("/w/new.lua"));
        workflows.fresh = None;
        assert_eq!(workflows.created.as_deref(), Some("/w/new.lua"));
    }

    #[test]
    fn a_broken_workflow_is_named_when_no_run_is_active() {
        let mut workflows = Workflows::default();
        workflows.listed(
            vec![
                info("ok", "manual", None),
                info("broken", "manual", Some("line 3: bad key")),
                info("worse", "manual", Some("line 1: nope")),
                info("worst", "manual", Some("x")),
            ],
            Vec::new(),
            None,
            Vec::new(),
        );
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("broken does not load: line 3: bad key (+2 more)")
        );
    }

    #[test]
    fn one_broken_workflow_has_no_more_suffix() {
        let mut workflows = Workflows::default();
        workflows.listed(
            vec![info("broken", "manual", Some("bad"))],
            Vec::new(),
            None,
            Vec::new(),
        );
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("broken does not load: bad")
        );
    }

    #[test]
    fn an_active_run_hides_the_broken_workflow_line() {
        let mut workflows = Workflows::default();
        workflows.listed(
            vec![info("broken", "manual", Some("bad"))],
            Vec::new(),
            None,
            Vec::new(),
        );
        started(&mut workflows, 1, "w");
        assert_eq!(workflows.status_line().as_deref(), Some("w: running ask"));
    }

    #[test]
    fn a_created_template_is_named_until_a_run_starts() {
        let mut workflows = Workflows::default();
        workflows.created("/home/u/workflows/commit.yml".into());
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("Created /home/u/workflows/commit.yml; edit it, then Reload Workflows")
        );
        started(&mut workflows, 1, "commit");
        assert_eq!(workflows.created, None);
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("commit: running ask")
        );
    }

    #[test]
    fn a_created_template_shows_before_a_broken_workflow() {
        let mut workflows = Workflows::default();
        workflows.listed(
            vec![info("broken", "manual", Some("bad"))],
            Vec::new(),
            None,
            Vec::new(),
        );
        workflows.created("/w/new.yml".into());
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("Created /w/new.yml; edit it, then Reload Workflows")
        );
    }

    #[test]
    fn a_finished_run_shows_once_nothing_is_broken() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "w");
        workflows.finished(1, WorkflowEnd::Done, None);
        assert_eq!(workflows.status_line().as_deref(), Some("w: done"));
        workflows.listed(Vec::new(), Vec::new(), None, Vec::new());
        assert_eq!(workflows.status_line().as_deref(), Some("w: done"));
    }

    #[test]
    fn a_reset_forgets_the_created_template() {
        let mut workflows = Workflows::default();
        workflows.created("/w/new.yml".into());
        workflows.reset();
        assert_eq!(workflows.created, None);
    }

    #[test]
    fn a_restack_approval_says_what_the_plan_moves_or_holds_back() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 4, "w");
        workflows.approval_requested(
            4,
            "restack.apply".into(),
            "Allow ctx.restack.apply?".into(),
            Some(conflict_plan("main", "feat", "other")),
        );
        assert_eq!(
            workflows.status_line().as_deref(),
            Some("w: waiting for approval: Moves 1 branch; holds back 1.")
        );
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
        workflows.approval_requested(3, "confirm".into(), "Create branch x?".into(), None);
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
        workflows.approval_requested(3, "confirm".into(), "Ok?".into(), None);
        workflows.run_changed(3, "w".into(), S::Running, Some("next".into()));
        assert_eq!(workflows.awaiting_approval(), None);
        assert_eq!(workflows.status_line().as_deref(), Some("w: running next"));
    }

    #[test]
    fn the_newest_run_waiting_for_approval_is_the_one_named() {
        let mut workflows = Workflows::default();
        started(&mut workflows, 1, "a");
        started(&mut workflows, 2, "b");
        workflows.approval_requested(1, "s".into(), "first?".into(), None);
        workflows.approval_requested(2, "s".into(), "second?".into(), None);
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
        workflows.listed(Vec::new(), Vec::new(), None, Vec::new());
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

    #[test]
    fn the_latest_request_of_a_workflow_is_the_one_kept() {
        let mut workflows = Workflows::default();
        workflows.wants_review("w".into(), "h1".into(), "commit.created".into());
        workflows.wants_review("w".into(), "h2".into(), "commit.created".into());
        assert_eq!(
            workflows.wanted.get("w"),
            Some(&WantedRun {
                hash: "h2".into(),
                trigger: "commit.created".into(),
            })
        );
        assert_eq!(workflows.wanted.len(), 1);
    }

    #[test]
    fn requests_are_kept_per_workflow() {
        let mut workflows = Workflows::default();
        workflows.wants_review("a".into(), "h".into(), "commit.created".into());
        workflows.wants_review("b".into(), "h".into(), "repo.opened".into());
        workflows.request_dropped("a");
        assert_eq!(workflows.wanted.keys().collect::<Vec<_>>(), ["b"]);
        workflows.request_dropped("missing");
        assert_eq!(workflows.wanted.len(), 1);
    }

    #[test]
    fn a_gate_link_belongs_to_its_own_operation() {
        let mut workflows = Workflows::default();
        workflows.gate_unreviewed(OperationId(7), "freeze".into(), "h".into());
        workflows.operation_changed(OperationId(7));
        assert!(workflows.gate_review.is_some());
        workflows.operation_changed(OperationId(8));
        assert_eq!(workflows.gate_review, None);
    }

    #[test]
    fn the_runs_of_one_workflow_are_found_by_name() {
        let mut workflows = Workflows::default();
        workflows.run_changed(1, "a".into(), S::Running, None);
        workflows.run_changed(2, "b".into(), S::Running, None);
        assert_eq!(workflows.active_of("a"), Some(1));
        assert_eq!(workflows.active_of("b"), Some(2));
        assert_eq!(workflows.active_of("c"), None);
        workflows.approval_requested(1, "ask".into(), "push".into(), None);
        assert_eq!(workflows.awaiting_approval_of("a"), Some((1, "push")));
        assert_eq!(workflows.awaiting_approval_of("b"), None);
    }

    #[test]
    fn a_run_waiting_for_approval_is_still_active_for_its_workflow() {
        let mut workflows = Workflows::default();
        workflows.run_changed(5, "a".into(), S::Running, None);
        workflows.approval_requested(5, "ask".into(), "create branch x".into(), None);
        assert_eq!(
            workflows.awaiting_approval_of("a"),
            Some((5, "create branch x"))
        );
        assert_eq!(workflows.active_of("a"), Some(5));
        workflows.finished(5, WorkflowEnd::Cancelled, None);
        assert_eq!(workflows.active_of("a"), None);
    }

    /// A plan of `root` in which `branch` stops on a conflict and `done` moves.
    fn conflict_plan(root: &str, branch: &str, done: &str) -> RestackPlan {
        let moved = PlannedBranch {
            name: done.into(),
            parent: root.into(),
            old_tip: "old".into(),
            old_base: None,
            onto: "onto".into(),
            commits: Vec::new(),
            new_tip: Some("new".into()),
            block: None,
        };
        let blocked = PlannedBranch {
            name: branch.into(),
            parent: root.into(),
            old_tip: "old".into(),
            old_base: None,
            onto: "onto".into(),
            commits: Vec::new(),
            new_tip: None,
            block: Some(RestackBlock::Conflict {
                commit: "c".into(),
                onto: "onto".into(),
            }),
        };
        RestackPlan {
            plan_id: "p".into(),
            from: root.into(),
            branches: vec![moved, blocked],
            stopped: Vec::new(),
        }
    }

    #[test]
    fn the_conflict_notice_names_the_workflow_and_the_blocked_branch_and_opens_on_the_root() {
        let mut workflows = Workflows::default();
        let plan = conflict_plan("main", "feat", "other");
        assert_eq!(conflict_branch(&plan), Some("feat"));
        workflows.restack_conflicted(4, "restack-children-on-parent-move".into(), plan);
        let notice = workflows.restack_conflict.as_ref().expect("a notice");
        assert_eq!(notice.branch(), Some("feat"));
        // Resolve conflict opens the restack dialog on the plan's root (D154).
        assert_eq!(notice.plan.from, "main");
        assert_eq!(
            restack_conflict_text(
                &workflows.display_name(&notice.workflow),
                notice.branch().unwrap_or_default()
            ),
            "restack-children-on-parent-move stopped on a restack conflict in feat"
        );
    }

    #[test]
    fn a_built_in_is_named_by_its_title_and_others_by_their_name() {
        let mut workflows = Workflows::default();
        let mut built_in = info(
            "restack-children-on-parent-move",
            "stack.parent_moved",
            None,
        );
        built_in.title = Some("Restack children when parent moves".into());
        workflows.listed(vec![built_in], Vec::new(), None, Vec::new());
        assert_eq!(
            workflows.display_name("restack-children-on-parent-move"),
            "Restack children when parent moves"
        );
        assert_eq!(workflows.display_name("own-flow"), "own-flow");
    }

    #[test]
    fn the_conflict_notice_is_dropped_when_its_run_ends_and_kept_for_another_run() {
        let mut workflows = Workflows::default();
        workflows.restack_conflicted(4, "w".into(), conflict_plan("main", "feat", "x"));
        workflows.finished(5, WorkflowEnd::Done, None);
        assert!(workflows.restack_conflict.is_some(), "another run ended");
        workflows.finished(4, WorkflowEnd::Done, None);
        assert_eq!(workflows.restack_conflict, None, "its run ended");
    }

    #[test]
    fn the_conflict_notice_can_be_dismissed() {
        let mut workflows = Workflows::default();
        workflows.restack_conflicted(4, "w".into(), conflict_plan("main", "feat", "x"));
        workflows.dismiss_restack_conflict();
        assert_eq!(workflows.restack_conflict, None);
    }

    #[test]
    fn a_plan_is_kept_while_its_approval_waits_and_leaves_with_it() {
        let mut workflows = Workflows::default();
        let plan = conflict_plan("main", "feat", "x");
        workflows.approval_requested(7, "apply".into(), "Allow".into(), Some(plan.clone()));
        assert_eq!(workflows.approval_plan(7), Some(&plan));
        workflows.run_changed(7, "w".into(), S::Running, None);
        assert_eq!(workflows.approval_plan(7), None, "the approval is answered");
        workflows.approval_requested(8, "apply".into(), "Allow".into(), None);
        assert_eq!(workflows.approval_plan(8), None, "an approval with no plan");
    }

    #[test]
    fn a_branch_is_blocked_on_a_conflict_only_for_a_conflict() {
        let plan = RestackPlan {
            plan_id: "p".into(),
            from: "main".into(),
            branches: Vec::new(),
            stopped: Vec::new(),
        };
        assert_eq!(conflict_branch(&plan), None);
    }
}
