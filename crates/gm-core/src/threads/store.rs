//! `threads::store`: the SQLite store of agent threads, `.git/gitmaster/threads.db`
//! (design/threads.md, "Storage (D122)"). Writes go through `apply`, one transaction
//! per batch and idempotent on replay. Reads return the rows the engine shows.
//! Nothing here knows a harness: callers hand over canonical entries.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use gm_schema::threads::{
    CaptureGap, EditChange, EntryKind, LinkKind, LinkOrigin, ThreadDetail, ThreadEdit, ThreadEntry,
    ThreadFilter, ThreadHit, ThreadLink, ThreadSummary,
};
use rusqlite::{
    Connection, OptionalExtension, Row, ToSql, Transaction, TransactionBehavior, params,
};
use sha2::{Digest, Sha256};

use super::query::build_fts_query;

/// Migrations in order; `PRAGMA user_version` counts how many have run.
const MIGRATIONS: &[&str] = &[SCHEMA_V1];

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

const BUSY_TIMEOUT: Duration = Duration::from_millis(500);
/// Opening a file another process is initializing is retried this many times, this far apart.
const OPEN_ATTEMPTS: u32 = 200;
const OPEN_PAUSE: Duration = Duration::from_millis(5);
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
    AND (:path IS NULL OR EXISTS (SELECT 1 FROM edit f WHERE f.thread_id = t.id AND f.path = :path))
    AND (:tag IS NULL OR EXISTS (SELECT 1 FROM thread_tag g WHERE g.thread_id = t.id AND g.tag = :tag))";

const EDIT_COLUMNS: &str = "id, entry_id, path, old_path, change, patch, added, removed, pre_oid, post_oid, from_input, at";

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

/// One batch of canonical events for one session, as the pipeline hands it over.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Per-pattern counts for this batch. Added to the thread only when at least one entry
    /// of the batch is new, so a replay adds nothing.
    pub redactions: Vec<(String, u32)>,
    /// `(at, reason)` for events dropped before storage. A repeated row is not added again.
    pub gaps: Vec<(i64, String)>,
}

/// One entry of a batch, redacted and capped by the pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub redactions: u32,
    pub at: i64,
    pub edit: Option<NewEdit>,
}

/// A file edit carried by an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The complete new tag set.
    pub tags: Option<Vec<String>>,
}

pub struct ThreadStore {
    conn: Connection,
    /// Entries kept per thread; tests lower it.
    entry_limit: u32,
}

