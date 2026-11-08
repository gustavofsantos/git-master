//! The title bar's content, in three zones (UI redesign, slice 1): the repository name over
//! "on <branch> ▾" at the left; the selection and its context actions in the middle; the
//! sync button and, while the stack of HEAD is behind, Restack at the right. The buttons are
//! registry actions, so they read the same rules as the menus and the palette;
//! while the engine cannot run one, it shows disabled with the reason as a
//! hint. The pure parts (labels, the path shown, the targets) are tested here.

use gm_schema::Feature;
use gpui::{
    Context, Div, MouseButton, MouseDownEvent, Pixels, SharedString, Stateful, div, prelude::*, px,
};

use crate::action_registry::{self, Action, ArgKind, Args};
use crate::context_actions::{self, ContextActions};
use crate::context_menu::Target;
use crate::icon::{self, Icon};
use crate::model::{CommitPage, CommitPane};
use crate::sync_state::{self, Standing};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::window_chrome;

/// Height of the title bar: the name row over the subtitle row.
pub(crate) const HEIGHT: Pixels = px(44.);

/// The widest the selection's name in the middle zone gets; a longer one is cut at its end.
const SUBJECT_MAX_WIDTH: f32 = 280.;

/// The target a toolbar button runs on: the checked-out branch for the actions
/// that take one, nothing for the others. A detached HEAD gives an empty name,
/// which the rule of the action refuses.
pub(crate) fn toolbar_args(action: Action, head: Option<&str>) -> Args {
    match action.spec().args {
        ArgKind::Branch => Args::Branch(head.unwrap_or_default().to_owned()),
        _ => Args::None,
    }
}

impl StatusView {
    /// The left zone: the repository name (the topic picker's place until topics exist) over
    /// "on <branch> ▾", the branch picker.
    pub(crate) fn title_block(&self, cx: &Context<Self>) -> Div {
        div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(self.project_button(cx).h(px(22.)))
            .child(
                div()
                    .min_w(px(0.))
                    .pl_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_color(theme::muted())
                    .child("on")
                    .child(self.branch_button(cx).h(px(18.))),
            )
    }

    /// What is selected on the page on screen, with its context menu read from the registry.
    pub(crate) fn context_actions(&self) -> Option<ContextActions> {
        if self.screen == Screen::Inbox {
            return self.inbox_context_actions();
        }
        let (kind, target) = self.selection_target()?;
        let subject = match &target {
            Target::Commit { oid, subject } => {
                format!("{} {subject}", oid.get(..7).unwrap_or(oid))
            }
            Target::Thread { title, .. } => title.clone(),
            Target::Label { name, .. } => name.clone(),
            Target::Stash { index, .. } => crate::stashes::label(*index),
            Target::ReflogEntry { index, .. } => crate::reflog::label(*index),
            Target::ReviewComment { place, .. } => place.clone(),
            Target::File(path) | Target::CodeViewer { path, .. } => path.clone(),
            _ => String::new(),
        };
        let items = self.menu_items(&target);
        Some(context_actions::for_items(kind, subject, &items))
    }

    /// The Inbox item selected, with its own actions ranked as a menu's are (slice 6).
    fn inbox_context_actions(&self) -> Option<ContextActions> {
        let item = self.inbox_selected_item()?;
        let ctx = self.action_context("");
        let ranked = item
            .actions
            .iter()
            .filter(|(action, args)| action.enabled(args, &ctx).is_ok())
            .filter(|(action, _)| !action.destructive() && !action.asks_first())
            .take(context_actions::SHOWN)
            .map(|(action, args)| (*action, args.clone(), action.spec().title))
            .collect();
        Some(ContextActions {
            kind: "Item",
            subject: item.what.clone(),
            ranked,
            all: item.actions,
        })
    }

