//! The New Workspace dialog (D183 (3); docs/guide/workspaces.md, "Create a workspace"). The rules
//! are pure and tested here: the form and what it asks the engine, which row an answer belongs to,
//! the branch suggestions and when Create is enabled. The window side is the `impl StatusView` at
//! the bottom: the layer, the fields, and the plan the engine answers after each edit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gm_schema::{
    Actor, BranchHow, CommandKind, MemberPlan, MemberRequest, OperationId, OperationState,
    WorkspaceProblem,
};
use gpui::{
    App, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, PathPromptOptions,
    SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::text_search::DEBOUNCE_MS;
use crate::theme;
use crate::view::StatusView;
use crate::workspace_branch::{Choice, Known, RepoBranches, choices};
use crate::worktree_create::move_pick;
use crate::worktree_list::same_path;

const WIDTH: f32 = 560.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The width of the repository button of a row.
const REPO_WIDTH: f32 = 180.;

/// The name a repository row shows: the folder's name, or the whole path when it has none.
#[must_use]
pub(crate) fn repo_label(path: &str) -> String {
    Path::new(path.trim_end_matches('/'))
        .file_name()
        .map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
}

/// One row of the form: the repository folder chosen and the branch typed for it. Untrimmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Member {
    pub(crate) repo: String,
    pub(crate) branch: String,
}

/// The text of the form, as the engine is asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Form {
    pub(crate) name: String,
    pub(crate) members: Vec<Member>,
}

impl Default for Form {
    /// A form with no name and no member.
    fn default() -> Self {
        Self {
            name: String::new(),
            members: Vec::new(),
        }
    }
}

impl Form {
    /// Whether the form can be planned: a name, and a repository and a branch in every row.
    #[must_use]
    pub(crate) fn complete(&self) -> bool {
        !self.name.trim().is_empty()
            && !self.members.is_empty()
            && self
                .members
                .iter()
                .all(|member| !member.repo.trim().is_empty() && !member.branch.trim().is_empty())
    }

    /// The members as the engine asks for them, trimmed.
    #[must_use]
    pub(crate) fn requests(&self) -> Vec<MemberRequest> {
        self.members
            .iter()
            .map(|member| MemberRequest {
                repo: member.repo.trim().to_owned(),
                branch: member.branch.trim().to_owned(),
            })
            .collect()
    }

    /// The folder the workspace gets: `<root>/<name>`. Nothing is shown without a root or a name.
    #[must_use]
    pub(crate) fn folder(&self, root: Option<&str>) -> String {
        match root {
            Some(root) if !self.name.trim().is_empty() => {
                Path::new(root).join(self.name.trim()).display().to_string()
            }
            _ => String::new(),
        }
    }

    /// Fills the branch of every row with `branch`, as **Branch for all** does.
    pub(crate) fn set_branch_for_all(&mut self, branch: &str) {
        for member in &mut self.members {
            branch.clone_into(&mut member.branch);
        }
    }
}

/// The repositories the chooser offers, in order: the window's own, then the recent ones
/// without it. No path appears twice.
#[must_use]
pub(crate) fn repo_choices(window_root: Option<&str>, recents: &[PathBuf]) -> Vec<String> {
    let mut choices: Vec<String> = Vec::new();
    let recent = recents
        .iter()
        .map(|path| path.to_string_lossy().into_owned());
    for path in window_root.map(str::to_owned).into_iter().chain(recent) {
        if !choices.iter().any(|known| same_path(known, &path)) {
            choices.push(path);
        }
    }
    choices
}

/// What a branch of a row does, as the row says it: the engine's plan for it.
#[must_use]
pub(crate) fn describe(plan: &MemberPlan) -> String {
    match &plan.how {
        BranchHow::CheckOut => format!("checks out {}", plan.branch),
        BranchHow::TrackRemote { remote } => format!("tracks {remote}"),
        BranchHow::CreateFrom { base } => format!("creates {} from {base}", plan.branch),
    }
}

/// The engine's answer to the plan of one form: the folder, each member's plan and the problems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) path: String,
    pub(crate) members: Vec<MemberPlan>,
    pub(crate) problems: Vec<WorkspaceProblem>,
}

/// What one row shows once the answer arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The row's branch, and what will happen to it.
    Plan(MemberPlan),
    /// The problem that refuses the row.
    Problem(WorkspaceProblem),
}

/// The outcome of each row, in order, and the problems no row claims, which the dialog lists
/// under the rows. A problem belongs to the row whose repository it names; a plan belongs to the
/// first row, in order, that asks for its branch. The engine answers every row with one plan or
/// one problem, in the order of the rows, so this matches them.
#[must_use]
pub(crate) fn outcomes(
    members: &[Member],
    answer: &Answer,
) -> (Vec<Option<Outcome>>, Vec<WorkspaceProblem>) {
    let mut claimed = vec![false; answer.problems.len()];
    let mut used = vec![false; answer.members.len()];
    let mut rows = Vec::with_capacity(members.len());
    for member in members {
        let repo = member.repo.trim();
        let problem = answer
            .problems
            .iter()
            .enumerate()
            .position(|(at, problem)| !claimed[at] && problem.repo.as_deref() == Some(repo));
        if let Some(at) = problem {
            claimed[at] = true;
            rows.push(Some(Outcome::Problem(answer.problems[at].clone())));
            continue;
        }
        let branch = member.branch.trim();
        let plan = answer
            .members
            .iter()
            .enumerate()
            .position(|(at, plan)| !used[at] && plan.branch == branch);
        match plan {
            Some(at) => {
                used[at] = true;
                rows.push(Some(Outcome::Plan(answer.members[at].clone())));
            }
            None => rows.push(None),
        }
    }
    let rest = answer
        .problems
        .iter()
        .enumerate()
        .filter(|(at, _)| !claimed[*at])
        .map(|(_, problem)| problem.clone())
        .collect();
    (rows, rest)
}

