//! The rows of the sidebar's branch trees. Pure: the view draws what this lists.
//!
//! The Stack section shows the local branches that are in a stack as trees of recorded
//! parents, each parent before its children and indented by depth (D138). The Branches
//! section is the classic view: local, then remote, grouped by the prefix before the first
//! slash. `rows` still builds both groupings, with "No stack" for the local branches in no
//! tree, so a filter can be tested on either.

use gm_schema::{BranchEntry, Feature, FeatureSet, StackBranch};

use crate::stacks::Stacks;

/// A section of the sidebar that folds away under its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The topic's stack tree (UI redesign, slice 1): the stacks of the repository until
    /// topics exist.
    Stack,
    Branches,
    Worktrees,
    /// The workspaces under the root (D183). It becomes a topic action in slice 5.
    Workspaces,
}

/// Which sections are folded. When a window starts, Stack is open and the others are folded
/// (D184, UI redesign); the choice is kept for the window only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Folded {
    /// Indexed by `Section::index`.
    folded: [bool; 4],
}

impl Default for Folded {
    /// The state a new window starts in: Stack open, the others folded.
    fn default() -> Self {
        Self {
            folded: [false, true, true, true],
        }
    }
}

impl Section {
    fn index(self) -> usize {
        match self {
            Section::Stack => 0,
            Section::Branches => 1,
            Section::Worktrees => 2,
            Section::Workspaces => 3,
        }
    }
}

impl Folded {
    #[must_use]
    pub fn is_folded(self, section: Section) -> bool {
        self.folded[section.index()]
    }

    pub fn toggle(&mut self, section: Section) {
        let flag = &mut self.folded[section.index()];
        *flag = !*flag;
    }
}

/// How a tree is organised: the stack tree of the Stack section, or the classic tree of the
/// Branches section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    ByStack,
    ByName,
}

/// One row of the tree. Every row is drawn at the same height.
#[derive(Debug, PartialEq, Eq)]
pub enum Row<'a> {
    Header(&'static str),
    /// A muted line standing in for a section with nothing to show.
    Note(&'static str),
    /// A prefix such as `feature/`, above the branches that share it.
    Group(&'a str),
    Branch {
        branch: &'a BranchEntry,
        /// The name as drawn: without the group's prefix when under a group.
        label: &'a str,
        /// Levels of indent: under a prefix group, or the depth in a stack.
        indent: u32,
        /// The branch's place in its stack, in the By stack view.
        stack: Option<&'a StackBranch>,
        /// The branch's recorded parents form a loop.
        looped: bool,
    },
}

/// Whether the hint that says how to set a parent shows under the sidebar. It describes the
/// drag in the stack tree, so it shows only while that tree is drawn: the Stacks preview on and
/// the Stack section open.
#[must_use]
pub fn drag_hint_shown(features: FeatureSet, folded: Folded) -> bool {
    features.is_enabled(Feature::Stacks) && !folded.is_folded(Section::Stack)
}

/// The rows of the Stack section: each stack's branches, parents first and indented by depth,
/// then the branches of a parent loop, with no section header (the section has its own). A note stands in while the first
/// snapshot is read and when there is no stack.
#[must_use]
pub fn stack_rows<'a>(branches: &[&'a BranchEntry], stacks: &'a Stacks) -> Vec<Row<'a>> {
    let mut rows = rows(branches, Grouping::ByStack, stacks);
    if let Some(end) = rows.iter().position(|row| *row == Row::Header("No stack")) {
        // The branches of a parent loop stay, after the trees, with their loop mark.
        let looped: Vec<Row<'a>> = rows
            .drain(end..)
            .filter(|row| matches!(row, Row::Branch { looped: true, .. }))
            .collect();
        rows.extend(looped);
    }
    if rows.first() == Some(&Row::Header("Stacks")) {
        rows.remove(0);
    }
    if rows.is_empty() {
        rows.push(Row::Note(if stacks.calculating() {
            "Calculating…"
        } else {
            "No stacks yet"
        }));
    }
    rows
}

/// The tree's rows for `branches` under `grouping`: the branches the filter
/// kept, in the order listed, and in "By stack" in the order of `stacks`. Empty
/// until a listing has arrived. A header whose section has no branch left is
/// not drawn.
#[must_use]
pub fn rows<'a>(
    branches: &[&'a BranchEntry],
    grouping: Grouping,
    stacks: &'a Stacks,
) -> Vec<Row<'a>> {
    let mut rows = Vec::new();
    if branches.is_empty() {
        return rows;
    }
    let local: Vec<&'a BranchEntry> = branches
        .iter()
        .copied()
        .filter(|branch| !branch.is_remote)
        .collect();
    let remote: Vec<&'a BranchEntry> = branches
        .iter()
        .copied()
        .filter(|branch| branch.is_remote)
        .collect();
    match grouping {
        Grouping::ByStack => by_stack(&mut rows, &local, stacks),
        Grouping::ByName => {
            if !local.is_empty() {
                rows.push(Row::Header("Local"));
                grouped(&mut rows, local.into_iter());
            }
            if !remote.is_empty() {
                rows.push(Row::Header("Remote"));
                grouped(&mut rows, remote.into_iter());
            }
        }
    }
    rows
}

