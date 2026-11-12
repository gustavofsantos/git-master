//! The Remove Workspace dialog (D183 (4); D31's dialog pattern). The engine inspects the workspace
//! when the dialog opens and checks the removal again when it is sent; this module only shows what
//! the inspection says and sends what the user confirms. The rules are pure and tested here: which
//! blockers the dialog lists, when Remove is enabled, what the checkbox names and what an outcome
//! means. The window side is the `impl StatusView` at the bottom. docs/guide/workspaces.md,
//! "Remove a workspace".

use std::path::Path;

use gm_schema::{
    Actor, CommandKind, OperationId, OperationState, RemovalBlocker, RemovalBlockerKind,
};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::icon::{self, Icon};
use crate::theme;
use crate::view::StatusView;
use crate::workspace_list::{self, AfterRemoval};

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// Shown while the engine has not answered the inspection yet.
const CHECKING: &str = "Checking the workspace…";

/// Whether the removal may be sent with the confirmation `confirmed` (the ticked checkbox). A
/// workspace with no blocker goes at once; one whose blockers can all be overridden needs the
/// confirmation; any other blocker refuses the removal whatever is ticked.
#[must_use]
pub(crate) fn may_remove(blockers: &[RemovalBlocker], confirmed: bool) -> bool {
    if blockers.iter().any(|blocker| !blocker.overridable) {
        return false;
    }
    blockers.is_empty() || confirmed
}

/// Whether the dialog asks for the confirmation: there are blockers, and every one of them can be
/// overridden. With a blocker that cannot be overridden the dialog only says why.
#[must_use]
pub(crate) fn asks_confirmation(blockers: &[RemovalBlocker]) -> bool {
    !blockers.is_empty() && blockers.iter().all(|blocker| blocker.overridable)
}

/// What the checkbox names: each kind of work the removal discards, in the order the blockers list it.
#[must_use]
pub(crate) fn confirmation_label(blockers: &[RemovalBlocker]) -> String {
    let mut lost: Vec<&'static str> = Vec::new();
    for blocker in blockers {
        let what = match blocker.kind {
            RemovalBlockerKind::MemberDirty => "uncommitted changes",
            RemovalBlockerKind::MemberDetached => "commits that no branch holds",
            RemovalBlockerKind::WorkspaceChanges => "changes of the workspace itself",
            RemovalBlockerKind::Current | RemovalBlockerKind::MemberRepositoryMissing => continue,
        };
        if !lost.contains(&what) {
            lost.push(what);
        }
    }
    if lost.is_empty() {
        return "I understand this removes the workspace folder".to_owned();
    }
    format!(
        "I understand this discards the {} listed above",
        lost.join(" and ")
    )
}

/// The lines the dialog shows under its title: one per blocker, with the member's name in front of
/// its reason, or, when there are none, what the removal does.
#[must_use]
pub(crate) fn body_lines(path: &str, blockers: &[RemovalBlocker]) -> Vec<String> {
    if blockers.is_empty() {
        return vec![format!(
            "Each member's worktree is removed from its clone, then the folder {path}. The branches stay in your clones."
        )];
    }
    blockers
        .iter()
        .map(|blocker| match &blocker.member {
            Some(member) => format!("{member}: {}", blocker.detail),
            None => blocker.detail.clone(),
        })
        .collect()
}

/// The removal in flight or settled, as the model keeps it. Events of other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceRemoval {
    /// The workspace the dialog is about.
    path: Option<String>,
    /// The operation the dialog waits for, and what it is.
    pending: Option<(OperationId, Phase)>,
    /// The blockers of the inspection of `path`, once the engine answered.
    inspection: Option<Vec<RemovalBlocker>>,
    /// `Ok` with the removed path, or `Err` with the reason, until the view takes it.
    outcome: Option<Result<String, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Inspecting,
    Removing,
}

impl WorkspaceRemoval {
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

    /// Keeps the blockers when the inspection is about the workspace the dialog shows.
    pub(crate) fn inspected(&mut self, path: &str, blockers: Vec<RemovalBlocker>) {
        if self.path.as_deref() == Some(path) {
            self.inspection = Some(blockers);
        }
    }

