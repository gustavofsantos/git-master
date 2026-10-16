//! Change-list tree: grouping by directory, tri-state checkboxes, filter.
//!
//! Pure data. The view turns `rows()` into a virtualized list and forwards
//! keys to it; nothing here reads the repository, so the tree is rebuilt from
//! the status the engine reported.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use gm_schema::StatusEntry;

/// The state of a checkbox: whether a file, or every file under a folder, is staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Checked,
    Unchecked,
    Mixed,
}

/// One line of the list. Within a directory, folders come before files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Dir {
        /// Full path of the folder from the repository root.
        path: String,
        /// The segments shown on the row; several when a chain is compressed.
        name: String,
        depth: usize,
        expanded: bool,
        check: Check,
    },
    File {
        /// Index into the `entries` slice the tree was built from.
        entry_index: usize,
        path: String,
        name: String,
        depth: usize,
        check: Check,
    },
}

/// What a toggle on one row does to the index, as file paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToggleAction {
    Stage(Vec<String>),
    Unstage(Vec<String>),
}

/// Counts over the whole status, ignoring the filter. Conflicted and untracked
/// files count only under their own field; a file staged and unstaged counts
/// under both `staged` and `unstaged`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Leaf {
    path: String,
    check: Check,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeTree {
    rows: Vec<Row>,
    /// Per row, the range of `leaves` it covers. A file covers its own leaf
    /// (none when conflicted); a folder covers every descendant file, even
    /// those hidden by a collapse.
    spans: Vec<Range<usize>>,
    /// Every non-conflicted file that passed the filter, in tree order.
    leaves: Vec<Leaf>,
    summary: Summary,
}

impl ChangeTree {
    /// Builds the list from `entries`. Only paths containing `filter`
    /// (case-insensitive) are kept; folders left empty disappear. `collapsed`
    /// holds the full paths of folders whose rows are hidden.
    #[must_use]
    pub fn build(entries: &[StatusEntry], filter: &str, collapsed: &HashSet<String>) -> Self {
        let needle = filter.to_lowercase();
        let mut root = Node::default();
        for (index, entry) in entries.iter().enumerate() {
            if !entry.path.to_lowercase().contains(&needle) {
                continue;
            }
            let parts: Vec<&str> = entry.path.split('/').collect();
            let Some((_, folders)) = parts.split_last() else {
                continue;
            };
            let mut node = &mut root;
            for folder in folders {
                node = node.dirs.entry((*folder).to_owned()).or_default();
            }
            node.files.push(index);
        }
        let mut walk = Walk {
            entries,
            collapsed,
            tree: ChangeTree {
                summary: summarize(entries),
                ..ChangeTree::default()
            },
        };
        walk.children(&root, "", 0, true);
        walk.tree
    }

    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    #[must_use]
    pub fn summary(&self) -> Summary {
        self.summary
    }

    /// The index into `entries` of the file on `row_index`, if that row is a file.
    #[must_use]
    pub fn file_at(&self, row_index: usize) -> Option<usize> {
        match self.rows.get(row_index)? {
            Row::File { entry_index, .. } => Some(*entry_index),
            Row::Dir { .. } => None,
        }
    }

    /// The row of the first file strictly after `from_row`; folders are skipped.
    #[must_use]
    pub fn next_file(&self, from_row: usize) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .skip(from_row.saturating_add(1))
            .find(|(_, row)| row.is_file())
            .map(|(index, _)| index)
    }

    /// The row of the last file strictly before `from_row`; folders are skipped.
    #[must_use]
    pub fn prev_file(&self, from_row: usize) -> Option<usize> {
        let end = from_row.min(self.rows.len());
        self.rows[..end].iter().rposition(Row::is_file)
    }

    /// What toggling `row_index` does. A folder that is fully checked is
    /// unstaged; any other folder stages the descendant files that are not
    /// fully checked. Conflicted files are never listed.
    #[must_use]
    pub fn toggle_action(&self, row_index: usize) -> ToggleAction {
        let Some(row) = self.rows.get(row_index) else {
            return ToggleAction::Stage(Vec::new());
        };
        let leaves = &self.leaves[self.spans[row_index].clone()];
        if row.check() == Check::Checked {
            ToggleAction::Unstage(leaves.iter().map(|leaf| leaf.path.clone()).collect())
        } else {
            ToggleAction::Stage(
                leaves
                    .iter()
                    .filter(|leaf| leaf.check != Check::Checked)
                    .map(|leaf| leaf.path.clone())
                    .collect(),
            )
        }
    }
}

