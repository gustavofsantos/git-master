//! Canonical thread types shared by the engine, the UI and MCP (design/threads.md).
//! Harness adapters produce `CanonicalEvent`s; nothing downstream of them sees a
//! harness-specific shape. Timestamps are milliseconds since the Unix epoch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The canonical class of a tool call (D129). The adapter maps the harness's
/// native tool name to one of these and keeps the native name alongside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    /// Changes an existing file: triggers pre and post images.
    Edit,
    /// Creates or overwrites a file.
    Write,
    Shell,
    Search,
    Web,
    /// A sub-agent.
    Agent,
    Other,
}

/// How a session began, as the harness reports it (`SessionStart` source).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionOrigin {
    New,
    Resumed,
    Cleared,
    Compacted,
    Unknown,
}

/// Whether an edit hook fired before or after the tool ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditPhase {
    Before,
    After,
}

/// The change kind of one file edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditChange {
    Create,
    Modify,
    Delete,
    Rename,
}

/// Fields common to every canonical event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct EventHeader {
    /// Stable harness id, such as `claude-code`.
    pub harness: String,
    /// The harness's own session id.
    pub session_id: String,
    /// The harness's id for the item, or a hash of harness, session and position
    /// when it has none, so a replay is idempotent.
    pub source_id: String,
    /// When the event happened, in milliseconds since the Unix epoch.
    pub at: i64,
}

/// An edit the adapter reports from the tool input, used when the pipeline
/// cannot measure the file (D123). Paths are repository-relative or absolute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct EditIntent {
    pub path: String,
    pub change: EditChange,
    /// The previous path, for a rename.
    pub old_path: Option<String>,
    /// The text the harness says was replaced, when it gives it.
    pub old_text: Option<String>,
    /// The text the harness says was written, when it gives it.
    pub new_text: Option<String>,
}

/// One event of the canonical model (design/threads.md, "Canonical events").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanonicalEvent {
    SessionStarted {
        header: EventHeader,
        cwd: String,
        model: Option<String>,
        harness_version: Option<String>,
        log_path: Option<String>,
        origin: SessionOrigin,
    },
    PromptSubmitted {
        header: EventHeader,
        text: String,
    },
    AssistantMessage {
        header: EventHeader,
        text: String,
        model: Option<String>,
    },
    ToolCalled {
        header: EventHeader,
        tool: ToolKind,
        native_tool: String,
        args: serde_json::Value,
        /// Set for a call made by a sub-agent: the `Agent` call it belongs to.
        parent_call: Option<String>,
        edit: Option<EditIntent>,
    },
    ToolReturned {
        header: EventHeader,
        /// `source_id` of the `ToolCalled` this result answers.
        call_source_id: String,
        text: String,
        is_error: bool,
    },
    /// Closes the turn; `HEAD` is read for links. `parent_call` is set for a sub-agent.
    TurnEnded {
        header: EventHeader,
        parent_call: Option<String>,
    },
    SessionEnded {
        header: EventHeader,
        reason: String,
    },
    /// Compaction, interruption and other things worth showing but not a message.
    Notice {
        header: EventHeader,
        text: String,
    },
}

impl CanonicalEvent {
    /// The common header of any event.
    #[must_use]
    pub fn header(&self) -> &EventHeader {
        match self {
            Self::SessionStarted { header, .. }
            | Self::PromptSubmitted { header, .. }
            | Self::AssistantMessage { header, .. }
            | Self::ToolCalled { header, .. }
            | Self::ToolReturned { header, .. }
            | Self::TurnEnded { header, .. }
            | Self::SessionEnded { header, .. }
            | Self::Notice { header, .. } => header,
        }
    }
}

/// The kind of a stored entry (`entry.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Notice,
}

/// One row of the thread list (the `thread` table, with counts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadSummary {
    /// First 16 hex digits of sha256(harness, NUL, session id).
    pub uid: String,
    pub harness: String,
    /// As the harness reports it, if it does.
    pub harness_version: Option<String>,
    pub session_id: String,
    /// Worktree root at the first event.
    pub worktree: String,
    /// Branch at the first event; `None` when detached.
    pub branch: Option<String>,
    pub model: Option<String>,
    /// From the first prompt unless edited.
    pub title: String,
    pub note: String,
    pub starred: bool,
    /// The names of the labels attached to the thread, by name (D145). The field keeps its
    /// name from the tags it replaced, so the consumers did not change.
    pub tags: Vec<String>,
    pub started_at: i64,
    pub last_event_at: i64,
    /// `None` while the session is open.
    pub ended_at: Option<i64>,
    pub entry_count: u32,
    pub edit_count: u32,
    pub redaction_count: u32,
    /// The workspace folder the thread belongs to, when it is a thread of another repository's
    /// workspace that the listing found through a member commit (D180 (2)). `None` for the
    /// threads of the open repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// One stored entry of a thread (the `entry` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadEntry {
    /// Row id, used by search hits and by `ReadThreadEdit`-style lookups.
    pub id: i64,
    /// Order inside the thread.
    pub seq: u32,
    /// 1-based; increments on each human prompt.
    pub turn: u32,
    pub kind: EntryKind,
    /// Harness id, unique per thread.
    pub source_id: String,
    /// `source_id` of the tool call a sub-agent entry belongs to.
    pub parent_id: Option<String>,
    /// Native tool name for tool calls and results.
    pub tool: Option<String>,
    /// Redacted, capped text.
    pub text: String,
    /// Redacted, capped tool input as JSON text.
    pub args_json: Option<String>,
    pub is_error: bool,
    /// Whether `text` or `args_json` was cut at the cap.
    pub truncated: bool,
    /// Size before the cap, in bytes.
    pub orig_bytes: Option<u64>,
    pub redactions: u32,
    pub at: i64,
}