    /// The blockers of the inspection of `path`, once the engine has answered.
    pub(crate) fn inspection(&self, path: &str) -> Option<&[RemovalBlocker]> {
        if self.path.as_deref() == Some(path) {
            self.inspection.as_deref()
        } else {
            None
        }
    }

    /// Ends the operation with its terminal `state`. An inspection that finished is nothing to
    /// show; a failed one or a failed removal gives the engine's first reason.
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
                    "The workspace could not be read.".to_owned()
                })))
            }
            (Phase::Removing, OperationState::Done) => Some(Ok(path)),
            (Phase::Removing, _) => {
                Some(Err(detail.unwrap_or_else(|| {
                    "The workspace was not removed.".to_owned()
                })))
            }
        };
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the workspace, whether its confirmation is ticked, the focus that
/// carries the keys, and the error it shows.
pub(crate) struct RemoveWorkspaceDialog {
    pub(crate) path: String,
    /// The window shows this workspace: the removal runs with `closing` and closes the window
    /// (D186 (3)).
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the dialog for the workspace at `path` and asks the engine what removing it would lose.
    pub(crate) fn open_remove_workspace_dialog(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closing = workspace_list::is_own_window(
            path,
            self.model.status().map(|status| status.root.as_str()),
        );
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::InspectWorkspaceRemoval {
                path: path.to_owned(),
                closing,
            },
        );
        self.model
            .workspace_removal
            .begin_inspect(id, path.to_owned());
        let focus = cx.focus_handle();
        focus.focus(window);
        self.workspace_removal_dialog = Some(RemoveWorkspaceDialog {
            path: path.to_owned(),
            closing,
            confirmed: false,
            focus,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_remove_workspace_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_removal_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Space and a click on the checkbox: ticks or unticks the confirmation, when the dialog asks
    /// for one and no removal is in flight.
    pub(crate) fn toggle_workspace_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.model.workspace_removal.is_removing() {
            return;
        }
        let asks = self
            .workspace_removal_dialog
            .as_ref()
            .and_then(|dialog| self.model.workspace_removal.inspection(&dialog.path))
            .is_some_and(asks_confirmation);
        if let Some(dialog) = self.workspace_removal_dialog.as_mut().filter(|_| asks) {
            dialog.confirmed = !dialog.confirmed;
            cx.notify();
        }
    }

    /// Enter and the remove button: checks the confirmation against the inspection, then sends the
    /// removal. Forced only when there are blockers.
    pub(crate) fn submit_workspace_removal(&mut self, cx: &mut Context<Self>) {
        if self.model.workspace_removal.is_removing() {
            return;
        }
        let Some(dialog) = self.workspace_removal_dialog.as_mut() else {
            return;
        };
        let confirmed = dialog.confirmed;
        let closing = dialog.closing;
        let path = dialog.path.clone();
        let Some(blockers) = self.model.workspace_removal.inspection(&path) else {
            dialog.error = Some(CHECKING.to_owned());
            cx.notify();
            return;
        };
        dialog.error = None;
        // A refusal is not an error text: the dialog already says what blocks it.
        if !may_remove(blockers, confirmed) {
            cx.notify();
            return;
        }
        // The checkbox is the force (D31): it is ticked only when blockers were listed.
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::RemoveWorkspace {
                path,
                force: confirmed,
                closing,
            },
        );
        self.model.workspace_removal.begin_remove(id);
        cx.notify();
    }

