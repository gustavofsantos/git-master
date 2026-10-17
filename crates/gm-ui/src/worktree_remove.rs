//! The remove-worktree dialog (slice 1c, W5; D31). The engine inspects the
//! worktree when the dialog opens and checks the removal again when it is sent;
//! this module only shows what the inspection says and sends what the user
//! confirms. The rules are pure and tested here: which blockers the dialog
//! lists, when Remove is enabled, and what an outcome means. The window side
//! is the `impl StatusView` at the bottom.

use gm_schema::{Actor, Blocker, CommandKind, OperationId, OperationState, WorktreeInspection};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// Shown while the engine has not answered the inspection yet.
const CHECKING: &str = "Checking the worktree…";

/// Whether the removal may be sent with the confirmation `confirmed` (the
/// ticked checkbox). A worktree with no blockers goes at once; one with soft
/// blockers needs the confirmation, and a hard blocker (main, the window's own,
/// a missing folder) never goes.
#[must_use]
pub(crate) fn may_remove(inspection: &WorktreeInspection, confirmed: bool) -> bool {
    inspection.check_removal(confirmed).is_ok()
}

/// Whether the dialog asks for the confirmation: a soft blocker applies and no
/// hard one does, since a hard blocker refuses whatever is ticked.
#[must_use]
pub(crate) fn asks_confirmation(inspection: &WorktreeInspection) -> bool {
    !inspection.blockers.is_empty() && !inspection.blockers.iter().any(|b| b.is_hard())
}

/// The checkbox label for the soft blockers the removal would discard.
#[must_use]
pub(crate) fn confirmation_label(inspection: &WorktreeInspection) -> &'static str {
    let dirty = inspection.blockers.contains(&Blocker::Dirty);
    let unpushed = inspection.blockers.contains(&Blocker::Unpushed);
    match (dirty, unpushed) {
        (true, true) => {
            "I understand this discards the uncommitted changes and unpushed commits listed above"
        }
        (true, false) => "I understand this discards the uncommitted changes listed above",
        (false, true) => "I understand this discards the unpushed commits listed above",
        (false, false) => "I understand the lock is overridden and the folder is deleted",
    }
}

/// The lines the dialog shows under its title: one per blocker, or, when there
/// are none, what the removal does.
#[must_use]
pub(crate) fn body_lines(inspection: &WorktreeInspection) -> Vec<String> {
    if inspection.blockers.is_empty() {
        return vec![format!(
            "The folder {} is deleted, with any ignored files in it.",
            inspection.path
        )];
    }
    inspection
        .blockers
        .iter()
        .map(|blocker| inspection.describe(*blocker))
        .collect()
}

/// The removal in flight or settled, as the model keeps it. Events of other
/// operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Removal {
    /// The worktree the dialog is about.
    path: Option<String>,
    /// The operation the dialog waits for, and what it is.
    pending: Option<(OperationId, Phase)>,
    inspection: Option<WorktreeInspection>,
    /// `Ok` with the removed path, or `Err` with the reason, until the view takes it.
    outcome: Option<Result<String, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Inspecting,
    Removing,
}

impl Removal {
    pub(crate) fn begin_inspect(&mut self, id: OperationId, path: String) {
        self.path = Some(path);
        self.pending = Some((id, Phase::Inspecting));
        self.inspection = None;
        self.outcome = None;
    }