/// The plan of the form on screen: the edit it answers, the plan in flight and the answer. Only
/// the newest edit's plan is sent, and only the plan in flight is applied, so an answer to an
/// older form never shows (D183 (3)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Planning {
    /// Counts the edits of the form.
    edits: u64,
    /// The plan in flight, if any.
    pending: Option<OperationId>,
    answer: Option<Answer>,
    /// Why the plan failed, such as an invalid name, until the next edit.
    failure: Option<String>,
}

impl Planning {
    /// The form changed: the answer no longer describes it. Returns the number of this edit.
    pub(crate) fn edited(&mut self) -> u64 {
        self.edits += 1;
        self.pending = None;
        self.answer = None;
        self.failure = None;
        self.edits
    }

    /// Whether `edit` is still the newest edit, so its plan should be sent.
    #[must_use]
    pub(crate) fn is_latest(&self, edit: u64) -> bool {
        self.edits == edit
    }

    /// Starts the plan that `id` answers.
    pub(crate) fn sent(&mut self, id: OperationId) {
        self.pending = Some(id);
    }

    /// Whether `id` is the plan in flight.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Keeps the answer to the plan `id`, when it is the one in flight.
    pub(crate) fn answered(&mut self, id: OperationId, answer: Answer) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.answer = Some(answer);
        true
    }

    /// Ends the plan `id` with a terminal `state`. A plan that did not finish keeps its reason.
    pub(crate) fn settled(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<String>,
    ) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        if state != OperationState::Done {
            self.failure = Some(detail.unwrap_or_else(|| "The plan could not be made.".to_owned()));
        }
        true
    }

    /// Whether a plan is in flight.
    #[must_use]
    pub(crate) fn is_planning(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub(crate) fn answer(&self) -> Option<&Answer> {
        self.answer.as_ref()
    }

    #[must_use]
    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Whether the answer is for the form as it is, and no member is refused.
    #[must_use]
    pub(crate) fn ready(&self) -> bool {
        self.pending.is_none()
            && self
                .answer
                .as_ref()
                .is_some_and(|answer| answer.problems.is_empty())
    }
}

/// The create in flight, and its outcome until the window takes it. Engine events for other
/// operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Making {
    pending: Option<OperationId>,
    /// The folder the engine reported as created, before the operation ends.
    made: Option<String>,
    outcome: Option<Result<String, String>>,
}

impl Making {
    pub(crate) fn start(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.made = None;
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// The engine created the folder `path` for the create `id`.
    pub(crate) fn created(&mut self, id: OperationId, path: String) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.made = Some(path);
        true
    }

