//! The Threads page's model (T5, D119, D125), with no GPUI in it: the threads of
//! the repository read a page at a time, the search, the thread selected and its
//! entries read in pages, the turns those entries group into, and the edits with
//! the state of their files. Each read is sent with a `Read` tag, and an answer is
//! applied only when its `OperationId` is the read in flight for that part of the
//! page. Any other answer is dropped, as the Stashes and Reflog lists drop theirs
//! (B2, B3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gm_schema::{
    CommandKind, EditState, EntryKind, HitKind, LinkKind, LinkOrigin, OperationId, OperationState,
    ThreadDetail, ThreadEdit, ThreadEntry, ThreadFilter, ThreadHit, ThreadLink, ThreadSummary,
};
use serde_json::Value;

use crate::shell_history::relative_time;
use crate::thread_rows::{self, GroupKey, Row};

/// What a copy of a thread says when the engine gave no reason it failed.
const NO_COPY: &str = "The thread could not be copied.";

/// The copy of a thread as Markdown in flight, and its outcome for the window to
/// put on the clipboard. Only the read this names is answered (T6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownCopy {
    pending: Option<OperationId>,
    outcome: Option<Result<String, String>>,
}

impl MarkdownCopy {
    /// Starts the read `id`. A copy in flight is superseded.
    pub fn sent(&mut self, id: OperationId) {
        self.pending = Some(id);
        self.outcome = None;
    }

    /// Whether `id` is the read in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.pending == Some(id)
    }

    /// Keeps the Markdown for the window to copy. Returns false for another read.
    pub fn answered(&mut self, id: OperationId, markdown: String) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.outcome = Some(Ok(markdown));
        true
    }

    /// Keeps the failure of the read `id` in flight, with the engine's message when it
    /// gave one. Returns false for another read.
    pub fn failed(&mut self, id: OperationId, message: Option<String>) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        self.outcome = Some(Err(message.unwrap_or_else(|| NO_COPY.to_owned())));
        true
    }

    /// The read `id` was stopped: nothing is copied and nothing is said. Returns false for another read.
    pub fn cancelled(&mut self, id: OperationId) -> bool {
        if !self.owns(id) {
            return false;
        }
        self.pending = None;
        true
    }

    /// The outcome, once: the Markdown or the failure. The window takes it and acts on it.
    pub fn take(&mut self) -> Option<Result<String, String>> {
        self.outcome.take()
    }
}

/// Threads per page of the list, and hits per search.
pub const PAGE: u32 = 50;
/// Entries per page of a thread.
pub const ENTRY_PAGE: u32 = 200;
/// The longest an argument summary is, in characters, before it is cut.
const SUMMARY_CHARS: usize = 120;
/// The message of a failed read when the engine gave none.
const NO_MESSAGE: &str = "The threads could not be read.";
/// The suffix the Claude Code adapter gives a tool result: `<call>:result`.
const RESULT_SUFFIX: &str = ":result";
/// The opening of a redaction marker, `[REDACTED:name]`.
const REDACTED_OPEN: &str = "[REDACTED:";
/// How many entries before a search hit are read with it, so its turn has its start.
const FOCUS_CONTEXT: u32 = 20;
/// The longest a plain title is, in characters, its ellipsis included.
const TITLE_CHARS: usize = 120;
/// The longest path a file line shows, in characters, before its start is cut.
const PATH_CHARS: usize = 72;
/// The title of a thread with no text to show.
const UNTITLED: &str = "Untitled thread";

/// What the details panel under the header shows. One is open at a time, per thread (T6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Commits,
    Files,
    Note,
}

/// What the list shows: the threads, or the hits of a search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Threads,
    Results,
}

/// What the list says when it has nothing to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Empty {
    /// No thread is stored and no filter is set: the agent integration is not there yet.
    Integration,
    /// No thread matches, and none is stored for this repository.
    Repository,
    /// Threads exist, but the search or the filters match none.
    Matches,
}

/// A filter chip of the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Starred,
    HasEdits,
}

/// What a read was sent for. Its answer is applied only while the read is in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// A page of the list. `append` adds it after the rows shown; `limit` says whether more follow.
    List {
        append: bool,
        limit: u32,
    },
    Search,
    /// The entries of thread `uid` from `from_seq` on, placed as `place` says.
    Detail {
        uid: String,
        from_seq: u32,
        place: Place,
    },
    /// The state of edit `id`.
    Edit {
        id: i64,
    },
}

/// Where a page of a thread's entries goes when its answer arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The window of the thread shown is replaced: the first page, or a read of what is loaded again.
    Replace,
    /// The page follows the entries shown.
    Later,
    /// The page comes before the entries shown.
    Earlier,
}

/// A read to send, with the command and the tag its answer is matched by.
#[derive(Clone, Debug)]
pub struct Request {
    pub kind: CommandKind,
    pub read: Read,
}

/// The state of the Threads page: the list, the search, the selected thread and
/// what the page shows of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadsPage {
    filter: ThreadFilter,
    query: String,
    /// Counts the changes of the search box. A search runs only for the latest one.
    generation: u64,
    threads: Vec<ThreadSummary>,
    /// Every thread in the store, whatever the filter. Known once a list is shown.
    total: Option<u32>,
    listed: bool,
    has_more: bool,
    /// The list read in flight: its id, whether it appends, and its limit.
    list_pending: Option<(OperationId, bool, u32)>,
    hits: Vec<ThreadHit>,
    hits_shown: bool,
    search_pending: Option<OperationId>,
    selected: Option<String>,
    detail: Option<ThreadDetail>,
    /// The entries read in flight, the `from_seq` they start at and where they go.
    detail_pending: Option<(OperationId, u32, Place)>,
    /// The entry a search hit opened the thread at. It is highlighted and scrolled to.
    focus: Option<i64>,
    collapsed: BTreeSet<u32>,
    /// The tool-call groups that are open. A group is closed until it is opened.
    groups: BTreeSet<GroupKey>,
    expanded: BTreeSet<i64>,
    edit_states: HashMap<i64, EditState>,
    /// The state reads in flight, by the edit they ask about.
    edit_reads: Vec<(OperationId, i64)>,
    /// The edit whose patch is open inline.
    open_edit: Option<i64>,
    /// The panel open under the header of each thread, by its uid. Closed until it is opened.
    panels: HashMap<String, Panel>,
    failure: Option<String>,
    /// The rows of the thread shown, and a hash of each: what the list draws.
    rows: Vec<Row>,
    row_keys: Vec<u64>,
}

impl ThreadsPage {
    /// Which list the page shows: the threads, or the hits of a search.
    #[must_use]
    pub fn mode(&self) -> Mode {
        if self.query.trim().is_empty() {
            Mode::Threads
        } else {
            Mode::Results
        }
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether any filter is set. The header's chips are the only ones the page sets.
    #[must_use]
    pub fn filter_active(&self) -> bool {
        self.filter != ThreadFilter::default()
    }

    #[must_use]
    pub fn threads(&self) -> &[ThreadSummary] {
        &self.threads
    }

    #[must_use]
    pub fn hits(&self) -> &[ThreadHit] {
        &self.hits
    }

    /// Every thread in the store, once a list has been shown.
    #[must_use]
    pub fn total(&self) -> Option<u32> {
        self.total
    }

    /// Whether a first page of the list has been shown.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.listed
    }

    #[must_use]
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Whether the answer to the search in flight, or to the last one, is on screen.
    #[must_use]
    pub fn hits_shown(&self) -> bool {
        self.hits_shown
    }

    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn detail(&self) -> Option<&ThreadDetail> {
        self.detail.as_ref()
    }

    #[must_use]
    pub fn is_loading_detail(&self) -> bool {
        self.detail_pending.is_some()
    }

    /// Git's or the engine's message when the newest read failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    #[must_use]
    pub fn focus(&self) -> Option<i64> {
        self.focus
    }

    /// The turn of the focused entry, once that entry is loaded.
    #[must_use]
    pub fn focus_turn(&self) -> Option<u32> {
        let focus = self.focus?;
        let detail = self.detail.as_ref()?;
        detail
            .entries
            .iter()
            .find(|entry| entry.id == focus)
            .map(|entry| entry.turn)
    }

    /// Whether the focused entry is not loaded yet, and more entries follow.
    #[must_use]
    pub fn wants_focus_entries(&self) -> bool {
        let (Some(focus), Some(detail)) = (self.focus, self.detail.as_ref()) else {
            return false;
        };
        detail.has_more
            && self.detail_pending.is_none()
            && !detail.entries.iter().any(|entry| entry.id == focus)
    }

    #[must_use]
    pub fn is_collapsed(&self, turn: u32) -> bool {
        self.collapsed.contains(&turn)
    }

    /// Whether the tool call `entry` shows its arguments and result.
    #[must_use]
    pub fn is_expanded(&self, entry: i64) -> bool {
        self.expanded.contains(&entry)
    }

    /// Whether the tool-call group `key` is open.
    #[must_use]
    pub fn is_group_open(&self, key: &GroupKey) -> bool {
        self.groups.contains(key)
    }

    /// Which page of entries is in flight, if any, and where it goes.
    #[must_use]
    pub fn loading(&self) -> Option<Place> {
        self.detail_pending.map(|(_, _, place)| place)
    }

    /// The rows of the thread shown, flattened for the list (see `thread_rows`).
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// A hash of each row, which the list compares to find the rows that changed.
    #[must_use]
    pub fn row_keys(&self) -> &[u64] {
        &self.row_keys
    }

    /// The edit whose patch is open inline.
    #[must_use]
    pub fn open_edit(&self) -> Option<i64> {
        self.open_edit
    }

    /// The state of edit `id`, once read.
    #[must_use]
    pub fn edit_state(&self, id: i64) -> Option<EditState> {
        self.edit_states.get(&id).copied()
    }

