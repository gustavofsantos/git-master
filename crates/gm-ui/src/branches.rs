//! The branch list behind the top bar's branch picker: what the engine last
//! listed, the filter typed into the picker, the highlighted row and the
//! checkout in flight. It has no GPUI in it; the picker renders what this
//! reports, and `Model` routes the engine's events here.

use gm_schema::{BranchEntry, OperationId};

/// The two groups the picker shows, local first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Local,
    Remote,
}

impl Section {
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Remote => "Remote",
        }
    }

    fn of(branch: &BranchEntry) -> Self {
        if branch.is_remote {
            Self::Remote
        } else {
            Self::Local
        }
    }
}

/// What the list was last asked for and what came back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Listing {
    /// Never asked for.
    #[default]
    Idle,
    /// The `ListBranches` the picker waits for.
    Waiting(OperationId),
    Listed(Vec<BranchEntry>),
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branches {
    listing: Listing,
    /// The last listing that arrived, kept while a newer one is awaited, so the
    /// sidebar does not blink when the picker asks again.
    known: Vec<BranchEntry>,
    filter: String,
    /// Index among `matches()` of the row Enter would choose.
    highlight: usize,
    /// The `Checkout` the window waits for.
    checkout: Option<OperationId>,
}

/// One row of the picker. Headers and "None" rows are not selectable.
#[derive(Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Header(Section),
    /// A section with no matching branch.
    Empty(Section),
    Branch {
        branch: &'a BranchEntry,
        highlighted: bool,
    },
}

impl Branches {
    /// Asks for a fresh listing. Replies to any earlier request are stale now.
    pub fn request(&mut self, id: OperationId) {
        self.listing = Listing::Waiting(id);
        self.highlight = 0;
    }

    /// Takes the reply to the request `id`. A reply to any other request is
    /// stale and is dropped. The highlight starts on the current branch.
    pub fn listed(&mut self, id: OperationId, branches: Vec<BranchEntry>) -> bool {
        if self.listing != Listing::Waiting(id) {
            return false;
        }
        self.known.clone_from(&branches);
        self.listing = Listing::Listed(branches);
        self.highlight = self
            .matches()
            .iter()
            .position(|branch| branch.is_current)
            .unwrap_or(0);
        true
    }

    /// Every branch of the last listing, unfiltered, in the order listed.
    #[must_use]
    pub fn known(&self) -> &[BranchEntry] {
        &self.known
    }

    /// Forgets the listing: it belonged to a repository that was left.
    pub fn clear(&mut self) {
        self.known.clear();
        self.listing = Listing::Idle;
    }

    /// Records that the request `id` failed. Returns whether it was the one waited for.
    pub fn list_failed(&mut self, id: OperationId) -> bool {
        if self.listing != Listing::Waiting(id) {
            return false;
        }
        self.listing = Listing::Failed;
        true
    }

