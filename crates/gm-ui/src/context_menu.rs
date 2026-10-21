//! The right-click menus of the sidebar's branch rows and the Working Copy's
//! file rows (slice 1c, item 3). The items come from the action registry: each
//! one names its action, its target and whether it runs now, so a menu offers
//! nothing that a key or a button would refuse. A click on an enabled item runs
//! it through `run_action`, with `Actor::Human`, like every other path.

use gpui::{
    Context, Div, MouseButton, MouseDownEvent, Pixels, Point, Stateful, Window, div, prelude::*, px,
};

use crate::action_registry::{Action, Args, Context as ActionContext, Reason};
use crate::theme;
use crate::view::StatusView;
use crate::window_chrome;

const ROW_HEIGHT: f32 = 24.;
/// The gap a separator takes, its line included.
const SEPARATOR_HEIGHT: f32 = 9.;
/// The padding above and below the items.
const PADDING: f32 = 8.;
/// The confirmation panel: its text and its two buttons.
const CONFIRM_HEIGHT: f32 = 2. * ROW_HEIGHT + PADDING;
const MENU_WIDTH: f32 = 300.;
/// The least space kept between a menu and the window's edges.
const MARGIN: f32 = 4.;

/// What a menu was opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Branch(String),
    File(String),
    /// A worktree, by its folder.
    Worktree(String),
}

/// One line of a menu: an action, the target it runs on, and whether it runs now.
/// Items of the same `group` sit together; a change of group draws a separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) action: Action,
    pub(crate) args: Args,
    pub(crate) group: u8,
    /// `Ok` when the item runs now; otherwise the reason, shown beside it.
    pub(crate) state: Result<(), Reason>,
}

impl Item {
    fn new(action: Action, args: Args, group: u8, ctx: &ActionContext<'_>) -> Self {
        let state = action.enabled(&args, ctx);
        Self {
            action,
            args,
            group,
            state,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.action.spec().title
    }

    pub(crate) fn destructive(&self) -> bool {
        self.action.destructive()
    }
}

/// The menu of a branch row, in the order of the design: checkout, the ways to
/// build on the branch, pushing, copying the name, then renaming and deleting.
/// Actions the engine cannot run yet are listed disabled.
pub(crate) fn branch_items(name: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    if let Some(worktree) = checked_out_in_worktree(name, ctx) {
        return worktree_branch_items(&worktree.path, name, ctx);
    }
    let target = Args::Branch(name.to_owned());
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![
        item(Action::CheckoutBranch, 0),
        item(Action::NewBranch, 1),
        item(Action::NewChildBranch, 1),
        item(Action::CreateWorktree, 1),
        item(Action::Restack, 1),
        item(Action::PushWithLease, 2),
        item(Action::CopyBranchName, 3),
        item(Action::RenameBranch, 4),
        item(Action::DeleteBranch, 4),
    ]
}

/// The menu of a file row: stage or unstage (both are listed, each disabled
/// when it has nothing to act on), discard, then copy the path and reveal it.
pub(crate) fn file_items(path: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::File(path.to_owned());
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![
        item(Action::StageFile, 0),
        item(Action::UnstageFile, 0),
        item(Action::DiscardFile, 1),
        item(Action::ViewFile, 2),
        item(Action::CopyFilePath, 2),
        item(Action::RevealFile, 2),
    ]
}

/// The linked worktree, not the main checkout, that has `name` checked out.
fn checked_out_in_worktree<'a>(
    name: &str,
    ctx: &ActionContext<'a>,
) -> Option<&'a gm_schema::WorktreeEntry> {
    ctx.worktrees
        .iter()
        .find(|entry| !entry.is_main && entry.branch.as_deref() == Some(name))
}

/// The branch menu of a branch checked out in the linked worktree at `path`
/// (W8): the worktree's own actions take the place of checkout, rename and
/// delete, which Git would refuse here anyway. Create Worktree is not offered
/// for a branch that already has one.
fn worktree_branch_items(path: &str, name: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let worktree = Args::Worktree(path.to_owned());
    let branch = Args::Branch(name.to_owned());
    vec![
        Item::new(Action::OpenWorktree, worktree.clone(), 0, ctx),
        Item::new(Action::RevealWorktree, worktree.clone(), 1, ctx),
        Item::new(Action::CopyWorktreePath, worktree.clone(), 1, ctx),
        Item::new(Action::NewBranch, branch.clone(), 2, ctx),
        Item::new(Action::NewChildBranch, branch.clone(), 2, ctx),
        Item::new(Action::Restack, branch, 2, ctx),
        Item::new(Action::RemoveWorktree, worktree, 3, ctx),
    ]
}