    /// Ends the create with the operation's terminal `state`. A success gives the folder; anything
    /// else gives the engine's reason, or a plain one.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() || self.pending.take().is_none() {
            return;
        }
        self.outcome = Some(match (state, self.made.take()) {
            (OperationState::Done, Some(path)) => Ok(path),
            (_, _) => Err(detail.unwrap_or_else(|| "The workspace was not created.".to_owned())),
        });
    }

    pub(crate) fn take(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// Whether Create Workspace is enabled: the form is complete, its plan is the answer for it with
/// no problem, and no create is in flight.
/// What Escape does in the dialog: an open repository list or branch list closes first and the
/// dialog stays, so a second Escape closes the dialog. Returns whether the dialog closes.
pub(crate) fn escape_closes_dialog(
    choosing: &mut Option<usize>,
    picking: &mut Option<usize>,
) -> bool {
    if choosing.take().is_some() {
        return false;
    }
    picking.take().is_none()
}

pub(crate) fn may_create(form: &Form, planning: &Planning, making: &Making) -> bool {
    form.complete() && planning.ready() && !making.is_pending()
}

/// One row of the dialog: its repository folder and the branch chosen for it. The branch is
/// picked from the repository's list, or typed as a new name in the list's filter.
/// The branch list open under a row, as the window shows it: the filter field, what the filter
/// leaves, why there may be nothing to choose yet, and the entry Enter takes.
#[derive(Clone)]
pub(crate) struct BranchList {
    pub(crate) row: usize,
    pub(crate) filter: Entity<TextArea>,
    pub(crate) choices: Vec<Choice>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) pick: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemberRow {
    pub(crate) repo: String,
    pub(crate) branch: String,
}

/// The dialog while it is open: its fields, the rows, the open choosers and the error it shows.
pub(crate) struct WorkspaceDialog {
    pub(crate) name: Entity<TextArea>,
    pub(crate) branch_all: Entity<TextArea>,
    /// The filter of the open branch list. One field serves whichever row's list is open.
    pub(crate) filter: Entity<TextArea>,
    pub(crate) rows: Vec<MemberRow>,
    /// The row whose repository list is open.
    pub(crate) choosing: Option<usize>,
    /// The row whose branch list is open.
    pub(crate) picking: Option<usize>,
    /// The entry Up and Down highlight in the open branch list; `None` until one is moved to, and
    /// then the first entry is the one Enter takes.
    pub(crate) pick: Option<usize>,
    pub(crate) error: Option<String>,
    /// The form, the **Branch for all** text and the filter the observers last saw. A field
    /// notifies on every caret blink, so a change counts only when these differ.
    seen: Form,
    seen_all: String,
    seen_filter: String,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceDialog {
    pub(crate) fn form(&self, cx: &App) -> Form {
        Form {
            name: self.name.read(cx).text().to_owned(),
            members: self
                .rows
                .iter()
                .map(|row| Member {
                    repo: row.repo.clone(),
                    branch: row.branch.clone(),
                })
                .collect(),
        }
    }

    /// The fields in the order Tab visits them: the name, Branch for all, then the filter of an
    /// open branch list.
    fn fields(&self) -> Vec<&Entity<TextArea>> {
        let mut fields = vec![&self.name, &self.branch_all];
        if self.picking.is_some() {
            fields.push(&self.filter);
        }
        fields
    }
}

impl StatusView {
    /// Opens the dialog. The first row starts with this window's repository and its branch, when
    /// it has them, and the plan follows. The repository's branches are asked for at once.
    pub(crate) fn open_workspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_dialog.is_some() {
            return;
        }
        let window_root = self.model.status().map(|status| status.root.clone());
        let head = self
            .model
            .status()
            .and_then(|status| status.branch.head.clone())
            .unwrap_or_default();
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextArea::new(placeholder, cx).with_lines(1, 1))
        };
        let name = field("Workspace name", cx);
        let branch_all = field("Branch for all", cx);
        let filter = field("Filter branches", cx);
        let mut subscriptions = Vec::new();
        for area in [&name, &branch_all] {
            subscriptions.push(cx.observe(area, |this, _, cx| this.on_workspace_field(cx)));
            subscriptions.push(cx.subscribe_in(
                area,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.cancel_workspace_dialog(window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.observe(&filter, |this, _, cx| this.on_branch_filter(cx)));
        subscriptions.push(cx.subscribe_in(
            &filter,
            window,
            |this, _, event: &TextAreaEvent, window, cx| {
                if *event == TextAreaEvent::Cancel {
                    this.cancel_workspace_dialog(window, cx);
                }
            },
        ));
        let focus = name.clone();
        let first_repo = window_root.unwrap_or_default();
        let first = MemberRow {
            repo: first_repo.clone(),
            branch: head,
        };
        let mut dialog = WorkspaceDialog {
            name,
            branch_all,
            filter,
            rows: vec![first],
            choosing: None,
            picking: None,
            pick: None,
            error: None,
            seen: Form::default(),
            seen_all: String::new(),
            seen_filter: String::new(),
            _subscriptions: subscriptions,
        };
        dialog.seen = dialog.form(cx);
        self.workspace_dialog = Some(dialog);
        self.model.workspace_plan = Planning::default();
        self.model.workspace_branches = RepoBranches::default();
        self.request_workspaces();
        self.request_branches(&first_repo, cx);
        focus.update(cx, |area, cx| area.focus(window, cx));
        self.plan_soon(cx);
    }

    pub(crate) fn close_workspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Escape in a field: closes the repository list when one is open and keeps the dialog and its
    /// input; with no list open it closes the dialog.
    fn cancel_workspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        if escape_closes_dialog(&mut dialog.choosing, &mut dialog.picking) {
            self.close_workspace_dialog(window, cx);
        } else {
            cx.notify();
        }
    }

    /// A field changed: the form is compared with the one last seen. **Branch for all** fills
    /// every row first. A change to the form clears the old answer and asks for a new plan.
    fn on_workspace_field(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        let all = dialog.branch_all.read(cx).text().to_owned();
        if all != dialog.seen_all {
            dialog.seen_all.clone_from(&all);
            let mut filled = dialog.form(cx);
            filled.set_branch_for_all(&all);
            for (row, member) in dialog.rows.iter_mut().zip(filled.members) {
                row.branch = member.branch;
            }
        }
        let form = dialog.form(cx);
        if dialog.seen == form {
            return;
        }
        dialog.seen = form;
        dialog.pick = None;
        dialog.error = None;
        self.plan_soon(cx);
    }

    /// The filter of the open branch list changed: its highlight starts again at the first entry.
    /// A caret blink notifies too, so only a change of the text counts.
    fn on_branch_filter(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        let text = dialog.filter.read(cx).text().to_owned();
        if text != dialog.seen_filter {
            dialog.seen_filter = text;
            dialog.pick = None;
            cx.notify();
        }
    }

    /// Asks for a plan after the edits settle: the plan of the newest edit is sent once the
    /// debounce has passed, and an older one is dropped.
    fn plan_soon(&mut self, cx: &mut Context<Self>) {
        let edit = self.model.workspace_plan.edited();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(DEBOUNCE_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.model.workspace_plan.is_latest(edit) {
                    view.send_workspace_plan(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Sends the plan of the form as it is, when the form is complete.
    pub(crate) fn send_workspace_plan(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_ref() else {
            return;
        };
        let form = dialog.form(cx);
        if !form.complete() {
            return;
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::PlanWorkspace {
                name: form.name.trim().to_owned(),
                members: form.requests(),
                base: None,
                root: None,
            },
        );
        self.model.workspace_plan.sent(id);
        cx.notify();
    }

    /// The repository list of row `index` opens or closes. Only one list is open at a time.
    pub(crate) fn toggle_repo_chooser(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            dialog.choosing = if dialog.choosing == Some(index) {
                None
            } else {
                Some(index)
            };
            dialog.picking = None;
        }
        cx.notify();
    }

    /// Row `index` takes `repo`, and its list closes. The repository's branches are asked for.
    pub(crate) fn choose_repo(&mut self, index: usize, repo: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            if let Some(row) = dialog.rows.get_mut(index) {
                repo.clone_into(&mut row.repo);
            }
            dialog.choosing = None;
            dialog.error = None;
        }
        self.request_branches(repo, cx);
        self.on_workspace_field(cx);
    }

    /// The branch list of row `index` opens or closes. It asks for the repository's branches when
    /// they are not known, and takes the focus for its filter.
    pub(crate) fn toggle_branch_list(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        if dialog.picking == Some(index) {
            dialog.picking = None;
            cx.notify();
            return;
        }
        dialog.picking = Some(index);
        dialog.choosing = None;
        dialog.pick = None;
        let repo = dialog
            .rows
            .get(index)
            .map(|row| row.repo.clone())
            .unwrap_or_default();
        let filter = dialog.filter.clone();
        filter.update(cx, |area, cx| area.set_text("", cx));
        self.request_branches(&repo, cx);
        filter.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    /// Asks the engine for the branches of `repo`, unless they are known or on their way. The
    /// answer is kept by the model, which drops it when its request is no longer the one waited for.
    pub(crate) fn request_branches(&mut self, repo: &str, cx: &mut Context<Self>) {
        let repo = repo.trim();
        if repo.is_empty() || !self.model.workspace_branches.wants(repo) {
            return;
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ListRepoBranches {
                repo: repo.to_owned(),
            },
        );
        self.model.workspace_branches.request(repo, id);
        cx.notify();
    }

    /// + Add repository: a new row, with the **Branch for all** text as its branch.
    pub(crate) fn add_repo_row(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return;
        };
        let branch = dialog.branch_all.read(cx).text().to_owned();
        dialog.rows.push(MemberRow {
            repo: String::new(),
            branch,
        });
        self.on_workspace_field(cx);
    }

    /// The remove button of row `index`. Both lists close: their rows may have moved.
    pub(crate) fn remove_repo_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            if index < dialog.rows.len() {
                dialog.rows.remove(index);
            }
            dialog.choosing = None;
            dialog.picking = None;
            dialog.pick = None;
        }
        self.on_workspace_field(cx);
    }

    /// The branch list open under a row, as the window reads it now. `None` when no list is open.
    pub(crate) fn workspace_branch_list(&self, cx: &App) -> Option<BranchList> {
        let dialog = self.workspace_dialog.as_ref()?;
        let row = dialog.picking?;
        let member = dialog.rows.get(row)?;
        let filter = dialog.filter.read(cx).text().to_owned();
        let (known, notice) = if member.repo.trim().is_empty() {
            (Known::Loading, Some("Choose a repository first."))
        } else {
            let known = self.model.workspace_branches.known(&member.repo);
            let notice = match known {
                Known::Loading => Some("Loading branches…"),
                Known::Failed => {
                    Some("Could not read this repository's branches. A new name still works.")
                }
                Known::Listed(_) => None,
            };
            (known, notice)
        };
        Some(BranchList {
            row,
            filter: dialog.filter.clone(),
            choices: choices(&filter, known),
            notice,
            pick: dialog.pick,
        })
    }

    /// Up or Down with a branch list open: moves the highlight. Returns whether it did, so the
    /// keys go on to the field when it did not.
    pub(crate) fn move_workspace_pick(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let count = match self.workspace_branch_list(cx) {
            Some(list) => list.choices.len(),
            None => return false,
        };
        if count == 0 {
            return false;
        }
        let Some(dialog) = self.workspace_dialog.as_mut() else {
            return false;
        };
        dialog.pick = move_pick(dialog.pick, count, down);
        cx.notify();
        true
    }

    /// Enter: takes the highlighted entry of an open branch list, or else sends the create. While
    /// a list is open Enter never sends the create, so a half-read list cannot create by accident.
    fn workspace_enter(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.workspace_branch_list(cx) else {
            self.submit_workspace(cx);
            return;
        };
        if let Some(choice) = list.choices.get(list.pick.unwrap_or(0)) {
            let value = choice.fills().to_owned();
            self.choose_workspace_branch(list.row, value, cx);
        }
    }

    /// Puts `branch` in row `index` and closes its list. The observer then asks for a plan.
    pub(crate) fn choose_workspace_branch(
        &mut self,
        index: usize,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            if let Some(row) = dialog.rows.get_mut(index) {
                row.branch = branch;
            }
            dialog.picking = None;
            dialog.pick = None;
            dialog.error = None;
        }
        self.on_workspace_field(cx);
    }

    /// Create Workspace and Enter: sends the create when the rules allow it.
    pub(crate) fn submit_workspace(&mut self, cx: &mut Context<Self>) {
        if self.model.workspace_making.is_pending() {
            return;
        }
        let Some(dialog) = self.workspace_dialog.as_ref() else {
            return;
        };
        let form = dialog.form(cx);
        if !may_create(
            &form,
            &self.model.workspace_plan,
            &self.model.workspace_making,
        ) {
            return;
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::CreateWorkspace {
                name: form.name.trim().to_owned(),
                members: form.requests(),
                base: None,
                root: None,
            },
        );
        self.model.workspace_making.start(id);
        if let Some(dialog) = self.workspace_dialog.as_mut() {
            dialog.error = None;
        }
        cx.notify();
    }

    /// Runs when the engine settles a create: a success closes the dialog, refreshes the list and
    /// opens the new workspace in its own window; a failure stays in the dialog with the reason.
    pub(crate) fn on_workspace_created(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_workspaces();
        match outcome {
            Ok(path) => {
                self.workspace_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&format!("Created workspace at {path}"), cx);
                self.open_worktree(&path, cx);
            }
            Err(message) => match self.workspace_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    /// Closes the dialog and opens `path` in its own window: **Go to that worktree**.
    fn open_from_workspace_dialog(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_workspace_dialog(window, cx);
        self.open_worktree(path, cx);
    }

    fn next_workspace_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.workspace_dialog.as_ref() else {
            return;
        };
        let fields = dialog.fields();
        let current = fields
            .iter()
            .position(|area| area.read(cx).is_focused(window))
            .unwrap_or(fields.len() - 1);
        let next = fields[(current + 1) % fields.len()].clone();
        next.update(cx, |area, cx| area.focus(window, cx));
    }

    pub(crate) fn workspace_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.workspace_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_workspace_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = self.workspace_panel(dialog, left, cx);
        let view = cx.entity().downgrade();
        let tab = view.clone();
        let up = view.clone();
        let down = view.clone();
        let enter = view;
        div().absolute().inset_0().child(outside).child(
            panel
                .capture_action(move |_: &FocusNext, window, cx| {
                    let _ = tab.update(cx, |this, cx| this.next_workspace_field(window, cx));
                    cx.stop_propagation();
                })
                // Up and Down are kept only while suggestions are on screen.
                .capture_action(move |_: &Up, _, cx| {
                    if matches!(
                        up.update(cx, |this, cx| this.move_workspace_pick(false, cx)),
                        Ok(true)
                    ) {
                        cx.stop_propagation();
                    }
                })
                .capture_action(move |_: &Down, _, cx| {
                    if matches!(
                        down.update(cx, |this, cx| this.move_workspace_pick(true, cx)),
                        Ok(true)
                    ) {
                        cx.stop_propagation();
                    }
                })
                .capture_action(move |_: &Newline, _, cx| {
                    let _ = enter.update(cx, StatusView::workspace_enter);
                    cx.stop_propagation();
                }),
        )
    }

    fn workspace_panel(&self, dialog: &WorkspaceDialog, left: f32, cx: &Context<Self>) -> Div {
        div()
            .absolute()
            .top(px(TOP))
            .left(px(left))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("New workspace"),
            )
            .child(focus_field_row("NAME", dialog.name.clone(), cx))
            .child(self.workspace_members(dialog, cx))
            .child(focus_field_row(
                "BRANCH FOR ALL",
                dialog.branch_all.clone(),
                cx,
            ))
            .child(folder_line(&dialog.form(cx), self.model.workspaces.root()))
            .children(self.workspace_problems(dialog, cx))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(
                self.model.workspace_making.is_pending(),
                may_create(
                    &dialog.form(cx),
                    &self.model.workspace_plan,
                    &self.model.workspace_making,
                ),
                cx,
            ))
    }

    /// The MEMBERS block: one row per member, then + Add repository.
    fn workspace_members(&self, dialog: &WorkspaceDialog, cx: &Context<Self>) -> Div {
        let form = dialog.form(cx);
        let planning = &self.model.workspace_plan;
        let rows = match planning.answer() {
            Some(answer) => outcomes(&form.members, answer).0,
            None => vec![None; dialog.rows.len()],
        };
        let window_root = self.model.status().map(|status| status.root.clone());
        let choices = repo_choices(window_root.as_deref(), self.recents.paths());
        let open = self.workspace_branch_list(cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().text_color(theme::muted()).child("MEMBERS"))
            .children(dialog.rows.iter().enumerate().map(|(index, row)| {
                member_row(
                    RowView {
                        index,
                        row,
                        outcome: rows.get(index).and_then(Option::as_ref),
                        planning: planning.is_planning(),
                        choosing: dialog.choosing == Some(index),
                        choices: &choices,
                        branches: open.as_ref().filter(|list| list.row == index),
                    },
                    cx,
                )
            }))
            .child(add_repo_button(cx))
    }

    /// The problems no row claims, and the plan's failure: what keeps Create workspace off.
    fn workspace_problems(&self, dialog: &WorkspaceDialog, cx: &Context<Self>) -> Vec<Div> {
        let form = dialog.form(cx);
        let unclaimed = self
            .model
            .workspace_plan
            .answer()
            .map(|answer| outcomes(&form.members, answer).1)
            .unwrap_or_default();
        unclaimed
            .iter()
            .map(|problem| problem_line(problem, cx))
            .chain(self.model.workspace_plan.failure().map(|failure| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(failure.to_owned())
            }))
            .collect()
    }
}

