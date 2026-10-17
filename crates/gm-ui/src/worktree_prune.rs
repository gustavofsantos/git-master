//! The prune dialog (slice 1c, W6; D31). It lists the worktree records that
//! `git worktree prune` would remove, from Git's dry run, and removes exactly
//! those when the user confirms. Folders are never touched, and locked
//! worktrees are not listed because Git keeps them. The rules are pure and
//! tested here; the window side is the `impl StatusView` at the bottom.

use gm_schema::{Actor, CommandKind, OperationId, OperationState, PruneCandidate};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px,
};

use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 480.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
const CHECKING: &str = "Checking which worktree records are stale…";
const NOTHING: &str = "No worktree record is stale: nothing to prune.";
const INTRO: &str = "Git removes the records of worktrees whose folders are gone. \
Folders are not touched, and locked worktrees are kept.";

/// Whether Prune may be sent: the list is in and not empty, and nothing is in flight.
#[must_use]
pub(crate) fn can_prune(candidates: Option<&[PruneCandidate]>, busy: bool) -> bool {
    !busy && candidates.is_some_and(|found| !found.is_empty())
}

/// The lines under the intro: one per record, or what the list says when it is empty.
#[must_use]
pub(crate) fn candidate_lines(candidates: Option<&[PruneCandidate]>) -> Vec<String> {
    match candidates {
        None => vec![CHECKING.to_owned()],
        Some([]) => vec![NOTHING.to_owned()],
        Some(found) => found
            .iter()
            .map(|candidate| {
                if candidate.reason.is_empty() {
                    candidate.name.clone()
                } else {
                    format!("{}: {}", candidate.name, candidate.reason)
                }
            })
            .collect(),
    }
}

/// The listing and the prune in flight, as the model keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pruning {
    /// The operation the dialog waits for, and what it is.
    pending: Option<(OperationId, Phase)>,
    /// The records the last listing showed; `None` until it arrives.
    candidates: Option<Vec<PruneCandidate>>,
    /// `Ok` with the number of records removed, or `Err` with the reason, until taken.
    outcome: Option<Result<usize, String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Listing,
    /// Pruning, with the number of records it was sent for.
    Pruning(usize),
}

impl Pruning {
    pub(crate) fn begin_list(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Listing));
        self.candidates = None;
        self.outcome = None;
    }

    pub(crate) fn begin_prune(&mut self, id: OperationId, count: usize) {
        self.pending = Some((id, Phase::Pruning(count)));
        self.outcome = None;
    }

    /// Whether an operation of this dialog is in flight.
    pub(crate) fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    pub(crate) fn listed(&mut self, candidates: Vec<PruneCandidate>) {
        self.candidates = Some(candidates);
    }

    pub(crate) fn candidates(&self) -> Option<&[PruneCandidate]> {
        self.candidates.as_deref()
    }

    /// Ends the operation with its terminal `state`. A listing that finished is
    /// nothing to report; a prune gives the count, or Git's first reason.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, phase)) = self.pending.take() else {
            return;
        };
        self.outcome =
            match (phase, state) {
                (Phase::Listing, OperationState::Done) => None,
                (Phase::Listing, _) => Some(Err(detail
                    .unwrap_or_else(|| "The worktree records could not be listed.".to_owned()))),
                (Phase::Pruning(count), OperationState::Done) => Some(Ok(count)),
                (Phase::Pruning(_), _) => {
                    Some(Err(detail.unwrap_or_else(|| {
                        "No worktree record was pruned.".to_owned()
                    })))
                }
            };
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Result<usize, String>> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the error it shows, if any.
#[derive(Debug, Default)]
pub(crate) struct PruneDialog {
    pub(crate) error: Option<String>,
}