    /// Whether the last request for a listing failed.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.listing == Listing::Failed
    }

    /// Sets the filter to the first line of `text`. Moving the filter moves the
    /// highlight back to the first match.
    pub fn set_filter(&mut self, text: &str) {
        let needle = text.lines().next().unwrap_or("");
        if needle != self.filter {
            needle.clone_into(&mut self.filter);
            self.highlight = 0;
        }
    }

    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The branches the filter keeps: a case-insensitive substring of the name.
    /// Local branches come first, then remote ones, each in the order listed.
    #[must_use]
    pub fn matches(&self) -> Vec<&BranchEntry> {
        let Listing::Listed(branches) = &self.listing else {
            return Vec::new();
        };
        let needle = self.filter.to_lowercase();
        let mut shown: Vec<&BranchEntry> = branches
            .iter()
            .filter(|branch| branch.name.to_lowercase().contains(&needle))
            .collect();
        shown.sort_by_key(|branch| Section::of(branch));
        shown
    }

    /// The rows of the picker. Empty until a listing has arrived. Both sections
    /// always have a header, and an empty section says so.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'_>> {
        if !matches!(self.listing, Listing::Listed(_)) {
            return Vec::new();
        }
        let matches = self.matches();
        let mut lines = Vec::new();
        for section in [Section::Local, Section::Remote] {
            lines.push(Line::Header(section));
            let mut any = false;
            for (ordinal, branch) in matches.iter().enumerate() {
                if Section::of(branch) == section {
                    any = true;
                    lines.push(Line::Branch {
                        branch,
                        highlighted: ordinal == self.highlight,
                    });
                }
            }
            if !any {
                lines.push(Line::Empty(section));
            }
        }
        lines
    }

    /// Moves the highlight one match down or up. It stops at either end.
    pub fn move_highlight(&mut self, down: bool) {
        let count = self.matches().len();
        if count == 0 {
            self.highlight = 0;
            return;
        }
        let current = self.highlight.min(count - 1);
        self.highlight = if down {
            (current + 1).min(count - 1)
        } else {
            current.saturating_sub(1)
        };
    }

    /// The branch Enter would choose.
    #[must_use]
    pub fn highlighted(&self) -> Option<&BranchEntry> {
        self.matches().get(self.highlight).copied()
    }

    pub fn begin_checkout(&mut self, id: OperationId) {
        self.checkout = Some(id);
    }

    #[must_use]
    pub fn checkout_in_flight(&self) -> bool {
        self.checkout.is_some()
    }

    /// Ends the checkout `id` if it is the one in flight. Returns whether it was.
    pub fn finish_checkout(&mut self, id: OperationId) -> bool {
        if self.checkout == Some(id) {
            self.checkout = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, current: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: false,
            is_current: current,
            upstream: None,
        }
    }

    fn remote(name: &str) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: true,
            is_current: false,
            upstream: None,
        }
    }

    /// A picker holding `branches` for request 1.
    fn listed(branches: Vec<BranchEntry>) -> Branches {
        let mut state = Branches::default();
        state.request(OperationId(1));
        assert!(state.listed(OperationId(1), branches));
        state
    }

    fn names(state: &Branches) -> Vec<&str> {
        state.matches().iter().map(|b| b.name.as_str()).collect()
    }

    /// The picker rows as short text: `#Local`, `-` for an empty section,
    /// `*name` for the highlighted row, `name` for the rest.
    fn rows(state: &Branches) -> Vec<String> {
        state
            .lines()
            .iter()
            .map(|line| match line {
                Line::Header(section) => format!("#{}", section.title()),
                Line::Empty(_) => "-".to_owned(),
                Line::Branch {
                    branch,
                    highlighted: true,
                } => format!("*{}", branch.name),
                Line::Branch { branch, .. } => branch.name.clone(),
            })
            .collect()
    }

    #[test]
    fn nothing_is_shown_before_a_listing_arrives() {
        let mut state = Branches::default();
        assert!(state.lines().is_empty());
        state.request(OperationId(4));
        assert!(state.lines().is_empty());
        assert!(state.highlighted().is_none());
    }

    #[test]
    fn local_branches_come_before_remote_ones_under_their_headers() {
        let state = listed(vec![
            remote("origin/main"),
            local("main", true),
            local("feature/x", false),
            remote("origin/feature/x"),
        ]);
        assert_eq!(
            rows(&state),
            [
                "#Local",
                "*main",
                "feature/x",
                "#Remote",
                "origin/main",
                "origin/feature/x",
            ]
        );
    }

    #[test]
    fn a_section_without_branches_keeps_its_header_and_says_none() {
        let state = listed(vec![local("main", true)]);
        assert_eq!(rows(&state), ["#Local", "*main", "#Remote", "-"]);
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_of_the_name() {
        let mut state = listed(vec![
            local("main", true),
            local("Feature/X", false),
            remote("origin/feature/y"),
        ]);
        state.set_filter("FEAT");
        assert_eq!(names(&state), ["Feature/X", "origin/feature/y"]);
        state.set_filter("ain");
        assert_eq!(names(&state), ["main"]);
        state.set_filter("nope");
        assert!(names(&state).is_empty());
        assert_eq!(rows(&state), ["#Local", "-", "#Remote", "-"]);
    }

    #[test]
    fn only_the_first_line_of_the_filter_counts() {
        let mut state = listed(vec![local("main", true)]);
        state.set_filter("main\nignored");
        assert_eq!(state.filter(), "main");
    }

    #[test]
    fn the_highlight_starts_on_the_current_branch() {
        let state = listed(vec![local("a", false), local("b", true), local("c", false)]);
        assert_eq!(state.highlighted().map(|b| b.name.as_str()), Some("b"));
    }

    #[test]
    fn a_detached_head_highlights_the_first_row() {
        let state = listed(vec![local("a", false), local("b", false)]);
        assert_eq!(state.highlighted().map(|b| b.name.as_str()), Some("a"));
    }

    #[test]
    fn the_highlight_moves_over_branches_and_stops_at_both_ends() {
        let mut state = listed(vec![local("a", true), remote("origin/b")]);
        state.move_highlight(false);
        assert_eq!(state.highlighted().map(|b| b.name.as_str()), Some("a"));
        state.move_highlight(true);
        assert_eq!(
            state.highlighted().map(|b| b.name.as_str()),
            Some("origin/b")
        );
        state.move_highlight(true);
        assert_eq!(
            state.highlighted().map(|b| b.name.as_str()),
            Some("origin/b")
        );
        assert_eq!(rows(&state), ["#Local", "a", "#Remote", "*origin/b"]);
    }

    #[test]
    fn changing_the_filter_moves_the_highlight_to_the_first_match() {
        let mut state = listed(vec![local("a", false), local("b", true)]);
        state.set_filter("b");
        assert_eq!(state.highlighted().map(|b| b.name.as_str()), Some("b"));
        state.set_filter("");
        assert_eq!(state.highlighted().map(|b| b.name.as_str()), Some("a"));
    }

    #[test]
    fn a_reply_to_an_older_request_is_dropped() {
        let mut state = Branches::default();
        state.request(OperationId(1));
        state.request(OperationId(2));
        assert!(!state.listed(OperationId(1), vec![local("old", true)]));
        assert!(state.lines().is_empty());
        assert!(state.listed(OperationId(2), vec![local("new", true)]));
        assert_eq!(names(&state), ["new"]);
    }

    #[test]
    fn a_reply_after_the_picker_was_reset_is_dropped() {
        let mut state = Branches::default();
        assert!(!state.listed(OperationId(9), vec![local("main", true)]));
        assert!(state.lines().is_empty());
    }

    #[test]
    fn a_failed_listing_is_recorded_only_for_the_request_waited_for() {
        let mut state = Branches::default();
        state.request(OperationId(5));
        assert!(!state.list_failed(OperationId(4)));
        assert!(!state.is_failed());
        assert!(state.list_failed(OperationId(5)));
        assert!(state.is_failed());
        assert!(state.lines().is_empty());
    }

    #[test]
    fn only_the_checkout_in_flight_finishes_it() {
        let mut state = Branches::default();
        state.begin_checkout(OperationId(3));
        assert!(state.checkout_in_flight());
        assert!(!state.finish_checkout(OperationId(2)));
        assert!(state.checkout_in_flight());
        assert!(state.finish_checkout(OperationId(3)));
        assert!(!state.checkout_in_flight());
    }
}