/// One file edit of a thread (the `edit` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadEdit {
    pub id: i64,
    /// The entry that carried the edit.
    pub entry_id: i64,
    pub path: String,
    pub old_path: Option<String>,
    pub change: EditChange,
    /// Unified diff, head and tail kept when capped.
    pub patch: String,
    pub added: u32,
    pub removed: u32,
    /// Blob id before the edit, when measured.
    pub pre_oid: Option<String>,
    /// Blob id after the edit, when measured.
    pub post_oid: Option<String>,
    /// True when the edit comes from the tool input, not from measuring the file (D123).
    pub from_input: bool,
    pub at: i64,
    /// The member of the workspace the file is in (D180). `None` for the workspace itself and for
    /// an ordinary repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// Where an edit's file is now, derived on read and never stored (D126).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditState {
    /// The file's blob equals the post image.
    InWorkingTree,
    /// The blob at `HEAD` equals the post image.
    InHead,
    /// Neither.
    ChangedSince,
    Unknown,
}

/// Who wrote a conversation entry, for the search `role` filter (D147). Only the two kinds
/// the search index covers (D132).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryRole {
    User,
    Assistant,
}

/// Filters for listing and searching threads. Each field is a column, not query
/// syntax (D128). `None` means no restriction. The entry fields (`role`, `turn_from`,
/// `turn_to`) restrict which entries a search can match and do not apply to listing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadFilter {
    pub harness: Option<String>,
    pub branch: Option<String>,
    /// Inclusive lower bound on `last_event_at`, in milliseconds.
    pub since: Option<i64>,
    /// Exclusive upper bound on `last_event_at`, in milliseconds.
    pub until: Option<i64>,
    pub starred: Option<bool>,
    pub has_edits: Option<bool>,
    /// A thread that edited this path. The path matches the path of an edit exactly, or,
    /// when the value contains `*`, `?` or `[`, as a glob too (SQLite `GLOB`). In a glob
    /// `*` also matches `/`, and matching is case-sensitive, for exact and glob alike. So
    /// a literal path with brackets, such as `app/[id]/page.tsx`, matches itself (D147).
    pub path: Option<String>,
    /// A thread linked to this commit. The value is a full id or a short one of at
    /// least seven hex digits; hex digits match in either case.
    pub commit: Option<String>,
    pub tag: Option<String>,
    /// Only entries of this role can match (search only). A match inside an edit counts
    /// the entry that carried the edit, which is a tool call, so `role` excludes it.
    #[serde(default)]
    pub role: Option<EntryRole>,
    /// Only this thread, by its uid (D147).
    #[serde(default)]
    pub thread: Option<String>,
    /// Inclusive lower bound on the entry's `turn`, 1-based (search only).
    #[serde(default)]
    pub turn_from: Option<u32>,
    /// Inclusive upper bound on the entry's `turn`, 1-based (search only).
    #[serde(default)]
    pub turn_to: Option<u32>,
    /// Exact match on the thread's model.
    #[serde(default)]
    pub model: Option<String>,
    /// Threads carrying every label named here (D145, search and listing). Each name is
    /// matched exactly, and an empty list restricts nothing.
    #[serde(default)]
    pub label: Option<Vec<String>>,
    /// Threads linked to the thread with this uid, in either direction (D143). Narrowed to
    /// `related_kind` when that is set.
    #[serde(default)]
    pub related_to: Option<String>,
    /// The kind of link that `related_to` counts; `None` counts every kind.
    #[serde(default)]
    pub related_kind: Option<ThreadRelKind>,
}

/// How a search hit matched (D131). `Exact` is the strict word-start search; the two
/// fallback stages are `Substring` (the typed text is inside a word) and `Fuzzy`
/// (a close misspelling). A hit without a kind, from older data, is `Exact`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HitKind {
    #[default]
    Exact,
    Substring,
    Fuzzy,
}

