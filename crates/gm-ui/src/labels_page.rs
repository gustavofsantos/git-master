//! The Labels page (P4, D145): the labels of the repository, filtered by name, and
//! what is attached to the one selected. Pure state, no GPUI. The page asks the
//! engine for reads and keeps only the answer to the read in flight, so a label
//! selected after another never shows the other's attachments. Reads are not
//! recorded (D28).
//!
//! Until the Labels view lands, only the model and the tests call most of these
//! functions, so the dead-code lint is off outside tests (as in `diff_pane`).
#![cfg_attr(not(test), allow(dead_code))]

use std::cmp::Reverse;

use gm_schema::threads::{AttachKind, Label, LabelAttachment, LabelDetail};
use gm_schema::{CommandKind, OperationId};

/// The most labels one list read asks for.
pub const LIST_LIMIT: u32 = 500;

/// The order the attachment groups are shown in.
const GROUP_ORDER: [AttachKind; 4] = [
    AttachKind::Thread,
    AttachKind::File,
    AttachKind::Commit,
    AttachKind::Label,
];

/// Why the label list shows no row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Empty {
    /// The repository has no label yet.
    NoLabels,
    /// The filter matches no label.
    NoMatches,
}

/// The label list, the filter box and the selected label with its attachments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelsPage {
    query: String,
    labels: Vec<Label>,
    list_pending: Option<OperationId>,
    selected: Option<i64>,
    detail: Option<LabelDetail>,
    detail_pending: Option<OperationId>,
    failure: Option<String>,
}

impl LabelsPage {
    /// Sets the filter box text. The next list read sends it as the query.
    pub fn set_query(&mut self, query: &str) {
        query.clone_into(&mut self.query);
    }

    /// The filter box text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The list read for the current filter.
    #[must_use]
    pub fn plan_list(&self) -> CommandKind {
        CommandKind::ListLabels {
            query: self.query.clone(),
            limit: LIST_LIMIT,
        }
    }

    /// Records that the list read `id` is in flight.
    pub fn request_list(&mut self, id: OperationId) {
        self.list_pending = Some(id);
    }

    /// Keeps the answer when it is the list read in flight, sorted by name, and
    /// returns true. A selected label that a complete list no longer holds is
    /// deselected: it was deleted. Returns false for any other answer.
    pub fn listed(&mut self, id: OperationId, mut labels: Vec<Label>) -> bool {
        if self.list_pending != Some(id) {
            return false;
        }
        self.list_pending = None;
        labels.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        self.labels = labels;
        self.failure = None;
        if let Some(selected) = self.selected {
            let present = self.labels.iter().any(|label| label.id == selected);
            if !present && self.list_is_complete() {
                self.clear_selection();
            }
        }
        true
    }

    /// Whether the loaded list holds every label: no filter sent, and fewer rows than the limit.
    fn list_is_complete(&self) -> bool {
        self.query.is_empty()
            && usize::try_from(LIST_LIMIT).is_ok_and(|limit| self.labels.len() < limit)
    }

    /// The loaded labels, by name.
    #[must_use]
    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    /// The labels the filter box lets through: a case-insensitive substring of the name.
    #[must_use]
    pub fn visible(&self) -> Vec<&Label> {
        let query = self.query.to_lowercase();
        self.labels
            .iter()
            .filter(|label| label.name.to_lowercase().contains(&query))
            .collect()
    }

    /// Why the list shows no row, or `None` when it shows some. Before the first
    /// answer an empty filter reads as no labels.
    #[must_use]
    pub fn empty(&self) -> Option<Empty> {
        if !self.visible().is_empty() {
            None
        } else if self.query.is_empty() {
            Some(Empty::NoLabels)
        } else {
            Some(Empty::NoMatches)
        }
    }

    /// Selects label `id` and returns the read of it. Selecting another label drops
    /// the detail shown; selecting the same one keeps it until the new answer.
    pub fn select(&mut self, id: i64) -> CommandKind {
        if self.selected != Some(id) {
            self.detail = None;
            self.detail_pending = None;
        }
        self.selected = Some(id);
        CommandKind::ReadLabel { id }
    }

    /// The selected label, if any.
    #[must_use]
    pub fn selected(&self) -> Option<i64> {
        self.selected
    }

    /// Records that the detail read `id` is in flight.
    pub fn request_detail(&mut self, id: OperationId) {
        self.detail_pending = Some(id);
    }

    /// Keeps the answer when it is the detail read in flight and its label is still
    /// selected. Returns false for any other.
    pub fn detail_read(&mut self, id: OperationId, detail: LabelDetail) -> bool {
        if self.detail_pending != Some(id) || self.selected != Some(detail.label.id) {
            return false;
        }
        self.detail_pending = None;
        self.failure = None;
        self.detail = Some(detail);
        true
    }

