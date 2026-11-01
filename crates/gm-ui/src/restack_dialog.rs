//! The restack dialog (D14, D142). It plans the restack of a stack, lists each
//! branch with what its replay gives, lets the person leave branches out, and
//! moves refs only when they confirm. After a restack it offers Undo. The state
//! and the wording are pure and tested here; the window side is the
//! `impl StatusView` at the bottom.

use std::fmt::Write as _;

use gm_schema::{
    Actor, CommandKind, OperationId, OperationState, PlannedBranch, ReplayStep, RestackBlock,
    RestackPlan, RestackRecord, RestackResolution,
};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Window, div, prelude::*, px,
};

use crate::conflict_resolver::RestackConflicts;
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 600.;
const TOP: f32 = 80.;
const LIST_MAX: f32 = 360.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Planning,
    Applying,
    Undoing,
}

/// What a finished operation means for the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The restack ran; the dialog shows Undo.
    Applied,
    /// The restack was undone; the dialog closes.
    Undone,
    /// The plan, the apply or the undo was refused or failed, with the reason.
    Failed(String),
}

/// A command of a restack conflict the window sent, as the engine answers it (D154).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolutionStep {
    /// `StartRestackResolution`: opens the resolution worktree on the conflict.
    Start,
    /// `FinishRestackResolution`: records the resolved commit and plans again.
    Finish,
    /// `CancelRestackResolution`: closes the resolution worktree.
    Cancel,
}

/// Where the window goes after the engine answered a restack conflict command (D154).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    /// The resolution worktree is open: the dialog closes and the resolver opens.
    Opened,
    /// The resolver is done (finished or cancelled): the dialog comes back.
    Returned,
}

/// The stack being restacked, the plan on screen, the branches left out, the
/// operation in flight, and the record undo would use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Restacking {
    pending: Option<(OperationId, Phase)>,
    /// The branch the plan starts from (the root of the stack).
    from: String,
    skip: Vec<String>,
    plan: Option<RestackPlan>,
    /// The last restack that can be undone, as the engine reported it.
    record: Option<RestackRecord>,
    /// Set once the open dialog's restack ran.
    applied: bool,
    outcome: Option<Outcome>,
    /// Whether `ResumeRestack` was sent for the open repository.
    resume_sent: bool,
    /// The restack conflict the resolver works on, once the engine opened its worktree.
    resolving: Option<RestackConflicts>,
    /// A resolution command the window sent and the engine has not answered yet.
    resolution_pending: Option<(OperationId, ResolutionStep)>,
    /// What the window does next: the resolver opens, or the dialog comes back.
    transition: Option<Transition>,
}

impl Restacking {
    /// Starts the dialog on the stack rooted at `from`.
    pub(crate) fn begin(&mut self, from: &str) {
        from.clone_into(&mut self.from);
        self.skip.clear();
        self.plan = None;
        self.applied = false;
        self.outcome = None;
    }

    /// True once per repository: the moment to send `ResumeRestack`.
    pub(crate) fn take_resume(&mut self) -> bool {
        !std::mem::replace(&mut self.resume_sent, true)
    }

    pub(crate) fn from(&self) -> &str {
        &self.from
    }

    pub(crate) fn skip(&self) -> &[String] {
        &self.skip
    }

    pub(crate) fn plan(&self) -> Option<&RestackPlan> {
        self.plan.as_ref()
    }

    pub(crate) fn record(&self) -> Option<&RestackRecord> {
        self.record.as_ref()
    }

    pub(crate) fn applied(&self) -> bool {
        self.applied
    }