    /// Runs when the engine settles the removal. A removal closes the dialog, refreshes the list and
    /// says so; a failure stays in the dialog with the engine's reason.
    pub(crate) fn on_workspace_removal_settled(
        &mut self,
        outcome: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(path) => {
                // The window's own repository went with its folder: the window goes back to the root
                // it showed before, when that folder is still there (D234 (4)), and otherwise closes;
                // the last window quits the app (D186 (3)). Another workspace keeps the window open.
                let here = self.model.status().map(|status| status.root.clone());
                if workspace_list::after_removal(&path, here.as_deref())
                    == AfterRemoval::CloseWindow
                {
                    let previous = here
                        .as_deref()
                        .and_then(|root| {
                            self.kept_views.previous_root(root).map(|prev| (root, prev))
                        })
                        .filter(|(_, prev)| Path::new(prev).is_dir());
                    let Some((removed, previous)) = previous else {
                        window.remove_window();
                        return;
                    };
                    self.kept_views.forget(removed);
                    self.workspace_removal_dialog = None;
                    self.request_workspaces();
                    self.open_worktree(&previous, cx);
                    cx.notify();
                    return;
                }
                self.workspace_removal_dialog = None;
                self.list_focus.focus(window);
                self.request_workspaces();
                self.notify_user(&format!("Removed workspace at {path}"), cx);
            }
            Err(message) => match self.workspace_removal_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn workspace_removal_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.workspace_removal_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_remove_workspace_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div().absolute().inset_0().child(outside).child(
            self.workspace_removal_panel(dialog, left, cx)
                // Tab has nothing else to reach while the dialog is open.
                .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
        )
    }

    fn workspace_removal_panel(
        &self,
        dialog: &RemoveWorkspaceDialog,
        left: f32,
        cx: &Context<Self>,
    ) -> Div {
        let pending = self.model.workspace_removal.is_removing();
        let blockers = self.model.workspace_removal.inspection(&dialog.path);
        let name = crate::workspace_list::folder_name(&dialog.path);
        let allowed = blockers.is_some_and(|found| may_remove(found, dialog.confirmed));
        let forced = blockers.is_some_and(asks_confirmation);
        let label = blockers.map(confirmation_label).unwrap_or_default();
        let body = blockers.map_or_else(
            || vec![CHECKING.to_owned()],
            |found| body_lines(&dialog.path, found),
        );
        let refused = blockers.is_some_and(|found| {
            !found.is_empty() && found.iter().any(|blocker| !blocker.overridable)
        });
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
                    "escape" => this.close_remove_workspace_dialog(window, cx),
                    "enter" => this.submit_workspace_removal(cx),
                    "space" => this.toggle_workspace_confirmation(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Remove workspace {name}?")),
            )
            .children(
                body.into_iter()
                    .map(|line| div().text_sm().text_color(theme::text()).child(line)),
            )
            .children(forced.then(|| confirm_row(dialog.confirmed, label, cx)))
            .children(
                dialog
                    .error
                    .clone()
                    .filter(|_| !refused)
                    .map(|error| div().text_sm().text_color(theme::error()).child(error)),
            )
            .child(buttons(pending, allowed, cx))
    }
}

/// The checkbox that confirms a forced removal. Space and a click tick it.
fn confirm_row(confirmed: bool, label: String, cx: &Context<StatusView>) -> Stateful<Div> {
    let mark = if confirmed {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id("workspace-remove-confirm-check")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(div().text_sm().text_color(theme::text()).child(label))
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.toggle_workspace_confirmation(cx);
        }))
}

