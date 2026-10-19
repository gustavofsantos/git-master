//! The sidebar's branch filter: whose branches it shows, and which names match
//! the typed query. Pure: no GPUI, so the rules are unit-tested on their own.
//!
//! The scope is `Mine` (the branches whose tip commit the user authored) or
//! `All`. Until the user picks one, the default applies: `Mine` when Git knows
//! the user's email, `All` otherwise. A choice is kept even when the email is
//! unknown; it applies again once the email is known.

use gm_schema::BranchEntry;

use crate::fuzzy;

/// Whose branches the sidebar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The current branch, and the branches whose tip the user authored.
    Mine,
    All,
}

impl Scope {
    /// The word the toggle shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mine => "Mine",
            Self::All => "All",
        }
    }

    /// The stable word the settings file keeps.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::All => "all",
        }
    }

    /// The scope a settings word names, if it names one.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "mine" => Some(Self::Mine),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// The email to match authors against: Git's `user.email` without surrounding
/// space. Blank counts as unknown.
#[must_use]
pub fn known_email(user_email: Option<&str>) -> Option<&str> {
    user_email.map(str::trim).filter(|email| !email.is_empty())
}

/// What the sidebar filters by: the scope the user chose, if any, and the query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchFilter {
    /// `None` until the user chooses; then the default scope applies.
    chosen: Option<Scope>,
    query: String,
}

impl BranchFilter {
    /// A filter whose scope was chosen earlier (kept in the settings), if any.
    #[must_use]
    pub fn with_chosen(chosen: Option<Scope>) -> Self {
        Self {
            chosen,
            query: String::new(),
        }
    }

    /// The scope the user chose, if they chose one. This is what the settings keep.
    #[must_use]
    pub fn chosen(&self) -> Option<Scope> {
        self.chosen
    }

    pub fn choose(&mut self, scope: Scope) {
        self.chosen = Some(scope);
    }

    /// Shows every branch with an empty query, and records that choice.
    pub fn show_all(&mut self) {
        self.chosen = Some(Scope::All);
        self.query.clear();
    }

    pub fn set_query(&mut self, text: &str) {
        text.clone_into(&mut self.query);
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The scope in effect for `user_email`. Without a known email, `Mine`
    /// behaves as `All`: there is no author to match.
    #[must_use]
    pub fn scope(&self, user_email: Option<&str>) -> Scope {
        if known_email(user_email).is_none() {
            return Scope::All;
        }
        self.chosen.unwrap_or(Scope::Mine)
    }
}

/// The branches `filter` keeps, in the order of `branches`.
///
/// Under `Mine`, a branch is kept when it is the current one, when its tip was
/// authored by `user_email` (compared without case), or when it is a local
/// branch whose author cannot be read: a branch that cannot be judged stays
/// visible. A remote branch whose author cannot be read is not kept.
///
/// The query then keeps the branches whose full name matches it, as the
/// palette's fuzzy match does; an empty query matches all.
#[must_use]
pub fn visible<'a>(
    branches: &'a [BranchEntry],
    filter: &BranchFilter,
    user_email: Option<&str>,
) -> Vec<&'a BranchEntry> {
    let scope = filter.scope(user_email);
    let email = known_email(user_email).map(str::to_lowercase);
    branches
        .iter()
        .filter(|branch| match scope {
            Scope::All => true,
            Scope::Mine => is_mine(branch, email.as_deref()),
        })
        .filter(|branch| fuzzy::fuzzy_match(filter.query(), &branch.name).is_some())
        .collect()
}

