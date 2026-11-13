//! The provenance view of a line range on the Code page (P4, D144): the side panel that
//! says why the lines are as they are. Pure: the panel's state, the rows it shows and
//! their wording. The window draws the rows and sends the read. The engine answers
//! `ReadProvenance` and supersedes a newer read itself; the panel keeps only the answer
//! to the read in flight, so an answer for another range never shows. Text from the
//! threads is untrusted: it is shown as plain text, with its line breaks removed and its
//! length capped.

use gm_schema::threads::{
    CutBound, EntryKind, Evidence, ProvenanceCommit, ProvenanceCut, ProvenanceResult,
    ProvenanceThread, RelationDirection, ThreadRelKind, Unverifiable,
};
use gm_schema::{OperationId, OperationState};

/// What the panel says when no thread explains the lines (D144).
pub const NO_THREAD: &str = "No thread is linked to these lines";
/// What the panel says while the read is in flight.
pub const READING: &str = "Reading history…";
/// What the panel says when the read stopped without an answer.
pub const NO_ANSWER: &str = "The read stopped before it answered.";
/// What the panel says when the selection moved after the question.
pub const MOVED: &str = "The selection moved. Choose Show Why This Code Is Here… again.";
/// What the panel says when the read failed without a reason.
const FAILED: &str = "The history could not be read.";
/// The most characters of thread text one row shows; the rest is cut with an ellipsis.
pub const TEXT_CAP: usize = 160;
/// How many characters of a commit id the rows show.
const SHORT_ID: usize = 7;
/// Milliseconds in a day, for the date of a commit or an edit.
const DAY_MS: i64 = 86_400_000;

/// The question the panel answers: a repository-relative file and 1-based lines, both ends
/// inclusive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub path: String,
    pub from: u32,
    pub to: u32,
}

impl Asked {
    fn is(&self, path: &str, from: u32, to: u32) -> bool {
        self.path == path && self.from == from && self.to == to
    }
}

/// The panel's state: the question, the read in flight, its answer or failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProvenancePanel {
    asked: Option<Asked>,
    pending: Option<OperationId>,
    answer: Option<ProvenanceResult>,
    failure: Option<String>,
    moved: bool,
}

impl ProvenancePanel {
    /// Asks about `path` lines `from` to `to`, which the read `id` answers. Any earlier
    /// question and answer go: the newer read supersedes the older one in the engine.
    pub fn request(&mut self, path: &str, from: u32, to: u32, id: OperationId) {
        self.asked = Some(Asked {
            path: path.to_owned(),
            from,
            to,
        });
        self.pending = Some(id);
        self.answer = None;
        self.failure = None;
        self.moved = false;
    }

    /// The question the panel answers, if the panel is open.
    #[must_use]
    pub fn asked(&self) -> Option<&Asked> {
        self.asked.as_ref()
    }

    /// Whether the panel is open: a question was asked and the panel was not closed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.asked.is_some()
    }

    /// Whether the read in flight has not answered yet.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether `id` is the read in flight. An answer to any other read is not shown.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Keeps the answer when it is the read in flight. Returns false for any other.
    pub fn answered(&mut self, id: OperationId, provenance: ProvenanceResult) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.answer = Some(provenance);
        true
    }

    /// The read `id` ended without an answer. A failure keeps its reason for the panel; a
    /// read that was stopped simply ends. A read that is `Done` is not over: its answer follows.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<&str>) {
        // The engine sends `Done` before the answer, so a read that is done keeps waiting for it.
        if !self.owns(id) || state == OperationState::Done {
            return;
        }
        self.pending = None;
        if state == OperationState::Failed {
            self.failure = Some(detail.unwrap_or(FAILED).to_owned());
        }
    }

    /// The selection on the Code page is now `now`. When it is not the question, the read
    /// in flight is left to the engine and its answer no longer shows, and the panel says
    /// the selection moved. Returns whether that happened.
    pub fn selection_moved(&mut self, now: Option<(&str, u32, u32)>) -> bool {
        let Some(asked) = &self.asked else {
            return false;
        };
        if now.is_some_and(|(path, from, to)| asked.is(path, from, to)) {
            return false;
        }
        self.pending = None;
        self.answer = None;
        self.failure = None;
        self.moved = true;
        true
    }

    /// Closes the panel. Its read, if any, is left to finish and its answer is dropped.
    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// The rows the panel shows, top to bottom. Empty when the panel is closed.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        if self.asked.is_none() {
            return Vec::new();
        }
        if self.moved {
            return vec![Row::Note(MOVED.to_owned())];
        }
        if let Some(reason) = &self.failure {
            return vec![Row::Failure(reason.clone())];
        }
        let Some(answer) = &self.answer else {
            let note = if self.is_pending() {
                READING
            } else {
                NO_ANSWER
            };
            return vec![Row::Note(note.to_owned())];
        };
        answer_rows(answer)
    }
}

