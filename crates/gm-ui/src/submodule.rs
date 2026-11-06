//! The submodule rows of the Working Copy (D175), with no GPUI in it: the detail
//! each submodule's read answered, the read in flight for each path, the marks a
//! row shows and the lines of the commit pane. A status asks for every submodule
//! again; an answer to a read that is no longer the one in flight is dropped, as
//! the Reflog list drops one (B3).

use std::collections::{BTreeMap, BTreeSet};

use gm_schema::{
    OperationId, SUBMODULE_COMMIT_LIMIT, StatusEntry, SubmoduleChange, SubmoduleDetail,
};

use crate::shell_history::relative_time;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Submodules {
    /// The read in flight for each path. Only its answer is applied.
    pending: BTreeMap<String, OperationId>,
    /// The submodules of the last status, which the next reads ask for.
    wanted: Vec<String>,
    /// The answer last applied for each path. Kept until a newer one replaces it.
    details: BTreeMap<String, SubmoduleDetail>,
}

impl Submodules {
    /// Takes the status of a repository: every submodule of `entries` is wanted
    /// again, reads in flight are forgotten, and details of paths that are no
    /// longer submodules are dropped.
    pub fn status_changed(&mut self, entries: &[StatusEntry]) {
        let paths: BTreeSet<&str> = entries
            .iter()
            .filter(|entry| entry.submodule.is_some())
            .map(|entry| entry.path.as_str())
            .collect();
        self.details.retain(|path, _| paths.contains(path.as_str()));
        self.wanted = paths.iter().map(|path| (*path).to_owned()).collect();
        self.pending.clear();
    }

    /// The submodules to read since the last call, once: the window asks for each.
    pub fn take_wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
    }

    /// Starts the read of `path`, which `id` answers.
    pub fn request(&mut self, path: String, id: OperationId) {
        self.pending.insert(path, id);
    }

    /// Applies the answer `id` to its path. Returns false when the answer is not
    /// the read in flight for its path.
    pub fn loaded(&mut self, id: OperationId, detail: SubmoduleDetail) -> bool {
        if self.pending.get(&detail.path) != Some(&id) {
            return false;
        }
        self.pending.remove(&detail.path);
        self.details.insert(detail.path.clone(), detail);
        true
    }

    /// The last answer for `path`, if one arrived.
    #[must_use]
    pub fn detail(&self, path: &str) -> Option<&SubmoduleDetail> {
        self.details.get(path)
    }
}

/// The marks a submodule's row shows, in order (D175): the branch, or `detached`,
/// then the commits it gained, the commits it is behind, `modified` and
/// `untracked`. Before the detail arrives the row shows the flags only. The counts
/// are shown only while the status says the commit changed.
#[must_use]
pub fn marks(change: SubmoduleChange, detail: Option<&SubmoduleDetail>) -> Vec<String> {
    let mut marks = Vec::new();
    if let Some(detail) = detail {
        if detail.detached {
            marks.push("detached".to_owned());
        } else if let Some(branch) = &detail.branch {
            marks.push(branch.clone());
        }
    }
    if let Some(detail) = detail.filter(|_| change.commit_changed) {
        if !detail.commits.is_empty() {
            marks.push(new_commits(detail.commits.len(), detail.more));
        }
        if detail.missing_commits > 0 {
            marks.push(format!("{} behind", detail.missing_commits));
        }
    }
    if change.modified {
        marks.push("modified".to_owned());
    }
    if change.untracked {
        marks.push("untracked".to_owned());
    }
    marks
}

/// `N new commits`, or `200+ new commits` when the engine stopped counting there.
fn new_commits(count: usize, more: bool) -> String {
    if more {
        format!("{SUBMODULE_COMMIT_LIMIT}+ new commits")
    } else if count == 1 {
        "1 new commit".to_owned()
    } else {
        format!("{count} new commits")
    }
}

/// One commit line of the commit pane: its subject, its author and how long ago it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneCommit {
    pub subject: String,
    pub author: String,
    pub when: String,
}

/// What the commit pane of a selected submodule says, read from its detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    /// The commits the submodule gained, newest first. Empty when not checked out.
    pub commits: Vec<PaneCommit>,
    /// The `and more commits` line follows the commits.
    pub more: bool,
    /// `N commits behind the recorded commit`, when the submodule is behind.
    pub behind: Option<String>,
    /// The folder is not checked out: the pane says so and lists nothing.
    pub not_checked_out: bool,
}

