//! The delete-merged-branches dialog. It asks the engine for the local branches
//! merged into the base branch, shows them, and deletes exactly that list when
//! the user confirms: `git branch -d` on each, so Git stays the last guard. A
//! branch Git refuses is reported with its message and the others still go.
//! The rules and the dialog state are pure and tested here; the window side is
//! the `impl StatusView` at the bottom.

use gm_schema::{
    Actor, BranchFailure, CommandKind, MERGED_LIST_CHANGED, MergedBranch, OperationId,
    OperationState,
};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Stateful, Window, div, prelude::*, px,
};

use crate::theme;
use crate::view::StatusView;

const WIDTH: f32 = 520.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The most branch rows drawn. The rest are counted, and still deleted when confirmed.
pub(crate) const ROWS_SHOWN: usize = 15;
const CHECKING: &str = "Checking which branches are merged…";
const NOTHING: &str = "No merged branches to delete.";
const NO_BASE: &str = "No base branch was found to compare with.";
const SQUASH_NOTE: &str =
    "Squash-merged and rebase-merged branches are not detected: Git cannot see them as merged.";
const INTRO: &str = "Each branch is deleted with git branch -d, so Git refuses any branch \
that is not merged into HEAD.";
const CHANGED_NOTICE: &str = "The merged branches changed, so the list was read again.";

/// Whether the delete button may be pressed: the list is in, not empty, and nothing is in flight.
#[must_use]
pub(crate) fn can_delete(listing: Option<&[MergedBranch]>, busy: bool) -> bool {
    !busy && listing.is_some_and(|found| !found.is_empty())
}

/// `Delete 3 branches`, `Delete 1 branch`, or the progress word while a delete runs.
#[must_use]
pub(crate) fn button_label(count: usize, busy: bool) -> String {
    if busy {
        "Deleting…".to_owned()
    } else {
        format!("Delete {count} {}", branches(count))
    }
}

/// `branch` or `branches` for `count`.
#[must_use]
pub(crate) fn branches(count: usize) -> &'static str {
    if count == 1 { "branch" } else { "branches" }
}

/// The line above the list: the base the check used.
#[must_use]
pub(crate) fn base_line(base: Option<&str>) -> String {
    match base {
        Some(name) => format!("Merged into {name}"),
        None => NO_BASE.to_owned(),
    }
}

/// One branch as the list shows it: its name, short tip hash and subject.
#[must_use]
pub(crate) fn branch_line(branch: &MergedBranch) -> String {
    if branch.subject.is_empty() {
        format!("{}  {}", branch.name, branch.tip)
    } else {
        format!("{}  {}  {}", branch.name, branch.tip, branch.subject)
    }
}

/// The lines of the list: a placeholder while it is read, the empty message,
/// or the first `ROWS_SHOWN` branches.
#[must_use]
pub(crate) fn list_lines(listing: Option<&Listing>) -> Vec<String> {
    match listing {
        None => vec![CHECKING.to_owned()],
        Some(found) if found.branches.is_empty() => vec![NOTHING.to_owned()],
        Some(found) => found
            .branches
            .iter()
            .take(ROWS_SHOWN)
            .map(branch_line)
            .collect(),
    }
}

/// The line that counts the branches the list does not draw. They are still deleted.
#[must_use]
pub(crate) fn more_line(listing: Option<&Listing>) -> Option<String> {
    let total = listing?.branches.len();
    (total > ROWS_SHOWN).then(|| {
        format!(
            "…and {} more, all of them deleted when confirmed",
            total - ROWS_SHOWN
        )
    })
}

/// The list the dialog shows: the base the check used and the branches it would delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) base: Option<String>,
    pub(crate) branches: Vec<MergedBranch>,
}

/// What a finished delete or listing means for the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Every confirmed branch is gone: the dialog closes and says how many.
    Deleted(usize),
    /// Some branches are gone and Git refused the rest: the dialog stays and shows why.
    Partial {
        deleted: usize,
        failures: Vec<BranchFailure>,
    },
    /// The list changed after it was shown: it is read again.
    Changed,
    /// The listing or the delete was refused or failed, with the reason.
    Failed(String),
}