    /// The detail of the selected label, once read.
    #[must_use]
    pub fn detail(&self) -> Option<&LabelDetail> {
        self.detail.as_ref()
    }

    /// The attachments of the selected label, in groups of one kind: thread, file,
    /// commit, label. Empty groups are left out; each group is newest first.
    #[must_use]
    pub fn groups(&self) -> Vec<(AttachKind, Vec<&LabelAttachment>)> {
        let Some(detail) = &self.detail else {
            return Vec::new();
        };
        GROUP_ORDER
            .into_iter()
            .filter_map(|kind| {
                let mut rows: Vec<&LabelAttachment> = detail
                    .attachments
                    .iter()
                    .filter(|attachment| attachment.kind == kind)
                    .collect();
                if rows.is_empty() {
                    return None;
                }
                rows.sort_by_key(|attachment| Reverse(attachment.at));
                Some((kind, rows))
            })
            .collect()
    }

    /// The reads to send after `LabelsChanged`: the list again and, when a label is
    /// selected, the label again.
    #[must_use]
    pub fn changed(&self) -> Vec<CommandKind> {
        let mut reads = vec![self.plan_list()];
        if let Some(id) = self.selected {
            reads.push(CommandKind::ReadLabel { id });
        }
        reads
    }

    /// Whether `id` is the list or detail read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.list_pending == Some(id) || self.detail_pending == Some(id)
    }

    /// The read `id` failed with `message`, and it is shown. A detail read that
    /// failed drops the selection, since the label cannot be read (it may be gone).
    /// Returns false when `id` is not in flight.
    pub fn failed(&mut self, id: OperationId, message: String) -> bool {
        if self.list_pending == Some(id) {
            self.list_pending = None;
        } else if self.detail_pending == Some(id) {
            self.clear_selection();
        } else {
            return false;
        }
        self.failure = Some(message);
        true
    }

    /// The read `id` ended without an answer: it is no longer in flight.
    pub fn ended(&mut self, id: OperationId) {
        if self.list_pending == Some(id) {
            self.list_pending = None;
        }
        if self.detail_pending == Some(id) {
            self.detail_pending = None;
        }
    }

    /// The message of the last failed read, if it is still shown.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.detail = None;
        self.detail_pending = None;
    }
}

/// The title of one attachment row: the commit subject, or the short commit id; the
/// file's current path, or the path as attached; the thread uid or label id otherwise.
#[must_use]
pub fn attachment_title(attachment: &LabelAttachment) -> String {
    match attachment.kind {
        AttachKind::Commit => attachment
            .subject
            .as_deref()
            .filter(|subject| !subject.is_empty())
            .map_or_else(
                || attachment.reference.chars().take(7).collect(),
                str::to_owned,
            ),
        AttachKind::File => attachment
            .current_path
            .clone()
            .unwrap_or_else(|| attachment.reference.clone()),
        AttachKind::Thread | AttachKind::Label => attachment.reference.clone(),
    }
}