    pub(crate) fn begin_plan(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Planning));
        self.plan = None;
        self.outcome = None;
    }

    pub(crate) fn begin_apply(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Applying));
        self.outcome = None;
    }

    pub(crate) fn begin_undo(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Undoing));
        self.outcome = None;
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// The restack conflict the resolver works on, while one is open.
    pub(crate) fn resolving(&self) -> Option<&RestackConflicts> {
        self.resolving.as_ref()
    }

    /// Whether a resolution command is in flight: the dialog and the resolver wait for it.
    pub(crate) fn resolution_busy(&self) -> bool {
        self.resolution_pending.is_some()
    }

    pub(crate) fn begin_resolution(&mut self, id: OperationId, step: ResolutionStep) {
        self.resolution_pending = Some((id, step));
    }

    /// Whether `id` is the answer the window waits for, as `step`.
    pub(crate) fn owns_resolution(&self, id: OperationId, step: ResolutionStep) -> bool {
        self.resolution_pending == Some((id, step))
    }

    /// Whether `id` is the operation of the resolution command in flight, whatever its step.
    pub(crate) fn awaits_resolution(&self, id: OperationId) -> bool {
        self.resolution_pending
            .is_some_and(|(pending, _)| pending == id)
    }

    /// The engine opened the resolution worktree: the resolver takes the conflict (D154).
    pub(crate) fn resolution_started(&mut self, resolution: RestackResolution) {
        self.resolution_pending = None;
        self.resolving = Some(RestackConflicts::new(resolution));
        self.transition = Some(Transition::Opened);
    }

    /// The resolution was cancelled: the plan on screen is unchanged, and the dialog returns to it.
    pub(crate) fn resolution_cancelled(&mut self) {
        self.resolution_pending = None;
        self.resolving = None;
        self.transition = Some(Transition::Returned);
    }

    /// Finish answered with the plan of the stack, which now holds the resolved commit (D154).
    pub(crate) fn resolution_finished(&mut self, plan: RestackPlan) {
        self.resolution_pending = None;
        self.resolving = None;
        self.plan = Some(plan);
        self.transition = Some(Transition::Returned);
    }

    /// A resolution command the engine refused or failed. A refused Start is the dialog's
    /// error; a refused Finish or Cancel is the resolver's note, and the resolver stays open.
    /// Done answers nothing here: the result event carries the change.
    pub(crate) fn settle_resolution(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<String>,
    ) {
        if !state.is_terminal() || state == OperationState::Done || !self.awaits_resolution(id) {
            return;
        }
        let Some((_, step)) = self.resolution_pending.take() else {
            return;
        };
        let message = detail.unwrap_or_else(|| {
            match step {
                ResolutionStep::Start => "The conflict could not be opened.",
                ResolutionStep::Finish => "The resolution could not be finished.",
                ResolutionStep::Cancel => "The resolution could not be cancelled.",
            }
            .to_owned()
        });
        match step {
            ResolutionStep::Start => self.outcome = Some(Outcome::Failed(message)),
            ResolutionStep::Finish | ResolutionStep::Cancel => {
                if let Some(resolving) = self.resolving.as_mut() {
                    resolving.set_note(Some(message));
                }
            }
        }
    }

    /// A file of the restack conflict was applied.
    pub(crate) fn mark_resolved(&mut self, path: &str) {
        if let Some(resolving) = self.resolving.as_mut() {
            resolving.mark_resolved(path);
        }
    }

    /// Whether Continue may send Finish. While files remain it says which in the resolver's
    /// note and returns false, so the answer is a message, not a disabled button (D116(4)).
    pub(crate) fn may_finish(&mut self) -> bool {
        let Some(resolving) = self.resolving.as_mut() else {
            return false;
        };
        let refusal = resolving.continue_refusal();
        resolving.set_note(refusal.clone());
        refusal.is_none()
    }

    pub(crate) fn take_transition(&mut self) -> Option<Transition> {
        self.transition.take()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    pub(crate) fn planned(&mut self, plan: RestackPlan) {
        self.plan = Some(plan);
    }

    /// The record undo would use: set by an apply, by a resume at open, and
    /// cleared by an undo.
    pub(crate) fn recorded(&mut self, record: Option<RestackRecord>) {
        self.record = record;
    }

    /// Leaves the branch out of the plan, or puts it back.
    pub(crate) fn toggle_skip(&mut self, name: &str) {
        match self.skip.iter().position(|skipped| skipped == name) {
            Some(at) => {
                self.skip.remove(at);
            }
            None => self.skip.push(name.to_owned()),
        }
    }

    /// Ends the operation with its terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, phase)) = self.pending.take() else {
            return;
        };
        self.outcome = match (phase, state) {
            (Phase::Planning, OperationState::Done) => None,
            (Phase::Applying, OperationState::Done) => {
                self.applied = true;
                Some(Outcome::Applied)
            }
            (Phase::Undoing, OperationState::Done) => Some(Outcome::Undone),
            (phase, _) => Some(Outcome::Failed(detail.unwrap_or_else(|| {
                match phase {
                    Phase::Planning => "The restack could not be planned.",
                    Phase::Applying => "The restack did not run.",
                    Phase::Undoing => "The restack could not be undone.",
                }
                .to_owned()
            }))),
        };
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// Whether the Restack button works: a plan that may start, nothing in flight,
/// and this dialog's restack not yet run.
#[must_use]
pub(crate) fn can_apply(plan: Option<&RestackPlan>, busy: bool, applied: bool) -> bool {
    !busy && !applied && plan.is_some_and(RestackPlan::can_apply)
}

