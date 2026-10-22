//! The Reflog page's model (B3, D95), with no GPUI in it: the entries of HEAD
//! the engine read, a page at a time, the read in flight and its failure, the
//! entry the user selected, and the detached checkout in flight. Ids follow
//! dispatch order: an answer to a request that is no longer the one in flight
//! is dropped, as the Stashes list drops one (B2).

use gm_schema::{CommandKind, OperationId, OperationState, ReflogEntry};

/// Git's message when a failed read gave none.
const NO_MESSAGE: &str = "The reflog could not be read.";
/// The message of a failed checkout when the engine gave no reason.
const NOT_MOVED: &str = "HEAD was not moved.";

/// The name of an entry as Git writes it: `HEAD@{n}`.
#[must_use]
pub fn label(index: u32) -> String {
    format!("HEAD@{{{index}}}")
}

/// The first seven characters of a commit id, as the rows show it.
#[must_use]
pub fn short(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

/// Whether the action is a commit's (`commit`, `commit (amend)`, ...).
#[must_use]
pub fn is_commit_action(action: &str) -> bool {
    action.split_whitespace().next() == Some("commit")
}

/// The text a row shows after its chip: the subject of a commit's action when
/// the subject is known, otherwise the entry's detail (such as `moving from main
/// to topic` for a checkout).
#[must_use]
pub fn row_text(entry: &ReflogEntry) -> &str {
    match &entry.subject {
        Some(subject) if is_commit_action(&entry.action) => subject,
        _ => &entry.detail,
    }
}

/// The detail panel's header: the entry's name and its action, such as `HEAD@{3} · checkout`.
#[must_use]
pub fn header(entry: &ReflogEntry) -> String {
    format!("{} · {}", label(entry.index), entry.action)
}

/// The count in the header: "1 entry", "12 entries", or "First N entries" while
/// more pages follow.
#[must_use]
pub fn count_label(count: usize, more: bool) -> String {
    if more {
        format!("First {count} entries")
    } else if count == 1 {
        "1 entry".to_owned()
    } else {
        format!("{count} entries")
    }
}

/// What identifies an entry across reloads: the commit it moved HEAD to, when it
/// was written and what it did. The index is not part of it, since a new entry
/// above moves every index down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryKey {
    oid: String,
    time: i64,
    action: String,
}

impl EntryKey {
    #[must_use]
    pub fn of(entry: &ReflogEntry) -> Self {
        Self {
            oid: entry.oid.clone(),
            time: entry.time,
            action: entry.action.clone(),
        }
    }

    /// The commit the entry moved HEAD to.
    #[must_use]
    pub fn oid(&self) -> &str {
        &self.oid
    }

    #[must_use]
    pub fn matches(&self, entry: &ReflogEntry) -> bool {
        self.oid == entry.oid && self.time == entry.time && self.action == entry.action
    }
}

/// The entries on the Reflog page, the read that will replace or extend them,
/// and the selection. A first page (`skip` 0) replaces what is shown; a next
/// page follows it. Only the answer to the read in flight is applied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReflogList {
    /// The read in flight. Its answer is the only one applied.
    pending: Option<OperationId>,
    /// Whether a first page has been shown.
    loaded: bool,
    entries: Vec<ReflogEntry>,
    has_more: bool,
    /// Git's message when the newest read failed. The next read clears it.
    failure: Option<String>,
    /// The entry the user selected. Kept across reloads while the entry is listed.
    selected: Option<EntryKey>,
}

impl ReflogList {
    /// Starts a read, of the first page or of the next one. What is shown stays
    /// on screen until its answer comes.
    pub fn request(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.failure = None;
    }

    /// Whether `id` is the read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Applies the answer `id` to the read that started after `skip` entries.
    /// Returns false when the answer is not the read in flight, or when a page
    /// does not follow the entries shown (the list moved on).
    pub fn listed(
        &mut self,
        id: OperationId,
        skip: u32,
        entries: Vec<ReflogEntry>,
        has_more: bool,
    ) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        if skip == 0 {
            self.entries = entries;
        } else if usize::try_from(skip) == Ok(self.entries.len()) {
            self.entries.extend(entries);
        } else {
            return false;
        }
        self.loaded = true;
        self.has_more = has_more;
        if let Some(key) = &self.selected
            && !self.entries.iter().any(|entry| key.matches(entry))
        {
            self.selected = None;
        }
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

    /// Selects the listed entry that `key` names. False when no listed entry matches.
    pub fn select(&mut self, key: &EntryKey) -> bool {
        if self.entries.iter().any(|entry| key.matches(entry)) {
            self.selected = Some(key.clone());
            true
        } else {
            false
        }
    }

