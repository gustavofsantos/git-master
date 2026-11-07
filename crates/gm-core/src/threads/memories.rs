//! Memories in `threads.db` (design/threads.md, "Memories"; D190, D192): short records the project
//! wants future sessions to know, each with the sources it came from and its review. The rules that
//! need no store (the text, the duplicate key, the source forms, who may act and the state machine)
//! are pure functions. The store methods run them and write each change in one transaction, so a
//! refused request writes nothing. The engine decides who is acting. Memory text is untrusted data.

use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gm_schema::threads::{
    FileChange, FileChangeState, Memory, MemoryFilter, MemoryKind, MemorySource, MemorySourceInput,
    MemorySourceKind, MemoryStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use super::query::build_fts_query;
use super::store::{
    CURSOR_MALFORMED, CURSOR_MISMATCH, StoreError, ThreadStore, begin, invalid, thread_id,
};

/// Characters of a memory's text (D192 (1)). Text over this is refused, not cut.
pub const TEXT_CHARS: usize = 1000;
/// Hex digits of a uid and of a cursor digest (D192 (8)).
const UID_HEX: usize = 16;
/// The origin of a human's action. Every other origin is an agent's or an import's.
const HUMAN: &str = "human";
/// Keeps two uids made in one clock tick apart.
static UID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The columns of a memory row, in the order `memory_row` reads them.
const MEMORY_COLUMNS: &str = "m.id, m.uid, m.kind, m.text, m.status, m.origin, m.proposed_by,
    m.model, m.created_at, m.decided_at, m.decided_by";

/// The list filters as SQL. Parameters 2 to 5 are always bound (NULL means no restriction).
const MEMORY_FILTER_SQL: &str = " AND (?2 IS NULL OR m.kind = ?2)
    AND (?3 IS NULL OR m.status = ?3)
    AND (?4 IS NULL OR EXISTS (SELECT 1 FROM memory_source s WHERE s.memory_id = m.id
        AND s.kind = 'file' AND (s.ref = ?4 OR substr(s.ref, 1, length(?4) + 1) = (?4 || '/'))))
    AND (?5 IS NULL OR m.origin = ?5 OR m.proposed_by = ?5)";

/// What a caller asks to store as a memory (`propose_memory`). `text` is already redacted (D121).
pub struct NewMemory<'a> {
    pub kind: MemoryKind,
    pub text: &'a str,
    /// `Proposed` for an agent's call; `Accepted` for a human's (D192 (3)).
    pub status: MemoryStatus,
    pub origin: &'a str,
    pub sources: &'a [MemorySource],
    /// Milliseconds since the Unix epoch.
    pub at: i64,
}

/// What a memory action changes, for `authorize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryAction {
    /// Creating a memory with `sources` sources.
    Propose {
        sources: usize,
    },
    Accept,
    Reject,
    Delete,
}

/// A state change of a memory (D192 (4)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Accept,
    Reject,
    Supersede,
}

/// The text of a memory as stored: trimmed, not empty, at most `TEXT_CHARS` characters.
pub fn check_text(text: &str) -> Result<String, StoreError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid("a memory needs text"));
    }
    if text.chars().count() > TEXT_CHARS {
        return Err(invalid(&format!(
            "a memory is at most {TEXT_CHARS} characters"
        )));
    }
    Ok(text.to_owned())
}

/// The duplicate key of a memory: its kind, then its text lowercased with each run of whitespace
/// made one space (D192 (2)).
#[must_use]
pub fn dedup_key(kind: MemoryKind, text: &str) -> String {
    let folded = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    format!("{}\0{folded}", kind_text(kind))
}

/// A source as stored: a thread names a stored thread's uid and may carry an entry range (`from`
/// not after `to`, both not negative); a commit is 40 or 64 lowercase hex digits (the engine
/// resolves it to a full id first); a file is a repository-relative path without `..`, a backslash
/// or a leading `/`, with leading `./` removed (D192 (5)).
pub fn check_source(source: &MemorySourceInput) -> Result<MemorySourceInput, StoreError> {
    let kind = source.kind;
    if kind != MemorySourceKind::Thread
        && (source.entry_from.is_some() || source.entry_to.is_some())
    {
        return Err(invalid("only a thread source can name entries"));
    }
    match kind {
        MemorySourceKind::Thread => check_thread(source),
        MemorySourceKind::Commit => {
            let reference = &source.reference;
            let hex = (reference.len() == 40 || reference.len() == 64)
                && reference
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            if !hex {
                return Err(invalid(
                    "a commit source is a full id: 40 or 64 lowercase hex digits",
                ));
            }
            Ok(plain(kind, reference))
        }
        MemorySourceKind::File => {
            let reference = &source.reference;
            if reference.contains('\\') {
                return Err(invalid("a file source is a path with / separators"));
            }
            let mut path = reference.as_str();
            while let Some(rest) = path.strip_prefix("./") {
                path = rest;
            }
            if path.is_empty() {
                return Err(invalid("a file source needs a path"));
            }
            if path.starts_with('/') {
                return Err(invalid(
                    "a file source is a path relative to the repository",
                ));
            }
            if path.split('/').any(|part| part == "..") {
                return Err(invalid("a file source cannot go up with .."));
            }
            Ok(plain(kind, path))
        }
    }
}

/// Who may act on memories (D192 (3)): a human may do everything; an agent may only propose, and
/// only with at least one source. The message says why a refusal happens.
pub fn authorize(is_human: bool, action: &MemoryAction) -> Result<(), String> {
    if is_human {
        return Ok(());
    }
    match action {
        MemoryAction::Propose { sources: 0 } => {
            Err("An agent must name at least one source for a memory.".to_owned())
        }
        MemoryAction::Propose { .. } => Ok(()),
        MemoryAction::Accept | MemoryAction::Reject | MemoryAction::Delete => {
            Err("An agent can only propose a memory.".to_owned())
        }
    }
}

/// The status a memory moves to, or the reason it cannot move (D192 (4)).
pub fn next_status(from: MemoryStatus, transition: Transition) -> Result<MemoryStatus, String> {
    use MemoryStatus::{Accepted, Proposed, Rejected, Superseded};
    let next = match (from, transition) {
        (Proposed | Accepted, Transition::Accept) => Some(Accepted),
        (Proposed, Transition::Reject) => Some(Rejected),
        (Accepted, Transition::Supersede) => Some(Superseded),
        _ => None,
    };
    next.ok_or_else(|| {
        format!(
            "the memory is {}, so it cannot be {}",
            status_text(from),
            transition_verb(transition)
        )
    })
}