    /// The selection of the page on screen as a menu target, with the kind the top bar names.
    fn selection_target(&self) -> Option<(&'static str, Target)> {
        match self.screen {
            Screen::WorkingCopy => self
                .model
                .selected_path()
                .map(|path| ("File", Target::File(path.to_owned()))),
            Screen::History => match self.model.commit_pane(CommitPage::History) {
                CommitPane::Shown(detail) => Some((
                    "Commit",
                    Target::Commit {
                        oid: detail.oid.clone(),
                        subject: detail.subject.clone(),
                    },
                )),
                _ => None,
            },
            Screen::Reflog => self.model.reflog.selected_entry().map(|entry| {
                (
                    "Reflog entry",
                    Target::ReflogEntry {
                        index: entry.index,
                        oid: entry.oid.clone(),
                    },
                )
            }),
            Screen::Stashes => self.model.stashes.selected_entry().map(|entry| {
                (
                    "Stash",
                    Target::Stash {
                        index: entry.index,
                        oid: entry.oid.clone(),
                    },
                )
            }),
            Screen::Threads => self.model.threads.selected_summary().map(|thread| {
                (
                    "Thread",
                    Target::Thread {
                        uid: thread.uid.clone(),
                        title: thread.title.clone(),
                        starred: thread.starred,
                    },
                )
            }),
            Screen::Labels => self.model.labels.selected_label().map(|label| {
                (
                    "Label",
                    Target::Label {
                        id: label.id,
                        name: label.name.clone(),
                        attachments: label.attachments,
                    },
                )
            }),
            Screen::LocalReview => {
                let id = self.model.local_review.selected_id()?;
                let comment = self
                    .model
                    .review
                    .comments()?
                    .iter()
                    .find(|comment| comment.id == id)?;
                Some((
                    "Comment",
                    Target::ReviewComment {
                        id,
                        place: comment.path.clone(),
                    },
                ))
            }
            Screen::Code => {
                let code = self.code_ctx();
                code.file.map(|path| {
                    (
                        "File",
                        Target::CodeViewer {
                            path: path.to_owned(),
                            lines: code.lines,
                        },
                    )
                })
            }
            _ => None,
        }
    }