/// The text that says who made a row, and whether it was an agent. `human` reads
/// as `You`; `agent:<client>` reads as the client name.
#[must_use]
pub fn origin_text(origin: &str) -> (String, bool) {
    if origin == "human" {
        return ("You".to_owned(), false);
    }
    match origin.strip_prefix("agent:") {
        Some("") => ("Agent".to_owned(), true),
        Some(client) => (client.to_owned(), true),
        None => (origin.to_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(id: i64, name: &str) -> Label {
        Label {
            id,
            name: name.to_owned(),
            description: String::new(),
            color: None,
            origin: "human".to_owned(),
            created_at: 0,
            attachments: 0,
        }
    }

    fn attachment(kind: AttachKind, reference: &str, at: i64) -> LabelAttachment {
        LabelAttachment {
            kind,
            reference: reference.to_owned(),
            origin: "human".to_owned(),
            at,
            subject: None,
            author: None,
            committed_at: None,
            resolved: None,
            state: None,
            current_path: None,
        }
    }

    fn detail(id: i64, attachments: Vec<LabelAttachment>) -> LabelDetail {
        LabelDetail {
            label: label(id, "flaky"),
            attachments,
        }
    }

    fn names(page: &LabelsPage) -> Vec<&str> {
        page.visible()
            .into_iter()
            .map(|label| label.name.as_str())
            .collect()
    }

    #[test]
    fn the_list_read_sends_the_query_and_the_limit() {
        let mut page = LabelsPage::default();
        assert_eq!(
            page.plan_list(),
            CommandKind::ListLabels {
                query: String::new(),
                limit: 500
            }
        );
        page.set_query("flak");
        assert_eq!(page.query(), "flak");
        assert_eq!(
            page.plan_list(),
            CommandKind::ListLabels {
                query: "flak".to_owned(),
                limit: 500
            }
        );
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_of_the_name() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        assert!(page.listed(
            OperationId(1),
            vec![
                label(1, "Flaky"),
                label(2, "perf"),
                label(3, "needs-flakiness")
            ]
        ));
        assert_eq!(names(&page), vec!["Flaky", "needs-flakiness", "perf"]);
        page.set_query("FLAK");
        assert_eq!(names(&page), vec!["Flaky", "needs-flakiness"]);
        page.set_query("");
        assert_eq!(names(&page).len(), 3, "an empty filter shows all");
    }

    #[test]
    fn the_answer_to_the_list_read_in_flight_is_kept_sorted_by_name() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(4));
        assert!(page.owns(OperationId(4)));
        assert!(page.listed(
            OperationId(4),
            vec![label(9, "zeta"), label(8, "Alpha"), label(7, "beta")]
        ));
        assert!(!page.owns(OperationId(4)));
        let order: Vec<&str> = page.labels().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(order, vec!["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn an_answer_to_a_superseded_list_read_is_dropped() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        page.request_list(OperationId(2));
        assert!(!page.listed(OperationId(1), vec![label(1, "old")]));
        assert!(page.labels().is_empty());
        assert!(page.listed(OperationId(2), vec![label(2, "new")]));
        assert_eq!(names(&page), vec!["new"]);
    }

    #[test]
    fn the_empty_state_says_whether_there_are_no_labels_or_no_match() {
        let mut page = LabelsPage::default();
        assert_eq!(page.empty(), Some(Empty::NoLabels));
        page.request_list(OperationId(1));
        page.listed(OperationId(1), vec![label(1, "bug")]);
        assert_eq!(page.empty(), None, "rows show, so no empty state");
        page.set_query("zzz");
        assert_eq!(page.empty(), Some(Empty::NoMatches));
    }

    #[test]
    fn selecting_a_label_asks_for_its_read_and_selects_it() {
        let mut page = LabelsPage::default();
        assert_eq!(page.select(5), CommandKind::ReadLabel { id: 5 });
        assert_eq!(page.selected(), Some(5));
    }

    #[test]
    fn the_detail_read_in_flight_is_kept_for_the_selected_label() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(3));
        assert!(page.detail_read(OperationId(3), detail(5, Vec::new())));
        assert_eq!(page.detail().map(|d| d.label.id), Some(5));
    }

    #[test]
    fn a_detail_answer_for_a_label_no_longer_selected_is_dropped() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(3));
        page.select(6);
        assert!(!page.detail_read(OperationId(3), detail(5, Vec::new())));
        assert!(page.detail().is_none());
    }

    #[test]
    fn a_superseded_detail_read_of_the_same_label_is_dropped() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(3));
        page.request_detail(OperationId(4));
        assert!(!page.detail_read(OperationId(3), detail(5, Vec::new())));
        assert!(page.detail_read(OperationId(4), detail(5, Vec::new())));
    }

    #[test]
    fn reselecting_the_same_label_keeps_its_detail_until_the_new_answer() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(1));
        assert!(page.detail_read(OperationId(1), detail(5, Vec::new())));
        page.select(5);
        assert!(page.detail().is_some(), "no flicker on a re-read");
        page.select(6);
        assert!(page.detail().is_none(), "another label drops it");
    }

    #[test]
    fn a_deleted_selection_clears_when_the_complete_list_no_longer_holds_it() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        page.listed(OperationId(1), vec![label(5, "gone"), label(6, "kept")]);
        page.select(5);
        page.request_detail(OperationId(2));
        assert!(page.detail_read(OperationId(2), detail(5, Vec::new())));
        let _ = page.changed();
        page.request_list(OperationId(3));
        assert!(page.listed(OperationId(3), vec![label(6, "kept")]));
        assert_eq!(page.selected(), None);
        assert!(page.detail().is_none());
    }

    #[test]
    fn a_filtered_list_that_lacks_the_selected_label_keeps_the_selection() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.set_query("kept");
        page.request_list(OperationId(1));
        assert!(page.listed(OperationId(1), vec![label(6, "kept")]));
        assert_eq!(page.selected(), Some(5), "a filter is not a deletion");
    }

    #[test]
    fn groups_come_in_thread_file_commit_label_order_newest_first_and_omit_empty_ones() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(1));
        page.detail_read(
            OperationId(1),
            detail(
                5,
                vec![
                    attachment(AttachKind::Label, "8", 10),
                    attachment(AttachKind::Commit, "abc", 1),
                    attachment(AttachKind::Thread, "t-old", 2),
                    attachment(AttachKind::Thread, "t-new", 9),
                    attachment(AttachKind::Commit, "def", 7),
                ],
            ),
        );
        let groups = page.groups();
        let kinds: Vec<AttachKind> = groups.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            vec![AttachKind::Thread, AttachKind::Commit, AttachKind::Label],
            "no file group: it is omitted"
        );
        let threads: Vec<&str> = groups[0].1.iter().map(|a| a.reference.as_str()).collect();
        assert_eq!(threads, vec!["t-new", "t-old"]);
        let commits: Vec<&str> = groups[1].1.iter().map(|a| a.reference.as_str()).collect();
        assert_eq!(commits, vec!["def", "abc"]);
    }

    #[test]
    fn there_are_no_groups_before_a_detail_is_read() {
        let mut page = LabelsPage::default();
        page.select(5);
        assert!(page.groups().is_empty());
    }

    #[test]
    fn a_commit_row_is_titled_by_its_subject_or_its_short_id() {
        let mut with_subject = attachment(AttachKind::Commit, "0123456789abcdef", 1);
        with_subject.subject = Some("Fix the parser".to_owned());
        assert_eq!(attachment_title(&with_subject), "Fix the parser");
        let mut empty_subject = attachment(AttachKind::Commit, "0123456789abcdef", 1);
        empty_subject.subject = Some(String::new());
        assert_eq!(attachment_title(&empty_subject), "0123456");
        let bare = attachment(AttachKind::Commit, "0123456789abcdef", 1);
        assert_eq!(attachment_title(&bare), "0123456");
    }

    #[test]
    fn a_file_row_is_titled_by_its_current_path_then_by_its_ref() {
        let mut moved = attachment(AttachKind::File, "src/old.rs", 1);
        moved.current_path = Some("src/new.rs".to_owned());
        assert_eq!(attachment_title(&moved), "src/new.rs");
        let untracked = attachment(AttachKind::File, "src/old.rs", 1);
        assert_eq!(attachment_title(&untracked), "src/old.rs");
    }

    #[test]
    fn a_thread_or_label_row_is_titled_by_its_ref() {
        assert_eq!(
            attachment_title(&attachment(AttachKind::Thread, "uid-1", 1)),
            "uid-1"
        );
        assert_eq!(
            attachment_title(&attachment(AttachKind::Label, "42", 1)),
            "42"
        );
    }

    #[test]
    fn origin_text_reads_human_as_you_and_agent_as_its_client() {
        assert_eq!(origin_text("human"), ("You".to_owned(), false));
        assert_eq!(
            origin_text("agent:claude-code"),
            ("claude-code".to_owned(), true)
        );
        assert_eq!(origin_text("agent:"), ("Agent".to_owned(), true));
    }

    #[test]
    fn the_reads_after_a_change_are_the_list_and_the_selected_label() {
        let mut page = LabelsPage::default();
        assert_eq!(
            page.changed(),
            vec![CommandKind::ListLabels {
                query: String::new(),
                limit: 500
            }]
        );
        page.select(5);
        assert_eq!(
            page.changed().last(),
            Some(&CommandKind::ReadLabel { id: 5 })
        );
        assert_eq!(page.changed().len(), 2);
    }

    #[test]
    fn a_failed_detail_read_shows_why_and_drops_the_selection() {
        let mut page = LabelsPage::default();
        page.select(5);
        page.request_detail(OperationId(3));
        assert!(page.failed(OperationId(3), "no such label".to_owned()));
        assert_eq!(page.failure(), Some("no such label"));
        assert_eq!(page.selected(), None);
        assert!(!page.owns(OperationId(3)));
    }

    #[test]
    fn a_failed_list_read_shows_why_and_keeps_the_selection() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        page.select(5);
        assert!(page.failed(OperationId(1), "git is busy".to_owned()));
        assert_eq!(page.failure(), Some("git is busy"));
        assert_eq!(page.selected(), Some(5));
        assert!(!page.owns(OperationId(1)));
    }

    #[test]
    fn a_failure_of_a_superseded_read_is_dropped() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        page.request_list(OperationId(2));
        assert!(!page.failed(OperationId(1), "old".to_owned()));
        assert_eq!(page.failure(), None);
        assert!(page.owns(OperationId(2)));
    }

    #[test]
    fn an_ended_read_leaves_nothing_in_flight_and_shows_no_failure() {
        let mut page = LabelsPage::default();
        page.request_list(OperationId(1));
        page.ended(OperationId(1));
        assert!(!page.owns(OperationId(1)));
        assert_eq!(page.failure(), None);
        page.select(5);
        page.request_detail(OperationId(2));
        page.ended(OperationId(2));
        assert!(!page.owns(OperationId(2)));
        assert_eq!(
            page.selected(),
            Some(5),
            "a cancelled read keeps the selection"
        );
    }
}
