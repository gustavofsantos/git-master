//! The discard dialog. Discarding a file's changes throws them away for good, so
//! the dialog asks first and says what will be lost (D93). The rules that say
//! what is lost are pure and tested here; the window side is the
//! `impl StatusView` at the bottom.

use gm_schema::{Actor, Change, CommandKind, OperationId, OperationState, StatusEntry};
use gpui::{
    Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The last line of every dialog: a discard cannot be taken back.
const IRREVERSIBLE: &str = "This cannot be undone.";

/// The title: which file the discard is about, and whether it takes the staged
/// changes too.
#[must_use]
pub(crate) fn title(path: &str, include_staged: bool) -> String {
    if include_staged {
        format!("Discard all changes to {path}?")
    } else {
        format!("Discard changes to {path}?")
    }
}

/// The label of the destructive button.
#[must_use]
pub(crate) fn confirm_label(include_staged: bool) -> &'static str {
    if include_staged {
        "Discard all changes"
    } else {
        "Discard changes"
    }
}

/// The plain English lines of what the discard loses, the last one saying it
/// cannot be undone. It follows what `GitPort::discard_file` does for each
/// kind of entry, with the staged changes only when `include_staged` is set.
#[must_use]
pub(crate) fn summary(entry: &StatusEntry, include_staged: bool) -> Vec<String> {
    let path = entry.path.as_str();
    let staged_stays = if entry.staged.is_some() && !include_staged {
        "; the staged changes stay"
    } else {
        ""
    };
    let mut lines = Vec::new();
    if entry.untracked {
        lines.push(if path.ends_with('/') {
            format!("The untracked folder {path} and everything in it will be deleted.")
        } else {
            format!("The untracked file {path} will be deleted.")
        });
    } else if include_staged && entry.staged == Some(Change::Added) {
        lines.push(format!(
            "The new file {path} will be removed from the index and deleted."
        ));
    } else if include_staged && entry.staged == Some(Change::Renamed) {
        let from = entry.orig_path.as_deref().unwrap_or(path);
        lines.push(format!("The rename {from} → {path} will be undone."));
    } else if include_staged && entry.staged.is_some() {
        lines.push(if entry.unstaged.is_some() {
            format!("Staged and unstaged changes to {path} will be discarded.")
        } else {
            format!("Staged changes to {path} will be discarded.")
        });
    } else if entry.unstaged == Some(Change::Added) {
        // An intent-to-add file has no content in the index to return to.
        lines.push(format!(
            "The new file {path} will be removed from the index and deleted."
        ));
    } else if entry.unstaged == Some(Change::Deleted) {
        lines.push(format!(
            "The deleted file {path} will be restored from the index{staged_stays}."
        ));
    } else if entry.unstaged.is_some() {
        lines.push(format!(
            "Unstaged changes to {path} will be discarded{staged_stays}."
        ));
    } else {
        lines.push(format!("Nothing in {path} will be discarded."));
    }
    lines.push(IRREVERSIBLE.to_owned());
    lines
}

/// The discard in flight, and its outcome until the window takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The changes are discarded; the path is the file's.
    Discarded(String),
    /// Git or the engine refused, with the message to show.
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Discarding {
    /// The operation in flight and the file it is for.
    pending: Option<(OperationId, String)>,
    outcome: Option<Outcome>,
}