impl StatusView {
    /// Opens the dialog and asks the engine for the records a prune would remove.
    pub(crate) fn open_prune_dialog(&mut self, cx: &mut Context<Self>) {
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListPrunable);
        self.model.pruning.begin_list(id);
        self.prune_dialog = Some(PruneDialog::default());
        cx.notify();
    }

    pub(crate) fn close_prune_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prune_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The Prune button: sends the prune with exactly the names the list showed.
    pub(crate) fn submit_prune(&mut self, cx: &mut Context<Self>) {
        let busy = self.model.pruning.is_busy();
        let Some(candidates) = self.model.pruning.candidates() else {
            return;
        };
        if !can_prune(Some(candidates), busy) {
            return;
        }
        let names: Vec<String> = candidates.iter().map(|c| c.name.clone()).collect();
        let count = names.len();
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::PruneWorktrees { names });
        self.model.pruning.begin_prune(id, count);
        if let Some(dialog) = self.prune_dialog.as_mut() {
            dialog.error = None;
        }
        cx.notify();
    }

    /// Closes the dialog on success and says how many records went; a failure
    /// stays in the dialog with Git's reason.
    pub(crate) fn on_worktrees_pruned(
        &mut self,
        outcome: Result<usize, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(count) => {
                self.prune_dialog = None;
                self.list_focus.focus(window);
                let plural = if count == 1 { "" } else { "s" };
                self.notify_user(&format!("Pruned {count} worktree record{plural}."), cx);
            }
            Err(message) => match self.prune_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn prune_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.prune_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_prune_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(self.prune_panel(dialog, left, cx))
    }

    fn prune_panel(&self, dialog: &PruneDialog, left: f32, cx: &Context<Self>) -> Div {
        let busy = self.model.pruning.is_busy();
        let candidates = self.model.pruning.candidates();
        let enabled = can_prune(candidates, busy);
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
                    .child("Prune worktrees?"),
            )
            .child(div().text_sm().text_color(theme::muted()).child(INTRO))
            .children(candidate_lines(candidates).into_iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .font_family(theme::code_font())
                    .child(line)
            }))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(busy, enabled, cx))
    }
}

/// Cancel, and Prune: enabled only with a listed, non-empty list and nothing in flight.
fn buttons(busy: bool, enabled: bool, cx: &Context<StatusView>) -> Div {
    let prune: Stateful<Div> = div()
        .id("worktree-prune-confirm")
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
        .child(if busy { "Pruning…" } else { "Prune" })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.submit_prune(cx);
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
                .id("worktree-prune-cancel")
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
                        this.close_prune_dialog(window, cx);
                    }),
                ),
        )
        .child(prune)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, reason: &str) -> PruneCandidate {
        PruneCandidate {
            name: name.to_owned(),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn prune_needs_a_listed_non_empty_list_and_nothing_in_flight() {
        let found = [record(
            "repo-feature",
            "gitdir file points to non-existent location",
        )];
        assert!(!can_prune(None, false), "not listed yet");
        assert!(!can_prune(Some(&[]), false), "nothing stale");
        assert!(!can_prune(Some(&found), true), "a prune is in flight");
        assert!(can_prune(Some(&found), false));
    }

    #[test]
    fn each_record_is_one_line_and_an_empty_list_says_so() {
        let found = [record("repo-feature", "gitdir gone"), record("repo-x", "")];
        assert_eq!(
            candidate_lines(Some(&found)),
            vec!["repo-feature: gitdir gone".to_owned(), "repo-x".to_owned()]
        );
        assert_eq!(candidate_lines(Some(&[])), vec![NOTHING.to_owned()]);
        assert_eq!(candidate_lines(None), vec![CHECKING.to_owned()]);
    }

    #[test]
    fn the_listing_is_kept_for_the_dialog_and_the_prune_reports_its_count() {
        let mut pruning = Pruning::default();
        pruning.begin_list(OperationId(1));
        assert!(pruning.owns(OperationId(1)));
        assert!(pruning.candidates().is_none());
        pruning.listed(vec![record("repo-feature", "gone")]);
        pruning.settle(OperationState::Done, None);
        assert!(pruning.take_outcome().is_none());
        assert_eq!(pruning.candidates().map(<[_]>::len), Some(1));

        pruning.begin_prune(OperationId(2), 1);
        assert!(pruning.is_busy());
        pruning.settle(OperationState::Done, None);
        assert_eq!(pruning.take_outcome(), Some(Ok(1)));
        assert!(!pruning.is_busy());
    }

    #[test]
    fn a_failed_prune_keeps_gits_reason_and_a_failed_listing_says_so() {
        let mut pruning = Pruning::default();
        pruning.begin_prune(OperationId(3), 2);
        pruning.settle(
            OperationState::Failed,
            Some("the worktrees to prune changed since they were listed".to_owned()),
        );
        assert_eq!(
            pruning.take_outcome(),
            Some(Err(
                "the worktrees to prune changed since they were listed".to_owned()
            ))
        );
        pruning.begin_list(OperationId(4));
        pruning.settle(OperationState::Failed, None);
        assert_eq!(
            pruning.take_outcome(),
            Some(Err("The worktree records could not be listed.".to_owned()))
        );
    }
}