    /// The row of the selected thread (or hit) in the list on screen.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        match self.mode() {
            Mode::Threads => self.threads.iter().position(|row| row.uid == selected),
            Mode::Results => self.hits.iter().position(|hit| hit.thread_uid == selected),
        }
    }

    /// What the list says when it has nothing to show, or `None` while it has rows or waits.
    #[must_use]
    pub fn empty(&self) -> Option<Empty> {
        match self.mode() {
            Mode::Results => (self.hits_shown && self.hits.is_empty()).then_some(Empty::Matches),
            Mode::Threads => {
                if !self.listed || !self.threads.is_empty() {
                    return None;
                }
                let total = self.total.unwrap_or(0);
                Some(match (self.filter_active(), total) {
                    (false, 0) => Empty::Integration,
                    (false, _) | (true, 0) => Empty::Repository,
                    (true, _) => Empty::Matches,
                })
            }
        }
    }

    /// Changes the search text. Returns the generation of the change, or `None`
    /// when the text is the same. A blank text shows the threads again.
    pub fn set_query(&mut self, text: &str) -> Option<u64> {
        if text == self.query {
            return None;
        }
        text.clone_into(&mut self.query);
        self.generation += 1;
        self.search_pending = None;
        self.hits_shown = false;
        if self.query.trim().is_empty() {
            self.hits.clear();
        }
        Some(self.generation)
    }

    /// Whether `generation` is the latest change of the search text.
    #[must_use]
    pub fn is_latest(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// The search for the text as it is, or `None` for a blank text.
    #[must_use]
    pub fn plan_search(&self) -> Option<Request> {
        let query = self.query.trim();
        if query.is_empty() {
            return None;
        }
        Some(Request {
            kind: CommandKind::SearchThreads {
                query: query.to_owned(),
                filter: self.filter.clone(),
                limit: PAGE,
            },
            read: Read::Search,
        })
    }

    /// The list again, from the top, with as many rows as are shown now, so a
    /// reload keeps the pages already read.
    #[must_use]
    pub fn plan_list(&self) -> Request {
        let limit = u32::try_from(self.threads.len())
            .unwrap_or(u32::MAX)
            .max(PAGE);
        Request {
            kind: CommandKind::ListThreads {
                filter: self.filter.clone(),
                before_at: None,
                before_uid: None,
                limit,
            },
            read: Read::List {
                append: false,
                limit,
            },
        }
    }

    /// What a refresh asks for: the list and the search when one is shown. The
    /// selected thread's entries are read again only once the list says its tail
    /// changed (see `plan_tail`), so a change elsewhere does not read them.
    #[must_use]
    pub fn plan_refresh(&self) -> Vec<Request> {
        let mut requests = vec![self.plan_list()];
        requests.extend(self.plan_search());
        requests
    }

    /// The read of the loaded window of the selected thread, from its first entry
    /// on, that replaces what is shown with the same entries as the store has now.
    fn window_request(&self) -> Option<Request> {
        let detail = self.detail.as_ref()?;
        let first = detail.entries.first().map_or(0, |entry| entry.seq);
        let loaded = u32::try_from(detail.entries.len()).unwrap_or(u32::MAX);
        Some(detail_request(
            &detail.summary.uid,
            first,
            loaded.max(ENTRY_PAGE),
            Place::Replace,
        ))
    }

    /// After a page of the list is applied: the selected thread's summary is taken
    /// from its row when the tail of the thread is the same (its entry count, its
    /// last event, its edits and its redactions). The window is read again when the
    /// tail changed or the row is not in the list.
    pub fn plan_tail(&mut self) -> Option<Request> {
        if self.detail_pending.is_some() {
            return None;
        }
        let detail = self.detail.as_ref()?;
        let row = self
            .threads
            .iter()
            .find(|row| row.uid == detail.summary.uid)
            .cloned();
        match row {
            Some(row) if same_tail(&row, &detail.summary) => {
                if let Some(detail) = self.detail.as_mut() {
                    detail.summary = row;
                }
                None
            }
            _ => self.window_request(),
        }
    }

    /// The page of the list after the rows shown, when there is one to ask for and
    /// no read of the list is in flight.
    #[must_use]
    pub fn plan_next(&self) -> Option<Request> {
        if !self.listed || !self.has_more || self.list_pending.is_some() || self.failure.is_some() {
            return None;
        }
        let last = self.threads.last()?;
        Some(Request {
            kind: CommandKind::ListThreads {
                filter: self.filter.clone(),
                before_at: Some(last.last_event_at),
                before_uid: Some(last.uid.clone()),
                limit: PAGE,
            },
            read: Read::List {
                append: true,
                limit: PAGE,
            },
        })
    }

    /// The entries after the ones shown, of the selected thread.
    #[must_use]
    pub fn plan_more(&self) -> Option<Request> {
        let detail = self.detail.as_ref()?;
        if !detail.has_more || self.detail_pending.is_some() {
            return None;
        }
        let from_seq = detail.entries.last()?.seq.checked_add(1)?;
        Some(detail_request(
            &detail.summary.uid,
            from_seq,
            ENTRY_PAGE,
            Place::Later,
        ))
    }

    /// The entries before the ones shown, when the thread's first entry shown is not
    /// its first. The page is at most `ENTRY_PAGE` entries, and ends where the shown ones start.
    #[must_use]
    pub fn plan_earlier(&self) -> Option<Request> {
        let detail = self.detail.as_ref()?;
        if self.detail_pending.is_some() {
            return None;
        }
        let first = detail.entries.first()?.seq;
        if first == 0 {
            return None;
        }
        let from_seq = first.saturating_sub(ENTRY_PAGE);
        Some(detail_request(
            &detail.summary.uid,
            from_seq,
            first - from_seq,
            Place::Earlier,
        ))
    }

    /// The state reads for the edits of the thread shown that have none yet.
    #[must_use]
    pub fn plan_edit_states(&self) -> Vec<Request> {
        let Some(detail) = self.detail.as_ref() else {
            return Vec::new();
        };
        detail
            .edits
            .iter()
            .filter(|edit| {
                !self.edit_states.contains_key(&edit.id)
                    && !self.edit_reads.iter().any(|(_, id)| *id == edit.id)
            })
            .map(|edit| Request {
                kind: CommandKind::ReadThreadEdit { id: edit.id },
                read: Read::Edit { id: edit.id },
            })
            .collect()
    }

    /// Records that `read` was sent as operation `id`. A read of the same part
    /// replaces the one in flight, which its late answer then does not match.
    pub fn sent(&mut self, read: &Read, id: OperationId) {
        match read {
            Read::List { append, limit } => {
                self.failure = None;
                self.list_pending = Some((id, *append, *limit));
            }
            Read::Search => {
                self.failure = None;
                self.search_pending = Some(id);
                self.hits_shown = false;
            }
            Read::Detail {
                from_seq, place, ..
            } => {
                self.failure = None;
                self.detail_pending = Some((id, *from_seq, *place));
                self.refresh_rows();
            }
            Read::Edit { id: edit } => self.edit_reads.push((id, *edit)),
        }
    }

    /// Whether `id` is one of the reads in flight.
    #[must_use]
    pub fn owns(&self, id: OperationId) -> bool {
        self.list_pending
            .is_some_and(|(pending, _, _)| pending == id)
            || self.search_pending == Some(id)
            || self
                .detail_pending
                .is_some_and(|(pending, _, _)| pending == id)
            || self.edit_reads.iter().any(|(pending, _)| *pending == id)
    }

    /// Applies a page of the list answered by `id`. Returns false for an answer
    /// that is not the read in flight.
    pub fn listed(&mut self, id: OperationId, threads: Vec<ThreadSummary>, total: u32) -> bool {
        let Some((pending, append, limit)) = self.list_pending else {
            return false;
        };
        if pending != id {
            return false;
        }
        self.list_pending = None;
        self.has_more = u32::try_from(threads.len()).is_ok_and(|len| len >= limit);
        if append {
            self.threads.extend(threads);
        } else {
            self.threads = threads;
        }
        self.total = Some(total);
        self.listed = true;
        true
    }

    /// Applies the hits of a search answered by `id`. Returns false for an answer
    /// that is not the search in flight.
    pub fn searched(&mut self, id: OperationId, hits: Vec<ThreadHit>) -> bool {
        if self.search_pending != Some(id) {
            return false;
        }
        self.search_pending = None;
        self.hits = hits;
        self.hits_shown = true;
        true
    }

    /// Applies the entries answered by `id`, as the read's `place` says: the window
    /// shown is replaced, or the page is appended after the entries shown or put
    /// before them. Returns false for an answer that does not apply. The read in
    /// flight is over once its answer arrives, whether or not it applies.
    pub fn read(&mut self, id: OperationId, detail: ThreadDetail) -> bool {
        let Some((pending, from_seq, place)) = self.detail_pending else {
            return false;
        };
        if pending != id || self.selected.as_deref() != Some(detail.summary.uid.as_str()) {
            return false;
        }
        self.detail_pending = None;
        let applied = match place {
            Place::Replace => {
                self.edit_states
                    .retain(|edit, _| detail.edits.iter().any(|shown| shown.id == *edit));
                self.detail = Some(detail);
                true
            }
            Place::Later => match self.detail.as_mut() {
                Some(current)
                    if current
                        .entries
                        .last()
                        .map(|entry| entry.seq.saturating_add(1))
                        == Some(from_seq) =>
                {
                    current.entries.extend(detail.entries);
                    current.edits.extend(detail.edits);
                    current.summary = detail.summary;
                    current.links = detail.links;
                    current.redactions = detail.redactions;
                    current.has_more = detail.has_more;
                    true
                }
                _ => false,
            },
            Place::Earlier => match self.detail.as_mut() {
                Some(current)
                    if detail
                        .entries
                        .last()
                        .map(|entry| entry.seq.saturating_add(1))
                        == current.entries.first().map(|entry| entry.seq) =>
                {
                    let mut entries = detail.entries;
                    entries.append(&mut current.entries);
                    current.entries = entries;
                    current.edits.extend(detail.edits);
                    current.summary = detail.summary;
                    current.links = detail.links;
                    current.redactions = detail.redactions;
                    true
                }
                _ => false,
            },
        };
        if applied {
            self.open_focus_group();
            self.refresh_rows();
        }
        applied
    }

    /// Applies the state of an edit answered by `id`, when the edit is in the thread shown.
    pub fn edit_read(&mut self, id: OperationId, edit: i64, state: EditState) -> bool {
        let Some(position) = self
            .edit_reads
            .iter()
            .position(|(pending, _)| *pending == id)
        else {
            return false;
        };
        self.edit_reads.swap_remove(position);
        let shown = self
            .detail
            .as_ref()
            .is_some_and(|detail| detail.edits.iter().any(|shown| shown.id == edit));
        if shown {
            self.edit_states.insert(edit, state);
            self.refresh_rows();
        }
        true
    }

    /// A read `id` ended without an answer. A failure is kept for the page to
    /// show; a stopped read leaves the page as it was.
    pub fn ended(&mut self, id: OperationId, state: OperationState, detail: Option<String>) {
        if !self.owns(id) || !state.is_terminal() {
            return;
        }
        if self
            .list_pending
            .is_some_and(|(pending, _, _)| pending == id)
        {
            self.list_pending = None;
        }
        if self.search_pending == Some(id) {
            self.search_pending = None;
        }
        if self
            .detail_pending
            .is_some_and(|(pending, _, _)| pending == id)
        {
            self.detail_pending = None;
            self.refresh_rows();
        }
        self.edit_reads.retain(|(pending, _)| *pending != id);
        if state == OperationState::Failed {
            self.failure = Some(detail.unwrap_or_else(|| NO_MESSAGE.to_owned()));
        }
    }

    /// Selects thread `uid`, optionally at a search hit: its entry id and its `seq`.
    /// Returns the read of its entries, or `None` when nothing is to be read. A hit
    /// reads from the entries before it, so its turn has its start.
    pub fn select(&mut self, uid: &str, focus: Option<(i64, u32)>) -> Option<Request> {
        if self.selected.as_deref() == Some(uid) {
            let (entry, seq) = focus?;
            self.focus = Some(entry);
            self.open_focus_group();
            self.refresh_rows();
            let loaded = self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.entries.iter().any(|shown| shown.id == entry));
            if loaded || self.detail_pending.is_some() || self.detail.is_none() {
                return None;
            }
            return Some(detail_request(
                uid,
                seq.saturating_sub(FOCUS_CONTEXT),
                ENTRY_PAGE,
                Place::Replace,
            ));
        }
        self.selected = Some(uid.to_owned());
        self.clear_selection();
        self.focus = focus.map(|(entry, _)| entry);
        let from_seq = focus.map_or(0, |(_, seq)| seq.saturating_sub(FOCUS_CONTEXT));
        self.refresh_rows();
        Some(detail_request(uid, from_seq, ENTRY_PAGE, Place::Replace))
    }

    /// Drops what the page shows of the selected thread, not the selection itself.
    fn clear_selection(&mut self) {
        self.detail = None;
        self.detail_pending = None;
        self.collapsed.clear();
        self.groups.clear();
        self.expanded.clear();
        self.edit_states.clear();
        self.edit_reads.clear();
        self.open_edit = None;
    }

    /// The file the list is filtered to: the threads that touched this path.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.filter.path.as_deref()
    }

    /// Sets or clears the file filter. Returns whether it changed; the caller reads the list again.
    pub fn set_path(&mut self, path: Option<String>) -> bool {
        if self.filter.path == path {
            return false;
        }
        self.filter.path = path;
        true
    }

    /// The summary of thread `uid`, from the thread shown or from the list.
    #[must_use]
    pub fn summary(&self, uid: &str) -> Option<&ThreadSummary> {
        self.detail
            .as_ref()
            .map(|detail| &detail.summary)
            .filter(|summary| summary.uid == uid)
            .or_else(|| self.threads.iter().find(|thread| thread.uid == uid))
    }

    /// The summary of the selected thread, when it is known.
    #[must_use]
    pub fn selected_summary(&self) -> Option<&ThreadSummary> {
        self.summary(self.selected.as_deref()?)
    }

    /// Drops the selection of thread `uid` when it is selected. Used when the
    /// thread is deleted, so no read of it is sent after the delete.
    pub fn forget(&mut self, uid: &str) {
        self.panels.remove(uid);
        if self.selected.as_deref() == Some(uid) {
            self.selected = None;
            self.focus = None;
            self.clear_selection();
            self.refresh_rows();
        }
    }

    /// Selects the row `index` of the list shown: a thread, or the thread of a hit,
    /// which opens at the hit's entry.
    pub fn choose(&mut self, index: usize) -> Option<Request> {
        match self.mode() {
            Mode::Threads => {
                let uid = self.threads.get(index)?.uid.clone();
                self.select(&uid, None)
            }
            Mode::Results => {
                let hit = self.hits.get(index)?.clone();
                self.select(&hit.thread_uid, Some((hit.entry_id, hit.seq)))
            }
        }
    }

    /// Moves the selection by `delta` rows of the list shown, and selects the row.
    /// With no selection, down picks the first row and up the last.
    pub fn step(&mut self, delta: isize) -> Option<Request> {
        let rows: Vec<&str> = match self.mode() {
            Mode::Threads => self.threads.iter().map(|row| row.uid.as_str()).collect(),
            Mode::Results => self
                .hits
                .iter()
                .map(|hit| hit.thread_uid.as_str())
                .collect(),
        };
        let last = rows.len().checked_sub(1)?;
        let current = self
            .selected
            .as_deref()
            .and_then(|selected| rows.iter().position(|row| *row == selected));
        let next = match current {
            Some(index) => index.saturating_add_signed(delta).min(last),
            None if delta > 0 => 0,
            None => last,
        };
        self.choose(next)
    }

    /// Changes a filter chip. The list is read again from the top, and the search too.
    pub fn toggle(&mut self, chip: Chip) -> Vec<Request> {
        let flag = match chip {
            Chip::Starred => &mut self.filter.starred,
            Chip::HasEdits => &mut self.filter.has_edits,
        };
        *flag = if *flag == Some(true) {
            None
        } else {
            Some(true)
        };
        let mut requests = vec![self.plan_list()];
        requests.extend(self.plan_search());
        requests
    }

    /// Whether the chip is set.
    #[must_use]
    pub fn chip_on(&self, chip: Chip) -> bool {
        match chip {
            Chip::Starred => self.filter.starred == Some(true),
            Chip::HasEdits => self.filter.has_edits == Some(true),
        }
    }

    /// Opens or closes a turn's entries.
    pub fn toggle_turn(&mut self, turn: u32) {
        if !self.collapsed.remove(&turn) {
            self.collapsed.insert(turn);
        }
        self.refresh_rows();
    }

    /// Opens or closes a tool call's arguments and result.
    pub fn toggle_call(&mut self, entry: i64) {
        if !self.expanded.remove(&entry) {
            self.expanded.insert(entry);
        }
        self.refresh_rows();
    }

    /// Opens or closes a tool-call group, which is closed until it is opened.
    pub fn toggle_group(&mut self, key: &GroupKey) {
        if !self.groups.remove(key) {
            self.groups.insert(key.clone());
        }
        self.refresh_rows();
    }

    /// Opens every tool-call group of the thread shown (Expand all).
    pub fn expand_all_groups(&mut self) {
        if let Some(detail) = self.detail.as_ref() {
            self.groups = thread_rows::group_keys(detail).into_iter().collect();
        }
        self.refresh_rows();
    }

    /// Closes every tool-call group of the thread shown (Collapse all).
    pub fn collapse_all_groups(&mut self) {
        self.groups.clear();
        self.refresh_rows();
    }

    /// Opens the patch of an edit, or closes it when it is open.
    pub fn toggle_diff(&mut self, edit: i64) {
        self.open_edit = if self.open_edit == Some(edit) {
            None
        } else {
            Some(edit)
        };
        self.refresh_rows();
    }

    /// The panel open under the header of the thread selected, if one is.
    #[must_use]
    pub fn open_panel(&self) -> Option<Panel> {
        let uid = self.selected.as_deref()?;
        self.panels.get(uid).copied()
    }

    /// Opens `panel` under the header of the thread selected, or closes it when it is
    /// the panel open. Another thread's panel is not touched.
    pub fn toggle_panel(&mut self, panel: Panel) {
        let Some(uid) = self.selected.clone() else {
            return;
        };
        if self.panels.get(&uid) == Some(&panel) {
            self.panels.remove(&uid);
        } else {
            self.panels.insert(uid, panel);
        }
    }

    /// Opens the turn and the tool-call group that hold the focused entry, so a
    /// search hit is not hidden under a closed one.
    fn open_focus_group(&mut self) {
        let (Some(focus), Some(detail)) = (self.focus, self.detail.as_ref()) else {
            return;
        };
        if let Some(turn) = self.focus_turn() {
            self.collapsed.remove(&turn);
        }
        if let Some(key) = thread_rows::group_of(detail, focus) {
            self.groups.insert(key);
        }
    }

    /// Flattens the thread shown into its rows, and keeps a hash of each.
    fn refresh_rows(&mut self) {
        let rows = thread_rows::flatten(self);
        self.row_keys = rows.iter().map(thread_rows::key_of).collect();
        self.rows = rows;
    }
}