/// Cancel, and the destructive remove button, enabled only when the rules allow.
fn buttons(pending: bool, allowed: bool, cx: &Context<StatusView>) -> Div {
    let remove: Stateful<Div> = div()
        .id("workspace-remove-confirm")
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
            "Remove workspace"
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_workspace_removal(cx);
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
                .id("workspace-remove-cancel")
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
                        this.close_remove_workspace_dialog(window, cx);
                    }),
                ),
        )
        .child(remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocker(
        kind: RemovalBlockerKind,
        member: Option<&str>,
        overridable: bool,
    ) -> RemovalBlocker {
        RemovalBlocker {
            kind,
            member: member.map(str::to_owned),
            detail: format!("{kind:?} detail"),
            overridable,
        }
    }

    #[test]
    fn a_workspace_with_no_blocker_is_removed_without_a_confirmation() {
        assert!(may_remove(&[], false));
        assert!(!asks_confirmation(&[]));
        assert_eq!(
            body_lines("/w/billing", &[]),
            vec![
                "Each member's worktree is removed from its clone, then the folder /w/billing. The branches stay in your clones."
                    .to_owned()
            ]
        );
    }

    #[test]
    fn overridable_blockers_need_the_confirmation_ticked() {
        let dirty = [blocker(
            RemovalBlockerKind::MemberDirty,
            Some("billing"),
            true,
        )];
        assert!(asks_confirmation(&dirty));
        assert!(!may_remove(&dirty, false));
        assert!(may_remove(&dirty, true));
    }

    #[test]
    fn a_non_overridable_blocker_refuses_the_removal_even_when_confirmed() {
        let current = [blocker(RemovalBlockerKind::Current, None, false)];
        assert!(!asks_confirmation(&current));
        assert!(!may_remove(&current, false));
        assert!(!may_remove(&current, true));
        let mixed = [
            blocker(RemovalBlockerKind::MemberDirty, Some("billing"), true),
            blocker(
                RemovalBlockerKind::MemberRepositoryMissing,
                Some("checkout"),
                false,
            ),
        ];
        assert!(!asks_confirmation(&mixed));
        assert!(!may_remove(&mixed, true));
    }

    #[test]
    fn the_checkbox_names_what_is_discarded_once_per_kind() {
        let all = [
            blocker(RemovalBlockerKind::MemberDirty, Some("billing"), true),
            blocker(RemovalBlockerKind::MemberDetached, Some("checkout"), true),
            blocker(RemovalBlockerKind::MemberDirty, Some("checkout"), true),
        ];
        assert_eq!(
            confirmation_label(&all),
            "I understand this discards the uncommitted changes and commits that no branch holds listed above"
        );
        let workspace = [blocker(RemovalBlockerKind::WorkspaceChanges, None, true)];
        assert_eq!(
            confirmation_label(&workspace),
            "I understand this discards the changes of the workspace itself listed above"
        );
    }

    #[test]
    fn each_blocker_is_a_line_with_its_member_in_front() {
        let blockers = [
            blocker(RemovalBlockerKind::MemberDirty, Some("billing"), true),
            blocker(RemovalBlockerKind::WorkspaceChanges, None, true),
        ];
        assert_eq!(
            body_lines("/w/x", &blockers),
            ["billing: MemberDirty detail", "WorkspaceChanges detail"]
        );
    }

    #[test]
    fn a_removal_that_is_inspected_is_kept_only_for_its_workspace() {
        let mut removal = WorkspaceRemoval::default();
        removal.begin_inspect(OperationId(2), "/w/billing".to_owned());
        assert!(removal.owns(OperationId(2)));
        removal.inspected("/w/other", Vec::new());
        assert!(removal.inspection("/w/billing").is_none());
        removal.inspected(
            "/w/billing",
            vec![blocker(
                RemovalBlockerKind::MemberDirty,
                Some("billing"),
                true,
            )],
        );
        assert_eq!(removal.inspection("/w/billing").map(<[_]>::len), Some(1));
        removal.settle(OperationState::Done, None);
        assert!(removal.take_outcome().is_none());
        assert!(!removal.owns(OperationId(2)));
    }

    #[test]
    fn a_removal_that_succeeds_names_the_path_and_one_that_fails_keeps_the_reason() {
        let mut removal = WorkspaceRemoval::default();
        removal.begin_inspect(OperationId(1), "/w/billing".to_owned());
        removal.settle(OperationState::Done, None);
        removal.begin_remove(OperationId(2));
        assert!(removal.is_removing());
        removal.settle(OperationState::Done, None);
        assert_eq!(removal.take_outcome(), Some(Ok("/w/billing".to_owned())));
        assert!(!removal.is_removing());

        removal.begin_remove(OperationId(3));
        removal.settle(
            OperationState::Failed,
            Some("no workspace was removed: this window shows it".to_owned()),
        );
        assert_eq!(
            removal.take_outcome(),
            Some(Err(
                "no workspace was removed: this window shows it".to_owned()
            ))
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_so() {
        let mut removal = WorkspaceRemoval::default();
        removal.begin_remove(OperationId(4));
        removal.settle(OperationState::Failed, None);
        assert_eq!(
            removal.take_outcome(),
            Some(Err("The workspace was not removed.".to_owned()))
        );
    }
}