    pub(crate) fn begin_remove(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Removing));
        self.outcome = None;
    }

    /// Whether a removal is in flight, so a second one is not sent.
    pub(crate) fn is_removing(&self) -> bool {
        matches!(self.pending, Some((_, Phase::Removing)))
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    /// Keeps the inspection when it is about the worktree the dialog shows.
    pub(crate) fn inspected(&mut self, inspection: WorktreeInspection) {
        if self.path.as_deref() == Some(inspection.path.as_str()) {
            self.inspection = Some(inspection);
        }
    }

    /// The inspection of `path`, once the engine has answered.
    pub(crate) fn inspection(&self, path: &str) -> Option<&WorktreeInspection> {
        self.inspection.as_ref().filter(|found| found.path == path)
    }

    /// Ends the operation with its terminal `state`. An inspection that
    /// finished is nothing to show; a failed one or a failed removal gives
    /// Git's first reason.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, phase)) = self.pending.take() else {
            return;
        };
        let path = self.path.clone().unwrap_or_default();
        self.outcome = match (phase, state) {
            (Phase::Inspecting, OperationState::Done) => None,
            (Phase::Inspecting, _) => {
                Some(Err(detail.unwrap_or_else(|| {
                    "The worktree could not be read.".to_owned()
                })))
            }
            (Phase::Removing, OperationState::Done) => Some(Ok(path)),
            (Phase::Removing, _) => {
                Some(Err(detail.unwrap_or_else(|| {
                    "The worktree was not removed.".to_owned()
                })))
            }
        };
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the worktree, whether its confirmation is
/// ticked, the focus that carries the keys, and the error it shows.
pub(crate) struct RemoveDialog {
    pub(crate) path: String,
    pub(crate) confirmed: bool,
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the dialog for the worktree at `path` and asks the engine what
    /// removing it would lose.
    pub(crate) fn open_remove_dialog(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::InspectWorktree {
                path: path.to_owned(),
            },
        );
        self.model.removal.begin_inspect(id, path.to_owned());
        let focus = cx.focus_handle();
        focus.focus(window);
        self.removal_dialog = Some(RemoveDialog {
            path: path.to_owned(),
            confirmed: false,
            focus,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_remove_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.removal_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Space and a click on the checkbox: ticks or unticks the confirmation,
    /// when the dialog asks for one and no removal is in flight.
    pub(crate) fn toggle_removal_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.model.removal.is_removing() {
            return;
        }
        let asks = self
            .removal_dialog
            .as_ref()
            .and_then(|dialog| self.model.removal.inspection(&dialog.path))
            .is_some_and(asks_confirmation);
        if let Some(dialog) = self.removal_dialog.as_mut().filter(|_| asks) {
            dialog.confirmed = !dialog.confirmed;
            cx.notify();
        }
    }

    /// Enter and the remove button: checks the confirmation against the
    /// inspection, then sends the removal. Forced only when there are blockers.
    pub(crate) fn submit_removal(&mut self, cx: &mut Context<Self>) {
        if self.model.removal.is_removing() {
            return;
        }
        let Some(dialog) = self.removal_dialog.as_mut() else {
            return;
        };
        let confirmed = dialog.confirmed;
        let path = dialog.path.clone();
        let Some(inspection) = self.model.removal.inspection(&path) else {
            dialog.error = Some(CHECKING.to_owned());
            cx.notify();
            return;
        };
        dialog.error = None;
        // A refusal is not an error text: the dialog already says what to tick
        // (and a hard blocker is shown above the buttons), so the layout stays put.
        if inspection.check_removal(confirmed).is_err() {
            cx.notify();
            return;
        }
        let force = !inspection.blockers.is_empty();
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::RemoveWorktree { path, force });
        self.model.removal.begin_remove(id);
        cx.notify();
    }

    /// Runs when the engine settles the inspection or the removal. A removal
    /// closes the dialog and says so; a failure stays in the dialog with Git's reason.
    pub(crate) fn on_worktree_removal_settled(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(path) => {
                self.removal_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&format!("Removed worktree at {path}"), cx);
            }
            Err(message) => match self.removal_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn removal_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.removal_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_remove_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div().absolute().inset_0().child(outside).child(
            self.removal_panel(dialog, left, cx)
                // Tab has nothing else to reach while the dialog is open.
                .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
        )
    }

    fn removal_panel(&self, dialog: &RemoveDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.removal.is_removing();
        let inspection = self.model.removal.inspection(&dialog.path);
        let name = inspection.map_or_else(
            || {
                dialog
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            },
            |found| found.name.clone(),
        );
        let allowed = inspection.is_some_and(|found| may_remove(found, dialog.confirmed));
        let hard = inspection.is_some_and(|found| found.blockers.iter().any(|b| b.is_hard()));
        let forced = inspection.is_some_and(asks_confirmation);
        let label = inspection.map_or("", confirmation_label);
        let refusal = inspection
            .filter(|_| hard)
            .and_then(|found| found.check_removal(true).err());
        let body = inspection.map_or_else(|| vec![CHECKING.to_owned()], body_lines);
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
                    "escape" => this.close_remove_dialog(window, cx),
                    "enter" => this.submit_removal(cx),
                    "space" => this.toggle_removal_confirmation(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Remove worktree {name}?")),
            )
            .children(
                body.into_iter()
                    .map(|line| div().text_sm().text_color(theme::text()).child(line)),
            )
            .children(forced.then(|| confirm_row(dialog.confirmed, label, cx)))
            .children(
                refusal
                    .or_else(|| dialog.error.clone())
                    .map(|error| div().text_sm().text_color(theme::error()).child(error)),
            )
            .child(buttons(pending, allowed, cx))
    }
}

/// The checkbox that confirms a forced removal. Space and a click tick it.
fn confirm_row(confirmed: bool, label: &'static str, cx: &Context<StatusView>) -> Stateful<Div> {
    let mark = if confirmed {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id("worktree-remove-confirm-check")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(div().text_sm().text_color(theme::text()).child(label))
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.toggle_removal_confirmation(cx);
        }))
}

