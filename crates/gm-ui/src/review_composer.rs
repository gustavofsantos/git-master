//! The review comment composer (B4c, D96): a small dialog that writes a comment
//! on lines of the Working Copy diff, or edits the body of one. The rules are
//! pure and tested here; the window side is the `impl StatusView` at the bottom.
//! Enter adds a line, Ctrl+Enter saves, Escape cancels. A refused write shows
//! the engine's message in the dialog.

use gm_schema::{Actor, CommandKind};
use gpui::{
    App, Context, Div, Entity, FontWeight, KeyBinding, MouseButton, MouseDownEvent, Stateful,
    Subscription, Window, actions, div, prelude::*, px,
};

use crate::FocusNext;
use crate::action_registry::{CommentSpot, Effect, shortcut_text};
use crate::diff_pane::PaneSpot;
use crate::diff_rows::Side;
use crate::review::{self, range_label};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::view::{Screen, StatusView};

actions!(gitmaster_review, [SaveReviewComment]);

/// The key context the save key is bound under.
const KEY_CONTEXT: &str = "ReviewComposer";
const WIDTH: f32 = 560.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;

/// What the dialog writes: a new comment on the lines of `path`, or the body of
/// comment `id`, whose lines are shown with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReviewTarget {
    New {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// A new comment on text picked in a surface that is not the working tree
    /// (D141): the engine looks for the text in the file now.
    NewByText { path: String, text: String },
    Edit {
        id: i64,
        path: String,
        start_line: u32,
        end_line: u32,
    },
}

impl ReviewTarget {
    fn path(&self) -> &str {
        match self {
            ReviewTarget::New { path, .. }
            | ReviewTarget::NewByText { path, .. }
            | ReviewTarget::Edit { path, .. } => path,
        }
    }

    /// The lines the dialog names: the target's own, or for picked text one to its length.
    fn lines(&self) -> (u32, u32) {
        match self {
            ReviewTarget::NewByText { text, .. } => (
                1,
                u32::try_from(text.lines().count().max(1)).unwrap_or(u32::MAX),
            ),
            ReviewTarget::New {
                start_line,
                end_line,
                ..
            }
            | ReviewTarget::Edit {
                start_line,
                end_line,
                ..
            } => (*start_line, *end_line),
        }
    }
}

/// The body a comment carries: trimmed, and none when it is blank.
#[must_use]
pub(crate) fn body_of(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Whether Comment or Save may be pressed: no write is in flight and the body is not blank.
#[must_use]
pub(crate) fn may_save(pending: bool, text: &str) -> bool {
    !pending && body_of(text).is_some()
}

/// The command the dialog sends for its target, with `Actor::Human`. None when the
/// body is blank.
#[must_use]
pub(crate) fn command(target: &ReviewTarget, text: &str) -> Option<CommandKind> {
    let body = body_of(text)?;
    Some(match target {
        ReviewTarget::New {
            path,
            start_line,
            end_line,
        } => CommandKind::AddReviewComment {
            path: path.clone(),
            start_line: *start_line,
            end_line: *end_line,
            body,
        },
        ReviewTarget::NewByText { path, text } => CommandKind::AddReviewCommentByText {
            path: path.clone(),
            text: text.clone(),
            body,
        },
        ReviewTarget::Edit { id, .. } => CommandKind::EditReviewComment { id: *id, body },
    })
}

/// The dialog's title: what the comment is about.
#[must_use]
pub(crate) fn title(target: &ReviewTarget) -> String {
    let (start, end) = target.lines();
    let range = match target {
        ReviewTarget::NewByText { .. } if end > 1 => format!("{end} selected lines"),
        ReviewTarget::NewByText { .. } => "the selected line".to_owned(),
        _ => range_label(start, end),
    };
    match target {
        ReviewTarget::New { .. } | ReviewTarget::NewByText { .. } => format!("Comment on {range}"),
        ReviewTarget::Edit { .. } => format!("Edit comment on {range}"),
    }
}

/// The label of the button that writes the comment.
#[must_use]
pub(crate) fn save_label(target: &ReviewTarget) -> &'static str {
    match target {
        ReviewTarget::New { .. } | ReviewTarget::NewByText { .. } => "Comment",
        ReviewTarget::Edit { .. } => "Save",
    }
}