/// Whether a file a memory names has changed since it was accepted (D192 (6)). `current` gives the
/// blob of a path at `HEAD`: `Ok(None)` when there is no such file, an error when Git cannot answer.
/// Only `accepted` and `superseded` memories are checked; a proposal has nothing to compare with.
/// The recorded blob is the one of the last acceptance; a file that was absent then and is there now
/// has changed. A renamed file is `missing` at its old path: nothing guesses its new name.
#[must_use]
pub fn files_changed_since(
    memory: &Memory,
    current: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Vec<FileChange> {
    if !matches!(
        memory.status,
        MemoryStatus::Accepted | MemoryStatus::Superseded
    ) {
        return Vec::new();
    }
    let mut files: Vec<&MemorySource> = memory
        .sources
        .iter()
        .filter(|source| source.kind == MemorySourceKind::File)
        .collect();
    files.sort_by(|a, b| a.reference.cmp(&b.reference));
    files
        .into_iter()
        .filter_map(|source| {
            let state = match (source.blob_oid.as_deref(), current(&source.reference)) {
                (_, Err(_)) => FileChangeState::Unknown,
                (Some(recorded), Ok(Some(now))) if recorded != now => FileChangeState::Changed,
                (Some(_), Ok(Some(_))) | (None, Ok(None)) => return None,
                (Some(_), Ok(None)) => FileChangeState::Missing,
                (None, Ok(Some(_))) => FileChangeState::Changed,
            };
            Some(FileChange {
                path: source.reference.clone(),
                state,
            })
        })
        .collect()
}

/// A new uid: the first 16 hex digits of SHA-256 over 16 bytes from the process's random hasher
/// state, the clock and a counter (D192 (8)).
#[must_use]
pub fn new_uid() -> String {
    let first = RandomState::new().build_hasher().finish();
    let second = RandomState::new().build_hasher().finish();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        });
    let count = UID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut seed = [0_u8; 16];
    seed[..8].copy_from_slice(&(first ^ nanos).to_le_bytes());
    seed[8..]
        .copy_from_slice(&(second ^ count.rotate_left(32) ^ nanos.rotate_left(17)).to_le_bytes());
    hex_prefix(&Sha256::digest(seed))
}

impl ThreadStore {
    /// Stores a memory, or returns the memory that holds its text: a non-rejected memory of the same
    /// kind with the same duplicate key is returned with `false`, and nothing is written (D192 (2)).
    /// Otherwise the memory is inserted and returned with `true`.
    pub fn propose_memory(&mut self, new: &NewMemory<'_>) -> Result<(Memory, bool), StoreError> {
        let text = check_text(new.text)?;
        let is_human = new.origin == HUMAN;
        authorize(
            is_human,
            &MemoryAction::Propose {
                sources: new.sources.len(),
            },
        )
        .map_err(|message| invalid(&message))?;
        if !matches!(new.status, MemoryStatus::Proposed | MemoryStatus::Accepted) {
            return Err(invalid("a memory is created proposed or accepted"));
        }
        if new.status == MemoryStatus::Accepted && !is_human {
            return Err(invalid("only a human creates an accepted memory"));
        }
        let sources = new
            .sources
            .iter()
            .map(checked_source)
            .collect::<Result<Vec<_>, _>>()?;
        let key = dedup_key(new.kind, &text);
        let tx = begin(&mut self.conn)?;
        for source in sources
            .iter()
            .filter(|source| source.kind == MemorySourceKind::Thread)
        {
            thread_id(&tx, &source.reference).map_err(|error| match error {
                StoreError::NotFound => invalid("a thread source names a stored thread"),
                other => other,
            })?;
        }
        if let Some(existing) = find_duplicate(&tx, new.kind, &key, None)? {
            let memory = memory_by_id(&tx, existing)?;
            return Ok((memory, false));
        }
        let accepted = new.status == MemoryStatus::Accepted;
        tx.execute(
            "INSERT INTO memory (uid, kind, text, status, origin, proposed_by, created_at,
                decided_at, decided_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
            params![
                new_uid(),
                kind_text(new.kind),
                text,
                status_text(new.status),
                new.origin,
                new.at,
                accepted.then_some(new.at),
                accepted.then_some(HUMAN),
            ],
        )?;
        let id = tx.last_insert_rowid();
        insert_sources(&tx, id, &sources)?;
        let memory = memory_by_id(&tx, id)?;
        tx.commit()?;
        Ok((memory, true))
    }