/// The commit pane's text for `detail`, with times read at `now` (Unix seconds).
#[must_use]
pub fn pane(detail: &SubmoduleDetail, now: i64) -> Pane {
    if !detail.checked_out {
        return Pane {
            commits: Vec::new(),
            more: false,
            behind: None,
            not_checked_out: true,
        };
    }
    Pane {
        commits: detail
            .commits
            .iter()
            .map(|commit| PaneCommit {
                subject: commit.subject.clone(),
                author: commit.author_name.clone(),
                when: relative_time(now, commit.author_time),
            })
            .collect(),
        more: detail.more,
        behind: (detail.missing_commits > 0).then(|| behind_line(detail.missing_commits)),
        not_checked_out: false,
    }
}

fn behind_line(count: usize) -> String {
    if count == 1 {
        "1 commit behind the recorded commit".to_owned()
    } else {
        format!("{count} commits behind the recorded commit")
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::SubmoduleCommit;

    use super::*;

    const NOW: i64 = 1_000_000;

    fn flags(commit_changed: bool, modified: bool, untracked: bool) -> SubmoduleChange {
        SubmoduleChange {
            commit_changed,
            modified,
            untracked,
        }
    }

    fn commit(subject: &str, time: i64) -> SubmoduleCommit {
        SubmoduleCommit {
            oid: "a".repeat(40),
            subject: subject.into(),
            author_name: "Ada".into(),
            author_time: time,
        }
    }

    fn detail(path: &str) -> SubmoduleDetail {
        SubmoduleDetail {
            path: path.into(),
            branch: Some("main".into()),
            detached: false,
            head: Some("b".repeat(40)),
            recorded: Some("c".repeat(40)),
            checked_out: true,
            commits: Vec::new(),
            more: false,
            missing_commits: 0,
            repo: None,
        }
    }

    fn submodule(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged: None,
            unstaged: None,
            untracked: false,
            conflicted: false,
            submodule: Some(SubmoduleChange::default()),
        }
    }

    fn file(path: &str) -> StatusEntry {
        StatusEntry {
            submodule: None,
            ..submodule(path)
        }
    }

    #[test]
    fn before_the_detail_arrives_only_the_flags_show() {
        assert!(marks(flags(false, false, false), None).is_empty());
        assert_eq!(
            marks(flags(true, true, true), None),
            ["modified", "untracked"]
        );
    }

    #[test]
    fn the_branch_or_detached_comes_first() {
        assert_eq!(
            marks(flags(false, false, false), Some(&detail("s"))),
            ["main"]
        );
        let detached = SubmoduleDetail {
            branch: None,
            detached: true,
            ..detail("s")
        };
        assert_eq!(
            marks(flags(false, false, false), Some(&detached)),
            ["detached"]
        );
        let not_checked_out = SubmoduleDetail {
            branch: None,
            detached: false,
            checked_out: false,
            ..detail("s")
        };
        assert!(marks(flags(false, false, false), Some(&not_checked_out)).is_empty());
    }

    #[test]
    fn new_commits_and_behind_are_counted_in_that_order() {
        let ahead = SubmoduleDetail {
            commits: vec![commit("one", NOW), commit("two", NOW)],
            missing_commits: 3,
            repo: None,
            ..detail("s")
        };
        assert_eq!(
            marks(flags(true, false, false), Some(&ahead)),
            ["main", "2 new commits", "3 behind"]
        );
        let one = SubmoduleDetail {
            commits: vec![commit("one", NOW)],
            ..detail("s")
        };
        assert_eq!(
            marks(flags(true, false, false), Some(&one)),
            ["main", "1 new commit"]
        );
    }

    #[test]
    fn more_commits_read_as_the_limit_and_plus() {
        let many = SubmoduleDetail {
            commits: (0..SUBMODULE_COMMIT_LIMIT)
                .map(|_| commit("c", NOW))
                .collect(),
            more: true,
            ..detail("s")
        };
        assert_eq!(
            marks(flags(true, false, false), Some(&many)),
            ["main", "200+ new commits"]
        );
    }

    #[test]
    fn modified_and_untracked_follow_the_counts() {
        let behind = SubmoduleDetail {
            missing_commits: 1,
            repo: None,
            ..detail("s")
        };
        assert_eq!(
            marks(flags(true, true, true), Some(&behind)),
            ["main", "1 behind", "modified", "untracked"]
        );
    }

    #[test]
    fn counts_are_not_shown_while_the_status_says_the_commit_did_not_change() {
        let stale = SubmoduleDetail {
            commits: vec![commit("one", NOW)],
            missing_commits: 2,
            repo: None,
            ..detail("s")
        };
        assert_eq!(
            marks(flags(false, true, false), Some(&stale)),
            ["main", "modified"]
        );
    }

    #[test]
    fn every_combination_of_the_flags_gives_its_marks() {
        for bits in 0..8u8 {
            let change = flags(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            let marks = marks(change, None);
            assert_eq!(marks.contains(&"modified".to_owned()), change.modified);
            assert_eq!(marks.contains(&"untracked".to_owned()), change.untracked);
        }
    }

    #[test]
    fn the_pane_lists_each_commit_with_its_author_and_age() {
        let two_days = SubmoduleDetail {
            commits: vec![commit("fix the parser", NOW - 2 * 86_400)],
            ..detail("s")
        };
        let text = pane(&two_days, NOW);
        assert_eq!(
            text.commits,
            vec![PaneCommit {
                subject: "fix the parser".into(),
                author: "Ada".into(),
                when: "2 days ago".into(),
            }]
        );
        assert!(!text.more);
        assert_eq!(text.behind, None);
        assert!(!text.not_checked_out);
    }

    #[test]
    fn the_pane_adds_the_more_line_and_the_behind_line() {
        let behind = SubmoduleDetail {
            more: true,
            missing_commits: 4,
            repo: None,
            ..detail("s")
        };
        let text = pane(&behind, NOW);
        assert!(text.more);
        assert_eq!(
            text.behind.as_deref(),
            Some("4 commits behind the recorded commit")
        );
        let one = SubmoduleDetail {
            missing_commits: 1,
            repo: None,
            ..detail("s")
        };
        assert_eq!(
            pane(&one, NOW).behind.as_deref(),
            Some("1 commit behind the recorded commit")
        );
    }

    #[test]
    fn a_folder_not_checked_out_says_so_and_lists_nothing() {
        let closed = SubmoduleDetail {
            checked_out: false,
            branch: None,
            ..detail("s")
        };
        let text = pane(&closed, NOW);
        assert!(text.not_checked_out);
        assert!(text.commits.is_empty());
        assert!(!text.more);
    }

    #[test]
    fn a_status_asks_for_each_submodule_and_no_file() {
        let mut state = Submodules::default();
        state.status_changed(&[submodule("libs/a"), file("main.rs"), submodule("libs/b")]);
        assert_eq!(state.take_wanted(), ["libs/a", "libs/b"]);
        assert!(state.take_wanted().is_empty());
    }

    #[test]
    fn an_answer_is_applied_only_to_the_read_in_flight() {
        let mut state = Submodules::default();
        state.status_changed(&[submodule("libs/a")]);
        state.request("libs/a".into(), OperationId(7));
        assert!(!state.loaded(OperationId(6), detail("libs/a")));
        assert!(state.detail("libs/a").is_none());
        assert!(state.loaded(OperationId(7), detail("libs/a")));
        assert_eq!(state.detail("libs/a"), Some(&detail("libs/a")));
    }

    #[test]
    fn a_newer_status_drops_the_answer_still_on_its_way() {
        let mut state = Submodules::default();
        state.status_changed(&[submodule("libs/a")]);
        state.request("libs/a".into(), OperationId(1));
        state.status_changed(&[submodule("libs/a")]);
        state.request("libs/a".into(), OperationId(2));
        assert!(!state.loaded(OperationId(1), detail("libs/a")));
        assert!(state.loaded(OperationId(2), detail("libs/a")));
    }

    #[test]
    fn a_path_that_stops_being_a_submodule_loses_its_detail() {
        let mut state = Submodules::default();
        state.status_changed(&[submodule("libs/a"), submodule("libs/b")]);
        state.request("libs/a".into(), OperationId(1));
        assert!(state.loaded(OperationId(1), detail("libs/a")));
        state.status_changed(&[submodule("libs/b")]);
        assert!(state.detail("libs/a").is_none());
    }
}