/// How a search groups its hits (D147).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchGroup {
    /// One best hit per thread.
    #[default]
    Thread,
    /// Every matching entry.
    Entry,
}

/// The order of search hits (D147). `Relevance` is the match rank; `Recent` sorts the
/// best hits by the time of their entry, newest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchOrder {
    #[default]
    Relevance,
    Recent,
}

/// One entry next to a search hit (`context`, D147): its place in the thread, its kind and
/// its text, capped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadEntryBrief {
    pub seq: u32,
    pub kind: EntryKind,
    pub text: String,
}

/// The most common values among the threads a search matches, each with its count (D147).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchFacets {
    pub branch: Vec<(String, u32)>,
    pub harness: Vec<(String, u32)>,
    /// Counted over the edits of the matching threads.
    pub path: Vec<(String, u32)>,
    /// The labels of the matching threads, by name (D145).
    #[serde(default)]
    pub label: Vec<(String, u32)>,
}

/// One search result: the best-matching entry of a thread, with the thread's metadata
/// (D147). A hit of a listing (no query) has `seq` 0 and the title as its snippet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadHit {
    pub thread_uid: String,
    pub title: String,
    pub entry_id: i64,
    pub seq: u32,
    pub turn: u32,
    /// Text around the match.
    pub snippet: String,
    pub at: i64,
    /// Which stage of the search found it (D131).
    #[serde(default)]
    pub kind: HitKind,
    /// The thread's harness.
    #[serde(default)]
    pub harness: String,
    /// The thread's branch at its first event; `None` when detached.
    #[serde(default)]
    pub branch: Option<String>,
    /// The thread's start time, in milliseconds.
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub edit_count: u32,
    /// The names of the thread's labels (D145), as `ThreadSummary::tags`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The user and assistant entries before and after the hit in its thread, when the
    /// search asks for context (D147).
    #[serde(default)]
    pub context: Vec<ThreadEntryBrief>,
}

/// What a link points at (D126).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Commit,
    Branch,
    File,
}

/// How a link was made: observed from Git and the events, or set by a human (D143; the
/// schema v5 migration renames `manual` to `human`). An agent's rows carry their origin
/// on the label or relation tables, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkOrigin {
    Observed,
    Human,
}

/// Where a commit link points now (D145). The stored id is never changed; this says what the
/// id resolves to when it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommitState {
    /// The stored commit is reachable from a ref (or HEAD).
    Current,
    /// The stored commit is not reachable; `resolved` names its replacement, found by a restack
    /// (D142) or by an exact unique `git patch-id --stable` match.
    Remapped,
    /// The stored commit is not reachable and more than one commit in the rewritten range has
    /// its patch, so none is picked.
    Ambiguous,
    /// The stored commit is not reachable and no commit in the rewritten range has its patch,
    /// or the commit object is missing.
    Gone,
    /// The check did not run: its per-read budget was used up, or Git failed. Nothing is cached.
    Unchecked,
}

/// A link from a thread to Git (D126): a commit id, a branch name or a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadLink {
    pub kind: LinkKind,
    /// Commit id, branch name or repository-relative path.
    pub reference: String,
    pub origin: LinkOrigin,
    /// For a commit link: the commit the stored id resolves to now, when it is not that id (D145).
    #[serde(default)]
    pub resolved: Option<String>,
    /// For a commit link: how the stored id resolves now (D145). `None` for other kinds.
    #[serde(default)]
    pub state: Option<CommitState>,
    /// The member of the workspace the commit was made in (D180). `None` for the workspace itself
    /// and for an ordinary repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// The thread a sub-agent thread belongs to (D199 (2)): the parent's uid and the `seq` of the
/// parent's `Agent` call, when the parent holds that entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadParent {
    /// The parent thread's uid, as `ThreadSummary::uid`.
    pub uid: String,
    /// The `seq` of the parent's `Agent` call entry; `None` when the parent does not hold it.
    pub seq: Option<u32>,
}

/// A thread with its entries, edits and links, as `ReadThread` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadDetail {
    pub summary: ThreadSummary,
    /// The parent of a sub-agent thread (D199 (2)); `None` for any other thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ThreadParent>,
    pub entries: Vec<ThreadEntry>,
    pub edits: Vec<ThreadEdit>,
    pub links: Vec<ThreadLink>,
    /// The thread-to-thread links in either direction, newest first, at most 50 (D143). Each
    /// names both ends by uid, so `from` tells the direction of the link.
    #[serde(default)]
    pub relations: Vec<ThreadRelation>,
    /// Redaction pattern name and how many times it fired.
    pub redactions: Vec<(String, u32)>,
    /// True when more entries follow `from_seq + limit`.
    pub has_more: bool,
}