/// A labelled field of the dialog, as `field_row`, but a click anywhere in its box takes the focus,
/// as a click on a row's branch field does. The padding of the box is not the field's own area, so
/// a click there would otherwise leave the focus where it was.
fn focus_field_row(label: &'static str, field: Entity<TextArea>, cx: &Context<StatusView>) -> Div {
    let target = field.clone();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child(label))
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::hairline())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _: &MouseDownEvent, window, cx| {
                        target.update(cx, |area, cx| area.focus(window, cx));
                    }),
                )
                .child(field),
        )
}

/// The FOLDER line: `<root>/<name>`, or a hint until the name is typed.
fn folder_line(form: &Form, root: Option<&str>) -> Div {
    let folder = form.folder(root);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child("FOLDER"))
        .child(
            div()
                .w_full()
                .min_w(px(0.))
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::text())
                .child(if folder.is_empty() {
                    "Name the workspace to see its folder.".to_owned()
                } else {
                    folder
                }),
        )
}

/// Choose Folder…: asks for a folder, then gives it to row `index`. The window does not wait for
/// the dialog; the answer comes back later.
fn pick_repo_folder(index: usize, cx: &mut Context<StatusView>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |this, cx| match picked.await {
        Ok(Ok(Some(paths))) => {
            if let Some(path) = paths.into_iter().next() {
                let text = path.display().to_string();
                let _ = this.update(cx, |view, cx| view.choose_repo(index, &text, cx));
            }
        }
        // Cancelled: nothing to do.
        Ok(Ok(None)) | Err(_) => {}
        Ok(Err(error)) => {
            let text = format!("Could not open the folder prompt: {error}");
            let _ = this.update(cx, |view, cx| view.notify_user(&text, cx));
        }
    })
    .detach();
}