/// The listing and delete in flight, the list the dialog shows, and the outcome until taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Merging {
    /// The operation the dialog waits for, and what it is.
    pending: Option<(OperationId, Phase)>,
    /// The list the last listing showed; `None` until it arrives.
    listing: Option<Listing>,
    /// The counts of the last delete's report, until its operation settles.
    report: Option<(usize, Vec<BranchFailure>)>,
    outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Listing,
    Deleting,
}

impl Merging {
    pub(crate) fn begin_list(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Listing));
        self.listing = None;
        self.report = None;
        self.outcome = None;
    }

    pub(crate) fn begin_delete(&mut self, id: OperationId) {
        self.pending = Some((id, Phase::Deleting));
        self.report = None;
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

    pub(crate) fn listed(&mut self, base: Option<String>, branches: Vec<MergedBranch>) {
        self.listing = Some(Listing { base, branches });
    }

    pub(crate) fn listing(&self) -> Option<&Listing> {
        self.listing.as_ref()
    }

    /// The report of a delete: what was removed and what Git refused.
    pub(crate) fn reported(&mut self, deleted: usize, failures: Vec<BranchFailure>) {
        self.report = Some((deleted, failures));
    }

    /// Ends the operation with its terminal `state`. A finished listing is nothing
    /// to report; a delete gives its report, a changed list is read again, and
    /// any other refusal is Git's reason.
    pub(crate) fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, phase)) = self.pending.take() else {
            return;
        };
        self.outcome = match (phase, state) {
            (Phase::Listing, OperationState::Done) => None,
            (Phase::Listing, _) => {
                Some(Outcome::Failed(detail.unwrap_or_else(|| {
                    "The merged branches could not be listed.".to_owned()
                })))
            }
            (Phase::Deleting, OperationState::Done) => Some(match self.report.take() {
                Some((deleted, failures)) if failures.is_empty() => Outcome::Deleted(deleted),
                Some((deleted, failures)) => Outcome::Partial { deleted, failures },
                None => Outcome::Failed("The branches were not reported.".to_owned()),
            }),
            (Phase::Deleting, _) if detail.as_deref() == Some(MERGED_LIST_CHANGED) => {
                Some(Outcome::Changed)
            }
            (Phase::Deleting, _) => Some(Outcome::Failed(
                detail.unwrap_or_else(|| "No branch was deleted.".to_owned()),
            )),
        };
    }

    pub(crate) fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

/// The dialog while it is open: the focus that carries the keys, what the last
/// delete refused, the notice and the error it shows.
pub(crate) struct MergedDialog {
    pub(crate) focus: FocusHandle,
    pub(crate) failures: Vec<BranchFailure>,
    /// A short line such as the list having been read again.
    pub(crate) notice: Option<String>,
    pub(crate) error: Option<String>,
}

/// The dialog's text for a finished delete that is only partly done.
#[must_use]
pub(crate) fn partial_notice(deleted: usize, refused: usize) -> String {
    format!(
        "Deleted {deleted} {}; Git refused {refused}.",
        branches(deleted)
    )
}

/// The notice for a changed list, shown once the list is read again.
#[must_use]
pub(crate) fn changed_notice() -> String {
    CHANGED_NOTICE.to_owned()
}