/// The evidence that ties a line to an edit or a commit (D144), strongest first. The order of the
/// variants is the order of strength, so sorting puts the strongest first.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The edit's `post_oid` equals the file's blob in the blamed commit.
    BlobMatch,
    /// A contiguous run of the lines appears contiguously in the added lines of one edit.
    LineMatch,
    /// The edit added a line that the blamed commit also added to the file.
    HunkOverlap,
    /// The commit appeared on the thread's branch during the thread (D126).
    ObservedCommit,
    /// The user linked the thread to the commit.
    Manual,
}

/// Why an edit cannot be compared with the lines (D144). Such an edit is reported as
/// unverifiable, never as no match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Unverifiable {
    /// The edit's text was redacted (D121).
    Redacted,
    /// The edit's patch was cut at the size limit.
    Truncated,
    /// The edit is known only from the tool input; the file was not measured (D123).
    FromInput,
}

/// A run of consecutive lines of the queried file, both ends inclusive, 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LineRun {
    pub from: u32,
    pub to: u32,
}

/// One edit of a provenance thread that was compared with the lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceEdit {
    pub edit_id: i64,
    /// The row id of the entry that carried the edit, as a thread's entries carry it. A client
    /// opens the thread focused on this entry. A payload without it reads as zero.
    #[serde(default)]
    pub entry_id: i64,
    /// The sequence number of the entry that carried the edit.
    pub seq: u32,
    /// The path as the edit recorded it.
    pub path: String,
    /// When the edit was made, in milliseconds.
    pub at: i64,
    /// The evidence for this edit, strongest first. Empty when the edit is only unverifiable.
    pub evidence: Vec<Evidence>,
    /// Set when the edit could not be compared, so no match is claimed either way.
    pub unverifiable: Option<Unverifiable>,
    /// The user and assistant entries around the edit's entry (two each side).
    pub entries: Vec<ThreadEntryBrief>,
}

/// A thread that explains some lines of a commit (D144).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceThread {
    pub uid: String,
    pub title: String,
    pub harness: String,
    /// The branch at the thread's first event; `None` when detached.
    pub branch: Option<String>,
    /// Every evidence the thread has for these lines, strongest first.
    pub evidence: Vec<Evidence>,
    /// Other edits also match these lines, so no edit is picked (D144).
    pub ambiguous: bool,
    pub edits: Vec<ProvenanceEdit>,
    /// The labels on the thread, at most 10; a cut says so (D144, second edge).
    #[serde(default)]
    pub labels: Vec<ProvenanceLabel>,
    /// The threads linked to it, at most 10; a cut says so (D144, second edge).
    #[serde(default)]
    pub links: Vec<ProvenanceLink>,
}

/// One commit that last changed some lines of the queried file, under the path Git reports for them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceCommit {
    /// Full commit id.
    pub id: String,
    pub subject: String,
    pub author: String,
    /// The author date, in milliseconds.
    pub author_at: i64,
    /// The path blame reports for these lines, which may differ from the queried path after a copy or rename.
    pub path: String,
    /// The lines of the query this commit owns, in order.
    pub lines: Vec<LineRun>,
    pub threads: Vec<ProvenanceThread>,
}

/// Which bound cut a provenance result (D144).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CutBound {
    /// More commits own the lines than the result shows.
    Commits,
    /// More threads match a commit than the result shows.
    Threads,
    /// More edits touch the path than the commit compares; the rest are not compared.
    Candidates,
    /// More labels are attached to a thread than the result shows (D144, second edge).
    Labels,
    /// More threads are linked to a thread than the result shows (D144, second edge).
    Links,
}

/// Which way a thread-to-thread link points, from the thread a provenance result shows (D143).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationDirection {
    /// The shown thread is the `from` end.
    Out,
    /// The shown thread is the `to` end.
    In,
}

/// A label on a thread that a provenance result shows (D144, second edge).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceLabel {
    pub name: String,
    /// How the attachment was made: `human` or `agent:<client>`.
    pub origin: String,
}

/// A thread linked to a thread that a provenance result shows (D144, second edge). The
/// linked thread is named, not walked further.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceLink {
    pub uid: String,
    pub title: String,
    pub kind: ThreadRelKind,
    pub direction: RelationDirection,
    /// How the link was made: `human` or `agent:<client>`.
    pub origin: String,
}

/// Something a provenance result left out, and the bound that did it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceCut {
    pub bound: CutBound,
    /// The bound's value, such as 10 commits.
    pub limit: u32,
    /// The commit the cut is in, for a thread or candidate cut.
    pub commit: Option<String>,
}

/// The answer of `ReadProvenance` (D144): why the lines are as they are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceResult {
    pub path: String,
    pub from_line: u32,
    pub to_line: u32,
    pub commits: Vec<ProvenanceCommit>,
    pub cuts: Vec<ProvenanceCut>,
}

/// What a label attaches to (`label_attach.kind`, D145). A label can also attach to
/// another label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachKind {
    Thread,
    File,
    Commit,
    Label,
}