/// What one row of the dialog shows: its place, its branch, the plan or problem of its branch,
/// and the pickers that are open for it.
#[derive(Clone, Copy)]
struct RowView<'a> {
    index: usize,
    row: &'a MemberRow,
    outcome: Option<&'a Outcome>,
    planning: bool,
    choosing: bool,
    choices: &'a [String],
    branches: Option<&'a BranchList>,
}

/// One row: the repository button, the branch button, the remove button, the chooser or the branch
/// list when they are open, and what the row will do or why it cannot.
fn member_row(view: RowView<'_>, cx: &Context<StatusView>) -> Div {
    let RowView {
        index,
        row,
        outcome,
        planning,
        choosing,
        choices,
        branches,
    } = view;
    let repo_text = if row.repo.is_empty() {
        "Choose a repository".to_owned()
    } else {
        repo_label(&row.repo)
    };
    let remove = SharedString::from(format!("workspace-row-remove-{index}"));
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id(SharedString::from(format!("workspace-row-repo-{index}")))
                        .w(px(REPO_WIDTH))
                        .flex_none()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::hairline())
                        .cursor_pointer()
                        .text_sm()
                        .text_color(if row.repo.is_empty() {
                            theme::muted()
                        } else {
                            theme::text()
                        })
                        .child(repo_text)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.toggle_repo_chooser(index, cx);
                            }),
                        ),
                )
                .child(branch_button(index, row, branches.is_some(), cx))
                .child(
                    div()
                        .id(remove)
                        .flex_none()
                        .px_2()
                        .cursor_pointer()
                        .text_color(theme::muted())
                        .child("×")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.remove_repo_row(index, cx);
                            }),
                        ),
                ),
        )
        .children(choosing.then(|| chooser(index, choices, cx)))
        .children(branches.map(|list| branch_list_view(index, list, cx)))
        .children(outcome_line(index, outcome, planning, cx))
}

