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
    pub tags: Vec<String>,
    pub started_at: i64,
    pub last_event_at: i64,
    /// `None` while the session is open.
    pub ended_at: Option<i64>,
    pub entry_count: u32,
    pub edit_count: u32,
    pub redaction_count: u32,
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

/// Filters for listing and searching threads. Each field is a column, not query
/// syntax (D128). `None` means no restriction.
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
    /// A thread that touched this path.
    pub path: Option<String>,
    pub tag: Option<String>,
}

/// One search result: the best-matching entry of a thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
}

/// What a link points at (D126).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Commit,
    Branch,
    File,
}

/// How a link was made: observed from Git and the events, or set by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkOrigin {
    Observed,
    Manual,
}

/// A link from a thread to Git (D126): a commit id, a branch name or a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadLink {
    pub kind: LinkKind,
    /// Commit id, branch name or repository-relative path.
    pub reference: String,
    pub origin: LinkOrigin,
}

/// A thread with its entries, edits and links, as `ReadThread` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ThreadDetail {
    pub summary: ThreadSummary,
    pub entries: Vec<ThreadEntry>,
    pub edits: Vec<ThreadEdit>,
    pub links: Vec<ThreadLink>,
    /// Redaction pattern name and how many times it fired.
    pub redactions: Vec<(String, u32)>,
    /// True when more entries follow `from_seq + limit`.
    pub has_more: bool,
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
    fn canonical_event_schema_serializes() {
        let schema = schemars::schema_for!(CanonicalEvent);
        let json = serde_json::to_string(&schema).expect("schema serializes");
        assert!(json.contains("prompt_submitted"));
    }
}
