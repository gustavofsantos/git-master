//! The stash list of the Stashes page (B2, D94), with no GPUI in it: the
//! listing the engine answered, the read in flight, its failure and the stash
//! the user selected. Ids follow dispatch order, so an answer older than the one
//! shown, or than the read in flight, is superseded and dropped.

use gm_schema::{OperationId, OperationState, StashEntry};

/// Git's message when a failed read gave none.
const NO_MESSAGE: &str = "The stashes could not be read.";

/// The name of a stash as Git writes it: `stash@{n}`.
#[must_use]
pub fn label(index: u32) -> String {
    format!("stash@{{{index}}}")
}

/// The stashes of the open repository and the read that will replace them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StashList {
    /// The newest read, while it is in flight.
    pending: Option<OperationId>,
    /// The answer on screen. An answer older than it is superseded.
    applied: Option<OperationId>,
    entries: Vec<StashEntry>,
    /// Git's message when the newest read failed. The next read clears it.
    failure: Option<String>,
    /// The stash the user selected, by its commit id. Kept across re-lists
    /// while the stash is listed.
    selected: Option<String>,
}

impl StashList {
    /// Starts the read `id`. The list on screen stays until its answer comes.
    pub fn request(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.failure = None;
    }

    /// Whether `id` is the read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Applies a listing answered by `id`. Returns false when the answer is
    /// superseded: an older one than the list on screen, or than the read in
    /// flight. A stash write answers with a listing too, and a newer one is shown
    /// and ends the read in flight, since it is at least as fresh.
    pub fn listed(&mut self, id: OperationId, entries: Vec<StashEntry>) -> bool {
        if self.applied.is_some_and(|shown| id < shown)
            || self.pending.is_some_and(|wanted| id < wanted)
        {
            return false;
        }
        if self.pending.is_some_and(|wanted| wanted <= id) {
            self.pending = None;
        }
        self.applied = Some(id);
        self.failure = None;
        if let Some(oid) = &self.selected
            && !entries.iter().any(|entry| &entry.oid == oid)
        {
            self.selected = None;
        }
        self.entries = entries;
        true
    }

    /// The read `id` failed with `message`. A failure of another read is ignored.
    pub fn failed(&mut self, id: OperationId, message: String) {
        if self.owns(id) {
            self.pending = None;
            self.failure = Some(message);
        }
    }

    /// The read `id` ended without an answer. A stopped read leaves the list as it was.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        if !self.owns(id) {
            return;
        }
        if state == OperationState::Failed {
            self.failed(id, detail.unwrap_or_else(|| NO_MESSAGE.to_owned()));
        } else if state.is_terminal() {
            self.pending = None;
        }
    }

    /// Selects the stash with commit id `oid`. False when no listed stash has it.
    pub fn select(&mut self, oid: &str) -> bool {
        if self.entries.iter().any(|entry| entry.oid == oid) {
            self.selected = Some(oid.to_owned());
            true
        } else {
            false
        }
    }

    /// The selected stash's commit id.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected stash as listed, with its current index.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&StashEntry> {
        let oid = self.selected.as_deref()?;
        self.entries.iter().find(|entry| entry.oid == oid)
    }

    /// The stashes as the answer on screen had them, newest first.
    #[must_use]
    pub fn entries(&self) -> &[StashEntry] {
        &self.entries
    }

    /// How many stashes are listed, once a listing has been shown.
    #[must_use]
    pub fn count(&self) -> Option<usize> {
        self.applied.map(|_| self.entries.len())
    }

    /// Whether a listing has been shown.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.applied.is_some()
    }

    /// Git's message when the newest read failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