#[must_use]
pub(crate) fn apply_label(plan: Option<&RestackPlan>) -> String {
    let moving = plan.map_or(0, |plan| {
        plan.branches
            .iter()
            .filter(|branch| branch.block.is_none() && branch.new_tip.is_some())
            .count()
    });
    format!("Restack {moving} {}", plural(moving, "branch", "branches"))
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// What a branch of the plan does, in one line.
#[must_use]
pub(crate) fn summary(branch: &PlannedBranch) -> String {
    match &branch.block {
        Some(RestackBlock::Refused { reason }) => reason.clone(),
        Some(RestackBlock::ParentBlocked { parent }) => {
            format!("Waits for {parent}, which does not move.")
        }
        Some(RestackBlock::Skipped) => "Left out. It stays where it is.".to_owned(),
        Some(RestackBlock::Conflict { commit, .. }) => {
            let at = branch.commits.iter().find(|planned| &planned.oid == commit);
            let subject = at.map_or("", |planned| planned.subject.as_str());
            let paths = match at.map(|planned| &planned.step) {
                Some(ReplayStep::Conflict { paths }) => paths.join(", "),
                _ => String::new(),
            };
            format!("Conflict in “{subject}”: {paths}. It stays where it is.")
        }
        None => {
            let replayed = branch.replayed();
            let resolved = branch
                .commits
                .iter()
                .filter(|commit| matches!(commit.step, ReplayStep::Resolved { .. }))
                .count();
            let dropped = branch.commits.len() - replayed;
            let mut line = if resolved == 0 {
                format!(
                    "Replays {replayed} {} onto {}.",
                    plural(replayed, "commit", "commits"),
                    branch.parent
                )
            } else if resolved == replayed {
                format!(
                    "Resolved {resolved} {} onto {}.",
                    plural(resolved, "commit", "commits"),
                    branch.parent
                )
            } else {
                let clean = replayed - resolved;
                format!(
                    "Replays {clean} {} onto {}. Resolved {resolved} {}.",
                    plural(clean, "commit", "commits"),
                    branch.parent,
                    plural(resolved, "commit", "commits")
                )
            };
            if dropped > 0 {
                let _ = write!(line, " {dropped} already in {} and dropped.", branch.parent);
            }
            line
        }
    }
}

/// The command Resolve conflict sends for `branch` (D154). It names the plan on screen, so
/// the engine refuses it when the stacks changed since. `None` for a branch with no conflict.
#[must_use]
pub(crate) fn resolve_command(
    from: &str,
    plan: &RestackPlan,
    skip: &[String],
    branch: &PlannedBranch,
) -> Option<CommandKind> {
    can_resolve(branch).then(|| CommandKind::StartRestackResolution {
        branch: from.to_owned(),
        plan_id: plan.plan_id.clone(),
        skip: skip.to_vec(),
        conflicted: branch.name.clone(),
    })
}

/// Whether the branch is blocked on a conflict, so Resolve conflict applies.
#[must_use]
pub(crate) fn can_resolve(branch: &PlannedBranch) -> bool {
    matches!(branch.block, Some(RestackBlock::Conflict { .. }))
}

/// Whether the branch is one the person can leave out: it is not already held
/// back by a parent, and it is not refused for its own reason.
#[must_use]
pub(crate) fn can_skip(branch: &PlannedBranch) -> bool {
    !matches!(branch.block, Some(RestackBlock::ParentBlocked { .. }))
}

/// The line under the title once the restack ran.
#[must_use]
pub(crate) fn done_line(record: &RestackRecord) -> String {
    let count = record.moves.len();
    format!(
        "Restacked {count} {}. Undo puts every one back.",
        plural(count, "branch", "branches")
    )
}

impl StatusView {
    /// Opens the dialog for the stack `branch` is in and plans it.
    pub(crate) fn open_restack_dialog(
        &mut self,
        branch: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.restacking.resolving().is_some() || self.model.restacking.resolution_busy() {
            self.notify_user("Finish or cancel the restack conflict first.", cx);
            return;
        }
        let root = self
            .model
            .stacks
            .snapshot()
            .and_then(|snapshot| {
                snapshot
                    .stacks
                    .iter()
                    .find(|stack| stack.branches.iter().any(|entry| entry.name == branch))
                    .map(|stack| stack.root().to_owned())
            })
            .unwrap_or_else(|| branch.to_owned());
        let focus = cx.focus_handle();
        focus.focus(window);
        self.restack_dialog = Some(RestackDialog { focus, error: None });
        self.model.restacking.begin(&root);
        self.request_restack_plan(cx);
    }

    fn request_restack_plan(&mut self, cx: &mut Context<Self>) {
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::PlanRestack {
                branch: self.model.restacking.from().to_owned(),
                skip: self.model.restacking.skip().to_vec(),
            },
        );
        self.model.restacking.begin_plan(id);
        cx.notify();
    }

    pub(crate) fn close_restack_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.restack_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    fn toggle_restack_skip(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.model.restacking.is_busy() || self.model.restacking.applied() {
            return;
        }
        self.model.restacking.toggle_skip(name);
        self.request_restack_plan(cx);
    }

    /// Resolve conflict on a blocked branch: the engine opens the resolution worktree on its
    /// conflict, in the plan on screen (D154). The resolver opens when the engine answers.
    pub(crate) fn resolve_restack_conflict(&mut self, branch: &str, cx: &mut Context<Self>) {
        let restacking = &self.model.restacking;
        if restacking.is_busy() || restacking.resolution_busy() || restacking.resolving().is_some()
        {
            return;
        }
        let command = restacking.plan().and_then(|plan| {
            plan.branches
                .iter()
                .find(|planned| planned.name == branch)
                .and_then(|planned| {
                    resolve_command(restacking.from(), plan, restacking.skip(), planned)
                })
        });
        let Some(command) = command else {
            return;
        };
        let id = self.engine.dispatch(Actor::Human, command);
        self.model
            .restacking
            .begin_resolution(id, ResolutionStep::Start);
        if let Some(dialog) = self.restack_dialog.as_mut() {
            dialog.error = None;
        }
        cx.notify();
    }

    /// Continue restack: sends Finish once every file is applied. Otherwise the resolver's
    /// note says what remains, and nothing is sent.
    pub(crate) fn continue_restack_resolution(&mut self, cx: &mut Context<Self>) {
        if self.model.restacking.resolution_busy() {
            return;
        }
        if self.model.restacking.may_finish() {
            let id = self
                .engine
                .dispatch(Actor::Human, CommandKind::FinishRestackResolution);
            self.model
                .restacking
                .begin_resolution(id, ResolutionStep::Finish);
        }
        cx.notify();
    }

    /// Cancel in the resolver: sends `CancelRestackResolution`; the dialog returns on its answer.
    pub(crate) fn cancel_restack_resolution(&mut self, cx: &mut Context<Self>) {
        if self.model.restacking.resolution_busy() {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::CancelRestackResolution);
        self.model
            .restacking
            .begin_resolution(id, ResolutionStep::Cancel);
        cx.notify();
    }

    /// The engine opened the resolution worktree: the dialog closes, the resolver opens, and
    /// the first conflicted file is read (D154).
    pub(crate) fn open_restack_resolver(&mut self, cx: &mut Context<Self>) {
        self.restack_dialog = None;
        self.restack_return = self.screen;
        self.model.conflict = None;
        self.show_conflicts(cx);
        let first = self
            .model
            .restacking
            .resolving()
            .and_then(RestackConflicts::first_unresolved)
            .map(str::to_owned);
        if let Some(path) = first {
            self.read_restack_file(&path, cx);
        }
    }

    /// Finish or Cancel answered: the resolver closes and the dialog comes back on the plan the
    /// model holds, on the screen it was opened from (D154).
    pub(crate) fn return_to_restack_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.conflict = None;
        self.screen = self.restack_return;
        let focus = cx.focus_handle();
        focus.focus(window);
        self.restack_dialog = Some(RestackDialog { focus, error: None });
        cx.notify();
    }

    /// The Restack button, and Enter: applies exactly the plan on screen.
    pub(crate) fn submit_restack(&mut self, cx: &mut Context<Self>) {
        let restacking = &self.model.restacking;
        if !can_apply(
            restacking.plan(),
            restacking.is_busy(),
            restacking.applied(),
        ) {
            return;
        }
        let Some(plan) = restacking.plan() else {
            return;
        };
        let command = CommandKind::ApplyRestack {
            branch: restacking.from().to_owned(),
            plan_id: plan.plan_id.clone(),
            skip: restacking.skip().to_vec(),
        };
        let id = self.engine.dispatch(Actor::Human, command);
        self.model.restacking.begin_apply(id);
        if let Some(dialog) = self.restack_dialog.as_mut() {
            dialog.error = None;
        }
        cx.notify();
    }

    pub(crate) fn submit_restack_undo(&mut self, cx: &mut Context<Self>) {
        if self.model.restacking.is_busy() || self.model.restacking.record().is_none() {
            return;
        }
        let id = self.engine.dispatch(Actor::Human, CommandKind::UndoRestack);
        self.model.restacking.begin_undo(id);
        if let Some(dialog) = self.restack_dialog.as_mut() {
            dialog.error = None;
        }
        cx.notify();
    }

    /// Runs when the engine settles a plan, an apply or an undo. A failure stays
    /// in the dialog; an undo closes it and says so.
    pub(crate) fn on_restack_settled(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Applied => {}
            Outcome::Undone => {
                self.restack_dialog = None;
                self.list_focus.focus(window);
                self.notify_user("Restack undone", cx);
            }
            Outcome::Failed(message) => match self.restack_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn restack_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.restack_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_restack_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(self.restack_panel(dialog, left, cx))
    }

    fn restack_panel(&self, dialog: &RestackDialog, left: f32, cx: &Context<Self>) -> Div {
        let restacking = &self.model.restacking;
        let plan = restacking.plan();
        let busy = restacking.is_busy();
        let header = if let (true, Some(record)) = (restacking.applied(), restacking.record()) {
            done_line(record)
        } else if plan.is_none() {
            "Planning…".to_owned()
        } else if plan.is_some_and(|plan| plan.branches.is_empty()) {
            "Every branch of this stack is up to date.".to_owned()
        } else {
            "Each branch is replayed commit by commit onto its parent. Your working copy is not touched, and Undo restores every branch.".to_owned()
        };
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
            .track_focus(&dialog.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_restack_dialog(window, cx),
                    "enter" => this.submit_restack(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Restack {}", restacking.from())),
            )
            .child(div().text_sm().text_color(theme::muted()).child(header))
            .children(
                plan.into_iter()
                    .flat_map(|plan| plan.stopped.iter())
                    .map(|reason| {
                        div()
                            .text_sm()
                            .text_color(theme::warn_fg())
                            .child(reason.clone())
                    }),
            )
            .child(self.restack_rows(plan, busy || restacking.applied(), cx))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(self.restack_buttons(busy, cx))
    }

    fn restack_rows(&self, plan: Option<&RestackPlan>, locked: bool, cx: &Context<Self>) -> Div {
        let skipped = self.model.restacking.skip();
        let rows = plan.into_iter().flat_map(|plan| plan.branches.iter());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .max_h(px(LIST_MAX))
            .overflow_y_hidden()
            .children(rows.enumerate().map(|(index, branch)| {
                let left_out = skipped.contains(&branch.name);
                let name = branch.name.clone();
                let conflicted = branch.name.clone();
                let blocked = branch.block.is_some();
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text())
                                    .font_family(theme::code_font())
                                    .child(branch.name.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(if blocked {
                                        theme::warn_fg()
                                    } else {
                                        theme::muted()
                                    })
                                    .child(summary(branch)),
                            ),
                    )
                    .when(can_resolve(branch) && !locked, |row| {
                        row.child(
                            div()
                                .id(("restack-resolve", index))
                                .text_sm()
                                .text_color(theme::accent())
                                .cursor_pointer()
                                .child("Resolve conflict")
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.resolve_restack_conflict(&conflicted, cx);
                                })),
                        )
                    })
                    .when(can_skip(branch) && !locked, |row| {
                        row.child(
                            div()
                                .id(("restack-skip", index))
                                .text_sm()
                                .text_color(theme::accent())
                                .cursor_pointer()
                                .child(if left_out {
                                    "Include"
                                } else {
                                    "Skip branch and children"
                                })
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.toggle_restack_skip(&name, cx);
                                })),
                        )
                    })
            }))
    }

    fn restack_buttons(&self, busy: bool, cx: &Context<Self>) -> Div {
        let restacking = &self.model.restacking;
        let enabled = can_apply(restacking.plan(), busy, restacking.applied());
        let label = if matches!(restacking.pending, Some((_, Phase::Applying))) {
            "Restacking…".to_owned()
        } else {
            apply_label(restacking.plan())
        };
        let undo_enabled = !busy && restacking.record().is_some();
        let go: Stateful<Div> = div()
            .id("restack-confirm")
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .rounded_md()
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(FontWeight::MEDIUM)
            .when(!enabled, |this| this.opacity(0.5))
            .when(enabled, gpui::Styled::cursor_pointer)
            .child(label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.submit_restack(cx);
            }));
        let undo: Stateful<Div> = div()
            .id("restack-undo")
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .rounded_md()
            .text_color(theme::text())
            .when(!undo_enabled, |this| this.opacity(0.5))
            .when(undo_enabled, gpui::Styled::cursor_pointer)
            .child("Undo restack")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.submit_restack_undo(cx);
            }));
        div()
            .flex()
            .flex_row()
            .justify_end()
            .items_center()
            .gap_2()
            .when(restacking.record().is_some(), |row| row.child(undo))
            .child(
                div()
                    .id("restack-close")
                    .px_3()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::text())
                    .child(if restacking.applied() {
                        "Close"
                    } else {
                        "Cancel"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.close_restack_dialog(window, cx);
                        }),
                    ),
            )
            .when(!restacking.applied(), |row| row.child(go))
    }
}