/// Registers the save key. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "secondary-enter",
        SaveReviewComment,
        Some(KEY_CONTEXT),
    )]);
}

/// The pane that gets the keyboard back when a dialog closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusReturn {
    /// The Working Copy file list. The Local review page has no focus handle of
    /// its own, so a dialog opened there falls back to this too.
    List,
    /// The diff pane, when it had the keyboard when the dialog opened.
    Diff,
}

/// Where focus goes back to: the diff pane when it had the keyboard when the
/// dialog opened, the file list otherwise.
#[must_use]
pub(crate) fn focus_return(diff_had_focus: bool) -> FocusReturn {
    if diff_had_focus {
        FocusReturn::Diff
    } else {
        FocusReturn::List
    }
}

/// The dialog while it is open: what it writes, the body field and the error it shows.
pub(crate) struct ReviewDialog {
    pub(crate) target: ReviewTarget,
    pub(crate) body: Entity<TextArea>,
    /// The engine's refusal of the last try.
    pub(crate) error: Option<String>,
    /// The text the dialog last saw. The field notifies on every caret blink,
    /// so an error is cleared only when the text really changed.
    seen: String,
    /// Where the keyboard goes when the dialog closes.
    return_focus: FocusReturn,
    _subscriptions: Vec<Subscription>,
}

impl ReviewDialog {
    fn text(&self, cx: &App) -> String {
        self.body.read(cx).text().to_owned()
    }
}

impl StatusView {
    /// Opens the dialog for `target`. An edit starts with the comment's body.
    pub(crate) fn open_review_composer(
        &mut self,
        target: ReviewTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = match &target {
            ReviewTarget::Edit { id, .. } => self
                .model
                .review
                .comment(*id)
                .map(|comment| comment.body.clone())
                .unwrap_or_default(),
            ReviewTarget::New { .. } | ReviewTarget::NewByText { .. } => String::new(),
        };
        let body = cx.new(|cx| TextArea::new("Comment", cx).with_lines(4, 12));
        body.update(cx, |area, cx| area.set_text(&initial, cx));
        let subscriptions = vec![
            cx.observe(&body, |this, _, cx| this.on_review_text(cx)),
            cx.subscribe_in(
                &body,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    if *event == TextAreaEvent::Cancel {
                        this.close_review_composer(window, cx);
                    }
                },
            ),
        ];
        let seen = body.read(cx).text().to_owned();
        // A dialog already open keeps where focus was when it opened.
        let return_focus = match self.review_dialog.as_ref() {
            Some(open) => open.return_focus,
            None => focus_return(self.diff_pane.read(cx).focus_handle().is_focused(window)),
        };
        self.review_dialog = Some(ReviewDialog {
            target,
            body: body.clone(),
            error: None,
            seen,
            return_focus,
            _subscriptions: subscriptions,
        });
        body.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_review_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.review_dialog.take() {
            self.restore_review_focus(dialog.return_focus, window, cx);
        }
        cx.notify();
    }

    /// Gives the keyboard back to the pane the dialog was opened from.
    fn restore_review_focus(&self, to: FocusReturn, window: &mut Window, cx: &App) {
        match to {
            FocusReturn::List => self.list_focus.focus(window),
            FocusReturn::Diff => window.focus(&self.diff_pane.read(cx).focus_handle()),
        }
    }

    /// The text changed: the error of the last try goes, since it may be about the old text.
    fn on_review_text(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.review_dialog.as_mut() else {
            return;
        };
        let text = dialog.text(cx);
        if dialog.seen == text {
            return;
        }
        dialog.seen = text;
        dialog.error = None;
        cx.notify();
    }

    /// Comment or Save, and Ctrl+Enter: sends the write with `Actor::Human`.
    pub(crate) fn submit_review_comment(&mut self, cx: &mut Context<Self>) {
        if self.model.review_write.is_pending() {
            return;
        }
        let Some(dialog) = self.review_dialog.as_mut() else {
            return;
        };
        let text = dialog.text(cx);
        if !may_save(false, &text) {
            return;
        }
        dialog.error = None;
        let Some(kind) = command(&dialog.target, &text) else {
            return;
        };
        let id = self.engine.dispatch(Actor::Human, kind);
        self.model.review_write.start(id);
        cx.notify();
    }

    /// Runs when the engine settles a write: success closes the dialog; a failure
    /// stays in it with the engine's message.
    pub(crate) fn on_review_written(
        &mut self,
        outcome: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(()) => {
                if let Some(dialog) = self.review_dialog.take() {
                    self.restore_review_focus(dialog.return_focus, window, cx);
                }
            }
            Err(message) => match self.review_dialog.as_mut() {
                Some(dialog) => dialog.error = Some(message),
                None => self.notify_user(&message, cx),
            },
        }
        cx.notify();
    }

    /// The dialog's layer, or nothing while it is closed. Clicks outside it do
    /// nothing, so a typed comment is not lost to a stray click.
    pub(crate) fn review_composer_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.review_dialog.as_ref()?;
        let blocker = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
            },
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let panel = self.review_panel(dialog, left, cx);
        Some(
            div()
                .absolute()
                .inset_0()
                .child(blocker)
                .child(panel.capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())),
        )
    }

    fn review_panel(&self, dialog: &ReviewDialog, left: f32, cx: &Context<Self>) -> Div {
        let pending = self.model.review_write.is_pending();
        let text = dialog.text(cx);
        let target = &dialog.target;
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
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &SaveReviewComment, _, cx| {
                this.submit_review_comment(cx);
            }))
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title(target)),
            )
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_color(theme::muted())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(target.path().to_owned()),
            )
            .child(dialog.body.clone())
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(format!("{} saves · Escape cancels", shortcut_text("⌘↵"))),
            )
            .children(dialog.error.as_ref().map(|error| {
                div()
                    .flex()
                    .flex_col()
                    .text_sm()
                    .text_color(theme::error())
                    .children(error.lines().map(|line| div().child(line.to_owned())))
            }))
            .child(buttons(target, pending, may_save(pending, &text), cx))
    }
}

