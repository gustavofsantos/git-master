//! The title bar's content: the repository name over `branch · path` at the
//! left, and the remote and stack buttons at the right. The buttons are
//! registry actions, so they read the same rules as the menus and the palette;
//! while the engine cannot run one, it shows disabled with the reason as a
//! hint. The pure parts (labels, the path shown, the targets) are tested here.

use gpui::{Context, Div, MouseButton, MouseDownEvent, Pixels, Stateful, div, prelude::*, px};

use crate::action_registry::{Action, ArgKind, Args};
use crate::theme;
use crate::view::StatusView;
use crate::window_chrome;

/// Height of the title bar: the name row over the subtitle row.
pub(crate) const HEIGHT: Pixels = px(44.);

/// The widest the upstream name in the header gets; a longer one is cut at its end.
pub(crate) const UPSTREAM_MAX_WIDTH: f32 = 220.;

/// The buttons on the right of the title bar, left to right.
pub(crate) const TOOLBAR: [Action; 4] = [
    Action::Fetch,
    Action::Pull,
    Action::PushWithLease,
    Action::Restack,
];

/// What a toolbar button says.
pub(crate) fn label(action: Action) -> &'static str {
    match action {
        Action::Fetch => "Fetch",
        Action::Pull => "Pull",
        Action::PushWithLease => "Push with lease",
        Action::Restack => "Restack",
        _ => action.spec().title,
    }
}

/// What a remote button says while its operation runs.
pub(crate) fn busy_label(action: Action) -> &'static str {
    match action {
        Action::Fetch => "Fetching…",
        Action::Pull => "Pulling…",
        _ => "Pushing…",
    }
}

/// The target a toolbar button runs on: the checked-out branch for the actions
/// that take one, nothing for the others. A detached HEAD gives an empty name,
/// which the rule of the action refuses.
pub(crate) fn toolbar_args(action: Action, head: Option<&str>) -> Args {
    match action.spec().args {
        ArgKind::Branch => Args::Branch(head.unwrap_or_default().to_owned()),
        _ => Args::None,
    }
}

/// The repository path as the subtitle shows it: under the home directory it
/// starts with `~`, as in the design.
pub(crate) fn display_path(root: &str, home: Option<&str>) -> String {
    let Some(home) = home.map(|home| home.trim_end_matches('/')) else {
        return root.to_owned();
    };
    if home.is_empty() {
        return root.to_owned();
    }
    if root == home {
        "~".to_owned()
    } else if let Some(rest) = root.strip_prefix(home).filter(|rest| rest.starts_with('/')) {
        format!("~{rest}")
    } else {
        root.to_owned()
    }
}

impl StatusView {
    /// The repository name, with `branch · path` under it. The branch is the
    /// branch picker, as it was in the header before.
    pub(crate) fn title_block(&self, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let root = self
            .model
            .status()
            .map(|status| status.root.clone())
            .unwrap_or_default();
        let home = std::env::var("HOME").ok();
        let path = display_path(&root, home.as_deref());
        div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(self.project_button(cx).h(px(22.)))
            .child(
                div()
                    .min_w(px(0.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.branch_button(cx).h(px(18.)))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(muted)
                            .child(format!("· {path}")),
                    ),
            )
    }

    /// The toolbar buttons at the right of the title bar.
    pub(crate) fn toolbar(&self, cx: &Context<Self>) -> Div {
        TOOLBAR
            .into_iter()
            .filter(|action| action.offered(self.features))
            .fold(
                div().flex().flex_row().items_center().gap_1(),
                |row, action| row.child(self.toolbar_button(action, cx)),
            )
    }

    fn toolbar_button(&self, action: Action, cx: &Context<Self>) -> Stateful<Div> {
        let head = self
            .model
            .status()
            .and_then(|status| status.branch.head.as_deref());
        let args = toolbar_args(action, head);
        let reason = action.enabled(&args, &self.action_context("")).err();
        // Restack is the one accent of the bar: amber, as in the design.
        let amber = action == Action::Restack;
        let ink = if amber {
            theme::warn_fg()
        } else if reason.is_some() {
            theme::muted()
        } else {
            theme::text()
        };
        // The button of the operation in flight says so; the other two stay disabled.
        let text = if self.model.remote.in_flight() == Some(action) {
            busy_label(action)
        } else {
            label(action)
        };
        let button = div()
            .id(action.id())
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .text_color(ink)
            .when(amber, |button| button.bg(theme::warn_bg()))
            .child(text);
        let button = match reason {
            None => button
                .cursor_pointer()
                .hover(|style| style.bg(theme::selected_row())),
            Some(reason) => button.cursor_default().tooltip(window_chrome::hint(reason)),
        };
        button.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                let head = this
                    .model
                    .status()
                    .and_then(|status| status.branch.head.clone());
                let args = toolbar_args(action, head.as_deref());
                this.run_action(action, &args, window, cx);
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_registry::{Context as RuleContext, MessageCtx, WorkflowCtx};

    #[test]
    fn the_home_directory_shows_as_a_tilde() {
        assert_eq!(
            display_path("/home/u/work/ledger", Some("/home/u")),
            "~/work/ledger"
        );
        assert_eq!(display_path("/home/u", Some("/home/u")), "~");
    }

    #[test]
    fn a_path_outside_home_or_a_sibling_prefix_is_shown_whole() {
        assert_eq!(display_path("/srv/ledger", Some("/home/u")), "/srv/ledger");
        assert_eq!(
            display_path("/home/user2/ledger", Some("/home/user")),
            "/home/user2/ledger"
        );
        assert_eq!(display_path("/home/u/ledger", None), "/home/u/ledger");
        assert_eq!(display_path("/home/u/ledger", Some("")), "/home/u/ledger");
    }

    #[test]
    fn the_toolbar_order_is_fetch_pull_push_restack() {
        assert_eq!(
            TOOLBAR.map(label),
            ["Fetch", "Pull", "Push with lease", "Restack"]
        );
    }

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
            features: gm_schema::FeatureSet::all(),
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            worktrees: &[],
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
            features: gm_schema::FeatureSet::all(),
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            worktrees: &[],
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
        assert_eq!(busy_label(Action::Fetch), "Fetching…");
        assert_eq!(busy_label(Action::Pull), "Pulling…");
        assert_eq!(busy_label(Action::PushWithLease), "Pushing…");
    }
}