/// The dialog while it is open: the focus that carries the keys and the last error.
pub(crate) struct RestackDialog {
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::PlannedCommit;

    fn planned(name: &str, block: Option<RestackBlock>, steps: &[ReplayStep]) -> PlannedBranch {
        PlannedBranch {
            name: name.into(),
            parent: "main".into(),
            old_tip: "old".into(),
            old_base: Some("base".into()),
            onto: "onto".into(),
            commits: steps
                .iter()
                .enumerate()
                .map(|(n, step)| PlannedCommit {
                    oid: format!("c{n}"),
                    subject: format!("subject {n}"),
                    step: step.clone(),
                })
                .collect(),
            new_tip: block.is_none().then(|| "new".to_owned()),
            block,
        }
    }

    fn plan(branches: Vec<PlannedBranch>) -> RestackPlan {
        RestackPlan {
            plan_id: "id".into(),
            from: "main".into(),
            branches,
            stopped: vec![],
        }
    }

    #[test]
    fn a_clean_branch_counts_replayed_and_dropped_commits() {
        let clean = ReplayStep::Clean { new: "n".into() };
        let branch = planned("a", None, &[clean.clone(), ReplayStep::Dropped, clean]);
        assert_eq!(
            summary(&branch),
            "Replays 2 commits onto main. 1 already in main and dropped."
        );
        let one = planned("a", None, &[ReplayStep::Clean { new: "n".into() }]);
        assert_eq!(summary(&one), "Replays 1 commit onto main.");
    }