impl ThreadStore {
    /// Opens (creating `gitmaster/threads.db` under `git_common_dir` when missing) and migrates
    /// the store. The file and its WAL and shared-memory files are private (mode `0600`).
    pub fn open(git_common_dir: &Path) -> Result<Self, StoreError> {
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
        let conn = open_connection(&path)?;
        Ok(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
        })
    }

    /// A store that lives only in memory (tests).
    pub fn open_memory() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()?;
        prepare(&mut conn)?;
        Ok(Self {
            conn,
            entry_limit: DEFAULT_ENTRY_LIMIT,
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
        if applied.inserted > 0 {
            add_redactions(&tx, thread, &batch.redactions)?;
        }
        for (at, reason) in &batch.gaps {
            record_gap(&tx, thread, *at, reason)?;
        }
        state.write_counters(&tx, thread)?;
        tx.commit()?;
        Ok(applied)
    }

    /// Thread summaries, newest `last_event_at` first. `before` is the `(last_event_at, id)`
    /// of the last row of the previous page.
    pub fn list_threads(
        &self,
        filter: &ThreadFilter,
        before: Option<(i64, i64)>,
        limit: u32,
    ) -> Result<Vec<ThreadSummary>, StoreError> {
        let (before_at, before_id) = match before {
            Some((at, id)) => (Some(at), Some(id)),
            None => (None, None),
        };
        let limit = i64::from(limit);
        let sql = format!(
            "{SUMMARY_SQL} WHERE 1 = 1{FILTER_SQL}
             AND (:before_at IS NULL OR (t.last_event_at, t.id) < (:before_at, :before_id))
             ORDER BY t.last_event_at DESC, t.id DESC LIMIT :limit"
        );
        let mut bindings = thread_bindings(filter);
        bindings.push((":before_at", &before_at));
        bindings.push((":before_id", &before_id));
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

    /// Hits for `user_query` (see `query::build_fts_query`), best first. Matches in entry
    /// text and tool names and in edit paths. One hit per entry. Text that makes no query
    /// returns no hits.
    pub fn search(
        &self,
        user_query: &str,
        filter: &ThreadFilter,
        limit: u32,
    ) -> Result<Vec<ThreadHit>, StoreError> {
        let Some(fts) = build_fts_query(user_query) else {
            return Ok(Vec::new());
        };
        let start = SNIPPET_MARK_START.to_owned();
        let end = SNIPPET_MARK_END.to_owned();
        let limit_value = i64::from(limit);
        let mut bindings = thread_bindings(filter);
        bindings.push((":fts", &fts));
        bindings.push((":mark_start", &start));
        bindings.push((":mark_end", &end));
        bindings.push((":limit", &limit_value));

        let mut scored: Vec<(f64, ThreadHit)> = Vec::new();
        for sql in [entry_search_sql(), edit_search_sql()] {
            let mut statement = self.conn.prepare(&sql)?;
            let rows = statement.query_map(bindings.as_slice(), hit_row)?;
            for row in rows {
                scored.push(row?);
            }
        }
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));

        let max_hits = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for (_, hit) in scored {
            if hits.len() >= max_hits {
                break;
            }
            if seen.insert(hit.entry_id) {
                hits.push(hit);
            }
        }
        Ok(hits)
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
                "SELECT kind, ref, origin FROM thread_link WHERE thread_id = ?1 ORDER BY kind, ref",
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

        Ok(ThreadDetail {
            summary,
            entries,
            edits,
            links,
            redactions,
            has_more,
        })
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
        if let Some(tags) = &meta.tags {
            tx.execute(
                "DELETE FROM thread_tag WHERE thread_id = ?1",
                params![thread],
            )?;
            for tag in tags {
                let tag = tag.trim();
                if tag.is_empty() {
                    return Err(invalid("a tag cannot be empty"));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO thread_tag (thread_id, tag) VALUES (?1, ?2)",
                    params![thread, tag],
                )?;
            }
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
            "INSERT INTO thread_link (thread_id, kind, ref, origin) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (thread_id, kind, ref) DO UPDATE SET origin = excluded.origin",
            params![
                thread,
                link_kind_text(link.kind),
                link.reference,
                origin_text(link.origin)
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

    /// Deletes a thread with its entries, edits, links, tags and gaps. The text is removed
    /// from the search index too, so `checkpoint` then leaves it out of the file.
    pub fn delete_thread(&mut self, uid: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let thread = thread_id(&tx, uid)?;
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
fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    let mut attempts = 0_u32;
    loop {
        let mut conn = Connection::open(path)?;
        match prepare(&mut conn) {
            Ok(()) => return Ok(conn),
            Err(StoreError::Sqlite(error)) if is_busy(&error) && attempts < OPEN_ATTEMPTS => {
                attempts += 1;
                std::thread::sleep(OPEN_PAUSE);
            }
            Err(error) => return Err(error),
        }
    }
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::DatabaseBusy
                || failure.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// Opens the connection, sets the pragmas and migrates.
fn prepare(conn: &mut Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "secure_delete", true)?;
    migrate(conn)
}

/// Runs the migrations the database has not seen, in one immediate transaction so two
/// processes opening a new file do not both create the schema.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
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
    for sql in &MIGRATIONS[from..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", latest)?;
    tx.commit()?;
    Ok(())
}

/// Creates the file with mode `0600` when missing, and makes it private when it is not.
fn create_private(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

fn begin(conn: &mut Connection) -> rusqlite::Result<Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
}

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.to_owned())
}

/// SQLite stores integers as signed 64-bit values; a larger unsigned value is refused.
fn to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| invalid("a number is too large to store"))
}

fn thread_id(conn: &Connection, uid: &str) -> Result<i64, StoreError> {
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

fn tags_of(conn: &Connection, thread: i64) -> Result<Vec<String>, StoreError> {
    let mut statement =
        conn.prepare_cached("SELECT tag FROM thread_tag WHERE thread_id = ?1 ORDER BY tag")?;
    let tags = statement
        .query_map(params![thread], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(tags)
}

/// The filter's parameters, in the names `FILTER_SQL` uses.
fn thread_bindings(filter: &ThreadFilter) -> Vec<(&'static str, &dyn ToSql)> {
    let mut bindings: Vec<(&'static str, &dyn ToSql)> = Vec::with_capacity(10);
    bindings.push((":harness", &filter.harness));
    bindings.push((":branch", &filter.branch));
    bindings.push((":since", &filter.since));
    bindings.push((":until", &filter.until));
    bindings.push((":starred", &filter.starred));
    bindings.push((":has_edits", &filter.has_edits));
    bindings.push((":path", &filter.path));
    bindings.push((":tag", &filter.tag));
    bindings
}

fn entry_search_sql() -> String {
    format!(
        "SELECT e.id, t.uid, t.title, e.seq, e.turn,
            snippet(entry_fts, 0, :mark_start, :mark_end, '...', 12), e.at, bm25(entry_fts)
         FROM entry_fts
         JOIN entry e ON e.id = entry_fts.rowid
         JOIN thread t ON t.id = e.thread_id
         WHERE entry_fts MATCH :fts{FILTER_SQL}
         ORDER BY bm25(entry_fts), e.id
         LIMIT :limit"
    )
}

fn edit_search_sql() -> String {
    format!(
        "SELECT en.id, t.uid, t.title, en.seq, en.turn,
            snippet(edit_fts, 0, :mark_start, :mark_end, '...', 12), en.at, bm25(edit_fts)
         FROM edit_fts
         JOIN edit ed ON ed.id = edit_fts.rowid
         JOIN entry en ON en.id = ed.entry_id
         JOIN thread t ON t.id = ed.thread_id
         WHERE edit_fts MATCH :fts{FILTER_SQL}
         ORDER BY bm25(edit_fts), en.id
         LIMIT :limit"
    )
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
    };
    Ok((row.get(0)?, summary))
}

fn hit_row(row: &Row<'_>) -> rusqlite::Result<(f64, ThreadHit)> {
    let hit = ThreadHit {
        entry_id: row.get(0)?,
        thread_uid: row.get(1)?,
        title: row.get(2)?,
        seq: row.get(3)?,
        turn: row.get(4)?,
        snippet: row.get(5)?,
        at: row.get(6)?,
    };
    Ok((row.get(7)?, hit))
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
    })
}