/// Whether `branch` belongs to the `Mine` scope. `email` is lower case, if known.
fn is_mine(branch: &BranchEntry, email: Option<&str>) -> bool {
    if branch.is_current {
        return true;
    }
    match (branch.author_email.as_deref(), email) {
        (Some(author), Some(mine)) => author.trim().to_lowercase() == mine,
        (None, _) => !branch.is_remote,
        (Some(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "me@example.com";

    fn branch(name: &str, remote: bool, current: bool, author: Option<&str>) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: remote,
            is_current: current,
            upstream: None,
            author_email: author.map(str::to_owned),
        }
    }

    fn names<'a>(shown: &[&'a BranchEntry]) -> Vec<&'a str> {
        shown.iter().map(|branch| branch.name.as_str()).collect()
    }

    /// A small repository: mine, someone else's, unknown, and the current branch by someone else.
    fn repo() -> Vec<BranchEntry> {
        vec![
            branch("main", false, true, Some("ana@example.com")),
            branch("feature/mine", false, false, Some(ME)),
            branch("feature/theirs", false, false, Some("bruno@example.com")),
            branch("local/unknown", false, false, None),
            branch("origin/mine-too", true, false, Some(ME)),
            branch("origin/theirs", true, false, Some("bruno@example.com")),
            branch("origin/unknown", true, false, None),
        ]
    }

    #[test]
    fn mine_keeps_the_user_branches_the_current_one_and_unknown_local_ones() {
        let all = repo();
        let shown = visible(&all, &BranchFilter::default(), Some(ME));
        assert_eq!(
            names(&shown),
            ["main", "feature/mine", "local/unknown", "origin/mine-too"]
        );
    }

    #[test]
    fn all_keeps_every_branch_in_listed_order() {
        let all = repo();
        let filter = BranchFilter::with_chosen(Some(Scope::All));
        assert_eq!(visible(&all, &filter, Some(ME)).len(), all.len());
    }

    #[test]
    fn the_default_scope_is_mine_only_when_the_email_is_known() {
        let filter = BranchFilter::default();
        assert_eq!(filter.scope(Some(ME)), Scope::Mine);
        assert_eq!(filter.scope(None), Scope::All);
        assert_eq!(filter.scope(Some("   ")), Scope::All);
    }

    #[test]
    fn a_chosen_scope_overrides_the_default() {
        let mut filter = BranchFilter::default();
        filter.choose(Scope::All);
        assert_eq!(filter.scope(Some(ME)), Scope::All);
        filter.choose(Scope::Mine);
        assert_eq!(filter.scope(Some(ME)), Scope::Mine);
    }

    #[test]
    fn mine_without_a_known_email_behaves_as_all_and_the_choice_survives() {
        let all = repo();
        let filter = BranchFilter::with_chosen(Some(Scope::Mine));
        assert_eq!(visible(&all, &filter, None).len(), all.len());
        assert_eq!(filter.chosen(), Some(Scope::Mine));
        assert_eq!(visible(&all, &filter, Some(ME)).len(), 4);
    }

    #[test]
    fn the_email_matches_without_case_or_surrounding_space() {
        let all = [branch("topic", false, false, Some("  Me@Example.COM "))];
        let shown = visible(&all, &BranchFilter::default(), Some("ME@example.com"));
        assert_eq!(names(&shown), ["topic"]);
    }

    #[test]
    fn a_blank_user_email_is_unknown() {
        let all = [branch("topic", false, false, Some(""))];
        let filter = BranchFilter::default();
        assert_eq!(visible(&all, &filter, Some("  ")).len(), 1);
        assert_eq!(known_email(Some(" \t ")), None);
        assert_eq!(known_email(Some(" a@b ")), Some("a@b"));
    }

    #[test]
    fn the_current_branch_is_kept_under_mine_even_when_someone_else_wrote_it() {
        let all = [branch("main", false, true, Some("ana@example.com"))];
        let shown = visible(&all, &BranchFilter::default(), Some(ME));
        assert_eq!(names(&shown), ["main"]);
    }

    #[test]
    fn a_remote_branch_without_an_author_is_not_kept_under_mine() {
        let all = [branch("origin/gone", true, false, None)];
        assert!(visible(&all, &BranchFilter::default(), Some(ME)).is_empty());
        let filter = BranchFilter::with_chosen(Some(Scope::All));
        assert_eq!(visible(&all, &filter, Some(ME)).len(), 1);
    }

    #[test]
    fn the_query_matches_the_full_name_without_case() {
        let all = repo();
        let mut filter = BranchFilter::with_chosen(Some(Scope::All));
        filter.set_query("THEIRS");
        assert_eq!(
            names(&visible(&all, &filter, Some(ME))),
            ["feature/theirs", "origin/theirs"]
        );
        filter.set_query("feat/mine");
        assert_eq!(names(&visible(&all, &filter, Some(ME))), ["feature/mine"]);
    }

    #[test]
    fn the_query_combines_with_the_scope() {
        let all = repo();
        let mut filter = BranchFilter::default();
        filter.set_query("mine");
        assert_eq!(
            names(&visible(&all, &filter, Some(ME))),
            ["feature/mine", "origin/mine-too"]
        );
        filter.show_all();
        filter.set_query("theirs");
        assert_eq!(
            names(&visible(&all, &filter, Some(ME))),
            ["feature/theirs", "origin/theirs"]
        );
        filter.choose(Scope::Mine);
        assert!(
            visible(&all, &filter, Some(ME))
                .iter()
                .all(|b| b.name != "origin/theirs")
        );
        assert!(visible(&all, &filter, Some(ME)).is_empty());
    }

    #[test]
    fn an_empty_or_blank_query_matches_everything_in_scope() {
        let all = repo();
        let mut filter = BranchFilter::default();
        filter.set_query("   ");
        assert_eq!(visible(&all, &filter, Some(ME)).len(), 4);
    }

    #[test]
    fn a_query_nothing_matches_gives_an_empty_result() {
        let all = repo();
        let mut filter = BranchFilter::with_chosen(Some(Scope::All));
        filter.set_query("zzz-none");
        assert!(visible(&all, &filter, Some(ME)).is_empty());
        assert!(visible(&[], &BranchFilter::default(), Some(ME)).is_empty());
    }

    #[test]
    fn show_all_clears_the_query_and_records_the_scope() {
        let mut filter = BranchFilter::default();
        filter.set_query("abc");
        filter.show_all();
        assert_eq!(filter.query(), "");
        assert_eq!(filter.chosen(), Some(Scope::All));
    }

    #[test]
    fn scope_words_round_trip_through_their_ids() {
        for scope in [Scope::Mine, Scope::All] {
            assert_eq!(Scope::from_id(scope.id()), Some(scope));
        }
        assert_eq!(Scope::from_id("everyone"), None);
    }
}
