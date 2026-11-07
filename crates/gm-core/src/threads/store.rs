//! `threads::store`: the SQLite store of agent threads, `.git/gitmaster/threads.db`
//! (design/threads.md, "Storage (D122)"). Writes go through `apply`, one transaction
//! per batch and idempotent on replay. Reads return the rows the engine shows.
//! Nothing here knows a harness: callers hand over canonical entries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use gm_schema::threads::{
    CaptureGap, EditChange, EntryKind, EntryRole, HitKind, LinkKind, LinkOrigin, SearchFacets,
    SearchGroup, SearchOrder, ThreadDetail, ThreadEdit, ThreadEntry, ThreadEntryBrief,
    ThreadFilter, ThreadHit, ThreadLink, ThreadSummary,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, ToSql, Transaction, TransactionBehavior, params,
};
use sha2::{Digest, Sha256};

use super::harness;
use super::labels::{DETAIL_RELATIONS, rel_kind_text};
use super::provenance::EditRow;
use super::query::{
    MIN_FUZZY_CHARS, TrigramQuery, build_fts_query, build_trigram_queries, dice, is_word_char,
    trigram_set,
};

/// Migrations in order; `PRAGMA user_version` counts how many have run.
const MIGRATIONS: &[&str] = &[
    SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
];
/// The index in `MIGRATIONS` of the schema that repairs stored entries with the cleaner (D133).
const REPAIR_MIGRATION: usize = 3;

/// What the cleaner makes of a stored entry (D133).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cleaned {
    /// The entry is harness markers only: it is deleted with its edits.
    Drop,
    /// The entry keeps this text.
    Text(String),
    /// The entry becomes a notice with this text.
    Notice(String),
}

/// Cleans the text of a stored entry of a harness, or says it has no rule for it. The store
/// takes it as a function, so the store never knows a harness's markers (D133).
pub type TextCleaner = fn(harness: &str, kind: EntryKind, text: &str) -> Option<Cleaned>;

/// Hits below which a search runs the next stage (D131). A store can be built with another
/// value for tests.
pub const FALLBACK_BELOW: usize = 8;
/// Candidates the fuzzy stage fetches, before they are checked in Rust. The pool is fixed, not
/// a function of the page asked for, so every page of a search sees the same ordered hits (D147).
const FUZZY_POOL: usize = 200;
/// Hits a search with `order: recent` takes from relevance before it sorts them by time (D147).
const RECENT_POOL: usize = 1000;
/// Hits a facet count reads at most, and the hits a zero-hit diagnosis counts per filter (D147).
const FACET_POOL: usize = 2000;
const DIAGNOSIS_POOL: usize = 200;
/// A zero-hit diagnosis runs only when the strict stage takes less time than this (D147).
const DIAGNOSIS_BUDGET: Duration = Duration::from_millis(200);
/// The most entries a hit carries as `context` on each side (D147).
const CONTEXT_MAX: u8 = 3;
/// Characters of a context entry's text (D147).
const CONTEXT_CHARS: usize = 600;
/// Values a facet lists, and filters a diagnosis names (D147).
const FACET_TOP: usize = 10;
const DIAGNOSIS_TOP: usize = 3;
pub(crate) const CURSOR_MALFORMED: &str = "the cursor is not valid";
pub(crate) const CURSOR_MISMATCH: &str =
    "the cursor belongs to another search; start again without a cursor";
/// Least trigram similarity between a typed word and a word of the text (D131).
const FUZZY_MIN_SIMILARITY: f32 = 0.5;
/// Words of context on each side of the match in a snippet built in Rust.
const CONTEXT_WORDS: usize = 6;
/// Words of a snippet taken from the start of a text when nothing matched in it.
const PLAIN_SNIPPET_WORDS: usize = 12;

const SCHEMA_V1: &str = "
CREATE TABLE thread (
    id INTEGER PRIMARY KEY,
    uid TEXT UNIQUE NOT NULL,
    harness TEXT NOT NULL,
    harness_version TEXT,
    session_id TEXT NOT NULL,
    worktree TEXT NOT NULL,
    branch TEXT,
    head_start TEXT,
    head_end TEXT,
    model TEXT,
    title TEXT NOT NULL,
    title_edited INTEGER NOT NULL DEFAULT 0,
    note TEXT NOT NULL DEFAULT '',
    starred INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    last_event_at INTEGER NOT NULL,
    ended_at INTEGER,
    transcript TEXT,
    cursor INTEGER NOT NULL DEFAULT 0,
    entry_count INTEGER NOT NULL DEFAULT 0,
    edit_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE (harness, session_id)
);
CREATE TABLE entry (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    turn INTEGER NOT NULL,
    kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    parent_id TEXT,
    tool TEXT,
    text TEXT NOT NULL,
    args_json TEXT,
    is_error INTEGER NOT NULL DEFAULT 0,
    truncated INTEGER NOT NULL DEFAULT 0,
    orig_bytes INTEGER,
    redactions INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL,
    UNIQUE (thread_id, source_id),
    UNIQUE (thread_id, seq)
);
CREATE TABLE edit (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    old_path TEXT,
    change TEXT NOT NULL,
    patch TEXT NOT NULL,
    added INTEGER NOT NULL,
    removed INTEGER NOT NULL,
    pre_oid TEXT,
    post_oid TEXT,
    from_input INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL
);
CREATE TABLE thread_link (
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    origin TEXT NOT NULL,
    PRIMARY KEY (thread_id, kind, ref)
);
CREATE TABLE thread_tag (
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (thread_id, tag)
);
CREATE TABLE thread_redaction (
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    pattern TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (thread_id, pattern)
);
CREATE TABLE capture_gap (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    reason TEXT NOT NULL
);
CREATE INDEX thread_last_event ON thread (last_event_at, id);
CREATE INDEX edit_thread ON edit (thread_id);
CREATE INDEX edit_entry ON edit (entry_id);
CREATE INDEX edit_path ON edit (path);
CREATE INDEX thread_tag_tag ON thread_tag (tag);
CREATE INDEX capture_gap_thread ON capture_gap (thread_id);
CREATE VIRTUAL TABLE entry_fts USING fts5(
    text, tool, content='entry', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2 tokenchars ''_''');
CREATE TRIGGER entry_fts_ai AFTER INSERT ON entry BEGIN
    INSERT INTO entry_fts (rowid, text, tool) VALUES (new.id, new.text, new.tool);
END;
CREATE TRIGGER entry_fts_ad AFTER DELETE ON entry BEGIN
    INSERT INTO entry_fts (entry_fts, rowid, text, tool) VALUES ('delete', old.id, old.text, old.tool);
END;
CREATE TRIGGER entry_fts_au AFTER UPDATE OF text, tool ON entry BEGIN
    INSERT INTO entry_fts (entry_fts, rowid, text, tool) VALUES ('delete', old.id, old.text, old.tool);
    INSERT INTO entry_fts (rowid, text, tool) VALUES (new.id, new.text, new.tool);
END;
CREATE VIRTUAL TABLE edit_fts USING fts5(
    path, content='edit', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2 tokenchars ''_''');
CREATE TRIGGER edit_fts_ai AFTER INSERT ON edit BEGIN
    INSERT INTO edit_fts (rowid, path) VALUES (new.id, new.path);
END;
CREATE TRIGGER edit_fts_ad AFTER DELETE ON edit BEGIN
    INSERT INTO edit_fts (edit_fts, rowid, path) VALUES ('delete', old.id, old.path);
END;
CREATE TRIGGER edit_fts_au AFTER UPDATE OF path ON edit BEGIN
    INSERT INTO edit_fts (edit_fts, rowid, path) VALUES ('delete', old.id, old.path);
    INSERT INTO edit_fts (rowid, path) VALUES (new.id, new.path);
END;
";

/// Schema 2 (D131): trigram indexes next to the word indexes, for substring and fuzzy
/// search. External-content tables, kept in step by triggers like the word indexes.
const SCHEMA_V2: &str = "
CREATE VIRTUAL TABLE entry_tri USING fts5(
    text, tool, content='entry', content_rowid='id', tokenize='trigram');
CREATE TRIGGER entry_tri_ai AFTER INSERT ON entry BEGIN
    INSERT INTO entry_tri (rowid, text, tool) VALUES (new.id, new.text, new.tool);
END;
CREATE TRIGGER entry_tri_ad AFTER DELETE ON entry BEGIN
    INSERT INTO entry_tri (entry_tri, rowid, text, tool) VALUES ('delete', old.id, old.text, old.tool);
END;
CREATE TRIGGER entry_tri_au AFTER UPDATE OF text, tool ON entry BEGIN
    INSERT INTO entry_tri (entry_tri, rowid, text, tool) VALUES ('delete', old.id, old.text, old.tool);
    INSERT INTO entry_tri (rowid, text, tool) VALUES (new.id, new.text, new.tool);
END;
CREATE VIRTUAL TABLE edit_tri USING fts5(
    path, content='edit', content_rowid='id', tokenize='trigram');
CREATE TRIGGER edit_tri_ai AFTER INSERT ON edit BEGIN
    INSERT INTO edit_tri (rowid, path) VALUES (new.id, new.path);
END;
CREATE TRIGGER edit_tri_ad AFTER DELETE ON edit BEGIN
    INSERT INTO edit_tri (edit_tri, rowid, path) VALUES ('delete', old.id, old.path);
END;
CREATE TRIGGER edit_tri_au AFTER UPDATE OF path ON edit BEGIN
    INSERT INTO edit_tri (edit_tri, rowid, path) VALUES ('delete', old.id, old.path);
    INSERT INTO edit_tri (rowid, path) VALUES (new.id, new.path);
END;
INSERT INTO entry_tri (entry_tri) VALUES ('rebuild');
INSERT INTO edit_tri (edit_tri) VALUES ('rebuild');
";

/// Schema 3 (D132): the word and trigram indexes cover only the conversation. Both are
/// external-content tables over the view `entry_msg` (`user` and `assistant` entries, `text`
/// only), so tool calls, tool results and notices are stored and shown but not indexed, and the
/// tool name is not indexed either. The old indexes are dropped and rebuilt from the rows the
/// view selects. `migrate` then runs `VACUUM` when the database is small (D132).
const SCHEMA_V3: &str = "
DROP TRIGGER entry_fts_ai;
DROP TRIGGER entry_fts_ad;
DROP TRIGGER entry_fts_au;
DROP TABLE entry_fts;
DROP TRIGGER entry_tri_ai;
DROP TRIGGER entry_tri_ad;
DROP TRIGGER entry_tri_au;
DROP TABLE entry_tri;
CREATE VIEW entry_msg AS SELECT id, text FROM entry WHERE kind IN ('user', 'assistant');
CREATE VIRTUAL TABLE entry_fts USING fts5(
    text, content='entry_msg', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2 tokenchars ''_''');
CREATE TRIGGER entry_fts_ai AFTER INSERT ON entry WHEN new.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER entry_fts_ad AFTER DELETE ON entry WHEN old.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_fts (entry_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER entry_fts_au AFTER UPDATE OF text ON entry
WHEN old.kind IN ('user', 'assistant') AND new.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_fts (entry_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO entry_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE VIRTUAL TABLE entry_tri USING fts5(
    text, content='entry_msg', content_rowid='id', tokenize='trigram');
CREATE TRIGGER entry_tri_ai AFTER INSERT ON entry WHEN new.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_tri (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER entry_tri_ad AFTER DELETE ON entry WHEN old.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_tri (entry_tri, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER entry_tri_au AFTER UPDATE OF text ON entry
WHEN old.kind IN ('user', 'assistant') AND new.kind IN ('user', 'assistant') BEGIN
    INSERT INTO entry_tri (entry_tri, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO entry_tri (rowid, text) VALUES (new.id, new.text);
END;
INSERT INTO entry_fts (entry_fts) VALUES ('rebuild');
INSERT INTO entry_tri (entry_tri) VALUES ('rebuild');
INSERT INTO entry_fts (entry_fts) VALUES ('optimize');
INSERT INTO entry_tri (entry_tri) VALUES ('optimize');
";

/// Schema 4 (D133): the update trigger of each index follows the kind as well as the text. A
/// row that becomes a notice leaves the index, and a notice that becomes a prompt or a reply
/// enters it. Each trigger removes the old row when the old kind was indexed and adds the
/// new one when the new kind is indexed.
const SCHEMA_V4: &str = "
DROP TRIGGER entry_fts_au;
CREATE TRIGGER entry_fts_au AFTER UPDATE OF text, kind ON entry BEGIN
    INSERT INTO entry_fts (entry_fts, rowid, text)
        SELECT 'delete', old.id, old.text WHERE old.kind IN ('user', 'assistant');
    INSERT INTO entry_fts (rowid, text)
        SELECT new.id, new.text WHERE new.kind IN ('user', 'assistant');
END;
DROP TRIGGER entry_tri_au;
CREATE TRIGGER entry_tri_au AFTER UPDATE OF text, kind ON entry BEGIN
    INSERT INTO entry_tri (entry_tri, rowid, text)
        SELECT 'delete', old.id, old.text WHERE old.kind IN ('user', 'assistant');
    INSERT INTO entry_tri (rowid, text)
        SELECT new.id, new.text WHERE new.kind IN ('user', 'assistant');
END;
";

/// Schema 5 (D143, D145): labels, their attachments, thread-to-thread links and the repository
/// path of each edit. `thread_tag` becomes labels with origin `human`, attached to their thread,
/// and is dropped; `thread_link`'s `manual` origin becomes `human`. The commit snapshot of
/// existing links is not here: it needs Git, so the engine fills it on its next open. Labels and
/// relations are stored as the user's own data, so an edit is never run by this migration.
const SCHEMA_V5: &str = "
CREATE TABLE label (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    color TEXT,
    origin TEXT NOT NULL,
    created_at INTEGER NOT NULL);
CREATE TABLE label_attach (
    label_id INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    origin TEXT NOT NULL,
    at INTEGER NOT NULL,
    subject TEXT, author TEXT, committed_at INTEGER,
    resolved TEXT,
    PRIMARY KEY(label_id, kind, ref));
CREATE TABLE thread_rel (
    from_thread INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    to_thread INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('continues', 'relates', 'supersedes')),
    origin TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY(from_thread, to_thread, kind));
ALTER TABLE thread_link ADD COLUMN subject TEXT;
ALTER TABLE thread_link ADD COLUMN author TEXT;
ALTER TABLE thread_link ADD COLUMN committed_at INTEGER;
ALTER TABLE thread_link ADD COLUMN resolved TEXT;
UPDATE thread_link SET origin = 'human' WHERE origin = 'manual';
ALTER TABLE edit ADD COLUMN rel_path TEXT;
UPDATE edit SET rel_path = (SELECT CASE
        WHEN substr(edit.path, 1, length(t.worktree) + 1) = t.worktree || '/'
        THEN substr(edit.path, length(t.worktree) + 2)
        ELSE edit.path END
    FROM thread t WHERE t.id = edit.thread_id);
CREATE INDEX edit_rel_path ON edit (rel_path);
INSERT INTO label (name, description, color, origin, created_at)
    SELECT DISTINCT tag, '', NULL, 'human', CAST(strftime('%s', 'now') AS INTEGER) * 1000
    FROM thread_tag;
INSERT INTO label_attach (label_id, kind, ref, origin, at)
    SELECT l.id, 'thread', t.uid, 'human', CAST(strftime('%s', 'now') AS INTEGER) * 1000
    FROM thread_tag g
    JOIN thread t ON t.id = g.thread_id
    JOIN label l ON l.name = g.tag;
DROP INDEX thread_tag_tag;
DROP TABLE thread_tag;
";

/// Schema 6 (D180): the member of a workspace thread. An edit and a commit link name the member
/// they belong to, `NULL` for the workspace itself and for an ordinary repository. `thread_member`
/// holds each member's HEAD at the thread's first event (`head_start`) and at the latest
/// invocation (`head_end`). Existing rows keep `NULL`: the migration does not run Git, so an edit
/// made in a member before this schema is read as the workspace's own file, as it was.
const SCHEMA_V6: &str = "
ALTER TABLE edit ADD COLUMN member TEXT;
ALTER TABLE thread_link ADD COLUMN member TEXT;
CREATE TABLE thread_member (
    thread_id INTEGER NOT NULL REFERENCES thread(id) ON DELETE CASCADE,
    member TEXT NOT NULL,
    head_start TEXT,
    head_end TEXT,
    PRIMARY KEY(thread_id, member));
";

/// Schema 7 (D190, D192): memories, their sources and their supersedes links. `memory_fts` indexes
/// the text the way `entry_fts` indexes entries (D128), with the same tokenizer and triggers.
const SCHEMA_V7: &str = "
CREATE TABLE memory (
    id          INTEGER PRIMARY KEY,
    uid         TEXT UNIQUE NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('decision','rule','lesson','fact')),
    text        TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('proposed','accepted','rejected','superseded')),
    origin      TEXT NOT NULL,
    proposed_by TEXT,
    model       TEXT,
    created_at  INTEGER NOT NULL,
    decided_at  INTEGER,
    decided_by  TEXT);
CREATE TABLE memory_source (
    memory_id  INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('thread','commit','file')),
    ref        TEXT NOT NULL,
    repo_id    TEXT,
    entry_from INTEGER,
    entry_to   INTEGER,
    blob_oid   TEXT,
    PRIMARY KEY(memory_id, kind, ref));
CREATE TABLE memory_rel (
    from_memory INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
    to_memory   INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind = 'supersedes'),
    origin      TEXT NOT NULL,
    at          INTEGER NOT NULL,
    PRIMARY KEY(from_memory, to_memory, kind));
CREATE INDEX memory_status ON memory (status);
CREATE INDEX memory_source_ref ON memory_source (kind, ref);
CREATE VIRTUAL TABLE memory_fts USING fts5(
    text, content='memory', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2 tokenchars ''_''');
CREATE TRIGGER memory_fts_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER memory_fts_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts (memory_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER memory_fts_au AFTER UPDATE OF text ON memory BEGIN
    INSERT INTO memory_fts (memory_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO memory_fts (rowid, text) VALUES (new.id, new.text);
END;
";

pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_millis(500);
/// Opening a file another process is initializing is retried this many times, this far apart.
pub(crate) const OPEN_ATTEMPTS: u32 = 200;
pub(crate) const OPEN_PAUSE: Duration = Duration::from_millis(5);
/// Entries at or above which the hook does not migrate the database (D132): it spools instead.
pub const HOOK_MIGRATION_LIMIT: i64 = 50_000;
/// Entries below which a migration ends with `VACUUM`, which takes the dropped index pages out
/// of the file at once (D132). Larger databases keep them until later checkpoints.
const VACUUM_ENTRY_LIMIT: i64 = 50_000;
/// Entries kept per thread before further ones are dropped (design/threads.md, Limits).
const DEFAULT_ENTRY_LIMIT: u32 = 20_000;
const LIMIT_GAP_REASON: &str = "entry limit reached";
const UNTITLED: &str = "Untitled thread";
const TITLE_CHARS: usize = 80;
const SNIPPET_MARK_START: &str = "\u{e000}";
const SNIPPET_MARK_END: &str = "\u{e001}";
const REWRITE_PAGE: i64 = 500;

/// Columns of a thread summary; `t` is the `thread` table. The first column is the row id,
/// which the caller strips, and the tags are read separately.
const SUMMARY_SQL: &str = "SELECT t.id, t.uid, t.harness, t.harness_version, t.session_id,
    t.worktree, t.branch, t.model, t.title, t.note, t.starred, t.started_at, t.last_event_at,
    t.ended_at, t.entry_count, t.edit_count,
    (SELECT COALESCE(SUM(r.count), 0) FROM thread_redaction r WHERE r.thread_id = t.id)
    FROM thread t";

/// The thread filter as SQL. Every parameter is always bound (NULL means no restriction),
/// so one statement serves every combination.
const FILTER_SQL: &str = " AND (:harness IS NULL OR t.harness = :harness)
    AND (:branch IS NULL OR t.branch = :branch)
    AND (:since IS NULL OR t.last_event_at >= :since)
    AND (:until IS NULL OR t.last_event_at < :until)
    AND (:starred IS NULL OR t.starred = :starred)
    AND (:has_edits IS NULL OR (t.edit_count > 0) = :has_edits)
    AND (:path IS NULL OR EXISTS (SELECT 1 FROM edit f WHERE f.thread_id = t.id
        AND (f.path = :path OR f.path GLOB :path)))
    AND (:thread IS NULL OR t.uid = :thread)
    AND (:model IS NULL OR t.model = :model)
    AND (:commit IS NULL OR EXISTS (SELECT 1 FROM thread_link k WHERE k.thread_id = t.id AND k.kind = 'commit' AND (
        lower(k.ref) = lower(:commit)
        OR (length(k.ref) >= 7 AND k.ref NOT GLOB '*[^0-9a-fA-F]*'
            AND substr(lower(:commit), 1, length(k.ref)) = lower(k.ref))
        OR (length(:commit) >= 7 AND substr(lower(k.ref), 1, length(:commit)) = lower(:commit)))))
    AND (:tag IS NULL OR EXISTS (SELECT 1 FROM label_attach a JOIN label l ON l.id = a.label_id
        WHERE a.kind = 'thread' AND a.ref = t.uid AND l.name = :tag))
    AND (:label IS NULL OR (SELECT COUNT(DISTINCT j.value) FROM json_each(:label) j
        JOIN label l ON l.name = j.value
        JOIN label_attach a ON a.label_id = l.id AND a.kind = 'thread' AND a.ref = t.uid)
        = (SELECT COUNT(DISTINCT j.value) FROM json_each(:label) j))
    AND (:related_to IS NULL OR EXISTS (SELECT 1 FROM thread_rel r JOIN thread o ON o.uid = :related_to
        WHERE ((r.from_thread = t.id AND r.to_thread = o.id) OR (r.to_thread = t.id AND r.from_thread = o.id))
        AND (:related_kind IS NULL OR r.kind = :related_kind)))";

/// The commit match of `FILTER_SQL`'s commit clause, on a link with a member: the same rule, kept
/// in step with it. Used with `ThreadFilter::commit` set, so `:commit` is bound.
const MEMBER_COMMIT_SQL: &str = " AND EXISTS (SELECT 1 FROM thread_link m WHERE m.thread_id = t.id
    AND m.kind = 'commit' AND m.member IS NOT NULL AND (
        lower(m.ref) = lower(:commit)
        OR (length(m.ref) >= 7 AND m.ref NOT GLOB '*[^0-9a-fA-F]*'
            AND substr(lower(:commit), 1, length(m.ref)) = lower(m.ref))
        OR (length(:commit) >= 7 AND substr(lower(m.ref), 1, length(:commit)) = lower(:commit))))";

/// The entry filters as SQL (D147), for the statements of a search only: `e` is the entry the
/// hit comes from. They are bound with the thread filter by `query_candidates`.
const ENTRY_FILTER_SQL: &str = " AND (:role IS NULL OR e.kind = :role)
    AND (:turn_from IS NULL OR e.turn >= :turn_from)
    AND (:turn_to IS NULL OR e.turn <= :turn_to)";

const EDIT_COLUMNS: &str = "id, entry_id, path, old_path, change, patch, added, removed, pre_oid, post_oid, from_input, at, member";

/// Errors of the thread store.
#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The thread, entry, edit or link named does not exist.
    NotFound,
    /// The request breaks a rule of the store; the message says which.
    Invalid(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "threads database: {error}"),
            Self::Io(error) => write!(f, "threads database files: {error}"),
            Self::NotFound => f.write_str("no such thread, entry or link"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::NotFound | Self::Invalid(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl StoreError {
    /// True when the database is locked by another connection (`SQLITE_BUSY` or
    /// `SQLITE_LOCKED`), so the same write may succeed later.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Sqlite(error) if is_busy(error))
    }
}

/// One batch of canonical events for one session, as the pipeline hands it over.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Batch {
    pub harness: String,
    pub session_id: String,
    pub worktree: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub model: Option<String>,
    pub harness_version: Option<String>,
    pub transcript: Option<String>,
    pub cursor: Option<u64>,
    pub started_at: i64,
    pub at: i64,
    pub ended: Option<i64>,
    pub entries: Vec<NewEntry>,
    /// `(at, reason)` for events dropped before storage. A repeated row is not added again.
    pub gaps: Vec<(i64, String)>,
}

/// One entry of a batch, redacted and capped by the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewEntry {
    pub source_id: String,
    pub kind: EntryKind,
    pub parent_id: Option<String>,
    pub tool: Option<String>,
    pub text: String,
    pub args_json: Option<String>,
    pub is_error: bool,
    pub truncated: bool,
    pub orig_bytes: Option<u64>,
    /// Total of `hits`, as stored on the entry.
    pub redactions: u32,
    /// Per-pattern counts of the redactions made in this entry's fields. Added to the thread
    /// only when the entry is newly inserted, so a replayed batch counts nothing twice.
    pub hits: Vec<(String, u32)>,
    pub at: i64,
    pub edit: Option<NewEdit>,
}

/// A file edit carried by an entry.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewEdit {
    pub path: String,
    pub old_path: Option<String>,
    pub change: EditChange,
    pub patch: String,
    pub added: u32,
    pub removed: u32,
    pub pre_oid: Option<String>,
    pub post_oid: Option<String>,
    pub from_input: bool,
    /// Redactions made in the path and the patch. Added when the edit is stored as a new row.
    pub hits: Vec<(String, u32)>,
    /// The workspace member the path is in, decided by the pipeline when it reads the edit (D180).
    #[serde(default)]
    pub member: Option<String>,
}

/// A member of a workspace thread and the HEADs it was seen at (D180).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadMember {
    /// The member's folder, relative to the workspace root.
    pub member: String,
    /// The member's HEAD at the thread's first event.
    pub head_start: Option<String>,
    /// The member's HEAD at the latest invocation.
    pub head_end: Option<String>,
}

/// What `apply` did with a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub uid: String,
    /// Entries stored by this call.
    pub inserted: u32,
    /// Entries already stored, which changed nothing.
    pub duplicates: u32,
}

/// Metadata a person edits. `None` leaves the field as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaUpdate {
    /// Setting a title marks it as edited, so a later prompt does not replace it.
    pub title: Option<String>,
    pub note: Option<String>,
    pub starred: Option<bool>,
}