/// Cancel, and the button that writes: it waits while a write runs or the body is blank.
fn buttons(target: &ReviewTarget, pending: bool, can_save: bool, cx: &Context<StatusView>) -> Div {
    let label = if pending {
        "Saving…"
    } else {
        save_label(target)
    };
    let save: Stateful<Div> = div()
        .id("review-save")
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .font_weight(FontWeight::MEDIUM)
        .when(!can_save, |this| this.opacity(0.5))
        .when(can_save, gpui::Styled::cursor_pointer)
        .child(label)
        .when(can_save, |this| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.submit_review_comment(cx);
                }),
            )
        });
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("review-cancel")
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
                        this.close_review_composer(window, cx);
                    }),
                ),
        )
        .child(save)
}

impl StatusView {
    /// What the registry reads of the lines picked to comment on (B4c, D141): in the
    /// Working Copy diff, the Code viewer, or a commit's, stash's or reflog entry's diff.
    /// Nothing elsewhere, or with no line picked.
    pub(crate) fn comment_spot<'a>(&'a self, cx: &'a App) -> CommentSpot<'a> {
        match self.screen {
            Screen::Code => return self.code_comment_spot(),
            Screen::History | Screen::Stashes | Screen::Reflog => {
                return match self.commit_diff_pane.read(cx).comment_spot() {
                    Some(spot) => stacked_spot(&spot),
                    None => CommentSpot::Nothing,
                };
            }
            Screen::WorkingCopy => {}
            _ => return CommentSpot::Nothing,
        }
        let Some(path) = self.model.selected_path() else {
            return CommentSpot::Nothing;
        };
        let Some(spot) = self.diff_pane.read(cx).comment_spot() else {
            return CommentSpot::Nothing;
        };
        if !spot.covered.has_line {
            return CommentSpot::Nothing;
        }
        match (spot.side, spot.covered.working) {
            // The unstaged new side is the working tree: its numbers are the file's.
            (Side::Unstaged, working) => CommentSpot::Lines { path, working },
            (Side::Staged, None) => CommentSpot::Lines {
                path,
                working: None,
            },
            (Side::Staged, Some(_)) if spot.picked.contiguous => CommentSpot::Text { path },
            (Side::Staged, Some(_)) => CommentSpot::Split,
        }
    }