/// Whether a summary of the list and the summary of the thread shown agree on the
/// parts that change with the entries: the counts and the last event.
fn same_tail(listed: &ThreadSummary, shown: &ThreadSummary) -> bool {
    listed.entry_count == shown.entry_count
        && listed.edit_count == shown.edit_count
        && listed.redaction_count == shown.redaction_count
        && listed.last_event_at == shown.last_event_at
        && listed.ended_at == shown.ended_at
}

/// The read of thread `uid`'s entries from `from_seq`, at most `limit` of them.
fn detail_request(uid: &str, from_seq: u32, limit: u32, place: Place) -> Request {
    Request {
        kind: CommandKind::ReadThread {
            uid: uid.to_owned(),
            from_seq,
            limit,
        },
        read: Read::Detail {
            uid: uid.to_owned(),
            from_seq,
            place,
        },
    }
}

/// The header's count: "1 thread", "12 threads", or "First N threads" while more follow.
#[must_use]
pub fn count_label(count: usize, more: bool) -> String {
    if more {
        format!("First {count} threads")
    } else if count == 1 {
        "1 thread".to_owned()
    } else {
        format!("{count} threads")
    }
}

/// The text of an empty list.
#[must_use]
pub fn empty_text(empty: Empty) -> &'static str {
    match empty {
        Empty::Integration => {
            "No threads yet. Install the agent integration to capture your agent sessions."
        }
        Empty::Repository => "No threads in this repository yet.",
        Empty::Matches => "No threads match.",
    }
}

/// The command that installs the integration, shown under the empty state.
pub const INSTALL_HINT: &str = "gitmaster harness install claude-code";

/// A thread's title as plain text, or a placeholder when the first prompt had none.
#[must_use]
pub fn title(summary: &ThreadSummary) -> String {
    plain_title(&summary.title)
}

/// The label of a search hit that a fallback stage found (D131): `approx.` for a substring
/// or a fuzzy match, nothing for a word-start match.
#[must_use]
pub fn approximate_label(kind: HitKind) -> Option<&'static str> {
    match kind {
        HitKind::Exact => None,
        HitKind::Substring | HitKind::Fuzzy => Some("approx."),
    }
}

/// A title as plain text, from the first non-empty line of `text`: the Markdown
/// markers at its start (`#`, `>`, list bullets, task boxes) and the emphasis,
/// code and link syntax in it are dropped, the whitespace is collapsed, and the
/// result is cut to 120 characters. Text with nothing left is `Untitled thread`.
#[must_use]
pub fn plain_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let plain = strip_inline(strip_markers(line));
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        UNTITLED.to_owned()
    } else {
        cut_end(&collapsed, TITLE_CHARS)
    }
}

/// `text` without the block markers at its start, repeated until none is left: a
/// heading's `#`s, a quote's `>`, a bullet, an ordered number, a task box.
fn strip_markers(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        let before = rest;
        let hashes = rest.len() - rest.trim_start_matches('#').len();
        if hashes > 0 && (rest.len() == hashes || rest[hashes..].starts_with(' ')) {
            rest = rest[hashes..].trim_start();
        } else if let Some(after) = rest.strip_prefix('>') {
            rest = after.trim_start();
        } else if let Some(after) = strip_list_marker(rest) {
            rest = after.trim_start();
        }
        if rest.len() == before.len() {
            return rest;
        }
    }
}

/// The text after a bullet or an ordered number, and after a task box that follows it.
fn strip_list_marker(text: &str) -> Option<&str> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(after) = text.strip_prefix(bullet) {
            return Some(task_box(after));
        }
    }
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let after = &text[digits..];
    after
        .strip_prefix(". ")
        .or_else(|| after.strip_prefix(") "))
        .map(task_box)
}

/// The text after a task box, `[ ]` or `[x]`, when one starts `text`.
fn task_box(text: &str) -> &str {
    for item in ["[ ]", "[x]", "[X]"] {
        if let Some(after) = text.strip_prefix(item)
            && (after.is_empty() || after.starts_with(' '))
        {
            return after;
        }
    }
    text
}

/// `text` with each `[label](url)` replaced by its label, and its emphasis and code
/// marks (`**`, `__`, backticks) removed.
fn strip_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|at| open + at) else {
            break;
        };
        let Some(end) = rest[close + 2..].find(')').map(|at| close + 2 + at) else {
            break;
        };
        let label = &rest[open + 1..close];
        if label.contains('[') {
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
            continue;
        }
        out.push_str(&rest[..open]);
        out.push_str(label);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

/// `text`, cut to `max` characters with an ellipsis at its end when it is longer.
fn cut_end(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// `text`, cut at its start to `max` characters with an ellipsis before it when it is
/// longer, so the end of a path, its file name, stays on screen.
fn cut_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let kept: Vec<char> = text.chars().skip(count - max.saturating_sub(1)).collect();
    format!("…{}", kept.iter().collect::<String>())
}

/// `path` as the panel shows it: relative to the worktree when it is under it, `~`
/// for a folder under `home`, and cut at its start past 72 characters.
#[must_use]
pub fn display_path(path: &str, worktree: &str, home: &str) -> String {
    let shown = if let Some(rest) = strip_dir(path, worktree) {
        rest.to_owned()
    } else if let Some(rest) = strip_dir(path, home) {
        format!("~/{rest}")
    } else {
        path.to_owned()
    };
    cut_start(&shown, PATH_CHARS)
}

/// The part of `path` after the folder `dir`, when `path` is inside it.
fn strip_dir<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        return None;
    }
    path.strip_prefix(dir)?.strip_prefix('/')
}

/// One file of a thread's details panel: its path, the lines its edits added and
/// removed, and the manual link that names it, if the user made one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLine {
    pub path: String,
    /// Whether an edit of the thread changed the file, so its line counts are known.
    pub edited: bool,
    pub added: u32,
    pub removed: u32,
    pub manual: Option<ThreadLink>,
}

/// The files of a thread: each edited path once, with the lines of all its edits,
/// in the order they were first edited, then the files linked by hand that no edit names.
#[must_use]
pub fn file_lines(detail: &ThreadDetail) -> Vec<FileLine> {
    let mut lines: Vec<FileLine> = Vec::new();
    for edit in &detail.edits {
        match lines.iter_mut().find(|line| line.path == edit.path) {
            Some(line) => {
                line.added = line.added.saturating_add(edit.added);
                line.removed = line.removed.saturating_add(edit.removed);
            }
            None => lines.push(FileLine {
                path: edit.path.clone(),
                edited: true,
                added: edit.added,
                removed: edit.removed,
                manual: None,
            }),
        }
    }
    for link in &detail.links {
        if link.kind != LinkKind::File || link.origin != LinkOrigin::Manual {
            continue;
        }
        match lines.iter_mut().find(|line| line.path == link.reference) {
            Some(line) => line.manual = Some(link.clone()),
            None => lines.push(FileLine {
                path: link.reference.clone(),
                edited: false,
                added: 0,
                removed: 0,
                manual: Some(link.clone()),
            }),
        }
    }
    lines
}

/// The line of a thread's header under its title: the harness, the branch, when it started,
/// whether it is open, and the tags. Cut by the view when the header is narrow.
#[must_use]
pub fn header_meta(summary: &ThreadSummary, now_secs: i64) -> String {
    let branch = summary.branch.as_deref().unwrap_or("detached HEAD");
    let mut text = format!(
        "{} · {branch} · started {}",
        summary.harness,
        relative(now_secs, summary.started_at)
    );
    if summary.ended_at.is_none() {
        text.push_str(" · open");
    }
    if !summary.tags.is_empty() {
        text.push_str(" · tags: ");
        text.push_str(&summary.tags.join(", "));
    }
    text
}

/// A chip's count: `1 commit`, `3 commits`.
#[must_use]
pub fn chip_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// The counts of a thread's header chips: its commit links, its files and its edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderCounts {
    pub commits: usize,
    pub files: usize,
    pub edits: usize,
}