/// What the hook process gets from `ThreadStore::open_for_hook` (D132).
pub enum HookStore {
    /// The store, opened and migrated as `ThreadStore::open` does it.
    Ready(ThreadStore),
    /// The database needs a migration that the hook must not run. This handle is read-only: the
    /// caller reads through it and spools its batch, and never writes.
    Deferred(ThreadStore),
}

/// `open_for_hook` with the entry count from which the hook defers a migration.
fn open_for_hook_below(git_common_dir: &Path, limit: i64) -> Result<HookStore, StoreError> {
    let path = git_common_dir.join("gitmaster").join("threads.db");
    if !path.is_file() {
        return Ok(HookStore::Ready(ThreadStore::open(git_common_dir)?));
    }
    let reader = ThreadStore::open_read_only(git_common_dir)?;
    let version: i64 = reader
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let latest = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if version >= latest || count_entries(&reader.conn, limit)? < limit {
        drop(reader);
        return Ok(HookStore::Ready(ThreadStore::open(git_common_dir)?));
    }
    Ok(HookStore::Deferred(reader))
}

pub struct ThreadStore {
    pub(super) conn: Connection,
    /// Entries kept per thread; tests lower it.
    entry_limit: u32,
    /// Hits below which search runs its next stage; tests lower it.
    fallback_below: usize,
}

impl ThreadStore {
    /// Opens (creating `gitmaster/threads.db` under `git_common_dir` when missing) and migrates
    /// the store. The file and its WAL and shared-memory files are private (mode `0600`).
    pub fn open(git_common_dir: &Path) -> Result<Self, StoreError> {
        Self::open_with(git_common_dir, harness::clean_stored)
    }