/// The kind of a link between two threads (`thread_rel`, D143).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRelKind {
    Continues,
    Relates,
    Supersedes,
}

/// A label: a named, repository-scoped node with an optional colour (D145). `origin` is
/// `human` or `agent:<client>`; a label is never `observed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// `#rrggbb`, when the label has a colour.
    pub color: Option<String>,
    pub origin: String,
    pub created_at: i64,
    /// How many rows are attached to the label, of every kind.
    pub attachments: u32,
}

/// One attachment of a label (`label_attach`). For a commit, the snapshot keeps the
/// subject, author and committer date, so a collected commit still shows (D145).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct LabelAttachment {
    pub kind: AttachKind,
    /// Thread uid, repository-relative path, commit id or label id, by `kind`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub origin: String,
    pub at: i64,
    pub subject: Option<String>,
    pub author: Option<String>,
    /// Committer date of the commit, in milliseconds.
    pub committed_at: Option<i64>,
    /// The commit id a rewrite resolved to, when it is not `ref` (D145).
    pub resolved: Option<String>,
    /// For a commit attachment: how `ref` resolves now (D145). `None` for other kinds.
    #[serde(default)]
    pub state: Option<CommitState>,
    /// For a file attachment: the path the file has now, followed through renames (D145).
    /// `None` for other kinds, and for a file past the per-read bound.
    #[serde(default)]
    pub current_path: Option<String>,
}

/// A label with what is attached to it, as `ReadLabel` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct LabelDetail {
    pub label: Label,
    pub attachments: Vec<LabelAttachment>,
}

/// A link from one thread to another (`thread_rel`, D143), by uid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadRelation {
    pub from: String,
    pub to: String,
    pub kind: ThreadRelKind,
    pub origin: String,
    pub at: i64,
}

/// Events dropped for a reason the user can see (the `capture_gap` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CaptureGap {
    pub at: i64,
    pub reason: String,
    /// How many events were dropped for this reason.
    pub count: u32,
}

/// The kind of a memory (D190): a decision taken, a rule to follow, a lesson learned or a fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    Rule,
    Lesson,
    Fact,
}

/// Where a memory is in its review (D190, D192). Only a human moves a memory out of `proposed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}

/// How a memory is known (D201 (1)). `unconfirmed` is a belief: the results mark it, so it is never
/// presented as established fact. A memory with no basis given is `observed`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBasis {
    /// Seen in a thread or a file.
    #[default]
    Observed,
    /// Checked by running something.
    Tested,
    /// A belief that nothing has confirmed.
    Unconfirmed,
}

/// A person's or an agent's judgement of a memory's usefulness (D202). `useful` means the memory
/// helped; `wrong` means it led somewhere it should not have, or is no longer true. Marks are signals:
/// they never change the memory's text, status or order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMark {
    Useful,
    Wrong,
}

/// One actor's mark of a memory (D202). `marks` on a memory holds the latest mark of each actor, and
/// the mark history of a memory lists every mark ever made, oldest first, each with the same fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct MemoryMarkSummary {
    /// `human` or `agent:<client>`, as the origin of a row.
    pub actor: String,
    pub mark: MemoryMark,
    /// When the mark was made, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The one-line reason the actor gave, redacted like the memory's text (at most 200 characters).
    /// Agent-written text: untrusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The kind of a memory's source: a thread (with an entry range), a commit or a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceKind {
    Thread,
    Commit,
    File,
}

/// A source a caller names for a memory. A commit is given as Git gives it (the engine resolves
/// it to a full id); a file is a repository-relative path; a thread is a stored thread's uid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct MemorySourceInput {
    pub kind: MemorySourceKind,
    /// Thread uid, commit id or repository-relative path, by `kind`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// First entry of a thread range (`thread` sources only).
    pub entry_from: Option<i64>,
    /// Last entry of a thread range, inclusive (`thread` sources only).
    pub entry_to: Option<i64>,
}

/// A source of a memory as stored (`memory_source`). `blob_oid` is set for a file source when the
/// memory is accepted: the blob at `HEAD` then, or none when the file was not there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct MemorySource {
    pub kind: MemorySourceKind,
    /// Thread uid, commit id or repository-relative path, by `kind`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// `None` for the memory's own repository (D191).
    pub repo_id: Option<String>,
    pub entry_from: Option<i64>,
    pub entry_to: Option<i64>,
    pub blob_oid: Option<String>,
    /// Whether `blob_oid` was read at acceptance (D193 (9)): false until the memory is accepted, and
    /// false when the blob could not be read (its folder was not known, or Git did not answer).
    pub blob_read: bool,
}