/// The header's counts of a thread. An edit counts once, a file once per path.
#[must_use]
pub fn header_counts(detail: &ThreadDetail) -> HeaderCounts {
    HeaderCounts {
        commits: detail
            .links
            .iter()
            .filter(|link| link.kind == LinkKind::Commit)
            .count(),
        files: file_lines(detail).len(),
        edits: detail.edits.len(),
    }
}

/// How long ago an event happened, from the current time in seconds and a time in milliseconds.
#[must_use]
pub fn relative(now_secs: i64, at_ms: i64) -> String {
    relative_time(now_secs, at_ms.div_euclid(1000))
}

/// The header's count of search hits: "1 match", "3 matches".
#[must_use]
pub fn hits_label(count: usize) -> String {
    if count == 1 {
        "1 match".to_owned()
    } else {
        format!("{count} matches")
    }
}

/// The number of edits: "1 edit", "3 edits".
#[must_use]
pub fn edits_label(count: u32) -> String {
    if count == 1 {
        "1 edit".to_owned()
    } else {
        format!("{count} edits")
    }
}

/// The added and removed line counts of an edit, as `+3 −1`.
#[must_use]
pub fn counts_label(added: u32, removed: u32) -> String {
    format!("+{added} −{removed}")
}

/// The state chip of an edit: how the file stands now. An edit the pipeline could
/// not measure says so, and an edit not read yet says it is being checked.
#[must_use]
pub fn edit_state_label(edit: &ThreadEdit, state: Option<EditState>) -> &'static str {
    if edit.from_input {
        return "From tool input";
    }
    match state {
        None => "Checking…",
        Some(EditState::InWorkingTree) => "In working tree",
        Some(EditState::InHead) => "In HEAD",
        Some(EditState::ChangedSince) => "Changed since",
        Some(EditState::Unknown) => "Unknown",
    }
}

/// A duration as the call line shows it: `120 ms`, `2.5 s`, `3 min 4 s`.
#[must_use]
pub fn duration_label(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{}.{} s", ms / 1_000, (ms % 1_000) / 100)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

/// The first `max_lines` lines of `text`, and how many lines were left out.
#[must_use]
pub fn head(text: &str, max_lines: usize) -> (&str, usize) {
    let total = text.lines().count();
    if total <= max_lines {
        return (text, 0);
    }
    let end = text
        .match_indices('\n')
        .nth(max_lines.saturating_sub(1))
        .map_or(text.len(), |(index, _)| index);
    (&text[..end], total - max_lines)
}

/// One line of a patch, and how the view colours it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    Added,
    Removed,
    Hunk,
    Context,
}

/// The lines of a unified diff, classed for the colours of the diff view.
#[must_use]
pub fn patch_lines(patch: &str) -> Vec<(PatchKind, &str)> {
    patch
        .lines()
        .map(|line| {
            let kind = if line.starts_with("@@") {
                PatchKind::Hunk
            } else if line.starts_with("+++")
                || line.starts_with("---")
                || line.starts_with("diff ")
                || line.starts_with("index ")
            {
                PatchKind::Context
            } else if line.starts_with('+') {
                PatchKind::Added
            } else if line.starts_with('-') {
                PatchKind::Removed
            } else {
                PatchKind::Context
            };
            (kind, line)
        })
        .collect()
}

/// A piece of a text: plain words, or a redaction marker that the view paints as a chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    /// The pattern name inside `[REDACTED:name]`.
    Redacted(String),
}

/// Splits a text at its redaction markers. Adjacent markers stay separate spans, and
/// a bracket that is not a complete marker stays text.
#[must_use]
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(REDACTED_OPEN) {
        let after = &rest[at + REDACTED_OPEN.len()..];
        match after.find(']') {
            Some(end) if end > 0 && !after[..end].contains('[') => {
                plain.push_str(&rest[..at]);
                flush(&mut out, &mut plain);
                out.push(Span::Redacted(after[..end].to_owned()));
                rest = &after[end + 1..];
            }
            _ => {
                plain.push_str(&rest[..at + REDACTED_OPEN.len()]);
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    flush(&mut out, &mut plain);
    out
}

fn flush(out: &mut Vec<Span>, plain: &mut String) {
    if !plain.is_empty() {
        out.push(Span::Text(std::mem::take(plain)));
    }
}

/// The argument of a tool call, as one short line: its command, path, pattern, URL
/// or description, or else its first text value.
#[must_use]
pub fn argument_summary(args_json: Option<&str>) -> String {
    const KEYS: [&str; 6] = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ];
    let Some(value) = args_json.and_then(|json| serde_json::from_str::<Value>(json).ok()) else {
        return String::new();
    };
    let text = KEYS
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .or_else(|| value.as_object()?.values().find_map(Value::as_str));
    text.map_or_else(String::new, |text| one_line(text, SUMMARY_CHARS))
}

/// The text collapsed to one line of at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// How a tool call ended, as its row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallStatus {
    Done,
    Failed,
    /// No result has been recorded: the call was interrupted, or its result is on a later page.
    Pending,
}

/// The result of a tool call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallResult {
    pub text: String,
    /// Whether the entry was cut at the capture limit.
    pub truncated: bool,
}

/// A tool call of a turn, with its result, its edits and the calls its sub-agent made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The entry id of the call: what the state and the expansion are keyed by.
    pub entry_id: i64,
    /// The harness's id of the call, which a tool-call group is keyed by.
    pub source_id: String,
    pub tool: String,
    pub summary: String,
    /// The arguments as stored, redacted.
    pub args: Option<String>,
    pub result: Option<CallResult>,
    pub status: CallStatus,
    pub duration_ms: Option<i64>,
    pub edits: Vec<ThreadEdit>,
    pub children: Vec<Block>,
}

/// The kind of a text entry of a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageKind {
    Assistant,
    Notice,
    /// A tool result whose call is not in the turn shown.
    Result,
}

/// A text entry of a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub entry_id: i64,
    pub kind: MessageKind,
    pub text: String,
}

/// One row of a turn: a text, or a tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Message(Message),
    Call(Call),
}

/// One prompt of the human and everything the agent did until it stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub number: u32,
    pub prompt: Option<String>,
    /// The entry id of the prompt, which a search hit can name.
    pub prompt_id: Option<i64>,
    pub blocks: Vec<Block>,
    /// Edits of this turn whose call is not in it.
    pub edits: Vec<ThreadEdit>,
}

/// Groups the entries of a thread by turn, in order. Each tool result joins the call
/// whose `<call>:result` id it has; a sub-agent's entries go under the call that
/// started them; and each edit goes under the call entry that carried it.
#[must_use]
pub fn turns(detail: &ThreadDetail) -> Vec<Turn> {
    let mut by_turn: BTreeMap<u32, Vec<&ThreadEntry>> = BTreeMap::new();
    for entry in &detail.entries {
        by_turn.entry(entry.turn).or_default().push(entry);
    }
    by_turn
        .into_iter()
        .map(|(number, mut entries)| {
            entries.sort_by_key(|entry| entry.seq);
            build_turn(number, &entries, &detail.edits)
        })
        .collect()
}

/// A block before its sub-agent entries are placed under it.
struct Node {
    /// The source id that other entries name as their parent.
    source: String,
    parent: Option<String>,
    block: Block,
}

fn build_turn(number: u32, entries: &[&ThreadEntry], edits: &[ThreadEdit]) -> Turn {
    let prompt_entry = entries.iter().find(|entry| entry.kind == EntryKind::User);
    let prompt = prompt_entry.map(|entry| entry.text.clone());
    let prompt_id = prompt_entry.map(|entry| entry.id);
    let calls: HashSet<&str> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::ToolCall)
        .map(|entry| entry.source_id.as_str())
        .collect();
    let results: HashMap<&str, &ThreadEntry> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::ToolResult)
        .filter_map(|entry| {
            let call = entry.source_id.strip_suffix(RESULT_SUFFIX)?;
            calls.contains(call).then_some((call, *entry))
        })
        .collect();
    let paired: HashSet<i64> = results.values().map(|result| result.id).collect();
    let call_entries: HashSet<i64> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::ToolCall)
        .map(|entry| entry.id)
        .collect();
    let turn_entries: HashSet<i64> = entries.iter().map(|entry| entry.id).collect();
    let attached_edits = |entry: i64| -> Vec<ThreadEdit> {
        edits
            .iter()
            .filter(|edit| edit.entry_id == entry)
            .cloned()
            .collect()
    };

    let mut nodes: Vec<Node> = Vec::new();
    for entry in entries {
        let block = match entry.kind {
            EntryKind::User => continue,
            EntryKind::ToolResult if paired.contains(&entry.id) => continue,
            EntryKind::ToolResult => Block::Message(Message {
                entry_id: entry.id,
                kind: MessageKind::Result,
                text: entry.text.clone(),
            }),
            EntryKind::Assistant => Block::Message(Message {
                entry_id: entry.id,
                kind: MessageKind::Assistant,
                text: entry.text.clone(),
            }),
            EntryKind::Notice => Block::Message(Message {
                entry_id: entry.id,
                kind: MessageKind::Notice,
                text: entry.text.clone(),
            }),
            EntryKind::ToolCall => {
                let result = results.get(entry.source_id.as_str()).copied();
                let duration_ms = result
                    .map(|result| result.at - entry.at)
                    .filter(|d| *d >= 0);
                let status = match result {
                    None => CallStatus::Pending,
                    Some(result) if result.is_error => CallStatus::Failed,
                    Some(_) => CallStatus::Done,
                };
                Block::Call(Call {
                    entry_id: entry.id,
                    source_id: entry.source_id.clone(),
                    tool: entry.tool.clone().unwrap_or_default(),
                    summary: argument_summary(entry.args_json.as_deref()),
                    args: entry.args_json.clone(),
                    result: result.map(|result| CallResult {
                        text: result.text.clone(),
                        truncated: result.truncated,
                    }),
                    status,
                    duration_ms,
                    edits: attached_edits(entry.id),
                    children: Vec::new(),
                })
            }
        };
        nodes.push(Node {
            source: entry.source_id.clone(),
            parent: entry.parent_id.clone(),
            block,
        });
    }

    let placed = place(nodes);
    let turn_edits = edits
        .iter()
        .filter(|edit| {
            turn_entries.contains(&edit.entry_id) && !call_entries.contains(&edit.entry_id)
        })
        .cloned()
        .collect();
    Turn {
        number,
        prompt,
        prompt_id,
        blocks: placed,
        edits: turn_edits,
    }
}

/// Puts each block under the tool call its parent names, and returns the blocks
/// that have no parent in the turn, in order. Every block has one parent at most,
/// so the walk from the roots reaches each block once.
fn place(nodes: Vec<Node>) -> Vec<Block> {
    let (roots, children) = {
        let index: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.block, Block::Call(_)))
            .map(|(position, node)| (node.source.as_str(), position))
            .collect();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        for (position, node) in nodes.iter().enumerate() {
            match node.parent.as_deref().and_then(|parent| index.get(parent)) {
                Some(&parent) if parent != position => children[parent].push(position),
                _ => roots.push(position),
            }
        }
        (roots, children)
    };
    let mut blocks: Vec<Option<Block>> = nodes.into_iter().map(|node| Some(node.block)).collect();
    roots
        .into_iter()
        .filter_map(|root| assemble(root, &mut blocks, &children))
        .collect()
}

