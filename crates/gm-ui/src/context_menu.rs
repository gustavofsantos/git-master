//! The right-click menus of the sidebar's branch rows and the Working Copy's
//! file rows (slice 1c, item 3). The items come from the action registry: each
//! one names its action, its target and whether it runs now, so a menu offers
//! nothing that a key or a button would refuse. A click on an enabled item runs
//! it through `run_action`, with `Actor::Human`, like every other path.

use gpui::{
    Context, Div, MouseButton, MouseDownEvent, Pixels, Point, Stateful, Window, anchored, deferred,
    div, prelude::*, px,
};

use crate::action_registry::{Action, Args, Context as ActionContext, Reason};
use crate::reflog;
use crate::stashes;
use crate::theme;
use crate::view::StatusView;
use crate::window_chrome;

const ROW_HEIGHT: f32 = 24.;
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
    /// A stash, by its index and commit id, as the Stashes page listed it.
    Stash {
        index: u32,
        oid: String,
    },
    /// An entry of the Reflog page, by its index and the commit it names (B3).
    ReflogEntry {
        index: u32,
        oid: String,
    },
    /// A thread row of the Threads page, with its title and star as the row shows them (T6).
    Thread {
        uid: String,
        title: String,
        starred: bool,
    },
    /// The More menu of the thread header, with the thread's title and star (T6).
    ThreadMore {
        uid: String,
        title: String,
        starred: bool,
    },
    /// A label row of the Labels page, with its name and attachment count (P4).
    Label {
        id: i64,
        name: String,
        attachments: u32,
    },
}

impl Target {
    /// The name the menu's question uses for what it was opened on.
    fn subject(&self) -> String {
        match self {
            Self::Branch(name)
            | Self::File(name)
            | Self::Worktree(name)
            | Self::Label { name, .. } => name.clone(),
            Self::Stash { index, .. } => stashes::label(*index),
            Self::ReflogEntry { index, .. } => reflog::label(*index),
            Self::Thread { title, .. } | Self::ThreadMore { title, .. } => title.clone(),
        }
    }
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
    /// The line the item shows: the action's title, unless the menu words it for its state.
    title: &'static str,
}

impl Item {
    fn new(action: Action, args: Args, group: u8, ctx: &ActionContext<'_>) -> Self {
        let state = action.enabled(&args, ctx);
        Self {
            action,
            args,
            group,
            state,
            title: action.spec().title,
        }
    }

    /// The same item with another line, for a menu that words it for the state it is in.
    fn retitled(mut self, title: &'static str) -> Self {
        self.title = title;
        self
    }

    pub(crate) fn label(&self) -> &'static str {
        self.title
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
        item(Action::SetStackParent, 2),
        item(Action::RemoveFromStack, 2),
        item(Action::ReparentToGrandparent, 2),
        item(Action::Restack, 2),
        item(Action::PushWithLease, 3),
        item(Action::CopyBranchName, 4),
        item(Action::RenameBranch, 5),
        item(Action::DeleteBranch, 5),
    ]
}

/// The menu of a file row: stage or unstage (both are listed, each disabled
/// when it has nothing to act on), the two discards, then copy the path and reveal it.
pub(crate) fn file_items(path: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::File(path.to_owned());
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    let mut items = vec![
        item(Action::StageFile, 0),
        item(Action::UnstageFile, 0),
        item(Action::DiscardFile, 1),
        item(Action::DiscardAllChanges, 1),
        item(Action::ViewFile, 2),
        item(Action::MoveFileToBranch, 2),
        item(Action::MoveFileToWorktree, 2),
        item(Action::ShowThreadsForFile, 2),
        item(Action::CopyFilePath, 2),
        item(Action::RevealFile, 2),
    ];
    // Only a conflicted file lists its resolver, which leads the menu.
    if ctx
        .entries
        .iter()
        .any(|entry| entry.path == path && entry.conflicted)
    {
        items.insert(0, item(Action::ResolveConflict, 0));
    }
    items
}