/// The "Stacks" section, then "No stack" with the local branches in no tree.
/// Until the first snapshot arrives, every branch waits under "No stack".
fn by_stack<'a>(rows: &mut Vec<Row<'a>>, local: &[&'a BranchEntry], stacks: &'a Stacks) {
    rows.push(Row::Header("Stacks"));
    let mut stacked = Vec::new();
    if stacks.calculating() {
        rows.push(Row::Note("Calculating…"));
    } else {
        let entries = stacks
            .snapshot()
            .into_iter()
            .flat_map(|snapshot| &snapshot.stacks)
            .flat_map(|stack| &stack.branches);
        for entry in entries {
            if let Some(branch) = local.iter().find(|branch| branch.name == entry.name) {
                stacked.push(entry.name.as_str());
                rows.push(Row::Branch {
                    branch,
                    label: &branch.name,
                    indent: entry.depth,
                    stack: Some(entry),
                    looped: false,
                });
            }
        }
        if stacked.is_empty() {
            rows.push(Row::Note("No stacks yet"));
        }
    }
    let alone: Vec<&'a BranchEntry> = local
        .iter()
        .copied()
        .filter(|branch| !stacked.contains(&branch.name.as_str()))
        .collect();
    if !alone.is_empty() {
        rows.push(Row::Header("No stack"));
        rows.extend(alone.into_iter().map(|branch| Row::Branch {
            branch,
            label: &branch.name,
            indent: 0,
            stack: None,
            looped: stacks.looped(&branch.name),
        }));
    }
}