/// The menu of a worktree row: open it, reveal and copy its folder, lock or
/// unlock it, prune it when its folder is gone, then remove it. Remove opens
/// its own dialog, which is the confirmation.
pub(crate) fn worktree_items(path: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::Worktree(path.to_owned());
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    let entry = ctx.worktrees.iter().find(|entry| entry.path == path);
    let lock = if entry.is_some_and(|entry| entry.locked.is_some()) {
        Action::UnlockWorktree
    } else {
        Action::LockWorktree
    };
    let mut items = vec![
        item(Action::OpenWorktree, 0),
        item(Action::RevealWorktree, 1),
        item(Action::CopyWorktreePath, 1),
        item(lock, 2),
    ];
    if entry.is_some_and(|entry| entry.prunable.is_some()) {
        items.push(Item::new(Action::PruneWorktrees, Args::None, 3, ctx));
    }
    items.push(item(Action::RemoveWorktree, 4));
    items
}

/// The items split where the group changes, one slice per group.
pub(crate) fn sections(items: &[Item]) -> Vec<&[Item]> {
    items.chunk_by(|a, b| a.group == b.group).collect()
}

/// Where a menu of `size` opened at `at` sits in a window of `viewport`: under
/// the pointer, pulled back in when it would run past the right or bottom edge.
pub(crate) fn clamp_origin(at: (f32, f32), size: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    let x = at.0.min(viewport.0 - size.0 - MARGIN).max(0.);
    let y = at.1.min(viewport.1 - size.1 - MARGIN).max(0.);
    (x, y)
}

/// The menu on screen: what it was opened on, where, and the destructive item
/// waiting for its confirmation.
#[derive(Debug)]
pub(crate) struct Open {
    target: Target,
    /// The pointer's position in window coordinates, when the menu opened.
    position: Point<Pixels>,
    armed: Option<Item>,
}

#[derive(Debug, Default)]
pub(crate) struct ContextMenu {
    pub(crate) open: Option<Open>,
}

impl StatusView {
    /// Opens the branch menu of `name` under the pointer. Other menus close.
    pub(crate) fn open_branch_menu(
        &mut self,
        name: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(Target::Branch(name.to_owned()), position, cx);
    }

    /// Opens the menu of the worktree whose folder is `path`, under the pointer.
    pub(crate) fn open_worktree_menu(
        &mut self,
        path: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(Target::Worktree(path.to_owned()), position, cx);
    }