    /// Accepts a memory (a human's action, D192 (3) and (4)). On a proposed memory this is the
    /// acceptance; on an accepted one it is an edit, or a supersede. `text` replaces the text when it
    /// differs (the origin becomes `human`; `proposed_by` stays). `supersedes` must each name an
    /// accepted memory other than this one; they become `superseded` with a `memory_rel` row.
    /// `blobs` gives the blob of each file source at `HEAD` (`None` when the file is absent) and is
    /// recorded only when the memory first becomes accepted: accepting an accepted memory again
    /// keeps its status, `decided_at`, `decided_by` and the recorded blobs (D192 (4)), so
    /// "files changed since" still counts from the first acceptance. Every change is one
    /// transaction: a refusal writes nothing.
    pub fn accept_memory(
        &mut self,
        id: i64,
        text: Option<&str>,
        supersedes: &[i64],
        blobs: &[(String, Option<String>)],
        at: i64,
    ) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let (kind, stored_text, before) = core_of(&tx, id)?;
        next_status(before, Transition::Accept).map_err(|message| invalid(&message))?;
        if let Some(text) = text {
            let text = check_text(text)?;
            if text != stored_text {
                if let Some(other) = find_duplicate(&tx, kind, &dedup_key(kind, &text), Some(id))? {
                    return Err(invalid(&format!("memory {other} already says this")));
                }
                tx.execute(
                    "UPDATE memory SET text = ?1, origin = 'human' WHERE id = ?2",
                    params![text, id],
                )?;
            }
        }
        // Accepting an accepted memory again keeps its decision and its recorded blobs (D192 (4)).
        if before != MemoryStatus::Accepted {
            tx.execute(
                "UPDATE memory SET status = 'accepted', decided_at = ?1, decided_by = 'human'
                 WHERE id = ?2",
                params![at, id],
            )?;
            for (path, blob) in blobs {
                tx.execute(
                    "UPDATE memory_source SET blob_oid = ?1
                     WHERE memory_id = ?2 AND kind = 'file' AND ref = ?3",
                    params![blob, id, path],
                )?;
            }
        }
        for &old in supersedes {
            if old == id {
                return Err(invalid("a memory cannot supersede itself"));
            }
            let (_, _, old_status) = core_of(&tx, old).map_err(|error| match error {
                StoreError::NotFound => invalid(&format!("memory {old} does not exist")),
                other => other,
            })?;
            next_status(old_status, Transition::Supersede)
                .map_err(|message| invalid(&format!("memory {old}: {message}")))?;
            tx.execute(
                "UPDATE memory SET status = 'superseded', decided_at = ?1, decided_by = 'human'
                 WHERE id = ?2",
                params![at, old],
            )?;
            tx.execute(
                "INSERT INTO memory_rel (from_memory, to_memory, kind, origin, at)
                 VALUES (?1, ?2, 'supersedes', 'human', ?3)",
                params![id, old, at],
            )?;
        }
        let memory = memory_by_id(&tx, id)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Rejects a proposed memory (a human's action).
    pub fn reject_memory(&mut self, id: i64, at: i64) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let (_, _, before) = core_of(&tx, id)?;
        next_status(before, Transition::Reject).map_err(|message| invalid(&message))?;
        tx.execute(
            "UPDATE memory SET status = 'rejected', decided_at = ?1, decided_by = 'human'
             WHERE id = ?2",
            params![at, id],
        )?;
        let memory = memory_by_id(&tx, id)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Deletes a memory with its sources and its links (a human's action). `NotFound` when missing.
    pub fn delete_memory(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        if tx.execute("DELETE FROM memory WHERE id = ?1", params![id])? == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// A memory with its sources and its supersedes links. `NotFound` when missing.
    pub fn read_memory(&self, id: i64) -> Result<Memory, StoreError> {
        memory_by_id(&self.conn, id)
    }

    /// The repository-relative paths of a memory's file sources, by path.
    pub fn memory_file_sources(&self, id: i64) -> Result<Vec<String>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT ref FROM memory_source WHERE memory_id = ?1 AND kind = 'file' ORDER BY ref",
        )?;
        let paths = statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(paths)
    }

    /// One page of memories. With a query they are ordered by relevance (the D128 builder over
    /// `memory_fts`), otherwise newest first. `filter` narrows them. The cursor is an opaque position
    /// tied to the query and the filter; a cursor from another request is refused (D192 (7)).
    pub fn list_memories(
        &self,
        query: &str,
        filter: &MemoryFilter,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<(Vec<Memory>, Option<String>), StoreError> {
        let fts = if query.trim().is_empty() {
            None
        } else {
            let Some(expression) = build_fts_query(query) else {
                return Ok((Vec::new(), None));
            };
            Some(expression)
        };
        let digest = list_digest(query, filter);
        let offset = match cursor {
            Some(cursor) => decode_offset(cursor, &digest)?,
            None => 0,
        };
        let page = i64::from(limit.max(1));
        let sql = match fts {
            Some(_) => format!(
                "SELECT m.id FROM memory m JOIN memory_fts ON memory_fts.rowid = m.id
                 WHERE memory_fts MATCH ?1{MEMORY_FILTER_SQL}
                 ORDER BY bm25(memory_fts), m.id LIMIT ?6 OFFSET ?7"
            ),
            None => format!(
                "SELECT m.id FROM memory m WHERE 1 = 1{MEMORY_FILTER_SQL}
                 ORDER BY m.created_at DESC, m.id DESC LIMIT ?6 OFFSET ?7"
            ),
        };
        let kind = filter.kind.map(kind_text);
        let status = filter.status.map(status_text);
        let path = filter.path.as_deref().filter(|path| !path.is_empty());
        let author = filter.author.as_deref().filter(|author| !author.is_empty());
        let mut statement = self.conn.prepare(&sql)?;
        let mut ids = statement
            .query_map(
                params![fts.as_deref(), kind, status, path, author, page + 1, offset],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = if ids.len() > usize::try_from(page).unwrap_or(usize::MAX) {
            ids.truncate(usize::try_from(page).unwrap_or(usize::MAX));
            Some(format!("{digest}.{}", offset + page))
        } else {
            None
        };
        let mut memories = Vec::with_capacity(ids.len());
        for id in ids {
            memories.push(memory_by_id(&self.conn, id)?);
        }
        Ok((memories, next))
    }
}

/// A source as stored, with its text checked and normalised.
fn checked_source(source: &MemorySource) -> Result<MemorySource, StoreError> {
    let checked = check_source(&MemorySourceInput {
        kind: source.kind,
        reference: source.reference.clone(),
        entry_from: source.entry_from,
        entry_to: source.entry_to,
    })?;
    Ok(MemorySource {
        kind: checked.kind,
        reference: checked.reference,
        repo_id: source.repo_id.clone(),
        entry_from: checked.entry_from,
        entry_to: checked.entry_to,
        blob_oid: source.blob_oid.clone(),
    })
}

/// A source with no entry range, for the kinds that take none.
fn plain(kind: MemorySourceKind, reference: &str) -> MemorySourceInput {
    MemorySourceInput {
        kind,
        reference: reference.to_owned(),
        entry_from: None,
        entry_to: None,
    }
}

/// The rules of a thread source: a non-empty uid and an entry range that is not negative and does
/// not end before it starts.
fn check_thread(source: &MemorySourceInput) -> Result<MemorySourceInput, StoreError> {
    if source.reference.trim().is_empty() {
        return Err(invalid("a thread source names a thread"));
    }
    if [source.entry_from, source.entry_to]
        .into_iter()
        .flatten()
        .any(|entry| entry < 0)
    {
        return Err(invalid("entry numbers are not negative"));
    }
    if matches!((source.entry_from, source.entry_to), (Some(from), Some(to)) if from > to) {
        return Err(invalid("a thread range does not end before it starts"));
    }
    Ok(MemorySourceInput {
        kind: MemorySourceKind::Thread,
        reference: source.reference.clone(),
        entry_from: source.entry_from,
        entry_to: source.entry_to,
    })
}

/// The id of a non-rejected memory of `kind` with duplicate key `key`, other than `except`.
fn find_duplicate(
    conn: &Connection,
    kind: MemoryKind,
    key: &str,
    except: Option<i64>,
) -> Result<Option<i64>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, text FROM memory WHERE kind = ?1 AND status != 'rejected' ORDER BY id",
    )?;
    let rows = statement.query_map(params![kind_text(kind)], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, text) = row?;
        if Some(id) != except && dedup_key(kind, &text) == key {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// The kind, text and status of a memory, or `NotFound`.
fn core_of(conn: &Connection, id: i64) -> Result<(MemoryKind, String, MemoryStatus), StoreError> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT kind, text, status FROM memory WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (kind, text, status) = row.ok_or(StoreError::NotFound)?;
    let kind = parse_kind(&kind).ok_or_else(|| invalid("a memory holds an unknown kind"))?;
    let status =
        parse_status(&status).ok_or_else(|| invalid("a memory holds an unknown status"))?;
    Ok((kind, text, status))
}

/// Inserts the sources of memory `id`. A source named twice is refused.
fn insert_sources(conn: &Connection, id: i64, sources: &[MemorySource]) -> Result<(), StoreError> {
    let mut seen = HashSet::new();
    for source in sources {
        if !seen.insert((source.kind, source.reference.as_str())) {
            return Err(invalid("a memory names the same source twice"));
        }
        conn.execute(
            "INSERT INTO memory_source (memory_id, kind, ref, repo_id, entry_from, entry_to, blob_oid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                source_kind_text(source.kind),
                source.reference,
                source.repo_id,
                source.entry_from,
                source.entry_to,
                source.blob_oid,
            ],
        )?;
    }
    Ok(())
}

/// A memory with its sources and relations, or `NotFound`.
fn memory_by_id(conn: &Connection, id: i64) -> Result<Memory, StoreError> {
    let sql = format!("SELECT {MEMORY_COLUMNS} FROM memory m WHERE m.id = ?1");
    let mut memory = conn
        .query_row(&sql, params![id], memory_row)
        .optional()?
        .ok_or(StoreError::NotFound)??;
    memory.sources = sources_of(conn, id)?;
    memory.supersedes = related_ids(
        conn,
        "SELECT to_memory FROM memory_rel WHERE from_memory = ?1 AND kind = 'supersedes'
         ORDER BY to_memory",
        id,
    )?;
    memory.superseded_by = related_ids(
        conn,
        "SELECT from_memory FROM memory_rel WHERE to_memory = ?1 AND kind = 'supersedes'
         ORDER BY from_memory",
        id,
    )?;
    Ok(memory)
}

/// A memory row without its sources and relations. A stored value that is not a known kind or
/// status is an error of the row, not of the query.
fn memory_row(row: &Row<'_>) -> rusqlite::Result<Result<Memory, StoreError>> {
    let kind: String = row.get(2)?;
    let status: String = row.get(4)?;
    let (Some(kind), Some(status)) = (parse_kind(&kind), parse_status(&status)) else {
        return Ok(Err(invalid("a memory holds an unknown kind or status")));
    };
    Ok(Ok(Memory {
        id: row.get(0)?,
        uid: row.get(1)?,
        kind,
        text: row.get(3)?,
        status,
        origin: row.get(5)?,
        proposed_by: row.get(6)?,
        model: row.get(7)?,
        created_at: row.get(8)?,
        decided_at: row.get(9)?,
        decided_by: row.get(10)?,
        reviewed: status != MemoryStatus::Proposed,
        sources: Vec::new(),
        supersedes: Vec::new(),
        superseded_by: Vec::new(),
        files_changed_since: Vec::new(),
    }))
}

fn sources_of(conn: &Connection, id: i64) -> Result<Vec<MemorySource>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT kind, ref, repo_id, entry_from, entry_to, blob_oid FROM memory_source
         WHERE memory_id = ?1 ORDER BY kind, ref",
    )?;
    let rows = statement.query_map(params![id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut sources = Vec::new();
    for row in rows {
        let (kind, reference, repo_id, entry_from, entry_to, blob_oid) = row?;
        let kind = parse_source_kind(&kind)
            .ok_or_else(|| invalid("a memory source holds an unknown kind"))?;
        sources.push(MemorySource {
            kind,
            reference,
            repo_id,
            entry_from,
            entry_to,
            blob_oid,
        });
    }
    Ok(sources)
}

fn related_ids(conn: &Connection, sql: &str, id: i64) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let ids = statement
        .query_map(params![id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// The digest a cursor carries: the first 16 hex digits of SHA-256 over the query and the filter.
fn list_digest(query: &str, filter: &MemoryFilter) -> String {
    let material = format!(
        "{query}\0{:?}\0{:?}\0{:?}\0{:?}",
        filter.kind, filter.status, filter.path, filter.author
    );
    hex_prefix(&Sha256::digest(material.as_bytes()))
}

/// The offset a cursor names, when the cursor was made for this query and filter.
fn decode_offset(cursor: &str, digest: &str) -> Result<i64, StoreError> {
    let (found, offset) = cursor
        .split_once('.')
        .ok_or_else(|| invalid(CURSOR_MALFORMED))?;
    if found != digest {
        return Err(invalid(CURSOR_MISMATCH));
    }
    let offset: i64 = offset.parse().map_err(|_| invalid(CURSOR_MALFORMED))?;
    if offset < 0 {
        return Err(invalid(CURSOR_MALFORMED));
    }
    Ok(offset)
}

/// The first 16 lowercase hex digits of `digest`.
fn hex_prefix(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(UID_HEX);
    for byte in digest.iter().take(UID_HEX / 2) {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

fn transition_verb(transition: Transition) -> &'static str {
    match transition {
        Transition::Accept => "accepted",
        Transition::Reject => "rejected",
        Transition::Supersede => "superseded",
    }
}

fn kind_text(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Decision => "decision",
        MemoryKind::Rule => "rule",
        MemoryKind::Lesson => "lesson",
        MemoryKind::Fact => "fact",
    }
}

fn parse_kind(text: &str) -> Option<MemoryKind> {
    match text {
        "decision" => Some(MemoryKind::Decision),
        "rule" => Some(MemoryKind::Rule),
        "lesson" => Some(MemoryKind::Lesson),
        "fact" => Some(MemoryKind::Fact),
        _ => None,
    }
}

fn status_text(status: MemoryStatus) -> &'static str {
    match status {
        MemoryStatus::Proposed => "proposed",
        MemoryStatus::Accepted => "accepted",
        MemoryStatus::Rejected => "rejected",
        MemoryStatus::Superseded => "superseded",
    }
}

fn parse_status(text: &str) -> Option<MemoryStatus> {
    match text {
        "proposed" => Some(MemoryStatus::Proposed),
        "accepted" => Some(MemoryStatus::Accepted),
        "rejected" => Some(MemoryStatus::Rejected),
        "superseded" => Some(MemoryStatus::Superseded),
        _ => None,
    }
}

fn source_kind_text(kind: MemorySourceKind) -> &'static str {
    match kind {
        MemorySourceKind::Thread => "thread",
        MemorySourceKind::Commit => "commit",
        MemorySourceKind::File => "file",
    }
}

fn parse_source_kind(text: &str) -> Option<MemorySourceKind> {
    match text {
        "thread" => Some(MemorySourceKind::Thread),
        "commit" => Some(MemorySourceKind::Commit),
        "file" => Some(MemorySourceKind::File),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::store::Batch;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn thread(store: &mut ThreadStore, session: &str) -> String {
        let batch = Batch {
            harness: "claude-code".into(),
            session_id: session.into(),
            worktree: "/repo".into(),
            branch: Some("main".into()),
            head: None,
            model: None,
            harness_version: None,
            transcript: None,
            cursor: None,
            started_at: 1,
            at: 2,
            ended: None,
            entries: Vec::new(),
            gaps: Vec::new(),
        };
        store.apply(&batch).expect("apply").uid
    }

    fn file(path: &str) -> MemorySource {
        MemorySource {
            kind: MemorySourceKind::File,
            reference: path.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: None,
        }
    }

    fn commit(id: &str) -> MemorySource {
        MemorySource {
            kind: MemorySourceKind::Commit,
            reference: id.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: None,
        }
    }

    fn agent_proposal<'a>(
        kind: MemoryKind,
        text: &'a str,
        sources: &'a [MemorySource],
        at: i64,
    ) -> NewMemory<'a> {
        NewMemory {
            kind,
            text,
            status: MemoryStatus::Proposed,
            origin: "agent:x",
            sources,
            at,
        }
    }

    fn propose(
        store: &mut ThreadStore,
        kind: MemoryKind,
        text: &str,
        sources: &[MemorySource],
    ) -> Memory {
        store
            .propose_memory(&agent_proposal(kind, text, sources, 10))
            .expect("propose")
            .0
    }

    /// A memory the human writes: accepted at once.
    fn human_memory(
        store: &mut ThreadStore,
        kind: MemoryKind,
        text: &str,
        sources: &[MemorySource],
        at: i64,
    ) -> Memory {
        let new = NewMemory {
            kind,
            text,
            status: MemoryStatus::Accepted,
            origin: HUMAN,
            sources,
            at,
        };
        store.propose_memory(&new).expect("human memory").0
    }

    /// A proposed memory that the human accepts.
    fn accepted(store: &mut ThreadStore, kind: MemoryKind, text: &str) -> Memory {
        let memory = propose(store, kind, text, &[file("src/a.rs")]);
        store
            .accept_memory(memory.id, None, &[], &[], 20)
            .expect("accept")
    }

    fn input(kind: MemorySourceKind, reference: &str) -> MemorySourceInput {
        MemorySourceInput {
            kind,
            reference: reference.to_owned(),
            entry_from: None,
            entry_to: None,
        }
    }

    fn count(store: &ThreadStore, sql: &str) -> i64 {
        store
            .conn
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn check_text_trims_refuses_empty_and_counts_characters() {
        assert_eq!(check_text("  a rule  ").expect("trimmed"), "a rule");
        assert!(check_text("").is_err());
        assert!(check_text(" \t\n ").is_err());
        let exact = "é".repeat(TEXT_CHARS);
        assert_eq!(
            check_text(&exact).expect("1000 characters").chars().count(),
            TEXT_CHARS
        );
        let over = "é".repeat(TEXT_CHARS + 1);
        assert!(matches!(check_text(&over), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn dedup_key_folds_case_and_whitespace_and_keeps_kinds_apart() {
        assert_eq!(
            dedup_key(MemoryKind::Rule, "  Run   the\tChecks "),
            dedup_key(MemoryKind::Rule, "run the checks")
        );
        assert_ne!(
            dedup_key(MemoryKind::Rule, "run the checks"),
            dedup_key(MemoryKind::Fact, "run the checks")
        );
    }

    #[test]
    fn check_source_refuses_each_bad_form_and_normalises_a_dot_prefix() {
        let bad_files = ["/etc/passwd", "src/../x", "..", "", "./", "src\\a.rs"];
        for path in bad_files {
            assert!(
                check_source(&input(MemorySourceKind::File, path)).is_err(),
                "{path:?} is refused"
            );
        }
        let normal = check_source(&input(MemorySourceKind::File, "./src/a.rs")).expect("file");
        assert_eq!(normal.reference, "src/a.rs");

        let upper = OID.to_uppercase();
        for id in [upper.as_str(), &OID[..39], "abc"] {
            assert!(
                check_source(&input(MemorySourceKind::Commit, id)).is_err(),
                "{id} refused"
            );
        }
        assert!(check_source(&input(MemorySourceKind::Commit, OID)).is_ok());
        assert!(check_source(&input(MemorySourceKind::Commit, &"a".repeat(64))).is_ok());

        let mut ranged = input(MemorySourceKind::File, "src/a.rs");
        ranged.entry_from = Some(1);
        assert!(
            check_source(&ranged).is_err(),
            "a file takes no entry range"
        );

        let thread_range = |from, to| MemorySourceInput {
            kind: MemorySourceKind::Thread,
            reference: "uid".to_owned(),
            entry_from: from,
            entry_to: to,
        };
        assert!(check_source(&thread_range(Some(3), Some(1))).is_err());
        assert!(check_source(&thread_range(Some(-1), None)).is_err());
        assert!(check_source(&thread_range(Some(1), Some(3))).is_ok());
        assert!(check_source(&input(MemorySourceKind::Thread, " ")).is_err());
    }

    #[test]
    fn authorize_lets_a_human_do_everything_and_an_agent_only_propose_with_a_source() {
        let actions = [
            MemoryAction::Propose { sources: 0 },
            MemoryAction::Accept,
            MemoryAction::Reject,
            MemoryAction::Delete,
        ];
        for action in &actions {
            assert!(authorize(true, action).is_ok(), "a human may {action:?}");
        }
        assert!(authorize(false, &MemoryAction::Propose { sources: 0 }).is_err());
        assert!(authorize(false, &MemoryAction::Propose { sources: 1 }).is_ok());
        for action in [
            MemoryAction::Accept,
            MemoryAction::Reject,
            MemoryAction::Delete,
        ] {
            let message = authorize(false, &action).expect_err("refused");
            assert_eq!(message, "An agent can only propose a memory.");
        }
    }

    #[test]
    fn next_status_covers_every_pair() {
        use MemoryStatus::{Accepted, Proposed, Rejected, Superseded};
        use Transition::{Accept, Reject, Supersede};
        let cases = [
            (Proposed, Accept, Some(Accepted)),
            (Proposed, Reject, Some(Rejected)),
            (Proposed, Supersede, None),
            (Accepted, Accept, Some(Accepted)),
            (Accepted, Reject, None),
            (Accepted, Supersede, Some(Superseded)),
            (Rejected, Accept, None),
            (Rejected, Reject, None),
            (Rejected, Supersede, None),
            (Superseded, Accept, None),
            (Superseded, Reject, None),
            (Superseded, Supersede, None),
        ];
        for (from, transition, want) in cases {
            if let Some(status) = want {
                assert_eq!(
                    next_status(from, transition),
                    Ok(status),
                    "{from:?} {transition:?}"
                );
            } else {
                let message = next_status(from, transition).expect_err("refused");
                assert!(
                    message.contains(status_text(from)),
                    "{message} names the status"
                );
            }
        }
    }

    #[test]
    fn files_changed_since_covers_each_case_and_skips_proposals() {
        let mut memory = Memory {
            id: 1,
            uid: "u".into(),
            kind: MemoryKind::Rule,
            text: "t".into(),
            status: MemoryStatus::Accepted,
            origin: "human".into(),
            proposed_by: None,
            model: None,
            created_at: 1,
            decided_at: Some(2),
            decided_by: Some("human".into()),
            reviewed: true,
            sources: Vec::new(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
        };
        let source = |path: &str, blob: Option<&str>| MemorySource {
            kind: MemorySourceKind::File,
            reference: path.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: blob.map(str::to_owned),
        };
        // a: same blob; b: edited; c: deleted; d: appeared; e: absent both times; f: git fails.
        memory.sources = vec![
            source("c.rs", Some("c1")),
            source("a.rs", Some("a1")),
            source("b.rs", Some("b1")),
            source("d.rs", None),
            source("e.rs", None),
            source("f.rs", Some("f1")),
            MemorySource {
                kind: MemorySourceKind::Commit,
                ..source("0123", Some("x"))
            },
        ];
        let now = |path: &str| match path {
            "a.rs" => Ok(Some("a1".to_owned())),
            "b.rs" => Ok(Some("b2".to_owned())),
            "c.rs" | "e.rs" => Ok(None),
            "d.rs" => Ok(Some("d1".to_owned())),
            "f.rs" => Err("git failed".to_owned()),
            other => panic!("unexpected path {other}"),
        };
        let changes = files_changed_since(&memory, &now);
        let want = vec![
            FileChange {
                path: "b.rs".into(),
                state: FileChangeState::Changed,
            },
            FileChange {
                path: "c.rs".into(),
                state: FileChangeState::Missing,
            },
            FileChange {
                path: "d.rs".into(),
                state: FileChangeState::Changed,
            },
            FileChange {
                path: "f.rs".into(),
                state: FileChangeState::Unknown,
            },
        ];
        assert_eq!(changes, want);

        memory.status = MemoryStatus::Superseded;
        assert_eq!(files_changed_since(&memory, &now), want, "superseded too");

        memory.status = MemoryStatus::Proposed;
        assert!(files_changed_since(&memory, &now).is_empty());
    }

    #[test]
    fn new_uid_is_sixteen_lowercase_hex_and_two_calls_differ() {
        let first = new_uid();
        let second = new_uid();
        assert_eq!(first.len(), 16);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_ne!(first, second);
    }

    #[test]
    fn a_same_text_in_other_case_and_spacing_returns_the_first_memory() {
        let mut store = ThreadStore::open_memory().expect("open");
        let sources = [file("src/a.rs")];
        let (first, created) = store
            .propose_memory(&agent_proposal(
                MemoryKind::Rule,
                "Run the  release checks",
                &sources,
                10,
            ))
            .expect("first");
        assert!(created);
        let (again, created) = store
            .propose_memory(&agent_proposal(
                MemoryKind::Rule,
                "run the release checks",
                &sources,
                11,
            ))
            .expect("again");
        assert!(!created);
        assert_eq!(again.id, first.id);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM memory"), 1);
    }

    #[test]
    fn a_rejected_duplicate_does_not_block_a_new_memory() {
        let mut store = ThreadStore::open_memory().expect("open");
        let first = propose(
            &mut store,
            MemoryKind::Fact,
            "The cache is per worktree",
            &[file("a")],
        );
        store.reject_memory(first.id, 11).expect("reject");
        let (second, created) = store
            .propose_memory(&agent_proposal(
                MemoryKind::Fact,
                "the cache is per worktree",
                &[file("a")],
                12,
            ))
            .expect("second");
        assert!(created);
        assert_ne!(second.id, first.id);
    }

    #[test]
    fn a_thread_source_must_name_a_stored_thread() {
        let mut store = ThreadStore::open_memory().expect("open");
        let sources = [MemorySource {
            kind: MemorySourceKind::Thread,
            reference: "missing".to_owned(),
            repo_id: None,
            entry_from: Some(0),
            entry_to: Some(2),
            blob_oid: None,
        }];
        let refused =
            store.propose_memory(&agent_proposal(MemoryKind::Lesson, "a lesson", &sources, 1));
        assert!(matches!(refused, Err(StoreError::Invalid(_))));
        assert_eq!(count(&store, "SELECT COUNT(*) FROM memory"), 0);

        let uid = thread(&mut store, "s1");
        let sources = [MemorySource {
            reference: uid,
            ..sources[0].clone()
        }];
        let (memory, created) = store
            .propose_memory(&agent_proposal(MemoryKind::Lesson, "a lesson", &sources, 2))
            .expect("existing thread");
        assert!(created);
        assert_eq!(memory.sources.len(), 1);
    }

    #[test]
    fn a_human_creates_an_accepted_memory_and_an_agent_cannot() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = human_memory(
            &mut store,
            MemoryKind::Decision,
            "We keep one store",
            &[],
            5,
        );
        assert_eq!(memory.status, MemoryStatus::Accepted);
        assert_eq!(memory.decided_by.as_deref(), Some("human"));
        assert!(memory.reviewed);

        let accepted_by_agent = NewMemory {
            kind: MemoryKind::Rule,
            text: "Agents accept",
            status: MemoryStatus::Accepted,
            origin: "agent:x",
            sources: &[file("a")],
            at: 6,
        };
        assert!(matches!(
            store.propose_memory(&accepted_by_agent),
            Err(StoreError::Invalid(_))
        ));
        let no_source = agent_proposal(MemoryKind::Rule, "A rule with no source", &[], 7);
        assert!(matches!(
            store.propose_memory(&no_source),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn accepting_a_proposal_makes_it_accepted_by_a_human() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(
            &mut store,
            MemoryKind::Rule,
            "Run the checks",
            &[file("src/a.rs")],
        );
        assert!(!memory.reviewed);
        let accepted = store
            .accept_memory(memory.id, None, &[], &[], 20)
            .expect("accept");
        assert_eq!(accepted.status, MemoryStatus::Accepted);
        assert_eq!(accepted.decided_by.as_deref(), Some("human"));
        assert_eq!(accepted.decided_at, Some(20));
        assert!(accepted.reviewed);
    }

    #[test]
    fn an_edit_sets_the_origin_to_human_and_keeps_who_proposed_it() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(
            &mut store,
            MemoryKind::Rule,
            "old text",
            &[file("src/a.rs")],
        );
        let edited = store
            .accept_memory(memory.id, Some("  new text  "), &[], &[], 20)
            .expect("edit and accept");
        assert_eq!(edited.text, "new text");
        assert_eq!(edited.origin, "human");
        assert_eq!(edited.proposed_by.as_deref(), Some("agent:x"));
    }

    #[test]
    fn an_edit_to_the_key_of_another_memory_is_refused_naming_it() {
        let mut store = ThreadStore::open_memory().expect("open");
        let held = accepted(&mut store, MemoryKind::Rule, "Run the release checks");
        let other = propose(
            &mut store,
            MemoryKind::Rule,
            "Something else",
            &[file("src/b.rs")],
        );
        let refused = store.accept_memory(other.id, Some("RUN   the release checks"), &[], &[], 21);
        match refused {
            Err(StoreError::Invalid(message)) => {
                assert!(message.contains(&held.id.to_string()), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(
            store.read_memory(other.id).expect("read").text,
            "Something else"
        );
    }

    #[test]
    fn supersede_marks_the_old_memory_and_links_both_ways() {
        let mut store = ThreadStore::open_memory().expect("open");
        let old = accepted(&mut store, MemoryKind::Decision, "Use the old scheme");
        let new = propose(
            &mut store,
            MemoryKind::Decision,
            "Use the new scheme",
            &[file("src/a.rs")],
        );
        store
            .accept_memory(new.id, None, &[old.id], &[], 30)
            .expect("supersede");
        let old = store.read_memory(old.id).expect("old");
        assert_eq!(old.status, MemoryStatus::Superseded);
        assert_eq!(old.superseded_by, vec![new.id]);
        let new = store.read_memory(new.id).expect("new");
        assert_eq!(new.supersedes, vec![old.id]);
        assert_eq!(new.status, MemoryStatus::Accepted);
    }

    #[test]
    fn superseding_a_proposed_memory_is_refused_and_writes_nothing() {
        let mut store = ThreadStore::open_memory().expect("open");
        let accepted_old = accepted(&mut store, MemoryKind::Fact, "An accepted fact");
        let proposed_old = propose(
            &mut store,
            MemoryKind::Fact,
            "A proposed fact",
            &[file("src/a.rs")],
        );
        let new = propose(
            &mut store,
            MemoryKind::Fact,
            "The newest fact",
            &[file("src/a.rs")],
        );

        let refused =
            store.accept_memory(new.id, None, &[accepted_old.id, proposed_old.id], &[], 40);
        assert!(matches!(refused, Err(StoreError::Invalid(_))));
        assert_eq!(
            store.read_memory(new.id).expect("new").status,
            MemoryStatus::Proposed
        );
        assert_eq!(
            store.read_memory(accepted_old.id).expect("old").status,
            MemoryStatus::Accepted,
            "the earlier supersede in the same request is rolled back"
        );
        assert_eq!(
            store.read_memory(proposed_old.id).expect("proposed").status,
            MemoryStatus::Proposed
        );
    }

    #[test]
    fn a_memory_cannot_supersede_itself() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(&mut store, MemoryKind::Rule, "A rule", &[file("src/a.rs")]);
        assert!(matches!(
            store.accept_memory(memory.id, None, &[memory.id], &[], 1),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(
            store.read_memory(memory.id).expect("read").status,
            MemoryStatus::Proposed
        );
    }

    #[test]
    fn accepting_records_the_blob_of_each_file_source() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(
            &mut store,
            MemoryKind::Rule,
            "Keep the blob",
            &[file("src/a.rs"), file("src/b.rs")],
        );
        let blobs = [
            ("src/a.rs".to_owned(), Some("abc123".to_owned())),
            ("src/b.rs".to_owned(), None),
        ];
        let accepted = store
            .accept_memory(memory.id, None, &[], &blobs, 20)
            .expect("accept");
        let blob_of = |path: &str| {
            accepted
                .sources
                .iter()
                .find(|source| source.reference == path)
                .and_then(|source| source.blob_oid.clone())
        };
        assert_eq!(blob_of("src/a.rs").as_deref(), Some("abc123"));
        assert_eq!(blob_of("src/b.rs"), None);
        assert!(
            accepted.files_changed_since.is_empty(),
            "derived on read, not from the store"
        );
    }

    #[test]
    fn accepting_an_accepted_memory_again_keeps_its_decision_and_blobs() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(
            &mut store,
            MemoryKind::Rule,
            "Keep the first",
            &[file("src/a.rs")],
        );
        let first = [("src/a.rs".to_owned(), Some("abc123".to_owned()))];
        let accepted = store
            .accept_memory(memory.id, None, &[], &first, 20)
            .expect("accept");

        let changed = [("src/a.rs".to_owned(), Some("def456".to_owned()))];
        let again = store
            .accept_memory(memory.id, None, &[], &changed, 99)
            .expect("accept again");
        assert_eq!(again.status, MemoryStatus::Accepted);
        assert_eq!(again.decided_at, Some(20));
        assert_eq!(again.decided_by.as_deref(), Some("human"));
        assert_eq!(again.sources[0].blob_oid.as_deref(), Some("abc123"));
        assert_eq!(again.text, accepted.text);
        assert_eq!(again.origin, accepted.origin);
    }

    #[test]
    fn accepting_an_accepted_memory_again_with_text_edits_only_the_text() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = accepted(&mut store, MemoryKind::Fact, "The first wording");
        let blobs = [("src/a.rs".to_owned(), Some("abc123".to_owned()))];
        let before = store.read_memory(memory.id).expect("read");
        assert!(before.sources[0].blob_oid.is_none());

        let edited = store
            .accept_memory(memory.id, Some("The second wording"), &[], &blobs, 99)
            .expect("edit");
        assert_eq!(edited.text, "The second wording");
        assert_eq!(edited.origin, HUMAN);
        assert_eq!(edited.decided_at, Some(20));
        assert_eq!(edited.decided_by.as_deref(), Some("human"));
        assert!(edited.sources[0].blob_oid.is_none());
    }

    #[test]
    fn accepting_an_accepted_memory_again_still_supersedes() {
        let mut store = ThreadStore::open_memory().expect("open");
        let old = accepted(&mut store, MemoryKind::Decision, "The old one");
        let new = accepted(&mut store, MemoryKind::Decision, "The new one");
        let superseded = store
            .accept_memory(new.id, None, &[old.id], &[], 40)
            .expect("supersede");
        assert_eq!(superseded.supersedes, vec![old.id]);
        assert_eq!(superseded.decided_at, Some(20));
        assert_eq!(
            store.read_memory(old.id).expect("old").status,
            MemoryStatus::Superseded
        );
    }

    #[test]
    fn reject_takes_only_a_proposed_memory() {
        let mut store = ThreadStore::open_memory().expect("open");
        let proposed = propose(&mut store, MemoryKind::Fact, "Rejectable", &[file("a")]);
        let rejected = store.reject_memory(proposed.id, 11).expect("reject");
        assert_eq!(rejected.status, MemoryStatus::Rejected);
        assert_eq!(rejected.decided_by.as_deref(), Some("human"));
        let kept = accepted(&mut store, MemoryKind::Fact, "Not rejectable");
        assert!(matches!(
            store.reject_memory(kept.id, 12),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn delete_removes_the_memory_and_cascades_its_sources_and_links() {
        let mut store = ThreadStore::open_memory().expect("open");
        let old = accepted(&mut store, MemoryKind::Decision, "The old one");
        let new = propose(
            &mut store,
            MemoryKind::Decision,
            "The new one",
            &[file("src/a.rs"), commit(OID)],
        );
        store
            .accept_memory(new.id, None, &[old.id], &[], 30)
            .expect("supersede");
        store.delete_memory(new.id).expect("delete");
        assert!(matches!(
            store.read_memory(new.id),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.delete_memory(new.id),
            Err(StoreError::NotFound)
        ));
        let sources_left = count(
            &store,
            &format!(
                "SELECT COUNT(*) FROM memory_source WHERE memory_id = {}",
                new.id
            ),
        );
        assert_eq!(sources_left, 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM memory_rel"), 0);
        assert_eq!(
            store.read_memory(old.id).expect("old").status,
            MemoryStatus::Superseded
        );
    }

    #[test]
    fn file_sources_are_listed_by_path() {
        let mut store = ThreadStore::open_memory().expect("open");
        let memory = propose(
            &mut store,
            MemoryKind::Rule,
            "Two files",
            &[file("src/b.rs"), file("src/a.rs"), commit(OID)],
        );
        assert_eq!(
            store.memory_file_sources(memory.id).expect("paths"),
            ["src/a.rs", "src/b.rs"]
        );
    }

    /// Three memories with known fields, for the list and search tests. Newest first: `stash`,
    /// `decision`, `release`.
    fn seeded(store: &mut ThreadStore) -> (Memory, Memory, Memory) {
        let release = propose(
            store,
            MemoryKind::Rule,
            "Always run the release checks before tagging",
            &[file("src/release.rs")],
        );
        let decision = human_memory(
            store,
            MemoryKind::Decision,
            "We keep threads in SQLite",
            &[file("src/store.rs")],
            20,
        );
        let sources = [commit(OID)];
        let new = NewMemory {
            kind: MemoryKind::Lesson,
            text: "The rebase helper drops the stash on conflict",
            status: MemoryStatus::Proposed,
            origin: "agent:y",
            sources: &sources,
            at: 30,
        };
        let stash = store.propose_memory(&new).expect("stash").0;
        (release, decision, stash)
    }

    fn ids(memories: &[Memory]) -> Vec<i64> {
        memories.iter().map(|memory| memory.id).collect()
    }

    #[test]
    fn list_filters_by_kind_status_path_and_author() {
        let mut store = ThreadStore::open_memory().expect("open");
        let (release, decision, stash) = seeded(&mut store);
        let list =
            |filter: &MemoryFilter| store.list_memories("", filter, 50, None).expect("list").0;
        let by = |kind: Option<MemoryKind>,
                  status: Option<MemoryStatus>,
                  path: Option<&str>,
                  author: Option<&str>| MemoryFilter {
            kind,
            status,
            path: path.map(str::to_owned),
            author: author.map(str::to_owned),
        };
        assert_eq!(
            ids(&list(&MemoryFilter::default())),
            [stash.id, decision.id, release.id]
        );
        assert_eq!(
            ids(&list(&by(Some(MemoryKind::Rule), None, None, None))),
            [release.id]
        );
        assert_eq!(
            ids(&list(&by(None, Some(MemoryStatus::Accepted), None, None))),
            [decision.id]
        );
        assert_eq!(
            ids(&list(&by(None, Some(MemoryStatus::Proposed), None, None))),
            [stash.id, release.id]
        );
        assert_eq!(
            ids(&list(&by(None, None, Some("src"), None))),
            [decision.id, release.id]
        );
        assert_eq!(
            ids(&list(&by(None, None, Some("src/store.rs"), None))),
            [decision.id]
        );
        assert!(
            list(&by(None, None, Some("src/stor"), None)).is_empty(),
            "a path is whole segments"
        );
        assert_eq!(
            ids(&list(&by(None, None, None, Some("agent:y")))),
            [stash.id]
        );
        assert_eq!(
            ids(&list(&by(None, None, None, Some("human")))),
            [decision.id]
        );
    }

    #[test]
    fn a_query_finds_words_by_prefix_and_ignores_fts_operators() {
        let mut store = ThreadStore::open_memory().expect("open");
        let (release, decision, stash) = seeded(&mut store);
        let find = |query: &str| {
            store
                .list_memories(query, &MemoryFilter::default(), 50, None)
                .expect("search")
                .0
        };
        assert_eq!(ids(&find("relea")), [release.id]);
        assert_eq!(ids(&find("sqlite")), [decision.id]);
        assert_eq!(ids(&find("stash conflict")), [stash.id]);
        assert!(find("nothing like this").is_empty());
        assert!(
            find("foo AND OR \"").is_empty(),
            "operators typed by accident are words"
        );
        assert!(find("!!!").is_empty(), "a query with no word finds nothing");
    }

    #[test]
    fn paging_walks_every_row_and_stops_at_the_last() {
        let mut store = ThreadStore::open_memory().expect("open");
        let (release, decision, stash) = seeded(&mut store);
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, next) = store
                .list_memories("", &MemoryFilter::default(), 1, cursor.as_deref())
                .expect("page");
            assert_eq!(page.len(), 1);
            seen.push(page[0].id);
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
            assert!(seen.len() < 10, "the walk ends");
        }
        assert_eq!(seen, [stash.id, decision.id, release.id]);
    }

    #[test]
    fn a_cursor_from_another_request_and_a_malformed_one_are_refused() {
        let mut store = ThreadStore::open_memory().expect("open");
        seeded(&mut store);
        let (_, next) = store
            .list_memories("", &MemoryFilter::default(), 1, None)
            .expect("first page");
        let next = next.expect("more rows");
        let other_query =
            store.list_memories("sqlite", &MemoryFilter::default(), 1, Some(next.as_str()));
        assert!(matches!(other_query, Err(StoreError::Invalid(ref m)) if m == CURSOR_MISMATCH));
        let other_filter = MemoryFilter {
            kind: Some(MemoryKind::Rule),
            ..MemoryFilter::default()
        };
        assert!(
            matches!(
                store.list_memories("", &other_filter, 1, Some(next.as_str())),
                Err(StoreError::Invalid(_))
            ),
            "the filter is part of the cursor"
        );
        let malformed = store.list_memories("", &MemoryFilter::default(), 1, Some("nonsense"));
        assert!(matches!(malformed, Err(StoreError::Invalid(ref m)) if m == CURSOR_MALFORMED));
    }
}