impl StatusView {
    /// Opens the dialog and asks the engine for the branches a delete would remove.
    pub(crate) fn open_merged_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        focus.focus(window);
        self.merged_dialog = Some(MergedDialog {
            focus,
            failures: Vec::new(),
            notice: None,
            error: None,
        });
        self.request_merged_list(cx);
    }

    /// Asks the engine for the merged list and waits for it in the dialog.
    fn request_merged_list(&mut self, cx: &mut Context<Self>) {
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListMergedBranches);
        self.model.merging.begin_list(id);
        cx.notify();
    }

    pub(crate) fn close_merged_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.merged_dialog.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// The delete button, and Enter: sends the delete with exactly the branches the list showed.
    pub(crate) fn submit_delete_merged(&mut self, cx: &mut Context<Self>) {
        let busy = self.model.merging.is_busy();
        let Some(listing) = self.model.merging.listing() else {
            return;
        };
        if !can_delete(Some(&listing.branches), busy) {
            return;
        }
        let names: Vec<String> = listing
            .branches
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::DeleteMergedBranches { branches: names },
        );
        self.model.merging.begin_delete(id);
        if let Some(dialog) = self.merged_dialog.as_mut() {
            dialog.error = None;
            dialog.notice = None;
        }
        cx.notify();
    }

    /// Runs when the engine settles a listing or a delete. A full delete closes the
    /// dialog and says so. A partial one keeps it open with each refusal, and reads
    /// the list again. A changed list is read again with a notice. Anything else
    /// stays in the dialog.
    pub(crate) fn on_merged_settled(
        &mut self,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Outcome::Deleted(count) => {
                self.merged_dialog = None;
                self.list_focus.focus(window);
                self.notify_user(&format!("Deleted {count} {}", branches(count)), cx);
            }
            Outcome::Partial { deleted, failures } => {
                let refused = failures.len();
                match self.merged_dialog.as_mut() {
                    Some(dialog) => {
                        dialog.notice = Some(partial_notice(deleted, refused));
                        dialog.failures = failures;
                        dialog.error = None;
                        self.request_merged_list(cx);
                    }
                    None => self.notify_user(&partial_notice(deleted, refused), cx),
                }
            }
            Outcome::Changed => {
                if let Some(dialog) = self.merged_dialog.as_mut() {
                    dialog.notice = Some(changed_notice());
                    dialog.error = None;
                    self.request_merged_list(cx);
                }
            }
            Outcome::Failed(message) => match self.merged_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    pub(crate) fn merged_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.merged_dialog.as_ref() else {
            return div();
        };
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_merged_dialog(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(self.merged_panel(dialog, left, cx))
    }

    fn merged_panel(&self, dialog: &MergedDialog, left: f32, cx: &Context<Self>) -> Div {
        let busy = self.model.merging.is_busy();
        let listing = self.model.merging.listing();
        let enabled = can_delete(listing.map(|found| found.branches.as_slice()), busy);
        let count = listing.map_or(0, |found| found.branches.len());
        let base = listing.map_or_else(
            || CHECKING.to_owned(),
            |found| base_line(found.base.as_deref()),
        );
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
                    "escape" => this.close_merged_dialog(window, cx),
                    "enter" => this.submit_delete_merged(cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Delete merged branches?"),
            )
            .child(div().text_sm().text_color(theme::muted()).child(INTRO))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(SQUASH_NOTE),
            )
            .child(div().text_sm().text_color(theme::text()).child(base))
            .children(list_lines(listing).into_iter().map(|line| {
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .font_family(theme::code_font())
                    .child(line)
            }))
            .children(
                more_line(listing)
                    .map(|line| div().text_sm().text_color(theme::muted()).child(line)),
            )
            .children(dialog.failures.iter().map(|failure| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(format!("{}: {}", failure.name, failure.message))
            }))
            .children(dialog.notice.as_ref().map(|notice| {
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(notice.clone())
            }))
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .text_sm()
                    .text_color(theme::error())
                    .child(error.clone())
            }))
            .child(buttons(count, busy, enabled, cx))
    }
}