fn link_row(row: &Row<'_>) -> rusqlite::Result<ThreadLink> {
    let kind: String = row.get(0)?;
    let origin: String = row.get(2)?;
    Ok(ThreadLink {
        kind: parse_link_kind(&kind).ok_or_else(|| bad_value(0, &kind))?,
        reference: row.get(1)?,
        origin: parse_origin(&origin).ok_or_else(|| bad_value(2, &origin))?,
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
        LinkOrigin::Manual => "manual",
    }
}

fn parse_origin(text: &str) -> Option<LinkOrigin> {
    match text {
        "observed" => Some(LinkOrigin::Observed),
        "manual" => Some(LinkOrigin::Manual),
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
        conn.execute(
            "INSERT INTO edit (thread_id, entry_id, path, old_path, change, patch, added, removed,
                pre_oid, post_oid, from_input, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                entry.at
            ],
        )?;
        state.edit_count += 1;
    }
    Ok(())
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
/// the index is optimized after every removal of text.
fn purge_search_index(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "INSERT INTO entry_fts (entry_fts) VALUES ('optimize');
         INSERT INTO edit_fts (edit_fts) VALUES ('optimize');",
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

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::EditChange;
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
            redactions: Vec::new(),
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
            at,
            edit: None,
        }
    }

    fn with_edit(mut entry: NewEntry, path: &str) -> NewEntry {
        entry.edit = Some(NewEdit {
            path: path.to_owned(),
            old_path: None,
            change: EditChange::Modify,
            patch: "@@ -1 +1 @@\n-a\n+b\n".to_owned(),
            added: 1,
            removed: 1,
            pre_oid: None,
            post_oid: None,
            from_input: false,
        });
        entry
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
        assert_eq!(version, 1);
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
        let cursor = (
            last.last_event_at,
            row_id(&store, "SELECT id FROM thread WHERE uid = ?1", &last.uid),
        );
        let second = store
            .list_threads(&filter, Some(cursor), 2)
            .expect("page 2");
        assert_eq!(titles(&second), vec!["t2", "t1"]);
        let last = &second[1];
        let cursor = (
            last.last_event_at,
            row_id(&store, "SELECT id FROM thread WHERE uid = ?1", &last.uid),
        );
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
            .update_meta(
                &uid("a"),
                &MetaUpdate {
                    tags: Some(vec!["bug".to_owned()]),
                    ..MetaUpdate::default()
                },
            )
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
        store.delete_thread(&uid("s")).expect("delete");
        assert_eq!(store.total_threads().expect("count"), 0);
        assert!(
            store
                .search(marker, &ThreadFilter::default(), 10)
                .expect("search")
                .is_empty()
        );
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
            kind: LinkKind::Commit,
            reference: "abc123".to_owned(),
            origin,
        };
        store
            .link(&uid("s"), &link(LinkOrigin::Observed))
            .expect("link");
        store
            .link(&uid("s"), &link(LinkOrigin::Manual))
            .expect("relink");
        let links = store.read_thread(&uid("s"), 0, 1).expect("read").links;
        assert_eq!(links, vec![link(LinkOrigin::Manual)]);
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
                    kind: LinkKind::File,
                    reference: " ".to_owned(),
                    origin: LinkOrigin::Manual
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
                vec![new_entry("u", EntryKind::User, "plain", 1), tool],
            ))
            .expect("apply");
        let scrub = |text: &str| {
            text.contains("hunter2")
                .then(|| text.replace("hunter2", "[redacted]"))
        };
        let changed = store.rewrite_texts(&scrub).expect("rewrite");
        assert_eq!(changed, 1);
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
    fn redaction_counts_are_added_once_per_new_batch() {
        let mut store = ThreadStore::open_memory().expect("open");
        let mut b = batch("s", 1, vec![new_entry("u", EntryKind::User, "x", 1)]);
        b.redactions = vec![("token".to_owned(), 2)];
        store.apply(&b).expect("first");
        store.apply(&b).expect("replay");
        let summary = store.read_thread(&uid("s"), 0, 10).expect("read").summary;
        assert_eq!(summary.redaction_count, 2);
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
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        let writers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|session| {
                let path = path.clone();
                thread::spawn(move || {
                    let mut store = ThreadStore::open(&path).expect("open");
                    for n in 0..200_i64 {
                        let entry = new_entry(&format!("e{n}"), EntryKind::Assistant, "text", n);
                        store.apply(&batch(session, n, vec![entry])).expect("apply");
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
}