/// The rows of an answer: a note when no thread explains the lines, then each commit with
/// its threads, then the partial-result notices.
fn answer_rows(answer: &ProvenanceResult) -> Vec<Row> {
    let mut rows = Vec::new();
    let threads: usize = answer.commits.iter().map(|c| c.threads.len()).sum();
    if threads == 0 {
        rows.push(Row::Note(NO_THREAD.to_owned()));
    }
    for commit in &answer.commits {
        push_commit(&mut rows, commit, &answer.path);
    }
    rows.extend(answer.cuts.iter().map(|cut| Row::Cut(cut_text(cut))));
    rows
}

fn push_commit(rows: &mut Vec<Row>, commit: &ProvenanceCommit, asked_path: &str) {
    let mut text = format!(
        "{} {} · {} · {} · lines {}",
        short(&commit.id),
        flatten(&commit.subject, TEXT_CAP),
        flatten(&commit.author, TEXT_CAP),
        civil_date(commit.author_at),
        lines_text(commit),
    );
    if commit.path != asked_path {
        text.push_str(" · under ");
        text.push_str(&flatten(&commit.path, TEXT_CAP));
    }
    rows.push(Row::Commit {
        oid: commit.id.clone(),
        text,
    });
    for thread in &commit.threads {
        push_thread(rows, thread);
    }
}

fn push_thread(rows: &mut Vec<Row>, thread: &ProvenanceThread) {
    rows.push(Row::Thread {
        uid: thread.uid.clone(),
        title: flatten(&thread.title, TEXT_CAP),
        harness: flatten(&thread.harness, TEXT_CAP),
        evidence: evidence_labels(&thread.evidence),
        ambiguous: thread.ambiguous,
    });
    for label in &thread.labels {
        rows.push(Row::Label {
            name: flatten(&label.name, TEXT_CAP),
            origin: origin_text(&label.origin),
        });
    }
    for link in &thread.links {
        rows.push(Row::Link {
            uid: link.uid.clone(),
            relation: relation_text(link.kind, link.direction),
            title: flatten(&link.title, TEXT_CAP),
            origin: origin_text(&link.origin),
        });
    }
    for edit in &thread.edits {
        rows.push(Row::Edit {
            uid: thread.uid.clone(),
            entry_id: edit.entry_id,
            seq: edit.seq,
            path: flatten(&edit.path, TEXT_CAP),
            at: civil_date(edit.at),
            evidence: evidence_labels(&edit.evidence),
            unverifiable: edit.unverifiable.map(unverifiable_text),
        });
        for entry in &edit.entries {
            rows.push(Row::Entry {
                speaker: speaker(entry.kind),
                text: flatten(&entry.text, TEXT_CAP),
            });
        }
    }
}

/// One row of the panel. Each row is one line high; the window clips what does not fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A line that says what the panel shows: a wait, an empty result, a moved selection.
    Note(String),
    /// The read failed, with its reason.
    Failure(String),
    /// A partial result, with the bound that cut it.
    Cut(String),
    /// A commit that last changed some of the lines, with its path when Git reports another.
    Commit { oid: String, text: String },
    /// A thread that explains some lines, with its evidence in strength order.
    Thread {
        uid: String,
        title: String,
        harness: String,
        evidence: Vec<&'static str>,
        ambiguous: bool,
    },
    /// A label on the thread above; the window draws it as a chip.
    Label { name: String, origin: String },
    /// A thread linked to the thread above.
    Link {
        uid: String,
        relation: &'static str,
        title: String,
        origin: String,
    },
    /// An edit of the thread above that was compared with the lines. `entry_id` and `seq` are
    /// the entry that carried the edit, where the thread opens focused.
    Edit {
        uid: String,
        entry_id: i64,
        seq: u32,
        path: String,
        at: String,
        evidence: Vec<&'static str>,
        unverifiable: Option<&'static str>,
    },
    /// An entry around an edit. The text is the thread's, untrusted, and shown as plain text.
    Entry { speaker: &'static str, text: String },
}

