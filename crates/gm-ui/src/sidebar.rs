//! The rows of the sidebar's branch tree. Pure: the view draws what this lists.
//!
//! No branch has a stack parent yet (slice 4), so "By stack" shows an empty
//! "Stacks" section above every local branch under "No stack". "By name" is
//! the classic view: local, then remote, grouped by the prefix before the
//! first slash.

use gm_schema::BranchEntry;

/// A section of the sidebar that folds away under its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Workspace,
    Branches,
    Worktrees,
}

/// Which sections are folded. All are open at the start; the choice is kept for
/// the window only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Folded {
    workspace: bool,
    branches: bool,
    worktrees: bool,
}

impl Folded {
    #[must_use]
    pub fn is_folded(self, section: Section) -> bool {
        match section {
            Section::Workspace => self.workspace,
            Section::Branches => self.branches,
            Section::Worktrees => self.worktrees,
        }
    }

    pub fn toggle(&mut self, section: Section) {
        let flag = match section {
            Section::Workspace => &mut self.workspace,
            Section::Branches => &mut self.branches,
            Section::Worktrees => &mut self.worktrees,
        };
        *flag = !*flag;
    }
}

/// How the branch tree is organised.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    ByStack,
    ByName,
}

impl Grouping {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ByStack => "By stack",
            Self::ByName => "By name",
        }
    }
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
        indented: bool,
    },
}

/// The tree's rows for `branches` under `grouping`: the branches the filter
/// kept, in the order listed. Empty until a listing has arrived. A header whose
/// section has no branch left is not drawn.
#[must_use]
pub fn rows<'a>(branches: &[&'a BranchEntry], grouping: Grouping) -> Vec<Row<'a>> {
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
        Grouping::ByStack => {
            rows.push(Row::Header("Stacks"));
            rows.push(Row::Note("No stacks yet"));
            if !local.is_empty() {
                rows.push(Row::Header("No stack"));
                rows.extend(local.into_iter().map(|branch| Row::Branch {
                    branch,
                    label: &branch.name,
                    indented: false,
                }));
            }
        }
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
                indented: true,
            });
        } else {
            open = None;
            rows.push(Row::Branch {
                branch,
                label: &branch.name,
                indented: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_start_open_and_fold_one_at_a_time() {
        let mut folded = Folded::default();
        assert!(!folded.is_folded(Section::Branches));
        folded.toggle(Section::Branches);
        assert!(folded.is_folded(Section::Branches));
        assert!(!folded.is_folded(Section::Workspace));
        assert!(!folded.is_folded(Section::Worktrees));
        folded.toggle(Section::Branches);
        assert!(!folded.is_folded(Section::Branches));
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
                Row::Branch {
                    label, indented, ..
                } => format!("{}{label}", if *indented { "  " } else { "" }),
            })
            .collect()
    }

    #[test]
    fn nothing_is_listed_before_a_listing_arrives() {
        assert!(rows(&[], Grouping::ByStack).is_empty());
        assert!(rows(&[], Grouping::ByName).is_empty());
    }

    #[test]
    fn by_stack_puts_every_local_branch_under_no_stack() {
        let all = [
            branch("main", false),
            branch("origin/main", true),
            branch("topic", false),
        ];
        let shown = rows(&refs(&all), Grouping::ByStack);
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
        let shown = rows(&refs(&all), Grouping::ByName);
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
        let shown = rows(&refs(&all), Grouping::ByName);
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
            texts(&rows(&kept, Grouping::ByName)),
            ["# Local", "main", "# Remote", "origin/", "  feature/a"]
        );
        let local_gone = [&all[2]];
        assert_eq!(
            texts(&rows(&local_gone, Grouping::ByName)),
            ["# Remote", "origin/", "  feature/a"]
        );
    }

    #[test]
    fn by_stack_drops_the_no_stack_header_when_no_local_branch_is_left() {
        let all = [branch("origin/main", true)];
        let shown = rows(&refs(&all), Grouping::ByStack);
        assert_eq!(texts(&shown), ["# Stacks", "~ No stacks yet"]);
    }

    #[test]
    fn nothing_kept_gives_no_rows() {
        assert!(rows(&[], Grouping::ByName).is_empty());
        assert!(rows(&[], Grouping::ByStack).is_empty());
    }
}
