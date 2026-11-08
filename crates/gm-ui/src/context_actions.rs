//! The top bar's context actions (UI redesign, slice 1): what is selected, and up to three
//! actions for it, primary first. They are a filtered, ranked view of the registry: the
//! items are the selection's context menu (`context_menu`), so a context action, a menu item,
//! a palette row and a key always agree. "⋯ ⌘K" opens the palette on the whole menu.
//!
//! The ranking keeps the menu's order, which already puts the action a row is for first, and
//! leaves out what a button should not run on one click: an action that is disabled now, a
//! destructive one, and one that asks first. Those stay one step away, in the palette.

use crate::action_registry::{Action, Args};
use crate::context_menu::Item;

/// The most actions the top bar shows for a selection.
pub(crate) const SHOWN: usize = 3;

/// What the middle zone shows: the selection's name and its ranked actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextActions {
    /// The kind of thing selected, such as "File" or "Commit".
    pub(crate) kind: &'static str,
    /// The name of the selection, such as a path or a short commit id.
    pub(crate) subject: String,
    /// Up to `SHOWN` actions, primary first.
    pub(crate) ranked: Vec<(Action, Args, &'static str)>,
    /// Every item of the selection's menu, for the palette that "⋯" opens.
    pub(crate) all: Vec<(Action, Args)>,
}

/// The actions the top bar shows for `items`, the context menu of the selection.
#[must_use]
pub(crate) fn rank(items: &[Item]) -> Vec<(Action, Args, &'static str)> {
    items
        .iter()
        .filter(|item| item.state.is_ok())
        .filter(|item| !item.action.destructive() && !item.action.asks_first())
        .take(SHOWN)
        .map(|item| (item.action, item.args.clone(), item.label()))
        .collect()
}

/// The context actions of a selection named `subject`, of kind `kind`, whose menu is `items`.
#[must_use]
pub(crate) fn for_items(kind: &'static str, subject: String, items: &[Item]) -> ContextActions {
    ContextActions {
        kind,
        subject,
        ranked: rank(items),
        all: items
            .iter()
            .filter(|item| item.state.is_ok() || !item.action.destructive())
            .map(|item| (item.action, item.args.clone()))
            .chain(
                items
                    .iter()
                    .filter(|item| item.state.is_err() && item.action.destructive())
                    .map(|item| (item.action, item.args.clone())),
            )
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, FeatureSet, StatusEntry};

    use super::*;
    use crate::action_registry::Context as ActionContext;
    use crate::context_menu::{file_items, stash_items};

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
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
            messages: crate::action_registry::MessageCtx::default(),
            workflows: crate::action_registry::WorkflowCtx::default(),
            worktrees: &[],
            workspaces: &[],
            stashes: None,
            author_pictures: crate::action_registry::AuthorPictures::Hidden,
            remote_running: false,
            review: crate::action_registry::ReviewCtx::default(),
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            stack: crate::action_registry::StackCtx::default(),
            code: crate::action_registry::CodeCtx::default(),
            submodules: None,
            features: FeatureSet::all(),
            undoable: None,
        }
    }

    #[test]
    fn an_unstaged_file_leads_with_stage_and_never_offers_a_discard() {
        let entries = [entry("a.rs", None, Some(Change::Modified))];
        let items = file_items("a.rs", &ctx(&entries));
        let ranked: Vec<Action> = rank(&items)
            .into_iter()
            .map(|(action, ..)| action)
            .collect();
        assert_eq!(ranked.first(), Some(&Action::StageFile));
        assert!(ranked.len() <= SHOWN);
        assert!(ranked.iter().all(|action| !action.destructive()));
        assert!(!ranked.contains(&Action::UnstageFile));
    }

    #[test]
    fn a_staged_file_leads_with_unstage() {
        let entries = [entry("a.rs", Some(Change::Modified), None)];
        let items = file_items("a.rs", &ctx(&entries));
        assert_eq!(rank(&items).first().map(|r| r.0), Some(Action::UnstageFile));
    }

    #[test]
    fn the_palette_gets_the_whole_menu_and_disabled_destructive_items_go_last() {
        let entries = [entry("a.rs", None, Some(Change::Modified))];
        let items = file_items("a.rs", &ctx(&entries));
        let actions = for_items("File", "a.rs".to_owned(), &items);
        assert_eq!(actions.all.len(), items.len());
        assert_eq!(actions.kind, "File");
        let stash = stash_items(0, "abc", &ctx(&entries));
        let ranked: Vec<Action> = rank(&stash).into_iter().map(|r| r.0).collect();
        assert!(!ranked.contains(&Action::StashDrop));
    }
}