    /// Opens the menu of the file row at `index`, which becomes the selection and
    /// takes the list focus, as a left click on it does.
    pub(crate) fn open_file_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .model
            .entries()
            .get(index)
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        self.list_focus.focus(window);
        self.select_row(index, cx);
        self.open_menu(Target::File(path), position, cx);
    }

    /// Opens the file menu of the selected row, at the pointer (the menu key).
    pub(crate) fn open_selected_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.model.selected_row() {
            let position = window.mouse_position();
            self.open_file_menu(index, position, window, cx);
        }
    }

    fn open_menu(&mut self, target: Target, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.project_menu.open = false;
        self.branch_menu.open = false;
        self.context_menu.open = Some(Open {
            target,
            position,
            armed: None,
        });
        cx.notify();
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu.open = None;
        cx.notify();
    }

    /// A click on an item. A disabled one says why and closes the menu; an
    /// enabled destructive one asks first; any other runs and closes the menu.
    fn choose_menu_item(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        match item.state {
            Err(reason) => {
                self.close_context_menu(cx);
                self.notify_user(reason, cx);
            }
            Ok(()) if item.destructive() && !item.action.opens_dialog() => {
                if let Some(open) = self.context_menu.open.as_mut() {
                    open.armed = Some(item);
                }
                cx.notify();
            }
            Ok(()) => {
                self.close_context_menu(cx);
                self.run_action(item.action, &item.args, window, cx);
            }
        }
    }

    /// The items of the open menu, read from the registry now.
    fn menu_items(&self, target: &Target) -> Vec<Item> {
        let ctx = self.action_context("");
        match target {
            Target::Branch(name) => branch_items(name, &ctx),
            Target::File(path) => file_items(path, &ctx),
            Target::Worktree(path) => worktree_items(path, &ctx),
        }
    }

    /// The menu and the layer under it that closes it on any click outside.
    /// Drawn last, so it sits above the whole window.
    pub(crate) fn context_menu_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let Some(open) = self.context_menu.open.as_ref() else {
            return div();
        };
        let items = self.menu_items(&open.target);
        let groups = sections(&items);
        let height = if open.armed.is_some() {
            CONFIRM_HEIGHT
        } else {
            let rows = lines(items.len()) * ROW_HEIGHT;
            let separators = lines(groups.len().saturating_sub(1)) * SEPARATOR_HEIGHT;
            rows + separators + PADDING
        };
        let viewport = window.viewport_size();
        let (left, top) = clamp_origin(
            (f32::from(open.position.x), f32::from(open.position.y)),
            (MENU_WIDTH, height),
            (f32::from(viewport.width), f32::from(viewport.height)),
        );

        let outside = div()
            .absolute()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_context_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_context_menu(cx)),
            );
        let body = if let Some(item) = &open.armed {
            Self::confirm_panel(item, &open.target, cx).into_any_element()
        } else {
            let mut body = div().flex().flex_col().py_1();
            for (section, group) in groups.iter().enumerate() {
                if section > 0 {
                    body = body.child(separator());
                }
                for item in *group {
                    body = body.child(Self::item_row(item, cx));
                }
            }
            body.into_any_element()
        };
        div().absolute().inset_0().child(outside).child(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(MENU_WIDTH))
                .bg(theme::panel())
                .border_1()
                .border_color(theme::hairline_strong())
                .rounded_md()
                .shadow_lg()
                .occlude()
                .child(body),
        )
    }

    /// One item: its title, and on the right its shortcut or, when it cannot run,
    /// the reason. A destructive item is red.
    fn item_row(item: &Item, cx: &Context<Self>) -> Stateful<Div> {
        let index = item.action as usize;
        let enabled = item.state.is_ok();
        let colour = if !enabled {
            theme::faint()
        } else if item.destructive() {
            theme::removed_fg()
        } else {
            theme::text()
        };
        // The shortcut stays on the row even when the item cannot run yet; the
        // reason then moves to a hint. Without a shortcut the reason is the note.
        let reason = item.state.err();
        let shortcut = item.action.shortcut_label();
        let note = shortcut.clone().or_else(|| reason.map(str::to_owned));
        let clicked = item.clone();
        let row = div()
            .id(("menu-item", index))
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .text_color(colour)
            .when(enabled, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(theme::selected_row()))
            })
            .child(div().whitespace_nowrap().child(item.label()))
            .children(note.map(|text| {
                div()
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(theme::muted())
                    .child(text)
            }));
        let row = match reason {
            Some(reason) if shortcut.is_some() => row.tooltip(window_chrome::hint(reason)),
            _ => row,
        };
        row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.choose_menu_item(clicked.clone(), window, cx);
            }),
        )
    }

    /// The question that replaces the items after a destructive one is chosen:
    /// its name, then Cancel and the item's own title in red.
    fn confirm_panel(item: &Item, target: &Target, cx: &Context<Self>) -> Div {
        let (Target::Branch(subject) | Target::File(subject) | Target::Worktree(subject)) = target;
        let confirm = item.clone();
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(format!(
                "{} to {subject}? This cannot be undone.",
                item.label()
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .id("menu-confirm-cancel")
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(theme::button_border())
                            .cursor_pointer()
                            .child("Cancel")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    this.close_context_menu(cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("menu-confirm")
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(theme::button_border())
                            .text_color(theme::removed_fg())
                            .cursor_pointer()
                            .child(item.label())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.close_context_menu(cx);
                                    this.run_action(confirm.action, &confirm.args, window, cx);
                                }),
                            ),
                    ),
            )
    }
}