impl Discarding {
    pub(crate) fn start(&mut self, id: OperationId, path: String) {
        self.pending = Some((id, path));
        self.outcome = None;
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    /// Ends the discard with the operation's terminal `state`.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, path)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            Outcome::Discarded(path)
        } else {
            let message = detail.unwrap_or_else(|| "The changes were not discarded.".to_owned());
            Outcome::Failed(message)
        });
    }

    pub(crate) fn take(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the file as the list had it when the dialog
/// opened, whether the staged changes go too, the focus that carries the keys,
/// and the error it shows.
pub(crate) struct DiscardDialog {
    pub(crate) entry: StatusEntry,
    pub(crate) include_staged: bool,
    pub(crate) focus: FocusHandle,
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the discard dialog for the file at `path`. A file the list no
    /// longer has gets a notice and no dialog.
    pub(crate) fn open_discard_file_dialog(
        &mut self,
        path: &str,
        include_staged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .model
            .entries()
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
        else {
            self.notify_user("That file is not in the working copy", cx);
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window);
        self.discard_dialog = Some(DiscardDialog {
            entry,
            include_staged,
            focus,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_discard_file_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.discard_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Enter and the discard button: sends the discard with `Actor::Human`.
    pub(crate) fn submit_discard_file(&mut self, cx: &mut Context<Self>) {
        if self.model.discarding.is_pending() {
            return;
        }
        let Some(dialog) = self.discard_dialog.as_mut() else {
            return;
        };
        dialog.error = None;
        let path = dialog.entry.path.clone();
        let include_staged = dialog.include_staged;
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::DiscardFile {
                path: path.clone(),
                include_staged,
            },
        );
        self.model.discarding.start(id, path);
        cx.notify();
    }

    /// Runs when the engine settles a discard. A discarded file closes the
    /// dialog and says so; a failure stays in the dialog with Git's message.
    pub(crate) fn on_file_discard_settled(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Discarded(path) => {
                self.discard_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&format!("Discarded changes to {path}"), cx);
            }
            Outcome::Failed(message) => match self.discard_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn discard_file_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.discard_dialog.as_ref()?;
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_discard_file_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        Some(
            div().absolute().inset_0().child(outside).child(
                self.discard_panel(dialog, left, cx)
                    // Tab has nothing else to reach while the dialog is open.
                    .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
                    .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation()),
            ),
        )
    }

    fn discard_panel(&self, dialog: &DiscardDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.discarding.is_pending();
        let lines = summary(&dialog.entry, dialog.include_staged);
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
                    "escape" => this.close_discard_file_dialog(window, cx),
                    "enter" => this.submit_discard_file(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title(&dialog.entry.path, dialog.include_staged)),
            )
            .children(
                lines
                    .into_iter()
                    .map(|line| div().text_sm().text_color(theme::text()).child(line)),
            )
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(dialog.include_staged, pending, cx))
    }
}