    /// Whether `entry` is the selected one.
    #[must_use]
    pub fn is_selected(&self, entry: &ReflogEntry) -> bool {
        self.selected.as_ref().is_some_and(|key| key.matches(entry))
    }

    /// The selected entry as listed, with its current index.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&ReflogEntry> {
        let key = self.selected.as_ref()?;
        self.entries.iter().find(|entry| key.matches(entry))
    }

    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    #[must_use]
    pub fn entries(&self) -> &[ReflogEntry] {
        &self.entries
    }

    #[must_use]
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Whether a first page has been shown.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Git's message when the newest read failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Whether the next page should be asked for: a page is shown, more follow,
    /// and no read is in flight or has failed.
    #[must_use]
    pub fn can_page(&self) -> bool {
        self.loaded && self.has_more && self.pending.is_none() && self.failure.is_none()
    }

    /// The `skip` of the next page: how many entries are shown.
    #[must_use]
    pub fn next_skip(&self) -> u32 {
        u32::try_from(self.entries.len()).unwrap_or(u32::MAX)
    }
}

/// A detached checkout in flight, and its outcome until the window takes it.
/// Engine events for other operations never touch it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetachedCheckout {
    /// The operation in flight and the short id it checks out.
    pending: Option<(OperationId, String)>,
    outcome: Option<CheckoutOutcome>,
}

/// How a detached checkout ended. A failure carries Git's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutOutcome {
    Done { short: String },
    Failed { short: String, text: String },
}

impl DetachedCheckout {
    pub fn start(&mut self, id: OperationId, oid: &str) {
        self.pending = Some((id, short(oid).to_owned()));
        self.outcome = None;
    }

    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
    }

    /// Ends the checkout with its terminal `state`. Anything but Done is a failure.
    pub fn settle(&mut self, state: OperationState, detail: Option<String>) {
        if !state.is_terminal() {
            return;
        }
        let Some((_, short)) = self.pending.take() else {
            return;
        };
        self.outcome = Some(if state == OperationState::Done {
            CheckoutOutcome::Done { short }
        } else {
            CheckoutOutcome::Failed {
                short,
                text: detail.unwrap_or_else(|| NOT_MOVED.to_owned()),
            }
        });
    }

    pub fn take(&mut self) -> Option<CheckoutOutcome> {
        self.outcome.take()
    }
}

/// The notice when a commit was checked out: "Checked out 3f2a9c1 (detached HEAD)".
#[must_use]
pub fn done_notice(short: &str) -> String {
    format!("Checked out {short} (detached HEAD)")
}

/// The title of the message when a commit was not checked out.
#[must_use]
pub fn failure_title(short: &str) -> String {
    format!("Could not check out {short}")
}