/// Cancel, and the destructive remove button, enabled only when the rules allow.
fn buttons(pending: bool, allowed: bool, cx: &Context<StatusView>) -> Div {
    let remove: Stateful<Div> = div()
        .id("worktree-remove-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::error())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!allowed || pending, |this| this.opacity(0.5))
        .when(allowed && !pending, gpui::Styled::cursor_pointer)
        .child(if pending {
            "Removing…"
        } else {
            "Remove worktree"
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_removal(cx);
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
                .id("worktree-remove-cancel")
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
                        this.close_remove_dialog(window, cx);
                    }),
                ),
        )
        .child(remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Blocker;

    fn inspection(path: &str, name: &str, blockers: Vec<Blocker>) -> WorktreeInspection {
        WorktreeInspection {
            path: path.to_owned(),
            branch: Some(name.to_owned()),
            name: name.to_owned(),
            locked: None,
            missing: blockers.contains(&Blocker::Missing),
            staged: 1,
            unstaged: 2,
            untracked: 0,
            unpushed: 3,
            blockers,
        }
    }

    #[test]
    fn a_clean_worktree_may_be_removed_without_a_confirmation() {
        let clean = inspection("/r-feature", "feature", Vec::new());
        assert!(may_remove(&clean, false));
        assert!(!asks_confirmation(&clean));
        assert_eq!(
            body_lines(&clean),
            vec!["The folder /r-feature is deleted, with any ignored files in it.".to_owned()]
        );
    }

    #[test]
    fn blockers_need_the_confirmation_ticked() {
        let dirty = inspection(
            "/r-feature",
            "feature",
            vec![Blocker::Dirty, Blocker::Unpushed],
        );
        assert!(asks_confirmation(&dirty));
        assert!(!may_remove(&dirty, false));
        assert!(may_remove(&dirty, true));
    }

    #[test]
    fn a_locked_worktree_is_forced_the_same_way() {
        let locked = inspection("/r-feature", "feature", vec![Blocker::Locked]);
        assert!(asks_confirmation(&locked));
        assert!(!may_remove(&locked, false));
        assert!(may_remove(&locked, true));
    }

    #[test]
    fn hard_blockers_are_never_removable_even_when_confirmed() {
        for blocker in [Blocker::Main, Blocker::Current, Blocker::Missing] {
            let hard = inspection("/r", "main", vec![blocker, Blocker::Dirty]);
            assert!(!asks_confirmation(&hard), "{blocker:?}");
            assert!(!may_remove(&hard, false), "{blocker:?}");
            assert!(!may_remove(&hard, true), "{blocker:?}");
        }
    }

    #[test]
    fn the_checkbox_label_names_what_is_discarded() {
        let dirty = inspection("/r", "f", vec![Blocker::Dirty]);
        let unpushed = inspection("/r", "f", vec![Blocker::Unpushed]);
        let both = inspection("/r", "f", vec![Blocker::Dirty, Blocker::Unpushed]);
        let locked = inspection("/r", "f", vec![Blocker::Locked]);
        assert_eq!(
            confirmation_label(&dirty),
            "I understand this discards the uncommitted changes listed above"
        );
        assert_eq!(
            confirmation_label(&unpushed),
            "I understand this discards the unpushed commits listed above"
        );
        assert_eq!(
            confirmation_label(&both),
            "I understand this discards the uncommitted changes and unpushed commits listed above"
        );
        assert_eq!(
            confirmation_label(&locked),
            "I understand the lock is overridden and the folder is deleted"
        );
    }

    #[test]
    fn the_body_lists_each_blocker_in_order() {
        let dirty = inspection(
            "/r-feature",
            "feature",
            vec![Blocker::Dirty, Blocker::Unpushed],
        );
        assert_eq!(
            body_lines(&dirty),
            vec![
                "It has uncommitted changes: 1 staged, 2 unstaged, 0 untracked.".to_owned(),
                "3 commits that no other branch or remote has.".to_owned(),
            ]
        );
    }

    #[test]
    fn a_removal_is_tracked_only_while_its_operation_runs() {
        let mut removal = Removal::default();
        removal.begin_inspect(OperationId(2), "/r-feature".to_owned());
        assert!(removal.owns(OperationId(2)));
        assert!(!removal.owns(OperationId(3)));
        removal.inspected(inspection("/other", "other", Vec::new()));
        assert!(removal.inspection("/r-feature").is_none());
        removal.inspected(inspection("/r-feature", "feature", vec![Blocker::Dirty]));
        assert!(removal.inspection("/r-feature").is_some());
        removal.settle(OperationState::Done, None);
        assert!(
            removal.take_outcome().is_none(),
            "an inspection leaves no outcome"
        );
        assert!(!removal.owns(OperationId(2)));
    }

    #[test]
    fn a_removal_that_succeeds_names_the_path_and_one_that_fails_keeps_gits_reason() {
        let mut removal = Removal::default();
        removal.begin_inspect(OperationId(1), "/r-feature".to_owned());
        removal.settle(OperationState::Done, None);
        removal.begin_remove(OperationId(2));
        assert!(removal.is_removing());
        removal.settle(OperationState::Done, None);
        assert_eq!(removal.take_outcome(), Some(Ok("/r-feature".to_owned())));
        assert!(!removal.is_removing());

        removal.begin_remove(OperationId(3));
        removal.settle(
            OperationState::Failed,
            Some("fatal: cannot remove a locked working tree".to_owned()),
        );
        assert_eq!(
            removal.take_outcome(),
            Some(Err("fatal: cannot remove a locked working tree".to_owned()))
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_so() {
        let mut removal = Removal::default();
        removal.begin_remove(OperationId(4));
        removal.settle(OperationState::Failed, None);
        assert_eq!(
            removal.take_outcome(),
            Some(Err("The worktree was not removed.".to_owned()))
        );
    }
}