/// The header's count: "1 stash", "3 stashes", or nothing before a listing.
#[must_use]
pub fn count_label(count: usize) -> String {
    if count == 1 {
        "1 stash".to_owned()
    } else {
        format!("{count} stashes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stash(index: u32, oid: &str) -> StashEntry {
        StashEntry {
            index,
            oid: oid.repeat(40),
            message: format!("On main: stash {index}"),
            branch: Some("main".into()),
            time: 1_000,
        }
    }

    fn ids(list: &StashList) -> Vec<String> {
        list.entries()
            .iter()
            .map(|entry| entry.oid.clone())
            .collect()
    }

    #[test]
    fn a_listing_is_shown_once_its_read_answers() {
        let mut list = StashList::default();
        assert!(!list.is_loaded());
        assert_eq!(list.count(), None);
        list.request(OperationId(3));
        assert!(!list.is_loaded());
        assert!(list.owns(OperationId(3)));
        assert!(list.listed(OperationId(3), vec![stash(0, "a")]));
        assert!(!list.owns(OperationId(3)));
        assert_eq!(list.count(), Some(1));
        assert_eq!(list.entries()[0].index, 0);
    }

    #[test]
    fn a_read_superseded_by_a_newer_one_is_dropped() {
        let mut list = StashList::default();
        list.request(OperationId(3));
        list.request(OperationId(5));
        assert!(!list.listed(OperationId(3), vec![stash(0, "a")]));
        assert!(!list.is_loaded());
        assert!(list.listed(OperationId(5), vec![stash(0, "b")]));
        assert_eq!(ids(&list), [stash(0, "b").oid]);
    }

    #[test]
    fn an_answer_older_than_the_one_shown_is_dropped() {
        let mut list = StashList::default();
        assert!(list.listed(OperationId(7), vec![stash(0, "a")]));
        assert!(!list.listed(OperationId(4), vec![]));
        assert_eq!(list.count(), Some(1));
    }

    #[test]
    fn a_newer_write_listing_is_shown_and_ends_the_read_in_flight() {
        let mut list = StashList::default();
        list.request(OperationId(4));
        assert!(list.listed(OperationId(6), vec![stash(0, "c"), stash(1, "d")]));
        assert!(!list.owns(OperationId(4)));
        assert!(list.is_loaded());
        // The read that was in flight answers late, and is older than what is shown.
        assert!(!list.listed(OperationId(4), vec![stash(0, "a")]));
        assert_eq!(list.count(), Some(2));
    }

    #[test]
    fn the_selection_is_kept_across_a_re_list_and_dropped_when_the_stash_is_gone() {
        let mut list = StashList::default();
        assert!(list.listed(OperationId(2), vec![stash(0, "a"), stash(1, "b")]));
        assert!(list.select(&stash(1, "b").oid));
        assert_eq!(list.selected_entry().map(|e| e.index), Some(1));
        // A stash above it was dropped: the same stash has a new index.
        assert!(list.listed(OperationId(3), vec![stash(0, "b")]));
        assert_eq!(list.selected(), Some(stash(0, "b").oid.as_str()));
        assert_eq!(list.selected_entry().map(|e| e.index), Some(0));
        // The selected stash is dropped: the selection goes with it.
        assert!(list.listed(OperationId(4), vec![stash(0, "c")]));
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn only_a_listed_stash_can_be_selected() {
        let mut list = StashList::default();
        assert!(list.listed(OperationId(2), vec![stash(0, "a")]));
        assert!(!list.select(&stash(5, "z").oid));
        assert_eq!(list.selected(), None);
        assert!(list.select(&stash(0, "a").oid));
    }

    #[test]
    fn a_failed_read_keeps_gits_message_and_the_list_it_had() {
        let mut list = StashList::default();
        assert!(list.listed(OperationId(2), vec![stash(0, "a")]));
        list.request(OperationId(5));
        list.failed(OperationId(5), "fatal: not a git repository".into());
        assert_eq!(list.failure(), Some("fatal: not a git repository"));
        assert!(!list.owns(OperationId(5)));
        assert_eq!(list.count(), Some(1));
        // The next read clears the message.
        list.request(OperationId(6));
        assert_eq!(list.failure(), None);
    }

    #[test]
    fn a_failure_of_a_read_not_in_flight_is_ignored() {
        let mut list = StashList::default();
        list.request(OperationId(5));
        list.failed(OperationId(4), "old".into());
        assert_eq!(list.failure(), None);
        assert!(list.owns(OperationId(5)));
    }

    #[test]
    fn a_failed_read_with_no_message_says_the_stashes_could_not_be_read() {
        let mut list = StashList::default();
        list.request(OperationId(5));
        list.ended(OperationId(5), OperationState::Failed, None);
        assert_eq!(list.failure(), Some(NO_MESSAGE));
        assert!(!list.owns(OperationId(5)));
    }

    #[test]
    fn a_cancelled_read_leaves_nothing_in_flight_and_no_message() {
        let mut list = StashList::default();
        list.request(OperationId(5));
        list.ended(OperationId(5), OperationState::Cancelled, None);
        assert!(!list.owns(OperationId(5)));
        assert_eq!(list.failure(), None);
        // A state that is not terminal never ends the read.
        list.request(OperationId(6));
        list.ended(OperationId(6), OperationState::Executing, None);
        assert!(list.owns(OperationId(6)));
    }

    #[test]
    fn labels_follow_git_and_counts_read_as_english() {
        assert_eq!(label(0), "stash@{0}");
        assert_eq!(label(12), "stash@{12}");
        assert_eq!(count_label(1), "1 stash");
        assert_eq!(count_label(0), "0 stashes");
        assert_eq!(count_label(3), "3 stashes");
    }
}