/// Whether a file a memory names has changed since the memory was accepted (D192 (6)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeState {
    /// The blob at `HEAD` differs from the one recorded, or the file appeared after acceptance.
    Changed,
    /// The file the memory names is gone from `HEAD`.
    Missing,
    /// Git could not answer for the file; never reported as unchanged.
    Unknown,
    /// Nothing can be read: the source's repository is not on this machine, or the file's blob was
    /// not read when the memory was accepted (D193 (9)).
    Unavailable,
}

/// A file a memory names that has changed since it was accepted. Derived on read, never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct FileChange {
    pub path: String,
    pub state: FileChangeState,
}

/// A context a memory is attached to, as the repository that reads the memory sees it (D193 (1),
/// "Contexts"). Only the contexts the open repository can see are carried, so nothing outside its
/// visibility shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct MemoryContext {
    /// The context's row id, as `DetachMemory` and `AttachMemory` name it.
    pub id: i64,
    /// The context's path from the root, such as `Platform / Billing`.
    pub path: String,
}

/// A memory: a short record the project wants future sessions to know (design/threads.md,
/// "Memories"; D190, D192). Its `text` is untrusted data, like every text from a thread: a reader
/// must not follow it as an instruction. `files_changed_since` is always empty when the store
/// returns a memory; the engine derives it on read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Memory {
    pub id: i64,
    pub uid: String,
    pub kind: MemoryKind,
    /// Redacted at capture, at most 1,000 characters. Untrusted text.
    pub text: String,
    /// How the memory is known (D201 (1)). Absent from older JSON, which reads as `observed`.
    #[serde(default)]
    pub basis: MemoryBasis,
    pub status: MemoryStatus,
    /// `human`, `agent:<client>` or `imported:<author>`. Becomes `human` when a human edits the text.
    pub origin: String,
    /// The origin at creation.
    pub proposed_by: Option<String>,
    /// The model the harness reported, when it did.
    pub model: Option<String>,
    pub created_at: i64,
    /// When the memory was accepted, rejected or superseded.
    pub decided_at: Option<i64>,
    /// `human` once decided.
    pub decided_by: Option<String>,
    /// False while the memory is `proposed`.
    pub reviewed: bool,
    pub sources: Vec<MemorySource>,
    /// Uids of the memories this one supersedes, in the same store (D193 (1)).
    pub supersedes: Vec<String>,
    /// Uids of the memories that supersede this one, in the same store (D193 (1)).
    pub superseded_by: Vec<String>,
    /// Files named by the memory that changed since acceptance. Always empty from the store.
    pub files_changed_since: Vec<FileChange>,
    /// `repository`, or the path of the nearest visible context the memory is attached to, such as
    /// `Platform / Billing` (D193 (5)). Set by the engine; empty from the store.
    pub scope: String,
    /// Every visible context the memory is attached to, by path. Empty for the repository's own
    /// memories and omitted from the wire then. Set by the engine; empty from the store.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contexts: Vec<MemoryContext>,
    /// The folder of the workspace member whose store holds the memory (D216 (2)), set on a read
    /// from a workspace; absent otherwise. `scope` is then `member <folder>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// How many times agents retrieved the memory on this machine (D201 (4)). Only the app shows it:
    /// no MCP result carries it, and it is never exported. Zero when the store has none.
    #[serde(default)]
    pub retrieval_count: u32,
    /// When the memory was last retrieved, in milliseconds (D201 (4)); `None` while it never was.
    #[serde(default)]
    pub last_retrieved_at: Option<i64>,
    /// The latest mark of each actor (D202 (2)), by actor. Derived on read, never stored as a value.
    /// Absent from older JSON, which reads as no marks.
    #[serde(default)]
    pub marks: Vec<MemoryMarkSummary>,
}

/// A context: a named set of repositories at the user level, with its nesting (design/threads.md,
/// "Contexts"; D191). Its counts and paths are derived on read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Context {
    pub id: i64,
    pub uid: String,
    pub name: String,
    /// The id of the parent context, `None` at the root.
    pub parent: Option<i64>,
    /// The names from the root down, joined with ` / `.
    pub path: String,
    /// The repositories of the context, by repository id.
    pub repos: Vec<ContextRepo>,
    /// Accepted memories attached to this context.
    pub accepted_memories: u32,
    /// Direct sub-contexts.
    pub sub_contexts: u32,
    /// The folders of the workspace members that see this context, in folder order. Set by a
    /// `ListContexts` from a workspace (D216 (4)); empty otherwise and omitted then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

/// A repository in a context: its identity (the content of `.git/gitmaster/repo-id`), the last
/// known common directory of its clone and its display name (D191 (4) (b)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ContextRepo {
    pub repo_id: String,
    pub path: String,
    pub name: String,
}