impl Row {
    fn check(&self) -> Check {
        match self {
            Row::Dir { check, .. } | Row::File { check, .. } => *check,
        }
    }

    fn is_file(&self) -> bool {
        matches!(self, Row::File { .. })
    }
}

/// A folder in the path trie. Only folders and files that passed the filter exist.
#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    /// Indices into `entries` of the files directly inside this folder.
    files: Vec<usize>,
}

struct Walk<'a> {
    entries: &'a [StatusEntry],
    collapsed: &'a HashSet<String>,
    tree: ChangeTree,
}

impl Walk<'_> {
    /// Emits the rows under `node`, folders first. `visible` is false below a
    /// collapsed folder: rows are then skipped, but the leaves are still
    /// recorded so the folder's checkbox and toggle stay correct.
    fn children(&mut self, node: &Node, parent: &str, depth: usize, visible: bool) {
        let mut folders: Vec<(&str, &Node)> = node
            .dirs
            .iter()
            .map(|(name, child)| (name.as_str(), child))
            .collect();
        folders.sort_by_cached_key(|(name, _)| sort_key(name));
        for (name, child) in folders {
            let (segments, last) = compress(name, child);
            let joined = segments.join("/");
            let path = if parent.is_empty() {
                joined.clone()
            } else {
                format!("{parent}/{joined}")
            };
            self.dir(last, &path, joined, depth, visible);
        }

        let entries = self.entries;
        let mut files = node.files.clone();
        files.sort_by_cached_key(|&index| sort_key(file_name(&entries[index].path)));
        for index in files {
            self.file(index, depth, visible);
        }
    }

    fn dir(&mut self, node: &Node, path: &str, name: String, depth: usize, visible: bool) {
        let expanded = !self.collapsed.contains(path);
        let row = visible.then(|| {
            self.push(
                Row::Dir {
                    path: path.to_owned(),
                    name,
                    depth,
                    expanded,
                    check: Check::Unchecked,
                },
                0..0,
            )
        });
        let start = self.tree.leaves.len();
        self.children(node, path, depth + 1, visible && expanded);
        let span = start..self.tree.leaves.len();
        if let Some(row) = row {
            let check = aggregate(&self.tree.leaves[span.clone()]);
            if let Row::Dir { check: slot, .. } = &mut self.tree.rows[row] {
                *slot = check;
            }
            self.tree.spans[row] = span;
        }
    }

    fn file(&mut self, index: usize, depth: usize, visible: bool) {
        let entries = self.entries;
        let entry = &entries[index];
        let path = entry.path.clone();
        let check = if entry.conflicted {
            Check::Unchecked
        } else {
            file_check(entry)
        };
        let start = self.tree.leaves.len();
        if !entry.conflicted {
            self.tree.leaves.push(Leaf {
                path: path.clone(),
                check,
            });
        }
        let span = start..self.tree.leaves.len();
        if visible {
            let name = file_name(&path).to_owned();
            self.push(
                Row::File {
                    entry_index: index,
                    path,
                    name,
                    depth,
                    check,
                },
                span,
            );
        }
    }

    fn push(&mut self, row: Row, span: Range<usize>) -> usize {
        self.tree.rows.push(row);
        self.tree.spans.push(span);
        self.tree.rows.len() - 1
    }
}

/// Folds a chain of folders that holds nothing but one folder into one step.
/// Returns the segments of the chain and the folder at its end.
fn compress<'n>(name: &'n str, mut node: &'n Node) -> (Vec<&'n str>, &'n Node) {
    let mut segments = vec![name];
    while node.files.is_empty() && node.dirs.len() == 1 {
        let Some((child_name, child)) = node.dirs.iter().next() else {
            break;
        };
        segments.push(child_name.as_str());
        node = child;
    }
    (segments, node)
}

/// A file checks on when its only change is staged. Untracked files are never
/// staged by a toggle, and a file with both changes is mixed.
fn file_check(entry: &StatusEntry) -> Check {
    match (
        entry.untracked,
        entry.staged.is_some(),
        entry.unstaged.is_some(),
    ) {
        (false, true, false) => Check::Checked,
        (false, true, true) => Check::Mixed,
        _ => Check::Unchecked,
    }
}

/// A folder checks on when every descendant file does, off when every one
/// does not, and is mixed otherwise. No files means off.
fn aggregate(leaves: &[Leaf]) -> Check {
    let mut checks = leaves.iter().map(|leaf| leaf.check);
    let Some(first) = checks.next() else {
        return Check::Unchecked;
    };
    if checks.all(|check| check == first) {
        first
    } else {
        Check::Mixed
    }
}