    #[test]
    fn a_conflict_names_the_commit_and_the_files() {
        let step = ReplayStep::Conflict {
            paths: vec!["a.txt".into(), "b.txt".into()],
        };
        let branch = planned(
            "a",
            Some(RestackBlock::Conflict {
                commit: "c0".into(),
                onto: "base".into(),
            }),
            &[step],
        );
        assert_eq!(
            summary(&branch),
            "Conflict in “subject 0”: a.txt, b.txt. It stays where it is."
        );
    }

    #[test]
    fn blocked_branches_say_why() {
        let waits = planned(
            "b",
            Some(RestackBlock::ParentBlocked { parent: "a".into() }),
            &[],
        );
        assert_eq!(summary(&waits), "Waits for a, which does not move.");
        assert!(!can_skip(&waits), "its parent already holds it back");
        let skipped = planned("a", Some(RestackBlock::Skipped), &[]);
        assert!(summary(&skipped).starts_with("Left out"));
        assert!(can_skip(&skipped), "a skipped branch can be put back");
    }

    #[test]
    fn the_button_needs_a_plan_that_can_run_and_nothing_in_flight() {
        let clean = planned("a", None, &[ReplayStep::Clean { new: "n".into() }]);
        let ready = plan(vec![clean]);
        assert!(can_apply(Some(&ready), false, false));
        assert!(
            !can_apply(Some(&ready), true, false),
            "something is in flight"
        );
        assert!(!can_apply(Some(&ready), false, true), "it already ran");
        assert!(!can_apply(None, false, false), "not planned yet");
        assert!(
            !can_apply(Some(&plan(vec![])), false, false),
            "nothing to do"
        );
        let mut stopped = ready.clone();
        stopped.stopped.push("dirty".into());
        assert!(!can_apply(Some(&stopped), false, false));
    }