/// The filters of a memory list. `author` matches the origin or the origin at creation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct MemoryFilter {
    pub kind: Option<MemoryKind>,
    pub status: Option<MemoryStatus>,
    /// A repository path: the memories naming that file or a file under that directory.
    pub path: Option<String>,
    pub author: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(source_id: &str) -> EventHeader {
        EventHeader {
            harness: "claude-code".to_owned(),
            session_id: "s-1".to_owned(),
            source_id: source_id.to_owned(),
            at: 1_700_000_000_000,
        }
    }

    fn round_trip(event: &CanonicalEvent) {
        let json = serde_json::to_string(event).expect("serialize");
        let back: CanonicalEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, event, "round trip changed the event: {json}");
    }

    #[test]
    fn canonical_events_round_trip_through_json() {
        let events = [
            CanonicalEvent::SessionStarted {
                header: header("s-1"),
                cwd: "/repo".to_owned(),
                model: Some("claude".to_owned()),
                harness_version: None,
                log_path: Some("/log.jsonl".to_owned()),
                origin: SessionOrigin::Resumed,
            },
            CanonicalEvent::PromptSubmitted {
                header: header("p-1"),
                text: "retry with backoff".to_owned(),
            },
            CanonicalEvent::AssistantMessage {
                header: header("a-1"),
                text: "done".to_owned(),
                model: None,
            },
            CanonicalEvent::ToolCalled {
                header: header("t-1"),
                tool: ToolKind::Edit,
                native_tool: "Edit".to_owned(),
                args: serde_json::json!({ "file_path": "src/lib.rs" }),
                parent_call: Some("agent-1".to_owned()),
                edit: Some(EditIntent {
                    path: "src/lib.rs".to_owned(),
                    change: EditChange::Modify,
                    old_path: None,
                    old_text: Some("a".to_owned()),
                    new_text: Some("b".to_owned()),
                }),
            },
            CanonicalEvent::ToolReturned {
                header: header("r-1"),
                call_source_id: "t-1".to_owned(),
                text: "ok".to_owned(),
                is_error: false,
            },
            CanonicalEvent::TurnEnded {
                header: header("stop-1"),
                parent_call: None,
            },
            CanonicalEvent::SessionEnded {
                header: header("end-1"),
                reason: "other".to_owned(),
            },
            CanonicalEvent::Notice {
                header: header("n-1"),
                text: "compacted".to_owned(),
            },
        ];
        for event in &events {
            round_trip(event);
        }
    }

    #[test]
    fn canonical_event_wire_shape_is_tagged_snake_case() {
        let event = CanonicalEvent::PromptSubmitted {
            header: header("p-1"),
            text: "hi".to_owned(),
        };
        let value = serde_json::to_value(&event).expect("serialize");
        assert_eq!(value["type"], "prompt_submitted");
        assert_eq!(value["header"]["session_id"], "s-1");
    }

    #[test]
    fn header_is_reachable_from_every_variant() {
        let event = CanonicalEvent::Notice {
            header: header("n-9"),
            text: String::new(),
        };
        assert_eq!(event.header().source_id, "n-9");
    }

    #[test]
    fn default_filter_round_trips_and_restricts_nothing() {
        let filter = ThreadFilter::default();
        let json = serde_json::to_string(&filter).expect("serialize");
        let back: ThreadFilter = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, filter);
        assert_eq!(back.harness, None);
        assert_eq!(back.starred, None);
        assert_eq!(back.tag, None);
    }

    #[test]
    fn hit_kind_round_trips_and_defaults_to_exact() {
        for (kind, text) in [
            (HitKind::Exact, "\"exact\""),
            (HitKind::Substring, "\"substring\""),
            (HitKind::Fuzzy, "\"fuzzy\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).expect("serialize"), text);
            let back: HitKind = serde_json::from_str(text).expect("deserialize");
            assert_eq!(back, kind);
        }
        assert_eq!(HitKind::default(), HitKind::Exact);
    }

    #[test]
    fn thread_hit_carries_its_kind_and_reads_older_hits_as_exact() {
        let hit = ThreadHit {
            thread_uid: "a1".to_owned(),
            title: "t".to_owned(),
            entry_id: 7,
            seq: 1,
            turn: 1,
            snippet: "x".to_owned(),
            at: 1,
            kind: HitKind::Fuzzy,
            ..ThreadHit::default()
        };
        let json = serde_json::to_value(&hit).expect("serialize");
        assert_eq!(json["kind"], "fuzzy");
        let back: ThreadHit = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(back, hit);

        let mut older = json;
        older.as_object_mut().expect("object").remove("kind");
        let parsed: ThreadHit = serde_json::from_value(older).expect("older hit");
        assert_eq!(parsed.kind, HitKind::Exact);
    }

    #[test]
    fn canonical_event_schema_serializes() {
        let schema = schemars::schema_for!(CanonicalEvent);
        let json = serde_json::to_string(&schema).expect("schema serializes");
        assert!(json.contains("prompt_submitted"));
    }

    #[test]
    fn a_memory_without_contexts_in_its_json_still_parses_and_an_empty_list_is_not_written() {
        let memory = Memory {
            id: 1,
            uid: "0123456789abcdef".to_owned(),
            kind: MemoryKind::Rule,
            text: "rule".to_owned(),
            basis: MemoryBasis::Observed,
            status: MemoryStatus::Accepted,
            origin: "human".to_owned(),
            proposed_by: None,
            model: None,
            created_at: 1,
            decided_at: None,
            decided_by: None,
            reviewed: true,
            sources: Vec::new(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
            scope: "repository".to_owned(),
            contexts: Vec::new(),
            member: None,
            retrieval_count: 0,
            last_retrieved_at: None,
            marks: Vec::new(),
        };
        let json = serde_json::to_value(&memory).expect("serialize");
        assert!(
            json.get("contexts").is_none(),
            "an empty list is not written: {json}"
        );
        let old: Memory = serde_json::from_value(json).expect("an old memory parses");
        assert!(old.contexts.is_empty());
    }

    #[test]
    fn a_memory_mark_is_useful_or_wrong_and_a_memory_without_marks_in_its_json_has_none() {
        assert_eq!(
            serde_json::to_value(MemoryMark::Useful).expect("serialize"),
            "useful"
        );
        assert_eq!(
            serde_json::to_value(MemoryMark::Wrong).expect("serialize"),
            "wrong"
        );
        assert!(serde_json::from_value::<MemoryMark>(serde_json::json!("helpful")).is_err());
        let summary: MemoryMarkSummary = serde_json::from_value(serde_json::json!({
            "actor": "agent:claude-code",
            "mark": "wrong",
            "at": 5
        }))
        .expect("a mark without a reason parses");
        assert_eq!(summary.reason, None);
        let json = serde_json::to_value(&summary).expect("serialize");
        assert!(json.get("reason").is_none(), "no reason is not written");
        let memory: Memory = serde_json::from_value(serde_json::json!({
            "id": 1, "uid": "0123456789abcdef", "kind": "rule", "text": "rule",
            "status": "accepted", "origin": "human", "created_at": 1, "reviewed": true,
            "sources": [], "supersedes": [], "superseded_by": [],
            "files_changed_since": [], "scope": "repository"
        }))
        .expect("an old memory parses");
        assert!(memory.marks.is_empty());
    }

    #[test]
    fn a_memory_without_a_basis_in_its_json_is_observed_and_the_basis_round_trips() {
        let mut json = serde_json::json!({
            "id": 3, "uid": "u", "kind": "fact", "text": "t", "status": "accepted",
            "origin": "human", "proposed_by": null, "model": null, "created_at": 1,
            "decided_at": null, "decided_by": null, "reviewed": true, "sources": [],
            "supersedes": [], "superseded_by": [], "files_changed_since": [], "scope": "repository"
        });
        let old: Memory = serde_json::from_value(json.clone()).expect("an old memory parses");
        assert_eq!(old.basis, MemoryBasis::Observed);
        json["basis"] = serde_json::json!("unconfirmed");
        let parsed: Memory = serde_json::from_value(json).expect("parses");
        assert_eq!(parsed.basis, MemoryBasis::Unconfirmed);
        let back = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(back["basis"], "unconfirmed");
    }

    #[test]
    fn a_memory_basis_is_one_of_three_snake_case_words() {
        for (basis, word) in [
            (MemoryBasis::Observed, "observed"),
            (MemoryBasis::Tested, "tested"),
            (MemoryBasis::Unconfirmed, "unconfirmed"),
        ] {
            assert_eq!(serde_json::to_value(basis).expect("serialize"), word);
        }
        assert!(serde_json::from_value::<MemoryBasis>(serde_json::json!("guessed")).is_err());
        assert_eq!(MemoryBasis::default(), MemoryBasis::Observed);
    }

    #[test]
    fn a_memory_with_contexts_carries_them_as_id_and_path() {
        let memory: serde_json::Value = serde_json::json!({
            "id": 2, "uid": "u", "kind": "rule", "text": "t", "status": "accepted",
            "origin": "human", "proposed_by": null, "model": null, "created_at": 1,
            "decided_at": null, "decided_by": null, "reviewed": true, "sources": [],
            "supersedes": [], "superseded_by": [], "files_changed_since": [],
            "scope": "Platform / Billing",
            "contexts": [{"id": 4, "path": "Partners"}, {"id": 7, "path": "Platform / Billing"}]
        });
        let parsed: Memory = serde_json::from_value(memory).expect("parses");
        assert_eq!(
            parsed.contexts,
            vec![
                MemoryContext {
                    id: 4,
                    path: "Partners".to_owned()
                },
                MemoryContext {
                    id: 7,
                    path: "Platform / Billing".to_owned()
                },
            ]
        );
        let back = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(
            back["contexts"],
            serde_json::json!([{"id": 4, "path": "Partners"}, {"id": 7, "path": "Platform / Billing"}])
        );
    }
}
