//! Names the two sides of a conflict by their role (D22, D114). Pure: the
//! adapter reads the facts from Git, and this module only chooses the names.
//! The branch name is also the label the conflict markers carry.

use gm_schema::{ConflictOperation, ConflictSide};

/// A commit one side of the conflict comes from, as Git showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideCommit {
    /// The branch that names the commit, when one does.
    pub branch: Option<String>,
    /// The short id, used as the name when `branch` is `None`.
    pub short: String,
    /// The subject line of the commit.
    pub subject: String,
}

/// The facts behind the names. `head` is HEAD. `incoming` is the commit the
/// operation brings in (`None` when the operation does not say which, as after
/// a stash apply). `onto` is the base a rebase replays onto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideFacts {
    pub operation: ConflictOperation,
    pub head: SideCommit,
    pub incoming: Option<SideCommit>,
    pub onto: Option<SideCommit>,
}

/// The left side (stage 2) and the right side (stage 3) of a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideLabels {
    pub left: ConflictSide,
    pub right: ConflictSide,
}

/// Names the sides by role. A merge is HEAD against the merged branch; a
/// cherry-pick and a revert are HEAD against the commit they bring in. A rebase
/// is the new parent (`onto`) against the commit being replayed.
#[must_use]
pub fn conflict_labels(facts: &SideFacts) -> SideLabels {
    let current = side(&facts.head, "current branch");
    let incoming = |role: &str| match &facts.incoming {
        Some(commit) => side(commit, role),
        None => ConflictSide {
            branch: "other side".to_owned(),
            role: "other side".to_owned(),
            subject: String::new(),
        },
    };
    match facts.operation {
        ConflictOperation::Rebase => SideLabels {
            left: facts.onto.as_ref().map_or_else(
                || side(&facts.head, "new parent"),
                |onto| side(onto, "new parent"),
            ),
            right: incoming("commit being replayed"),
        },
        ConflictOperation::Merge => SideLabels {
            left: current,
            right: incoming("merged branch"),
        },
        ConflictOperation::CherryPick => SideLabels {
            left: current,
            right: incoming("picked commit"),
        },
        ConflictOperation::Revert => SideLabels {
            left: current,
            right: incoming("reverted commit"),
        },
        ConflictOperation::Unknown => SideLabels {
            left: current,
            right: incoming("other side"),
        },
    }
}

fn side(commit: &SideCommit, role: &str) -> ConflictSide {
    ConflictSide {
        branch: commit
            .branch
            .clone()
            .unwrap_or_else(|| commit.short.clone()),
        role: role.to_owned(),
        subject: commit.subject.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(branch: Option<&str>, short: &str, subject: &str) -> SideCommit {
        SideCommit {
            branch: branch.map(str::to_owned),
            short: short.to_owned(),
            subject: subject.to_owned(),
        }
    }

    fn facts(operation: ConflictOperation, incoming: bool, onto: bool) -> SideFacts {
        SideFacts {
            operation,
            head: commit(Some("main"), "1111111", "head subject"),
            incoming: incoming.then(|| commit(Some("feature"), "2222222", "incoming subject")),
            onto: onto.then(|| commit(None, "3333333", "onto subject")),
        }
    }

    #[test]
    fn a_merge_names_the_current_and_the_merged_branch() {
        let labels = conflict_labels(&facts(ConflictOperation::Merge, true, false));
        assert_eq!(labels.left.branch, "main");
        assert_eq!(labels.left.role, "current branch");
        assert_eq!(labels.left.subject, "head subject");
        assert_eq!(labels.right.branch, "feature");
        assert_eq!(labels.right.role, "merged branch");
        assert_eq!(labels.right.subject, "incoming subject");
    }

    #[test]
    fn a_rebase_names_the_new_parent_and_the_replayed_commit() {
        let labels = conflict_labels(&facts(ConflictOperation::Rebase, true, true));
        assert_eq!(labels.left.branch, "3333333");
        assert_eq!(labels.left.role, "new parent");
        assert_eq!(labels.left.subject, "onto subject");
        assert_eq!(labels.right.branch, "feature");
        assert_eq!(labels.right.role, "commit being replayed");
        assert_eq!(labels.right.subject, "incoming subject");
    }

    #[test]
    fn a_rebase_without_an_onto_falls_back_to_head() {
        let labels = conflict_labels(&facts(ConflictOperation::Rebase, true, false));
        assert_eq!(labels.left.branch, "main");
        assert_eq!(labels.left.role, "new parent");
    }

    #[test]
    fn a_cherry_pick_and_a_revert_name_the_commit_they_bring_in() {
        let pick = conflict_labels(&facts(ConflictOperation::CherryPick, true, false));
        assert_eq!(pick.left.role, "current branch");
        assert_eq!(pick.right.role, "picked commit");
        let revert = conflict_labels(&facts(ConflictOperation::Revert, true, false));
        assert_eq!(revert.left.role, "current branch");
        assert_eq!(revert.right.role, "reverted commit");
        assert_eq!(revert.right.subject, "incoming subject");
    }

    #[test]
    fn a_detached_head_is_named_by_its_short_id() {
        let mut detached = facts(ConflictOperation::Merge, true, false);
        detached.head = commit(None, "1111111", "head subject");
        let labels = conflict_labels(&detached);
        assert_eq!(labels.left.branch, "1111111");
    }

    #[test]
    fn an_unknown_operation_without_an_incoming_commit_still_names_both_sides() {
        let labels = conflict_labels(&facts(ConflictOperation::Unknown, false, false));
        assert_eq!(labels.left.role, "current branch");
        assert_eq!(labels.right.branch, "other side");
        assert_eq!(labels.right.role, "other side");
        assert_eq!(labels.right.subject, "");
    }
}