    /// `open` with the cleaner the schema 4 repair runs (D133).
    fn open_with(git_common_dir: &Path, clean: TextCleaner) -> Result<Self, StoreError> {
        let dir = git_common_dir.join("gitmaster");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("threads.db");
        for file in [
            path.clone(),
            dir.join("threads.db-wal"),
            dir.join("threads.db-shm"),
        ] {
            create_private(&file)?;
        }
        let conn = open_connection(&path, clean)?;
        Ok(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
            fallback_below: FALLBACK_BELOW,
        })
    }

    /// The store for the hook process (D132). A database that needs a migration and holds
    /// `HOOK_MIGRATION_LIMIT` entries or more is not migrated here: the hook gets a read-only
    /// handle and spools its batch. Any other database is opened as `open` does.
    pub fn open_for_hook(git_common_dir: &Path) -> Result<HookStore, StoreError> {
        open_for_hook_below(git_common_dir, HOOK_MIGRATION_LIMIT)
    }

    /// A read-only handle on an existing database, which runs no migration and writes nothing.
    /// It serves the hook's deferred path (D132) and tests. Fails when the file does not exist.
    pub fn open_read_only(git_common_dir: &Path) -> Result<Self, StoreError> {
        let path = git_common_dir.join("gitmaster").join("threads.db");
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
            fallback_below: FALLBACK_BELOW,
        })
    }

    /// A read-only handle on the store of another repository's workspace (D180 (2)). `None` when
    /// the file is missing or was written by another schema: such a store is skipped, never
    /// created or migrated. Nothing is written.
    pub fn open_workspace_read_only(git_common_dir: &Path) -> Result<Option<Self>, StoreError> {
        let path = git_common_dir.join("gitmaster").join("threads.db");
        if !path.is_file() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let latest = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
        if version != latest {
            return Ok(None);
        }
        Ok(Some(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
            fallback_below: FALLBACK_BELOW,
        }))
    }

    /// A store that lives only in memory (tests).
    pub fn open_memory() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()?;
        prepare(&mut conn, harness::clean_stored)?;
        Ok(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
            fallback_below: FALLBACK_BELOW,
        })
    }

    /// The uid of a thread: the first 16 hex digits of SHA-256 over harness, a NUL byte and
    /// the session id.
    #[must_use]
    pub fn thread_uid(harness: &str, session_id: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hasher = Sha256::new();
        hasher.update(harness.as_bytes());
        hasher.update([0_u8]);
        hasher.update(session_id.as_bytes());
        let digest = hasher.finalize();
        let mut uid = String::with_capacity(16);
        for byte in &digest[..8] {
            uid.push(char::from(HEX[usize::from(byte >> 4)]));
            uid.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        uid
    }

    /// Stores a batch in one transaction. Entries already stored (same `source_id`) change
    /// nothing, so applying a batch twice, or a prefix and then the whole batch, gives the
    /// same database as one pass.
    pub fn apply(&mut self, batch: &Batch) -> Result<Applied, StoreError> {
        let uid = Self::thread_uid(&batch.harness, &batch.session_id);
        let entry_limit = i64::from(self.entry_limit);
        let tx = begin(&mut self.conn)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM thread WHERE harness = ?1 AND session_id = ?2",
                params![batch.harness, batch.session_id],
                |row| row.get(0),
            )
            .optional()?;
        let thread = match existing {
            Some(id) => id,
            None => insert_thread(&tx, &uid, batch)?,
        };
        touch_thread(&tx, thread, batch)?;

        let mut state = ApplyState::load(&tx, thread)?;
        let mut applied = Applied {
            uid,
            inserted: 0,
            duplicates: 0,
        };
        for entry in &batch.entries {
            if stored(&tx, thread, &entry.source_id)? {
                applied.duplicates += 1;
            } else if state.entry_count >= entry_limit {
                record_limit_gap(&tx, thread, batch.at)?;
            } else {
                insert_entry(&tx, thread, entry, &mut state)?;
                applied.inserted += 1;
            }
        }
        for (at, reason) in &batch.gaps {
            record_gap(&tx, thread, *at, reason)?;
        }
        state.write_counters(&tx, thread)?;
        tx.commit()?;
        Ok(applied)
    }

    /// Thread summaries, newest `last_event_at` first, then by uid. `before` is the
    /// `(last_event_at, uid)` of the last row of the previous page.
    pub fn list_threads(
        &self,
        filter: &ThreadFilter,
        before: Option<(i64, String)>,
        limit: u32,
    ) -> Result<Vec<ThreadSummary>, StoreError> {
        self.list_rows(filter, before, limit, "")
    }

    /// The threads linked to `commit` through a member of this workspace (D180 (2)): a commit
    /// link with a member, whose commit matches as `list_threads` matches a commit filter. The
    /// first page only; the caller decides what to do with the rest.
    pub fn list_member_commit_threads(
        &self,
        commit: &str,
        limit: u32,
    ) -> Result<Vec<ThreadSummary>, StoreError> {
        let filter = ThreadFilter {
            commit: Some(commit.to_owned()),
            ..ThreadFilter::default()
        };
        self.list_rows(&filter, None, limit, MEMBER_COMMIT_SQL)
    }

    /// The listing behind `list_threads`, with `extra` (a condition that starts with `AND`) added
    /// to the filter.
    fn list_rows(
        &self,
        filter: &ThreadFilter,
        before: Option<(i64, String)>,
        limit: u32,
        extra: &str,
    ) -> Result<Vec<ThreadSummary>, StoreError> {
        let (before_at, before_uid) = match before {
            Some((at, uid)) => (Some(at), Some(uid)),
            None => (None, None),
        };
        let limit = i64::from(limit);
        let sql = format!(
            "{SUMMARY_SQL} WHERE 1 = 1{FILTER_SQL}{extra}
             AND (:before_at IS NULL OR (t.last_event_at, t.uid) < (:before_at, :before_uid))
             ORDER BY t.last_event_at DESC, t.uid DESC LIMIT :limit"
        );
        let values = FilterValues::of(filter);
        let mut bindings = thread_bindings(filter, &values);
        bindings.push((":before_at", &before_at));
        bindings.push((":before_uid", &before_uid));
        bindings.push((":limit", &limit));
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement
            .query_map(bindings.as_slice(), summary_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut summaries = Vec::with_capacity(rows.len());
        for (id, mut summary) in rows {
            summary.tags = tags_of(&self.conn, id)?;
            summaries.push(summary);
        }
        Ok(summaries)
    }

    /// Hits for the user's text, best first, one per entry (D128, D131). Stage 1 is the
    /// strict word-start search (`query::build_fts_query`). When it returns fewer than
    /// `fallback_below` hits, stage 2 adds substring matches over the trigram index, and
    /// when still short, stage 3 adds close misspellings verified in Rust. Each stage
    /// appends only what is new and never exceeds `limit`. Filters apply to every stage,
    /// the entry filters (`role`, `turn_from`, `turn_to`) included (D147).
    pub fn search(
        &self,
        user_query: &str,
        filter: &ThreadFilter,
        limit: u32,
    ) -> Result<Vec<ThreadHit>, StoreError> {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        self.stream(user_query, filter, limit)
    }

    /// One page of a search, or of a listing when the query is empty (D147). The hits of a
    /// search are a fixed order: a page is the window of that order that its cursor names, so
    /// pages neither repeat nor skip a hit. A cursor made for another request is refused.
    pub fn search_page(&self, request: &SearchRequest<'_>) -> Result<SearchPage, StoreError> {
        let query = request.query.trim();
        let digest = request_digest(query, request.filter, request.group, request.order);
        let position = request
            .cursor
            .map(|text| decode_cursor(text, &digest))
            .transpose()?;
        let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
        let context = usize::from(request.context.min(CONTEXT_MAX));
        let first_page = request.cursor.is_none();
        let mut page = if query.is_empty() {
            self.list_page(request.filter, position, limit, &digest)?
        } else {
            self.match_page(request, query, position.as_ref(), limit, context, &digest)?
        };
        if request.facets {
            page.facets = Some(self.facets(query, request.filter)?);
        }
        if page.hits.is_empty() && first_page {
            page.diagnosis = self.diagnosis(query, request.filter, request.group)?;
        }
        Ok(page)
    }

    /// The page of a search: hits from `ranked`, windowed by the offset in the cursor.
    fn match_page(
        &self,
        request: &SearchRequest<'_>,
        query: &str,
        position: Option<&Position>,
        limit: usize,
        context: usize,
        digest: &str,
    ) -> Result<SearchPage, StoreError> {
        let offset = match position {
            None => 0,
            Some(&Position::Offset(offset)) => offset,
            Some(Position::Keyset(..)) => return Err(invalid(CURSOR_MALFORMED)),
        };
        let relevance = request.order == SearchOrder::Relevance;
        // Relevance pages need the window only; recency sorts a fixed pool, so every page sees
        // the same hits to sort.
        let wanted = if relevance {
            offset.saturating_add(limit).saturating_add(1)
        } else {
            RECENT_POOL
        };
        let mut ranked = self.ranked(query, request.filter, request.group, wanted)?;
        if !relevance {
            ranked.sort_by(|a, b| b.at.cmp(&a.at).then(a.entry_id.cmp(&b.entry_id)));
        }
        let has_more = ranked.len() > offset.saturating_add(limit);
        let next_cursor =
            has_more.then(|| encode_cursor(digest, &Position::Offset(offset + limit)));
        let mut hits: Vec<ThreadHit> = ranked.into_iter().skip(offset).take(limit).collect();
        self.fill_metadata(&mut hits)?;
        self.attach_context(&mut hits, context)?;
        Ok(SearchPage {
            hits,
            has_more,
            next_cursor,
            ..SearchPage::default()
        })
    }

    /// The page of a listing (empty query): threads by `last_event_at`, newest first, from the
    /// `(last_event_at, uid)` of the cursor (D147).
    fn list_page(
        &self,
        filter: &ThreadFilter,
        position: Option<Position>,
        limit: usize,
        digest: &str,
    ) -> Result<SearchPage, StoreError> {
        let before = match position {
            None => None,
            Some(Position::Keyset(at, uid)) => Some((at, uid)),
            Some(Position::Offset(_)) => return Err(invalid(CURSOR_MALFORMED)),
        };
        let fetch = u32::try_from(limit.saturating_add(1)).unwrap_or(u32::MAX);
        let mut threads = self.list_threads(filter, before, fetch)?;
        let has_more = threads.len() > limit;
        threads.truncate(limit);
        let next_cursor = if has_more {
            threads.last().map(|thread| {
                encode_cursor(
                    digest,
                    &Position::Keyset(thread.last_event_at, thread.uid.clone()),
                )
            })
        } else {
            None
        };
        let hits = threads.into_iter().map(listing_hit).collect();
        Ok(SearchPage {
            hits,
            has_more,
            next_cursor,
            ..SearchPage::default()
        })
    }

    /// The first `wanted` hits of a query in the order of its page, grouped as asked. The
    /// entry stream is read again with twice the room while grouping leaves too few, and it
    /// stops growing when the data runs out.
    fn ranked(
        &self,
        query: &str,
        filter: &ThreadFilter,
        group: SearchGroup,
        wanted: usize,
    ) -> Result<Vec<ThreadHit>, StoreError> {
        let mut cap = wanted.max(1);
        let mut previous = None;
        loop {
            let entries = self.stream(query, filter, cap)?;
            let exhausted = previous.is_some_and(|before| entries.len() <= before);
            let mut grouped = group_hits(&entries, group);
            if grouped.len() >= wanted || exhausted {
                grouped.truncate(wanted);
                return Ok(grouped);
            }
            previous = Some(entries.len());
            cap = cap.saturating_mul(2);
        }
    }

    /// The entry hits of a query, at most `cap`, best first: the strict stage, then the
    /// stages that are short of `fallback_below` hits (D131).
    fn stream(
        &self,
        user_query: &str,
        filter: &ThreadFilter,
        cap: usize,
    ) -> Result<Vec<ThreadHit>, StoreError> {
        let mut hits = Hits::new(cap);
        if let Some(fts) = build_fts_query(user_query) {
            self.exact_stage(&fts, filter, &mut hits)?;
        }
        let Some(trigram) = build_trigram_queries(user_query) else {
            return Ok(hits.list);
        };
        if hits.list.len() < self.fallback_below {
            self.substring_stage(&trigram, filter, &mut hits)?;
        }
        if hits.list.len() < self.fallback_below && !trigram.fuzzy.is_empty() {
            self.fuzzy_stage(&trigram, filter, &mut hits)?;
        }
        Ok(hits.list)
    }

    /// Fills the thread's metadata of each hit: harness, branch, start, edits and tags (D147).
    fn fill_metadata(&self, hits: &mut [ThreadHit]) -> Result<(), StoreError> {
        let mut cache: HashMap<String, ThreadSummary> = HashMap::new();
        for hit in hits {
            if !cache.contains_key(&hit.thread_uid) {
                let summary = summary_by_uid(&self.conn, &hit.thread_uid)?;
                cache.insert(hit.thread_uid.clone(), summary);
            }
            let summary = &cache[&hit.thread_uid];
            hit.harness.clone_from(&summary.harness);
            hit.branch.clone_from(&summary.branch);
            hit.started_at = summary.started_at;
            hit.edit_count = summary.edit_count;
            hit.tags.clone_from(&summary.tags);
        }
        Ok(())
    }

    /// Adds to each hit up to `context` user and assistant entries before it and after it in
    /// the same thread, in thread order, with their text capped (D147).
    fn attach_context(&self, hits: &mut [ThreadHit], context: usize) -> Result<(), StoreError> {
        if context == 0 {
            return Ok(());
        }
        let limit = i64::try_from(context).unwrap_or(i64::MAX);
        let mut earlier = self.conn.prepare(
            "SELECT e.seq, e.kind, e.text FROM entry e JOIN thread t ON t.id = e.thread_id
             WHERE t.uid = ?1 AND e.seq < ?2 AND e.kind IN ('user', 'assistant')
             ORDER BY e.seq DESC LIMIT ?3",
        )?;
        let mut later = self.conn.prepare(
            "SELECT e.seq, e.kind, e.text FROM entry e JOIN thread t ON t.id = e.thread_id
             WHERE t.uid = ?1 AND e.seq > ?2 AND e.kind IN ('user', 'assistant')
             ORDER BY e.seq ASC LIMIT ?3",
        )?;
        for hit in hits {
            let mut before = brief_rows(&mut earlier, &hit.thread_uid, hit.seq, limit)?;
            before.reverse();
            let after = brief_rows(&mut later, &hit.thread_uid, hit.seq, limit)?;
            hit.context = before.into_iter().chain(after).collect();
        }
        Ok(())
    }

    /// The most common branches, harnesses and edited paths among the threads a search matches
    /// (D147). The query's threads come from the strict and fallback stages, at most
    /// `FACET_POOL` hits, so `limit` and the cursor do not apply.
    fn facets(&self, query: &str, filter: &ThreadFilter) -> Result<SearchFacets, StoreError> {
        let uids = if query.is_empty() {
            self.matching_uids(filter)?
        } else {
            self.stream(query, filter, FACET_POOL)?
                .into_iter()
                .map(|hit| hit.thread_uid)
                .collect()
        };
        let mut harness: BTreeMap<String, u32> = BTreeMap::new();
        let mut branch: BTreeMap<String, u32> = BTreeMap::new();
        let mut statement = self
            .conn
            .prepare("SELECT uid, harness, branch FROM thread")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (uid, thread_harness, thread_branch) = row?;
            if uids.contains(&uid) {
                *harness.entry(thread_harness).or_default() += 1;
                if let Some(name) = thread_branch {
                    *branch.entry(name).or_default() += 1;
                }
            }
        }
        let mut path: BTreeMap<String, u32> = BTreeMap::new();
        let mut statement = self
            .conn
            .prepare("SELECT t.uid, f.path FROM edit f JOIN thread t ON t.id = f.thread_id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (uid, edited) = row?;
            if uids.contains(&uid) {
                *path.entry(edited).or_default() += 1;
            }
        }
        let mut label: BTreeMap<String, u32> = BTreeMap::new();
        let mut statement = self.conn.prepare(
            "SELECT a.ref, l.name FROM label_attach a JOIN label l ON l.id = a.label_id
             WHERE a.kind = 'thread'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (uid, name) = row?;
            if uids.contains(&uid) {
                *label.entry(name).or_default() += 1;
            }
        }
        Ok(SearchFacets {
            branch: top_counts(branch),
            harness: top_counts(harness),
            path: top_counts(path),
            label: top_counts(label),
        })
    }

    /// The uids of the threads the filter matches, with no query.
    fn matching_uids(&self, filter: &ThreadFilter) -> Result<HashSet<String>, StoreError> {
        let sql = format!("SELECT t.uid FROM thread t WHERE 1 = 1{FILTER_SQL}");
        let values = FilterValues::of(filter);
        let bindings = thread_bindings(filter, &values);
        let mut statement = self.conn.prepare(&sql)?;
        let uids = statement
            .query_map(bindings.as_slice(), |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<String>>>()?;
        Ok(uids)
    }

    /// Why a search with filters found nothing (D147): for each set filter, how many hits the
    /// search would have with that filter removed alone, most first, only those above zero.
    /// Skipped (`None`) when no filter is set, or when the strict stage alone takes longer than
    /// `DIAGNOSIS_BUDGET`. Each filter is counted once, over at most `DIAGNOSIS_POOL` hits.
    fn diagnosis(
        &self,
        query: &str,
        filter: &ThreadFilter,
        group: SearchGroup,
    ) -> Result<Option<Vec<(String, u32)>>, StoreError> {
        let relaxed = set_filters(filter);
        if relaxed.is_empty() {
            return Ok(None);
        }
        if let Some(fts) = build_fts_query(query) {
            let started = Instant::now();
            self.exact_stage(&fts, filter, &mut Hits::new(DIAGNOSIS_POOL))?;
            if started.elapsed() > DIAGNOSIS_BUDGET {
                return Ok(None);
            }
        }
        let limit = u32::try_from(DIAGNOSIS_POOL).unwrap_or(u32::MAX);
        let mut counts = Vec::new();
        for (name, without) in relaxed {
            let hits = if query.is_empty() {
                self.list_threads(&without, None, limit)?.len()
            } else {
                group_hits(&self.stream(query, &without, DIAGNOSIS_POOL)?, group).len()
            };
            if hits > 0 {
                counts.push((name.to_owned(), u32::try_from(hits).unwrap_or(u32::MAX)));
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts.truncate(DIAGNOSIS_TOP);
        Ok(Some(counts))
    }

    /// Stage 1: word-start matches, best first.
    fn exact_stage(
        &self,
        fts: &str,
        filter: &ThreadFilter,
        hits: &mut Hits,
    ) -> Result<(), StoreError> {
        if hits.room() == 0 {
            return Ok(());
        }
        let statements = stage_statements("entry_fts", "edit_fts", true, false);
        for candidate in self.query_candidates(&statements, fts, filter, hits.room())? {
            hits.add(candidate.hit);
        }
        Ok(())
    }

    /// Stage 2: the typed words inside words, over the trigram index, best first.
    fn substring_stage(
        &self,
        trigram: &TrigramQuery,
        filter: &ThreadFilter,
        hits: &mut Hits,
    ) -> Result<(), StoreError> {
        if hits.room() == 0 {
            return Ok(());
        }
        let statements = stage_statements("entry_tri", "edit_tri", true, true);
        // The seen hits may take some of the candidates, so the room is added to the cap.
        let cap = hits.room() + hits.seen.len();
        let candidates = self.query_candidates(&statements, &trigram.substring, filter, cap)?;
        for candidate in candidates {
            if hits.seen.contains(&candidate.hit.entry_id) {
                continue;
            }
            let mut hit = candidate.hit;
            hit.kind = HitKind::Substring;
            if !hit.snippet.contains(SNIPPET_MARK_START) {
                hit.snippet = candidate
                    .body
                    .as_deref()
                    .map_or_else(String::new, |body| substring_snippet(body, &trigram.tokens));
            }
            hits.add(hit);
        }
        Ok(())
    }

    /// Stage 3: close misspellings. The trigram OR query only gathers candidates; each one
    /// is kept when every typed word has a word of its text at similarity 0.5 or more.
    fn fuzzy_stage(
        &self,
        trigram: &TrigramQuery,
        filter: &ThreadFilter,
        hits: &mut Hits,
    ) -> Result<(), StoreError> {
        if hits.room() == 0 {
            return Ok(());
        }
        let probes: Vec<&str> = trigram
            .tokens
            .iter()
            .filter(|token| !token.contains(' ') && token.chars().count() >= MIN_FUZZY_CHARS)
            .map(String::as_str)
            .collect();
        let statements = stage_statements("entry_tri", "edit_tri", false, true);
        let candidates = self.query_candidates(&statements, &trigram.fuzzy, filter, FUZZY_POOL)?;
        let mut scored: Vec<(f32, f64, ThreadHit)> = Vec::new();
        for candidate in candidates {
            if hits.seen.contains(&candidate.hit.entry_id) {
                continue;
            }
            let Some(body) = candidate.body.as_deref() else {
                continue;
            };
            if let Some((score, span)) = fuzzy_match(body, &probes) {
                let snippet = marked_snippet(body, span);
                let hit = ThreadHit {
                    snippet,
                    kind: HitKind::Fuzzy,
                    ..candidate.hit
                };
                scored.push((score, candidate.rank, hit));
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
        for (_, _, hit) in scored {
            hits.add(hit);
        }
        Ok(())
    }

    /// Runs one stage's statements (entry index, then edit index) with the query and the
    /// filter, and returns the rows best first. `limit` is per statement.
    fn query_candidates(
        &self,
        statements: &[String],
        query: &str,
        filter: &ThreadFilter,
        limit: usize,
    ) -> Result<Vec<Candidate>, StoreError> {
        let fts = query.to_owned();
        let start = SNIPPET_MARK_START.to_owned();
        let end = SNIPPET_MARK_END.to_owned();
        let limit_value = i64::try_from(limit).unwrap_or(i64::MAX);
        let role = filter.role.map(entry_role_text);
        let turn_from = filter.turn_from.map(i64::from);
        let turn_to = filter.turn_to.map(i64::from);
        let values = FilterValues::of(filter);
        let mut bindings = thread_bindings(filter, &values);
        bindings.push((":role", &role));
        bindings.push((":turn_from", &turn_from));
        bindings.push((":turn_to", &turn_to));
        bindings.push((":fts", &fts));
        // rusqlite refuses a value for a name the statement does not use, so the markers
        // are bound only for a stage that asks for snippets.
        if statements.iter().any(|sql| sql.contains(":mark_start")) {
            bindings.push((":mark_start", &start));
            bindings.push((":mark_end", &end));
        }
        bindings.push((":limit", &limit_value));

        let mut found = Vec::new();
        for sql in statements {
            let mut statement = self.conn.prepare(sql)?;
            let rows = statement.query_map(bindings.as_slice(), candidate_row)?;
            for row in rows {
                found.push(row?);
            }
        }
        found.sort_by(|a, b| a.rank.total_cmp(&b.rank));
        Ok(found)
    }

    /// A thread with its entries from `from_seq` on (at most `limit`), every edit of those
    /// entries, its links and its redaction counts.
    pub fn read_thread(
        &self,
        uid: &str,
        from_seq: u32,
        limit: u32,
    ) -> Result<ThreadDetail, StoreError> {
        let thread = thread_id(&self.conn, uid)?;
        let summary = summary_of(&self.conn, thread)?;
        let limit = i64::from(limit);
        let keep = usize::try_from(limit).unwrap_or(usize::MAX);

        let mut entries = {
            let mut statement = self.conn.prepare_cached(
                "SELECT id, seq, turn, kind, source_id, parent_id, tool, text, args_json,
                    is_error, truncated, orig_bytes, redactions, at
                 FROM entry WHERE thread_id = ?1 AND seq >= ?2 ORDER BY seq LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![thread, i64::from(from_seq), limit.saturating_add(1)],
                    entry_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let has_more = entries.len() > keep;
        entries.truncate(keep);

        let edits = match (entries.first(), entries.last()) {
            (Some(first), Some(last)) => {
                let sql = format!(
                    "SELECT {EDIT_COLUMNS} FROM edit
                     WHERE entry_id IN (SELECT id FROM entry WHERE thread_id = ?1 AND seq BETWEEN ?2 AND ?3)
                     ORDER BY (SELECT seq FROM entry WHERE entry.id = edit.entry_id), id"
                );
                let mut statement = self.conn.prepare(&sql)?;
                statement
                    .query_map(params![thread, first.seq, last.seq], edit_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            _ => Vec::new(),
        };

        let links = {
            let mut statement = self.conn.prepare_cached(
                "SELECT kind, ref, origin, member FROM thread_link WHERE thread_id = ?1 ORDER BY kind, ref",
            )?;
            statement
                .query_map(params![thread], link_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let redactions = {
            let mut statement = self.conn.prepare_cached(
                "SELECT pattern, count FROM thread_redaction WHERE thread_id = ?1 ORDER BY pattern",
            )?;
            statement
                .query_map(params![thread], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<(String, u32)>>>()?
        };

        let relations = self.thread_relation_rows(uid, DETAIL_RELATIONS)?;

        Ok(ThreadDetail {
            summary,
            entries,
            edits,
            links,
            relations,
            redactions,
            has_more,
        })
    }

    /// The edits of `path` that could explain a line (D144), newest first, with their patches. An
    /// edit counts when it was made by `before` (the commit's author date) or when its blob is
    /// `blob`, the file's blob in the commit, which needs no date. A path is the repository path
    /// the edit's `rel_path` holds (D143, schema v5).
    /// The caller passes `limit` plus one to learn whether the list was cut.
    pub fn path_edits(
        &self,
        path: &str,
        before: i64,
        blob: Option<&str>,
        limit: u32,
    ) -> Result<Vec<EditRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT ed.id, t.uid, e.seq, ed.path, ed.at, ed.post_oid, ed.from_input, ed.patch,
                    ed.entry_id
             FROM edit ed
             JOIN thread t ON t.id = ed.thread_id
             JOIN entry e ON e.id = ed.entry_id
             WHERE ed.rel_path = ?1
               AND (ed.at <= ?2 OR COALESCE(ed.post_oid = ?3, 0))
             ORDER BY COALESCE(ed.post_oid = ?3, 0) DESC, ed.at DESC, ed.id DESC
             LIMIT ?4",
        )?;
        let rows = statement
            .query_map(params![path, before, blob, i64::from(limit)], |row| {
                Ok(EditRow {
                    edit_id: row.get(0)?,
                    thread_uid: row.get(1)?,
                    seq: row.get(2)?,
                    path: row.get(3)?,
                    at: row.get(4)?,
                    post_oid: row.get(5)?,
                    from_input: row.get(6)?,
                    patch: row.get(7)?,
                    entry_id: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The threads linked to commit `commit` (full id), with how each link was made (D126).
    pub fn commit_links(&self, commit: &str) -> Result<Vec<(String, LinkOrigin)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT t.uid, l.origin FROM thread_link l JOIN thread t ON t.id = l.thread_id
             WHERE l.kind = 'commit' AND l.ref = ?1 ORDER BY t.uid",
        )?;
        let rows = statement
            .query_map(params![commit], |row| {
                Ok((row.get(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(uid, origin)| Some((uid, parse_origin(&origin)?)))
            .collect())
    }

    /// The metadata of thread `uid`, as listings show it.
    pub fn thread_summary(&self, uid: &str) -> Result<ThreadSummary, StoreError> {
        summary_by_uid(&self.conn, uid)
    }

    /// The user and assistant entries around entry `seq` of thread `uid`, up to `limit` on each
    /// side, earlier ones first (D147's context, used for provenance).
    pub fn entries_around(
        &self,
        uid: &str,
        seq: u32,
        limit: u32,
    ) -> Result<Vec<ThreadEntryBrief>, StoreError> {
        let limit = i64::from(limit);
        let mut earlier = self.conn.prepare(
            "SELECT e.seq, e.kind, e.text FROM entry e JOIN thread t ON t.id = e.thread_id
             WHERE t.uid = ?1 AND e.seq < ?2 AND e.kind IN ('user', 'assistant')
             ORDER BY e.seq DESC LIMIT ?3",
        )?;
        let mut later = self.conn.prepare(
            "SELECT e.seq, e.kind, e.text FROM entry e JOIN thread t ON t.id = e.thread_id
             WHERE t.uid = ?1 AND e.seq > ?2 AND e.kind IN ('user', 'assistant')
             ORDER BY e.seq ASC LIMIT ?3",
        )?;
        let mut before = brief_rows(&mut earlier, uid, seq, limit)?;
        before.reverse();
        let after = brief_rows(&mut later, uid, seq, limit)?;
        Ok(before.into_iter().chain(after).collect())
    }

    /// One edit, with its patch.
    pub fn read_edit(&self, id: i64) -> Result<ThreadEdit, StoreError> {
        let sql = format!("SELECT {EDIT_COLUMNS} FROM edit WHERE id = ?1");
        self.conn
            .query_row(&sql, params![id], edit_row)
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Sets the metadata of a thread. Entry text is never changed here.
    pub fn update_meta(&mut self, uid: &str, meta: &MetaUpdate) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        if let Some(title) = &meta.title {
            let title = title.trim();
            if title.is_empty() {
                return Err(invalid("a thread needs a title"));
            }
            tx.execute(
                "UPDATE thread SET title = ?2, title_edited = 1 WHERE id = ?1",
                params![thread, title],
            )?;
        }
        if let Some(note) = &meta.note {
            tx.execute(
                "UPDATE thread SET note = ?2 WHERE id = ?1",
                params![thread, note],
            )?;
        }
        if let Some(starred) = meta.starred {
            tx.execute(
                "UPDATE thread SET starred = ?2 WHERE id = ?1",
                params![thread, starred],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Adds a link, or changes the origin of the link already there.
    pub fn link(&mut self, uid: &str, link: &ThreadLink) -> Result<(), StoreError> {
        if link.reference.trim().is_empty() {
            return Err(invalid("a link needs a reference"));
        }
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        tx.execute(
            "INSERT INTO thread_link (thread_id, kind, ref, origin, member) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (thread_id, kind, ref) DO UPDATE SET origin = excluded.origin,
                member = COALESCE(excluded.member, thread_link.member)",
            params![
                thread,
                link_kind_text(link.kind),
                link.reference,
                origin_text(link.origin),
                link.member
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a link. `NotFound` when the thread has no such link.
    pub fn unlink(&mut self, uid: &str, kind: LinkKind, reference: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        let removed = tx.execute(
            "DELETE FROM thread_link WHERE thread_id = ?1 AND kind = ?2 AND ref = ?3",
            params![thread, link_kind_text(kind), reference],
        )?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes a thread with its entries, edits, links, relations, label attachments and gaps.
    /// The text is removed from the search index too, so `checkpoint` then leaves it out of the
    /// file.
    pub fn delete_thread(&mut self, uid: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        tx.execute(
            "DELETE FROM label_attach WHERE kind = 'thread' AND ref = ?1",
            params![uid],
        )?;
        tx.execute("DELETE FROM thread WHERE id = ?1", params![thread])?;
        purge_search_index(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes one entry with its edits, and fixes the thread's counters.
    pub fn delete_entry(&mut self, entry_id: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread: i64 = tx
            .query_row(
                "SELECT thread_id FROM entry WHERE id = ?1",
                params![entry_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        tx.execute("DELETE FROM entry WHERE id = ?1", params![entry_id])?;
        tx.execute(
            "UPDATE thread SET
                entry_count = (SELECT COUNT(*) FROM entry WHERE thread_id = ?1),
                edit_count = (SELECT COUNT(*) FROM edit WHERE thread_id = ?1)
             WHERE id = ?1",
            params![thread],
        )?;
        purge_search_index(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// The transcript offset and path stored for a session, if the session is known.
    pub fn cursor(
        &self,
        harness: &str,
        session_id: &str,
    ) -> Result<Option<(u64, Option<String>)>, StoreError> {
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT cursor, transcript FROM thread WHERE harness = ?1 AND session_id = ?2",
                params![harness, session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(offset, path)| {
            let offset = u64::try_from(offset)
                .map_err(|_| invalid("a stored transcript offset is negative"))?;
            Ok((offset, path))
        })
        .transpose()
    }

    /// Attaches an edit to the entry `entry_source_id` of a thread. An entry with no edit gets
    /// this one. An entry whose edit came from the tool input (`from_input`) gets it replaced
    /// by a measured edit; any other existing edit is kept. Returns whether the edit was stored.
    /// Only a new row counts toward the thread's edit count and redactions.
    pub fn attach_edit(
        &mut self,
        uid: &str,
        entry_source_id: &str,
        edit: &NewEdit,
    ) -> Result<bool, StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        let (entry, at): (i64, i64) = tx
            .query_row(
                "SELECT id, at FROM entry WHERE thread_id = ?1 AND source_id = ?2",
                params![thread, entry_source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        let existing: Option<(i64, bool)> = tx
            .query_row(
                "SELECT id, from_input FROM edit WHERE entry_id = ?1 ORDER BY id LIMIT 1",
                params![entry],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let stored = match existing {
            None => {
                insert_edit(&tx, thread, entry, at, edit)?;
                tx.execute(
                    "UPDATE thread SET edit_count = edit_count + 1 WHERE id = ?1",
                    params![thread],
                )?;
                true
            }
            Some((old, true)) if !edit.from_input => {
                tx.execute("DELETE FROM edit WHERE id = ?1", params![old])?;
                insert_edit(&tx, thread, entry, at, edit)?;
                purge_search_index(&tx)?;
                true
            }
            Some(_) => false,
        };
        tx.commit()?;
        Ok(stored)
    }

    /// The arguments JSON stored for a tool call, if the call is stored. The outer `None` means
    /// no such entry; the inner one that the call has no arguments stored.
    pub fn entry_args(
        &self,
        harness: &str,
        session_id: &str,
        source_id: &str,
    ) -> Result<Option<String>, StoreError> {
        let args: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT e.args_json FROM entry e JOIN thread t ON t.id = e.thread_id
                 WHERE t.harness = ?1 AND t.session_id = ?2 AND e.source_id = ?3",
                params![harness, session_id, source_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(args.flatten())
    }

    /// The native tool name stored for a tool call, if the call is stored.
    pub fn entry_tool(
        &self,
        harness: &str,
        session_id: &str,
        source_id: &str,
    ) -> Result<Option<String>, StoreError> {
        let tool: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT e.tool FROM entry e JOIN thread t ON t.id = e.thread_id
                 WHERE t.harness = ?1 AND t.session_id = ?2 AND e.source_id = ?3",
                params![harness, session_id, source_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(tool.flatten())
    }

    /// The first and last commit seen for a thread: `(head_start, head_end)`.
    pub fn heads(&self, uid: &str) -> Result<(Option<String>, Option<String>), StoreError> {
        let thread = thread_id(&self.conn, uid)?;
        Ok(self.conn.query_row(
            "SELECT head_start, head_end FROM thread WHERE id = ?1",
            params![thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    /// The members of a workspace thread with the HEADs they were seen at (D180), by name.
    pub fn member_heads(&self, uid: &str) -> Result<Vec<ThreadMember>, StoreError> {
        let mut query = self.conn.prepare(
            "SELECT m.member, m.head_start, m.head_end FROM thread_member m
             JOIN thread t ON t.id = m.thread_id WHERE t.uid = ?1 ORDER BY m.member",
        )?;
        let rows = query.query_map(params![uid], |row| {
            Ok(ThreadMember {
                member: row.get(0)?,
                head_start: row.get(1)?,
                head_end: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records the HEAD of a member seen at this invocation (D180). The first one seen is the
    /// member's `head_start`; every call moves `head_end` to `head`.
    pub fn record_member_head(
        &mut self,
        uid: &str,
        member: &str,
        head: &str,
    ) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
        tx.execute(
            "INSERT INTO thread_member (thread_id, member, head_start, head_end)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (thread_id, member) DO UPDATE SET head_end = excluded.head_end",
            params![thread, member, head],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Whether the entry `entry_source_id` of a session already carries an edit.
    pub fn has_edit(
        &self,
        harness: &str,
        session_id: &str,
        entry_source_id: &str,
    ) -> Result<bool, StoreError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM edit ed
                JOIN entry e ON e.id = ed.entry_id
                JOIN thread t ON t.id = e.thread_id
                WHERE t.harness = ?1 AND t.session_id = ?2 AND e.source_id = ?3)",
            params![harness, session_id, entry_source_id],
            |row| row.get(0),
        )?)
    }

    /// The capture gaps of a thread, one per reason.
    pub fn gaps(&self, uid: &str) -> Result<Vec<CaptureGap>, StoreError> {
        let thread = thread_id(&self.conn, uid)?;
        let mut statement = self.conn.prepare_cached(
            "SELECT MIN(at), reason, COUNT(*) FROM capture_gap
             WHERE thread_id = ?1 GROUP BY reason ORDER BY MIN(at), reason",
        )?;
        let gaps = statement
            .query_map(params![thread], |row| {
                Ok(CaptureGap {
                    at: row.get(0)?,
                    reason: row.get(1)?,
                    count: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(gaps)
    }

    pub fn total_threads(&self) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM thread", [], |row| row.get(0))?)
    }

    /// Applies `f` to every entry text, tool argument JSON and edit patch. A `Some` result
    /// replaces the value, and the search index follows. Returns how many rows changed.
    pub fn rewrite_texts(&mut self, f: &dyn Fn(&str) -> Option<String>) -> Result<u32, StoreError> {
        let tx = begin(&mut self.conn)?;
        let changed = rewrite_entries(&tx, f)? + rewrite_patches(&tx, f)?;
        if changed > 0 {
            purge_search_index(&tx)?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Moves the WAL into the database file and empties the WAL.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}

/// Opens the file and prepares it. SQLite can report busy at once, without waiting, while
/// another process is creating the WAL or the schema, so the attempt is repeated for a
/// short, bounded time.
fn open_connection(path: &Path, clean: TextCleaner) -> Result<Connection, StoreError> {
    retry_busy(|| {
        let mut conn = Connection::open(path)?;
        prepare(&mut conn, clean)?;
        Ok(conn)
    })
}

/// Runs `attempt` again, up to `OPEN_ATTEMPTS` times and `OPEN_PAUSE` apart, while it fails because
/// the database is locked (the open of a file another process is creating, D122).
pub(crate) fn retry_busy<T>(
    mut attempt: impl FnMut() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let mut attempts = 0_u32;
    loop {
        match attempt() {
            Err(StoreError::Sqlite(error)) if is_busy(&error) && attempts < OPEN_ATTEMPTS => {
                attempts += 1;
                std::thread::sleep(OPEN_PAUSE);
            }
            other => return other,
        }
    }
}

pub(crate) fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::DatabaseBusy
                || failure.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// Opens the connection, sets the pragmas and migrates.
fn prepare(conn: &mut Connection, clean: TextCleaner) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "secure_delete", true)?;
    migrate(conn, clean)
}

/// Runs the migrations the database has not seen, in one immediate transaction so two
/// processes opening a new file do not both create the schema. The repair of stored entries
/// runs inside the transaction, right after its schema (D133).
fn migrate(conn: &mut Connection, clean: TextCleaner) -> Result<(), StoreError> {
    let tx = begin(conn)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let latest = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    let from = usize::try_from(version)
        .ok()
        .filter(|from| *from <= MIGRATIONS.len())
        .ok_or_else(|| invalid("threads.db was written by a newer GitMaster"))?;
    if from == MIGRATIONS.len() {
        return Ok(());
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(from) {
        tx.execute_batch(sql)?;
        if index == REPAIR_MIGRATION {
            repair_entries(&tx, clean)?;
        }
    }
    tx.pragma_update(None, "user_version", latest)?;
    tx.commit()?;
    // A database created just now has nothing to reclaim. Skipping the VACUUM there also keeps a
    // second process that opens the new file at the same moment from meeting the exclusive lock
    // in the middle of connecting the full-text table.
    if from > 0 && count_entries(conn, VACUUM_ENTRY_LIMIT)? < VACUUM_ENTRY_LIMIT {
        // The dropped tool index is out of the file once the pages are reused (D132). A busy
        // database skips this: the next checkpoint and later writes reclaim the pages.
        match conn.execute_batch("VACUUM") {
            Ok(()) => {}
            Err(error) if is_busy(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The number of entries, counted up to `limit` so a large database is not scanned.
fn count_entries(conn: &Connection, limit: i64) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM entry LIMIT ?1)",
        params![limit],
        |row| row.get(0),
    )?)
}

/// Creates the file with mode `0600` when missing, and makes it private when it is not.
pub(crate) fn create_private(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn begin(conn: &mut Connection) -> rusqlite::Result<Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
}

pub(crate) fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.to_owned())
}

/// SQLite stores integers as signed 64-bit values; a larger unsigned value is refused.
fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| invalid("a number is too large to store"))
}

pub(super) fn thread_id(conn: &Connection, uid: &str) -> Result<i64, StoreError> {
    conn.query_row(
        "SELECT id FROM thread WHERE uid = ?1",
        params![uid],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

fn summary_of(conn: &Connection, thread: i64) -> Result<ThreadSummary, StoreError> {
    let sql = format!("{SUMMARY_SQL} WHERE t.id = ?1");
    let (id, mut summary) = conn.query_row(&sql, params![thread], summary_row)?;
    summary.tags = tags_of(conn, id)?;
    Ok(summary)
}

/// The names of the labels attached to thread `thread` (D145), by name.
fn tags_of(conn: &Connection, thread: i64) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT l.name FROM label_attach a JOIN label l ON l.id = a.label_id
         WHERE a.kind = 'thread' AND a.ref = (SELECT uid FROM thread WHERE id = ?1)
         ORDER BY l.name",
    )?;
    let tags = statement
        .query_map(params![thread], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(tags)
}

/// The filter values that are not plain columns: the label names as a JSON array, and the kind
/// of relation as its stored text. They own their text, so the bindings can borrow them.
struct FilterValues {
    label: Option<String>,
    related_kind: Option<&'static str>,
}

impl FilterValues {
    fn of(filter: &ThreadFilter) -> Self {
        Self {
            label: filter
                .label
                .as_ref()
                .map(|names| serde_json::to_string(names).unwrap_or_else(|_| "[]".to_owned())),
            related_kind: filter.related_kind.map(rel_kind_text),
        }
    }
}

/// The filter's parameters, in the names `FILTER_SQL` uses.
fn thread_bindings<'a>(
    filter: &'a ThreadFilter,
    values: &'a FilterValues,
) -> Vec<(&'static str, &'a dyn ToSql)> {
    let mut bindings: Vec<(&'static str, &'a dyn ToSql)> = Vec::with_capacity(15);
    bindings.push((":harness", &filter.harness));
    bindings.push((":branch", &filter.branch));
    bindings.push((":since", &filter.since));
    bindings.push((":until", &filter.until));
    bindings.push((":starred", &filter.starred));
    bindings.push((":has_edits", &filter.has_edits));
    bindings.push((":path", &filter.path));
    bindings.push((":thread", &filter.thread));
    bindings.push((":model", &filter.model));
    bindings.push((":commit", &filter.commit));
    bindings.push((":tag", &filter.tag));
    bindings.push((":label", &values.label));
    bindings.push((":related_to", &filter.related_to));
    bindings.push((":related_kind", &values.related_kind));
    bindings
}

/// One search or listing as the engine asks for it (D147).
#[derive(Clone, Copy, Debug)]
pub struct SearchRequest<'a> {
    pub query: &'a str,
    pub filter: &'a ThreadFilter,
    /// The page size.
    pub limit: u32,
    /// The `next_cursor` of the previous page, or `None` for the first.
    pub cursor: Option<&'a str>,
    pub group: SearchGroup,
    pub order: SearchOrder,
    /// Entries before and after each hit; clamped to 3.
    pub context: u8,
    /// Whether to count the common values of the matching threads.
    pub facets: bool,
}

/// One page of a search or a listing (D147).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<ThreadHit>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub facets: Option<SearchFacets>,
    pub diagnosis: Option<Vec<(String, u32)>>,
}

/// Where a page starts. A search is windowed by an offset in its fixed order; a listing is
/// keyed by the last thread of the previous page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Position {
    Offset(usize),
    Keyset(i64, String),
}

/// What a hit of a listing looks like: the thread, its title as the snippet, and seq 0.
fn listing_hit(thread: ThreadSummary) -> ThreadHit {
    ThreadHit {
        thread_uid: thread.uid,
        title: thread.title.clone(),
        entry_id: 0,
        seq: 0,
        turn: 0,
        snippet: thread.title,
        at: thread.last_event_at,
        kind: HitKind::Exact,
        harness: thread.harness,
        branch: thread.branch,
        started_at: thread.started_at,
        edit_count: thread.edit_count,
        tags: thread.tags,
        context: Vec::new(),
    }
}

/// The hits of one thread at most, the best of each (its first, as the stream is ranked).
fn group_hits(entries: &[ThreadHit], group: SearchGroup) -> Vec<ThreadHit> {
    match group {
        SearchGroup::Entry => entries.to_vec(),
        SearchGroup::Thread => {
            let mut seen = HashSet::new();
            entries
                .iter()
                .filter(|hit| seen.insert(hit.thread_uid.as_str()))
                .cloned()
                .collect()
        }
    }
}

/// The filters a diagnosis may remove, with the name it reports: each set one, with a copy of
/// the filter that has it cleared (D147).
fn set_filters(filter: &ThreadFilter) -> Vec<(&'static str, ThreadFilter)> {
    type Check = (&'static str, bool, fn(&mut ThreadFilter));
    let checks: [Check; 15] = [
        ("branch", filter.branch.is_some(), |f| f.branch = None),
        ("harness", filter.harness.is_some(), |f| f.harness = None),
        ("model", filter.model.is_some(), |f| f.model = None),
        ("since", filter.since.is_some(), |f| f.since = None),
        ("until", filter.until.is_some(), |f| f.until = None),
        ("starred", filter.starred.is_some(), |f| f.starred = None),
        ("has_edits", filter.has_edits.is_some(), |f| {
            f.has_edits = None;
        }),
        ("path", filter.path.is_some(), |f| f.path = None),
        ("commit", filter.commit.is_some(), |f| f.commit = None),
        ("tag", filter.tag.is_some(), |f| f.tag = None),
        ("label", filter.label.is_some(), |f| f.label = None),
        ("related_to", filter.related_to.is_some(), |f| {
            f.related_to = None;
            f.related_kind = None;
        }),
        ("role", filter.role.is_some(), |f| f.role = None),
        ("thread", filter.thread.is_some(), |f| f.thread = None),
        (
            "turn_range",
            filter.turn_from.is_some() || filter.turn_to.is_some(),
            |f| {
                f.turn_from = None;
                f.turn_to = None;
            },
        ),
    ];
    checks
        .into_iter()
        .filter(|(_, set, _)| *set)
        .map(|(name, _, clear)| {
            let mut relaxed = filter.clone();
            clear(&mut relaxed);
            (name, relaxed)
        })
        .collect()
}

/// The `count` most common values, most first, ties by name (facets, D147).
fn top_counts(counts: BTreeMap<String, u32>) -> Vec<(String, u32)> {
    let mut values: Vec<(String, u32)> = counts.into_iter().collect();
    values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    values.truncate(FACET_TOP);
    values
}

/// Digest of what a cursor belongs to: the query and every option that shapes the hits. A
/// cursor from another request fails this check (D147).
fn request_digest(
    query: &str,
    filter: &ThreadFilter,
    group: SearchGroup,
    order: SearchOrder,
) -> String {
    use sha2::Digest as _;
    let mut hasher = Sha256::new();
    hasher.update(query.as_bytes());
    hasher.update([0x1f_u8]);
    hasher.update(format!("{filter:?}|{group:?}|{order:?}").as_bytes());
    hex_encode(&hasher.finalize()[..8])
}

/// An opaque cursor: the digest and the position, as hex.
fn encode_cursor(digest: &str, position: &Position) -> String {
    let payload = match position {
        Position::Offset(offset) => format!("{digest}:o{offset}"),
        Position::Keyset(at, uid) => format!("{digest}:k{at}:{uid}"),
    };
    hex_encode(payload.as_bytes())
}

fn decode_cursor(text: &str, digest: &str) -> Result<Position, StoreError> {
    let bytes = hex_decode(text).ok_or_else(|| invalid(CURSOR_MALFORMED))?;
    let payload = String::from_utf8(bytes).map_err(|_| invalid(CURSOR_MALFORMED))?;
    let rest = payload
        .strip_prefix(digest)
        .and_then(|rest| rest.strip_prefix(':'))
        .ok_or_else(|| invalid(CURSOR_MISMATCH))?;
    if let Some(offset) = rest.strip_prefix('o') {
        offset
            .parse()
            .map(Position::Offset)
            .map_err(|_| invalid(CURSOR_MALFORMED))
    } else if let Some(key) = rest.strip_prefix('k') {
        let (at, uid) = key
            .split_once(':')
            .ok_or_else(|| invalid(CURSOR_MALFORMED))?;
        let at = at.parse().map_err(|_| invalid(CURSOR_MALFORMED))?;
        Ok(Position::Keyset(at, uid.to_owned()))
    } else {
        Err(invalid(CURSOR_MALFORMED))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 == 1 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

/// The metadata of one thread by its uid, with its tags.
fn summary_by_uid(conn: &Connection, uid: &str) -> Result<ThreadSummary, StoreError> {
    let sql = format!("{SUMMARY_SQL} WHERE t.uid = ?1");
    let (id, mut summary) = conn
        .query_row(&sql, params![uid], summary_row)
        .optional()?
        .ok_or(StoreError::NotFound)?;
    summary.tags = tags_of(conn, id)?;
    Ok(summary)
}

/// The entries of a context query (`seq`, `kind`, `text`), in the query's order, with the text
/// capped (D147). Rows of an unknown kind are skipped.
fn brief_rows(
    statement: &mut rusqlite::Statement<'_>,
    uid: &str,
    seq: u32,
    limit: i64,
) -> Result<Vec<ThreadEntryBrief>, StoreError> {
    let rows = statement
        .query_map(params![uid, seq, limit], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(seq, kind, text)| {
            Some(ThreadEntryBrief {
                seq,
                kind: parse_kind(&kind)?,
                text: text.chars().take(CONTEXT_CHARS).collect(),
            })
        })
        .collect())
}

/// The `entry.kind` text of a role (`kind_text` for the two kinds a search covers).
fn entry_role_text(role: EntryRole) -> &'static str {
    match role {
        EntryRole::User => "user",
        EntryRole::Assistant => "assistant",
    }
}

/// The statements of one stage over the entry index and the edit index. `snippet` asks FTS
/// for a snippet, `body` for the indexed text (entry text or edit path).
fn stage_statements(entry_index: &str, edit_index: &str, snippet: bool, body: bool) -> Vec<String> {
    vec![
        stage_sql(entry_index, false, snippet, body),
        stage_sql(edit_index, true, snippet, body),
    ]
}

fn stage_sql(index: &str, edit: bool, snippet: bool, body: bool) -> String {
    let snippet = if snippet {
        format!("snippet({index}, 0, :mark_start, :mark_end, '...', 12)")
    } else {
        "NULL".to_owned()
    };
    let body = match (body, edit) {
        (false, _) => "NULL",
        (true, false) => "e.text",
        (true, true) => "ed.path",
    };
    let from = if edit {
        format!("JOIN edit ed ON ed.id = {index}.rowid JOIN entry e ON e.id = ed.entry_id")
    } else {
        format!("JOIN entry e ON e.id = {index}.rowid")
    };
    format!(
        "SELECT e.id, t.uid, t.title, e.seq, e.turn, {snippet}, e.at, bm25({index}), {body}
         FROM {index} {from}
         JOIN thread t ON t.id = e.thread_id
         WHERE {index} MATCH :fts{FILTER_SQL}{ENTRY_FILTER_SQL}
         ORDER BY bm25({index}), e.id
         LIMIT :limit"
    )
}

/// A row of a stage before it is merged: the hit, its bm25 rank and the text it matched
/// (`None` when the stage did not ask for it).
struct Candidate {
    rank: f64,
    hit: ThreadHit,
    body: Option<String>,
}

/// The hits of one search, filled stage by stage: one per entry, never more than `limit`.
struct Hits {
    list: Vec<ThreadHit>,
    seen: HashSet<i64>,
    limit: usize,
}

impl Hits {
    fn new(limit: usize) -> Self {
        Self {
            list: Vec::new(),
            seen: HashSet::new(),
            limit,
        }
    }

    fn room(&self) -> usize {
        self.limit.saturating_sub(self.list.len())
    }

    fn add(&mut self, hit: ThreadHit) {
        if self.room() > 0 && self.seen.insert(hit.entry_id) {
            self.list.push(hit);
        }
    }
}

fn candidate_row(row: &Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        rank: row.get(7)?,
        hit: ThreadHit {
            entry_id: row.get(0)?,
            thread_uid: row.get(1)?,
            title: row.get(2)?,
            seq: row.get(3)?,
            turn: row.get(4)?,
            snippet: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            at: row.get(6)?,
            kind: HitKind::Exact,
            ..ThreadHit::default()
        },
        body: row.get(8)?,
    })
}

/// The fuzzy check of one text: every typed word needs a word of the text at trigram
/// similarity `FUZZY_MIN_SIMILARITY` or more. Returns the sum of the best similarities and
/// the span of the most similar word, for the snippet.
fn fuzzy_match(text: &str, probes: &[&str]) -> Option<(f32, (usize, usize))> {
    if probes.is_empty() {
        return None;
    }
    let probe_sets: Vec<Vec<[char; 3]>> = probes.iter().map(|probe| trigram_set(probe)).collect();
    let mut best = vec![0.0_f32; probes.len()];
    let mut best_word: Option<((usize, usize), f32)> = None;
    for span in word_spans(text) {
        let word = trigram_set(&text[span.0..span.1]);
        let mut word_best = 0.0_f32;
        for (slot, probe) in best.iter_mut().zip(&probe_sets) {
            let score = dice(probe, &word);
            *slot = slot.max(score);
            word_best = word_best.max(score);
        }
        if word_best > best_word.map_or(0.0, |(_, score)| score) {
            best_word = Some((span, word_best));
        }
    }
    if best.iter().all(|score| *score >= FUZZY_MIN_SIMILARITY) {
        let (span, _) = best_word?;
        Some((best.iter().sum(), span))
    } else {
        None
    }
}

/// Byte spans of the words of a text: maximal runs of word characters.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (at, ch) in text.char_indices() {
        if is_word_char(ch) {
            start.get_or_insert(at);
        } else if let Some(from) = start.take() {
            spans.push((from, at));
        }
    }
    if let Some(from) = start {
        spans.push((from, text.len()));
    }
    spans
}

/// The first case-insensitive occurrence of `needle` in `text`, as byte offsets.
fn find_ci(text: &str, needle: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    // Each lowercased character with the byte range of the original character it came from.
    let mut lowered: Vec<(char, usize, usize)> = Vec::with_capacity(text.len());
    for (at, ch) in text.char_indices() {
        for low in ch.to_lowercase() {
            lowered.push((low, at, at + ch.len_utf8()));
        }
    }
    lowered
        .windows(needle.len())
        .position(|window| {
            window
                .iter()
                .zip(&needle)
                .all(|(seen, want)| seen.0 == *want)
        })
        .map(|at| (lowered[at].1, lowered[at + needle.len() - 1].2))
}

/// The earliest occurrence of any token, marked, or the start of the text when none
/// occurs (the match was on the tool name).
fn substring_snippet(text: &str, tokens: &[String]) -> String {
    tokens
        .iter()
        .filter_map(|token| find_ci(text, token))
        .min_by_key(|span| span.0)
        .map_or_else(|| plain_snippet(text), |span| marked_snippet(text, span))
}

/// The text with `span` marked and about `CONTEXT_WORDS` words of context on each side.
fn marked_snippet(text: &str, span: (usize, usize)) -> String {
    let (start, end) = span;
    let from = back_words(text, start, CONTEXT_WORDS);
    let to = forward_words(text, end, CONTEXT_WORDS);
    let mut out = String::new();
    if from > 0 {
        out.push_str("...");
    }
    out.push_str(&text[from..start]);
    out.push_str(SNIPPET_MARK_START);
    out.push_str(&text[start..end]);
    out.push_str(SNIPPET_MARK_END);
    out.push_str(&text[end..to]);
    if to < text.len() {
        out.push_str("...");
    }
    out
}

/// The start of a text, unmarked, about `PLAIN_SNIPPET_WORDS` words long.
fn plain_snippet(text: &str) -> String {
    let to = forward_words(text, 0, PLAIN_SNIPPET_WORDS);
    let mut out = text[..to].to_owned();
    if to < text.len() {
        out.push_str("...");
    }
    out
}

/// Where the `words` words before byte `at` begin (`0` when there are fewer).
fn back_words(text: &str, at: usize, words: usize) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for (index, ch) in text[..at].char_indices().rev() {
        if ch.is_whitespace() {
            if in_word {
                count += 1;
                in_word = false;
                if count == words {
                    return index + ch.len_utf8();
                }
            }
        } else {
            in_word = true;
        }
    }
    0
}

/// Where the `words` words after byte `at` end (the text's end when there are fewer).
fn forward_words(text: &str, at: usize, words: usize) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for (index, ch) in text[at..].char_indices() {
        if ch.is_whitespace() {
            if in_word {
                count += 1;
                in_word = false;
                if count == words {
                    return at + index;
                }
            }
        } else {
            in_word = true;
        }
    }
    text.len()
}

fn summary_row(row: &Row<'_>) -> rusqlite::Result<(i64, ThreadSummary)> {
    let summary = ThreadSummary {
        uid: row.get(1)?,
        harness: row.get(2)?,
        harness_version: row.get(3)?,
        session_id: row.get(4)?,
        worktree: row.get(5)?,
        branch: row.get(6)?,
        model: row.get(7)?,
        title: row.get(8)?,
        note: row.get(9)?,
        starred: row.get(10)?,
        tags: Vec::new(),
        started_at: row.get(11)?,
        last_event_at: row.get(12)?,
        ended_at: row.get(13)?,
        entry_count: row.get(14)?,
        edit_count: row.get(15)?,
        redaction_count: row.get(16)?,
        workspace: None,
    };
    Ok((row.get(0)?, summary))
}

fn entry_row(row: &Row<'_>) -> rusqlite::Result<ThreadEntry> {
    let kind: String = row.get(3)?;
    Ok(ThreadEntry {
        id: row.get(0)?,
        seq: row.get(1)?,
        turn: row.get(2)?,
        kind: parse_kind(&kind).ok_or_else(|| bad_value(3, &kind))?,
        source_id: row.get(4)?,
        parent_id: row.get(5)?,
        tool: row.get(6)?,
        text: row.get(7)?,
        args_json: row.get(8)?,
        is_error: row.get(9)?,
        truncated: row.get(10)?,
        orig_bytes: row
            .get::<_, Option<i64>>(11)?
            .and_then(|bytes| u64::try_from(bytes).ok()),
        redactions: row.get(12)?,
        at: row.get(13)?,
    })
}

fn edit_row(row: &Row<'_>) -> rusqlite::Result<ThreadEdit> {
    let change: String = row.get(4)?;
    Ok(ThreadEdit {
        id: row.get(0)?,
        entry_id: row.get(1)?,
        path: row.get(2)?,
        old_path: row.get(3)?,
        change: parse_change(&change).ok_or_else(|| bad_value(4, &change))?,
        patch: row.get(5)?,
        added: row.get(6)?,
        removed: row.get(7)?,
        pre_oid: row.get(8)?,
        post_oid: row.get(9)?,
        from_input: row.get(10)?,
        at: row.get(11)?,
        member: row.get(12)?,
    })
}

fn link_row(row: &Row<'_>) -> rusqlite::Result<ThreadLink> {
    let kind: String = row.get(0)?;
    let origin: String = row.get(2)?;
    Ok(ThreadLink {
        kind: parse_link_kind(&kind).ok_or_else(|| bad_value(0, &kind))?,
        reference: row.get(1)?,
        origin: parse_origin(&origin).ok_or_else(|| bad_value(2, &origin))?,
        // The resolution is made on read by the engine (D145), not stored here.
        resolved: None,
        state: None,
        member: row.get(3)?,
    })
}

fn bad_value(index: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("unknown stored value {value:?}").into(),
    )
}

fn kind_text(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "user",
        EntryKind::Assistant => "assistant",
        EntryKind::ToolCall => "tool_call",
        EntryKind::ToolResult => "tool_result",
        EntryKind::Notice => "notice",
    }
}

fn parse_kind(text: &str) -> Option<EntryKind> {
    match text {
        "user" => Some(EntryKind::User),
        "assistant" => Some(EntryKind::Assistant),
        "tool_call" => Some(EntryKind::ToolCall),
        "tool_result" => Some(EntryKind::ToolResult),
        "notice" => Some(EntryKind::Notice),
        _ => None,
    }
}

fn change_text(change: EditChange) -> &'static str {
    match change {
        EditChange::Create => "create",
        EditChange::Modify => "modify",
        EditChange::Delete => "delete",
        EditChange::Rename => "rename",
    }
}

fn parse_change(text: &str) -> Option<EditChange> {
    match text {
        "create" => Some(EditChange::Create),
        "modify" => Some(EditChange::Modify),
        "delete" => Some(EditChange::Delete),
        "rename" => Some(EditChange::Rename),
        _ => None,
    }
}

fn link_kind_text(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Commit => "commit",
        LinkKind::Branch => "branch",
        LinkKind::File => "file",
    }
}

fn parse_link_kind(text: &str) -> Option<LinkKind> {
    match text {
        "commit" => Some(LinkKind::Commit),
        "branch" => Some(LinkKind::Branch),
        "file" => Some(LinkKind::File),
        _ => None,
    }
}

fn origin_text(origin: LinkOrigin) -> &'static str {
    match origin {
        LinkOrigin::Observed => "observed",
        LinkOrigin::Human => "human",
    }
}

fn parse_origin(text: &str) -> Option<LinkOrigin> {
    match text {
        "observed" => Some(LinkOrigin::Observed),
        "human" => Some(LinkOrigin::Human),
        _ => None,
    }
}

/// The title a first prompt gives: its first non-empty line, trimmed, at most 80 characters.
fn derive_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.chars().take(TITLE_CHARS).collect())
}

fn insert_thread(conn: &Connection, uid: &str, batch: &Batch) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO thread (uid, harness, session_id, worktree, branch, head_start, title,
            started_at, last_event_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            uid,
            batch.harness,
            batch.session_id,
            batch.worktree,
            batch.branch,
            batch.head,
            UNTITLED,
            batch.started_at,
            batch.at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The fields a later batch may move: the latest time, and the values it reports when set.
fn touch_thread(conn: &Connection, thread: i64, batch: &Batch) -> Result<(), StoreError> {
    let cursor = batch.cursor.map(to_i64).transpose()?;
    conn.execute(
        "UPDATE thread SET
            last_event_at = MAX(last_event_at, ?2),
            head_end = COALESCE(?3, head_end),
            model = COALESCE(?4, model),
            harness_version = COALESCE(?5, harness_version),
            transcript = COALESCE(?6, transcript),
            cursor = COALESCE(?7, cursor),
            ended_at = COALESCE(?8, ended_at)
         WHERE id = ?1",
        params![
            thread,
            batch.at,
            batch.head,
            batch.model,
            batch.harness_version,
            batch.transcript,
            cursor,
            batch.ended
        ],
    )?;
    Ok(())
}

fn stored(conn: &Connection, thread: i64, source_id: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM entry WHERE thread_id = ?1 AND source_id = ?2)",
        params![thread, source_id],
        |row| row.get(0),
    )?)
}

/// Counters an apply carries across its entries.
struct ApplyState {
    entry_count: i64,
    edit_count: i64,
    next_seq: i64,
    turn: i64,
    title_edited: bool,
}

impl ApplyState {
    fn load(conn: &Connection, thread: i64) -> Result<Self, StoreError> {
        let (entry_count, edit_count, title_edited) = conn.query_row(
            "SELECT entry_count, edit_count, title_edited FROM thread WHERE id = ?1",
            params![thread],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )?;
        let (next_seq, turn) = conn.query_row(
            "SELECT COALESCE(MAX(seq) + 1, 0), COALESCE(MAX(turn), 0)
             FROM entry WHERE thread_id = ?1",
            params![thread],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        Ok(Self {
            entry_count,
            edit_count,
            next_seq,
            turn,
            title_edited,
        })
    }

    fn write_counters(&self, conn: &Connection, thread: i64) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE thread SET entry_count = ?2, edit_count = ?3 WHERE id = ?1",
            params![thread, self.entry_count, self.edit_count],
        )?;
        Ok(())
    }
}

fn insert_entry(
    conn: &Connection,
    thread: i64,
    entry: &NewEntry,
    state: &mut ApplyState,
) -> Result<(), StoreError> {
    let first_prompt = entry.kind == EntryKind::User && state.turn == 0;
    let orig_bytes = entry.orig_bytes.map(to_i64).transpose()?;
    if entry.kind == EntryKind::User {
        state.turn += 1;
    }
    conn.execute(
        "INSERT INTO entry (thread_id, seq, turn, kind, source_id, parent_id, tool, text,
            args_json, is_error, truncated, orig_bytes, redactions, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            thread,
            state.next_seq,
            state.turn,
            kind_text(entry.kind),
            entry.source_id,
            entry.parent_id,
            entry.tool,
            entry.text,
            entry.args_json,
            entry.is_error,
            entry.truncated,
            orig_bytes,
            entry.redactions,
            entry.at
        ],
    )?;
    let entry_id = conn.last_insert_rowid();
    state.next_seq += 1;
    state.entry_count += 1;
    add_redactions(conn, thread, &entry.hits)?;

    let title = (first_prompt && !state.title_edited)
        .then(|| derive_title(&entry.text))
        .flatten();
    if let Some(title) = title {
        conn.execute(
            "UPDATE thread SET title = ?2 WHERE id = ?1",
            params![thread, title],
        )?;
    }
    if let Some(edit) = &entry.edit {
        insert_edit(conn, thread, entry_id, entry.at, edit)?;
        state.edit_count += 1;
    }
    Ok(())
}

/// Stores one edit row for an entry, with its redaction counts.
fn insert_edit(
    conn: &Connection,
    thread: i64,
    entry_id: i64,
    at: i64,
    edit: &NewEdit,
) -> Result<(), StoreError> {
    let worktree: String = conn.query_row(
        "SELECT worktree FROM thread WHERE id = ?1",
        params![thread],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO edit (thread_id, entry_id, path, old_path, change, patch, added, removed,
            pre_oid, post_oid, from_input, at, rel_path, member)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            thread,
            entry_id,
            edit.path,
            edit.old_path,
            change_text(edit.change),
            edit.patch,
            edit.added,
            edit.removed,
            edit.pre_oid,
            edit.post_oid,
            edit.from_input,
            at,
            repo_path(&edit.path, &worktree),
            edit.member
        ],
    )?;
    add_redactions(conn, thread, &edit.hits)
}

/// The repository path of an edit (D144): the path as recorded, with the worktree and its `/`
/// taken off when the edit recorded an absolute path under the worktree. The same rule as the
/// migration's `UPDATE`, so a new edit and a migrated one agree.
pub(super) fn repo_path(path: &str, worktree: &str) -> String {
    match path.strip_prefix(&format!("{worktree}/")) {
        Some(rest) => rest.to_owned(),
        None => path.to_owned(),
    }
}

/// Records the entry-limit gap once per thread.
fn record_limit_gap(conn: &Connection, thread: i64, at: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO capture_gap (thread_id, at, reason)
         SELECT ?1, ?2, ?3 WHERE NOT EXISTS
            (SELECT 1 FROM capture_gap WHERE thread_id = ?1 AND reason = ?3)",
        params![thread, at, LIMIT_GAP_REASON],
    )?;
    Ok(())
}

fn record_gap(conn: &Connection, thread: i64, at: i64, reason: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO capture_gap (thread_id, at, reason)
         SELECT ?1, ?2, ?3 WHERE NOT EXISTS
            (SELECT 1 FROM capture_gap WHERE thread_id = ?1 AND at = ?2 AND reason = ?3)",
        params![thread, at, reason],
    )?;
    Ok(())
}

fn add_redactions(
    conn: &Connection,
    thread: i64,
    counts: &[(String, u32)],
) -> Result<(), StoreError> {
    for (pattern, count) in counts.iter().filter(|(_, count)| *count > 0) {
        conn.execute(
            "INSERT INTO thread_redaction (thread_id, pattern, count) VALUES (?1, ?2, ?3)
             ON CONFLICT (thread_id, pattern)
             DO UPDATE SET count = thread_redaction.count + excluded.count",
            params![thread, pattern, count],
        )?;
    }
    Ok(())
}

/// Secure delete leaves the text in the index's delete markers until the index merges, so
/// every index (word and trigram) is optimized after every removal of text (D131).
fn purge_search_index(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "INSERT INTO entry_fts (entry_fts) VALUES ('optimize');
         INSERT INTO edit_fts (edit_fts) VALUES ('optimize');
         INSERT INTO entry_tri (entry_tri) VALUES ('optimize');
         INSERT INTO edit_tri (edit_tri) VALUES ('optimize');",
    )?;
    Ok(())
}

fn rewrite_entries(
    tx: &Transaction<'_>,
    f: &dyn Fn(&str) -> Option<String>,
) -> Result<u32, StoreError> {
    let mut changed = 0;
    let mut after = 0_i64;
    loop {
        let rows: Vec<(i64, String, Option<String>)> = {
            let mut statement = tx.prepare_cached(
                "SELECT id, text, args_json FROM entry WHERE id > ?1 ORDER BY id LIMIT ?2",
            )?;
            statement
                .query_map(params![after, REWRITE_PAGE], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let Some(last) = rows.last().map(|row| row.0) else {
            return Ok(changed);
        };
        after = last;
        for (id, text, args) in rows {
            let new_text = f(&text).filter(|new| *new != text);
            let new_args = args
                .as_deref()
                .and_then(f)
                .filter(|new| Some(new.as_str()) != args.as_deref());
            if new_text.is_none() && new_args.is_none() {
                continue;
            }
            let text = new_text.unwrap_or(text);
            let args = new_args.or(args);
            tx.execute(
                "UPDATE entry SET text = ?2, args_json = ?3 WHERE id = ?1",
                params![id, text, args],
            )?;
            changed += 1;
        }
    }
}

fn rewrite_patches(
    tx: &Transaction<'_>,
    f: &dyn Fn(&str) -> Option<String>,
) -> Result<u32, StoreError> {
    let mut changed = 0;
    let mut after = 0_i64;
    loop {
        let rows: Vec<(i64, String)> = {
            let mut statement =
                tx.prepare_cached("SELECT id, patch FROM edit WHERE id > ?1 ORDER BY id LIMIT ?2")?;
            statement
                .query_map(params![after, REWRITE_PAGE], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let Some(last) = rows.last().map(|row| row.0) else {
            return Ok(changed);
        };
        after = last;
        for (id, patch) in rows {
            let Some(new_patch) = f(&patch).filter(|new| *new != patch) else {
                continue;
            };
            tx.execute(
                "UPDATE edit SET patch = ?2 WHERE id = ?1",
                params![id, new_patch],
            )?;
            changed += 1;
        }
    }
}

/// A thread whose title follows its prompts, as the repair found it, before the cleaner ran.
struct TitleCheck {
    thread: i64,
    harness: String,
    title: String,
    /// The text of the thread's first prompt before the repair.
    first: Option<String>,
}

fn title_checks(tx: &Transaction<'_>) -> Result<Vec<TitleCheck>, StoreError> {
    let mut statement = tx.prepare(
        "SELECT t.id, t.harness, t.title,
            (SELECT e.text FROM entry e WHERE e.thread_id = t.id AND e.kind = 'user'
             ORDER BY e.seq LIMIT 1)
         FROM thread t WHERE t.title_edited = 0",
    )?;
    let checks = statement
        .query_map([], |row| {
            Ok(TitleCheck {
                thread: row.get(0)?,
                harness: row.get(1)?,
                title: row.get(2)?,
                first: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(checks)
}

/// The text of a thread's first prompt now, if it has one.
fn first_prompt(tx: &Transaction<'_>, thread: i64) -> Result<Option<String>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT text FROM entry WHERE thread_id = ?1 AND kind = 'user'
             ORDER BY seq LIMIT 1",
            params![thread],
            |row| row.get(0),
        )
        .optional()?)
}

/// Whether the cleaner changes `text` as a prompt, so it holds a harness marker.
fn marked(clean: TextCleaner, harness: &str, text: &str) -> bool {
    match clean(harness, EntryKind::User, text) {
        None => false,
        Some(Cleaned::Text(cleaned)) => cleaned != text,
        Some(Cleaned::Drop | Cleaned::Notice(_)) => true,
    }
}

/// The schema 4 repair (D133). Every stored entry goes through the cleaner. An entry that is
/// harness markers only is deleted with its edits; a cleaned entry takes its new text, and a
/// notice also its new kind, which moves it out of the search index. A thread whose title came
/// from a prompt that changed takes the title of its first remaining prompt, or the untitled
/// name. Titles the person edited are left alone.
fn repair_entries(tx: &Transaction<'_>, clean: TextCleaner) -> Result<(), StoreError> {
    let titles = title_checks(tx)?;
    let mut changed = 0_u32;
    let mut dropped = 0_u32;
    let mut after = 0_i64;
    loop {
        let rows: Vec<(i64, String, String, String)> = {
            let mut statement = tx.prepare_cached(
                "SELECT e.id, t.harness, e.kind, e.text
                 FROM entry e JOIN thread t ON t.id = e.thread_id
                 WHERE e.id > ?1 ORDER BY e.id LIMIT ?2",
            )?;
            statement
                .query_map(params![after, REWRITE_PAGE], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?
        };
        let Some(last) = rows.last().map(|row| row.0) else {
            break;
        };
        after = last;
        for (id, harness, kind, text) in rows {
            let Some(kind) = parse_kind(&kind) else {
                continue;
            };
            match clean(&harness, kind, &text) {
                None => {}
                Some(Cleaned::Text(new)) => {
                    if new != text {
                        tx.execute("UPDATE entry SET text = ?2 WHERE id = ?1", params![id, new])?;
                        changed += 1;
                    }
                }
                Some(Cleaned::Drop) => {
                    tx.execute("DELETE FROM entry WHERE id = ?1", params![id])?;
                    dropped += 1;
                }
                Some(Cleaned::Notice(notice)) => {
                    tx.execute(
                        "UPDATE entry SET text = ?2, kind = ?3 WHERE id = ?1",
                        params![id, notice, kind_text(EntryKind::Notice)],
                    )?;
                    changed += 1;
                }
            }
        }
    }
    if dropped > 0 {
        // Counters are recounted, as `delete_entry` does for one entry.
        tx.execute_batch(
            "UPDATE thread SET
                entry_count = (SELECT COUNT(*) FROM entry WHERE entry.thread_id = thread.id),
                edit_count = (SELECT COUNT(*) FROM edit WHERE edit.thread_id = thread.id)",
        )?;
    }
    for check in titles {
        let derived = check.first.as_deref().and_then(derive_title);
        let affected = derived.as_deref() == Some(check.title.as_str())
            || marked(clean, &check.harness, &check.title)
            || check
                .first
                .as_deref()
                .is_some_and(|text| marked(clean, &check.harness, text));
        if !affected {
            continue;
        }
        let title = first_prompt(tx, check.thread)?
            .as_deref()
            .and_then(derive_title)
            .unwrap_or_else(|| UNTITLED.to_owned());
        if title != check.title {
            tx.execute(
                "UPDATE thread SET title = ?2 WHERE id = ?1",
                params![check.thread, title],
            )?;
        }
    }
    if changed + dropped > 0 {
        purge_search_index(tx)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::{AttachKind, EditChange};
    use proptest::prelude::*;
    use std::thread;

    fn batch(session: &str, at: i64, entries: Vec<NewEntry>) -> Batch {
        Batch {
            harness: "claude-code".to_owned(),
            session_id: session.to_owned(),
            worktree: "/repo".to_owned(),
            branch: Some("main".to_owned()),
            head: Some("1111111".to_owned()),
            model: None,
            harness_version: None,
            transcript: None,
            cursor: None,
            started_at: 1_000,
            at,
            ended: None,
            entries,
            gaps: Vec::new(),
        }
    }

    fn new_entry(source: &str, kind: EntryKind, text: &str, at: i64) -> NewEntry {
        NewEntry {
            source_id: source.to_owned(),
            kind,
            parent_id: None,
            tool: None,
            text: text.to_owned(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            hits: Vec::new(),
            at,
            edit: None,
        }
    }

    fn with_edit(mut entry: NewEntry, path: &str) -> NewEntry {
        entry.edit = Some(measured_edit(path));
        entry
    }

    fn measured_edit(path: &str) -> NewEdit {
        NewEdit {
            member: None,
            path: path.to_owned(),
            old_path: None,
            change: EditChange::Modify,
            patch: "@@ -1 +1 @@\n-a\n+b\n".to_owned(),
            added: 1,
            removed: 1,
            pre_oid: None,
            post_oid: None,
            from_input: false,
            hits: Vec::new(),
        }
    }

    fn uid(session: &str) -> String {
        ThreadStore::thread_uid("claude-code", session)
    }

    /// The row id of a thread or entry, which the summaries do not carry.
    fn row_id(store: &ThreadStore, sql: &str, value: &str) -> i64 {
        store
            .conn
            .query_row(sql, [value], |row| row.get(0))
            .expect("row id")
    }

    fn titles(list: &[ThreadSummary]) -> Vec<String> {
        list.iter().map(|thread| thread.title.clone()).collect()
    }

    fn sorted_titles(store: &ThreadStore, filter: &ThreadFilter) -> Vec<String> {
        let mut titles = titles(&store.list_threads(filter, None, 10).expect("list"));
        titles.sort();
        titles
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn schema_is_created_and_reopening_runs_no_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut store = ThreadStore::open(dir.path()).expect("create");
            store.apply(&batch("s", 1, vec![])).expect("apply");
        }
        let store = ThreadStore::open(dir.path()).expect("reopen");
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, 7);
        assert_eq!(store.total_threads().expect("count"), 1);
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gitmaster");
        std::fs::create_dir_all(&path).expect("dir");
        {
            let conn = Connection::open(path.join("threads.db")).expect("raw");
            conn.pragma_update(None, "user_version", 99).expect("bump");
        }
        assert!(matches!(
            ThreadStore::open(dir.path()),
            Err(StoreError::Invalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let _store = ThreadStore::open(dir.path()).expect("open");
        let path = dir.path().join("gitmaster").join("threads.db");
        let mode = std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn thread_uid_is_sixteen_hex_digits_and_separates_its_parts() {
        let uid = ThreadStore::thread_uid("claude-code", "abc");
        assert_eq!(uid.len(), 16);
        assert!(uid.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(uid, ThreadStore::thread_uid("claude-code", "abc"));
        assert_ne!(
            ThreadStore::thread_uid("ab", "c"),
            ThreadStore::thread_uid("a", "bc")
        );
    }

    #[test]
    fn applying_a_batch_twice_changes_nothing_the_second_time() {
        let mut store = ThreadStore::open_memory().expect("open");
        let entries = vec![
            new_entry("u1", EntryKind::User, "fix the parser", 1),
            with_edit(new_entry("t1", EntryKind::ToolCall, "Edit", 2), "src/p.rs"),
            new_entry("a1", EntryKind::Assistant, "done", 3),
        ];
        let b = batch("s", 3, entries);
        let first = store.apply(&b).expect("first");
        let before = store.read_thread(&uid("s"), 0, 100).expect("read");
        let second = store.apply(&b).expect("second");
        let after = store.read_thread(&uid("s"), 0, 100).expect("read");
        assert_eq!(first.inserted, 3);
        assert_eq!(second.inserted, 0);
        assert_eq!(second.duplicates, 3);
        assert_eq!(before, after);
        assert_eq!(after.summary.entry_count, 3);
        assert_eq!(after.summary.edit_count, 1);
    }

    #[test]
    fn seq_and_turn_follow_the_human_prompts() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "s",
                4,
                vec![
                    new_entry("n0", EntryKind::Notice, "started", 1),
                    new_entry("u1", EntryKind::User, "first", 2),
                    new_entry("a1", EntryKind::Assistant, "ok", 3),
                    new_entry("u2", EntryKind::User, "second", 4),
                ],
            ))
            .expect("apply");
        let detail = store.read_thread(&uid("s"), 0, 100).expect("read");
        let shape: Vec<(u32, u32)> = detail.entries.iter().map(|e| (e.seq, e.turn)).collect();
        assert_eq!(shape, vec![(0, 0), (1, 1), (2, 1), (3, 2)]);
    }

    #[test]
    fn the_title_comes_from_the_first_prompt_until_it_is_edited() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "s",
                2,
                vec![new_entry(
                    "u1",
                    EntryKind::User,
                    "  \n  Fix the parser\nmore",
                    1,
                )],
            ))
            .expect("first");
        store
            .apply(&batch(
                "s",
                3,
                vec![new_entry("u2", EntryKind::User, "second prompt", 3)],
            ))
            .expect("second");
        let title = store
            .read_thread(&uid("s"), 0, 1)
            .expect("read")
            .summary
            .title;
        assert_eq!(title, "Fix the parser");

        store
            .update_meta(
                &uid("s"),
                &MetaUpdate {
                    title: Some("My title".to_owned()),
                    ..MetaUpdate::default()
                },
            )
            .expect("rename");
        store
            .apply(&batch(
                "s",
                4,
                vec![new_entry("u3", EntryKind::User, "third", 4)],
            ))
            .expect("later");
        let title = store
            .read_thread(&uid("s"), 0, 1)
            .expect("read")
            .summary
            .title;
        assert_eq!(title, "My title");
    }

    #[test]
    fn a_long_title_is_capped_by_characters_and_a_promptless_thread_is_untitled() {
        let mut store = ThreadStore::open_memory().expect("open");
        let long = "é".repeat(100);
        store
            .apply(&batch(
                "long",
                1,
                vec![new_entry("u", EntryKind::User, &long, 1)],
            ))
            .expect("long");
        store
            .apply(&batch(
                "none",
                1,
                vec![new_entry("n", EntryKind::Notice, "x", 1)],
            ))
            .expect("none");
        let long_title = store
            .read_thread(&uid("long"), 0, 1)
            .expect("read")
            .summary
            .title;
        assert_eq!(long_title.chars().count(), TITLE_CHARS);
        let none = store
            .read_thread(&uid("none"), 0, 1)
            .expect("read")
            .summary
            .title;
        assert_eq!(none, UNTITLED);
    }

    /// A generated entry: ids repeat (so duplicates occur), kinds vary, some carry an edit.
    fn generated_entry(id: u8, kind: u8, at: usize) -> NewEntry {
        let kind = match kind {
            0 => EntryKind::User,
            1 => EntryKind::Assistant,
            2 => EntryKind::ToolCall,
            _ => EntryKind::ToolResult,
        };
        let entry = new_entry(
            &format!("e{id}"),
            kind,
            &format!("text {id} {at}"),
            i64::try_from(at).unwrap_or(0),
        );
        if kind == EntryKind::ToolCall && id.is_multiple_of(2) {
            with_edit(entry, &format!("src/f{}.rs", id % 3))
        } else {
            entry
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn replaying_a_prefix_then_the_batch_equals_one_clean_pass(
            spec in prop::collection::vec((0_u8..10, 0_u8..4), 0..24),
            cut in 0_usize..24,
        ) {
            let entries: Vec<NewEntry> = spec
                .iter()
                .enumerate()
                .map(|(at, (id, kind))| generated_entry(*id, *kind, at))
                .collect();
            let cut = cut.min(entries.len());
            let full = batch("p", 2_000, entries.clone());
            let prefix = batch("p", 1_500, entries[..cut].to_vec());

            let mut clean = ThreadStore::open_memory().expect("open");
            clean.apply(&full).expect("clean");
            let mut replayed = ThreadStore::open_memory().expect("open");
            replayed.apply(&prefix).expect("prefix");
            replayed.apply(&full).expect("full");

            let expected = clean.read_thread(&uid("p"), 0, u32::MAX).expect("read");
            let actual = replayed.read_thread(&uid("p"), 0, u32::MAX).expect("read");
            prop_assert_eq!(expected, actual);
        }
    }

    #[test]
    fn list_pages_by_the_last_event_and_id_cursor() {
        let mut store = ThreadStore::open_memory().expect("open");
        for i in 0..5_i64 {
            let session = format!("s{i}");
            store
                .apply(&batch(
                    &session,
                    1_000 + i * 100,
                    vec![new_entry(
                        "u",
                        EntryKind::User,
                        &format!("t{i}"),
                        1_000 + i * 100,
                    )],
                ))
                .expect("apply");
        }
        let filter = ThreadFilter::default();
        let first = store.list_threads(&filter, None, 2).expect("page 1");
        assert_eq!(titles(&first), vec!["t4", "t3"]);
        let last = &first[1];
        let cursor = (last.last_event_at, last.uid.clone());
        let second = store
            .list_threads(&filter, Some(cursor), 2)
            .expect("page 2");
        assert_eq!(titles(&second), vec!["t2", "t1"]);
        let last = &second[1];
        let cursor = (last.last_event_at, last.uid.clone());
        let third = store
            .list_threads(&filter, Some(cursor), 2)
            .expect("page 3");
        assert_eq!(titles(&third), vec!["t0"]);
    }

    #[test]
    fn list_filters_restrict_by_each_field() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut a = batch(
            "a",
            100,
            vec![with_edit(
                new_entry("u", EntryKind::User, "alpha", 100),
                "src/a.rs",
            )],
        );
        a.started_at = 90;
        store.apply(&a).expect("a");
        let mut b = batch("b", 200, vec![new_entry("u", EntryKind::User, "beta", 200)]);
        b.harness = "other".to_owned();
        b.branch = Some("dev".to_owned());
        store.apply(&b).expect("b");
        store
            .update_meta(
                &ThreadStore::thread_uid("other", "b"),
                &MetaUpdate {
                    starred: Some(true),
                    ..MetaUpdate::default()
                },
            )
            .expect("star");
        store
            .attach_label("bug", AttachKind::Thread, &uid("a"), None, "human", 1)
            .expect("tag");
        let list = |filter: ThreadFilter| sorted_titles(&store, &filter);
        let none = ThreadFilter::default();
        assert_eq!(list(none.clone()), vec!["alpha", "beta"]);
        assert_eq!(
            list(ThreadFilter {
                harness: Some("other".to_owned()),
                ..none.clone()
            }),
            vec!["beta"]
        );
        assert_eq!(
            list(ThreadFilter {
                branch: Some("main".to_owned()),
                ..none.clone()
            }),
            vec!["alpha"]
        );
        assert_eq!(
            list(ThreadFilter {
                since: Some(150),
                ..none.clone()
            }),
            vec!["beta"]
        );
        assert_eq!(
            list(ThreadFilter {
                until: Some(150),
                ..none.clone()
            }),
            vec!["alpha"]
        );
        assert_eq!(
            list(ThreadFilter {
                starred: Some(true),
                ..none.clone()
            }),
            vec!["beta"]
        );
        assert_eq!(
            list(ThreadFilter {
                has_edits: Some(true),
                ..none.clone()
            }),
            vec!["alpha"]
        );
        assert_eq!(
            list(ThreadFilter {
                has_edits: Some(false),
                ..none.clone()
            }),
            vec!["beta"]
        );
        assert_eq!(
            list(ThreadFilter {
                path: Some("src/a.rs".to_owned()),
                ..none.clone()
            }),
            vec!["alpha"]
        );
        assert_eq!(
            list(ThreadFilter {
                tag: Some("bug".to_owned()),
                ..none
            }),
            vec!["alpha"]
        );
    }

    #[test]
    fn list_filters_combine_and_can_match_nothing() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                100,
                vec![new_entry("u", EntryKind::User, "alpha", 100)],
            ))
            .expect("a");
        let filter = ThreadFilter {
            harness: Some("other".to_owned()),
            tag: Some("missing".to_owned()),
            ..ThreadFilter::default()
        };
        assert!(
            store
                .list_threads(&filter, None, 10)
                .expect("list")
                .is_empty()
        );
    }

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER_OID: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn prompted(session: &str, at: i64, text: &str) -> Batch {
        batch(session, at, vec![new_entry("u", EntryKind::User, text, at)])
    }

    fn link_to(store: &mut ThreadStore, session: &str, kind: LinkKind, reference: &str) {
        let link = ThreadLink {
            member: None,
            kind,
            reference: reference.to_owned(),
            origin: LinkOrigin::Human,
            resolved: None,
            state: None,
        };
        store.link(&uid(session), &link).expect("link");
    }

    fn with_commit(commit: &str) -> ThreadFilter {
        ThreadFilter {
            commit: Some(commit.to_owned()),
            ..ThreadFilter::default()
        }
    }

    #[test]
    fn commit_filter_matches_a_full_oid_in_either_case() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        store.apply(&prompted("b", 200, "beta")).expect("b");
        link_to(&mut store, "a", LinkKind::Commit, OID);
        link_to(&mut store, "b", LinkKind::Commit, OTHER_OID);
        assert_eq!(sorted_titles(&store, &with_commit(OID)), vec!["alpha"]);
        assert_eq!(
            sorted_titles(&store, &with_commit(&OID.to_uppercase())),
            vec!["alpha"]
        );
        assert!(sorted_titles(&store, &with_commit("0000000")).is_empty());
    }

    #[test]
    fn commit_filter_accepts_a_short_oid_link_for_a_full_filter() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        store.apply(&prompted("b", 200, "beta")).expect("b");
        link_to(&mut store, "a", LinkKind::Commit, "0123456");
        link_to(&mut store, "b", LinkKind::Commit, "012345");
        assert_eq!(sorted_titles(&store, &with_commit(OID)), vec!["alpha"]);
        assert_eq!(
            sorted_titles(&store, &with_commit("0123456")),
            vec!["alpha"]
        );
        // Six digits are too few to name a commit, so only the exact link matches.
        assert_eq!(sorted_titles(&store, &with_commit("012345")), vec!["beta"]);
    }

    #[test]
    fn commit_filter_accepts_a_short_filter_for_a_full_oid_link() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        link_to(&mut store, "a", LinkKind::Commit, OID);
        assert_eq!(
            sorted_titles(&store, &with_commit("0123456")),
            vec!["alpha"]
        );
        assert_eq!(
            sorted_titles(&store, &with_commit(&OID[..12].to_uppercase())),
            vec!["alpha"]
        );
        assert!(sorted_titles(&store, &with_commit("012345")).is_empty());
    }

    #[test]
    fn ref_looking_links_that_are_not_commits_do_not_match() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        store.apply(&prompted("b", 200, "beta")).expect("b");
        store.apply(&prompted("c", 300, "gamma")).expect("c");
        // A branch named like an oid is not a commit link.
        link_to(&mut store, "a", LinkKind::Branch, OID);
        // A commit link that is not hex is never a short oid.
        link_to(&mut store, "b", LinkKind::Commit, "0123456g");
        link_to(&mut store, "c", LinkKind::Commit, "main");
        assert!(sorted_titles(&store, &with_commit(OID)).is_empty());
        assert!(sorted_titles(&store, &with_commit("0123456gfff")).is_empty());
        assert!(sorted_titles(&store, &with_commit("main-wip")).is_empty());
        // The exact reference still matches, because it is the same text.
        assert_eq!(sorted_titles(&store, &with_commit("main")), vec!["gamma"]);
    }

    #[test]
    fn commit_filter_ignores_threads_without_links() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        store.apply(&prompted("b", 200, "beta")).expect("b");
        link_to(&mut store, "b", LinkKind::File, OID);
        assert!(sorted_titles(&store, &with_commit(OID)).is_empty());
        assert!(
            store
                .list_threads(&with_commit(OID), None, 10)
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn commit_filter_combines_with_other_filters() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&prompted("a", 100, "alpha")).expect("a");
        let mut other = prompted("b", 200, "beta");
        other.harness = "other".to_owned();
        store.apply(&other).expect("b");
        link_to(&mut store, "a", LinkKind::Commit, OID);
        let other_uid = ThreadStore::thread_uid("other", "b");
        let link = ThreadLink {
            member: None,
            kind: LinkKind::Commit,
            reference: OID.to_owned(),
            origin: LinkOrigin::Human,
            resolved: None,
            state: None,
        };
        store.link(&other_uid, &link).expect("link");
        let in_other = ThreadFilter {
            harness: Some("other".to_owned()),
            commit: Some(OID.to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(sorted_titles(&store, &in_other), vec!["beta"]);
        let elsewhere = ThreadFilter {
            branch: Some("dev".to_owned()),
            commit: Some(OID.to_owned()),
            ..ThreadFilter::default()
        };
        assert!(sorted_titles(&store, &elsewhere).is_empty());
    }

    #[test]
    fn search_applies_the_commit_filter() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&prompted("a", 100, "alpha backoff"))
            .expect("a");
        store.apply(&prompted("b", 200, "beta backoff")).expect("b");
        link_to(&mut store, "a", LinkKind::Commit, "0123456");
        link_to(&mut store, "b", LinkKind::Commit, OTHER_OID);
        let hits = store
            .search("backoff", &with_commit(OID), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_uid, uid("a"));
        let none = store
            .search("backoff", &with_commit("ffffffff"), 10)
            .expect("search");
        assert!(none.is_empty());
    }

    fn in_thread(filter: ThreadFilter, session: &str) -> ThreadFilter {
        ThreadFilter {
            thread: Some(uid(session)),
            ..filter
        }
    }

    fn seqs_of(hits: &[ThreadHit]) -> Vec<u32> {
        hits.iter().map(|hit| hit.seq).collect()
    }

    #[test]
    fn role_picks_the_user_or_the_assistant_entry() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u", EntryKind::User, "retry with backoff", 1),
                    new_entry("x", EntryKind::Assistant, "the backoff uses jitter", 2),
                ],
            ))
            .expect("a");
        let search = |role: Option<EntryRole>| {
            store
                .search(
                    "backoff",
                    &ThreadFilter {
                        role,
                        ..ThreadFilter::default()
                    },
                    10,
                )
                .expect("search")
        };
        assert_eq!(seqs_of(&search(None)), vec![0, 1]);
        assert_eq!(seqs_of(&search(Some(EntryRole::User))), vec![0]);
        assert_eq!(seqs_of(&search(Some(EntryRole::Assistant))), vec![1]);
        let assistant = search(Some(EntryRole::Assistant));
        assert_eq!(assistant[0].thread_uid, uid("a"));
        assert_eq!(assistant[0].kind, HitKind::Exact);
    }

    #[test]
    fn turn_range_is_inclusive_and_counts_entries_by_turn() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u1", EntryKind::User, "first backoff", 1),
                    new_entry("x1", EntryKind::Assistant, "reply backoff", 2),
                    new_entry("u2", EntryKind::User, "second backoff", 3),
                    new_entry("u3", EntryKind::User, "third backoff", 4),
                ],
            ))
            .expect("a");
        let turns = |turn_from: Option<u32>, turn_to: Option<u32>| {
            let filter = ThreadFilter {
                turn_from,
                turn_to,
                ..ThreadFilter::default()
            };
            store
                .search("backoff", &filter, 10)
                .expect("search")
                .iter()
                .map(|hit| (hit.seq, hit.turn))
                .collect::<Vec<_>>()
        };
        assert_eq!(turns(None, None), vec![(0, 1), (1, 1), (2, 2), (3, 3)]);
        assert_eq!(turns(Some(2), None), vec![(2, 2), (3, 3)]);
        assert_eq!(turns(None, Some(1)), vec![(0, 1), (1, 1)]);
        assert_eq!(turns(Some(2), Some(2)), vec![(2, 2)]);
        assert!(turns(Some(4), None).is_empty());
        assert!(turns(Some(3), Some(2)).is_empty());

        // Listing has no entries to restrict, so the turn range does not narrow it.
        let listed = store
            .list_threads(
                &ThreadFilter {
                    turn_from: Some(9),
                    ..ThreadFilter::default()
                },
                None,
                10,
            )
            .expect("list");
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn thread_and_model_restrict_to_one_thread() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut a = prompted("a", 100, "retry with backoff");
        a.model = Some("opus-4".to_owned());
        store.apply(&a).expect("a");
        let mut b = prompted("b", 200, "retry with backoff");
        b.model = Some("sonnet-4".to_owned());
        store.apply(&b).expect("b");
        store
            .apply(&prompted("c", 300, "retry with backoff"))
            .expect("c");

        let by = |filter: ThreadFilter| {
            store
                .search("backoff", &filter, 10)
                .expect("search")
                .into_iter()
                .map(|hit| hit.thread_uid)
                .collect::<Vec<_>>()
        };
        assert_eq!(by(in_thread(ThreadFilter::default(), "b")), vec![uid("b")]);
        assert!(by(in_thread(ThreadFilter::default(), "missing")).is_empty());
        let model = |name: &str| ThreadFilter {
            model: Some(name.to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(by(model("opus-4")), vec![uid("a")]);
        assert_eq!(by(model("sonnet-4")), vec![uid("b")]);
        assert!(
            by(model("sonnet")).is_empty(),
            "the model is an exact match"
        );
        assert!(
            by(model("Opus-4")).is_empty(),
            "the model is case-sensitive"
        );
        let listed = store
            .list_threads(&model("opus-4"), None, 10)
            .expect("list");
        assert_eq!(
            listed.iter().map(|t| t.uid.clone()).collect::<Vec<_>>(),
            vec![uid("a")]
        );
        let both = ThreadFilter {
            model: Some("opus-4".to_owned()),
            ..in_thread(ThreadFilter::default(), "b")
        };
        assert!(by(both).is_empty());
    }

    #[test]
    fn a_path_is_exact_or_a_glob_when_it_has_wildcards() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![with_edit(
                    new_entry("t", EntryKind::ToolCall, "Edit", 1),
                    "src/search/index.rs",
                )],
            ))
            .expect("a");
        store
            .apply(&batch(
                "b",
                2,
                vec![with_edit(
                    new_entry("t", EntryKind::ToolCall, "Edit", 2),
                    "docs/index.md",
                )],
            ))
            .expect("b");
        store
            .apply(&batch(
                "c",
                3,
                vec![with_edit(
                    new_entry("t", EntryKind::ToolCall, "Edit", 3),
                    "src/lib.rs",
                )],
            ))
            .expect("c");
        let listed = |path: &str| {
            let filter = ThreadFilter {
                path: Some(path.to_owned()),
                ..ThreadFilter::default()
            };
            let mut uids = store
                .list_threads(&filter, None, 10)
                .expect("list")
                .into_iter()
                .map(|thread| thread.uid)
                .collect::<Vec<_>>();
            uids.sort();
            uids
        };
        let mut both = vec![uid("a"), uid("c")];
        both.sort();
        assert_eq!(listed("src/*.rs"), both);
        assert_eq!(listed("*.md"), vec![uid("b")]);
        assert_eq!(listed("docs/*"), vec![uid("b")]);
        assert_eq!(listed("src/?ib.rs"), vec![uid("c")]);
        assert_eq!(listed("src/[l]ib.rs"), vec![uid("c")]);
        assert!(listed("src/[xy]ib.rs").is_empty());
        assert!(listed("*.RS").is_empty(), "globs are case-sensitive");

        let edits_in_docs = ThreadFilter {
            path: Some("docs/*".to_owned()),
            ..ThreadFilter::default()
        };
        let hits = store.search("index", &edits_in_docs, 10).expect("search");
        assert_eq!(uids_of(&hits), vec![uid("b")]);
    }

    #[test]
    fn search_ranks_by_bm25_and_returns_marked_snippets() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u", EntryKind::User, "retry with exponential back off", 1),
                    new_entry(
                        "x",
                        EntryKind::Assistant,
                        "the function file_index is rebuilt",
                        2,
                    ),
                ],
            ))
            .expect("a");
        store
            .apply(&batch(
                "b",
                2,
                vec![new_entry("u", EntryKind::User, "flaky test, flaky", 1)],
            ))
            .expect("b");
        store
            .apply(&batch(
                "c",
                3,
                vec![new_entry(
                    "u",
                    EntryKind::User,
                    "flaky flaky flaky flaky",
                    1,
                )],
            ))
            .expect("c");
        let filter = ThreadFilter::default();

        let hits = store.search("file_index", &filter, 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].snippet.contains("\u{e000}file_index\u{e001}"),
            "{}",
            hits[0].snippet
        );

        let hits = store.search("flaky", &filter, 10).expect("search");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].thread_uid, uid("c"));
        assert_eq!(hits[1].thread_uid, uid("b"));
    }

    #[test]
    fn search_finds_prefixes_phrases_and_edit_paths() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u", EntryKind::User, "retry with exponential back off", 1),
                    with_edit(
                        new_entry("t", EntryKind::ToolCall, "Edit", 2),
                        "src/search/index.rs",
                    ),
                ],
            ))
            .expect("a");
        let filter = ThreadFilter::default();
        assert_eq!(store.search("retry back", &filter, 10).expect("s").len(), 1);
        assert_eq!(
            store.search("\"back off\"", &filter, 10).expect("s").len(),
            1
        );
        assert!(
            store
                .search("\"off back\"", &filter, 10)
                .expect("s")
                .is_empty()
        );
        let hits = store.search("index", &filter, 10).expect("s");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].seq, 1);
        assert!(
            hits[0].snippet.contains("\u{e000}index\u{e001}"),
            "{}",
            hits[0].snippet
        );
    }

    #[test]
    fn search_with_hostile_input_does_not_error() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![new_entry("u", EntryKind::User, "foo bar", 1)],
            ))
            .expect("a");
        let filter = ThreadFilter::default();
        for query in [
            "\"",
            "*",
            "AND (",
            "NEAR(",
            "foo\"bar",
            "^",
            "-",
            "\"unterminated",
            "",
            "*:()^-+",
        ] {
            store
                .search(query, &filter, 10)
                .unwrap_or_else(|e| panic!("{query:?}: {e}"));
        }
        assert_eq!(store.search("", &filter, 10).expect("empty").len(), 0);
        assert_eq!(store.search("foo", &filter, 1).expect("limit").len(), 1);
    }

    #[test]
    fn delete_thread_removes_its_text_from_the_database_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = "zqx7secretmarker";
        let mut store = ThreadStore::open(dir.path()).expect("open");
        let mut tool = with_edit(
            new_entry("t", EntryKind::ToolCall, &format!("Edit {marker}"), 2),
            "src/lib.rs",
        );
        tool.args_json = Some(format!("{{\"text\":\"{marker}\"}}"));
        store
            .apply(&batch(
                "s",
                2,
                vec![
                    new_entry(
                        "u",
                        EntryKind::User,
                        &format!("use {marker} for the token"),
                        1,
                    ),
                    tool,
                ],
            ))
            .expect("apply");
        // The trigram index holds the text too: it is found by a substring before the delete.
        let filter = ThreadFilter::default();
        // The tool call holds the marker too, but only conversation is indexed: one hit.
        assert_eq!(
            store
                .search("ecretmark", &filter, 10)
                .expect("substring")
                .len(),
            1
        );
        assert!(trigram_rows(&store, "secretmark") >= 1);
        store.delete_thread(&uid("s")).expect("delete");
        assert_eq!(store.total_threads().expect("count"), 0);
        assert!(
            store
                .search(marker, &filter, 10)
                .expect("search")
                .is_empty()
        );
        assert_eq!(trigram_rows(&store, "secretmark"), 0);
        store.checkpoint().expect("checkpoint");

        let db = dir.path().join("gitmaster");
        for name in ["threads.db", "threads.db-wal"] {
            if let Ok(bytes) = std::fs::read(db.join(name)) {
                assert!(
                    !contains(&bytes, marker.as_bytes()),
                    "{name} still holds the text"
                );
            }
        }
    }

    #[test]
    fn delete_entry_removes_its_edits_and_fixes_the_counters() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "s",
                3,
                vec![
                    new_entry("u", EntryKind::User, "go", 1),
                    with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "a.rs"),
                    new_entry("a", EntryKind::Assistant, "done", 3),
                ],
            ))
            .expect("apply");
        let entry = row_id(&store, "SELECT id FROM entry WHERE source_id = ?1", "t");
        store.delete_entry(entry).expect("delete entry");
        let summary = store.read_thread(&uid("s"), 0, 10).expect("read").summary;
        assert_eq!(summary.entry_count, 2);
        assert_eq!(summary.edit_count, 0);
        assert!(matches!(
            store.delete_entry(entry),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn links_are_added_replaced_and_removed() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&batch("s", 1, vec![])).expect("apply");
        let link = |origin| ThreadLink {
            member: None,
            kind: LinkKind::Commit,
            reference: "abc123".to_owned(),
            origin,
            resolved: None,
            state: None,
        };
        store
            .link(&uid("s"), &link(LinkOrigin::Observed))
            .expect("link");
        store
            .link(&uid("s"), &link(LinkOrigin::Human))
            .expect("relink");
        let links = store.read_thread(&uid("s"), 0, 1).expect("read").links;
        assert_eq!(links, vec![link(LinkOrigin::Human)]);
        store
            .unlink(&uid("s"), LinkKind::Commit, "abc123")
            .expect("unlink");
        assert!(matches!(
            store.unlink(&uid("s"), LinkKind::Commit, "abc123"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.link(
                &uid("s"),
                &ThreadLink {
                    member: None,
                    kind: LinkKind::File,
                    reference: " ".to_owned(),
                    origin: LinkOrigin::Human,
                    resolved: None,
                    state: None,
                }
            ),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn rewrite_texts_keeps_the_search_index_in_step() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut tool = with_edit(
            new_entry("t", EntryKind::ToolCall, "Edit hunter2", 2),
            "a.rs",
        );
        tool.args_json = Some("{\"k\":\"hunter2\"}".to_owned());
        store
            .apply(&batch(
                "s",
                2,
                vec![
                    new_entry("u", EntryKind::User, "plain", 1),
                    tool,
                    new_entry("x", EntryKind::Assistant, "the key is hunter2", 3),
                ],
            ))
            .expect("apply");
        let scrub = |text: &str| {
            text.contains("hunter2")
                .then(|| text.replace("hunter2", "[redacted]"))
        };
        // The tool row and the assistant row; the edit patch holds no secret.
        let changed = store.rewrite_texts(&scrub).expect("rewrite");
        assert_eq!(changed, 2);
        let filter = ThreadFilter::default();
        assert!(
            store
                .search("hunter2", &filter, 10)
                .expect("old")
                .is_empty()
        );
        assert_eq!(store.search("redacted", &filter, 10).expect("new").len(), 1);
        let detail = store.read_thread(&uid("s"), 0, 10).expect("read");
        assert_eq!(
            detail.entries[1].args_json.as_deref(),
            Some("{\"k\":\"[redacted]\"}")
        );
    }

    #[test]
    fn redaction_counts_are_added_once_per_new_entry() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut entry = new_entry("u", EntryKind::User, "x", 1);
        entry.hits = vec![("token".to_owned(), 2), ("jwt".to_owned(), 1)];
        entry.redactions = 3;
        let b = batch("s", 1, vec![entry]);
        store.apply(&b).expect("first");
        let replay = store.apply(&b).expect("replay");
        assert_eq!(replay.duplicates, 1);
        let detail = store.read_thread(&uid("s"), 0, 10).expect("read");
        assert_eq!(detail.summary.redaction_count, 3);
        assert_eq!(
            detail.redactions,
            vec![("jwt".to_owned(), 1), ("token".to_owned(), 2)]
        );
    }

    #[test]
    fn attach_edit_stores_once_and_a_measured_edit_replaces_one_from_input() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "s",
                2,
                vec![new_entry("t1", EntryKind::ToolCall, "edit a.rs", 2)],
            ))
            .expect("apply");
        let from_input = NewEdit {
            from_input: true,
            patch: "+guess\n".to_owned(),
            ..measured_edit("a.rs")
        };
        assert!(
            store
                .attach_edit(&uid("s"), "t1", &from_input)
                .expect("input")
        );
        assert!(store.has_edit("claude-code", "s", "t1").expect("has"));

        let measured = measured_edit("a.rs");
        assert!(
            store
                .attach_edit(&uid("s"), "t1", &measured)
                .expect("measured")
        );
        assert!(
            !store
                .attach_edit(&uid("s"), "t1", &from_input)
                .expect("late input")
        );
        assert!(
            !store
                .attach_edit(&uid("s"), "t1", &measured)
                .expect("again")
        );

        let detail = store.read_thread(&uid("s"), 0, 10).expect("read");
        assert_eq!(detail.edits.len(), 1);
        assert!(!detail.edits[0].from_input);
        assert_eq!(detail.summary.edit_count, 1);
    }

    #[test]
    fn attach_edit_counts_redactions_only_for_a_new_row() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "s",
                2,
                vec![new_entry("t1", EntryKind::ToolCall, "edit", 2)],
            ))
            .expect("apply");
        let mut edit = measured_edit("a.rs");
        edit.hits = vec![("github-token".to_owned(), 1)];
        store.attach_edit(&uid("s"), "t1", &edit).expect("attach");
        store.attach_edit(&uid("s"), "t1", &edit).expect("again");
        let summary = store.read_thread(&uid("s"), 0, 10).expect("read").summary;
        assert_eq!(summary.redaction_count, 1);
    }

    #[test]
    fn attach_edit_to_an_unknown_entry_or_thread_is_not_found() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.apply(&batch("s", 1, vec![])).expect("apply");
        assert!(matches!(
            store.attach_edit(&uid("s"), "missing", &measured_edit("a.rs")),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.attach_edit("0000000000000000", "t1", &measured_edit("a.rs")),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn entry_args_heads_and_has_edit_read_what_was_stored() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut call = new_entry("t1", EntryKind::ToolCall, "Read x", 2);
        call.args_json = Some("{\"file_path\":\"x\"}".to_owned());
        let mut b = batch(
            "s",
            2,
            vec![call, new_entry("t2", EntryKind::ToolCall, "Bash", 3)],
        );
        b.head = Some("abc".to_owned());
        store.apply(&b).expect("apply");
        assert_eq!(
            store.entry_args("claude-code", "s", "t1").expect("args"),
            Some("{\"file_path\":\"x\"}".to_owned())
        );
        assert_eq!(
            store.entry_args("claude-code", "s", "t2").expect("none"),
            None
        );
        assert_eq!(
            store.entry_args("claude-code", "s", "zz").expect("absent"),
            None
        );
        assert_eq!(
            store.heads(&uid("s")).expect("heads"),
            (Some("abc".to_owned()), Some("abc".to_owned()))
        );
        assert!(!store.has_edit("claude-code", "s", "t1").expect("has"));
    }

    #[test]
    fn a_locked_database_reports_busy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ThreadStore::open(dir.path()).expect("open");
        let other = Connection::open(dir.path().join("gitmaster").join("threads.db"))
            .expect("second connection");
        other
            .busy_timeout(Duration::from_millis(1))
            .expect("timeout");
        other
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold the write lock");
        let error = store.apply(&batch("s", 1, vec![])).expect_err("locked");
        assert!(error.is_busy());
        assert!(!StoreError::NotFound.is_busy());
        other.execute_batch("COMMIT").expect("release");
        store.apply(&batch("s", 1, vec![])).expect("after release");
    }

    #[test]
    fn a_batch_round_trips_through_json() {
        let mut entry = with_edit(new_entry("t1", EntryKind::ToolCall, "Edit a", 2), "a.rs");
        entry.hits = vec![("jwt".to_owned(), 1)];
        let original = batch("s", 2, vec![entry]);
        let text = serde_json::to_string(&original).expect("serialize");
        let back: Batch = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn the_entry_limit_drops_entries_and_records_one_gap() {
        let mut store = ThreadStore::open_memory().expect("open");
        store.entry_limit = 3;
        let entries: Vec<NewEntry> = (0..5_i64)
            .map(|n| new_entry(&format!("e{n}"), EntryKind::Assistant, "x", n))
            .collect();
        let applied = store.apply(&batch("s", 5, entries.clone())).expect("apply");
        assert_eq!(applied.inserted, 3);
        store.apply(&batch("s", 6, entries)).expect("replay");
        store
            .apply(&batch(
                "s",
                7,
                vec![new_entry("late", EntryKind::Assistant, "y", 7)],
            ))
            .expect("later");
        let gaps = store.gaps(&uid("s")).expect("gaps");
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].reason, LIMIT_GAP_REASON);
        assert_eq!(gaps[0].count, 1);
        assert_eq!(
            store
                .read_thread(&uid("s"), 0, 100)
                .expect("read")
                .summary
                .entry_count,
            3
        );
    }

    #[test]
    fn batch_gaps_are_recorded_once_per_time_and_reason() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut b = batch("s", 1, vec![]);
        b.gaps = vec![
            (1, "unknown event".to_owned()),
            (2, "unknown event".to_owned()),
        ];
        store.apply(&b).expect("first");
        store.apply(&b).expect("replay");
        let gaps = store.gaps(&uid("s")).expect("gaps");
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].count, 2);
    }

    #[test]
    fn read_thread_pages_entries_and_keeps_the_edits_of_each_page() {
        let mut store = ThreadStore::open_memory().expect("open");
        let entries = vec![
            new_entry("e0", EntryKind::User, "a", 0),
            new_entry("e1", EntryKind::Assistant, "b", 1),
            with_edit(new_entry("e2", EntryKind::ToolCall, "c", 2), "x.rs"),
            new_entry("e3", EntryKind::Assistant, "d", 3),
            new_entry("e4", EntryKind::Assistant, "e", 4),
        ];
        store.apply(&batch("s", 4, entries)).expect("apply");
        let first = store.read_thread(&uid("s"), 0, 2).expect("page 1");
        assert_eq!(first.entries.len(), 2);
        assert!(first.has_more);
        assert!(first.edits.is_empty());
        let second = store.read_thread(&uid("s"), 2, 10).expect("page 2");
        assert_eq!(
            second.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(second.edits.len(), 1);
        assert!(!second.has_more);
        assert_eq!(
            store.read_edit(second.edits[0].id).expect("edit").patch,
            "@@ -1 +1 @@\n-a\n+b\n"
        );
        assert!(matches!(store.read_edit(9_999), Err(StoreError::NotFound)));
    }

    #[test]
    fn cursor_reports_the_offset_and_path_of_a_session() {
        let mut store = ThreadStore::open_memory().expect("open");
        assert_eq!(store.cursor("claude-code", "s").expect("cursor"), None);
        let mut b = batch("s", 1, vec![]);
        b.cursor = Some(42);
        b.transcript = Some("/t.jsonl".to_owned());
        store.apply(&b).expect("apply");
        assert_eq!(
            store.cursor("claude-code", "s").expect("cursor"),
            Some((42, Some("/t.jsonl".to_owned())))
        );
    }

    #[test]
    fn two_writers_on_one_directory_both_succeed() {
        // A busy database is an expected outcome (threads.md: the batch waits in the spool and a
        // later run stores it). So a writer that finds the database busy keeps its batch and
        // tries again, in order, as the spool drain does. Every batch must end up stored.
        const ATTEMPTS: u32 = 60_000;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        let writers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|session| {
                let path = path.clone();
                thread::spawn(move || {
                    let mut store = ThreadStore::open(&path).expect("open");
                    let mut pending: std::collections::VecDeque<Batch> = (0..200_i64)
                        .map(|n| {
                            let entry =
                                new_entry(&format!("e{n}"), EntryKind::Assistant, "text", n);
                            batch(session, n, vec![entry])
                        })
                        .collect();
                    let mut attempts = 0_u32;
                    while let Some(next) = pending.pop_front() {
                        attempts += 1;
                        assert!(attempts <= ATTEMPTS, "the database stayed busy");
                        match store.apply(&next) {
                            Ok(_) => {}
                            Err(error) if error.is_busy() => {
                                pending.push_front(next);
                                thread::sleep(Duration::from_millis(1));
                            }
                            Err(error) => panic!("apply: {error:?}"),
                        }
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer");
        }
        let store = ThreadStore::open(&path).expect("open");
        let total: u32 = store
            .list_threads(&ThreadFilter::default(), None, 10)
            .expect("list")
            .iter()
            .map(|thread| thread.entry_count)
            .sum();
        assert_eq!(total, 400);
    }

    /// Rows of the trigram indexes that match `text` as a substring.
    fn trigram_rows(store: &ThreadStore, text: &str) -> i64 {
        let query = format!("\"{text}\"");
        store
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM entry_tri WHERE entry_tri MATCH ?1)
                      + (SELECT COUNT(*) FROM edit_tri WHERE edit_tri MATCH ?1)",
                params![query],
                |row| row.get(0),
            )
            .expect("trigram rows")
    }

    /// Every byte of the database files, the WAL included, after a checkpoint.
    fn file_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let db = dir.join("gitmaster");
        ["threads.db", "threads.db-wal"]
            .iter()
            .filter_map(|name| {
                std::fs::read(db.join(name))
                    .ok()
                    .map(|bytes| ((*name).to_owned(), bytes))
            })
            .collect()
    }

    fn indexed(store: &mut ThreadStore, session: &str, at: i64, text: &str) {
        store
            .apply(&batch(
                session,
                at,
                vec![new_entry("u", EntryKind::User, text, at)],
            ))
            .expect("apply");
    }

    fn uids_of(hits: &[ThreadHit]) -> Vec<String> {
        hits.iter().map(|hit| hit.thread_uid.clone()).collect()
    }

    #[test]
    fn the_bundled_sqlite_has_the_trigram_tokenizer() {
        assert!(
            rusqlite::version_number() >= 3_034_000,
            "trigram needs SQLite 3.34"
        );
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(x, tokenize='trigram');
             INSERT INTO t(x) VALUES ('file_index rebuilt');",
        )
        .expect("trigram table");
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM t WHERE t MATCH '\"ndex\"'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        assert_eq!(found, 1);
    }

    #[test]
    fn a_version_one_database_gains_the_trigram_indexes_with_its_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gm = dir.path().join("gitmaster");
        std::fs::create_dir_all(&gm).expect("dir");
        {
            let conn = Connection::open(gm.join("threads.db")).expect("raw");
            conn.pragma_update(None, "journal_mode", "WAL")
                .expect("wal");
            conn.execute_batch(SCHEMA_V1).expect("schema 1");
            conn.execute_batch(
                "INSERT INTO thread (uid, harness, session_id, worktree, title, started_at,
                    last_event_at)
                 VALUES ('old1', 'claude-code', 's1', '/repo', 'old', 1, 1);
                 INSERT INTO entry (thread_id, seq, turn, kind, source_id, text, at)
                 VALUES (1, 0, 1, 'user', 'u1', 'the function file_index is rebuilt', 1);
                 INSERT INTO edit (thread_id, entry_id, path, change, patch, added, removed, at)
                 VALUES (1, 1, 'src/search/index.rs', 'modify', '', 0, 0, 1);",
            )
            .expect("rows");
            conn.pragma_update(None, "user_version", 1)
                .expect("version");
        }
        let store = ThreadStore::open(dir.path()).expect("migrate");
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, 7);
        let hits = store
            .search("ndex", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, HitKind::Substring);
        assert_eq!(hits[0].seq, 0);
        let by_path = store
            .search("search", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(by_path.len(), 1);
    }

    #[test]
    fn index_finds_file_index_as_a_substring_when_the_words_do_not_match() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "the function file_index is rebuilt");
        let hits = store
            .search("index", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, HitKind::Substring);
        assert!(
            hits[0].snippet.contains("\u{e000}") && hits[0].snippet.contains("index\u{e001}"),
            "{}",
            hits[0].snippet
        );
    }

    #[test]
    fn stream_finds_history_stream_ignoring_case() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "the HistoryStream type holds the rows");
        let hits = store
            .search("stream", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, HitKind::Substring);
    }

    #[test]
    fn a_transposed_misspelling_is_found_as_fuzzy() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "retry with backoff after failures");
        let hits = store
            .search("bakcoff", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1, "similarity of bakcoff and backoff is 0.5");
        assert_eq!(hits[0].kind, HitKind::Fuzzy);
        assert!(
            hits[0].snippet.contains("\u{e000}backoff\u{e001}"),
            "{}",
            hits[0].snippet
        );
    }

    #[test]
    fn a_four_letter_typo_with_no_similar_word_finds_nothing() {
        // `fidx` has four characters, so the fuzzy stage runs for it. No trigram of it occurs
        // in the text, and no word is similar enough (below 0.5), so no result is the
        // documented outcome (D131).
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "the function file_index is rebuilt");
        let hits = store
            .search("fidx", &ThreadFilter::default(), 10)
            .expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn strict_hits_come_first_and_are_exact() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "the file_index is rebuilt");
        indexed(&mut store, "b", 2, "index of the files");
        let hits = store
            .search("index", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(uids_of(&hits), vec![uid("b"), uid("a")]);
        assert_eq!(hits[0].kind, HitKind::Exact);
        assert_eq!(hits[1].kind, HitKind::Substring);
    }

    #[test]
    fn a_fallback_stage_runs_only_below_the_threshold() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "index of the files");
        indexed(&mut store, "b", 2, "the file_index is rebuilt");
        store.fallback_below = 1;
        let hits = store
            .search("index", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(uids_of(&hits), vec![uid("a")], "one exact hit is enough");
        store.fallback_below = 8;
        let hits = store
            .search("index", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn filters_apply_to_substring_and_fuzzy_hits() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u", EntryKind::User, "the file_index is rebuilt", 1),
                    with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "src/lib.rs"),
                ],
            ))
            .expect("a");
        let mut other = batch(
            "b",
            2,
            vec![new_entry(
                "u",
                EntryKind::User,
                "the file_index is rebuilt",
                3,
            )],
        );
        other.branch = Some("dev".to_owned());
        store.apply(&other).expect("b");
        store
            .update_meta(
                &uid("b"),
                &MetaUpdate {
                    starred: Some(true),
                    ..MetaUpdate::default()
                },
            )
            .expect("star");
        let by = |filter: ThreadFilter| {
            store
                .search("index", &filter, 10)
                .expect("search")
                .into_iter()
                .map(|hit| (hit.thread_uid, hit.kind))
                .collect::<Vec<_>>()
        };
        let branch = ThreadFilter {
            branch: Some("dev".to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(by(branch), vec![(uid("b"), HitKind::Substring)]);
        let starred = ThreadFilter {
            starred: Some(true),
            ..ThreadFilter::default()
        };
        assert_eq!(by(starred), vec![(uid("b"), HitKind::Substring)]);
        let path = ThreadFilter {
            path: Some("src/lib.rs".to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(by(path), vec![(uid("a"), HitKind::Substring)]);

        // The fuzzy stage obeys the same filter: this thread has the word `backoff`.
        let mut backoff = batch(
            "c",
            4,
            vec![new_entry("u", EntryKind::User, "retry with backoff", 5)],
        );
        backoff.branch = Some("dev".to_owned());
        store.apply(&backoff).expect("c");
        let fuzzy =
            |filter: ThreadFilter| store.search("bakcoff", &filter, 10).expect("search").len();
        let dev = ThreadFilter {
            branch: Some("dev".to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(fuzzy(dev), 1, "only the dev thread has the word");
        assert_eq!(fuzzy(ThreadFilter::default()), 1);
        let main = ThreadFilter {
            branch: Some("main".to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(fuzzy(main), 0);
    }

    /// Two threads for the fallback stages: `a` has three prompts and one reply, `b` one prompt.
    fn fallback_store() -> ThreadStore {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u1", EntryKind::User, "retry with backoff", 1),
                    new_entry("x1", EntryKind::Assistant, "backoff uses jitter", 2),
                    new_entry("u2", EntryKind::User, "again, backoff", 3),
                ],
            ))
            .expect("a");
        store
            .apply(&prompted("b", 200, "retry with backoff"))
            .expect("b");
        store
    }

    /// The hits of a search as `(thread, seq, kind)`, sorted by thread and seq.
    fn found_rows(
        store: &ThreadStore,
        query: &str,
        filter: &ThreadFilter,
    ) -> Vec<(String, u32, HitKind)> {
        let mut rows: Vec<(String, u32, HitKind)> = store
            .search(query, filter, 10)
            .expect("search")
            .into_iter()
            .map(|hit| (hit.thread_uid, hit.seq, hit.kind))
            .collect();
        rows.sort_by(|x, y| (&x.0, x.1).cmp(&(&y.0, y.1)));
        rows
    }

    fn found_seqs(store: &ThreadStore, query: &str, filter: &ThreadFilter) -> Vec<(String, u32)> {
        found_rows(store, query, filter)
            .into_iter()
            .map(|(uid, seq, _)| (uid, seq))
            .collect()
    }

    fn role_filter(role: EntryRole) -> ThreadFilter {
        ThreadFilter {
            role: Some(role),
            ..ThreadFilter::default()
        }
    }

    #[test]
    fn role_turn_and_thread_filters_apply_to_the_substring_stage() {
        let store = fallback_store();
        // `ackoff` sits inside a word, so only the substring stage finds it.
        let (a, b) = (uid("a"), uid("b"));
        let seqs = |filter: &ThreadFilter| found_seqs(&store, "ackoff", filter);
        assert_eq!(
            seqs(&ThreadFilter::default()),
            vec![
                (a.clone(), 0),
                (a.clone(), 1),
                (a.clone(), 2),
                (b.clone(), 0)
            ]
        );
        assert_eq!(
            seqs(&role_filter(EntryRole::User)),
            vec![(a.clone(), 0), (a.clone(), 2), (b.clone(), 0)]
        );
        assert_eq!(
            seqs(&role_filter(EntryRole::Assistant)),
            vec![(a.clone(), 1)]
        );
        let second_turn_on = ThreadFilter {
            turn_from: Some(2),
            ..ThreadFilter::default()
        };
        assert_eq!(seqs(&second_turn_on), vec![(a.clone(), 2)]);
        let only_b = ThreadFilter {
            thread: Some(b),
            ..ThreadFilter::default()
        };
        assert_eq!(seqs(&only_b), vec![(uid("b"), 0)]);
        let rows = found_rows(&store, "ackoff", &ThreadFilter::default());
        assert!(rows.iter().all(|(_, _, kind)| *kind == HitKind::Substring));
    }

    #[test]
    fn role_turn_and_thread_filters_apply_to_the_fuzzy_stage() {
        let store = fallback_store();
        // `bakcoff` is a misspelling of `backoff`, so only the fuzzy stage finds it.
        let (a, b) = (uid("a"), uid("b"));
        let fuzzy = found_rows(&store, "bakcoff", &ThreadFilter::default());
        assert_eq!(fuzzy.len(), 4, "{fuzzy:?}");
        assert!(fuzzy.iter().all(|(_, _, kind)| *kind == HitKind::Fuzzy));
        assert_eq!(
            found_seqs(&store, "bakcoff", &role_filter(EntryRole::User)),
            vec![(a.clone(), 0), (a.clone(), 2), (b.clone(), 0)]
        );
        assert_eq!(
            found_seqs(&store, "bakcoff", &role_filter(EntryRole::Assistant)),
            vec![(a.clone(), 1)]
        );
        let first_turn_users = ThreadFilter {
            turn_to: Some(1),
            role: Some(EntryRole::User),
            ..ThreadFilter::default()
        };
        assert_eq!(
            found_seqs(&store, "bakcoff", &first_turn_users),
            vec![(a.clone(), 0), (b.clone(), 0)]
        );
        let b_replies = ThreadFilter {
            thread: Some(b),
            role: Some(EntryRole::Assistant),
            ..ThreadFilter::default()
        };
        assert!(found_seqs(&store, "bakcoff", &b_replies).is_empty());
    }

    #[test]
    fn limit_is_respected_across_stages_and_no_entry_repeats() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "w", 0, "index rebuilt by hand");
        for n in 1..=12 {
            indexed(
                &mut store,
                &format!("s{n}"),
                n,
                &format!("file_index number {n}"),
            );
        }
        let five = store
            .search("index", &ThreadFilter::default(), 5)
            .expect("search");
        assert_eq!(five.len(), 5);
        assert_eq!(five[0].kind, HitKind::Exact);
        let all = store
            .search("index", &ThreadFilter::default(), 50)
            .expect("search");
        assert_eq!(all.len(), 13);
        let mut ids: Vec<i64> = all.iter().map(|hit| hit.entry_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 13);
    }

    #[test]
    fn a_text_matched_by_word_and_by_path_is_one_hit() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![with_edit(
                    new_entry("t", EntryKind::ToolCall, "retry retry", 1),
                    "src/retry.rs",
                )],
            ))
            .expect("a");
        assert_eq!(
            store
                .search("retry", &ThreadFilter::default(), 10)
                .expect("s")
                .len(),
            1
        );
        assert_eq!(
            store
                .search("etr", &ThreadFilter::default(), 10)
                .expect("s")
                .len(),
            1
        );
    }

    #[test]
    fn a_phrase_is_a_substring_and_never_fuzzy() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "the file_index in backoff");
        let hits = store
            .search("\"ndex in\"", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, HitKind::Substring);
        let typo = store
            .search("\"bakcoff in\"", &ThreadFilter::default(), 10)
            .expect("search");
        assert!(typo.is_empty());
    }

    #[test]
    fn hostile_input_does_not_error_in_any_stage() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "foo bar index backoff");
        let filter = ThreadFilter::default();
        for query in [
            "\"",
            "*",
            "AND (",
            "NEAR(",
            "foo\"bar",
            "^",
            "-",
            "\"unterminated",
            "",
            "*:()^-+",
            "\"index\" OR (",
            "x) AND (y",
            "'\"); DROP TABLE entry;--",
            "bakc\"off",
            "\"\"\"\"",
        ] {
            store
                .search(query, &filter, 10)
                .unwrap_or_else(|e| panic!("{query:?}: {e}"));
        }
    }

    #[test]
    fn deleted_and_scrubbed_text_is_gone_from_every_stage() {
        let mut store = ThreadStore::open_memory().expect("open");
        indexed(&mut store, "a", 1, "file_index backoff");
        indexed(&mut store, "b", 2, "scrub hunter2 soon");
        let filter = ThreadFilter::default();
        assert_eq!(store.search("bakcoff", &filter, 10).expect("s").len(), 1);
        store.delete_thread(&uid("a")).expect("delete");
        assert!(store.search("index", &filter, 10).expect("s").is_empty());
        assert!(store.search("bakcoff", &filter, 10).expect("s").is_empty());
        let scrub = |text: &str| {
            text.contains("hunter2")
                .then(|| text.replace("hunter2", "[redacted]"))
        };
        store.rewrite_texts(&scrub).expect("scrub");
        assert!(store.search("nter2", &filter, 10).expect("s").is_empty());
    }

    #[test]
    fn delete_entry_and_scrub_leave_no_trigram_text_in_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scrubbed = "zqx9scrubbedtoken";
        let deleted = "zqx9deletedentry";
        let mut store = ThreadStore::open(dir.path()).expect("open");
        let mut patched = measured_edit("src/a.rs");
        patched.patch = format!("+{scrubbed}\n");
        let mut tool = new_entry("t", EntryKind::ToolCall, &format!("Bash {scrubbed}"), 2);
        tool.args_json = Some(format!("{{\"cmd\":\"{scrubbed}\"}}"));
        tool.edit = Some(patched);
        store
            .apply(&batch(
                "s",
                2,
                vec![
                    new_entry("u", EntryKind::User, "plain words here", 1),
                    tool,
                    new_entry("x", EntryKind::Assistant, &format!("note {deleted}"), 3),
                    new_entry("y", EntryKind::Assistant, &format!("kept {scrubbed}"), 4),
                ],
            ))
            .expect("apply");
        // Only the assistant reply that kept the token is indexed.
        assert_eq!(trigram_rows(&store, "scrubbedtok"), 1);
        let entry = row_id(&store, "SELECT id FROM entry WHERE source_id = ?1", "x");
        store.delete_entry(entry).expect("delete entry");
        let scrub = |text: &str| {
            text.contains(scrubbed)
                .then(|| text.replace(scrubbed, "[redacted]"))
        };
        // The tool row, the kept reply and the edit patch.
        assert_eq!(store.rewrite_texts(&scrub).expect("scrub"), 3);
        store.checkpoint().expect("checkpoint");
        assert_eq!(trigram_rows(&store, "scrubbedtok"), 0);
        for (name, bytes) in file_bytes(dir.path()) {
            for marker in [scrubbed, deleted] {
                assert!(
                    !contains(&bytes, marker.as_bytes()),
                    "{name} still holds {marker}"
                );
            }
        }
    }

    /// Rows of an older database, written by the schema of its version: every entry kind, and
    /// an edit on the tool call.
    const EVERY_KIND_ROWS: &str = "
        INSERT INTO thread (uid, harness, session_id, worktree, title, started_at, last_event_at)
        VALUES ('old1', 'claude-code', 's1', '/repo', 'old', 1, 1);
        INSERT INTO entry (thread_id, seq, turn, kind, source_id, tool, text, at) VALUES
            (1, 0, 1, 'user', 'u1', NULL, 'please revisit backoff', 1),
            (1, 1, 1, 'tool_call', 't1', 'Bash', 'run plumbago', 2),
            (1, 2, 1, 'tool_result', 'r1', NULL, 'gizmoflux output lines', 3),
            (1, 3, 1, 'notice', 'n1', NULL, 'frabjous notice', 4),
            (1, 4, 1, 'assistant', 'a1', NULL, 'the reply says tangerine', 5);
        INSERT INTO edit (thread_id, entry_id, path, change, patch, added, removed, at)
        VALUES (1, 2, 'src/hexagonal_port.rs', 'modify', '', 0, 0, 2);";

    /// A database file at schema `versions` (1, 2 or 3), with `rows` written after the schema.
    fn legacy_database(dir: &Path, versions: usize, rows: &str) {
        let gm = dir.join("gitmaster");
        std::fs::create_dir_all(&gm).expect("dir");
        let conn = Connection::open(gm.join("threads.db")).expect("raw");
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("wal");
        for sql in &MIGRATIONS[..versions] {
            conn.execute_batch(sql).expect("schema");
        }
        conn.execute_batch(rows).expect("rows");
        conn.pragma_update(
            None,
            "user_version",
            i64::try_from(versions).expect("version"),
        )
        .expect("version");
    }

    fn version_of(dir: &Path) -> i64 {
        let conn = Connection::open(dir.join("gitmaster").join("threads.db")).expect("raw");
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version")
    }

    /// Rows of the word and trigram indexes (their `docsize` tables: one row per indexed entry).
    fn index_rows(store: &ThreadStore) -> (i64, i64) {
        store
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM entry_fts_docsize),
                        (SELECT COUNT(*) FROM entry_tri_docsize)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("index rows")
    }

    /// Hits of the exact, substring and fuzzy stage for a word, its inner part and a typo of it.
    fn stage_hits(store: &ThreadStore, word: &str, inner: &str, typo: &str) -> [usize; 3] {
        let filter = ThreadFilter::default();
        [word, inner, typo].map(|query| store.search(query, &filter, 10).expect("search").len())
    }

    #[test]
    fn only_the_conversation_and_the_edited_paths_are_searched() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut call = with_edit(
            new_entry("t1", EntryKind::ToolCall, "Bash plumbago", 2),
            "src/hexagonal_port.rs",
        );
        call.tool = Some("Bash".to_owned());
        call.args_json = Some("{\"command\":\"zebrafinch\"}".to_owned());
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u1", EntryKind::User, "please revisit backoff", 1),
                    call,
                    new_entry("r1", EntryKind::ToolResult, "gizmoflux output lines", 3),
                    new_entry("n1", EntryKind::Notice, "frabjous notice", 4),
                    new_entry("a1", EntryKind::Assistant, "the reply says tangerine", 5),
                ],
            ))
            .expect("apply");

        // A user prompt and an assistant reply are found by every stage.
        assert_eq!(stage_hits(&store, "backoff", "ckof", "bakcoff"), [1, 1, 1]);
        assert_eq!(
            stage_hits(&store, "tangerine", "angeri", "tangerien"),
            [1, 1, 1]
        );
        // A tool result, a tool call (summary or arguments) and a notice are not.
        assert_eq!(
            stage_hits(&store, "gizmoflux", "moflu", "gizmofulx"),
            [0, 0, 0]
        );
        assert_eq!(
            stage_hits(&store, "plumbago", "umbag", "plumbgao"),
            [0, 0, 0]
        );
        assert_eq!(
            stage_hits(&store, "zebrafinch", "brafin", "zebrafnich"),
            [0, 0, 0]
        );
        assert_eq!(
            stage_hits(&store, "frabjous", "abjou", "frabjuos"),
            [0, 0, 0]
        );
        // The path of an edited file is found, and the hit is the entry that edited it.
        let hits = store
            .search("hexagonal", &ThreadFilter::default(), 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].seq, 1);
    }

    #[test]
    fn tool_entries_do_not_grow_the_indexes() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                1,
                vec![
                    new_entry("u", EntryKind::User, "please revisit backoff", 1),
                    new_entry("a", EntryKind::Assistant, "the reply says tangerine", 2),
                ],
            ))
            .expect("apply");
        let before = index_rows(&store);
        assert_eq!(before, (2, 2));
        let tools: Vec<NewEntry> = (0..40)
            .map(|n| {
                let kind = if n % 2 == 0 {
                    EntryKind::ToolCall
                } else {
                    EntryKind::ToolResult
                };
                new_entry(&format!("t{n}"), kind, &"output line ".repeat(200), 10 + n)
            })
            .collect();
        store.apply(&batch("a", 2, tools)).expect("apply tools");
        assert_eq!(index_rows(&store), before);
    }

    #[test]
    fn a_legacy_database_keeps_only_the_conversation_in_its_indexes() {
        // Versions 1 and 2 index tool rows too; the migration to 3 drops them from the index.
        for versions in [1, 2, 3, 4, 5] {
            let dir = tempfile::tempdir().expect("tempdir");
            legacy_database(dir.path(), versions, EVERY_KIND_ROWS);
            let store = ThreadStore::open(dir.path()).expect("migrate");
            assert_eq!(version_of(dir.path()), 7, "from version {versions}");
            assert_eq!(index_rows(&store), (2, 2), "from version {versions}");
            assert_eq!(stage_hits(&store, "backoff", "ckof", "bakcoff"), [1, 1, 1]);
            assert_eq!(
                stage_hits(&store, "tangerine", "angeri", "tangerien"),
                [1, 1, 1]
            );
            assert_eq!(
                stage_hits(&store, "gizmoflux", "moflu", "gizmofulx"),
                [0, 0, 0]
            );
            assert_eq!(
                stage_hits(&store, "plumbago", "umbag", "plumbgao"),
                [0, 0, 0]
            );
            assert_eq!(
                stage_hits(&store, "frabjous", "abjou", "frabjuos"),
                [0, 0, 0]
            );
            let by_path = store
                .search("hexagonal", &ThreadFilter::default(), 10)
                .expect("search");
            assert_eq!(by_path.len(), 1);
            assert_eq!(by_path[0].seq, 1);
            // The tool text is still stored and shown; only the index left it out.
            let detail = store.read_thread("old1", 0, 10).expect("read");
            assert_eq!(detail.entries[1].text, "run plumbago");
        }
    }

    #[test]
    fn a_version_five_database_gains_the_member_columns_and_keeps_its_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows = format!(
            "{EVERY_KIND_ROWS}
            INSERT INTO thread_link (thread_id, kind, ref, origin)
            VALUES (1, 'commit', 'c0ffee1', 'observed');"
        );
        legacy_database(dir.path(), 5, &rows);

        let store = ThreadStore::open(dir.path()).expect("migrate");

        assert_eq!(version_of(dir.path()), 7);
        let detail = store.read_thread("old1", 0, 10).expect("read");
        assert_eq!(detail.edits.len(), 1);
        assert_eq!(detail.edits[0].path, "src/hexagonal_port.rs");
        assert_eq!(detail.edits[0].member, None);
        assert_eq!(detail.links.len(), 1);
        assert_eq!(detail.links[0].reference, "c0ffee1");
        assert_eq!(detail.links[0].member, None);
        let members: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM thread_member", [], |row| row.get(0))
            .expect("count");
        assert_eq!(members, 0);
    }

    #[test]
    fn a_member_is_stored_on_edits_and_links_and_read_back() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut call = with_edit(
            new_entry("t1", EntryKind::ToolCall, "Edit billing", 2),
            "billing/src/x.rs",
        );
        if let Some(edit) = call.edit.as_mut() {
            edit.member = Some("billing".to_owned());
        }
        let uid = store
            .apply(&batch(
                "a",
                1,
                vec![new_entry("u1", EntryKind::User, "go", 1), call],
            ))
            .expect("apply")
            .uid;
        let link = |kind, reference: &str, member: Option<&str>| ThreadLink {
            kind,
            reference: reference.to_owned(),
            origin: LinkOrigin::Observed,
            resolved: None,
            state: None,
            member: member.map(str::to_owned),
        };
        store
            .link(&uid, &link(LinkKind::Commit, "c0ffee1", Some("billing")))
            .expect("link member commit");
        store
            .link(&uid, &link(LinkKind::Commit, "c0ffee2", None))
            .expect("link workspace commit");

        let detail = store.read_thread(&uid, 0, 100).expect("read");

        assert_eq!(detail.edits[0].member.as_deref(), Some("billing"));
        let member_of = |reference: &str| {
            detail
                .links
                .iter()
                .find(|link| link.reference == reference)
                .and_then(|link| link.member.clone())
        };
        assert_eq!(member_of("c0ffee1").as_deref(), Some("billing"));
        assert_eq!(member_of("c0ffee2"), None);
    }

    #[test]
    fn a_later_link_without_a_member_keeps_the_member_of_the_commit() {
        let mut store = ThreadStore::open_memory().expect("open");
        let uid = store
            .apply(&batch(
                "a",
                1,
                vec![new_entry("u1", EntryKind::User, "go", 1)],
            ))
            .expect("apply")
            .uid;
        let commit = |origin, member: Option<&str>| ThreadLink {
            kind: LinkKind::Commit,
            reference: "c0ffee1".to_owned(),
            origin,
            resolved: None,
            state: None,
            member: member.map(str::to_owned),
        };
        store
            .link(&uid, &commit(LinkOrigin::Observed, Some("billing")))
            .expect("observed");

        store
            .link(&uid, &commit(LinkOrigin::Human, None))
            .expect("linked by hand");

        let detail = store.read_thread(&uid, 0, 100).expect("read");
        assert_eq!(detail.links[0].origin, LinkOrigin::Human);
        assert_eq!(detail.links[0].member.as_deref(), Some("billing"));
    }

    #[test]
    fn a_member_keeps_its_first_head_and_moves_the_last_one() {
        let mut store = ThreadStore::open_memory().expect("open");
        let uid = store
            .apply(&batch(
                "a",
                1,
                vec![new_entry("u1", EntryKind::User, "go", 1)],
            ))
            .expect("apply")
            .uid;
        assert!(store.member_heads(&uid).expect("heads").is_empty());

        store
            .record_member_head(&uid, "billing", "1111111")
            .expect("first");
        store
            .record_member_head(&uid, "billing", "2222222")
            .expect("second");
        store
            .record_member_head(&uid, "docs", "3333333")
            .expect("docs");

        let heads = store.member_heads(&uid).expect("heads");
        let row = |name: &str| heads.iter().find(|row| row.member == name).cloned();
        assert_eq!(heads.len(), 2);
        assert_eq!(
            row("billing"),
            Some(ThreadMember {
                member: "billing".to_owned(),
                head_start: Some("1111111".to_owned()),
                head_end: Some("2222222".to_owned()),
            })
        );
        assert_eq!(
            row("docs").map(|row| row.head_start),
            Some(Some("3333333".to_owned()))
        );

        store.delete_thread(&uid).expect("delete");
        let left: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM thread_member", [], |row| row.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }

    /// Rows of a version 3 database holding Claude Code markers of every kind: a slash command
    /// with its output, a bash run, injected reminders, an interruption, a marker-only first
    /// prompt with an edit, and a title the person edited.
    const MARKER_ROWS: &str = "
        INSERT INTO thread (uid, harness, session_id, worktree, title, started_at, last_event_at)
        VALUES ('m1', 'claude-code', 'm1', '/repo', '<command-name>/model</command-name>', 1, 9),
               ('m2', 'claude-code', 'm2', '/repo', '<local-command-caveat>c</local-command-caveat>', 1, 9),
               ('m3', 'claude-code', 'm3', '/repo', 'Renamed by hand', 1, 9);
        UPDATE thread SET title_edited = 1 WHERE uid = 'm3';
        UPDATE thread SET entry_count = 8, edit_count = 1 WHERE uid = 'm1';
        UPDATE thread SET entry_count = 2 WHERE uid = 'm2';
        INSERT INTO entry (thread_id, seq, turn, kind, source_id, tool, text, at) VALUES
            (1, 0, 1, 'user', 'u1', NULL, '<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>', 1),
            (1, 1, 1, 'assistant', 'a1', NULL, 'Switched to opus.', 2),
            (1, 2, 2, 'user', 'u2', NULL, '<local-command-stdout>\u{1b}[32mSet model\u{1b}[0m</local-command-stdout>', 3),
            (1, 3, 2, 'user', 'u3', NULL, '<bash-input>ls -la</bash-input>', 4),
            (1, 4, 2, 'tool_result', 'r1', NULL, 'gizmo output\n<system-reminder>secretreminder</system-reminder>', 5),
            (1, 5, 2, 'user', 'u4', NULL, 'Please check <system-reminder>secretreminder</system-reminder>the parser', 6),
            (1, 6, 3, 'user', 'u5', NULL, '[Request interrupted by user]', 7),
            (1, 7, 4, 'user', 'u6', NULL, '<local-command-caveat>Caveat: generated</local-command-caveat>', 8),
            (2, 0, 1, 'user', 'c1', NULL, '<local-command-caveat>c</local-command-caveat>', 1),
            (2, 1, 2, 'user', 'c2', NULL, 'fix the second thing', 2),
            (3, 0, 1, 'user', 'e1', NULL, '<command-name>/x</command-name>', 1);
        INSERT INTO edit (thread_id, entry_id, path, change, patch, added, removed, at)
        VALUES (1, (SELECT id FROM entry WHERE source_id = 'u6'), 'src/x.rs', 'modify', '', 0, 0, 8);";

    /// The kinds and texts of a thread, in order.
    fn entries_of(store: &ThreadStore, uid: &str) -> Vec<(&'static str, String)> {
        store
            .read_thread(uid, 0, 50)
            .expect("read")
            .entries
            .into_iter()
            .map(|entry| (kind_text(entry.kind), entry.text))
            .collect()
    }

    /// Both index tables pass their own integrity check, which fails on a stale row.
    fn integrity_checks_pass(store: &ThreadStore) {
        store
            .conn
            .execute_batch(
                "INSERT INTO entry_fts (entry_fts) VALUES ('integrity-check');
                 INSERT INTO entry_tri (entry_tri) VALUES ('integrity-check');
                 INSERT INTO edit_fts (edit_fts) VALUES ('integrity-check');
                 INSERT INTO edit_tri (edit_tri) VALUES ('integrity-check');",
            )
            .expect("the index passes its integrity check");
    }

    #[test]
    fn a_version_three_database_is_repaired_by_the_cleaner() {
        let dir = tempfile::tempdir().expect("tempdir");
        legacy_database(dir.path(), 3, MARKER_ROWS);
        let store = ThreadStore::open(dir.path()).expect("migrate");
        assert_eq!(version_of(dir.path()), 7);

        // Slash command as typed, output and bash as notices and a bash prompt, reminders
        // gone, the interruption as a notice; the marker-only prompt is deleted.
        assert_eq!(
            entries_of(&store, "m1"),
            vec![
                ("user", "/model".to_owned()),
                ("assistant", "Switched to opus.".to_owned()),
                ("notice", "Set model".to_owned()),
                ("user", "! ls -la".to_owned()),
                ("tool_result", "gizmo output".to_owned()),
                ("user", "Please check the parser".to_owned()),
                ("notice", "Request interrupted by user".to_owned()),
            ]
        );
        assert_eq!(
            entries_of(&store, "m2"),
            vec![("user", "fix the second thing".to_owned())]
        );
        assert_eq!(entries_of(&store, "m3"), vec![("user", "/x".to_owned())]);

        // Counters are recounted where an entry and its edit were deleted.
        let m1 = store.read_thread("m1", 0, 50).expect("read").summary;
        assert_eq!((m1.entry_count, m1.edit_count), (7, 0));
        assert_eq!(m1.title, "/model");
        let m2 = store.read_thread("m2", 0, 50).expect("read").summary;
        assert_eq!((m2.entry_count, m2.edit_count), (1, 0));
        assert_eq!(m2.title, "fix the second thing");
        // A title the person edited is not touched.
        let m3 = store.read_thread("m3", 0, 50).expect("read").summary;
        assert_eq!(m3.title, "Renamed by hand");

        // The reminder text was in the index before the repair and is not after it. Only
        // the prompts and replies are indexed: the notices and tool results are not.
        let found = |query: &str| {
            store
                .search(query, &ThreadFilter::default(), 10)
                .expect("search")
        };
        assert_eq!(
            stage_hits(&store, "secretreminder", "ecretremind", "secretremider"),
            [0, 0, 0]
        );
        assert_eq!(uids_of(&found("/model")), ["m1"]);
        assert_eq!(uids_of(&found("parser")), ["m1"]);
        assert!(found("interrupted").is_empty());
        assert_eq!(index_rows(&store), (6, 6));
        let edits: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM edit", [], |row| row.get(0))
            .expect("edits");
        assert_eq!(edits, 0);
        integrity_checks_pass(&store);
    }

    #[test]
    fn a_row_that_changes_kind_leaves_and_enters_the_index() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "k",
                1,
                vec![new_entry("u", EntryKind::User, "the quokka sleeps", 1)],
            ))
            .expect("apply");
        let count = |store: &ThreadStore, word: &str| {
            store
                .search(word, &ThreadFilter::default(), 10)
                .expect("search")
                .len()
        };
        assert_eq!(count(&store, "quokka"), 1);

        store
            .conn
            .execute_batch("UPDATE entry SET kind = 'notice' WHERE source_id = 'u'")
            .expect("to a notice");
        assert_eq!(count(&store, "quokka"), 0);
        assert_eq!(index_rows(&store), (0, 0));
        integrity_checks_pass(&store);

        store
            .conn
            .execute_batch(
                "UPDATE entry SET kind = 'user', text = 'the quokka wakes' WHERE source_id = 'u'",
            )
            .expect("back to a prompt");
        assert_eq!(count(&store, "quokka"), 1);
        assert_eq!(count(&store, "wakes"), 1);
        assert_eq!(index_rows(&store), (1, 1));
        integrity_checks_pass(&store);
    }

    #[test]
    fn the_hook_defers_a_large_migration_and_runs_a_small_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        legacy_database(dir.path(), 2, EVERY_KIND_ROWS);
        // Five entries, and a threshold of five: the hook may not migrate.
        match open_for_hook_below(dir.path(), 5).expect("hook") {
            HookStore::Deferred(reader) => {
                assert_eq!(reader.total_threads().expect("count"), 1);
                // The hook reads the cursor of a session, not filtered searches: those need the
                // label tables that schema 5 adds.
                assert!(
                    reader
                        .cursor("claude-code", "s1")
                        .expect("cursor")
                        .is_some()
                );
            }
            HookStore::Ready(_) => panic!("a large database must not migrate in the hook"),
        }
        assert_eq!(version_of(dir.path()), 2, "the hook left the schema alone");

        // A threshold above the count: the hook migrates as `open` does.
        assert!(matches!(
            open_for_hook_below(dir.path(), 6).expect("hook"),
            HookStore::Ready(_)
        ));
        assert_eq!(version_of(dir.path()), 7);

        // An up-to-date database is opened whatever its size.
        assert!(matches!(
            open_for_hook_below(dir.path(), 1).expect("hook"),
            HookStore::Ready(_)
        ));
        // A new repository gets a new database.
        let fresh = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            open_for_hook_below(fresh.path(), 1).expect("hook"),
            HookStore::Ready(_)
        ));
        assert_eq!(version_of(fresh.path()), 7);
    }

    /// A deterministic generator for the benchmark: no randomness crate, same text every run.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next()).unwrap_or(0) % n
        }
    }

    /// Words of prompts and code: real identifiers plus generated ones.
    fn synthetic_vocabulary() -> Vec<String> {
        let mut words: Vec<String> = [
            "the",
            "a",
            "to",
            "of",
            "and",
            "fix",
            "add",
            "retry",
            "backoff",
            "parser",
            "cache",
            "HistoryStream",
            "file_index",
            "timeout",
            "offset",
            "function",
            "return",
            "error",
            "test",
            "config",
            "branch",
            "commit",
            "rebase",
            "stash",
            "merge",
            "refactor",
            "let",
            "mut",
            "fn",
            "impl",
            "struct",
            "match",
            "Option",
            "Result",
            "self",
            "async",
            "await",
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
        let syllables = [
            "ba", "ke", "ti", "mo", "ra", "sul", "nex", "or", "dim", "fal", "gro", "ven", "pi",
            "qu", "ser", "tan", "ul", "wex", "yo", "zib", "chi", "lum",
        ];
        let mut rng = Lcg(0x00c0_ffee);
        while words.len() < 4_000 {
            let parts = 2 + rng.below(3);
            let mut word = String::new();
            for _ in 0..parts {
                word.push_str(syllables[rng.below(syllables.len())]);
            }
            if rng.below(4) == 0 {
                word.push('_');
                word.push_str(syllables[rng.below(syllables.len())]);
            }
            words.push(word);
        }
        words
    }

    /// One text of `min` to `max` words with code and prose separators.
    fn synthetic_text(rng: &mut Lcg, vocabulary: &[String], min: usize, max: usize) -> String {
        const SEPARATORS: [&str; 7] = [" ", " ", " ", ", ", ". ", "(", "\n"];
        let count = min + rng.below(max - min + 1);
        let mut text = String::new();
        for _ in 0..count {
            text.push_str(&vocabulary[rng.below(vocabulary.len())]);
            text.push_str(SEPARATORS[rng.below(SEPARATORS.len())]);
        }
        text
    }

    /// Times one search: the fastest of three runs, and the stages that answered.
    fn timed(store: &ThreadStore, query: &str, limit: u32) -> (std::time::Duration, Vec<HitKind>) {
        let mut fastest = std::time::Duration::MAX;
        let mut kinds = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let hits = store
                .search(query, &ThreadFilter::default(), limit)
                .expect("search");
            fastest = fastest.min(started.elapsed());
            kinds = hits.iter().map(|hit| hit.kind).collect();
        }
        (fastest, kinds)
    }

    /// Fills a store with the realistic corpus of `bench`: `entries` entries in threads of
    /// `per_thread`. Returns the bytes of conversation and of tool text.
    fn populate(
        store: &mut ThreadStore,
        vocabulary: &[String],
        entries: usize,
        per_thread: usize,
    ) -> (usize, usize) {
        let mut rng = Lcg(0x5eed_1234_abcd_0001);
        let mut conversation_bytes = 0_usize;
        let mut tool_bytes = 0_usize;
        for thread in 0..entries / per_thread {
            let entries: Vec<NewEntry> = (0..per_thread)
                .map(|n| {
                    let at = i64::try_from(thread * per_thread + n + 1).unwrap_or(i64::MAX);
                    match n % 5 {
                        0 | 1 => {
                            let text = synthetic_text(&mut rng, vocabulary, 15, 60);
                            conversation_bytes += text.len();
                            let kind = if n % 5 == 0 {
                                EntryKind::User
                            } else {
                                EntryKind::Assistant
                            };
                            new_entry(&format!("c{n}"), kind, &text, at)
                        }
                        2 => {
                            let text = format!("Bash {}", vocabulary[rng.below(vocabulary.len())]);
                            let args = format!("{{\"command\":\"{}\"}}", text.len());
                            tool_bytes += text.len() + args.len();
                            let mut entry =
                                new_entry(&format!("t{n}"), EntryKind::ToolCall, &text, at);
                            entry.tool = Some("Bash".to_owned());
                            entry.args_json = Some(args);
                            if rng.below(4) == 0 {
                                entry.edit = Some(measured_edit(&format!(
                                    "src/{}/{}.rs",
                                    vocabulary[rng.below(vocabulary.len())],
                                    vocabulary[rng.below(vocabulary.len())]
                                )));
                            }
                            entry
                        }
                        _ => {
                            let text = synthetic_text(&mut rng, vocabulary, 100, 300);
                            tool_bytes += text.len();
                            let mut entry =
                                new_entry(&format!("r{n}"), EntryKind::ToolResult, &text, at);
                            entry.tool = Some("Read".to_owned());
                            entry
                        }
                    }
                })
                .collect();
            let at = i64::try_from(thread).unwrap_or(i64::MAX);
            store
                .apply(&batch(&format!("s{thread}"), at, entries))
                .expect("apply");
        }
        (conversation_bytes, tool_bytes)
    }

    /// The measurement behind D131 and D132: synthetic entries shaped like real threads, the
    /// database size with and without the trigram tables, and the time of each kind of search.
    /// Per 100 entries: 40 conversation entries (user prompts and assistant replies of 15 to 60
    /// words) and 60 tool entries (20 calls, 40 results of 100 to 300 words), so about 85% of the
    /// bytes are tool results. It is not a regression test; run it by hand (see
    /// `design/decisions.md`, D132):
    /// `cargo test -p gm-core --lib threads::store::tests::bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement: builds 100,000 entries, run by hand"]
    fn bench() {
        const ENTRIES: usize = 100_000;
        const PER_THREAD: usize = 100;
        const LIMIT: u32 = 50;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ThreadStore::open(dir.path()).expect("open");
        let vocabulary = synthetic_vocabulary();

        let started = std::time::Instant::now();
        let (conversation_bytes, tool_bytes) =
            populate(&mut store, &vocabulary, ENTRIES, PER_THREAD);
        eprintln!("populate {ENTRIES} entries: {:?}", started.elapsed());
        let share = tool_bytes * 100 / (tool_bytes + conversation_bytes).max(1);
        eprintln!("corpus: {share}% of the text bytes are tool calls and results");

        store
            .conn
            .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("vacuum");
        let db = dir.path().join("gitmaster").join("threads.db");
        let with = std::fs::metadata(&db).expect("size").len();
        let copy = dir.path().join("without.db");
        store
            .conn
            .execute_batch(&format!("VACUUM INTO '{}';", copy.display()))
            .expect("copy");
        {
            let raw = Connection::open(&copy).expect("copy open");
            raw.execute_batch(
                "DROP TRIGGER entry_tri_ai; DROP TRIGGER entry_tri_ad; DROP TRIGGER entry_tri_au;
                 DROP TRIGGER edit_tri_ai; DROP TRIGGER edit_tri_ad; DROP TRIGGER edit_tri_au;
                 DROP TABLE entry_tri; DROP TABLE edit_tri; VACUUM;",
            )
            .expect("drop trigram tables");
        }
        let without = std::fs::metadata(&copy).expect("size").len();
        // Ratio in thousandths, in integers.
        let permille = with * 1000 / without.max(1);
        eprintln!(
            "db size: {with} bytes with trigram tables, {without} without; ratio {}.{:03}",
            permille / 1000,
            permille % 1000
        );

        for (label, query, bound) in [
            ("strict", "backoff", 2_u64),
            ("substring", "ckof", 10),
            ("fuzzy", "bakcoff", 10),
        ] {
            let (fastest, kinds) = timed(&store, query, LIMIT);
            let exact = kinds.iter().filter(|k| **k == HitKind::Exact).count();
            let substring = kinds.iter().filter(|k| **k == HitKind::Substring).count();
            let fuzzy = kinds.iter().filter(|k| **k == HitKind::Fuzzy).count();
            eprintln!(
                "{label} search {query:?}: {fastest:?} (exact {exact}, substring {substring}, fuzzy {fuzzy})"
            );
            assert!(
                fastest < std::time::Duration::from_secs(bound),
                "{label} took {fastest:?}, bound {bound} s"
            );
        }
    }

    /// A search request with the options of a first page: entries, relevance, no context.
    fn request<'a>(query: &'a str, filter: &'a ThreadFilter, limit: u32) -> SearchRequest<'a> {
        SearchRequest {
            query,
            filter,
            limit,
            cursor: None,
            group: SearchGroup::Entry,
            order: SearchOrder::Relevance,
            context: 0,
            facets: false,
        }
    }

    /// The `(thread, seq)` pairs of hits, in order.
    fn keys(hits: &[ThreadHit]) -> Vec<(String, u32)> {
        hits.iter()
            .map(|hit| (hit.thread_uid.clone(), hit.seq))
            .collect()
    }

    /// Every page of a request, each as its `(thread, seq)` pairs, following `next_cursor`
    /// until `has_more` is false. A last page carries no cursor.
    fn pages_of(store: &ThreadStore, base: SearchRequest<'_>) -> Vec<Vec<(String, u32)>> {
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let page = store
                .search_page(&SearchRequest {
                    cursor: cursor.as_deref(),
                    ..base
                })
                .expect("page");
            pages.push(keys(&page.hits));
            if !page.has_more {
                assert!(page.next_cursor.is_none(), "a last page has no cursor");
                return pages;
            }
            cursor = Some(page.next_cursor.expect("a cursor while more pages follow"));
        }
        panic!("the pages never end");
    }

    #[test]
    fn cursor_paging_neither_repeats_nor_skips_a_hit() {
        let mut store = ThreadStore::open_memory().expect("open");
        for session in ["a", "b", "c", "d", "e"] {
            store
                .apply(&batch(
                    session,
                    100,
                    vec![
                        new_entry("u", EntryKind::User, "backoff first", 1),
                        new_entry("x", EntryKind::Assistant, "backoff second", 2),
                    ],
                ))
                .expect("apply");
        }
        let filter = ThreadFilter::default();
        for order in [SearchOrder::Relevance, SearchOrder::Recent] {
            let whole = store
                .search_page(&SearchRequest {
                    order,
                    ..request("backoff", &filter, 100)
                })
                .expect("whole");
            let whole = keys(&whole.hits);
            assert_eq!(whole.len(), 10, "{order:?}");

            let base = SearchRequest {
                order,
                ..request("backoff", &filter, 3)
            };
            let first = store.search_page(&base).expect("first");
            assert!(first.has_more, "{order:?}");
            assert!(first.next_cursor.is_some(), "{order:?}");

            let pages = pages_of(&store, base);
            let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
            assert_eq!(sizes, vec![3, 3, 3, 1], "{order:?}");
            let walked: Vec<(String, u32)> = pages.into_iter().flatten().collect();
            assert_eq!(walked, whole, "{order:?}");
        }
    }

    #[test]
    fn a_thread_group_page_walks_each_thread_once() {
        let mut store = ThreadStore::open_memory().expect("open");
        for session in ["a", "b", "c", "d", "e"] {
            store
                .apply(&batch(
                    session,
                    100,
                    vec![
                        new_entry("u", EntryKind::User, "backoff first", 1),
                        new_entry("x", EntryKind::Assistant, "backoff second", 2),
                    ],
                ))
                .expect("apply");
        }
        let filter = ThreadFilter::default();
        let base = SearchRequest {
            group: SearchGroup::Thread,
            ..request("backoff", &filter, 2)
        };
        let pages = pages_of(&store, base);
        let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
        let uids: HashSet<String> = pages.iter().flatten().map(|(uid, _)| uid.clone()).collect();
        assert_eq!(uids.len(), 5);
    }

    #[test]
    fn a_cursor_from_another_search_is_refused() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&prompted("a", 100, "backoff jitter"))
            .expect("a");
        store
            .apply(&prompted("b", 200, "backoff retry"))
            .expect("b");
        let filter = ThreadFilter::default();
        let first = store
            .search_page(&request("backoff", &filter, 1))
            .expect("first");
        let cursor = first.next_cursor.expect("a cursor");
        let resume = |query: &str, filter: &ThreadFilter, group, order| {
            store.search_page(&SearchRequest {
                cursor: Some(cursor.as_str()),
                group,
                order,
                ..request(query, filter, 1)
            })
        };
        let same = resume(
            "backoff",
            &filter,
            SearchGroup::Entry,
            SearchOrder::Relevance,
        );
        assert_eq!(same.expect("the same search resumes").hits.len(), 1);

        let other_filter = ThreadFilter {
            harness: Some("codex".to_owned()),
            ..ThreadFilter::default()
        };
        let refused = [
            resume(
                "jitter",
                &filter,
                SearchGroup::Entry,
                SearchOrder::Relevance,
            ),
            resume(
                "backoff",
                &other_filter,
                SearchGroup::Entry,
                SearchOrder::Relevance,
            ),
            resume(
                "backoff",
                &filter,
                SearchGroup::Thread,
                SearchOrder::Relevance,
            ),
            resume("backoff", &filter, SearchGroup::Entry, SearchOrder::Recent),
        ];
        for result in refused {
            let error = result.expect_err("a cursor of another search");
            assert!(error.to_string().contains("another search"), "{error}");
        }

        for text in ["zz", "abc", "ff"] {
            let garbage = store.search_page(&SearchRequest {
                cursor: Some(text),
                ..request("backoff", &filter, 1)
            });
            assert!(garbage.is_err(), "{text} is not a cursor");
        }
    }

    #[test]
    fn thread_group_keeps_one_hit_per_thread_and_entry_group_keeps_all() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                100,
                vec![
                    new_entry("u", EntryKind::User, "backoff one", 1),
                    new_entry("x", EntryKind::Assistant, "backoff two", 2),
                ],
            ))
            .expect("a");
        store
            .apply(&prompted("b", 200, "backoff three"))
            .expect("b");
        let filter = ThreadFilter::default();
        let by_thread = store
            .search_page(&SearchRequest {
                group: SearchGroup::Thread,
                ..request("backoff", &filter, 10)
            })
            .expect("thread");
        let by_entry = store
            .search_page(&request("backoff", &filter, 10))
            .expect("entry");
        assert_eq!(by_thread.hits.len(), 2);
        let uids: HashSet<&str> = by_thread
            .hits
            .iter()
            .map(|hit| hit.thread_uid.as_str())
            .collect();
        assert_eq!(uids.len(), 2);
        assert!(!by_thread.has_more);
        assert_eq!(by_entry.hits.len(), 3);
    }

    #[test]
    fn relevance_ranks_by_match_and_recent_by_entry_time() {
        let mut store = ThreadStore::open_memory().expect("open");
        // Equal length, so the four matches of "strong" outrank the one of "recent".
        store
            .apply(&batch(
                "strong",
                10,
                vec![new_entry(
                    "u",
                    EntryKind::User,
                    "flaky flaky flaky flaky",
                    10,
                )],
            ))
            .expect("strong");
        store
            .apply(&batch(
                "recent",
                500,
                vec![new_entry("u", EntryKind::User, "flaky test here now", 500)],
            ))
            .expect("recent");
        let filter = ThreadFilter::default();
        let relevance = store
            .search_page(&request("flaky", &filter, 10))
            .expect("relevance");
        let uids: Vec<&str> = relevance
            .hits
            .iter()
            .map(|hit| hit.thread_uid.as_str())
            .collect();
        assert_eq!(uids, vec![uid("strong"), uid("recent")]);

        let recent = store
            .search_page(&SearchRequest {
                order: SearchOrder::Recent,
                ..request("flaky", &filter, 10)
            })
            .expect("recent");
        let uids: Vec<&str> = recent
            .hits
            .iter()
            .map(|hit| hit.thread_uid.as_str())
            .collect();
        assert_eq!(uids, vec![uid("recent"), uid("strong")]);
        assert_eq!(recent.hits[0].at, 500);
    }

    #[test]
    fn context_is_up_to_three_conversation_entries_each_side_and_capped() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                100,
                vec![
                    new_entry("s", EntryKind::User, "start of the session", 1),
                    new_entry("a1", EntryKind::Assistant, "first answer", 2),
                    new_entry("t", EntryKind::ToolCall, "Bash", 3),
                    new_entry("h", EntryKind::User, "backoff hit here", 4),
                    new_entry("a2", EntryKind::Assistant, "second answer", 5),
                    new_entry("q", EntryKind::User, "later question", 6),
                    new_entry("a3", EntryKind::Assistant, "last answer", 7),
                ],
            ))
            .expect("a");
        store
            .apply(&batch(
                "b",
                200,
                vec![
                    new_entry("h", EntryKind::User, "backoff alone", 1),
                    new_entry("x", EntryKind::Assistant, &"z".repeat(1000), 2),
                ],
            ))
            .expect("b");
        let filter = ThreadFilter::default();
        let page = |context: u8| {
            store
                .search_page(&SearchRequest {
                    context,
                    ..request("backoff", &filter, 10)
                })
                .expect("search")
        };
        let context_of = |context: u8| -> Vec<u32> {
            let page = page(context);
            let hit = page
                .hits
                .iter()
                .find(|hit| hit.thread_uid == uid("a"))
                .expect("the hit of a");
            hit.context.iter().map(|entry| entry.seq).collect()
        };
        assert!(context_of(0).is_empty());
        assert_eq!(context_of(1), vec![1, 4]);
        assert_eq!(context_of(2), vec![0, 1, 4, 5]);
        assert_eq!(context_of(3), vec![0, 1, 4, 5, 6]);
        assert_eq!(context_of(9), vec![0, 1, 4, 5, 6]);

        let capped = page(1);
        let hit = capped
            .hits
            .iter()
            .find(|hit| hit.thread_uid == uid("b"))
            .expect("the hit of b");
        assert_eq!(hit.context.len(), 1);
        assert_eq!(hit.context[0].text.chars().count(), CONTEXT_CHARS);
    }

    #[test]
    fn facets_count_the_matching_threads_only() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                100,
                vec![
                    new_entry("u", EntryKind::User, "backoff a", 1),
                    with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "src/a.rs"),
                ],
            ))
            .expect("a");
        store
            .apply(&batch(
                "b",
                200,
                vec![
                    new_entry("u", EntryKind::User, "backoff b", 1),
                    with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "src/a.rs"),
                    with_edit(new_entry("s", EntryKind::ToolCall, "Edit", 3), "src/b.rs"),
                ],
            ))
            .expect("b");
        store
            .apply(&Batch {
                harness: "codex".to_owned(),
                branch: Some("feature".to_owned()),
                ..batch(
                    "c",
                    300,
                    vec![
                        new_entry("u", EntryKind::User, "unrelated words", 1),
                        with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "src/c.rs"),
                    ],
                )
            })
            .expect("c");
        store
            .apply(&Batch {
                branch: None,
                ..batch(
                    "d",
                    400,
                    vec![new_entry("u", EntryKind::User, "backoff d", 1)],
                )
            })
            .expect("d");
        let filter = ThreadFilter::default();

        let page = store
            .search_page(&SearchRequest {
                facets: true,
                ..request("backoff", &filter, 10)
            })
            .expect("search");
        let facets = page.facets.expect("facets");
        assert_eq!(facets.branch, vec![("main".to_owned(), 2)]);
        assert_eq!(facets.harness, vec![("claude-code".to_owned(), 3)]);
        assert_eq!(
            facets.path,
            vec![("src/a.rs".to_owned(), 2), ("src/b.rs".to_owned(), 1)]
        );
        let plain = store
            .search_page(&request("backoff", &filter, 10))
            .expect("plain");
        assert!(plain.facets.is_none());

        let feature = ThreadFilter {
            branch: Some("feature".to_owned()),
            ..ThreadFilter::default()
        };
        let listed = store
            .search_page(&SearchRequest {
                facets: true,
                ..request("", &feature, 10)
            })
            .expect("listing");
        assert_eq!(listed.hits.len(), 1);
        let facets = listed.facets.expect("facets");
        assert_eq!(facets.branch, vec![("feature".to_owned(), 1)]);
        assert_eq!(facets.harness, vec![("codex".to_owned(), 1)]);
        assert_eq!(facets.path, vec![("src/c.rs".to_owned(), 1)]);
    }

    #[test]
    fn an_empty_result_says_which_filter_removed_the_most() {
        let mut store = ThreadStore::open_memory().expect("open");
        store
            .apply(&batch(
                "a",
                100,
                vec![
                    new_entry("u", EntryKind::User, "backoff a", 1),
                    with_edit(new_entry("t", EntryKind::ToolCall, "Edit", 2), "src/a.rs"),
                ],
            ))
            .expect("a");
        store
            .apply(&Batch {
                branch: Some("feature".to_owned()),
                ..batch(
                    "b",
                    200,
                    vec![new_entry("u", EntryKind::User, "backoff b", 1)],
                )
            })
            .expect("b");
        // Each filter alone keeps one thread, but no thread has both.
        let both = ThreadFilter {
            branch: Some("feature".to_owned()),
            has_edits: Some(true),
            ..ThreadFilter::default()
        };
        let page = store
            .search_page(&request("backoff", &both, 10))
            .expect("search");
        assert!(page.hits.is_empty());
        assert_eq!(
            page.diagnosis,
            Some(vec![("branch".to_owned(), 1), ("has_edits".to_owned(), 1)])
        );
        let listed = store.search_page(&request("", &both, 10)).expect("listing");
        assert_eq!(listed.diagnosis, page.diagnosis);

        let found = store
            .search_page(&request(
                "backoff",
                &ThreadFilter {
                    branch: Some("main".to_owned()),
                    ..ThreadFilter::default()
                },
                10,
            ))
            .expect("found");
        assert_eq!(found.hits.len(), 1);
        assert!(found.diagnosis.is_none());

        let nothing = store
            .search_page(&request("nowhere", &ThreadFilter::default(), 10))
            .expect("nothing");
        assert!(nothing.diagnosis.is_none());
    }

    #[test]
    fn empty_query_lists_threads_newest_first_in_pages_without_gaps() {
        let mut store = ThreadStore::open_memory().expect("open");
        for (session, at) in [("a", 100), ("b", 300), ("c", 200), ("d", 300), ("e", 50)] {
            store
                .apply(&prompted(session, at, "task x"))
                .expect("apply");
        }
        let filter = ThreadFilter::default();
        let all = store.list_threads(&filter, None, 100).expect("list");
        assert_eq!(all.len(), 5);
        assert!(
            all.windows(2)
                .all(|pair| pair[0].last_event_at >= pair[1].last_event_at)
        );
        let expected: Vec<(String, u32)> =
            all.iter().map(|thread| (thread.uid.clone(), 0)).collect();

        let base = request("", &filter, 2);
        let pages = pages_of(&store, base);
        let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
        let walked: Vec<(String, u32)> = pages.into_iter().flatten().collect();
        assert_eq!(walked, expected);

        let first = store.search_page(&base).expect("first");
        assert_eq!(first.hits[0].snippet, first.hits[0].title);

        let search = store
            .search_page(&request("task", &filter, 2))
            .expect("search");
        let cursor = search.next_cursor.expect("a cursor");
        let misplaced = store.search_page(&SearchRequest {
            cursor: Some(cursor.as_str()),
            ..request("", &filter, 2)
        });
        assert!(misplaced.is_err(), "a search cursor does not list");

        let nothing = ThreadFilter {
            branch: Some("nope".to_owned()),
            ..ThreadFilter::default()
        };
        let empty = store
            .search_page(&request("", &nothing, 2))
            .expect("empty listing");
        assert!(empty.hits.is_empty());
        assert!(!empty.has_more);
        assert_eq!(empty.diagnosis, Some(vec![("branch".to_owned(), 5)]));
    }

    /// Rows of a version 4 database: two threads, a manual and an observed commit link, tags on
    /// both threads, and edits whose paths are absolute under the worktree, repository-relative,
    /// or outside the worktree.
    const VERSION_FOUR_ROWS: &str = "
        INSERT INTO thread (uid, harness, session_id, worktree, title, started_at, last_event_at)
        VALUES ('t1uid', 'claude-code', 's1', '/repo', 'one', 1, 1),
               ('t2uid', 'claude-code', 's2', '/repo', 'two', 2, 2);
        INSERT INTO entry (thread_id, seq, turn, kind, source_id, text, at) VALUES
            (1, 0, 1, 'user', 'u1', 'hello', 1),
            (1, 1, 1, 'tool_call', 't1', 'edit', 2),
            (2, 0, 1, 'user', 'u2', 'hi', 3);
        INSERT INTO edit (thread_id, entry_id, path, change, patch, added, removed, at) VALUES
            (1, 2, '/repo/src/a.rs', 'modify', '', 0, 0, 2),
            (1, 2, 'src/b.rs', 'modify', '', 0, 0, 2),
            (2, 3, '/elsewhere/c.rs', 'modify', '', 0, 0, 3);
        INSERT INTO thread_link (thread_id, kind, ref, origin) VALUES
            (1, 'commit', 'abc1234', 'manual'),
            (2, 'commit', 'deadbeef', 'observed');
        INSERT INTO thread_tag (thread_id, tag) VALUES (1, 'bug'), (1, 'api'), (2, 'bug');";

    fn raw_connection(dir: &Path) -> Connection {
        Connection::open(dir.join("gitmaster").join("threads.db")).expect("raw")
    }

    #[test]
    fn a_version_four_database_keeps_every_tag_as_a_human_label() {
        let dir = tempfile::tempdir().expect("tempdir");
        legacy_database(dir.path(), 4, VERSION_FOUR_ROWS);
        let store = ThreadStore::open(dir.path()).expect("migrate");
        assert_eq!(version_of(dir.path()), 7);
        assert_eq!(
            store.thread_summary("t1uid").expect("t1").tags,
            ["api", "bug"]
        );
        assert_eq!(store.thread_summary("t2uid").expect("t2").tags, ["bug"]);
        let raw = raw_connection(dir.path());
        let labels: Vec<(String, String)> = raw
            .prepare("SELECT name, origin FROM label ORDER BY name")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(
            labels,
            [
                ("api".to_owned(), "human".to_owned()),
                ("bug".to_owned(), "human".to_owned())
            ]
        );
        let attached: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM label_attach
                 WHERE kind = 'thread' AND origin = 'human' AND ref IN ('t1uid', 't2uid')",
                [],
                |row| row.get(0),
            )
            .expect("attachments");
        assert_eq!(attached, 3);
        let kept: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'thread_tag'",
                [],
                |row| row.get(0),
            )
            .expect("master");
        assert_eq!(kept, 0, "thread_tag is dropped");
    }

    #[test]
    fn the_migration_to_schema_seven_keeps_threads_and_adds_memories() {
        let dir = tempfile::tempdir().expect("tempdir");
        legacy_database(
            dir.path(),
            6,
            "INSERT INTO thread (uid, harness, session_id, worktree, title, started_at, last_event_at)
             VALUES ('old1', 'claude-code', 's1', '/repo', 'old', 1, 1);",
        );
        let store = ThreadStore::open(dir.path()).expect("migrate");
        assert_eq!(store.total_threads().expect("count"), 1);
        assert_eq!(version_of(dir.path()), 7);
        let raw = raw_connection(dir.path());
        for name in ["memory", "memory_source", "memory_rel", "memory_fts"] {
            let found: i64 = raw
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .expect("master");
            assert_eq!(found, 1, "{name} exists");
        }
    }

    #[test]
    fn the_migration_fills_rel_path_and_renames_manual_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        legacy_database(dir.path(), 4, VERSION_FOUR_ROWS);
        let store = ThreadStore::open(dir.path()).expect("migrate");
        let raw = raw_connection(dir.path());
        let paths: Vec<(String, String)> = raw
            .prepare("SELECT path, rel_path FROM edit ORDER BY id")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(
            paths,
            [
                ("/repo/src/a.rs".to_owned(), "src/a.rs".to_owned()),
                ("src/b.rs".to_owned(), "src/b.rs".to_owned()),
                ("/elsewhere/c.rs".to_owned(), "/elsewhere/c.rs".to_owned()),
            ]
        );
        let index: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'edit_rel_path'",
                [],
                |row| row.get(0),
            )
            .expect("index");
        assert_eq!(index, 1);
        let links: Vec<(String, String)> = raw
            .prepare("SELECT ref, origin FROM thread_link ORDER BY ref")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(
            links,
            [
                ("abc1234".to_owned(), "human".to_owned()),
                ("deadbeef".to_owned(), "observed".to_owned()),
            ]
        );
        // The provenance lookup finds the edit recorded under the worktree by its repository path.
        let found = store
            .path_edits("src/a.rs", i64::MAX, None, 10)
            .expect("path edits");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].thread_uid, "t1uid");
        // A thread's tag filter reads the labels.
        let tagged = store
            .list_threads(
                &ThreadFilter {
                    tag: Some("bug".to_owned()),
                    ..ThreadFilter::default()
                },
                None,
                10,
            )
            .expect("list");
        assert_eq!(tagged.len(), 2);
    }

    #[test]
    fn a_new_edit_fills_rel_path_from_its_worktree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = ThreadStore::open(dir.path()).expect("open");
        let absolute = with_edit(
            new_entry("u", EntryKind::ToolCall, "Edit", 5),
            "/repo/src/x.rs",
        );
        let relative = with_edit(new_entry("v", EntryKind::ToolCall, "Edit", 6), "src/y.rs");
        store
            .apply(&batch("s", 5, vec![absolute, relative]))
            .expect("apply");
        let rel: Vec<String> = raw_connection(dir.path())
            .prepare("SELECT rel_path FROM edit ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(rel, ["src/x.rs", "src/y.rs"]);
    }

    #[test]
    fn a_workspace_store_opens_read_only_only_when_it_exists_with_this_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join(".git");
        std::fs::create_dir(&common).expect("common dir");
        // Missing: nothing is created.
        assert!(
            ThreadStore::open_workspace_read_only(&common)
                .expect("no error")
                .is_none()
        );
        assert!(!common.join("gitmaster").exists(), "no folder is created");

        drop(ThreadStore::open(&common).expect("create the store"));
        assert!(
            ThreadStore::open_workspace_read_only(&common)
                .expect("no error")
                .is_some()
        );

        // Another schema: skipped, and the file is not migrated.
        let path = common.join("gitmaster").join("threads.db");
        let conn = Connection::open(&path).expect("open");
        conn.pragma_update(None, "user_version", 99)
            .expect("bump the version");
        drop(conn);
        assert!(
            ThreadStore::open_workspace_read_only(&common)
                .expect("no error")
                .is_none()
        );
        let conn = Connection::open(&path).expect("open");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, 99, "a foreign schema is left as it is");
    }
}
