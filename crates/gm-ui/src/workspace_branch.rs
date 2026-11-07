//! The branch list of a New Workspace row (D195): the choices a filter leaves in a member's
//! repository, and the branch listings the dialog holds for each repository it has asked. Pure: no
//! GPUI, so the rules are unit-tested here.

use std::collections::{BTreeMap, BTreeSet};

use gm_schema::{BranchEntry, OperationId};

use crate::fuzzy;

/// The most branches the list shows, as the worktree dialog does: the dialog does not scroll, and the
/// filter narrows the rest. A new name is always offered.
pub(crate) const SHOWN: usize = 8;

/// What the branch list of a repository is, as the dialog shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Known<'a> {
    /// Asked for and not answered yet, or never asked.
    Loading,
    /// The branches the repository has, as the engine listed them.
    Listed(&'a [BranchEntry]),
    /// The last request failed: only a new name can be offered.
    Failed,
}

/// One repository's listing: the request it waits for, the branches it got or that it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Listing {
    Waiting(OperationId),
    Listed(Vec<BranchEntry>),
    Failed,
}

/// The listings of the repositories the dialog has asked, by path. An answer is taken only for
/// the request its repository waits for, so an answer to an earlier request is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepoBranches {
    listings: BTreeMap<String, Listing>,
}

impl RepoBranches {
    /// Whether a request for `repo` is wanted: none is in flight and none has succeeded. A failed
    /// listing is asked again when the repository is chosen again.
    #[must_use]
    pub(crate) fn wants(&self, repo: &str) -> bool {
        !matches!(
            self.listings.get(repo.trim()),
            Some(Listing::Waiting(_) | Listing::Listed(_))
        )
    }

    /// Records that `repo` waits for the request `id`. Any earlier request for it is stale now.
    pub(crate) fn request(&mut self, repo: &str, id: OperationId) {
        self.listings
            .insert(repo.trim().to_owned(), Listing::Waiting(id));
    }

    /// Takes the answer `branches` to the request `id` of `repo`. Returns whether it was the one
    /// waited for.
    pub(crate) fn listed(
        &mut self,
        id: OperationId,
        repo: &str,
        branches: Vec<BranchEntry>,
    ) -> bool {
        let key = repo.trim();
        if self.listings.get(key) != Some(&Listing::Waiting(id)) {
            return false;
        }
        self.listings
            .insert(key.to_owned(), Listing::Listed(branches));
        true
    }

    /// Records that the request `id` failed. Returns whether a repository waited for it.
    pub(crate) fn failed(&mut self, id: OperationId) -> bool {
        let Some(key) = self.waiting_on(id) else {
            return false;
        };
        self.listings.insert(key, Listing::Failed);
        true
    }

    /// Whether a repository waits for the request `id`.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.waiting_on(id).is_some()
    }

    fn waiting_on(&self, id: OperationId) -> Option<String> {
        self.listings
            .iter()
            .find(|(_, listing)| **listing == Listing::Waiting(id))
            .map(|(key, _)| key.clone())
    }

    /// The branch list of `repo` as the dialog shows it.
    #[must_use]
    pub(crate) fn known(&self, repo: &str) -> Known<'_> {
        match self.listings.get(repo.trim()) {
            None | Some(Listing::Waiting(_)) => Known::Loading,
            Some(Listing::Listed(branches)) => Known::Listed(branches.as_slice()),
            Some(Listing::Failed) => Known::Failed,
        }
    }
}

/// One line the branch list of a row offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// A branch of the repository. A remote-only one is named by its short name, the name the
    /// planner resolves to its remote branch.
    Branch { name: String, remote_only: bool },
    /// A new branch, named by the filter text.
    Create(String),
}

impl Choice {
    /// The name the row's branch takes when this choice is picked.
    #[must_use]
    pub(crate) fn fills(&self) -> &str {
        match self {
            Self::Branch { name, .. } | Self::Create(name) => name,
        }
    }
}

/// The name a remote branch has without its remote: `origin/feat/x` is `feat/x`.
#[must_use]
pub(crate) fn short_name(remote_branch: &str) -> &str {
    remote_branch
        .split_once('/')
        .map_or(remote_branch, |(_, name)| name)
}

/// Whether `branch` is named `name`: a local branch by its name, a remote one by its short name.
fn named(branch: &BranchEntry, name: &str) -> bool {
    if branch.is_remote {
        short_name(&branch.name) == name
    } else {
        branch.name == name
    }
}