fn assemble(
    position: usize,
    blocks: &mut [Option<Block>],
    children: &[Vec<usize>],
) -> Option<Block> {
    let mut block = blocks.get_mut(position)?.take()?;
    if let Block::Call(call) = &mut block {
        call.children = children[position]
            .iter()
            .filter_map(|&child| assemble(child, blocks, children))
            .collect();
    }
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{EditChange, ThreadLink};

    /// The entry id a block is keyed by.
    fn entry_id_of(block: &Block) -> i64 {
        match block {
            Block::Message(message) => message.entry_id,
            Block::Call(call) => call.entry_id,
        }
    }

    fn page_len() -> usize {
        usize::try_from(PAGE).unwrap_or(usize::MAX)
    }

    fn summary(uid: &str, last: i64) -> ThreadSummary {
        ThreadSummary {
            uid: uid.to_owned(),
            harness: "claude-code".to_owned(),
            harness_version: None,
            session_id: format!("s-{uid}"),
            worktree: "/repo".to_owned(),
            branch: Some("main".to_owned()),
            model: None,
            title: format!("Thread {uid}"),
            note: String::new(),
            starred: false,
            tags: Vec::new(),
            started_at: last - 10,
            last_event_at: last,
            ended_at: None,
            entry_count: 0,
            edit_count: 0,
            redaction_count: 0,
        }
    }

    fn rows(uids: &[&str]) -> Vec<ThreadSummary> {
        uids.iter()
            .enumerate()
            .map(|(i, uid)| summary(uid, 1_000 - i64::try_from(i).unwrap_or(0)))
            .collect()
    }

    fn entry(id: i64, seq: u32, turn: u32, kind: EntryKind, source: &str) -> ThreadEntry {
        ThreadEntry {
            id,
            seq,
            turn,
            kind,
            source_id: source.to_owned(),
            parent_id: None,
            tool: None,
            text: String::new(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            at: 1_000 * i64::from(seq),
        }
    }

    fn tool_call(
        id: i64,
        seq: u32,
        turn: u32,
        source: &str,
        tool: &str,
        args: &str,
    ) -> ThreadEntry {
        ThreadEntry {
            tool: Some(tool.to_owned()),
            args_json: Some(args.to_owned()),
            ..entry(id, seq, turn, EntryKind::ToolCall, source)
        }
    }

    fn result(id: i64, seq: u32, turn: u32, call: &str, text: &str, error: bool) -> ThreadEntry {
        ThreadEntry {
            text: text.to_owned(),
            is_error: error,
            ..entry(
                id,
                seq,
                turn,
                EntryKind::ToolResult,
                &format!("{call}:result"),
            )
        }
    }

    fn with_text(mut entry: ThreadEntry, text: &str) -> ThreadEntry {
        entry.text = text.to_owned();
        entry
    }

    fn edit(id: i64, entry_id: i64, path: &str) -> ThreadEdit {
        ThreadEdit {
            id,
            entry_id,
            path: path.to_owned(),
            old_path: None,
            change: EditChange::Modify,
            patch: "@@ -1 +1 @@\n-a\n+b".to_owned(),
            added: 1,
            removed: 1,
            pre_oid: None,
            post_oid: None,
            from_input: false,
            at: 1,
        }
    }

    fn detail(
        uid: &str,
        entries: Vec<ThreadEntry>,
        edits: Vec<ThreadEdit>,
        more: bool,
    ) -> ThreadDetail {
        ThreadDetail {
            summary: summary(uid, 1_000),
            entries,
            edits,
            links: Vec::<ThreadLink>::new(),
            redactions: Vec::new(),
            has_more: more,
        }
    }

    fn hit(uid: &str, entry_id: i64) -> ThreadHit {
        ThreadHit {
            thread_uid: uid.to_owned(),
            title: format!("Thread {uid}"),
            entry_id,
            seq: 1,
            turn: 1,
            snippet: "retry".to_owned(),
            at: 1,
            kind: HitKind::Exact,
        }
    }

    #[test]
    fn a_fallback_hit_is_marked_approximate_and_the_page_keeps_each_kind() {
        assert_eq!(approximate_label(HitKind::Exact), None);
        assert_eq!(approximate_label(HitKind::Substring), Some("approx."));
        assert_eq!(approximate_label(HitKind::Fuzzy), Some("approx."));
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        page.set_query("bakcoff");
        let search = page.plan_search().expect("a search");
        page.sent(&search.read, id(2));
        let fuzzy = ThreadHit {
            kind: HitKind::Fuzzy,
            ..hit("a", 3)
        };
        assert!(page.searched(id(2), vec![hit("a", 1), fuzzy]));
        let kinds: Vec<HitKind> = page.hits().iter().map(|hit| hit.kind).collect();
        assert_eq!(kinds, vec![HitKind::Exact, HitKind::Fuzzy]);
    }

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    /// A page with `uids` listed under operation `op`, as the engine answers it.
    fn listed_page(page: &mut ThreadsPage, op: u64, uids: &[&str], total: u32) {
        let request = page.plan_list();
        page.sent(&request.read, id(op));
        assert!(page.listed(id(op), rows(uids), total));
    }

    fn selected_page(page: &mut ThreadsPage, uid: &str, entries: Vec<ThreadEntry>, more: bool) {
        page.select(uid, None);
        page.sent(
            &Read::Detail {
                uid: uid.to_owned(),
                from_seq: 0,
                place: Place::Replace,
            },
            id(50),
        );
        assert!(page.read(id(50), detail(uid, entries, Vec::new(), more)));
    }

    fn read_uid(request: &Request) -> Option<&str> {
        match &request.kind {
            CommandKind::ReadThread { uid, .. } => Some(uid.as_str()),
            _ => None,
        }
    }

    #[test]
    fn the_first_page_is_shown_when_its_read_answers_and_the_total_is_known() {
        let mut page = ThreadsPage::default();
        assert!(!page.is_loaded());
        assert_eq!(page.total(), None);
        let request = page.plan_list();
        assert!(matches!(
            request.kind,
            CommandKind::ListThreads {
                before_at: None,
                before_uid: None,
                limit: PAGE,
                ..
            }
        ));
        page.sent(&request.read, id(3));
        assert!(!page.is_loaded());
        assert!(page.listed(id(3), rows(&["a", "b"]), 7));
        assert!(page.is_loaded());
        assert_eq!(page.threads().len(), 2);
        assert_eq!(page.total(), Some(7));
        assert!(!page.has_more(), "a short page is the last one");
    }

    #[test]
    fn a_full_page_says_more_follow_and_the_next_page_starts_after_the_last_row() {
        let mut page = ThreadsPage::default();
        let uids: Vec<String> = (0..PAGE).map(|i| format!("t{i:02}")).collect();
        let refs: Vec<&str> = uids.iter().map(String::as_str).collect();
        listed_page(&mut page, 1, &refs, 120);
        assert!(page.has_more());
        let next = page.plan_next().expect("a next page");
        match &next.kind {
            CommandKind::ListThreads {
                before_at,
                before_uid,
                ..
            } => {
                let last = page.threads().last().expect("rows");
                assert_eq!(*before_at, Some(last.last_event_at));
                assert_eq!(before_uid.as_deref(), Some(last.uid.as_str()));
            }
            other => panic!("not a list: {other:?}"),
        }
        page.sent(&next.read, id(2));
        assert!(
            page.plan_next().is_none(),
            "nothing is asked while a page is in flight"
        );
        assert!(page.listed(id(2), rows(&["z1"]), 120));
        assert_eq!(page.threads().len(), page_len() + 1);
        assert!(!page.has_more());
        assert!(page.plan_next().is_none());
    }

    #[test]
    fn a_page_answer_that_is_not_the_read_in_flight_is_dropped() {
        let mut page = ThreadsPage::default();
        let first = page.plan_list();
        page.sent(&first.read, id(3));
        let second = page.plan_list();
        page.sent(&second.read, id(5));
        assert!(!page.listed(id(3), rows(&["old"]), 1));
        assert!(!page.is_loaded());
        assert!(page.listed(id(5), rows(&["new"]), 1));
        assert_eq!(page.threads()[0].uid, "new");
    }

    #[test]
    fn a_next_page_answer_arriving_after_a_refresh_is_dropped() {
        let mut page = ThreadsPage::default();
        let uids: Vec<String> = (0..PAGE).map(|i| format!("t{i:02}")).collect();
        let refs: Vec<&str> = uids.iter().map(String::as_str).collect();
        listed_page(&mut page, 1, &refs, 80);
        let next = page.plan_next().expect("next");
        page.sent(&next.read, id(2));
        let refresh = page.plan_list();
        page.sent(&refresh.read, id(3));
        assert!(
            !page.listed(id(2), rows(&["late"]), 80),
            "the page is stale"
        );
        assert_eq!(page.threads().len(), page_len());
        assert!(page.listed(id(3), rows(&refs), 80));
        assert_eq!(
            page.threads().len(),
            page_len(),
            "the refresh keeps the rows loaded"
        );
    }

    #[test]
    fn a_refresh_asks_for_as_many_rows_as_are_shown() {
        let mut page = ThreadsPage::default();
        let uids: Vec<String> = (0..60).map(|i| format!("t{i:02}")).collect();
        let refs: Vec<&str> = uids.iter().map(String::as_str).collect();
        listed_page(&mut page, 1, &refs, 60);
        match page.plan_list().kind {
            CommandKind::ListThreads { limit, .. } => assert_eq!(limit, 60),
            other => panic!("not a list: {other:?}"),
        }
    }

    #[test]
    fn a_failed_read_keeps_the_rows_and_shows_the_engines_message() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        let next = page.plan_list();
        page.sent(&next.read, id(4));
        page.ended(
            id(4),
            OperationState::Failed,
            Some("fatal: database is locked".into()),
        );
        assert_eq!(page.failure(), Some("fatal: database is locked"));
        assert_eq!(page.threads().len(), 1);
        assert!(!page.owns(id(4)));
        assert_eq!(page.failure(), Some("fatal: database is locked"));
        // A failure with no message says the threads could not be read.
        let again = page.plan_list();
        page.sent(&again.read, id(5));
        page.ended(id(5), OperationState::Failed, None);
        assert_eq!(page.failure(), Some(NO_MESSAGE));
        // A read that was cancelled leaves nothing behind.
        let cancelled = page.plan_list();
        page.sent(&cancelled.read, id(6));
        page.ended(id(6), OperationState::Cancelled, None);
        assert!(!page.owns(id(6)));
    }

    #[test]
    fn a_failure_of_a_read_not_in_flight_is_ignored() {
        let mut page = ThreadsPage::default();
        let request = page.plan_list();
        page.sent(&request.read, id(9));
        page.ended(id(8), OperationState::Failed, Some("old".into()));
        assert_eq!(page.failure(), None);
        assert!(page.owns(id(9)));
    }

    #[test]
    fn the_search_runs_only_for_a_non_blank_text_and_only_its_latest_answer_applies() {
        let mut page = ThreadsPage::default();
        assert_eq!(page.mode(), Mode::Threads);
        assert!(page.set_query("   ").is_some());
        assert_eq!(page.mode(), Mode::Threads, "a blank text is the list");
        assert!(page.plan_search().is_none());
        let first = page.set_query("retry").expect("changed");
        let second = page.set_query("retry back").expect("changed");
        assert!(!page.is_latest(first));
        assert!(page.is_latest(second));
        assert_eq!(
            page.set_query("retry back"),
            None,
            "the same text changes nothing"
        );
        assert_eq!(page.mode(), Mode::Results);
        let search = page.plan_search().expect("a search");
        match &search.kind {
            CommandKind::SearchThreads { query, limit, .. } => {
                assert_eq!(query, "retry back");
                assert_eq!(*limit, PAGE);
            }
            other => panic!("not a search: {other:?}"),
        }
        page.sent(&search.read, id(7));
        assert!(!page.hits_shown());
        assert!(
            !page.searched(id(6), vec![hit("a", 1)]),
            "an older search is dropped"
        );
        assert!(page.searched(id(7), vec![hit("a", 1)]));
        assert_eq!(page.hits().len(), 1);
        assert!(page.hits_shown());
    }

    #[test]
    fn clearing_the_search_drops_its_hits_and_its_answer() {
        let mut page = ThreadsPage::default();
        page.set_query("x");
        let search = page.plan_search().expect("search");
        page.sent(&search.read, id(2));
        page.set_query("");
        assert!(page.hits().is_empty());
        assert!(!page.searched(id(2), vec![hit("a", 1)]));
        assert_eq!(page.mode(), Mode::Threads);
    }

    #[test]
    fn a_thread_is_selected_once_and_its_entries_read_at_zero() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        let request = page.choose(1).expect("selects b");
        assert_eq!(read_uid(&request), Some("b"));
        assert_eq!(page.selected(), Some("b"));
        assert!(
            page.select("b", None).is_none(),
            "the same thread is not read again"
        );
        assert_eq!(page.selected_row(), Some(1));
    }

    #[test]
    fn a_selection_superseded_by_another_drops_its_entries_answer() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        let first = page.choose(0).expect("a");
        page.sent(&first.read, id(10));
        let second = page.choose(1).expect("b");
        page.sent(&second.read, id(11));
        assert!(!page.read(
            id(10),
            detail(
                "a",
                vec![entry(1, 0, 1, EntryKind::User, "u")],
                Vec::new(),
                false
            )
        ));
        assert!(page.detail().is_none());
        assert!(page.read(
            id(11),
            detail(
                "b",
                vec![entry(2, 0, 1, EntryKind::User, "u")],
                Vec::new(),
                false
            )
        ));
        assert_eq!(page.detail().map(|d| d.summary.uid.as_str()), Some("b"));
    }

    #[test]
    fn a_later_page_of_entries_appends_when_it_starts_where_the_shown_entries_end() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        selected_page(
            &mut page,
            "a",
            vec![
                entry(1, 0, 1, EntryKind::User, "u"),
                entry(2, 1, 1, EntryKind::Assistant, "x"),
            ],
            true,
        );
        let more = page.plan_more().expect("more entries");
        match &more.kind {
            CommandKind::ReadThread {
                from_seq, limit, ..
            } => {
                assert_eq!(*from_seq, 2);
                assert_eq!(*limit, ENTRY_PAGE);
            }
            other => panic!("not a read: {other:?}"),
        }
        page.sent(&more.read, id(60));
        assert!(page.plan_more().is_none(), "one page at a time");
        assert!(
            !page.read(
                id(59),
                detail(
                    "a",
                    vec![entry(3, 2, 1, EntryKind::Assistant, "y")],
                    Vec::new(),
                    false
                )
            ),
            "an older answer is dropped"
        );
        assert!(page.read(
            id(60),
            detail(
                "a",
                vec![entry(3, 2, 1, EntryKind::Assistant, "y")],
                Vec::new(),
                false
            )
        ));
        let detail = page.detail().expect("shown");
        assert_eq!(detail.entries.len(), 3);
        assert!(!detail.has_more);
        assert!(page.plan_more().is_none());
    }

    #[test]
    fn the_focused_entry_of_a_search_hit_is_loaded_before_its_turn_is_known() {
        let mut page = ThreadsPage::default();
        page.set_query("retry");
        let search = page.plan_search().expect("search");
        page.sent(&search.read, id(1));
        assert!(page.searched(id(1), vec![hit("a", 99)]));
        let request = page.choose(0).expect("opens the hit's thread");
        assert_eq!(read_uid(&request), Some("a"));
        page.sent(&request.read, id(2));
        assert_eq!(page.focus(), Some(99));
        assert!(page.read(
            id(2),
            detail(
                "a",
                vec![entry(1, 0, 1, EntryKind::User, "u")],
                Vec::new(),
                true
            )
        ));
        assert!(
            page.wants_focus_entries(),
            "the hit's entry is not loaded and more follow"
        );
        assert_eq!(page.focus_turn(), None);
        assert!(page.plan_more().is_some());
    }

    #[test]
    fn a_refresh_keeps_the_selection_and_reads_the_entries_only_when_the_tail_changed() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        let request = page.choose(1).expect("b");
        page.sent(&request.read, id(2));
        assert!(page.read(
            id(2),
            detail(
                "b",
                vec![entry(1, 0, 1, EntryKind::User, "u")],
                Vec::new(),
                false
            )
        ));
        page.toggle_turn(1);
        page.toggle_call(7);
        let refresh = page.plan_refresh();
        assert_eq!(
            refresh.len(),
            1,
            "the list only: the entries wait for its tail"
        );
        page.sent(&refresh[0].read, id(3));
        assert!(page.listed(id(3), rows(&["b", "a", "c"]), 3));
        assert!(
            page.plan_tail().is_none(),
            "the tail is the same, so nothing is read again"
        );
        assert_eq!(page.selected(), Some("b"));
        assert_eq!(page.selected_row(), Some(0));
        assert!(page.is_collapsed(1), "the collapse state survives");
        assert!(page.is_expanded(7), "the expansion survives");

        // A new entry changes the tail: the window is read again, and the view keeps its state.
        let mut changed = rows(&["b", "a", "c"]);
        changed[0].entry_count = 2;
        changed[0].last_event_at = 2_000;
        let refresh = page.plan_refresh();
        page.sent(&refresh[0].read, id(4));
        assert!(page.listed(id(4), changed, 3));
        let request = page.plan_tail().expect("the window is read again");
        assert!(matches!(
            request.read,
            Read::Detail {
                from_seq: 0,
                place: Place::Replace,
                ..
            }
        ));
        page.sent(&request.read, id(5));
        assert!(page.read(
            id(5),
            detail(
                "b",
                vec![
                    entry(1, 0, 1, EntryKind::User, "u"),
                    entry(2, 1, 1, EntryKind::Assistant, "x"),
                ],
                Vec::new(),
                false
            )
        ));
        assert!(page.is_collapsed(1));
        assert!(page.is_expanded(7));
        assert_eq!(page.detail().map(|d| d.entries.len()), Some(2));
    }

    #[test]
    fn a_thread_that_is_no_longer_listed_keeps_its_selection_after_a_refresh() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        page.choose(1);
        let list = page.plan_list();
        page.sent(&list.read, id(2));
        assert!(page.listed(id(2), rows(&["a"]), 1));
        assert_eq!(page.selected(), Some("b"));
        assert_eq!(page.selected_row(), None);
    }

    #[test]
    fn stepping_moves_the_selection_through_the_rows_and_stops_at_the_ends() {
        let mut page = ThreadsPage::default();
        assert!(page.step(1).is_none(), "no rows");
        listed_page(&mut page, 1, &["a", "b", "c"], 3);
        let down = page.step(1).expect("first row");
        assert_eq!(read_uid(&down), Some("a"));
        page.sent(&down.read, id(2));
        let down = page.step(1).expect("second row");
        assert_eq!(read_uid(&down), Some("b"));
        page.sent(&down.read, id(3));
        assert!(
            page.step(-5).is_some(),
            "up past the top selects the first row"
        );
        assert_eq!(page.selected(), Some("a"));
        assert!(page.step(-1).is_none(), "already at the top");
        page.select("c", None);
        assert!(page.step(9).is_none(), "already at the bottom");
        assert_eq!(page.selected(), Some("c"));
    }

    #[test]
    fn up_from_no_selection_picks_the_last_row() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        let up = page.step(-1).expect("last");
        assert_eq!(read_uid(&up), Some("b"));
    }

    #[test]
    fn a_hit_chosen_in_the_results_opens_its_thread_at_the_entry() {
        let mut page = ThreadsPage::default();
        page.set_query("q");
        let search = page.plan_search().expect("search");
        page.sent(&search.read, id(1));
        page.searched(id(1), vec![hit("a", 4), hit("b", 8)]);
        let request = page.choose(1).expect("b");
        assert_eq!(read_uid(&request), Some("b"));
        assert_eq!(page.focus(), Some(8));
        assert_eq!(page.selected_row(), Some(1));
        assert!(page.step(1).is_none(), "already at the last hit");
        assert_eq!(page.selected(), Some("b"));
    }

    #[test]
    fn a_filter_chip_reads_the_list_again_and_the_search_too_when_one_is_shown() {
        let mut page = ThreadsPage::default();
        let requests = page.toggle(Chip::Starred);
        assert_eq!(requests.len(), 1, "no search while the text is blank");
        assert!(page.chip_on(Chip::Starred));
        match &requests[0].kind {
            CommandKind::ListThreads { filter, .. } => assert_eq!(filter.starred, Some(true)),
            other => panic!("not a list: {other:?}"),
        }
        assert!(page.filter_active());
        page.set_query("x");
        let requests = page.toggle(Chip::HasEdits);
        assert_eq!(requests.len(), 2, "the list and the search");
        assert!(page.chip_on(Chip::HasEdits));
        let requests = page.toggle(Chip::Starred);
        assert!(!page.chip_on(Chip::Starred));
        assert_eq!(requests.len(), 2);
        page.toggle(Chip::HasEdits);
        assert!(!page.filter_active(), "both chips off is no filter");
    }

    #[test]
    fn the_empty_states_name_the_missing_integration_the_empty_repository_and_no_match() {
        let mut page = ThreadsPage::default();
        assert_eq!(page.empty(), None, "nothing is known before a listing");
        listed_page(&mut page, 1, &[], 0);
        assert_eq!(page.empty(), Some(Empty::Integration));
        assert!(
            empty_text(Empty::Integration)
                .starts_with("No threads yet. Install the agent integration")
        );
        // With a filter set, an empty store is a list with nothing in it.
        let mut filtered = ThreadsPage::default();
        filtered.toggle(Chip::Starred);
        let request = filtered.plan_list();
        filtered.sent(&request.read, id(2));
        filtered.listed(id(2), Vec::new(), 0);
        assert_eq!(filtered.empty(), Some(Empty::Repository));
        // Threads exist, the filter matches none.
        let mut none = ThreadsPage::default();
        none.toggle(Chip::HasEdits);
        let request = none.plan_list();
        none.sent(&request.read, id(3));
        none.listed(id(3), Vec::new(), 5);
        assert_eq!(none.empty(), Some(Empty::Matches));
        assert_eq!(empty_text(Empty::Matches), "No threads match.");
        assert_eq!(
            empty_text(Empty::Repository),
            "No threads in this repository yet."
        );
        // A repository with threads and no filter shows rows, so it has no empty state.
        let mut full = ThreadsPage::default();
        listed_page(&mut full, 4, &["a"], 1);
        assert_eq!(full.empty(), None);
    }

    #[test]
    fn a_search_with_no_hit_says_no_match_once_answered() {
        let mut page = ThreadsPage::default();
        page.set_query("zzz");
        assert_eq!(page.empty(), None, "no answer yet");
        let search = page.plan_search().expect("search");
        page.sent(&search.read, id(1));
        page.searched(id(1), Vec::new());
        assert_eq!(page.empty(), Some(Empty::Matches));
    }

    #[test]
    fn the_thread_count_follows_the_store_and_the_header_says_when_more_follow() {
        assert_eq!(count_label(1, false), "1 thread");
        assert_eq!(count_label(0, false), "0 threads");
        assert_eq!(count_label(50, true), "First 50 threads");
    }

    #[test]
    fn a_read_of_an_edit_state_is_applied_only_to_an_edit_shown() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        selected_page(
            &mut page,
            "a",
            vec![entry(1, 0, 1, EntryKind::ToolCall, "c")],
            false,
        );
        // The detail from `selected_page` has no edits; add one through a paged read.
        page.detail
            .as_mut()
            .expect("shown")
            .edits
            .push(edit(70, 1, "src/a.rs"));
        let requests = page.plan_edit_states();
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            requests[0].kind,
            CommandKind::ReadThreadEdit { id: 70 }
        ));
        page.sent(&requests[0].read, id(20));
        assert!(
            page.plan_edit_states().is_empty(),
            "one read in flight per edit"
        );
        assert!(
            !page.edit_read(id(21), 70, EditState::InHead),
            "another operation"
        );
        assert!(page.edit_read(id(20), 70, EditState::InHead));
        assert_eq!(page.edit_state(70), Some(EditState::InHead));
        assert!(page.plan_edit_states().is_empty());
        // A state for an edit that is no longer shown is dropped.
        page.detail.as_mut().expect("shown").edits.clear();
        page.sent(&Read::Edit { id: 71 }, id(22));
        assert!(page.edit_read(id(22), 71, EditState::ChangedSince));
        assert_eq!(page.edit_state(71), None);
    }

    #[test]
    fn an_edit_label_reads_from_the_file_or_says_it_came_from_the_tool_input() {
        let measured = edit(1, 1, "a.rs");
        assert_eq!(edit_state_label(&measured, None), "Checking…");
        assert_eq!(
            edit_state_label(&measured, Some(EditState::InWorkingTree)),
            "In working tree"
        );
        assert_eq!(
            edit_state_label(&measured, Some(EditState::InHead)),
            "In HEAD"
        );
        assert_eq!(
            edit_state_label(&measured, Some(EditState::ChangedSince)),
            "Changed since"
        );
        assert_eq!(
            edit_state_label(&measured, Some(EditState::Unknown)),
            "Unknown"
        );
        let from_input = ThreadEdit {
            from_input: true,
            ..measured
        };
        assert_eq!(
            edit_state_label(&from_input, Some(EditState::InHead)),
            "From tool input"
        );
        assert_eq!(counts_label(3, 1), "+3 −1");
        assert_eq!(edits_label(1), "1 edit");
        assert_eq!(edits_label(4), "4 edits");
    }

    #[test]
    fn an_edit_patch_opens_and_closes_by_its_button() {
        let mut page = ThreadsPage::default();
        page.toggle_diff(5);
        assert_eq!(page.open_edit(), Some(5));
        page.toggle_diff(6);
        assert_eq!(page.open_edit(), Some(6), "one patch is open at a time");
        page.toggle_diff(6);
        assert_eq!(page.open_edit(), None);
    }

    #[test]
    fn a_turn_and_a_call_open_and_close_by_their_click() {
        let mut page = ThreadsPage::default();
        page.toggle_turn(2);
        assert!(page.is_collapsed(2));
        page.toggle_turn(2);
        assert!(!page.is_collapsed(2));
        page.toggle_call(9);
        assert!(page.is_expanded(9));
        page.toggle_call(9);
        assert!(!page.is_expanded(9));
    }

    #[test]
    fn the_redaction_markers_split_the_text_and_adjacent_markers_stay_apart() {
        assert_eq!(
            spans("token [REDACTED:github-token] in use"),
            vec![
                Span::Text("token ".into()),
                Span::Redacted("github-token".into()),
                Span::Text(" in use".into()),
            ]
        );
        assert_eq!(
            spans("[REDACTED:a][REDACTED:b]"),
            vec![Span::Redacted("a".into()), Span::Redacted("b".into())]
        );
        assert_eq!(spans("plain text"), vec![Span::Text("plain text".into())]);
        assert!(spans("").is_empty());
        // Not a marker: an empty name, an unclosed one, or a bracket inside the name.
        assert_eq!(spans("[REDACTED:]"), vec![Span::Text("[REDACTED:]".into())]);
        assert_eq!(
            spans("[REDACTED:open and [REDACTED:ok]"),
            vec![
                Span::Text("[REDACTED:open and ".into()),
                Span::Redacted("ok".into())
            ]
        );
        assert_eq!(
            spans("x[REDACTED:y"),
            vec![Span::Text("x[REDACTED:y".into())]
        );
    }

    #[test]
    fn a_marker_at_the_start_and_at_the_end_leaves_no_empty_text() {
        assert_eq!(spans("[REDACTED:key]"), vec![Span::Redacted("key".into())]);
        assert_eq!(
            spans("a[REDACTED:key]"),
            vec![Span::Text("a".into()), Span::Redacted("key".into())]
        );
    }

    #[test]
    fn a_tool_call_line_names_its_argument_and_cuts_a_long_one() {
        assert_eq!(
            argument_summary(Some(r#"{"command":"cargo  test\n -p gm-ui"}"#)),
            "cargo test -p gm-ui"
        );
        assert_eq!(
            argument_summary(Some(r#"{"file_path":"src/lib.rs","limit":5}"#)),
            "src/lib.rs"
        );
        assert_eq!(
            argument_summary(Some(r#"{"limit":5,"query":"find me"}"#)),
            "find me"
        );
        assert_eq!(argument_summary(None), "");
        assert_eq!(argument_summary(Some("not json")), "");
        let long = format!(r#"{{"command":"{}"}}"#, "x".repeat(300));
        let line = argument_summary(Some(&long));
        assert_eq!(line.chars().count(), SUMMARY_CHARS);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn durations_read_in_ms_seconds_and_minutes() {
        assert_eq!(duration_label(120), "120 ms");
        assert_eq!(duration_label(2_540), "2.5 s");
        assert_eq!(duration_label(184_000), "3 min 4 s");
        assert_eq!(duration_label(-5), "0 ms");
    }

    #[test]
    fn the_head_of_a_long_text_is_the_first_lines_and_the_rest_is_counted() {
        assert_eq!(head("a\nb", 5), ("a\nb", 0));
        assert_eq!(head("a\nb\nc", 2), ("a\nb", 1));
        assert_eq!(head("a\nb\nc\nd", 1), ("a", 3));
    }

    #[test]
    fn a_patch_is_classed_line_by_line() {
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same";
        let kinds: Vec<PatchKind> = patch_lines(patch)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                PatchKind::Context,
                PatchKind::Context,
                PatchKind::Context,
                PatchKind::Hunk,
                PatchKind::Removed,
                PatchKind::Added,
                PatchKind::Context,
            ]
        );
    }

    #[test]
    fn the_relative_time_reads_the_milliseconds_of_the_store() {
        assert_eq!(relative(1_000, 1_000_000), "just now");
        assert_eq!(relative(1_000 + 3_600, 1_000_000), "1 h ago");
    }

    #[test]
    fn a_turn_groups_its_prompt_text_and_calls_and_pairs_each_result_with_its_call() {
        let entries = vec![
            with_text(entry(1, 0, 1, EntryKind::User, "u1"), "p1"),
            with_text(entry(2, 1, 1, EntryKind::Assistant, "a1"), "I will look."),
            tool_call(3, 2, 1, "t1", "Read", r#"{"file_path":"src/a.rs"}"#),
            result(4, 3, 1, "t1", "fn main() {}", false),
            tool_call(5, 4, 1, "t2", "Bash", r#"{"command":"cargo test"}"#),
            result(6, 5, 1, "t2", "failed", true),
            with_text(entry(7, 6, 2, EntryKind::User, "u2"), "p2"),
            with_text(entry(8, 7, 2, EntryKind::Assistant, "done"), "done"),
        ];
        let turns = turns(&detail("a", entries, Vec::new(), false));
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].number, 1);
        assert_eq!(turns[0].prompt.as_deref(), Some("p1"));
        assert_eq!(turns[0].blocks.len(), 3, "text, read, bash");
        match &turns[0].blocks[1] {
            Block::Call(call) => {
                assert_eq!(call.tool, "Read");
                assert_eq!(call.summary, "src/a.rs");
                assert_eq!(call.status, CallStatus::Done);
                assert_eq!(
                    call.result.as_ref().map(|r| r.text.as_str()),
                    Some("fn main() {}")
                );
                assert_eq!(call.duration_ms, Some(1_000));
            }
            other @ Block::Message(_) => panic!("not a call: {other:?}"),
        }
        match &turns[0].blocks[2] {
            Block::Call(call) => {
                assert_eq!(call.status, CallStatus::Failed);
                assert_eq!(call.summary, "cargo test");
            }
            other @ Block::Message(_) => panic!("not a call: {other:?}"),
        }
        assert_eq!(turns[1].prompt.as_deref(), Some("p2"));
        assert_eq!(turns[1].blocks.len(), 1);
    }

    #[test]
    fn a_call_without_a_result_is_pending_and_an_orphan_result_is_a_message() {
        let entries = vec![
            entry(1, 0, 1, EntryKind::User, "p"),
            tool_call(2, 1, 1, "t1", "Bash", r#"{"command":"sleep 9"}"#),
            // The result of a call that is in no turn shown.
            result(3, 2, 1, "t0", "orphan output", false),
        ];
        let turns = turns(&detail("a", entries, Vec::new(), false));
        let blocks = &turns[0].blocks;
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::Call(call) => {
                assert_eq!(call.status, CallStatus::Pending);
                assert!(call.result.is_none());
                assert_eq!(call.duration_ms, None);
            }
            other @ Block::Message(_) => panic!("not a call: {other:?}"),
        }
        match &blocks[1] {
            Block::Message(message) => {
                assert_eq!(message.kind, MessageKind::Result);
                assert_eq!(message.text, "orphan output");
            }
            other @ Block::Call(_) => panic!("not a message: {other:?}"),
        }
    }

    #[test]
    fn a_sub_agent_call_nests_the_entries_it_started_under_its_agent_call() {
        let agent = tool_call(2, 1, 1, "agent-1", "Agent", r#"{"description":"survey"}"#);
        let mut inner = tool_call(3, 2, 1, "inner-1", "Grep", r#"{"pattern":"retry"}"#);
        inner.parent_id = Some("agent-1".to_owned());
        let mut inner_text = with_text(
            entry(4, 3, 1, EntryKind::Assistant, "inner-text"),
            "found it",
        );
        inner_text.parent_id = Some("agent-1".to_owned());
        let mut deepest = tool_call(5, 4, 1, "deep-1", "Read", r#"{"file_path":"x.rs"}"#);
        deepest.parent_id = Some("inner-1".to_owned());
        let entries = vec![
            entry(1, 0, 1, EntryKind::User, "p"),
            agent,
            inner,
            inner_text,
            deepest,
            result(6, 5, 1, "agent-1", "survey done", false),
        ];
        let turns = turns(&detail("a", entries, Vec::new(), false));
        let blocks = &turns[0].blocks;
        assert_eq!(blocks.len(), 1, "only the agent call is at the top");
        let Block::Call(agent) = &blocks[0] else {
            panic!("not a call");
        };
        assert_eq!(agent.status, CallStatus::Done);
        assert_eq!(agent.children.len(), 2);
        let Block::Call(inner) = &agent.children[0] else {
            panic!("the inner call comes first");
        };
        assert_eq!(inner.summary, "retry");
        assert_eq!(
            inner.children.len(),
            1,
            "the call of the call nests one level further"
        );
        assert_eq!(entry_id_of(&inner.children[0]), 5);
        assert_eq!(entry_id_of(&agent.children[1]), 4);
    }

    #[test]
    fn an_edit_sits_under_the_call_that_carried_it_and_one_of_no_call_under_the_turn() {
        let entries = vec![
            entry(1, 0, 1, EntryKind::User, "p"),
            tool_call(2, 1, 1, "t1", "Edit", r#"{"file_path":"a.rs"}"#),
            result(3, 2, 1, "t1", "ok", false),
        ];
        let edits = vec![edit(10, 2, "a.rs"), edit(11, 99, "gone.rs")];
        let turns = turns(&detail("a", entries, edits, false));
        let Block::Call(call) = &turns[0].blocks[0] else {
            panic!("a call");
        };
        assert_eq!(call.edits.len(), 1);
        assert_eq!(call.edits[0].id, 10);
        assert!(
            turns[0].edits.is_empty(),
            "an edit of a call in the turn is not repeated"
        );
    }

    #[test]
    fn an_edit_whose_entry_is_in_the_turn_but_is_no_call_is_shown_with_the_turn() {
        let entries = vec![
            entry(1, 0, 1, EntryKind::User, "p"),
            entry(2, 1, 1, EntryKind::Assistant, "wrote it"),
        ];
        let turns = turns(&detail("a", entries, vec![edit(10, 2, "a.rs")], false));
        assert_eq!(turns[0].edits.len(), 1);
    }

    #[test]
    fn the_turns_of_a_partly_read_thread_group_what_is_loaded() {
        let entries = vec![entry(1, 0, 1, EntryKind::User, "p")];
        let turns = turns(&detail("a", entries, Vec::new(), true));
        assert_eq!(turns.len(), 1);
        assert!(turns[0].blocks.is_empty());
    }

    #[test]
    fn a_result_is_paired_only_with_a_call_in_the_thread_loaded() {
        // A call on a later page is not yet loaded: its result shows as a message.
        let entries = vec![
            entry(1, 0, 1, EntryKind::User, "p"),
            result(2, 1, 1, "later", "text", false),
        ];
        let turns = turns(&detail("a", entries, Vec::new(), false));
        assert!(matches!(&turns[0].blocks[0], Block::Message(m) if m.kind == MessageKind::Result));
    }

    #[test]
    fn a_plain_title_drops_the_markdown_markers_of_its_first_line() {
        assert_eq!(plain_title("## Retry plan"), "Retry plan");
        assert_eq!(plain_title("- [ ] do it"), "do it");
        assert_eq!(plain_title("- [x] done it"), "done it");
        assert_eq!(plain_title("> 1. **bold** `code` step"), "bold code step");
        assert_eq!(plain_title("* + bullet"), "bullet");
        assert_eq!(plain_title("#42 is a number"), "#42 is a number");
    }

    #[test]
    fn a_plain_title_takes_the_first_non_empty_line_and_its_link_text() {
        assert_eq!(
            plain_title("\n\n  See [the docs](https://example.org/x)  \nsecond line"),
            "See the docs"
        );
        assert_eq!(plain_title("a [b [c](d) e"), "a [b c e");
        assert_eq!(plain_title("tabs\t\tand   spaces"), "tabs and spaces");
    }

    #[test]
    fn a_plain_title_is_untitled_when_nothing_is_left_and_capped_with_an_ellipsis() {
        assert_eq!(plain_title(""), "Untitled thread");
        assert_eq!(plain_title(" \n ## \n"), "Untitled thread");
        assert_eq!(plain_title("- [ ]"), "Untitled thread");
        let long = "word ".repeat(60);
        let cut = plain_title(&long);
        assert_eq!(cut.chars().count(), TITLE_CHARS);
        assert!(cut.ends_with('…'));
        assert!(!cut.contains("  "));
    }

    #[test]
    fn a_path_is_shown_relative_to_the_worktree_or_home_and_cut_at_its_start() {
        assert_eq!(
            display_path("/w/repo/src/a.rs", "/w/repo", "/home/u"),
            "src/a.rs"
        );
        assert_eq!(
            display_path("/home/u/notes/todo.md", "/w/repo", "/home/u"),
            "~/notes/todo.md"
        );
        assert_eq!(
            display_path("/opt/x.txt", "/w/repo", "/home/u"),
            "/opt/x.txt"
        );
        // A sibling folder with the same start is not inside the worktree.
        assert_eq!(
            display_path("/w/repo2/a.rs", "/w/repo", "/home/u"),
            "/w/repo2/a.rs"
        );
        let deep = format!("/w/repo/{}/file_name.rs", "dir/".repeat(40));
        let shown = display_path(&deep, "/w/repo", "/home/u");
        assert_eq!(shown.chars().count(), PATH_CHARS);
        assert!(shown.starts_with('…'));
        assert!(shown.ends_with("/file_name.rs"));
    }

    #[test]
    fn the_files_of_a_thread_are_its_edited_paths_once_and_its_manual_file_links() {
        let mut detail = detail("a", Vec::new(), Vec::new(), false);
        let mut first = edit(1, 10, "src/a.rs");
        first.added = 3;
        first.removed = 1;
        let mut again = edit(2, 11, "src/a.rs");
        again.added = 2;
        let mut other = edit(3, 12, "src/b.rs");
        other.added = 1;
        detail.edits = vec![first, again, other];
        detail.links = vec![
            ThreadLink {
                kind: LinkKind::File,
                reference: "docs/manual.md".to_owned(),
                origin: LinkOrigin::Manual,
            },
            ThreadLink {
                kind: LinkKind::File,
                reference: "src/b.rs".to_owned(),
                origin: LinkOrigin::Manual,
            },
            ThreadLink {
                kind: LinkKind::File,
                reference: "src/a.rs".to_owned(),
                origin: LinkOrigin::Observed,
            },
        ];
        let lines = file_lines(&detail);
        let paths: Vec<&str> = lines.iter().map(|line| line.path.as_str()).collect();
        assert_eq!(paths, ["src/a.rs", "src/b.rs", "docs/manual.md"]);
        assert_eq!((lines[0].added, lines[0].removed), (5, 2));
        assert!(
            lines[0].manual.is_none(),
            "an observed link is not unlinkable"
        );
        assert!(lines[1].manual.is_some());
        assert_eq!((lines[2].added, lines[2].removed), (0, 0));
        assert_eq!(
            header_counts(&detail),
            HeaderCounts {
                commits: 0,
                files: 3,
                edits: 3,
            }
        );
    }

    #[test]
    fn the_header_line_names_the_harness_branch_age_state_and_tags() {
        let mut thread = summary("a", 1_000);
        thread.started_at = 0;
        thread.ended_at = Some(5_000);
        assert_eq!(
            header_meta(&thread, 3 * 3_600),
            "claude-code · main · started 3 h ago"
        );
        thread.ended_at = None;
        thread.branch = None;
        thread.tags = vec!["bug".to_owned(), "api".to_owned()];
        let text = header_meta(&thread, 60);
        assert!(text.starts_with("claude-code · detached HEAD · started 1 min ago · open"));
        assert!(text.ends_with(" · tags: bug, api"));
        assert_eq!(chip_label(1, "commit", "commits"), "1 commit");
        assert_eq!(chip_label(0, "file", "files"), "0 files");
    }

    #[test]
    fn the_commit_count_is_the_commit_links_whatever_their_origin() {
        let mut detail = detail("a", Vec::new(), Vec::new(), false);
        detail.links = vec![
            ThreadLink {
                kind: LinkKind::Commit,
                reference: "abc".to_owned(),
                origin: LinkOrigin::Observed,
            },
            ThreadLink {
                kind: LinkKind::Commit,
                reference: "def".to_owned(),
                origin: LinkOrigin::Manual,
            },
            ThreadLink {
                kind: LinkKind::Branch,
                reference: "main".to_owned(),
                origin: LinkOrigin::Observed,
            },
        ];
        assert_eq!(header_counts(&detail).commits, 2);
        assert_eq!(header_counts(&detail).files, 0);
    }

    #[test]
    fn the_details_panel_is_per_thread_and_only_one_is_open_at_a_time() {
        let mut page = ThreadsPage::default();
        assert_eq!(page.open_panel(), None, "closed with no thread selected");
        page.toggle_panel(Panel::Files);
        assert_eq!(page.open_panel(), None, "a panel needs a thread");
        page.selected = Some("a".to_owned());
        assert_eq!(page.open_panel(), None, "closed by default");
        page.toggle_panel(Panel::Commits);
        assert_eq!(page.open_panel(), Some(Panel::Commits));
        page.toggle_panel(Panel::Files);
        assert_eq!(page.open_panel(), Some(Panel::Files));
        page.toggle_panel(Panel::Files);
        assert_eq!(page.open_panel(), None, "the same chip closes it");
        page.toggle_panel(Panel::Note);
        page.selected = Some("b".to_owned());
        assert_eq!(page.open_panel(), None, "another thread keeps its own");
        page.toggle_panel(Panel::Files);
        page.selected = Some("a".to_owned());
        assert_eq!(page.open_panel(), Some(Panel::Note));
        page.forget("a");
        page.selected = Some("a".to_owned());
        assert_eq!(
            page.open_panel(),
            None,
            "a deleted thread forgets its panel"
        );
    }

    #[test]
    fn a_thread_with_no_title_says_untitled_and_a_title_is_trimmed() {
        let mut thread = summary("a", 1);
        thread.title = "   ".to_owned();
        assert_eq!(title(&thread), "Untitled thread");
        thread.title = "  retry with backoff ".to_owned();
        assert_eq!(title(&thread), "retry with backoff");
    }

    #[test]
    fn the_selection_survives_a_refresh_that_changes_the_rows_around_it() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        page.choose(0);
        let list = page.plan_list();
        page.sent(&list.read, id(2));
        page.listed(id(2), rows(&["new", "a", "b"]), 3);
        assert_eq!(page.selected(), Some("a"));
        assert_eq!(page.selected_row(), Some(1));
    }

    #[test]
    fn a_thread_of_no_edits_plans_no_state_reads() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        selected_page(&mut page, "a", Vec::new(), false);
        assert!(page.plan_edit_states().is_empty());
        assert!(page.plan_more().is_none());
        assert_eq!(page.focus_turn(), None);
    }
    #[test]
    fn the_file_filter_is_set_and_cleared_and_only_a_change_says_so() {
        let mut page = ThreadsPage::default();
        assert_eq!(page.path(), None);
        assert!(!page.filter_active());
        assert!(page.set_path(Some("src/a.rs".to_owned())));
        assert!(!page.set_path(Some("src/a.rs".to_owned())));
        assert_eq!(page.path(), Some("src/a.rs"));
        assert!(page.filter_active());
        assert!(page.set_path(None));
        assert_eq!(page.path(), None);
        assert!(!page.filter_active());
    }

    #[test]
    fn a_summary_is_found_in_the_list_and_in_the_thread_shown() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        assert_eq!(page.summary("b").map(|s| s.uid.as_str()), Some("b"));
        assert!(page.summary("zz").is_none());
        selected_page(&mut page, "a", vec![], false);
        assert_eq!(page.selected_summary().map(|s| s.uid.as_str()), Some("a"));
    }

    #[test]
    fn a_deleted_thread_is_forgotten_so_no_read_of_it_follows() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a", "b"], 2);
        selected_page(&mut page, "a", vec![], false);
        page.forget("b");
        assert_eq!(
            page.selected(),
            Some("a"),
            "another thread leaves the selection"
        );
        page.forget("a");
        assert_eq!(page.selected(), None);
        assert!(page.detail().is_none());
        assert!(
            page.plan_refresh()
                .iter()
                .all(|request| !matches!(request.kind, CommandKind::ReadThread { .. }))
        );
    }

    #[test]
    fn a_hit_reads_from_the_entries_before_it_and_the_earlier_part_is_read_on_demand() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        page.set_query("needle");
        let search = page.plan_search().expect("a search");
        page.sent(&search.read, id(2));
        let hit_at_30 = ThreadHit {
            seq: 30,
            entry_id: 30,
            ..hit("a", 30)
        };
        assert!(page.searched(id(2), vec![hit_at_30]));
        let request = page.choose(0).expect("the hit opens its thread");
        assert!(matches!(
            request.kind,
            CommandKind::ReadThread {
                from_seq: 10,
                limit: ENTRY_PAGE,
                ..
            }
        ));
        page.sent(&request.read, id(3));
        let entries: Vec<ThreadEntry> = (10..=30)
            .map(|seq| {
                entry(
                    i64::from(seq),
                    seq,
                    1,
                    EntryKind::Assistant,
                    &format!("m{seq}"),
                )
            })
            .collect();
        assert!(page.read(id(3), detail("a", entries, Vec::new(), false)));
        assert_eq!(page.focus(), Some(30));
        let earlier = page
            .plan_earlier()
            .expect("entries before the first one shown");
        assert!(matches!(
            earlier.kind,
            CommandKind::ReadThread {
                from_seq: 0,
                limit: 10,
                ..
            }
        ));
        page.sent(&earlier.read, id(4));
        let before: Vec<ThreadEntry> = (0..10)
            .map(|seq| entry(i64::from(seq) + 100, seq, 1, EntryKind::User, "x"))
            .collect();
        assert!(page.read(id(4), detail("a", before, Vec::new(), true)));
        let shown = page.detail().expect("shown");
        assert_eq!(
            shown.entries.len(),
            31,
            "the earlier page is put before the hit"
        );
        assert_eq!(shown.entries.first().map(|e| e.seq), Some(0));
        assert_eq!(shown.entries.last().map(|e| e.seq), Some(30));
        assert!(
            page.plan_earlier().is_none(),
            "the thread starts at its first entry"
        );
    }

    #[test]
    fn an_unlisted_selected_thread_has_its_window_read_again_after_a_list_page() {
        let mut page = ThreadsPage::default();
        listed_page(&mut page, 1, &["a"], 1);
        let request = page.choose(0).expect("a");
        page.sent(&request.read, id(2));
        assert!(page.read(
            id(2),
            detail(
                "a",
                vec![entry(1, 0, 1, EntryKind::User, "u")],
                Vec::new(),
                false
            )
        ));
        // The filter leaves the selected thread out of the list.
        listed_page(&mut page, 3, &["b"], 2);
        let window = page
            .plan_tail()
            .expect("the thread is not listed, so it is read again");
        assert!(matches!(
            window.read,
            Read::Detail {
                from_seq: 0,
                place: Place::Replace,
                ..
            }
        ));
    }

    #[test]
    fn a_copy_of_a_thread_as_markdown_keeps_only_its_own_answer_and_gives_it_once() {
        let mut copy = MarkdownCopy::default();
        copy.sent(id(7));
        assert!(!copy.answered(id(8), "other".to_owned()));
        assert!(copy.answered(id(7), "# Title\n".to_owned()));
        assert_eq!(copy.take(), Some(Ok("# Title\n".to_owned())));
        assert_eq!(copy.take(), None, "the outcome is taken once");
    }

    #[test]
    fn a_failed_copy_says_the_engines_reason_and_a_cancelled_one_says_nothing() {
        let mut copy = MarkdownCopy::default();
        copy.sent(id(1));
        assert!(copy.failed(id(1), Some("invalid redaction pattern".to_owned())));
        assert_eq!(
            copy.take(),
            Some(Err("invalid redaction pattern".to_owned()))
        );
        copy.sent(id(2));
        assert!(copy.failed(id(2), None));
        assert_eq!(
            copy.take(),
            Some(Err("The thread could not be copied.".to_owned()))
        );
        copy.sent(id(3));
        assert!(copy.cancelled(id(3)));
        assert_eq!(copy.take(), None);
    }
}
