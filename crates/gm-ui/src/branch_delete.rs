//! The delete-branch dialog. Deleting a branch asks first: the dialog says what
//! Git will do, and one press deletes it safely (`git branch -d`). When Git
//! refuses because the branch has commits that are not merged, the dialog
//! shows Git's message and offers a force delete (`-D`) behind a checkbox that
//! is unticked, so the button stays off until the user ticks it. The rules are
//! pure and tested here; the window side is the `impl StatusView` at the bottom.

use gm_schema::{Actor, CommandKind, OperationId, OperationState};
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
/// The checkbox that allows the force delete.
const CONFIRM_FORCE: &str = "Discard commits that exist only on this branch";
/// The text of the safe delete, before Git has refused anything.
const SAFE_BODY: &str =
    "Git deletes the branch only when its commits are merged into HEAD or its upstream.";

/// Whether Git's refusal of the safe delete is the unmerged one, which a force
/// delete can overcome. Git's message is English (the engine sets `LC_ALL=C`).
#[must_use]
pub(crate) fn is_unmerged(message: &str) -> bool {
    message.contains("is not fully merged")
}

/// Whether the delete button may be pressed. Before Git refuses, it always may;
/// after an unmerged refusal, only once the force checkbox is ticked.
#[must_use]
pub(crate) fn may_delete(refusal: Option<&str>, confirmed: bool) -> bool {
    refusal.is_none() || confirmed
}

/// The delete in flight, and its outcome until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The branch is gone.
    Deleted(String),
    /// Git refused the safe delete: the branch has unmerged commits.
    Unmerged(String),
    /// Anything else, with Git's message or a plain one.
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Deleting {
    /// The operation in flight, the branch and whether it is a force delete.
    pending: Option<(OperationId, String, bool)>,
    outcome: Option<Outcome>,
}

impl Deleting {
    pub(crate) fn start(&mut self, id: OperationId, branch: String, force: bool) {
        self.pending = Some((id, branch, force));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _, _)| *pending == id)
    }

    /// Ends the delete with the operation's terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, branch, force)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Deleted(branch)
        } else {
            let message = detail.unwrap_or_else(|| "The branch was not deleted.".to_owned());
            if !force && is_unmerged(&message) {
                Outcome::Unmerged(message)
            } else {
                Outcome::Failed(message)
            }
        });
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the branch, Git's refusal once it has come,
/// whether the force checkbox is ticked, the focus that carries the keys, and
/// the error it shows.
pub(crate) struct DeleteDialog {
    pub(crate) branch: String,
    /// Git's refusal of the safe delete, once it has refused as unmerged.
    pub(crate) refusal: Option<String>,
    pub(crate) confirmed: bool,
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the dialog for the local branch `branch`.
    pub(crate) fn open_delete_branch_dialog(
        &mut self,
        branch: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.delete_dialog = Some(DeleteDialog {
            branch: branch.to_owned(),
            refusal: None,
            confirmed: false,
            focus,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_delete_branch_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.delete_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Space and a click on the checkbox: ticks or unticks the force delete,
    /// when Git has refused and no delete is in flight.
    pub(crate) fn toggle_delete_force(&mut self, cx: &mut Context<Self>) {
        if self.model.branch_delete.is_pending() {
            return;
        }
        if let Some(dialog) = self
            .delete_dialog
            .as_mut()
            .filter(|dialog| dialog.refusal.is_some())
        {
            dialog.confirmed = !dialog.confirmed;
            cx.notify();
        }
    }

    /// Enter and the delete button: sends the safe delete, or the force delete
    /// once the checkbox is ticked after Git's refusal.
    pub(crate) fn submit_delete_branch(&mut self, cx: &mut Context<Self>) {
        if self.model.branch_delete.is_pending() {
            return;
        }
        let Some(dialog) = self.delete_dialog.as_mut() else {
            return;
        };
        if !may_delete(dialog.refusal.as_deref(), dialog.confirmed) {
            cx.notify();
            return;
        }
        dialog.error = None;
        let branch = dialog.branch.clone();
        let force = dialog.refusal.is_some();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::DeleteBranch {
                name: branch.clone(),
                force,
            },
        );
        self.model.branch_delete.start(id, branch, force);
        cx.notify();
    }

    /// Runs when the engine settles a delete. A deleted branch closes the
    /// dialog and says so; an unmerged refusal shows Git's message and the
    /// force checkbox; any other failure stays in the dialog.
    pub(crate) fn on_branch_delete_settled(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Deleted(branch) => {
                self.delete_dialog = None;
                self.list_focus.focus(window);
                self.request_branch_list();
                self.notify_user(&format!("Deleted branch {branch}"), cx);
            }
            Outcome::Unmerged(message) => match self.delete_dialog.as_mut() {
                Some(dialog) => {
                    dialog.refusal = Some(message);
                    dialog.confirmed = false;
                    dialog.error = None;
                }
                None => self.notify_user(&message, cx),
            },
            Outcome::Failed(message) => match self.delete_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn delete_branch_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.delete_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_delete_branch_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div().absolute().inset_0().child(outside).child(
            self.delete_panel(dialog, left, cx)
                // Tab has nothing else to reach while the dialog is open.
                .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
        )
    }

    fn delete_panel(&self, dialog: &DeleteDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.branch_delete.is_pending();
        let allowed = !pending && may_delete(dialog.refusal.as_deref(), dialog.confirmed);
        let body: Vec<String> = match dialog.refusal.as_deref() {
            Some(message) => message.lines().map(str::to_owned).collect(),
            None => vec![SAFE_BODY.to_owned()],
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
                    "escape" => this.close_delete_branch_dialog(window, cx),
                    "enter" => this.submit_delete_branch(cx),
                    "space" => this.toggle_delete_force(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Delete branch {}?", dialog.branch)),
            )
            .children(body.into_iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(if dialog.refusal.is_some() {
                        theme::error()
                    } else {
                        theme::text()
                    })
                    .child(line)
            }))
            .children(
                dialog
                    .refusal
                    .is_some()
                    .then(|| force_row(dialog.confirmed, cx)),
            )
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(pending, allowed, cx))
    }
}