/// Where a row leads when it is clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Jump {
    /// The Threads page, on this thread.
    Thread(String),
    /// The Threads page, on this thread, focused on the entry that carried an edit.
    Edit {
        uid: String,
        entry_id: i64,
        seq: u32,
    },
    /// The History page, on this commit.
    Commit(String),
}

impl Row {
    /// The indent of the row: a commit at the edge, a thread under it, its parts under that.
    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Self::Note(_) | Self::Failure(_) | Self::Cut(_) | Self::Commit { .. } => 0,
            Self::Thread { .. } => 1,
            Self::Label { .. } | Self::Link { .. } | Self::Edit { .. } => 2,
            Self::Entry { .. } => 3,
        }
    }

    /// The row as one line of plain text.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Note(text)
            | Self::Failure(text)
            | Self::Cut(text)
            | Self::Commit { text, .. } => text.clone(),
            Self::Thread {
                title,
                harness,
                evidence,
                ambiguous,
                ..
            } => join_parts(&[title, harness], evidence, *ambiguous),
            Self::Label { name, origin } => format!("Label {name} · {origin}"),
            Self::Link {
                relation,
                title,
                origin,
                ..
            } => format!("{relation} {title} · {origin}"),
            Self::Edit {
                path,
                at,
                evidence,
                unverifiable,
                ..
            } => {
                let mut parts = vec![path.as_str(), at.as_str()];
                parts.extend(evidence.iter().copied());
                if let Some(reason) = *unverifiable {
                    parts.push(reason);
                }
                parts.join(" · ")
            }
            Self::Entry { speaker, text } => format!("{speaker}: {text}"),
        }
    }

    /// Where the row leads when clicked: a thread, a commit, or nothing.
    #[must_use]
    pub fn jump(&self) -> Option<Jump> {
        match self {
            Self::Commit { oid, .. } => Some(Jump::Commit(oid.clone())),
            Self::Thread { uid, .. } | Self::Link { uid, .. } => Some(Jump::Thread(uid.clone())),
            Self::Edit {
                uid, entry_id, seq, ..
            } => Some(Jump::Edit {
                uid: uid.clone(),
                entry_id: *entry_id,
                seq: *seq,
            }),
            _ => None,
        }
    }
}

fn join_parts(head: &[&String], evidence: &[&str], ambiguous: bool) -> String {
    let mut parts: Vec<&str> = head.iter().map(|part| part.as_str()).collect();
    parts.extend(evidence.iter().copied());
    if ambiguous {
        parts.push("ambiguous");
    }
    parts.join(" · ")
}

/// The evidence as the docs name it (D144), strongest first, each kind once.
#[must_use]
pub fn evidence_labels(evidence: &[Evidence]) -> Vec<&'static str> {
    let mut sorted = evidence.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.into_iter().map(evidence_text).collect()
}

fn evidence_text(evidence: Evidence) -> &'static str {
    match evidence {
        Evidence::BlobMatch => "blob match",
        Evidence::LineMatch => "line match",
        Evidence::HunkOverlap => "hunk overlap",
        Evidence::ObservedCommit => "created during this thread",
        Evidence::Manual => "linked by you",
    }
}

fn unverifiable_text(reason: Unverifiable) -> &'static str {
    match reason {
        Unverifiable::Redacted => "unverifiable: text redacted",
        Unverifiable::Truncated => "unverifiable: cut at the size limit",
        Unverifiable::FromInput => "unverifiable: From tool input",
    }
}

