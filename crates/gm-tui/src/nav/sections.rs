//! The section tree every screen is made of (D241 (1)): foldable sections under one cursor,
//! with a selection of rows. A section's id is stable across refreshes, so folds and the
//! cursor stay where they were when the content is read again. What a section is about
//! (a file, a hunk, a line, a thread) is the screen's own payload, `T`.

use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<T> {
    /// Stable for the thing it shows: `unstaged`, `unstaged/src/a.rs`, `…/h_7f3a`.
    pub id: String,
    pub item: T,
    pub children: Vec<Section<T>>,
    /// Whether it starts folded, as Magit's files do, until the person unfolds it.
    pub starts_folded: bool,
}

impl<T> Section<T> {
    pub fn leaf(id: impl Into<String>, item: T) -> Self {
        Self {
            id: id.into(),
            item,
            children: Vec::new(),
            starts_folded: false,
        }
    }

    pub fn with(id: impl Into<String>, item: T, children: Vec<Section<T>>) -> Self {
        Self {
            id: id.into(),
            item,
            children,
            starts_folded: false,
        }
    }

    #[must_use]
    pub fn folded(mut self) -> Self {
        self.starts_folded = true;
        self
    }
}

/// A row on screen: a section shown, with its depth and whether it is folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row<'a, T> {
    pub section: &'a Section<T>,
    pub depth: usize,
    pub folded: bool,
}

/// The tree a screen shows, its folds, its cursor and its selection.
#[derive(Clone, Debug)]
pub struct Tree<T> {
    roots: Vec<Section<T>>,
    /// Sections whose fold the person changed from how they start.
    toggled: BTreeSet<String>,
    /// The id of the section under the cursor.
    cursor: Option<String>,
    /// The other end of the selection, by id; the cursor is this end.
    anchor: Option<String>,
}

impl<T> Default for Tree<T> {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            toggled: BTreeSet::new(),
            cursor: None,
            anchor: None,
        }
    }
}

impl<T> Tree<T> {
    /// Takes new content. Folds and the cursor stay with their ids; a cursor whose section
    /// is gone moves to the row that is now where it was.
    pub fn set(&mut self, roots: Vec<Section<T>>) {
        let at = self.cursor_index();
        self.roots = roots;
        if self
            .cursor
            .as_ref()
            .is_some_and(|id| self.index_of(id).is_none())
        {
            self.cursor = None;
        }
        if self.cursor.is_none() {
            self.place(at.unwrap_or(0));
        }
        if self
            .anchor
            .as_ref()
            .is_some_and(|id| self.index_of(id).is_none())
        {
            self.anchor = None;
        }
    }

    fn is_folded(&self, section: &Section<T>) -> bool {
        !section.children.is_empty()
            && (section.starts_folded != self.toggled.contains(&section.id))
    }