/// The choices a filter leaves, in the order the list shows them. A local branch matching the
/// filter comes first, then a remote-only one, each group by how well it matches. A remote branch
/// that has a local branch of its name is left out, and so is a second remote of the same name.
/// At most `SHOWN` branches; then, when the filter is not empty and names no branch exactly, a
/// new branch with the filter's name. While the listing is loading there is nothing to choose.
#[must_use]
pub(crate) fn choices(filter: &str, known: Known<'_>) -> Vec<Choice> {
    let needle = filter.trim();
    let (branches, offer_new): (&[BranchEntry], bool) = match known {
        Known::Loading => return Vec::new(),
        Known::Listed(branches) => (branches, true),
        Known::Failed => (&[], true),
    };
    let locals: BTreeSet<&str> = branches
        .iter()
        .filter(|branch| !branch.is_remote)
        .map(|branch| branch.name.as_str())
        .collect();
    let mut seen_remote = BTreeSet::new();
    let mut local_rows = Vec::new();
    let mut remote_rows = Vec::new();
    for branch in branches {
        if branch.is_remote {
            let short = short_name(&branch.name);
            if locals.contains(short) || !seen_remote.insert(short) {
                continue;
            }
            if let Some(found) = fuzzy::fuzzy_match(needle, short) {
                remote_rows.push((
                    found.score,
                    Choice::Branch {
                        name: short.to_owned(),
                        remote_only: true,
                    },
                ));
            }
        } else if let Some(found) = fuzzy::fuzzy_match(needle, &branch.name) {
            local_rows.push((
                found.score,
                Choice::Branch {
                    name: branch.name.clone(),
                    remote_only: false,
                },
            ));
        }
    }
    local_rows.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    remote_rows.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut list: Vec<Choice> = local_rows
        .into_iter()
        .chain(remote_rows)
        .map(|(_, choice)| choice)
        .take(SHOWN)
        .collect();
    if offer_new && !needle.is_empty() && !branches.iter().any(|branch| named(branch, needle)) {
        list.push(Choice::Create(needle.to_owned()));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: false,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    fn remote(name: &str) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: true,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    fn branch(name: &str, remote_only: bool) -> Choice {
        Choice::Branch {
            name: name.to_owned(),
            remote_only,
        }
    }

    fn sample() -> Vec<BranchEntry> {
        vec![
            local("feat"),
            local("main"),
            remote("origin/main"),
            remote("origin/pay"),
        ]
    }

    #[test]
    fn a_remote_branch_is_named_by_what_follows_its_remote() {
        assert_eq!(short_name("origin/feat/x"), "feat/x");
        assert_eq!(short_name("feat"), "feat");
    }

    #[test]
    fn local_branches_come_first_then_remote_only_ones_marked() {
        let list = choices("", Known::Listed(&sample()));
        assert_eq!(
            list,
            [
                branch("feat", false),
                branch("main", false),
                branch("pay", true)
            ],
            "origin/main has a local main, so only the local one is listed"
        );
    }

    #[test]
    fn the_filter_is_fuzzy_and_keeps_the_local_first_order() {
        let list = choices("p", Known::Listed(&sample()));
        assert_eq!(list, [branch("pay", true), Choice::Create("p".to_owned())]);
        let list = choices("ma", Known::Listed(&sample()));
        assert_eq!(
            list,
            [branch("main", false), Choice::Create("ma".to_owned())]
        );
    }

    #[test]
    fn a_name_no_branch_has_is_offered_as_a_new_branch() {
        let list = choices("pay-v2", Known::Listed(&sample()));
        assert_eq!(list, [Choice::Create("pay-v2".to_owned())]);
        assert_eq!(list[0].fills(), "pay-v2");
    }

    #[test]
    fn a_new_name_comes_after_the_matches_and_an_exact_name_is_not_offered_again() {
        let list = choices("fe", Known::Listed(&sample()));
        assert_eq!(
            list,
            [branch("feat", false), Choice::Create("fe".to_owned())]
        );
        assert!(
            choices("feat", Known::Listed(&sample()))
                .iter()
                .all(|choice| !matches!(choice, Choice::Create(_))),
            "feat exists"
        );
        assert!(
            choices("pay", Known::Listed(&sample()))
                .iter()
                .all(|choice| !matches!(choice, Choice::Create(_))),
            "a remote-only pay exists"
        );
    }

    #[test]
    fn an_empty_filter_offers_no_new_branch() {
        assert!(choices("  ", Known::Listed(&[])).is_empty());
    }

    #[test]
    fn a_remote_branch_with_the_same_name_in_two_remotes_is_listed_once() {
        let branches = [remote("origin/pay"), remote("upstream/pay")];
        assert_eq!(choices("", Known::Listed(&branches)), [branch("pay", true)]);
    }

    #[test]
    fn a_loading_list_offers_nothing_and_a_failed_one_offers_only_a_new_name() {
        assert!(choices("feat", Known::Loading).is_empty());
        assert_eq!(
            choices("feat", Known::Failed),
            [Choice::Create("feat".to_owned())]
        );
        assert!(choices("", Known::Failed).is_empty());
    }

    #[test]
    fn the_list_shows_at_most_shown_branches_and_the_new_name_still_comes() {
        let branches: Vec<BranchEntry> = (0..SHOWN + 20)
            .map(|at| local(&format!("b{at:03}")))
            .collect();
        let list = choices("b", Known::Listed(&branches));
        assert_eq!(list.len(), SHOWN + 1);
        assert_eq!(list.last(), Some(&Choice::Create("b".to_owned())));
    }

    #[test]
    fn the_cache_takes_only_the_answer_to_the_request_it_waits_for() {
        let mut cache = RepoBranches::default();
        assert!(cache.wants("/a"));
        assert_eq!(cache.known("/a"), Known::Loading);
        cache.request("/a ", OperationId(3));
        assert!(!cache.wants("/a"), "a request is in flight");
        assert!(
            !cache.listed(OperationId(2), "/a", vec![local("x")]),
            "stale"
        );
        assert_eq!(cache.known("/a"), Known::Loading);
        assert!(cache.owns(OperationId(3)));
        assert!(cache.listed(OperationId(3), "/a", vec![local("x")]));
        assert_eq!(cache.known("/a"), Known::Listed(&[local("x")]));
        assert!(!cache.wants("/a"), "a listing is kept");
    }

    #[test]
    fn a_failed_listing_is_asked_again() {
        let mut cache = RepoBranches::default();
        cache.request("/a", OperationId(1));
        assert!(cache.failed(OperationId(1)));
        assert!(!cache.failed(OperationId(1)), "only once");
        assert_eq!(cache.known("/a"), Known::Failed);
        assert!(cache.wants("/a"));
    }
}