fn relation_text(kind: ThreadRelKind, direction: RelationDirection) -> &'static str {
    match (kind, direction) {
        (ThreadRelKind::Continues, RelationDirection::Out) => "Continues",
        (ThreadRelKind::Continues, RelationDirection::In) => "Continued by",
        (ThreadRelKind::Relates, _) => "Relates to",
        (ThreadRelKind::Supersedes, RelationDirection::Out) => "Supersedes",
        (ThreadRelKind::Supersedes, RelationDirection::In) => "Superseded by",
    }
}

fn speaker(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "You",
        EntryKind::Assistant => "Assistant",
        EntryKind::ToolCall => "Tool call",
        EntryKind::ToolResult => "Tool result",
        EntryKind::Notice => "Notice",
    }
}

/// Who made a label or a link, as the rows say it: `human` is "by you".
fn origin_text(origin: &str) -> String {
    match origin.strip_prefix("agent:") {
        Some(client) => format!("by agent {}", flatten(client, TEXT_CAP)),
        None if origin == "human" => "by you".to_owned(),
        None => flatten(origin, TEXT_CAP),
    }
}

/// What a bound that cut the result says, and for which commit it was.
#[must_use]
pub fn cut_text(cut: &ProvenanceCut) -> String {
    let limit = cut.limit;
    let commit = cut
        .commit
        .as_deref()
        .map_or_default(|id| format!(" for commit {}", short(id)));
    match cut.bound {
        CutBound::Commits => {
            format!("Partial result: more commits own these lines than the {limit} shown.")
        }
        CutBound::Threads => {
            format!("Partial result: more threads match a commit{commit} than the {limit} shown.")
        }
        CutBound::Candidates => format!(
            "Partial result: more edits touch this path{commit} than the {limit} compared; the rest are not compared."
        ),
        CutBound::Labels => {
            format!("Partial result: more labels are on a thread than the {limit} shown.")
        }
        CutBound::Links => {
            format!("Partial result: more threads are linked to a thread than the {limit} shown.")
        }
    }
}

