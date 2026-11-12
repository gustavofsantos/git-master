//! The view each repository root had when the window left it (D234 (3)). Kept in
//! memory only, never written to disk: a root reopened in the same window gets its
//! page, selected file and commit message draft back, and the removal of the shown
//! workspace goes back to the root left most recently.

use std::collections::HashMap;

use crate::view::Screen;

/// What the window showed for one repository root when it left that root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeptView {
    pub(crate) screen: Screen,
    /// The file selected in the Working Copy, if one was.
    pub(crate) selected_path: Option<String>,
    /// The commit message being written, empty when none.
    pub(crate) commit_draft: String,
}

/// The kept views of a window, by root, and the order in which roots were left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeptViews {
    views: HashMap<String, KeptView>,
    /// Roots the window left, oldest first. Each root appears at most once.
    left: Vec<String>,
}

impl KeptViews {
    /// Keeps `view` for `root`, which the window has just left. The root becomes the
    /// most recently left one; an earlier view of it is replaced.
    pub(crate) fn leave(&mut self, root: String, view: KeptView) {
        self.left.retain(|known| known != &root);
        self.left.push(root.clone());
        self.views.insert(root, view);
    }

    /// The view kept for `root`, taken: it is returned once, and the window shows it
    /// from then on. `None` when no view was kept or it was taken already.
    pub(crate) fn take(&mut self, root: &str) -> Option<KeptView> {
        self.views.remove(root)
    }

    /// The root the window left most recently, other than `current`.
    #[must_use]
    pub(crate) fn previous_root(&self, current: &str) -> Option<String> {
        self.left
            .iter()
            .rev()
            .find(|root| root.as_str() != current)
            .cloned()
    }

    /// Drops everything kept for `root`, for a root that is gone.
    pub(crate) fn forget(&mut self, root: &str) {
        self.views.remove(root);
        self.left.retain(|known| known != root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(selected: Option<&str>, draft: &str) -> KeptView {
        KeptView {
            screen: Screen::WorkingCopy,
            selected_path: selected.map(str::to_owned),
            commit_draft: draft.to_owned(),
        }
    }

    #[test]
    fn a_left_root_is_taken_back_once() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(Some("src/lib.rs"), "fix: parse"));
        assert_eq!(
            kept.take("/a"),
            Some(view(Some("src/lib.rs"), "fix: parse"))
        );
        assert_eq!(kept.take("/a"), None);
    }

    #[test]
    fn a_view_with_no_selection_and_an_empty_draft_is_still_kept() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        assert_eq!(kept.take("/a"), Some(view(None, "")));
    }

    #[test]
    fn a_root_never_left_has_no_view() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        assert_eq!(kept.take("/b"), None);
    }

    #[test]
    fn leaving_a_root_again_replaces_its_view() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, "old"));
        kept.leave("/a".into(), view(Some("x"), "new"));
        assert_eq!(kept.take("/a"), Some(view(Some("x"), "new")));
    }

    #[test]
    fn previous_root_is_the_most_recently_left_root() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        kept.leave("/b".into(), view(None, ""));
        assert_eq!(kept.previous_root("/c"), Some("/b".to_owned()));
    }

    #[test]
    fn previous_root_skips_the_current_root() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        kept.leave("/b".into(), view(None, ""));
        // The window shows /b, so it is not the previous root; /a is.
        assert_eq!(kept.previous_root("/b"), Some("/a".to_owned()));
    }

    #[test]
    fn previous_root_is_none_when_nothing_else_was_left() {
        let mut kept = KeptViews::default();
        assert_eq!(kept.previous_root("/a"), None);
        kept.leave("/a".into(), view(None, ""));
        assert_eq!(kept.previous_root("/a"), None);
    }

    #[test]
    fn forgotten_roots_are_not_previous_and_have_no_view() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        kept.leave("/b".into(), view(None, ""));
        kept.forget("/b");
        assert_eq!(kept.previous_root("/c"), Some("/a".to_owned()));
        assert_eq!(kept.take("/b"), None);
        kept.forget("/a");
        assert_eq!(kept.previous_root("/c"), None);
    }

    #[test]
    fn leaving_a_root_moves_it_to_the_front_of_the_order() {
        let mut kept = KeptViews::default();
        kept.leave("/a".into(), view(None, ""));
        kept.leave("/b".into(), view(None, ""));
        kept.leave("/a".into(), view(None, ""));
        assert_eq!(kept.previous_root("/c"), Some("/a".to_owned()));
    }
}