/// The branch button of row `index`: the chosen branch, or "Choose a branch". A click opens or
/// closes the row's branch list.
fn branch_button(
    index: usize,
    row: &MemberRow,
    open: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let chosen = !row.branch.trim().is_empty();
    div()
        .id(SharedString::from(format!("workspace-row-branch-{index}")))
        .flex_1()
        .min_w(px(0.))
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if open {
            theme::accent()
        } else {
            theme::hairline()
        })
        .cursor_pointer()
        .text_sm()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(if chosen {
            theme::text()
        } else {
            theme::muted()
        })
        .child(if chosen {
            row.branch.clone()
        } else {
            "Choose a branch".to_owned()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.toggle_branch_list(index, window, cx);
            }),
        )
}

/// The repositories a row can take, then Choose Folder….
fn chooser(index: usize, choices: &[String], cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline())
        .children(choices.iter().enumerate().map(|(at, path)| {
            let repo = path.clone();
            div()
                .id(SharedString::from(format!("workspace-choice-{index}-{at}")))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .cursor_pointer()
                .text_color(theme::text())
                .hover(|style| style.bg(theme::selected_row()))
                .child(div().flex_none().child(repo_label(path)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(path.clone()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.choose_repo(index, &repo, cx);
                    }),
                )
        }))
        .child(
            div()
                .id(SharedString::from(format!(
                    "workspace-choice-folder-{index}"
                )))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .text_sm()
                .cursor_pointer()
                .text_color(theme::accent())
                .hover(|style| style.bg(theme::selected_row()))
                .child("Choose Folder…")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                        pick_repo_folder(index, cx);
                    }),
                ),
        )
}

/// The branch list of row `index`: its filter, the notice while it loads or fails, and the
/// choices with the entry Enter takes highlighted.
fn branch_list_view(index: usize, list: &BranchList, cx: &Context<StatusView>) -> Div {
    let target = list.filter.clone();
    let highlighted = list.pick.unwrap_or(0);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme::hairline())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _: &MouseDownEvent, window, cx| {
                        target.update(cx, |area, cx| area.focus(window, cx));
                    }),
                )
                .child(list.filter.clone()),
        )
        .children(
            list.notice
                .map(|notice| div().text_xs().text_color(theme::muted()).child(notice)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(theme::hairline())
                .children(
                    list.choices.iter().enumerate().map(|(at, choice)| {
                        branch_choice(index, at, choice, at == highlighted, cx)
                    }),
                ),
        )
}