/// Cancel, and the destructive delete, which waits for a listed, non-empty list
/// and for nothing else in flight.
fn buttons(count: usize, busy: bool, enabled: bool, cx: &Context<StatusView>) -> Div {
    let delete: Stateful<Div> = div()
        .id("merged-branches-confirm")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::error())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!enabled, |this| this.opacity(0.5))
        .when(enabled, gpui::Styled::cursor_pointer)
        .child(button_label(count, busy))
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.submit_delete_merged(cx);
        }));
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("merged-branches-cancel")
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
                        this.close_merged_dialog(window, cx);
                    }),
                ),
        )
        .child(delete)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, subject: &str) -> MergedBranch {
        MergedBranch {
            name: name.to_owned(),
            tip: "abc1234".to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn listing(count: usize) -> Listing {
        Listing {
            base: Some("main".to_owned()),
            branches: (0..count)
                .map(|n| branch(&format!("topic-{n}"), "work"))
                .collect(),
        }
    }

    #[test]
    fn delete_needs_a_listed_non_empty_list_and_nothing_in_flight() {
        let found = [branch("topic", "work")];
        assert!(!can_delete(None, false), "not listed yet");
        assert!(!can_delete(Some(&[]), false), "nothing merged");
        assert!(!can_delete(Some(&found), true), "a delete is in flight");
        assert!(can_delete(Some(&found), false));
    }

    #[test]
    fn the_button_counts_its_branches_and_says_when_it_is_deleting() {
        assert_eq!(button_label(0, false), "Delete 0 branches");
        assert_eq!(button_label(1, false), "Delete 1 branch");
        assert_eq!(button_label(3, false), "Delete 3 branches");
        assert_eq!(button_label(3, true), "Deleting…");
    }

    #[test]
    fn the_base_line_names_the_base_or_says_there_is_none() {
        assert_eq!(base_line(Some("develop")), "Merged into develop");
        assert_eq!(base_line(None), NO_BASE);
    }

    #[test]
    fn each_branch_is_one_line_with_its_hash_and_subject() {
        assert_eq!(
            branch_line(&branch("topic", "Fix the login form")),
            "topic  abc1234  Fix the login form"
        );
        assert_eq!(branch_line(&branch("topic", "")), "topic  abc1234");
    }

    #[test]
    fn the_list_says_so_when_it_is_read_or_empty_and_caps_its_rows() {
        assert_eq!(list_lines(None), vec![CHECKING.to_owned()]);
        let empty = Listing {
            base: Some("main".into()),
            branches: Vec::new(),
        };
        assert_eq!(list_lines(Some(&empty)), vec![NOTHING.to_owned()]);
        let short = listing(2);
        assert_eq!(list_lines(Some(&short)).len(), 2);
        assert_eq!(more_line(Some(&short)), None);
        let long = listing(ROWS_SHOWN + 3);
        assert_eq!(list_lines(Some(&long)).len(), ROWS_SHOWN);
        assert_eq!(
            more_line(Some(&long)).as_deref(),
            Some("…and 3 more, all of them deleted when confirmed")
        );
    }

    #[test]
    fn a_listing_is_kept_for_the_dialog_and_a_full_delete_reports_its_count() {
        let mut merging = Merging::default();
        merging.begin_list(OperationId(1));
        assert!(merging.owns(OperationId(1)));
        assert!(merging.listing().is_none());
        merging.listed(
            Some("main".into()),
            vec![branch("a", "x"), branch("b", "y")],
        );
        merging.settle(OperationState::Done, None);
        assert!(merging.take_outcome().is_none());
        assert_eq!(merging.listing().map(|l| l.branches.len()), Some(2));

        merging.begin_delete(OperationId(2));
        assert!(merging.is_busy());
        merging.reported(2, Vec::new());
        merging.settle(OperationState::Done, None);
        assert_eq!(merging.take_outcome(), Some(Outcome::Deleted(2)));
        assert!(!merging.is_busy());
    }

    #[test]
    fn a_partial_delete_keeps_each_refusal_with_gits_message() {
        let mut merging = Merging::default();
        merging.begin_delete(OperationId(5));
        let refusal = BranchFailure {
            name: "x".into(),
            message: "error: The branch 'x' is not fully merged.".into(),
        };
        merging.reported(1, vec![refusal.clone()]);
        merging.settle(OperationState::Done, None);
        assert_eq!(
            merging.take_outcome(),
            Some(Outcome::Partial {
                deleted: 1,
                failures: vec![refusal],
            })
        );
    }

    #[test]
    fn a_changed_list_is_read_again_and_any_other_refusal_is_gits_reason() {
        let mut merging = Merging::default();
        merging.begin_delete(OperationId(6));
        merging.settle(OperationState::Failed, Some(MERGED_LIST_CHANGED.to_owned()));
        assert_eq!(merging.take_outcome(), Some(Outcome::Changed));

        merging.begin_delete(OperationId(7));
        merging.settle(
            OperationState::Failed,
            Some("only a human can delete merged branches".into()),
        );
        assert_eq!(
            merging.take_outcome(),
            Some(Outcome::Failed(
                "only a human can delete merged branches".to_owned()
            ))
        );

        merging.begin_list(OperationId(8));
        merging.settle(OperationState::Failed, None);
        assert_eq!(
            merging.take_outcome(),
            Some(Outcome::Failed(
                "The merged branches could not be listed.".to_owned()
            ))
        );
    }

    #[test]
    fn the_notices_say_what_happened_in_plain_words() {
        assert_eq!(partial_notice(1, 1), "Deleted 1 branch; Git refused 1.");
        assert_eq!(partial_notice(0, 2), "Deleted 0 branches; Git refused 2.");
        assert!(changed_notice().contains("read again"));
    }
}