/// Cancel, and the destructive discard button, which waits while a discard runs.
fn buttons(include_staged: bool, pending: bool, cx: &Context<StatusView>) -> Div {
    let label = if pending {
        "Discarding…"
    } else {
        confirm_label(include_staged)
    };
    let discard = div()
        .id("discard-file-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::error())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(pending, |this| this.opacity(0.5))
        .when(!pending, gpui::Styled::cursor_pointer)
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_discard_file(cx);
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
                .id("discard-file-cancel")
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
                        this.close_discard_file_dialog(window, cx);
                    }),
                ),
        )
        .child(discard)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: "a.txt".into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    fn lines(entry: &StatusEntry, include_staged: bool) -> Vec<String> {
        summary(entry, include_staged)
    }

    #[test]
    fn the_last_line_says_the_discard_cannot_be_undone() {
        let modified = entry(None, Some(Change::Modified));
        for include_staged in [false, true] {
            assert_eq!(
                lines(&modified, include_staged).last().map(String::as_str),
                Some(IRREVERSIBLE)
            );
        }
    }

    #[test]
    fn the_title_and_the_button_name_the_mode() {
        assert_eq!(title("src/a.rs", false), "Discard changes to src/a.rs?");
        assert_eq!(title("src/a.rs", true), "Discard all changes to src/a.rs?");
        assert_eq!(confirm_label(false), "Discard changes");
        assert_eq!(confirm_label(true), "Discard all changes");
    }

    #[test]
    fn a_modified_file_loses_its_unstaged_changes_in_both_modes() {
        let modified = entry(None, Some(Change::Modified));
        assert_eq!(
            lines(&modified, false),
            ["Unstaged changes to a.txt will be discarded.", IRREVERSIBLE]
        );
        assert_eq!(
            lines(&modified, true),
            ["Unstaged changes to a.txt will be discarded.", IRREVERSIBLE]
        );
    }

    #[test]
    fn a_deleted_file_is_restored_from_the_index() {
        let deleted = entry(None, Some(Change::Deleted));
        assert_eq!(
            lines(&deleted, false),
            [
                "The deleted file a.txt will be restored from the index.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn an_untracked_file_is_deleted_in_both_modes() {
        let untracked = StatusEntry {
            untracked: true,
            ..entry(None, None)
        };
        for include_staged in [false, true] {
            assert_eq!(
                lines(&untracked, include_staged),
                ["The untracked file a.txt will be deleted.", IRREVERSIBLE]
            );
        }
    }

    #[test]
    fn an_untracked_folder_is_named_with_its_slash_and_its_contents() {
        let folder = StatusEntry {
            path: "build/".into(),
            untracked: true,
            ..entry(None, None)
        };
        assert_eq!(
            lines(&folder, false),
            [
                "The untracked folder build/ and everything in it will be deleted.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn a_new_file_staged_is_removed_from_the_index_and_the_disk_in_all_mode() {
        let added = entry(Some(Change::Added), None);
        assert_eq!(
            lines(&added, true),
            [
                "The new file a.txt will be removed from the index and deleted.",
                IRREVERSIBLE
            ]
        );
        assert_eq!(
            lines(&added, false),
            ["Nothing in a.txt will be discarded.", IRREVERSIBLE]
        );
    }

    #[test]
    fn a_new_file_with_unstaged_edits_keeps_the_staged_part_in_the_plain_mode() {
        let added = entry(Some(Change::Added), Some(Change::Modified));
        assert_eq!(
            lines(&added, false),
            [
                "Unstaged changes to a.txt will be discarded; the staged changes stay.",
                IRREVERSIBLE
            ]
        );
        assert_eq!(
            lines(&added, true),
            [
                "The new file a.txt will be removed from the index and deleted.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn an_intent_to_add_file_is_removed_whole() {
        let intent = entry(None, Some(Change::Added));
        assert_eq!(
            lines(&intent, false),
            [
                "The new file a.txt will be removed from the index and deleted.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn staged_and_unstaged_changes_are_named_in_all_mode_and_split_in_the_plain_one() {
        let both = entry(Some(Change::Modified), Some(Change::Modified));
        assert_eq!(
            lines(&both, true),
            [
                "Staged and unstaged changes to a.txt will be discarded.",
                IRREVERSIBLE
            ]
        );
        assert_eq!(
            lines(&both, false),
            [
                "Unstaged changes to a.txt will be discarded; the staged changes stay.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn staged_changes_alone_are_only_named_in_all_mode() {
        let staged = entry(Some(Change::Modified), None);
        assert_eq!(
            lines(&staged, true),
            ["Staged changes to a.txt will be discarded.", IRREVERSIBLE]
        );
        assert_eq!(
            lines(&staged, false),
            ["Nothing in a.txt will be discarded.", IRREVERSIBLE]
        );
    }

    #[test]
    fn a_deleted_file_with_staged_changes_keeps_them_in_the_plain_mode() {
        let both = entry(Some(Change::Modified), Some(Change::Deleted));
        assert_eq!(
            lines(&both, false),
            [
                "The deleted file a.txt will be restored from the index; the staged changes stay.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn a_rename_is_undone_in_all_mode_with_both_paths() {
        let renamed = StatusEntry {
            path: "new.txt".into(),
            orig_path: Some("old.txt".into()),
            ..entry(Some(Change::Renamed), None)
        };
        assert_eq!(
            lines(&renamed, true),
            ["The rename old.txt → new.txt will be undone.", IRREVERSIBLE]
        );
    }

    #[test]
    fn a_rename_with_unstaged_edits_only_loses_the_edits_in_the_plain_mode() {
        let renamed = StatusEntry {
            path: "new.txt".into(),
            orig_path: Some("old.txt".into()),
            ..entry(Some(Change::Renamed), Some(Change::Modified))
        };
        assert_eq!(
            lines(&renamed, false),
            [
                "Unstaged changes to new.txt will be discarded; the staged changes stay.",
                IRREVERSIBLE
            ]
        );
    }

    #[test]
    fn a_discard_is_tracked_only_while_its_operation_runs() {
        let mut discarding = Discarding::default();
        discarding.start(OperationId(3), "a.txt".into());
        assert!(discarding.is_pending());
        assert!(!discarding.owns(OperationId(2)));
        assert!(discarding.owns(OperationId(3)));
        discarding.settle(OperationState::Executing, None);
        assert!(discarding.is_pending());
        assert_eq!(discarding.take(), None);
        discarding.settle(OperationState::Done, None);
        assert_eq!(
            discarding.take(),
            Some(Outcome::Discarded("a.txt".to_owned()))
        );
        assert!(!discarding.is_pending());
        assert_eq!(discarding.take(), None);
    }

    #[test]
    fn a_refused_discard_carries_gits_message() {
        let mut discarding = Discarding::default();
        discarding.start(OperationId(5), "a.txt".into());
        discarding.settle(
            OperationState::Failed,
            Some("fatal: a.txt is conflicted: resolve the conflict first".into()),
        );
        assert_eq!(
            discarding.take(),
            Some(Outcome::Failed(
                "fatal: a.txt is conflicted: resolve the conflict first".to_owned()
            ))
        );
    }

    #[test]
    fn a_failure_without_a_reason_says_so() {
        let mut discarding = Discarding::default();
        discarding.start(OperationId(6), "a.txt".into());
        discarding.settle(OperationState::Vetoed, None);
        assert_eq!(
            discarding.take(),
            Some(Outcome::Failed(
                "The changes were not discarded.".to_owned()
            ))
        );
    }
}