/// One choice of a branch list: a branch, a remote-only branch marked as such, or the new name.
fn branch_choice(
    index: usize,
    at: usize,
    choice: &Choice,
    highlighted: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let (label, marker) = match choice {
        Choice::Branch {
            name,
            remote_only: false,
        } => (name.clone(), None),
        Choice::Branch {
            name,
            remote_only: true,
        } => (name.clone(), Some("remote only")),
        Choice::Create(name) => (format!("Create branch '{name}'"), None),
    };
    let value = choice.fills().to_owned();
    div()
        .id(SharedString::from(format!("workspace-branch-{index}-{at}")))
        .h(px(24.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .cursor_pointer()
        .text_color(theme::text())
        .when(highlighted, |this| {
            this.bg(theme::selection()).text_color(theme::accent())
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
        .children(marker.map(|text| {
            div()
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child(text)
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.choose_workspace_branch(index, value.clone(), cx);
            }),
        )
}

/// What a row will do with its branch, or why it cannot. While the plan is in flight the row says so.
fn outcome_line(
    index: usize,
    outcome: Option<&Outcome>,
    planning: bool,
    cx: &Context<StatusView>,
) -> Option<Div> {
    let line = div().flex().flex_row().items_start().gap_2().text_xs();
    match outcome {
        Some(Outcome::Plan(plan)) => Some(line.text_color(theme::muted()).child(describe(plan))),
        Some(Outcome::Problem(problem)) => Some(
            line.text_color(theme::error())
                .child(div().flex_1().min_w(px(0.)).child(problem.message.clone()))
                .children(
                    problem
                        .path
                        .as_deref()
                        .map(|path| go_button(index, path, cx)),
                ),
        ),
        None => planning.then(|| line.text_color(theme::muted()).child("Checking…")),
    }
}

/// A problem no row claims, with its **Go to that worktree** when it names one.
fn problem_line(problem: &WorkspaceProblem, cx: &Context<StatusView>) -> Div {
    let line = div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .text_sm()
        .text_color(theme::error())
        .child(div().flex_1().min_w(px(0.)).child(problem.message.clone()));
    match problem.path.as_deref() {
        Some(path) => line.child(go_to_button(path, cx)),
        None => line,
    }
}

/// **Go to that worktree**, for a row's problem.
fn go_button(index: usize, path: &str, cx: &Context<StatusView>) -> Stateful<Div> {
    let target = path.to_owned();
    div()
        .id(SharedString::from(format!("workspace-go-{index}")))
        .flex_none()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .border_1()
        .border_color(theme::hairline_strong())
        .text_color(theme::text())
        .child("Go to that worktree")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_from_workspace_dialog(&target, window, cx);
            }),
        )
}

/// **Go to that worktree**, for a problem no row claims.
fn go_to_button(path: &str, cx: &Context<StatusView>) -> Stateful<Div> {
    let id = SharedString::from(format!("workspace-go-to-{path}"));
    let target = path.to_owned();
    div()
        .id(id)
        .flex_none()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .border_1()
        .border_color(theme::hairline_strong())
        .text_color(theme::text())
        .child("Go to that worktree")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_from_workspace_dialog(&target, window, cx);
            }),
        )
}

/// + Add repository.
fn add_repo_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("workspace-add-repo")
        .text_sm()
        .text_color(theme::accent())
        .cursor_pointer()
        .child("+ Add repository")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.add_repo_row(cx);
            }),
        )
}

