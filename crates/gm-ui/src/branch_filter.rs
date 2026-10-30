//! The sidebar's branch filter: which names match the typed query. Pure: no
//! GPUI, so the rule is unit-tested on its own. Every branch of the repository
//! is listed; the query only narrows the list.

use gm_schema::BranchEntry;

use crate::fuzzy;

/// What the sidebar filters by: the typed query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchFilter {
    query: String,
}

impl BranchFilter {
    pub fn set_query(&mut self, text: &str) {
        text.clone_into(&mut self.query);
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// The branches whose full name matches the query, as the palette's fuzzy
/// match does, in the order of `branches`. An empty query keeps all.
#[must_use]
pub fn visible<'a>(branches: &'a [BranchEntry], filter: &BranchFilter) -> Vec<&'a BranchEntry> {
    branches
        .iter()
        .filter(|branch| fuzzy::fuzzy_match(filter.query(), &branch.name).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, remote: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: remote,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    fn names<'a>(shown: &[&'a BranchEntry]) -> Vec<&'a str> {
        shown.iter().map(|branch| branch.name.as_str()).collect()
    }

    fn repo() -> Vec<BranchEntry> {
        vec![
            branch("main", false),
            branch("feature/mine", false),
            branch("feature/theirs", false),
            branch("origin/theirs", true),
        ]
    }

    #[test]
    fn an_empty_or_blank_query_keeps_every_branch_in_listed_order() {
        let all = repo();
        let mut filter = BranchFilter::default();
        assert_eq!(visible(&all, &filter).len(), all.len());
        filter.set_query("   ");
        assert_eq!(visible(&all, &filter).len(), all.len());
    }

    #[test]
    fn the_query_matches_the_full_name_without_case() {
        let all = repo();
        let mut filter = BranchFilter::default();
        filter.set_query("THEIRS");
        assert_eq!(
            names(&visible(&all, &filter)),
            ["feature/theirs", "origin/theirs"]
        );
        filter.set_query("feat/mine");
        assert_eq!(names(&visible(&all, &filter)), ["feature/mine"]);
    }

    #[test]
    fn a_query_nothing_matches_gives_an_empty_result() {
        let all = repo();
        let mut filter = BranchFilter::default();
        filter.set_query("zzz-none");
        assert!(visible(&all, &filter).is_empty());
        assert!(visible(&[], &BranchFilter::default()).is_empty());
    }
}