/// The lines a commit owns, as `12-18, 22`.
fn lines_text(commit: &ProvenanceCommit) -> String {
    commit
        .lines
        .iter()
        .map(|run| {
            if run.from == run.to {
                run.from.to_string()
            } else {
                format!("{}-{}", run.from, run.to)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(oid: &str) -> String {
    oid.chars().take(SHORT_ID).collect()
}

/// Untrusted text as one line: control characters become spaces, runs of spaces collapse,
/// and the result is cut to `cap` characters with an ellipsis.
#[must_use]
pub fn flatten(text: &str, cap: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= cap {
        return joined;
    }
    let mut cut: String = joined.chars().take(cap).collect();
    cut.push('…');
    cut
}

/// The UTC calendar date of `ms` milliseconds since the Unix epoch, as `2024-02-29`.
#[must_use]
pub fn civil_date(ms: i64) -> String {
    // Howard Hinnant's `civil_from_days`, with the day count shifted to 0000-03-01.
    let z = ms.div_euclid(DAY_MS) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::{
        LineRun, ProvenanceEdit, ProvenanceLabel, ProvenanceLink, ThreadEntryBrief,
    };

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    fn empty_answer() -> ProvenanceResult {
        ProvenanceResult {
            path: "src/a.rs".to_owned(),
            from_line: 3,
            to_line: 5,
            commits: Vec::new(),
            cuts: Vec::new(),
        }
    }

    fn thread(uid: &str) -> ProvenanceThread {
        ProvenanceThread {
            uid: uid.to_owned(),
            title: "Fix the parser".to_owned(),
            harness: "claude-code".to_owned(),
            branch: Some("main".to_owned()),
            evidence: vec![Evidence::Manual, Evidence::LineMatch, Evidence::BlobMatch],
            ambiguous: false,
            edits: Vec::new(),
            labels: Vec::new(),
            links: Vec::new(),
        }
    }

    fn commit(threads: Vec<ProvenanceThread>) -> ProvenanceCommit {
        ProvenanceCommit {
            id: OID.to_owned(),
            subject: "Parse the header".to_owned(),
            author: "Gustavo Santos".to_owned(),
            author_at: 1_709_164_800_000,
            path: "src/a.rs".to_owned(),
            lines: vec![LineRun { from: 3, to: 5 }, LineRun { from: 9, to: 9 }],
            threads,
        }
    }

    fn answer(commits: Vec<ProvenanceCommit>) -> ProvenanceResult {
        ProvenanceResult {
            commits,
            ..empty_answer()
        }
    }

    fn answered_panel(provenance: ProvenanceResult) -> ProvenancePanel {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(1));
        assert!(panel.answered(id(1), provenance));
        panel
    }

    #[test]
    fn asking_opens_the_panel_and_shows_the_reading_note_until_the_answer() {
        let mut panel = ProvenancePanel::default();
        assert!(!panel.is_open());
        assert!(panel.rows().is_empty(), "a closed panel shows nothing");
        panel.request("src/a.rs", 3, 5, id(1));
        assert!(panel.is_open() && panel.is_pending() && panel.owns(id(1)));
        assert_eq!(panel.rows(), vec![Row::Note(READING.to_owned())]);
    }

    #[test]
    fn only_the_read_in_flight_is_answered_and_a_newer_question_drops_the_older_one() {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(1));
        panel.request("src/a.rs", 7, 8, id(2));
        assert!(
            !panel.answered(id(1), empty_answer()),
            "a superseded answer is dropped"
        );
        assert!(panel.answered(id(2), empty_answer()));
        assert_eq!(panel.asked().map(|a| (a.from, a.to)), Some((7, 8)));
    }

    #[test]
    fn an_answer_with_no_thread_says_so() {
        let panel = answered_panel(empty_answer());
        assert_eq!(panel.rows(), vec![Row::Note(NO_THREAD.to_owned())]);
        assert_eq!(NO_THREAD, "No thread is linked to these lines");
    }

    #[test]
    fn a_commit_without_threads_shows_its_header_and_the_empty_note() {
        let panel = answered_panel(answer(vec![commit(Vec::new())]));
        let rows = panel.rows();
        assert_eq!(rows[0], Row::Note(NO_THREAD.to_owned()));
        assert_eq!(
            rows[1].line(),
            "0123456 Parse the header · Gustavo Santos · 2024-02-29 · lines 3-5, 9"
        );
    }

    #[test]
    fn a_failed_read_shows_its_reason_and_a_stopped_one_says_so() {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(1));
        panel.ended(id(1), OperationState::Failed, Some("git blame died"));
        assert_eq!(
            panel.rows(),
            vec![Row::Failure("git blame died".to_owned())]
        );

        let mut stopped = ProvenancePanel::default();
        stopped.request("src/a.rs", 3, 5, id(4));
        stopped.ended(id(4), OperationState::Cancelled, None);
        assert_eq!(stopped.rows(), vec![Row::Note(NO_ANSWER.to_owned())]);
    }

    #[test]
    fn a_failure_without_a_reason_says_the_history_could_not_be_read() {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(1));
        panel.ended(id(1), OperationState::Failed, None);
        assert_eq!(panel.rows(), vec![Row::Failure(FAILED.to_owned())]);
    }

    #[test]
    fn an_end_of_a_read_that_is_not_the_one_in_flight_changes_nothing() {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(2));
        panel.ended(id(1), OperationState::Failed, Some("old"));
        assert!(panel.owns(id(2)));
        assert_eq!(panel.rows(), vec![Row::Note(READING.to_owned())]);
    }

    #[test]
    fn a_moved_selection_drops_the_answer_and_says_so() {
        let mut panel = answered_panel(answer(vec![commit(vec![thread("t1")])]));
        assert!(
            !panel.selection_moved(Some(("src/a.rs", 3, 5))),
            "the same range keeps it"
        );
        assert!(panel.selection_moved(Some(("src/a.rs", 3, 6))));
        assert_eq!(panel.rows(), vec![Row::Note(MOVED.to_owned())]);
        assert!(panel.is_open());
    }

    #[test]
    fn a_moved_selection_leaves_the_read_to_finish_but_its_answer_no_longer_shows() {
        let mut panel = ProvenancePanel::default();
        panel.request("src/a.rs", 3, 5, id(1));
        assert!(panel.selection_moved(None));
        assert!(!panel.answered(id(1), empty_answer()));
        assert_eq!(panel.rows(), vec![Row::Note(MOVED.to_owned())]);
    }

    #[test]
    fn a_selection_moved_while_closed_changes_nothing() {
        let mut panel = ProvenancePanel::default();
        assert!(!panel.selection_moved(None));
    }

    #[test]
    fn closing_forgets_the_question_and_the_answer() {
        let mut panel = answered_panel(answer(vec![commit(vec![thread("t1")])]));
        panel.close();
        assert!(!panel.is_open());
        assert!(panel.rows().is_empty());
        assert!(!panel.owns(id(1)));
    }

    #[test]
    fn a_new_question_clears_a_moved_note() {
        let mut panel = answered_panel(empty_answer());
        panel.selection_moved(None);
        panel.request("src/a.rs", 3, 5, id(9));
        assert_eq!(panel.rows(), vec![Row::Note(READING.to_owned())]);
    }

    #[test]
    fn evidence_is_named_as_the_docs_name_it_and_strongest_first() {
        assert_eq!(
            evidence_labels(&[
                Evidence::Manual,
                Evidence::ObservedCommit,
                Evidence::LineMatch,
                Evidence::BlobMatch,
                Evidence::HunkOverlap,
                Evidence::BlobMatch,
            ]),
            vec![
                "blob match",
                "line match",
                "hunk overlap",
                "created during this thread",
                "linked by you",
            ]
        );
        assert!(evidence_labels(&[]).is_empty());
    }

    #[test]
    fn an_unverifiable_edit_names_its_reason_and_has_no_evidence() {
        assert_eq!(
            unverifiable_text(Unverifiable::Redacted),
            "unverifiable: text redacted"
        );
        assert_eq!(
            unverifiable_text(Unverifiable::Truncated),
            "unverifiable: cut at the size limit"
        );
        assert_eq!(
            unverifiable_text(Unverifiable::FromInput),
            "unverifiable: From tool input"
        );
    }

    #[test]
    fn a_thread_row_shows_its_title_harness_evidence_and_the_ambiguous_marker() {
        let mut ambiguous = thread("t1");
        ambiguous.ambiguous = true;
        let panel = answered_panel(answer(vec![commit(vec![ambiguous])]));
        let rows = panel.rows();
        assert_eq!(
            rows[1],
            Row::Thread {
                uid: "t1".to_owned(),
                title: "Fix the parser".to_owned(),
                harness: "claude-code".to_owned(),
                evidence: vec!["blob match", "line match", "linked by you"],
                ambiguous: true,
            }
        );
        assert_eq!(
            rows[1].line(),
            "Fix the parser · claude-code · blob match · line match · linked by you · ambiguous"
        );
    }

    #[test]
    fn labels_links_edits_and_entries_follow_their_thread() {
        let mut busy = thread("t1");
        busy.labels = vec![ProvenanceLabel {
            name: "parser".to_owned(),
            origin: "agent:claude-code".to_owned(),
        }];
        busy.links = vec![ProvenanceLink {
            uid: "t0".to_owned(),
            title: "First pass".to_owned(),
            kind: ThreadRelKind::Continues,
            direction: RelationDirection::In,
            origin: "human".to_owned(),
        }];
        busy.edits = vec![ProvenanceEdit {
            edit_id: 7,
            entry_id: 70,
            seq: 4,
            path: "src/a.rs".to_owned(),
            at: 1_700_000_000_000,
            evidence: vec![Evidence::LineMatch],
            unverifiable: None,
            entries: vec![ThreadEntryBrief {
                seq: 3,
                kind: EntryKind::User,
                text: "Make the header\n\tparse  now".to_owned(),
            }],
        }];
        let panel = answered_panel(answer(vec![commit(vec![busy])]));
        let lines: Vec<String> = panel.rows().iter().map(Row::line).collect();
        assert_eq!(lines[2], "Label parser · by agent claude-code");
        assert_eq!(lines[3], "Continued by First pass · by you");
        assert_eq!(lines[4], "src/a.rs · 2023-11-14 · line match");
        assert_eq!(lines[5], "You: Make the header parse now");
    }

    #[test]
    fn an_edit_row_carries_its_thread_and_a_thread_row_jumps_to_it() {
        let mut busy = thread("t1");
        busy.edits = vec![ProvenanceEdit {
            edit_id: 1,
            entry_id: 10,
            seq: 1,
            path: "src/a.rs".to_owned(),
            at: 0,
            evidence: Vec::new(),
            unverifiable: Some(Unverifiable::Truncated),
            entries: Vec::new(),
        }];
        let panel = answered_panel(answer(vec![commit(vec![busy])]));
        let rows = panel.rows();
        assert_eq!(
            rows[2].jump(),
            Some(Jump::Edit {
                uid: "t1".to_owned(),
                entry_id: 10,
                seq: 1,
            }),
            "an edit row opens the thread focused on the entry that carried the edit"
        );
        assert_eq!(rows[1].jump(), Some(Jump::Thread("t1".to_owned())));
        assert_eq!(rows[0].jump(), Some(Jump::Commit(OID.to_owned())));
        assert_eq!(
            rows[2].line(),
            "src/a.rs · 1970-01-01 · unverifiable: cut at the size limit"
        );
    }

    #[test]
    fn labels_entries_and_notes_do_not_jump() {
        assert_eq!(
            Row::Label {
                name: "x".into(),
                origin: "by you".into()
            }
            .jump(),
            None
        );
        assert_eq!(
            Row::Entry {
                speaker: "You",
                text: "x".into()
            }
            .jump(),
            None
        );
        assert_eq!(Row::Note("x".into()).jump(), None);
    }

    #[test]
    fn a_commit_under_another_path_says_which_path_blame_reports() {
        let mut moved = commit(Vec::new());
        moved.path = "src/old.rs".to_owned();
        let rows = answer_rows(&answer(vec![moved]));
        assert!(
            rows[1]
                .line()
                .ends_with("· lines 3-5, 9 · under src/old.rs")
        );
    }

    #[test]
    fn every_bound_says_what_it_cut_and_where() {
        let cut = |bound, commit: Option<&str>| ProvenanceCut {
            bound,
            limit: 5,
            commit: commit.map(str::to_owned),
        };
        assert_eq!(
            cut_text(&cut(CutBound::Commits, None)),
            "Partial result: more commits own these lines than the 5 shown."
        );
        assert_eq!(
            cut_text(&cut(CutBound::Threads, Some(OID))),
            "Partial result: more threads match a commit for commit 0123456 than the 5 shown."
        );
        assert_eq!(
            cut_text(&cut(CutBound::Candidates, Some(OID))),
            "Partial result: more edits touch this path for commit 0123456 than the 5 compared; the rest are not compared."
        );
        assert_eq!(
            cut_text(&cut(CutBound::Labels, None)),
            "Partial result: more labels are on a thread than the 5 shown."
        );
        assert_eq!(
            cut_text(&cut(CutBound::Links, None)),
            "Partial result: more threads are linked to a thread than the 5 shown."
        );
    }

    #[test]
    fn untrusted_text_is_one_line_and_capped() {
        let long = format!("first\nsecond\u{7}{}", "x".repeat(300));
        let flat = flatten(&long, TEXT_CAP);
        assert!(!flat.contains('\n') && !flat.contains('\u{7}'));
        assert!(flat.starts_with("first second x"));
        assert_eq!(
            flat.chars().count(),
            TEXT_CAP + 1,
            "the cap plus the ellipsis"
        );
        assert!(flat.ends_with('…'));
        assert_eq!(flatten("  a \t b  ", 10), "a b");
    }

    #[test]
    fn a_date_is_the_utc_calendar_day() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(-1), "1969-12-31");
        assert_eq!(civil_date(1_700_000_000_000), "2023-11-14");
        assert_eq!(civil_date(1_709_164_800_000), "2024-02-29");
        assert_eq!(civil_date(951_782_400_000), "2000-02-29");
    }

    #[test]
    fn the_indent_follows_the_nesting() {
        assert_eq!(Row::Note(String::new()).depth(), 0);
        assert_eq!(
            Row::Thread {
                uid: String::new(),
                title: String::new(),
                harness: String::new(),
                evidence: Vec::new(),
                ambiguous: false
            }
            .depth(),
            1
        );
        assert_eq!(
            Row::Entry {
                speaker: "You",
                text: String::new()
            }
            .depth(),
            3
        );
    }
}