/// The commit a checkout command names, so the window can track the run.
#[must_use]
pub fn checkout_target(kind: &CommandKind) -> Option<&str> {
    match kind {
        CommandKind::CheckoutCommit { oid } => Some(oid.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: u32, oid: char, action: &str, detail: &str, time: i64) -> ReflogEntry {
        ReflogEntry {
            index,
            oid: oid.to_string().repeat(40),
            action: action.into(),
            detail: detail.into(),
            subject: None,
            time,
        }
    }

    fn with_subject(mut entry: ReflogEntry, subject: &str) -> ReflogEntry {
        entry.subject = Some(subject.into());
        entry
    }

    fn indexes(list: &ReflogList) -> Vec<u32> {
        list.entries().iter().map(|entry| entry.index).collect()
    }

    /// A list whose first page of `entries` has been shown, with `more` after it.
    fn shown(entries: Vec<ReflogEntry>, more: bool) -> ReflogList {
        let mut list = ReflogList::default();
        list.request(OperationId(1));
        assert!(list.listed(OperationId(1), 0, entries, more));
        list
    }

    #[test]
    fn labels_and_counts_follow_git_and_read_as_english() {
        assert_eq!(label(0), "HEAD@{0}");
        assert_eq!(label(12), "HEAD@{12}");
        assert_eq!(short(&"a".repeat(40)), "aaaaaaa");
        assert_eq!(short("abc"), "abc");
        assert_eq!(count_label(1, false), "1 entry");
        assert_eq!(count_label(0, false), "0 entries");
        assert_eq!(count_label(3, false), "3 entries");
        assert_eq!(count_label(200, true), "First 200 entries");
    }

    #[test]
    fn a_commit_action_shows_its_subject_and_other_actions_their_detail() {
        let commit = with_subject(
            entry(0, 'a', "commit (amend)", "fix: the new subject", 5),
            "fix: the subject",
        );
        assert!(is_commit_action("commit"));
        assert!(is_commit_action("commit (amend)"));
        assert!(!is_commit_action("checkout"));
        assert!(!is_commit_action("merge main"));
        assert!(!is_commit_action("rebase (finish)"));
        assert_eq!(row_text(&commit), "fix: the subject");
        let no_subject = entry(1, 'b', "commit", "fix: gone", 5);
        assert_eq!(row_text(&no_subject), "fix: gone");
        let checkout = with_subject(
            entry(2, 'c', "checkout", "moving from main to topic", 5),
            "a subject that is not shown",
        );
        assert_eq!(row_text(&checkout), "moving from main to topic");
        assert_eq!(header(&checkout), "HEAD@{2} · checkout");
    }

    #[test]
    fn the_first_page_is_shown_once_its_read_answers() {
        let mut list = ReflogList::default();
        assert!(!list.is_loaded());
        list.request(OperationId(3));
        assert!(list.owns(OperationId(3)));
        assert!(!list.is_loaded());
        assert!(list.listed(
            OperationId(3),
            0,
            vec![
                entry(0, 'a', "commit", "x", 1),
                entry(1, 'b', "commit", "y", 2)
            ],
            true,
        ));
        assert!(!list.owns(OperationId(3)));
        assert!(list.is_loaded());
        assert_eq!(indexes(&list), [0, 1]);
        assert!(list.has_more());
        assert!(list.can_page());
    }

    #[test]
    fn a_next_page_follows_the_list_and_nothing_is_asked_while_it_is_in_flight() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], true);
        assert_eq!(list.next_skip(), 1);
        list.request(OperationId(5));
        assert!(!list.can_page());
        assert!(list.listed(
            OperationId(5),
            1,
            vec![entry(1, 'b', "commit", "y", 2)],
            false
        ));
        assert_eq!(indexes(&list), [0, 1]);
        assert!(!list.has_more());
        assert!(!list.can_page());
    }

    #[test]
    fn a_page_that_does_not_follow_the_list_is_dropped() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], true);
        list.request(OperationId(5));
        assert!(!list.listed(
            OperationId(5),
            7,
            vec![entry(7, 'z', "commit", "z", 9)],
            false
        ));
        assert_eq!(indexes(&list), [0]);
        assert!(!list.owns(OperationId(5)));
    }

    #[test]
    fn a_read_superseded_by_a_newer_one_is_dropped() {
        let mut list = ReflogList::default();
        list.request(OperationId(3));
        list.request(OperationId(5));
        assert!(!list.listed(
            OperationId(3),
            0,
            vec![entry(0, 'a', "commit", "x", 1)],
            false
        ));
        assert!(!list.is_loaded());
        assert!(list.listed(
            OperationId(5),
            0,
            vec![entry(0, 'b', "commit", "y", 2)],
            false
        ));
        assert_eq!(list.entries()[0].oid, "b".repeat(40));
    }

    #[test]
    fn a_page_superseded_by_a_first_page_is_dropped() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], true);
        list.request(OperationId(4));
        list.request(OperationId(6));
        assert!(!list.listed(
            OperationId(4),
            1,
            vec![entry(1, 'b', "commit", "y", 2)],
            false
        ));
        assert_eq!(indexes(&list), [0]);
        assert!(list.owns(OperationId(6)));
    }

    #[test]
    fn a_reload_keeps_only_the_first_page() {
        let mut list = shown(
            vec![
                entry(0, 'a', "commit", "x", 1),
                entry(1, 'b', "commit", "y", 2),
                entry(2, 'c', "commit", "z", 3),
            ],
            true,
        );
        list.request(OperationId(7));
        assert!(list.listed(
            OperationId(7),
            0,
            vec![entry(0, 'd', "commit", "w", 4)],
            true
        ));
        assert_eq!(indexes(&list), [0]);
        assert!(list.has_more());
    }

    #[test]
    fn the_selection_survives_a_reload_by_its_identity_and_follows_the_new_index() {
        let mut list = shown(
            vec![
                entry(0, 'a', "checkout", "moving from main to topic", 10),
                entry(1, 'b', "commit", "fix: b", 5),
            ],
            false,
        );
        assert!(list.select(&EntryKey::of(&entry(1, 'b', "commit", "fix: b", 5))));
        assert!(list.is_selected(&entry(1, 'b', "commit", "fix: b", 5)));
        // A commit above it moved every index down by one.
        list.request(OperationId(4));
        assert!(list.listed(
            OperationId(4),
            0,
            vec![
                entry(0, 'c', "commit", "new", 20),
                entry(1, 'a', "checkout", "moving from main to topic", 10),
                entry(2, 'b', "commit", "fix: b", 5),
            ],
            false,
        ));
        assert!(list.has_selection());
        assert_eq!(list.selected_entry().map(|e| e.index), Some(2));
    }

    #[test]
    fn the_selection_is_dropped_when_its_entry_is_gone() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], false);
        assert!(list.select(&EntryKey::of(&entry(0, 'a', "commit", "x", 1))));
        list.request(OperationId(4));
        assert!(list.listed(
            OperationId(4),
            0,
            vec![entry(0, 'q', "commit", "q", 9)],
            false
        ));
        assert!(!list.has_selection());
        assert_eq!(list.selected_entry(), None);
    }

    #[test]
    fn only_a_listed_entry_can_be_selected() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], false);
        assert!(!list.select(&EntryKey::of(&entry(0, 'z', "commit", "x", 1))));
        assert!(!list.select(&EntryKey::of(&entry(0, 'a', "commit", "x", 2))));
        assert!(!list.has_selection());
    }

    #[test]
    fn a_failed_read_keeps_the_list_and_gits_message_until_the_next_read() {
        let mut list = shown(vec![entry(0, 'a', "commit", "x", 1)], true);
        list.request(OperationId(9));
        list.failed(OperationId(9), "fatal: bad revision".into());
        assert_eq!(list.failure(), Some("fatal: bad revision"));
        assert!(!list.owns(OperationId(9)));
        assert_eq!(indexes(&list), [0]);
        assert!(!list.can_page());
        list.request(OperationId(10));
        assert_eq!(list.failure(), None);
    }

    #[test]
    fn a_failure_without_a_message_says_the_reflog_could_not_be_read() {
        let mut list = ReflogList::default();
        list.request(OperationId(5));
        list.ended(OperationId(5), OperationState::Failed, None);
        assert_eq!(list.failure(), Some("The reflog could not be read."));
        assert!(!list.owns(OperationId(5)));
    }

    #[test]
    fn a_cancelled_read_leaves_nothing_in_flight_and_no_message() {
        let mut list = ReflogList::default();
        list.request(OperationId(5));
        list.ended(OperationId(5), OperationState::Cancelled, None);
        assert!(!list.owns(OperationId(5)));
        assert_eq!(list.failure(), None);
        list.request(OperationId(6));
        list.ended(OperationId(6), OperationState::Executing, None);
        assert!(list.owns(OperationId(6)));
    }

    #[test]
    fn a_failure_of_a_read_not_in_flight_is_ignored() {
        let mut list = ReflogList::default();
        list.request(OperationId(5));
        list.failed(OperationId(4), "old".into());
        assert_eq!(list.failure(), None);
        assert!(list.owns(OperationId(5)));
    }

    #[test]
    fn a_detached_checkout_reports_done_with_the_short_id_once_it_ends() {
        let mut checkout = DetachedCheckout::default();
        checkout.start(OperationId(4), &"a".repeat(40));
        assert!(checkout.owns(OperationId(4)));
        checkout.settle(OperationState::Executing, None);
        assert_eq!(checkout.take(), None);
        checkout.settle(OperationState::Done, None);
        assert!(!checkout.owns(OperationId(4)));
        assert_eq!(
            checkout.take(),
            Some(CheckoutOutcome::Done {
                short: "aaaaaaa".into()
            })
        );
        assert_eq!(checkout.take(), None);
    }

    #[test]
    fn a_refused_checkout_keeps_gits_text() {
        let mut checkout = DetachedCheckout::default();
        checkout.start(OperationId(8), &"b".repeat(40));
        let git = "error: Your local changes would be overwritten by checkout.".to_owned();
        checkout.settle(OperationState::Failed, Some(git.clone()));
        assert_eq!(
            checkout.take(),
            Some(CheckoutOutcome::Failed {
                short: "bbbbbbb".into(),
                text: git,
            })
        );
    }

    #[test]
    fn a_checkout_without_a_reason_says_head_was_not_moved() {
        let mut checkout = DetachedCheckout::default();
        checkout.start(OperationId(9), &"c".repeat(40));
        checkout.settle(OperationState::Cancelled, None);
        assert_eq!(
            checkout.take(),
            Some(CheckoutOutcome::Failed {
                short: "ccccccc".into(),
                text: "HEAD was not moved.".into(),
            })
        );
    }

    #[test]
    fn the_notice_and_the_title_name_the_commit() {
        assert_eq!(
            done_notice("3f2a9c1"),
            "Checked out 3f2a9c1 (detached HEAD)"
        );
        assert_eq!(failure_title("3f2a9c1"), "Could not check out 3f2a9c1");
    }

    #[test]
    fn only_the_checkout_command_names_a_commit_to_track() {
        assert_eq!(
            checkout_target(&CommandKind::CheckoutCommit {
                oid: "d".repeat(40),
            }),
            Some("dddddddddddddddddddddddddddddddddddddddd")
        );
        assert_eq!(checkout_target(&CommandKind::LoadReflog { skip: 0 }), None);
    }
}