    /// The text picked in the diff, for a comment found by its text (D141).
    pub(crate) fn comment_text(&self, cx: &App) -> Option<String> {
        match self.screen {
            Screen::History | Screen::Stashes | Screen::Reflog => {
                self.commit_diff_pane.read(cx).comment_text()
            }
            _ => self.diff_pane.read(cx).comment_text(),
        }
    }

    /// Opens the composer for what Comment on Selection planned: the working-tree
    /// lines, or the text picked in the diff.
    pub(crate) fn open_comment_composer(
        &mut self,
        effect: Effect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match effect {
            Effect::OpenReviewComposer {
                path,
                start_line,
                end_line,
            } => ReviewTarget::New {
                path,
                start_line,
                end_line,
            },
            Effect::OpenReviewComposerByText { path } => {
                let Some(text) = self.comment_text(cx) else {
                    self.notify_user("Select lines to comment on first.", cx);
                    return;
                };
                ReviewTarget::NewByText { path, text }
            }
            _ => return,
        };
        self.open_review_composer(target, window, cx);
    }

    /// Edit Comment: the composer on the comment's body, with its lines.
    pub(crate) fn open_review_edit(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(comment) = self.model.review.comment(id) else {
            self.notify_user("This comment is no longer listed.", cx);
            return;
        };
        let (start_line, end_line) = review::shown_range(comment);
        let target = ReviewTarget::Edit {
            id,
            path: comment.path.clone(),
            start_line,
            end_line,
        };
        self.open_review_composer(target, window, cx);
    }

    /// Gives the diff pane the comments the engine listed last, so it draws them.
    pub(crate) fn sync_review_comments(&mut self, cx: &mut Context<Self>) {
        let comments = self.model.review.comments().unwrap_or_default();
        self.diff_pane
            .update(cx, |pane, cx| pane.set_review_comments(comments, cx));
    }

    /// Asks the engine for the comments. Sent when a repository is opened and after
    /// every status refresh; the engine lists again after each write itself.
    pub(crate) fn request_review_list(&mut self) {
        if self.model.status().is_none() || self.model.is_opening() {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListReviewComments);
        self.model.review.request(id);
    }
}