/// The menu of a thread row: rename, note, tags and star, then the link and the
/// export, and delete last in red. The star's line says what the click does.
pub(crate) fn thread_items(uid: &str, starred: bool, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::Thread {
        uid: uid.to_owned(),
        starred,
    };
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    let star = item(Action::ThreadStar, 0);
    let star = if starred {
        star.retitled("Unstar Thread")
    } else {
        star
    };
    vec![
        item(Action::ThreadRename, 0),
        item(Action::ThreadNote, 0),
        item(Action::ThreadTags, 0),
        star,
        item(Action::ThreadLinkCommit, 1),
        item(Action::ThreadLinkThread, 1),
        item(Action::ThreadExport, 2),
        item(Action::ThreadCopyMarkdown, 2),
        item(Action::ThreadDelete, 3),
    ]
}

/// The More menu of the thread header: rename, note and tags, the link and the
/// export, the two tool-call folds, and delete last in red. The star and the copy
/// are the header's own buttons, so they are not listed here.
pub(crate) fn thread_more_items(uid: &str, starred: bool, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::Thread {
        uid: uid.to_owned(),
        starred,
    };
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![
        item(Action::ThreadRename, 0),
        item(Action::ThreadNote, 0),
        item(Action::ThreadTags, 0),
        item(Action::ThreadLinkCommit, 1),
        item(Action::ThreadLinkThread, 1),
        item(Action::ThreadExport, 1),
        item(Action::ThreadExpandAll, 2),
        item(Action::ThreadCollapseAll, 2),
        item(Action::ThreadDelete, 3),
    ]
}

/// The menu of a label row: rename, then delete in red. Delete opens its confirmation,
/// which says how many attachments go with the label.
pub(crate) fn label_items(
    id: i64,
    name: &str,
    attachments: u32,
    ctx: &ActionContext<'_>,
) -> Vec<Item> {
    let target = Args::Label {
        id,
        name: name.to_owned(),
        attachments,
    };
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![item(Action::LabelRename, 0), item(Action::LabelDelete, 1)]
}

/// The menu of a stash row: apply and pop, then drop in its own group. Drop
/// opens its dialog, which is the confirmation.
pub(crate) fn stash_items(index: u32, oid: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::Stash {
        index,
        oid: oid.to_owned(),
        label: stashes::label(index),
    };
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![
        item(Action::StashApply, 0),
        item(Action::StashPop, 0),
        item(Action::StashDrop, 1),
    ]
}