    /// The rows shown: every section whose parents are unfolded, depth first.
    pub fn rows(&self) -> Vec<Row<'_, T>> {
        fn walk<'a, T>(
            tree: &Tree<T>,
            sections: &'a [Section<T>],
            depth: usize,
            out: &mut Vec<Row<'a, T>>,
        ) {
            for section in sections {
                let folded = tree.is_folded(section);
                out.push(Row {
                    section,
                    depth,
                    folded,
                });
                if !folded {
                    walk(tree, &section.children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &self.roots, 0, &mut out);
        out
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.rows().iter().position(|row| row.section.id == id)
    }

    /// The row of the cursor.
    pub fn cursor_index(&self) -> Option<usize> {
        self.index_of(self.cursor.as_deref()?)
    }

    /// The section under the cursor.
    pub fn current(&self) -> Option<&Section<T>> {
        let at = self.cursor_index()?;
        self.rows().get(at).map(|row| row.section)
    }

    fn place(&mut self, index: usize) {
        let rows = self.rows();
        let index = index.min(rows.len().saturating_sub(1));
        self.cursor = rows.get(index).map(|row| row.section.id.clone());
    }

    /// Moves the cursor `by` rows, stopping at either end.
    pub fn move_by(&mut self, by: isize) {
        let at = self.cursor_index().unwrap_or(0);
        self.place(at.saturating_add_signed(by));
    }

    pub fn move_to(&mut self, index: usize) {
        self.place(index);
    }

    /// Moves the cursor to the next (`forward`) or previous row whose item `wanted` accepts.
    pub fn jump(&mut self, forward: bool, wanted: impl Fn(&T) -> bool) {
        let rows = self.rows();
        let at = self.cursor_index().unwrap_or(0);
        let found = if forward {
            (at + 1..rows.len()).find(|&i| wanted(&rows[i].section.item))
        } else {
            (0..at).rev().find(|&i| wanted(&rows[i].section.item))
        };
        if let Some(index) = found {
            self.place(index);
        }
    }

    /// Folds or unfolds the section at the cursor; one with no children does nothing.
    pub fn toggle(&mut self) {
        if let Some(id) = self
            .current()
            .filter(|s| !s.children.is_empty())
            .map(|s| s.id.clone())
            && !self.toggled.remove(&id)
        {
            self.toggled.insert(id);
        }
    }

    /// Folds (`true`) or unfolds the section at the cursor.
    pub fn set_fold(&mut self, fold: bool) {
        let Some(section) = self.current().filter(|s| !s.children.is_empty()) else {
            return;
        };
        let id = section.id.clone();
        let changed = fold != section.starts_folded;
        if changed {
            self.toggled.insert(id);
        } else {
            self.toggled.remove(&id);
        }
    }

    /// Folds or unfolds every section, as `z M` and `z R` do in vim.
    pub fn set_all(&mut self, fold: bool) {
        fn walk<T>(sections: &[Section<T>], fold: bool, out: &mut BTreeSet<String>) {
            for section in sections {
                if !section.children.is_empty() && fold != section.starts_folded {
                    out.insert(section.id.clone());
                }
                walk(&section.children, fold, out);
            }
        }
        let at = self.cursor_index();
        self.toggled.clear();
        walk(&self.roots, fold, &mut self.toggled);
        // The cursor's section may be hidden now: keep it on a row that shows.
        if self.cursor_index().is_none() {
            self.place(at.unwrap_or(0));
        }
    }

    /// Starts a selection at the cursor, or ends the one there is.
    pub fn toggle_selection(&mut self) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => self.cursor.clone(),
        };
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Extends the selection by `by` rows, starting it at the cursor first: Shift+arrows.
    pub fn extend(&mut self, by: isize) {
        if self.anchor.is_none() {
            self.anchor = self.cursor.clone();
        }
        self.move_by(by);
    }

    /// The selected rows, first and last; `None` without a selection.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.index_of(self.anchor.as_deref()?)?;
        let cursor = self.cursor_index()?;
        Some((anchor.min(cursor), anchor.max(cursor)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree<&'static str> {
        let mut tree = Tree::default();
        tree.set(vec![
            Section::with(
                "unstaged",
                "Unstaged",
                vec![
                    Section::with(
                        "unstaged/a.rs",
                        "a.rs",
                        vec![
                            Section::leaf("unstaged/a.rs/h1", "hunk 1"),
                            Section::leaf("unstaged/a.rs/h2", "hunk 2"),
                        ],
                    )
                    .folded(),
                    Section::leaf("unstaged/b.rs", "b.rs"),
                ],
            ),
            Section::with(
                "staged",
                "Staged",
                vec![Section::leaf("staged/c.rs", "c.rs")],
            ),
        ]);
        tree
    }

    fn shown(tree: &Tree<&'static str>) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|row| {
                format!(
                    "{}{}{}",
                    "  ".repeat(row.depth),
                    if row.folded { "+" } else { "" },
                    row.section.item
                )
            })
            .collect()
    }

    #[test]
    fn files_start_folded_and_tab_unfolds_them() {
        let mut tree = tree();
        assert_eq!(
            shown(&tree),
            ["Unstaged", "  +a.rs", "  b.rs", "Staged", "  c.rs"]
        );
        tree.move_by(1);
        tree.toggle();
        assert_eq!(
            shown(&tree),
            [
                "Unstaged",
                "  a.rs",
                "    hunk 1",
                "    hunk 2",
                "  b.rs",
                "Staged",
                "  c.rs"
            ]
        );
        tree.toggle();
        assert_eq!(shown(&tree).len(), 5);
    }

    #[test]
    fn folds_and_the_cursor_survive_new_content() {
        let mut tree = tree();
        tree.move_by(1);
        tree.toggle(); // unfold a.rs
        tree.move_by(2); // hunk 2
        assert_eq!(tree.current().map(|s| s.item), Some("hunk 2"));
        let again = tree.rows().len();
        let roots = tree.roots.clone();
        tree.set(roots);
        assert_eq!(tree.rows().len(), again);
        assert_eq!(tree.current().map(|s| s.item), Some("hunk 2"));
        // hunk 2 is staged: the cursor stays on the row where it was.
        let mut roots = tree.roots.clone();
        roots[0].children[0].children.pop();
        tree.set(roots);
        assert_eq!(tree.current().map(|s| s.item), Some("b.rs"));
    }

    #[test]
    fn everything_folds_and_unfolds_and_the_cursor_stays_visible() {
        let mut tree = tree();
        tree.move_by(1);
        tree.toggle();
        tree.move_by(2); // hunk 2
        tree.set_all(true);
        assert_eq!(shown(&tree), ["+Unstaged", "+Staged"]);
        assert!(tree.current().is_some());
        tree.set_all(false);
        assert_eq!(shown(&tree).len(), 7);
    }

    #[test]
    fn a_selection_spans_rows_from_the_anchor_to_the_cursor() {
        let mut tree = tree();
        tree.move_by(1);
        tree.toggle();
        tree.move_by(1); // hunk 1
        tree.extend(1); // hunk 2
        assert_eq!(tree.selection(), Some((2, 3)));
        tree.extend(-2);
        assert_eq!(tree.selection(), Some((1, 2)));
        tree.clear_selection();
        assert_eq!(tree.selection(), None);
    }

    #[test]
    fn jump_goes_to_the_next_row_of_a_kind() {
        let mut tree = tree();
        tree.jump(true, |item| item.contains(".rs"));
        assert_eq!(tree.current().map(|s| s.item), Some("a.rs"));
        tree.jump(true, |item| item.contains(".rs"));
        assert_eq!(tree.current().map(|s| s.item), Some("b.rs"));
        tree.jump(false, |item| item.starts_with("Un"));
        assert_eq!(tree.current().map(|s| s.item), Some("Unstaged"));
    }
}