/// Cancel and Create workspace. The button stays disabled until the rules allow the create.
fn buttons(pending: bool, allowed: bool, cx: &Context<StatusView>) -> Div {
    let create = div()
        .id("workspace-create")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!allowed || pending, |this| this.opacity(0.5))
        .when(allowed && !pending, gpui::Styled::cursor_pointer)
        .child(if pending {
            "Creating…"
        } else {
            "Create workspace"
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_workspace(cx);
            }),
        );
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("workspace-cancel")
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::text())
                .child("Cancel")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_workspace_dialog(window, cx);
                    }),
                ),
        )
        .child(create)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::WorkspaceProblem;

    fn member(repo: &str, branch: &str) -> Member {
        Member {
            repo: repo.to_owned(),
            branch: branch.to_owned(),
        }
    }

    fn form(name: &str, members: Vec<Member>) -> Form {
        Form {
            name: name.to_owned(),
            members,
        }
    }

    fn plan(repo: &str, member: &str, branch: &str, how: BranchHow) -> MemberPlan {
        MemberPlan {
            repo: repo.to_owned(),
            member: member.to_owned(),
            branch: branch.to_owned(),
            how,
        }
    }

    fn problem(repo: Option<&str>, message: &str, path: Option<&str>) -> WorkspaceProblem {
        WorkspaceProblem {
            repo: repo.map(str::to_owned),
            message: message.to_owned(),
            path: path.map(str::to_owned),
        }
    }

    fn answer(members: Vec<MemberPlan>, problems: Vec<WorkspaceProblem>) -> Answer {
        Answer {
            path: "/w/checkout".to_owned(),
            members,
            problems,
        }
    }

    #[test]
    fn escape_closes_an_open_list_first_and_the_dialog_on_the_next() {
        let mut choosing = Some(1);
        let mut picking = None;
        assert!(
            !escape_closes_dialog(&mut choosing, &mut picking),
            "the repository list closes, not the dialog"
        );
        assert_eq!(choosing, None);
        let mut picking = Some(0);
        assert!(
            !escape_closes_dialog(&mut choosing, &mut picking),
            "the branch list closes, not the dialog"
        );
        assert_eq!(picking, None);
        assert!(
            escape_closes_dialog(&mut choosing, &mut picking),
            "a second Escape closes the dialog"
        );
    }

    #[test]
    fn a_form_needs_a_name_and_a_repository_and_a_branch_in_every_row() {
        assert!(form("checkout", vec![member("/r", "feat")]).complete());
        assert!(!form("  ", vec![member("/r", "feat")]).complete());
        assert!(!form("checkout", Vec::new()).complete());
        assert!(!form("checkout", vec![member("", "feat")]).complete());
        assert!(!form("checkout", vec![member("/r", "  ")]).complete());
    }

    #[test]
    fn the_requests_are_trimmed() {
        let requests = form(" checkout ", vec![member(" /r ", " feat ")]).requests();
        assert_eq!(requests[0].repo, "/r");
        assert_eq!(requests[0].branch, "feat");
    }

    #[test]
    fn the_folder_is_root_and_name_and_nothing_without_either() {
        let named = form("checkout-v2", Vec::new());
        assert_eq!(
            named.folder(Some("/home/me/workspaces")),
            "/home/me/workspaces/checkout-v2"
        );
        assert_eq!(named.folder(None), "");
        assert_eq!(form("", Vec::new()).folder(Some("/w")), "");
    }

    #[test]
    fn branch_for_all_fills_every_row() {
        let mut form = form("checkout", vec![member("/a", "old"), member("/b", "")]);
        form.set_branch_for_all("feat/pay");
        assert!(form.members.iter().all(|m| m.branch == "feat/pay"));
    }

    #[test]
    fn the_repository_choices_are_the_window_then_the_recent_without_repeats() {
        let recents = [
            PathBuf::from("/r"),
            PathBuf::from("/billing"),
            PathBuf::from("/billing"),
        ];
        assert_eq!(repo_choices(Some("/r"), &recents), ["/r", "/billing"]);
        assert_eq!(repo_choices(None, &recents), ["/r", "/billing"]);
        assert!(repo_choices(None, &[]).is_empty());
    }

    #[test]
    fn a_row_says_what_its_branch_does() {
        assert_eq!(
            describe(&plan("/r", "r", "feat", BranchHow::CheckOut)),
            "checks out feat"
        );
        assert_eq!(
            describe(&plan(
                "/r",
                "r",
                "feat",
                BranchHow::TrackRemote {
                    remote: "origin/feat".into()
                }
            )),
            "tracks origin/feat"
        );
        assert_eq!(
            describe(&plan(
                "/r",
                "r",
                "feat",
                BranchHow::CreateFrom {
                    base: "main".into()
                }
            )),
            "creates feat from main"
        );
    }

    #[test]
    fn each_row_takes_its_own_plan_and_a_problem_names_its_repository() {
        let rows = [
            member("/a", "feat"),
            member("/b", "feat"),
            member("/c", "x"),
        ];
        let answer = answer(
            vec![
                plan("/a", "a", "feat", BranchHow::CheckOut),
                plan("/c", "c", "x", BranchHow::CheckOut),
            ],
            vec![problem(
                Some("/b"),
                "its branch is checked out elsewhere",
                Some("/b-feat"),
            )],
        );
        let (shown, rest) = outcomes(&rows, &answer);
        assert_eq!(
            shown[0],
            Some(Outcome::Plan(plan("/a", "a", "feat", BranchHow::CheckOut)))
        );
        assert_eq!(
            shown[1],
            Some(Outcome::Problem(problem(
                Some("/b"),
                "its branch is checked out elsewhere",
                Some("/b-feat")
            )))
        );
        assert_eq!(
            shown[2],
            Some(Outcome::Plan(plan("/c", "c", "x", BranchHow::CheckOut)))
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn a_problem_no_row_names_is_listed_under_the_rows() {
        let rows = [member("/a", "feat")];
        let answer = answer(
            Vec::new(),
            vec![problem(None, "two members would be the same folder", None)],
        );
        let (shown, rest) = outcomes(&rows, &answer);
        assert_eq!(shown, [None]);
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn create_is_enabled_only_for_a_complete_form_with_a_clean_answer() {
        let complete = form("checkout", vec![member("/a", "feat")]);
        let mut planning = Planning::default();
        let making = Making::default();
        let edit = planning.edited();
        assert!(!may_create(&complete, &planning, &making), "no answer yet");
        planning.sent(OperationId(4));
        assert!(
            !may_create(&complete, &planning, &making),
            "answer in flight"
        );
        assert!(planning.answered(
            OperationId(4),
            answer(
                vec![plan("/a", "a", "feat", BranchHow::CheckOut)],
                Vec::new()
            )
        ));
        assert!(may_create(&complete, &planning, &making));
        assert!(!may_create(
            &form("", vec![member("/a", "feat")]),
            &planning,
            &making
        ));
        assert!(planning.is_latest(edit));
    }

    #[test]
    fn a_refused_member_disables_create() {
        let complete = form("checkout", vec![member("/a", "feat")]);
        let mut planning = Planning::default();
        planning.edited();
        planning.sent(OperationId(1));
        planning.answered(
            OperationId(1),
            answer(Vec::new(), vec![problem(Some("/a"), "no", None)]),
        );
        assert!(!may_create(&complete, &planning, &Making::default()));
    }

    #[test]
    fn an_answer_to_an_older_form_is_dropped() {
        let mut planning = Planning::default();
        planning.edited();
        planning.sent(OperationId(1));
        let newer = planning.edited();
        assert!(!planning.answered(OperationId(1), answer(Vec::new(), Vec::new())));
        assert!(planning.answer().is_none());
        assert!(planning.is_latest(newer));
        assert!(
            !planning.is_latest(newer - 1),
            "only the newest edit sends a plan"
        );
    }

    #[test]
    fn a_plan_that_fails_keeps_its_reason_until_the_next_edit() {
        let mut planning = Planning::default();
        planning.edited();
        planning.sent(OperationId(2));
        assert!(planning.settled(
            OperationId(2),
            OperationState::Failed,
            Some("'bad name' is not a workspace name".into())
        ));
        assert_eq!(
            planning.failure(),
            Some("'bad name' is not a workspace name")
        );
        planning.edited();
        assert_eq!(planning.failure(), None);
    }

    #[test]
    fn creating_keeps_only_its_own_operation_and_gives_the_folder() {
        let mut making = Making::default();
        making.start(OperationId(9));
        assert!(!making.created(OperationId(3), "/w/x".into()));
        assert!(making.created(OperationId(9), "/w/x".into()));
        making.settle(OperationState::Done, None);
        assert_eq!(making.take(), Some(Ok("/w/x".to_owned())));
        assert!(!making.is_pending());
    }

    #[test]
    fn a_failed_create_keeps_the_engine_reason() {
        let mut making = Making::default();
        making.start(OperationId(1));
        making.settle(
            OperationState::Failed,
            Some("no workspace was created: a member is checked out elsewhere".into()),
        );
        assert_eq!(
            making.take(),
            Some(Err(
                "no workspace was created: a member is checked out elsewhere".to_owned()
            ))
        );
        making.start(OperationId(2));
        making.settle(OperationState::Failed, None);
        assert_eq!(
            making.take(),
            Some(Err("The workspace was not created.".to_owned()))
        );
    }
}