/// The menu of a Reflog row (B3): check out the commit detached and branch from
/// it, then copy its hash. Checking out is refused while a change is in flight, with the reason.
pub(crate) fn reflog_items(oid: &str, ctx: &ActionContext<'_>) -> Vec<Item> {
    let target = Args::Commit(oid.to_owned());
    let item = |action, group| Item::new(action, target.clone(), group, ctx);
    vec![
        item(Action::CheckoutCommit, 0),
        item(Action::NewBranchFromCommit, 0),
        item(Action::CopyCommitHash, 1),
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
        Item::new(Action::SetStackParent, branch.clone(), 3, ctx),
        Item::new(Action::RemoveFromStack, branch.clone(), 3, ctx),
        Item::new(Action::ReparentToGrandparent, branch.clone(), 3, ctx),
        Item::new(Action::Restack, branch, 3, ctx),
        Item::new(Action::RemoveWorktree, worktree, 4, ctx),
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

    /// Opens the menu of the stash `oid` at `index`, under the pointer (B2).
    pub(crate) fn open_stash_menu(
        &mut self,
        index: u32,
        oid: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(
            Target::Stash {
                index,
                oid: oid.to_owned(),
            },
            position,
            cx,
        );
    }

    /// Opens the More menu of the thread header of `uid`, under the pointer (T6).
    pub(crate) fn open_thread_more_menu(
        &mut self,
        uid: &str,
        title: &str,
        starred: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(
            Target::ThreadMore {
                uid: uid.to_owned(),
                title: title.to_owned(),
                starred,
            },
            position,
            cx,
        );
    }

    /// Opens the menu of the thread row `uid`, under the pointer (T6).
    pub(crate) fn open_thread_menu(
        &mut self,
        uid: &str,
        title: &str,
        starred: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(
            Target::Thread {
                uid: uid.to_owned(),
                title: title.to_owned(),
                starred,
            },
            position,
            cx,
        );
    }

    /// Opens the menu of the label row `id`, under the pointer (P4).
    pub(crate) fn open_label_menu(
        &mut self,
        id: i64,
        name: &str,
        attachments: u32,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(
            Target::Label {
                id,
                name: name.to_owned(),
                attachments,
            },
            position,
            cx,
        );
    }

    /// Opens the menu of the Reflog row `index` that names the commit `oid`, under the pointer (B3).
    pub(crate) fn open_reflog_menu(
        &mut self,
        index: u32,
        oid: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(
            Target::ReflogEntry {
                index,
                oid: oid.to_owned(),
            },
            position,
            cx,
        );
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
            Ok(())
                if (item.destructive() || item.action.asks_first())
                    && !item.action.opens_dialog() =>
            {
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
            Target::Stash { index, oid } => stash_items(*index, oid, &ctx),
            Target::ReflogEntry { oid, .. } => reflog_items(oid, &ctx),
            Target::Thread { uid, starred, .. } => thread_items(uid, *starred, &ctx),
            Target::ThreadMore { uid, starred, .. } => thread_more_items(uid, *starred, &ctx),
            Target::Label {
                id,
                name,
                attachments,
            } => label_items(*id, name, *attachments, &ctx),
        }
    }

    /// The menu and the layer under it that closes it on any click outside.
    /// Drawn last, so it sits above the whole window.
    pub(crate) fn context_menu_layer(&self, cx: &Context<Self>) -> Div {
        let Some(open) = self.context_menu.open.as_ref() else {
            return div();
        };
        let items = self.menu_items(&open.target);
        let groups = sections(&items);
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
        // Anchored at the pointer and measured by GPUI, so the menu stays inside
        // the window whatever its height.
        div().absolute().inset_0().child(outside).child(deferred(
            anchored()
                .position(open.position)
                .snap_to_window_with_margin(px(MARGIN))
                .child(
                    div()
                        .w(px(MENU_WIDTH))
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::hairline_strong())
                        .rounded_md()
                        .shadow_lg()
                        .occlude()
                        .child(body),
                ),
        ))
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
        let subject = target.subject();
        let confirm = item.clone();
        let confirm_destructive = item.destructive();
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(if item.destructive() {
                format!("{} to {subject}? This cannot be undone.", item.label())
            } else {
                format!(
                    "{} for {subject}? Its commits do not move until you restack.",
                    item.label()
                )
            })
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
                            .when(confirm_destructive, |this| {
                                this.text_color(theme::removed_fg())
                            })
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

fn separator() -> Div {
    div().h(px(1.)).my_1().bg(theme::hairline())
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, StashEntry, StatusEntry, WorktreeEntry};

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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            entries,
            staged_count: 0,
            message: "",
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
            workflows: crate::action_registry::WorkflowCtx::default(),
            worktrees: &[],
            author_pictures: crate::action_registry::AuthorPictures::Hidden,
            remote_running: false,
            stashes: None,
            review: crate::action_registry::ReviewCtx::default(),
        }
    }

    fn find(items: &[Item], action: Action) -> &Item {
        items
            .iter()
            .find(|item| item.action == action)
            .expect("the menu lists the action")
    }

    #[test]
    fn the_more_menu_of_a_thread_lists_its_actions_in_order_and_delete_last() {
        let items = thread_more_items("u1", true, &ctx(&[]));
        let actions: Vec<Action> = items.iter().map(|item| item.action).collect();
        assert_eq!(
            actions,
            [
                Action::ThreadRename,
                Action::ThreadNote,
                Action::ThreadTags,
                Action::ThreadLinkCommit,
                Action::ThreadLinkThread,
                Action::ThreadExport,
                Action::ThreadExpandAll,
                Action::ThreadCollapseAll,
                Action::ThreadDelete,
            ]
        );
        assert!(items.iter().all(|item| item.args
            == Args::Thread {
                uid: "u1".into(),
                starred: true,
            }));
        assert_eq!(items.last().map(Item::label), Some("Delete Thread…"));
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
                "Set Parent…",
                "Remove From Stack",
                "Reparent to Grandparent",
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
        assert_eq!(sizes, [1, 3, 4, 1, 1, 2]);
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
                "Set Parent…",
                "Remove From Stack",
                "Reparent to Grandparent",
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
        assert_eq!(
            find(&items, Action::RenameBranch).state,
            Err("Not available yet")
        );
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
    fn file_menu_lists_resolve_conflict_first_and_only_for_a_conflicted_file() {
        let mut conflicted = entry("a.txt", None, Some(Change::Modified));
        conflicted.conflicted = true;
        let entries = [conflicted, entry("b.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        assert_eq!(items[0].label(), "Resolve Conflict");
        assert_eq!(items[0].state, Ok(()));
        let plain = file_items("b.txt", &ctx(&entries));
        assert!(plain.iter().all(|item| item.label() != "Resolve Conflict"));
    }

    #[test]
    fn file_menu_discards_are_destructive_and_enabled_for_a_modified_file() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        for action in [Action::DiscardFile, Action::DiscardAllChanges] {
            let discard = find(&items, action);
            assert_eq!(discard.state, Ok(()), "{action:?}");
            assert!(discard.destructive(), "{action:?}");
        }
        let destructive: Vec<&str> = items
            .iter()
            .filter(|item| item.destructive())
            .map(Item::label)
            .collect();
        assert_eq!(destructive, ["Discard Changes", "Discard All Changes"]);
    }

    #[test]
    fn file_menu_discard_all_is_disabled_for_a_clean_file_and_discard_for_staged_only() {
        let clean = [entry("a.txt", None, None)];
        let items = file_items("a.txt", &ctx(&clean));
        assert_eq!(
            find(&items, Action::DiscardFile).state,
            Err("Nothing unstaged to discard")
        );
        assert_eq!(
            find(&items, Action::DiscardAllChanges).state,
            Err("Nothing to discard")
        );

        let staged = [entry("a.txt", Some(Change::Modified), None)];
        let items = file_items("a.txt", &ctx(&staged));
        assert_eq!(
            find(&items, Action::DiscardFile).state,
            Err("Nothing unstaged to discard")
        );
        assert_eq!(find(&items, Action::DiscardAllChanges).state, Ok(()));
    }

    #[test]
    fn file_menu_copy_and_reveal_work_and_are_grouped_together() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        assert_eq!(find(&items, Action::CopyFilePath).state, Ok(()));
        assert_eq!(find(&items, Action::RevealFile).state, Ok(()));
        assert_eq!(find(&items, Action::ViewFile).state, Ok(()));
        let sizes: Vec<usize> = sections(&items).iter().map(|s| s.len()).collect();
        assert_eq!(sizes, [2, 2, 6]);
    }

    #[test]
    fn the_move_items_follow_view_file_in_the_same_group() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let items = file_items("a.txt", &ctx(&entries));
        let at = |action| items.iter().position(|item| item.action == action);
        let view = at(Action::ViewFile).expect("view file is listed");
        assert_eq!(at(Action::MoveFileToBranch), Some(view + 1));
        assert_eq!(at(Action::MoveFileToWorktree), Some(view + 2));
        let branch = find(&items, Action::MoveFileToBranch);
        assert_eq!(branch.args, Args::File("a.txt".into()));
        assert_eq!(branch.group, 2);
        assert_eq!(branch.state, Ok(()));
        assert_eq!(find(&items, Action::MoveFileToWorktree).state, Ok(()));
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            stack: crate::action_registry::StackCtx::default(),
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
    fn a_stash_menu_lists_apply_and_pop_then_drop_in_its_own_group() {
        let stashes = [StashEntry {
            index: 0,
            oid: "a".repeat(40),
            message: "On main: wip".into(),
            branch: Some("main".into()),
            time: 1,
        }];
        let context = ActionContext {
            stashes: Some(&stashes[..]),
            review: crate::action_registry::ReviewCtx::default(),
            ..ctx(&[])
        };
        let items = stash_items(0, &"a".repeat(40), &context);
        let groups: Vec<Vec<&str>> = sections(&items)
            .iter()
            .map(|group| group.iter().map(Item::label).collect())
            .collect();
        assert_eq!(
            groups,
            [vec!["Apply Stash", "Pop Stash"], vec!["Drop Stash"]]
        );
        assert!(items.iter().all(|item| item.state == Ok(())));
        assert_eq!(
            items.iter().map(Item::destructive).collect::<Vec<_>>(),
            [false, false, true]
        );
        // Each item runs on the listed stash: its index, commit id and name.
        assert_eq!(
            find(&items, Action::StashPop).args,
            Args::Stash {
                index: 0,
                oid: "a".repeat(40),
                label: "stash@{0}".into(),
            }
        );
    }

    #[test]
    fn a_stash_menu_says_why_its_items_cannot_run_before_the_listing() {
        let items = stash_items(0, &"a".repeat(40), &ctx(&[]));
        for item in &items {
            assert_eq!(item.state, Err("Open the Stashes page first."), "{item:?}");
        }
    }

    #[test]
    fn a_reflog_menu_checks_out_or_branches_from_the_entry_then_copies_its_hash() {
        let context = ctx(&[]);
        let oid = "b".repeat(40);
        let items = reflog_items(&oid, &context);
        let groups: Vec<Vec<&str>> = sections(&items)
            .iter()
            .map(|group| group.iter().map(Item::label).collect())
            .collect();
        assert_eq!(
            groups,
            [
                vec!["Check Out Commit (Detached)", "New Branch From Commit…"],
                vec!["Copy Commit Hash"],
            ]
        );
        assert!(items.iter().all(|item| item.state == Ok(())));
        assert!(items.iter().all(|item| !item.destructive()));
        assert_eq!(
            find(&items, Action::CheckoutCommit).args,
            Args::Commit(oid.clone())
        );
        assert_eq!(
            find(&items, Action::NewBranchFromCommit).args,
            Args::Commit(oid)
        );
    }

    #[test]
    fn a_reflog_menu_says_why_checkout_and_branch_wait_while_a_change_is_running() {
        let busy = ActionContext {
            busy: Some("Wait for the repository to open."),
            ..ctx(&[])
        };
        let items = reflog_items(&"b".repeat(40), &busy);
        for action in [Action::CheckoutCommit, Action::NewBranchFromCommit] {
            assert_eq!(
                find(&items, action).state,
                Err("Wait for the repository to open."),
                "{action:?}"
            );
        }
        // Copying the hash runs even while the repository is busy.
        assert_eq!(find(&items, Action::CopyCommitHash).state, Ok(()));
    }

    #[test]
    fn a_label_row_menu_renames_or_deletes_that_label_and_deletes_in_red() {
        let items = label_items(5, "flaky", 3, &ctx(&[]));
        let labels: Vec<&str> = items.iter().map(Item::label).collect();
        assert_eq!(labels, vec!["Rename Label…", "Delete Label…"]);
        assert_eq!(
            items[0].args,
            Args::Label {
                id: 5,
                name: "flaky".into(),
                attachments: 3,
            }
        );
        assert_eq!(items[0].state, Ok(()));
        assert!(items[1].destructive());
        assert_eq!(sections(&items).len(), 2, "delete sits in its own group");
    }
}