    #[test]
    fn the_button_counts_the_branches_that_move() {
        let clean = planned("a", None, &[ReplayStep::Clean { new: "n".into() }]);
        let skipped = planned("b", Some(RestackBlock::Skipped), &[]);
        assert_eq!(
            apply_label(Some(&plan(vec![clean.clone(), skipped]))),
            "Restack 1 branch"
        );
        assert_eq!(
            apply_label(Some(&plan(vec![clean.clone(), clean]))),
            "Restack 2 branches"
        );
    }

    #[test]
    fn skipping_toggles_and_a_new_dialog_starts_without_skips() {
        let mut state = Restacking::default();
        state.begin("main");
        state.toggle_skip("a");
        state.toggle_skip("b");
        state.toggle_skip("a");
        assert_eq!(state.skip(), ["b"]);
        state.begin("main");
        assert!(state.skip().is_empty());
    }

    #[test]
    fn a_planning_settles_quietly_and_an_apply_marks_the_dialog_applied() {
        let mut state = Restacking::default();
        state.begin("main");
        state.begin_plan(OperationId(1));
        assert!(state.owns(OperationId(1)) && state.is_busy());
        state.settle(OperationState::Done, None);
        assert_eq!(state.take_outcome(), None);
        state.begin_apply(OperationId(2));
        state.settle(OperationState::Done, None);
        assert_eq!(state.take_outcome(), Some(Outcome::Applied));
        assert!(state.applied());
        state.begin_undo(OperationId(3));
        state.settle(OperationState::Done, None);
        assert_eq!(state.take_outcome(), Some(Outcome::Undone));
    }