fn summarize(entries: &[StatusEntry]) -> Summary {
    let mut summary = Summary::default();
    for entry in entries {
        if entry.conflicted {
            summary.conflicted += 1;
        } else if entry.untracked {
            summary.untracked += 1;
        } else {
            summary.staged += usize::from(entry.staged.is_some());
            summary.unstaged += usize::from(entry.unstaged.is_some());
        }
    }
    summary
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Case-insensitive order first, exact order to break ties.
fn sort_key(name: &str) -> (String, String) {
    (name.to_lowercase(), name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Change::{self, Added, Modified};

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
        }
    }

    fn staged(path: &str) -> StatusEntry {
        entry(path, Some(Modified), None)
    }

    fn unstaged(path: &str) -> StatusEntry {
        entry(path, None, Some(Modified))
    }

    fn untracked(path: &str) -> StatusEntry {
        StatusEntry {
            untracked: true,
            ..entry(path, None, None)
        }
    }

    fn conflicted(path: &str) -> StatusEntry {
        StatusEntry {
            conflicted: true,
            ..entry(path, Some(Modified), Some(Modified))
        }
    }

    fn build(entries: &[StatusEntry], filter: &str) -> ChangeTree {
        ChangeTree::build(entries, filter, &HashSet::new())
    }

    /// One line per row: folders end in `/`, indented two spaces per depth.
    fn lines(tree: &ChangeTree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|row| match row {
                Row::Dir { name, depth, .. } => format!("{}{name}/", "  ".repeat(*depth)),
                Row::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    fn checks(tree: &ChangeTree) -> Vec<Check> {
        tree.rows().iter().map(Row::check).collect()
    }

    fn file_rows(tree: &ChangeTree) -> usize {
        tree.rows().iter().filter(|row| row.is_file()).count()
    }

    /// The mixed tree, with entry indices 0..=5 in this order:
    /// rows are docs/ (Checked), d.md, e.md, src/ (Mixed), a.rs, b.rs, c.rs,
    /// README.md (Unchecked).
    fn mixed_entries() -> Vec<StatusEntry> {
        vec![
            staged("src/a.rs"),
            unstaged("src/b.rs"),
            entry("src/c.rs", Some(Modified), Some(Modified)),
            staged("docs/d.md"),
            entry("docs/e.md", Some(Added), None),
            untracked("README.md"),
        ]
    }

    #[test]
    fn folders_come_before_files_and_both_sort_case_insensitively() {
        let entries = [
            staged("zeta.rs"),
            staged("beta/two.rs"),
            staged("Alpha/one.rs"),
            staged("alpha.rs"),
            staged("Beta.rs"),
        ];
        assert_eq!(
            lines(&build(&entries, "")),
            [
                "Alpha/", "  one.rs", "beta/", "  two.rs", "alpha.rs", "Beta.rs", "zeta.rs"
            ]
        );
    }

    #[test]
    fn a_chain_of_single_folders_becomes_one_row() {
        let tree = build(&[staged("a/b/c/x.txt")], "");
        assert_eq!(lines(&tree), ["a/b/c/", "  x.txt"]);
        let Row::Dir { path, depth, .. } = &tree.rows()[0] else {
            panic!("the first row is a folder");
        };
        assert_eq!((path.as_str(), *depth), ("a/b/c", 0));
    }

    #[test]
    fn a_chain_stops_at_the_first_folder_with_two_children() {
        let entries = [staged("a/b/c/x"), staged("a/b/d/y")];
        assert_eq!(
            lines(&build(&entries, "")),
            ["a/b/", "  c/", "    x", "  d/", "    y"]
        );
    }

    #[test]
    fn a_filter_keeps_matches_and_drops_folders_without_them() {
        let entries = [
            staged("src/alpha.rs"),
            staged("src/beta.rs"),
            staged("docs/Alpha.md"),
            staged("lib/gamma.rs"),
        ];
        let expected = ["docs/", "  Alpha.md", "src/", "  alpha.rs"];
        assert_eq!(lines(&build(&entries, "ALPHA")), expected);
        assert_eq!(lines(&build(&entries, "alpha")), expected);
        assert!(build(&entries, "zzz").rows().is_empty());
        assert_eq!(file_rows(&build(&entries, "")), 4);
    }

    #[test]
    fn a_filter_compresses_the_chain_it_leaves_behind() {
        let entries = [staged("a/b/c/x.txt"), staged("a/b/q.md")];
        assert_eq!(lines(&build(&entries, "x.t")), ["a/b/c/", "  x.txt"]);
    }

    #[test]
    fn a_collapsed_folder_hides_its_rows_but_keeps_its_leaves() {
        let entries = [staged("src/a.rs"), unstaged("src/b.rs"), staged("top.md")];
        let collapsed = HashSet::from(["src".to_owned()]);
        let tree = ChangeTree::build(&entries, "", &collapsed);
        assert_eq!(lines(&tree), ["src/", "top.md"]);
        assert!(matches!(
            tree.rows()[0],
            Row::Dir {
                expanded: false,
                ..
            }
        ));
        assert_eq!(checks(&tree)[0], Check::Mixed);
        assert_eq!(
            tree.toggle_action(0),
            ToggleAction::Stage(vec!["src/b.rs".to_owned()])
        );
    }

    #[test]
    fn collapsing_a_compressed_chain_uses_its_full_path() {
        let entries = [staged("a/b/c/x.txt")];
        let collapsed = HashSet::from(["a/b/c".to_owned()]);
        let tree = ChangeTree::build(&entries, "", &collapsed);
        assert_eq!(lines(&tree), ["a/b/c/"]);
    }

    #[test]
    fn a_folder_is_mixed_when_its_files_disagree() {
        let tree = build(&mixed_entries(), "");
        assert_eq!(
            checks(&tree),
            [
                Check::Checked,
                Check::Checked,
                Check::Checked,
                Check::Mixed,
                Check::Checked,
                Check::Unchecked,
                Check::Mixed,
                Check::Unchecked,
            ]
        );
    }

    #[test]
    fn a_folder_of_only_unstaged_and_untracked_is_unchecked() {
        let entries = [unstaged("u/x.rs"), untracked("u/y.rs")];
        let tree = build(&entries, "");
        assert_eq!(
            checks(&tree),
            [Check::Unchecked, Check::Unchecked, Check::Unchecked]
        );
    }

    #[test]
    fn a_folder_checks_only_the_files_that_pass_the_filter() {
        let entries = [staged("src/a.rs"), unstaged("src/b.rs")];
        let tree = build(&entries, "a.rs");
        assert_eq!(lines(&tree), ["src/", "  a.rs"]);
        assert_eq!(checks(&tree), [Check::Checked, Check::Checked]);
    }

    #[test]
    fn toggling_a_checked_folder_unstages_its_files() {
        let tree = build(&mixed_entries(), "");
        assert_eq!(
            tree.toggle_action(0),
            ToggleAction::Unstage(vec!["docs/d.md".to_owned(), "docs/e.md".to_owned()])
        );
    }

    #[test]
    fn toggling_a_mixed_folder_stages_only_what_is_not_checked() {
        let tree = build(&mixed_entries(), "");
        assert_eq!(
            tree.toggle_action(3),
            ToggleAction::Stage(vec!["src/b.rs".to_owned(), "src/c.rs".to_owned()])
        );
    }

    #[test]
    fn toggling_a_file_flips_it() {
        let tree = build(&mixed_entries(), "");
        assert_eq!(
            tree.toggle_action(4),
            ToggleAction::Unstage(vec!["src/a.rs".to_owned()])
        );
        assert_eq!(
            tree.toggle_action(5),
            ToggleAction::Stage(vec!["src/b.rs".to_owned()])
        );
        assert_eq!(
            tree.toggle_action(7),
            ToggleAction::Stage(vec!["README.md".to_owned()])
        );
    }

    #[test]
    fn toggling_a_mixed_file_stages_it() {
        let tree = build(&[entry("m.rs", Some(Modified), Some(Modified))], "");
        assert_eq!(
            tree.toggle_action(0),
            ToggleAction::Stage(vec!["m.rs".to_owned()])
        );
    }

    #[test]
    fn conflicted_are_skipped_by_toggle_and_by_the_folder_check() {
        let entries = [
            conflicted("lib/x.rs"),
            unstaged("lib/y.rs"),
            staged("lib/z.rs"),
        ];
        let tree = build(&entries, "");
        assert_eq!(lines(&tree), ["lib/", "  x.rs", "  y.rs", "  z.rs"]);
        assert_eq!(
            checks(&tree),
            [
                Check::Mixed,
                Check::Unchecked,
                Check::Unchecked,
                Check::Checked
            ]
        );
        assert_eq!(
            tree.toggle_action(0),
            ToggleAction::Stage(vec!["lib/y.rs".to_owned()])
        );
        assert_eq!(tree.toggle_action(1), ToggleAction::Stage(Vec::new()));
    }

    #[test]
    fn a_folder_of_only_conflicted_stages_nothing() {
        let tree = build(&[conflicted("c/x.rs")], "");
        assert_eq!(checks(&tree)[0], Check::Unchecked);
        assert_eq!(tree.toggle_action(0), ToggleAction::Stage(Vec::new()));
    }

    #[test]
    fn navigation_skips_folders_and_stops_at_the_ends() {
        let tree = build(&mixed_entries(), "");
        // Rows: 0 docs/, 1 d.md, 2 e.md, 3 src/, 4 a.rs, 5 b.rs, 6 c.rs, 7 README.md.
        assert_eq!(tree.next_file(0), Some(1));
        assert_eq!(tree.next_file(2), Some(4));
        assert_eq!(tree.next_file(7), None);
        assert_eq!(tree.prev_file(4), Some(2));
        assert_eq!(tree.prev_file(0), None);
        assert_eq!(tree.prev_file(100), Some(7));
        assert_eq!(tree.file_at(3), None);
        // a.rs is the first entry in the slice the tree was built from.
        assert_eq!(tree.file_at(4), Some(0));
        assert_eq!(tree.file_at(99), None);
    }

    #[test]
    fn summary_counts_the_whole_status_whatever_the_filter() {
        let expected = Summary {
            staged: 4,
            unstaged: 2,
            untracked: 1,
            conflicted: 0,
        };
        let entries = mixed_entries();
        assert_eq!(build(&entries, "").summary(), expected);
        assert_eq!(build(&entries, "docs").summary(), expected);
    }

    #[test]
    fn summary_counts_conflicted_under_their_own_field() {
        let entries = [conflicted("x.rs"), untracked("y.rs")];
        assert_eq!(
            build(&entries, "").summary(),
            Summary {
                staged: 0,
                unstaged: 0,
                untracked: 1,
                conflicted: 1,
            }
        );
    }

    #[test]
    fn unicode_paths_sort_filter_and_toggle() {
        let entries = [
            unstaged("docs/café.md"),
            staged("日本/ファイル.txt"),
            entry("Émile/Ünïcode.rs", Some(Modified), Some(Modified)),
        ];
        assert_eq!(
            lines(&build(&entries, "")),
            [
                "docs/",
                "  café.md",
                "Émile/",
                "  Ünïcode.rs",
                "日本/",
                "  ファイル.txt"
            ]
        );
        assert_eq!(lines(&build(&entries, "CAFÉ")), ["docs/", "  café.md"]);
        assert_eq!(lines(&build(&entries, "ファ")), ["日本/", "  ファイル.txt"]);
        let tree = build(&entries, "");
        assert_eq!(
            tree.toggle_action(0),
            ToggleAction::Stage(vec!["docs/café.md".to_owned()])
        );
    }

    #[test]
    fn ten_thousand_entries_build_with_every_file_listed() {
        let entries: Vec<StatusEntry> = (0..10_000_usize)
            .map(|i| unstaged(&format!("dir{}/sub{}/file{i}.rs", i % 100, i % 7)))
            .collect();
        let tree = build(&entries, "");
        assert_eq!(file_rows(&tree), 10_000);
        assert_eq!(tree.summary().unstaged, 10_000);
        // "dir99/sub3" is one compressed folder row, then the file.
        assert_eq!(build(&entries, "file9999").row_count(), 2);
    }

    /// Small xorshift generator, so the property test needs no extra crate.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % n as u64).unwrap_or(0)
        }
    }

    #[test]
    fn every_file_appears_once_when_nothing_is_collapsed_or_filtered() {
        const SEGMENTS: [&str; 5] = ["a", "B", "src", "lib", "日本"];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300 {
            let mut paths = std::collections::BTreeSet::new();
            for _ in 0..rng.below(60) {
                let depth = 1 + rng.below(4);
                let path: Vec<&str> = (0..depth).map(|_| SEGMENTS[rng.below(5)]).collect();
                paths.insert(path.join("/"));
            }
            let entries: Vec<StatusEntry> = paths.iter().map(|p| unstaged(p)).collect();
            let tree = build(&entries, "");
            let mut listed: Vec<String> = tree
                .rows()
                .iter()
                .filter_map(|row| match row {
                    Row::File { path, .. } => Some(path.clone()),
                    Row::Dir { .. } => None,
                })
                .collect();
            listed.sort();
            assert_eq!(listed, paths.into_iter().collect::<Vec<_>>());
        }
    }
}