    /// The middle zone: "Selected", what is selected, up to three actions for it and "⋯ ⌘K",
    /// which opens the palette on the selection's whole menu.
    pub(crate) fn context_zone(&self, cx: &Context<Self>) -> Option<Div> {
        let actions = self.context_actions()?;
        let muted = theme::muted();
        let buttons = actions
            .ranked
            .iter()
            .enumerate()
            .map(|(index, (action, args, title))| {
                let (action, args) = (*action, args.clone());
                let primary = index == 0;
                div()
                    .id(SharedString::from(format!("context-action-{index}")))
                    .flex_none()
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme::hairline())
                    .cursor_pointer()
                    .when(primary, |button| {
                        button.bg(theme::selection()).text_color(theme::accent())
                    })
                    .hover(|style| style.bg(theme::selected_row()))
                    .child(*title)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.run_action(action, &args, window, cx);
                        }),
                    )
            });
        let title = format!("Actions for {}", actions.subject);
        let all = actions.all.clone();
        let more = div()
            .id("context-actions-more")
            .flex_none()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(muted)
            .hover(|style| style.bg(theme::selected_row()))
            .child("⋯")
            .child(action_registry::shortcut_text("⌘K"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.open_palette_scoped(title.clone(), all.clone(), window, cx);
                }),
            );
        Some(
            div()
                .min_w(px(0.))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child("Selected"),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .max_w(px(SUBJECT_MAX_WIDTH))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(format!("{}: {}", actions.kind, actions.subject)),
                )
                .children(buttons)
                .child(more),
        )
    }

    /// The right zone: the sync button, its menu, and Restack while the stack of HEAD is behind.
    pub(crate) fn toolbar(&self, cx: &Context<Self>) -> Div {
        let head = self.head_name();
        let behind = if self.features.is_enabled(Feature::Stacks) {
            sync_state::stack_behind(self.model.stacks.snapshot(), head.as_deref())
        } else {
            0
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .children(sync_state::restack_label(behind).map(|label| self.restack_button(label, cx)))
            .child(self.sync_button(cx))
    }

    /// The standing of the checked-out branch, for the sync button.
    pub(crate) fn standing(&self) -> Standing<'_> {
        let branch = self.model.status().map(|status| &status.branch);
        Standing {
            branch: branch.and_then(|branch| branch.head.as_deref()),
            upstream: branch.and_then(|branch| branch.upstream.as_deref()),
            ahead: branch.map_or(0, |branch| branch.ahead),
            behind: branch.map_or(0, |branch| branch.behind),
            running: self.model.remote.in_flight(),
        }
    }

    fn sync_button(&self, cx: &Context<Self>) -> Div {
        let sync = sync_state::sync(self.standing());
        let primary = sync.primary;
        let head = self.head_name();
        let reason = primary.and_then(|action| {
            action
                .enabled(
                    &toolbar_args(action, head.as_deref()),
                    &self.action_context(""),
                )
                .err()
        });
        let ink = if primary.is_none() || reason.is_some() {
            theme::muted()
        } else if sync.pending {
            theme::accent()
        } else {
            theme::text()
        };
        let main = div()
            .id("sync-button")
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .text_color(ink)
            .when(sync.pending, |button| button.bg(theme::selection()))
            .child(sync.label);
        let main = match (primary, reason) {
            (Some(action), None) => main
                .cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        let head = this.head_name();
                        let args = toolbar_args(action, head.as_deref());
                        this.run_action(action, &args, window, cx);
                    }),
                ),
            (_, Some(reason)) => main.cursor_default().tooltip(window_chrome::hint(reason)),
            (None, None) => main.cursor_default(),
        };
        let caret = div()
            .id("sync-menu")
            .h(px(26.))
            .px_1()
            .flex()
            .items_center()
            .border_l_1()
            .border_color(theme::hairline())
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon::view(Icon::CaretDown, icon::SMALL, theme::muted()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let head = this.head_name();
                    this.open_menu(Target::Sync { head }, event.position, cx);
                }),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .overflow_hidden()
            .child(main)
            .child(caret)
    }

    fn restack_button(&self, label: String, cx: &Context<Self>) -> Stateful<Div> {
        let head = self.head_name();
        let args = toolbar_args(Action::Restack, head.as_deref());
        let reason = Action::Restack
            .enabled(&args, &self.action_context(""))
            .err();
        let button = div()
            .id(Action::Restack.id())
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .text_color(theme::warn_fg())
            .bg(theme::warn_bg())
            .child(label);
        let button = match reason {
            None => button.cursor_pointer(),
            Some(reason) => button.cursor_default().tooltip(window_chrome::hint(reason)),
        };
        button.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                let head = this.head_name();
                let args = toolbar_args(Action::Restack, head.as_deref());
                this.run_action(Action::Restack, &args, window, cx);
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_registry::{Context as RuleContext, MessageCtx, WorkflowCtx};

    #[test]
    fn branch_actions_target_the_checked_out_branch() {
        assert_eq!(
            toolbar_args(Action::PushWithLease, Some("main")),
            Args::Branch("main".into())
        );
        assert_eq!(
            toolbar_args(Action::Restack, None),
            Args::Branch(String::new())
        );
        assert_eq!(toolbar_args(Action::Fetch, Some("main")), Args::None);
    }

    #[test]
    fn the_remote_buttons_are_enabled_on_a_branch_and_restack_needs_the_stacks() {
        let context = RuleContext {
            repo_open: true,
            selected_file: None,
            selected_thread: None,
            code: crate::action_registry::CodeCtx::default(),
            submodules: None,
            selected_label: None,
            selected_memory: None,
            features: gm_schema::FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            worktrees: &[],
            workspaces: &[],
            author_pictures: crate::action_registry::AuthorPictures::Hidden,
            entries: &[],
            staged_count: 0,
            message: "",
            current_branch: Some("main"),
            root: Some("/repo"),
            messages: MessageCtx::default(),
            workflows: WorkflowCtx::default(),
            remote_running: false,
            stashes: None,
            review: crate::action_registry::ReviewCtx::default(),
        };
        for action in [Action::Fetch, Action::Pull, Action::PushWithLease] {
            let args = toolbar_args(action, Some("main"));
            assert_eq!(action.enabled(&args, &context), Ok(()), "{action:?}");
        }
        let restack = toolbar_args(Action::Restack, Some("main"));
        // Restack works on a branch of a stack, so it waits for the stacks.
        assert_eq!(
            Action::Restack.enabled(&restack, &context),
            Err("The stacks are still being read.")
        );
        assert!(Action::Restack.available());
    }

    #[test]
    fn a_running_remote_operation_disables_the_three_buttons_and_labels_its_own() {
        let context = RuleContext {
            repo_open: true,
            selected_file: None,
            selected_thread: None,
            code: crate::action_registry::CodeCtx::default(),
            submodules: None,
            selected_label: None,
            selected_memory: None,
            features: gm_schema::FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            worktrees: &[],
            workspaces: &[],
            entries: &[],
            staged_count: 0,
            message: "",
            current_branch: Some("main"),
            root: Some("/repo"),
            messages: MessageCtx::default(),
            workflows: WorkflowCtx::default(),
            author_pictures: crate::action_registry::AuthorPictures::Hidden,
            remote_running: true,
            stashes: None,
            review: crate::action_registry::ReviewCtx::default(),
        };
        for action in [Action::Fetch, Action::Pull, Action::PushWithLease] {
            let args = toolbar_args(action, Some("main"));
            assert_eq!(
                action.enabled(&args, &context),
                Err("A remote operation is running"),
                "{action:?}"
            );
        }
    }
}