/// A count as the pixels it takes; a menu never has 65 536 lines.
fn lines(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

fn separator() -> Div {
    div().h(px(1.)).my_1().bg(theme::hairline())
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, StatusEntry, WorktreeEntry};

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
        }
    }

    fn ctx(entries: &[StatusEntry]) -> ActionContext<'_> {
        ActionContext {
            repo_open: true,
            busy: None,
            entries,
            staged_count: 0,
            message: "",
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
            workflows: crate::action_registry::WorkflowCtx::default(),
            worktrees: &[],
            branch_scope: crate::branch_filter::Scope::All,
            user_email_known: false,
            remote_running: false,
        }
    }

    fn find(items: &[Item], action: Action) -> &Item {
        items
            .iter()
            .find(|item| item.action == action)
            .expect("the menu lists the action")
    }

    #[test]
    fn branch_menu_lists_the_design_items_in_order_with_registry_titles() {
        let items = branch_items("feature", &ctx(&[]));
        let labels: Vec<&str> = items.iter().map(Item::label).collect();
        assert_eq!(
            labels,
            [
                "Checkout Branch",
                "New Branch…",
                "New Child Branch",
                "Create Worktree",
                "Restack This Stack",
                "Push With Lease",
                "Copy Branch Name",
                "Rename Branch",
                "Delete Branch",
            ]
        );
        assert!(
            items
                .iter()
                .all(|item| item.args == Args::Branch("feature".into()))
        );
    }

    #[test]
    fn branch_menu_is_split_into_groups_by_the_separators() {
        let items = branch_items("feature", &ctx(&[]));
        let sizes: Vec<usize> = sections(&items).iter().map(|s| s.len()).collect();
        assert_eq!(sizes, [1, 4, 1, 1, 2]);
    }

    #[test]
    fn a_branch_in_a_linked_worktree_gets_the_worktree_menu_and_main_keeps_the_branch_menu() {
        let entry = |path: &str, branch: &str, is_main: bool| WorktreeEntry {
            path: path.to_owned(),
            head: None,
            branch: Some(branch.to_owned()),
            detached: false,
            bare: false,
            is_main,
            locked: None,
            prunable: None,
        };
        let listed = [
            entry("/r", "main", true),
            entry("/r-feature", "feature", false),
        ];
        let context = ActionContext {
            root: Some("/r"),
            worktrees: &listed,
            ..ctx(&[])
        };
        let titles = |items: &[Item]| items.iter().map(Item::label).collect::<Vec<_>>();
        assert_eq!(
            titles(&branch_items("feature", &context)),
            vec![
                "Open Worktree",
                "Reveal Worktree in File Manager",
                "Copy Worktree Path",
                "New Branch…",
                "New Child Branch",
                "Restack This Stack",
                "Remove Worktree",
            ]
        );
        let main = branch_items("main", &context);
        assert!(
            main.iter()
                .any(|item| item.action == Action::CheckoutBranch)
        );
        assert!(
            main.iter()
                .any(|item| item.action == Action::CreateWorktree)
        );
    }

    #[test]
    fn a_worktree_menu_offers_lock_or_unlock_and_prune_only_when_its_folder_is_gone() {
        let entry = |path: &str, locked: Option<&str>, prunable: Option<&str>| WorktreeEntry {
            path: path.to_owned(),
            head: None,
            branch: Some("feature".to_owned()),
            detached: false,
            bare: false,
            is_main: false,
            locked: locked.map(str::to_owned),
            prunable: prunable.map(str::to_owned),
        };
        let listed = [
            entry("/r-live", None, None),
            entry("/r-locked", Some(""), None),
            entry("/r-gone", None, Some("")),
        ];
        let context = ActionContext {
            root: Some("/r"),
            worktrees: &listed,
            ..ctx(&[])
        };
        let titles = |items: &[Item]| items.iter().map(Item::label).collect::<Vec<_>>();
        assert_eq!(
            titles(&worktree_items("/r-live", &context)),
            vec![
                "Open Worktree",
                "Reveal Worktree in File Manager",
                "Copy Worktree Path",
                "Lock Worktree",
                "Remove Worktree"
            ]
        );
        assert_eq!(
            titles(&worktree_items("/r-locked", &context)),
            vec![
                "Open Worktree",
                "Reveal Worktree in File Manager",
                "Copy Worktree Path",
                "Unlock Worktree",
                "Remove Worktree"
            ]
        );
        let gone = worktree_items("/r-gone", &context);
        assert!(
            gone.iter()
                .any(|item| item.action == Action::PruneWorktrees && item.state.is_ok())
        );
        assert_eq!(
            gone.last().map(|item| item.action),
            Some(Action::RemoveWorktree)
        );
    }

    #[test]
    fn checkout_is_disabled_on_the_current_branch_and_enabled_elsewhere() {
        let context = ctx(&[]);
        let on_main = branch_items("main", &context);
        assert_eq!(
            find(&on_main, Action::CheckoutBranch).state,
            Err("This is the current branch")
        );
        let elsewhere = branch_items("feature", &context);
        assert_eq!(find(&elsewhere, Action::CheckoutBranch).state, Ok(()));
    }

    #[test]
    fn branch_items_the_engine_lacks_are_listed_disabled_with_a_reason() {
        let items = branch_items("feature", &ctx(&[]));
        for action in [
            Action::NewChildBranch,
            Action::Restack,
            Action::RenameBranch,
        ] {
            assert_eq!(
                find(&items, action).state,
                Err("Not available yet"),
                "{action:?}"
            );
        }
    }

    #[test]
    fn copy_branch_name_runs_even_while_the_repository_is_busy() {
        let entries: [StatusEntry; 0] = [];
        let busy = ActionContext {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&entries)
        };
        let items = branch_items("feature", &busy);
        assert_eq!(find(&items, Action::CopyBranchName).state, Ok(()));
        assert_eq!(
            find(&items, Action::CheckoutBranch).state,
            Err("Wait for the branch checkout to finish.")
        );
    }

    #[test]
    fn only_delete_is_destructive_in_the_branch_menu() {
        let items = branch_items("feature", &ctx(&[]));
        let destructive: Vec<&str> = items
            .iter()
            .filter(|item| item.destructive())
            .map(Item::label)
            .collect();
        assert_eq!(destructive, ["Delete Branch"]);
    }

    #[test]
    fn delete_runs_on_a_free_branch_and_says_why_it_cannot_on_the_checked_out_one() {
        let on_main = branch_items("main", &ctx(&[]));
        assert_eq!(
            find(&on_main, Action::DeleteBranch).state,
            Err("Cannot delete the checked-out branch")
        );
        let elsewhere = branch_items("feature", &ctx(&[]));
        assert_eq!(find(&elsewhere, Action::DeleteBranch).state, Ok(()));
        assert_eq!(find(&elsewhere, Action::NewBranch).state, Ok(()));
    }

    #[test]
    fn file_menu_lists_stage_and_unstage_by_what_the_file_has() {
        let entries = [
            entry("unstaged.txt", None, Some(Change::Modified)),
            entry("staged.txt", Some(Change::Added), None),
        ];
        let context = ctx(&entries);

        let unstaged = file_items("unstaged.txt", &context);
        assert_eq!(find(&unstaged, Action::StageFile).state, Ok(()));
        assert_eq!(
            find(&unstaged, Action::UnstageFile).state,
            Err("Nothing staged in this file")
        );

        let staged = file_items("staged.txt", &context);
        assert_eq!(
            find(&staged, Action::StageFile).state,
            Err("Nothing unstaged in this file")
        );
        assert_eq!(find(&staged, Action::UnstageFile).state, Ok(()));
    }

    #[test]
    fn file_menu_discard_is_destructive_and_not_available_yet() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        let discard = find(&items, Action::DiscardFile);
        assert_eq!(discard.state, Err("Not available yet"));
        assert!(discard.destructive());
        let destructive: Vec<&str> = items
            .iter()
            .filter(|item| item.destructive())
            .map(Item::label)
            .collect();
        assert_eq!(destructive, ["Discard Changes"]);
    }

    #[test]
    fn file_menu_copy_and_reveal_work_and_are_grouped_together() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        assert_eq!(find(&items, Action::CopyFilePath).state, Ok(()));
        assert_eq!(find(&items, Action::RevealFile).state, Ok(()));
        assert_eq!(find(&items, Action::ViewFile).state, Ok(()));
        let sizes: Vec<usize> = sections(&items).iter().map(|s| s.len()).collect();
        assert_eq!(sizes, [2, 1, 3]);
    }

    #[test]
    fn file_menu_stage_says_why_it_cannot_run_while_busy() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let busy = ActionContext {
            busy: Some("Wait for the repository to open."),
            ..ctx(&entries)
        };
        let items = file_items("a.txt", &busy);
        assert_eq!(
            find(&items, Action::StageFile).state,
            Err("Wait for the repository to open.")
        );
        assert_eq!(find(&items, Action::CopyFilePath).state, Ok(()));
        assert_eq!(find(&items, Action::RevealFile).state, Ok(()));
    }

    #[test]
    fn a_menu_without_a_repository_offers_nothing_that_runs() {
        let closed = ActionContext {
            repo_open: false,
            root: None,
            ..ctx(&[])
        };
        let items = file_items("a.txt", &closed);
        assert_eq!(
            find(&items, Action::RevealFile).state,
            Err("No repository is open.")
        );
        assert_eq!(
            find(&items, Action::StageFile).state,
            Err("No repository is open.")
        );
    }

    #[test]
    fn a_menu_is_pulled_back_inside_the_window() {
        let size = (MENU_WIDTH, 100.);
        let viewport = (800., 600.);
        assert_eq!(clamp_origin((100., 50.), size, viewport), (100., 50.));
        assert_eq!(
            clamp_origin((780., 590.), size, viewport),
            (800. - MENU_WIDTH - MARGIN, 600. - 100. - MARGIN)
        );
    }
}