/// Appends `branches` with a `Group` row before each run of branches that share
/// a prefix. A name without a slash stays at the top level. The listing is
/// sorted by name, so the members of a group are already adjacent.
fn grouped<'a>(rows: &mut Vec<Row<'a>>, branches: impl Iterator<Item = &'a BranchEntry>) {
    let mut open: Option<&'a str> = None;
    for branch in branches {
        if let Some((prefix, rest)) = branch.name.split_once('/') {
            if open != Some(prefix) {
                rows.push(Row::Group(prefix));
                open = Some(prefix);
            }
            rows.push(Row::Branch {
                branch,
                label: rest,
                indent: 1,
                stack: None,
                looped: false,
            });
        } else {
            open = None;
            rows.push(Row::Branch {
                branch,
                label: &branch.name,
                indent: 0,
                stack: None,
                looped: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drag_hint_shows_only_in_the_open_stack_section_with_stacks_on() {
        let mut folded = Folded::default();
        assert!(drag_hint_shown(FeatureSet::all(), folded));
        assert!(!drag_hint_shown(FeatureSet::none(), folded));
        folded.toggle(Section::Stack);
        assert!(!drag_hint_shown(FeatureSet::all(), folded));
    }

    #[test]
    fn a_new_window_starts_with_the_stack_open_and_the_folds_folded() {
        let folded = Folded::default();
        assert!(!folded.is_folded(Section::Stack));
        assert!(folded.is_folded(Section::Branches));
        assert!(folded.is_folded(Section::Worktrees));
    }

    #[test]
    fn toggling_a_section_opens_or_folds_only_that_section() {
        let mut folded = Folded::default();
        folded.toggle(Section::Branches);
        assert!(!folded.is_folded(Section::Branches));
        assert!(folded.is_folded(Section::Worktrees));
        assert!(!folded.is_folded(Section::Stack));
        folded.toggle(Section::Branches);
        assert!(folded.is_folded(Section::Branches));
        folded.toggle(Section::Worktrees);
        assert!(!folded.is_folded(Section::Worktrees));
        assert!(folded.is_folded(Section::Branches));
    }

    #[test]
    fn the_stack_section_lists_only_the_stacked_branches() {
        let all = [
            branch("main", false),
            branch("ledger-base", false),
            branch("ledger-ui", false),
            branch("lonely", false),
            branch("a", false),
        ];
        let stacks = ledger();
        let shown = texts(&stack_rows(&refs(&all), &stacks));
        assert!(shown.iter().all(|row| !row.contains("lonely")), "{shown:?}");
        assert!(
            shown.iter().all(|row| !row.contains("No stack")),
            "{shown:?}"
        );
        assert!(!shown.is_empty());
        assert!(shown.iter().any(|row| row == "a"), "{shown:?}");
        let none = texts(&stack_rows(&refs(&all), &Stacks::default()));
        assert_eq!(none, ["~ No stacks yet"]);
    }

    fn branch(name: &str, remote: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: remote,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    /// The branches as the filter hands them over: all of them, in order.
    fn refs(all: &[BranchEntry]) -> Vec<&BranchEntry> {
        all.iter().collect()
    }

    /// The rows as short text, for comparing whole trees.
    fn texts(shown: &[Row<'_>]) -> Vec<String> {
        shown
            .iter()
            .map(|row| match row {
                Row::Header(title) => format!("# {title}"),
                Row::Note(text) => format!("~ {text}"),
                Row::Group(prefix) => format!("{prefix}/"),
                Row::Branch { label, indent, .. } => {
                    format!("{}{label}", "  ".repeat(*indent as usize))
                }
            })
            .collect()
    }

    #[test]
    fn nothing_is_listed_before_a_listing_arrives() {
        assert!(rows(&[], Grouping::ByStack, &Stacks::default()).is_empty());
        assert!(rows(&[], Grouping::ByName, &Stacks::default()).is_empty());
    }

    #[test]
    fn by_stack_puts_every_local_branch_under_no_stack() {
        let all = [
            branch("main", false),
            branch("origin/main", true),
            branch("topic", false),
        ];
        let none = Stacks::default();
        let shown = rows(&refs(&all), Grouping::ByStack, &none);
        assert_eq!(shown[0], Row::Header("Stacks"));
        assert_eq!(shown[1], Row::Note("No stacks yet"));
        assert_eq!(shown[2], Row::Header("No stack"));
        let names: Vec<&str> = shown[3..]
            .iter()
            .map(|row| match row {
                Row::Branch { label, .. } => *label,
                other => panic!("unexpected row {other:?}"),
            })
            .collect();
        assert_eq!(names, ["main", "topic"]);
    }

    #[test]
    fn by_name_groups_a_shared_prefix_and_lists_remotes_last() {
        let all = [
            branch("feature/a", false),
            branch("feature/b", false),
            branch("main", false),
            branch("origin/main", true),
        ];
        let none = Stacks::default();
        let shown = rows(&refs(&all), Grouping::ByName, &none);
        assert_eq!(
            texts(&shown),
            [
                "# Local", "feature/", "  a", "  b", "main", "# Remote", "origin/", "  main"
            ]
        );
    }

    #[test]
    fn by_name_leaves_out_an_empty_remote_section() {
        let all = [branch("main", false)];
        let none = Stacks::default();
        let shown = rows(&refs(&all), Grouping::ByName, &none);
        assert!(!shown.contains(&Row::Header("Remote")));
    }

    #[test]
    fn a_filtered_out_group_leaves_no_header_behind() {
        let all = [
            branch("feature/a", false),
            branch("main", false),
            branch("origin/feature/a", true),
        ];
        // The filter kept `main` and the remote branch only.
        let kept = [&all[1], &all[2]];
        assert_eq!(
            texts(&rows(&kept, Grouping::ByName, &Stacks::default())),
            ["# Local", "main", "# Remote", "origin/", "  feature/a"]
        );
        let local_gone = [&all[2]];
        assert_eq!(
            texts(&rows(&local_gone, Grouping::ByName, &Stacks::default())),
            ["# Remote", "origin/", "  feature/a"]
        );
    }

    #[test]
    fn by_stack_drops_the_no_stack_header_when_no_local_branch_is_left() {
        let all = [branch("origin/main", true)];
        let none = Stacks::default();
        let shown = rows(&refs(&all), Grouping::ByStack, &none);
        assert_eq!(texts(&shown), ["# Stacks", "~ No stacks yet"]);
    }

    #[test]
    fn nothing_kept_gives_no_rows() {
        assert!(rows(&[], Grouping::ByName, &Stacks::default()).is_empty());
        assert!(rows(&[], Grouping::ByStack, &Stacks::default()).is_empty());
    }

    fn stack_entry(name: &str, parent: Option<&str>, depth: u32) -> gm_schema::StackBranch {
        gm_schema::StackBranch {
            name: name.into(),
            parent: parent.map(str::to_owned),
            depth,
            state: if parent.is_some() {
                gm_schema::StackState::Current
            } else {
                gm_schema::StackState::Root
            },
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        }
    }

    fn ledger() -> Stacks {
        let mut stacks = Stacks::default();
        stacks.loaded(
            gm_schema::OperationId(1),
            gm_schema::StackSnapshot {
                stacks: vec![gm_schema::Stack {
                    branches: vec![
                        stack_entry("main", None, 0),
                        stack_entry("ledger-base", Some("main"), 1),
                        stack_entry("ledger-ui", Some("ledger-base"), 2),
                    ],
                }],
                looped: vec!["a".into()],
            },
        );
        stacks
    }

    #[test]
    fn by_stack_draws_each_tree_then_the_branches_in_none() {
        let all = [
            branch("a", false),
            branch("ledger-base", false),
            branch("ledger-ui", false),
            branch("lone", false),
            branch("main", false),
            branch("origin/main", true),
        ];
        let stacks = ledger();
        let shown = rows(&refs(&all), Grouping::ByStack, &stacks);
        assert_eq!(
            texts(&shown),
            [
                "# Stacks",
                "main",
                "  ledger-base",
                "    ledger-ui",
                "# No stack",
                "a",
                "lone"
            ]
        );
        let looped: Vec<&str> = shown
            .iter()
            .filter_map(|row| match row {
                Row::Branch {
                    label,
                    looped: true,
                    ..
                } => Some(*label),
                _ => None,
            })
            .collect();
        assert_eq!(looped, ["a"]);
        assert!(matches!(
            shown[2],
            Row::Branch {
                stack: Some(entry),
                ..
            } if entry.parent.as_deref() == Some("main")
        ));
    }

    #[test]
    fn a_filter_keeps_the_depth_of_the_stack_branches_it_shows() {
        let all = [branch("ledger-ui", false), branch("main", false)];
        let stacks = ledger();
        assert_eq!(
            texts(&rows(&refs(&all), Grouping::ByStack, &stacks)),
            ["# Stacks", "main", "    ledger-ui"]
        );
    }

    #[test]
    fn by_stack_says_calculating_until_the_first_snapshot() {
        let all = [branch("main", false)];
        let mut stacks = Stacks::default();
        stacks.request(gm_schema::OperationId(3));
        assert_eq!(
            texts(&rows(&refs(&all), Grouping::ByStack, &stacks)),
            ["# Stacks", "~ Calculating…", "# No stack", "main"]
        );
    }
}