/// The checkbox that allows the force delete. Space and a click tick it.
fn force_row(confirmed: bool, cx: &Context<StatusView>) -> Stateful<Div> {
    let mark = if confirmed {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id("branch-delete-force-check")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child(CONFIRM_FORCE),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.toggle_delete_force(cx);
        }))
}

/// Cancel, and the destructive delete button, which waits for the checkbox
/// after Git's refusal and for an answer while a delete runs.
fn buttons(pending: bool, allowed: bool, cx: &Context<StatusView>) -> Div {
    let delete: Stateful<Div> = div()
        .id("branch-delete-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::error())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!allowed, |this| this.opacity(0.5))
        .when(allowed, gpui::Styled::cursor_pointer)
        .child(if pending {
            "Deleting…"
        } else {
            "Delete branch"
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_delete_branch(cx);
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
                .id("branch-delete-cancel")
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
                        this.close_delete_branch_dialog(window, cx);
                    }),
                ),
        )
        .child(delete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unmerged_refusal_is_recognised_and_other_failures_are_not() {
        let refusal = "error: The branch 'wip' is not fully merged.\n\
            If you are sure you want to delete it, run 'git branch -D wip'.";
        assert!(is_unmerged(refusal));
        assert!(!is_unmerged("error: branch 'wip' not found."));
        assert!(!is_unmerged("Cannot delete the checked-out branch main."));
    }

    #[test]
    fn the_safe_delete_needs_no_checkbox_and_the_force_needs_it_ticked() {
        assert!(may_delete(None, false));
        assert!(!may_delete(Some("not fully merged"), false));
        assert!(may_delete(Some("not fully merged"), true));
    }

    #[test]
    fn a_delete_is_tracked_only_while_its_operation_runs() {
        let mut deleting = Deleting::default();
        deleting.start(OperationId(7), "wip".into(), false);
        assert!(deleting.is_pending());
        assert!(!deleting.owns(OperationId(6)));
        assert!(deleting.owns(OperationId(7)));
        deleting.settle(OperationState::Done, None);
        assert_eq!(deleting.take(), Some(Outcome::Deleted("wip".to_owned())));
        assert!(!deleting.is_pending());
        assert_eq!(deleting.take(), None);
    }

    #[test]
    fn a_refused_safe_delete_is_an_unmerged_outcome_with_gits_message() {
        let mut deleting = Deleting::default();
        deleting.start(OperationId(7), "wip".into(), false);
        deleting.settle(
            OperationState::Failed,
            Some("error: The branch 'wip' is not fully merged.".into()),
        );
        assert_eq!(
            deleting.take(),
            Some(Outcome::Unmerged(
                "error: The branch 'wip' is not fully merged.".to_owned()
            ))
        );
    }

    #[test]
    fn a_refused_force_delete_is_a_plain_failure_and_so_is_any_other_refusal() {
        let mut deleting = Deleting::default();
        deleting.start(OperationId(8), "wip".into(), true);
        deleting.settle(
            OperationState::Failed,
            Some("error: The branch 'wip' is not fully merged.".into()),
        );
        assert!(matches!(deleting.take(), Some(Outcome::Failed(_))));

        deleting.start(OperationId(9), "main".into(), false);
        deleting.settle(
            OperationState::Failed,
            Some("Cannot delete the checked-out branch main.".into()),
        );
        assert_eq!(
            deleting.take(),
            Some(Outcome::Failed(
                "Cannot delete the checked-out branch main.".to_owned()
            ))
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_so() {
        let mut deleting = Deleting::default();
        deleting.start(OperationId(4), "wip".into(), false);
        deleting.settle(OperationState::Vetoed, None);
        assert_eq!(
            deleting.take(),
            Some(Outcome::Failed("The branch was not deleted.".to_owned()))
        );
    }
}