    #[test]
    fn the_resume_is_asked_once_per_repository() {
        let mut state = Restacking::default();
        assert!(state.take_resume());
        assert!(!state.take_resume());
        state = Restacking::default();
        assert!(state.take_resume(), "a new repository starts again");
    }

    #[test]
    fn a_refusal_carries_the_engines_reason() {
        let mut state = Restacking::default();
        state.begin_apply(OperationId(2));
        state.settle(
            OperationState::Failed,
            Some("The stacks changed since the plan was made. Plan again.".into()),
        );
        assert_eq!(
            state.take_outcome(),
            Some(Outcome::Failed(
                "The stacks changed since the plan was made. Plan again.".into()
            ))
        );
        assert!(!state.applied());
    }

    fn conflicting(name: &str, commit: &str) -> PlannedBranch {
        planned(
            name,
            Some(RestackBlock::Conflict {
                commit: commit.into(),
                onto: "base".into(),
            }),
            &[ReplayStep::Conflict {
                paths: vec!["a.txt".into()],
            }],
        )
    }

    fn resolution(paths: &[&str]) -> RestackResolution {
        RestackResolution {
            from: "main".into(),
            conflicted: "a".into(),
            parent: "main".into(),
            commit: "c0".into(),
            subject: "subject 0".into(),
            onto: "base".into(),
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    #[test]
    fn resolve_conflict_is_offered_on_a_conflict_and_names_the_plan_on_screen() {
        let blocked = conflicting("a", "c0");
        let clean = planned("b", None, &[ReplayStep::Clean { new: "n".into() }]);
        assert!(can_resolve(&blocked));
        assert!(
            !can_resolve(&clean),
            "a clean branch has nothing to resolve"
        );
        let ready = plan(vec![blocked.clone(), clean.clone()]);
        assert_eq!(
            resolve_command("main", &ready, &["x".to_owned()], &blocked),
            Some(CommandKind::StartRestackResolution {
                branch: "main".into(),
                plan_id: "id".into(),
                skip: vec!["x".into()],
                conflicted: "a".into(),
            })
        );
        assert_eq!(resolve_command("main", &ready, &[], &clean), None);
    }

    #[test]
    fn a_resolved_step_reads_resolved_and_a_clean_one_keeps_its_replay_wording() {
        let resolved = ReplayStep::Resolved { new: "r".into() };
        let clean = ReplayStep::Clean { new: "n".into() };
        let only_resolved = planned("a", None, std::slice::from_ref(&resolved));
        assert_eq!(summary(&only_resolved), "Resolved 1 commit onto main.");
        let mixed = planned("a", None, &[clean.clone(), resolved.clone()]);
        assert_eq!(
            summary(&mixed),
            "Replays 1 commit onto main. Resolved 1 commit."
        );
        let clean_only = planned("a", None, &[clean]);
        assert_eq!(summary(&clean_only), "Replays 1 commit onto main.");
        assert_eq!(only_resolved.replayed(), 1, "a resolved step is replayed");
    }

    #[test]
    fn a_started_resolution_opens_the_resolver_and_a_cancel_returns_to_the_same_plan() {
        let mut state = Restacking::default();
        state.begin("main");
        state.begin_plan(OperationId(1));
        state.planned(plan(vec![conflicting("a", "c0")]));
        state.settle(OperationState::Done, None);
        let before = state.plan().cloned();

        state.begin_resolution(OperationId(2), ResolutionStep::Start);
        assert!(state.resolution_busy());
        assert!(state.owns_resolution(OperationId(2), ResolutionStep::Start));
        assert!(!state.owns_resolution(OperationId(2), ResolutionStep::Finish));
        state.resolution_started(resolution(&["a.txt"]));
        assert!(!state.resolution_busy(), "the start is answered");
        assert_eq!(state.take_transition(), Some(Transition::Opened));
        assert_eq!(
            state.resolving().map(RestackConflicts::first_unresolved),
            Some(Some("a.txt"))
        );

        state.begin_resolution(OperationId(3), ResolutionStep::Cancel);
        state.settle_resolution(OperationId(3), OperationState::Done, None);
        assert!(
            state.resolution_busy(),
            "a Done is left to the answer event"
        );
        state.resolution_cancelled();
        assert!(state.resolving().is_none());
        assert_eq!(state.take_transition(), Some(Transition::Returned));
        assert_eq!(
            state.plan().cloned(),
            before,
            "a cancel keeps the plan as it was"
        );
    }

    #[test]
    fn a_finish_answers_with_the_plan_that_holds_the_resolved_commit() {
        let mut state = Restacking::default();
        state.begin("main");
        state.planned(plan(vec![conflicting("a", "c0")]));
        state.resolution_started(resolution(&["a.txt"]));
        state.take_transition();
        state.begin_resolution(OperationId(4), ResolutionStep::Finish);
        let replanned = plan(vec![planned(
            "a",
            None,
            &[ReplayStep::Resolved { new: "r".into() }],
        )]);
        state.resolution_finished(replanned.clone());
        assert_eq!(state.plan(), Some(&replanned));
        assert!(state.resolving().is_none());
        assert!(!state.resolution_busy());
        assert_eq!(state.take_transition(), Some(Transition::Returned));
    }

    #[test]
    fn a_refused_finish_stays_in_the_resolver_with_the_engines_reason() {
        let mut state = Restacking::default();
        state.resolution_started(resolution(&["a.txt"]));
        state.take_transition();
        state.begin_resolution(OperationId(7), ResolutionStep::Finish);
        state.settle_resolution(
            OperationId(7),
            OperationState::Failed,
            Some("a.txt is still unmerged".into()),
        );
        let resolving = state.resolving().expect("the resolver stays open");
        assert_eq!(resolving.note(), Some("a.txt is still unmerged"));
        assert_eq!(state.take_transition(), None);
        assert!(!state.resolution_busy());
    }

    #[test]
    fn a_refused_start_is_the_dialogs_error_and_other_operations_are_not_its_answer() {
        let mut state = Restacking::default();
        state.begin_resolution(OperationId(2), ResolutionStep::Start);
        state.settle_resolution(OperationId(9), OperationState::Failed, Some("other".into()));
        assert!(
            state.resolution_busy(),
            "an answer for another operation is ignored"
        );
        state.settle_resolution(
            OperationId(2),
            OperationState::Failed,
            Some("The stacks changed since the plan was made. Plan again.".into()),
        );
        assert_eq!(
            state.take_outcome(),
            Some(Outcome::Failed(
                "The stacks changed since the plan was made. Plan again.".into()
            ))
        );
        assert!(!state.resolution_busy());
    }

    #[test]
    fn continue_is_refused_with_a_message_while_a_file_is_unresolved() {
        let mut state = Restacking::default();
        state.resolution_started(resolution(&["a.txt", "b.txt"]));
        assert!(!state.may_finish());
        assert_eq!(
            state.resolving().and_then(RestackConflicts::note),
            Some("2 files are still unresolved. Resolve them and Apply, or Cancel.")
        );
        state.mark_resolved("a.txt");
        assert!(!state.may_finish(), "one file is still open");
        state.mark_resolved("b.txt");
        assert!(state.may_finish());
        assert_eq!(state.resolving().and_then(RestackConflicts::note), None);
    }

    #[test]
    fn continue_may_run_when_rerere_left_no_file() {
        let mut state = Restacking::default();
        state.resolution_started(resolution(&[]));
        assert!(state.may_finish());
        assert!(
            !Restacking::default().may_finish(),
            "nothing is being resolved"
        );
    }
}