/// The spot of lines picked in a commit's diff: found by their text in the file now.
fn stacked_spot<'a>(spot: &PaneSpot<'a>) -> CommentSpot<'a> {
    let Some(path) = spot.file else {
        return CommentSpot::Nothing;
    };
    if !spot.covered.has_line {
        return CommentSpot::Nothing;
    }
    if spot.picked.lines.is_empty() {
        // Only removed lines: the file has none of them.
        CommentSpot::Lines {
            path,
            working: None,
        }
    } else if spot.picked.contiguous {
        CommentSpot::Text { path }
    } else {
        CommentSpot::Split
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::ReviewWrite;
    use gm_schema::OperationState;

    fn new_target() -> ReviewTarget {
        ReviewTarget::New {
            path: "src/a.rs".into(),
            start_line: 12,
            end_line: 15,
        }
    }

    fn pane_spot<'a>(lines: &[&str], contiguous: bool, file: Option<&'a str>) -> PaneSpot<'a> {
        PaneSpot {
            side: Side::Unstaged,
            covered: crate::review::Covered {
                has_line: true,
                working: None,
            },
            picked: crate::review::Picked {
                lines: lines.iter().map(|line| (*line).to_owned()).collect(),
                contiguous,
            },
            file,
        }
    }

    #[test]
    fn lines_picked_in_a_commit_are_found_by_text_when_they_are_one_run_in_one_file() {
        let spot = |lines, contiguous, file| stacked_spot(&pane_spot(lines, contiguous, file));
        assert_eq!(
            spot(&["a"], true, Some("f.rs")),
            CommentSpot::Text { path: "f.rs" }
        );
        assert_eq!(spot(&["a", "b"], false, Some("f.rs")), CommentSpot::Split);
        assert_eq!(
            spot(&[], true, Some("f.rs")),
            CommentSpot::Lines {
                path: "f.rs",
                working: None
            }
        );
        assert_eq!(spot(&["a"], true, None), CommentSpot::Nothing);
    }

    #[test]
    fn a_blank_body_is_no_body_and_the_rest_is_trimmed() {
        assert_eq!(body_of(""), None);
        assert_eq!(body_of(" \n\t "), None);
        assert_eq!(body_of("  rename this\n  "), Some("rename this".to_owned()));
    }

    #[test]
    fn a_comment_on_picked_text_sends_the_text_and_names_the_lines_it_has() {
        let target = ReviewTarget::NewByText {
            path: "src/a.rs".into(),
            text: "one\ntwo".into(),
        };
        assert_eq!(
            command(&target, " check "),
            Some(CommandKind::AddReviewCommentByText {
                path: "src/a.rs".into(),
                text: "one\ntwo".into(),
                body: "check".into(),
            })
        );
        assert_eq!(title(&target), "Comment on 2 selected lines");
        assert_eq!(save_label(&target), "Comment");
    }

    #[test]
    fn save_waits_for_a_body_and_for_no_write_in_flight() {
        assert!(may_save(false, "text"));
        assert!(!may_save(false, "  \n"));
        assert!(!may_save(true, "text"));
    }

    #[test]
    fn a_new_comment_sends_the_lines_and_the_trimmed_body_as_the_human() {
        assert_eq!(
            command(&new_target(), "  check this \n"),
            Some(CommandKind::AddReviewComment {
                path: "src/a.rs".into(),
                start_line: 12,
                end_line: 15,
                body: "check this".into(),
            })
        );
        assert_eq!(command(&new_target(), " "), None);
    }

    #[test]
    fn an_edit_sends_the_id_and_the_new_body() {
        let edit = ReviewTarget::Edit {
            id: 7,
            path: "src/a.rs".into(),
            start_line: 1,
            end_line: 1,
        };
        assert_eq!(
            command(&edit, "better"),
            Some(CommandKind::EditReviewComment {
                id: 7,
                body: "better".into(),
            })
        );
    }

    #[test]
    fn the_title_and_the_button_say_what_the_write_does() {
        assert_eq!(title(&new_target()), "Comment on lines 12–15");
        assert_eq!(save_label(&new_target()), "Comment");
        let one = ReviewTarget::Edit {
            id: 1,
            path: "a".into(),
            start_line: 3,
            end_line: 3,
        };
        assert_eq!(title(&one), "Edit comment on line 3");
        assert_eq!(save_label(&one), "Save");
    }

    #[test]
    fn the_save_key_is_a_valid_key_binding() {
        // A malformed keystroke panics when bound at startup; this catches it earlier.
        let binding = KeyBinding::new("secondary-enter", SaveReviewComment, Some(KEY_CONTEXT));
        assert_eq!(binding.keystrokes().len(), 1);
    }

    #[test]
    fn focus_goes_back_to_the_diff_only_when_the_diff_had_it() {
        assert_eq!(focus_return(true), FocusReturn::Diff);
        assert_eq!(focus_return(false), FocusReturn::List);
    }

    #[test]
    fn a_write_settles_on_its_own_operation_and_a_refusal_keeps_the_message() {
        let mut write = ReviewWrite::default();
        write.start(gm_schema::OperationId(9));
        write.settle(
            OperationState::Failed,
            Some("path is outside the worktree".into()),
        );
        assert_eq!(
            write.take(),
            Some(Err("path is outside the worktree".to_owned()))
        );
    }
}
