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
    FileChange, FileChangeState, Memory, MemoryBasis, MemoryFilter, MemoryKind, MemoryMark,
    MemoryMarkSummary, MemorySource, MemorySourceInput, MemorySourceKind, MemoryStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use super::query::build_fts_query;
use super::store::{
    CURSOR_MALFORMED, CURSOR_MISMATCH, StoreError, ThreadStore, begin, invalid, thread_id,
};

/// Characters of a memory's text (D192 (1)). Text over this is refused, not cut.
pub const TEXT_CHARS: usize = 1000;
/// Characters of the reason of a mark (D202). A longer reason is refused, not cut.
pub const REASON_CHARS: usize = 200;
/// Hex digits of a uid and of a cursor digest (D192 (8)).
const UID_HEX: usize = 16;
/// The origin of a human's action. Every other origin is an agent's or an import's.
const HUMAN: &str = "human";
/// Keeps two uids made in one clock tick apart.
static UID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The columns of a memory row, in the order `memory_row` reads them.
const MEMORY_COLUMNS: &str = "m.id, m.uid, m.kind, m.text, m.status, m.origin, m.proposed_by,
    m.model, m.created_at, m.decided_at, m.decided_by, m.basis, m.retrieval_count,
    m.last_retrieved_at";

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
    /// How the memory is known (D201 (1)).
    pub basis: MemoryBasis,
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
    /// Marking a memory useful or wrong (D202): either actor may.
    Mark,
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

/// The word a basis is stored and sent as (D201 (1)).
#[must_use]
pub fn basis_text(basis: MemoryBasis) -> &'static str {
    match basis {
        MemoryBasis::Observed => "observed",
        MemoryBasis::Tested => "tested",
        MemoryBasis::Unconfirmed => "unconfirmed",
    }
}

/// The word a mark is stored and sent as (D202).
#[must_use]
pub fn mark_text(mark: MemoryMark) -> &'static str {
    match mark {
        MemoryMark::Useful => "useful",
        MemoryMark::Wrong => "wrong",
    }
}

/// The mark a word names: `useful` or `wrong`. Any other word is refused, and the message names the
/// words that are allowed (D202).
pub fn check_mark(text: &str) -> Result<MemoryMark, String> {
    parse_mark(text).ok_or_else(|| format!("mark must be useful or wrong, not {text:?}"))
}

fn parse_mark(text: &str) -> Option<MemoryMark> {
    match text {
        "useful" => Some(MemoryMark::Useful),
        "wrong" => Some(MemoryMark::Wrong),
        _ => None,
    }
}

/// The reason of a mark as stored: trimmed, `None` when it is empty, and at most `REASON_CHARS`
/// characters. Called after the redaction, as the text of a memory is (D202).
pub fn check_reason(text: Option<&str>) -> Result<Option<String>, String> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > REASON_CHARS {
        return Err(format!("a reason is at most {REASON_CHARS} characters"));
    }
    Ok(Some(text.to_owned()))
}

/// Whether a mark is a repeat of the actor's latest mark, and so writes nothing (D202 (2)).
#[must_use]
pub fn is_repeat(latest: Option<MemoryMark>, mark: MemoryMark) -> bool {
    latest == Some(mark)
}

/// The actors whose latest mark is `wrong`, in the order the summaries give them (D202 (3)). The
/// result is what a `wrong` note names.
#[must_use]
pub fn marked_wrong_by(marks: &[MemoryMarkSummary]) -> Vec<&str> {
    marks
        .iter()
        .filter(|summary| summary.mark == MemoryMark::Wrong)
        .map(|summary| summary.actor.as_str())
        .collect()
}

/// A memory can be marked only while it is accepted (D202 (2)): a proposal is not yet a memory
/// anyone relies on, and a rejected or superseded one is history.
pub fn check_markable(status: MemoryStatus) -> Result<(), String> {
    if status == MemoryStatus::Accepted {
        Ok(())
    } else {
        Err(format!(
            "only an accepted memory can be marked, and this one is {}",
            status_text(status)
        ))
    }
}

/// The basis a word names: `observed`, `tested` or `unconfirmed`. Any other word is refused, and the
/// message names the words that are allowed (D201 (1)).
pub fn check_basis(text: &str) -> Result<MemoryBasis, String> {
    parse_basis(text)
        .ok_or_else(|| format!("basis must be observed, tested or unconfirmed, not {text:?}"))
}

/// A memory's basis when the word is a known one.
fn parse_basis(text: &str) -> Option<MemoryBasis> {
    match text {
        "observed" => Some(MemoryBasis::Observed),
        "tested" => Some(MemoryBasis::Tested),
        "unconfirmed" => Some(MemoryBasis::Unconfirmed),
        _ => None,
    }
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
        MemoryAction::Propose { .. } | MemoryAction::Mark => Ok(()),
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

/// Why a file source could not be read at `HEAD` (D193 (9)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The source's repository is not on this machine, or its recorded folder is gone: nothing is
    /// read, and nothing is compared.
    Unavailable,
    /// Git could not answer for the file.
    Failed,
}

/// What acceptance read of a file source's blob at `HEAD` (D193 (9)). A blob that could not be read
/// is `NotRead`, never taken for an absent file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobRead {
    /// Read: the file is at `HEAD` with this blob.
    Blob(String),
    /// Read: the file is not at `HEAD`.
    Absent,
    /// Not read: the source's folder is not known, or Git did not answer.
    NotRead,
}

impl BlobRead {
    /// The answer of Git for a file: `Ok(None)` is an absent file, an error is not read.
    pub fn of<E>(answer: Result<Option<String>, E>) -> Self {
        match answer {
            Ok(Some(blob)) => Self::Blob(blob),
            Ok(None) => Self::Absent,
            Err(_) => Self::NotRead,
        }
    }

    /// The blob to record: `None` when the file is absent or was not read.
    #[must_use]
    pub fn oid(&self) -> Option<&str> {
        match self {
            Self::Blob(blob) => Some(blob.as_str()),
            Self::Absent | Self::NotRead => None,
        }
    }

    /// Whether the blob was read, the value `blob_read` stores.
    #[must_use]
    pub fn was_read(&self) -> bool {
        !matches!(self, Self::NotRead)
    }
}

/// Whether a file a memory names has changed since it was accepted (D192 (6)). `current` gives the
/// blob of a file source at `HEAD` of the repository the source names: `Ok(None)` when there is no
/// such file, and an error when the source cannot be read. Only `accepted` and `superseded` memories
/// are checked; a proposal has nothing to compare with. The recorded blob is the one of the last
/// acceptance; a file that was absent then and is there now has changed. A renamed file is `missing`
/// at its old path: nothing guesses its new name.
#[must_use]
pub fn files_changed_since(
    memory: &Memory,
    current: &dyn Fn(&MemorySource) -> Result<Option<String>, Lookup>,
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
            let state = if source.blob_read {
                changed_state(source, current)?
            } else {
                // Never read at acceptance, so there is nothing to compare with: no lookup either.
                FileChangeState::Unavailable
            };
            Some(FileChange {
                path: source.reference.clone(),
                state,
            })
        })
        .collect()
}

/// The state of a file whose blob was read at acceptance; `None` when it is unchanged.
fn changed_state(
    source: &MemorySource,
    current: &dyn Fn(&MemorySource) -> Result<Option<String>, Lookup>,
) -> Option<FileChangeState> {
    match (source.blob_oid.as_deref(), current(source)) {
        (_, Err(Lookup::Unavailable)) => Some(FileChangeState::Unavailable),
        (_, Err(Lookup::Failed)) => Some(FileChangeState::Unknown),
        (Some(recorded), Ok(Some(now))) if recorded != now => Some(FileChangeState::Changed),
        (Some(_), Ok(Some(_))) | (None, Ok(None)) => None,
        (Some(_), Ok(None)) => Some(FileChangeState::Missing),
        (None, Ok(Some(_))) => Some(FileChangeState::Changed),
    }
}

/// A new uid: the first 16 hex digits of SHA-256 over 16 bytes from the process's random hasher
/// state, the clock and a counter (D192 (8)).
#[must_use]
pub fn new_uid() -> String {
    hex_prefix(&Sha256::digest(random_seed()))
}

/// `digits` lowercase hex digits of SHA-256 over the same random seed as `new_uid`, for ids longer
/// than a uid (the repository id, D193 (2)). `digits` is at most 64.
#[must_use]
pub fn new_hex(digits: usize) -> String {
    hex_of(&Sha256::digest(random_seed()), digits)
}

/// 16 bytes from the process's random hasher state, the clock and a counter.
fn random_seed() -> [u8; 16] {
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
    seed
}

/// Where a memory is written and what its duplicate key is looked for among (D190 (7), D193 (6)).
pub(crate) struct Placement<'a> {
    /// Each thread source must name a thread stored in this store (`threads.db` only).
    pub threads: bool,
    /// The memories a duplicate is looked for among; `None` is every memory of the store.
    pub among: Option<&'a [i64]>,
}

/// A proposal after the checks that need no store: its text as stored, its sources and its
/// duplicate key.
pub(crate) struct Checked {
    pub text: String,
    pub sources: Vec<MemorySource>,
    pub key: String,
}

/// The rules of a proposal that need no store: the text, who may propose and with what status,
/// the sources and the duplicate key (D192 (1), (2), (3), (5)).
pub(crate) fn check_proposal(new: &NewMemory<'_>) -> Result<Checked, StoreError> {
    check_text(new.text)?;
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
    check_content(new)
}

/// The rules of a memory's content that need no store: its text, its sources and its duplicate key
/// (D192 (1), (2), (5)). Who may write it is checked by the caller.
pub(crate) fn check_content(new: &NewMemory<'_>) -> Result<Checked, StoreError> {
    let text = check_text(new.text)?;
    let sources = new
        .sources
        .iter()
        .map(checked_source)
        .collect::<Result<Vec<_>, _>>()?;
    let key = dedup_key(new.kind, &text);
    Ok(Checked { text, sources, key })
}

/// The history a memory keeps when it moves into a context (D193 (8)): its proposer, model, creation
/// and decision, copied as they are.
pub(crate) struct Carried<'a> {
    pub proposed_by: Option<&'a str>,
    pub model: Option<&'a str>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub decided_by: Option<&'a str>,
    /// The retrieval count and time it has so far (D201 (4)): a moved memory is the same memory.
    pub retrieval_count: u32,
    pub last_retrieved_at: Option<i64>,
    /// Every mark the memory has, oldest first (D202): a moved memory keeps its history.
    pub marks: &'a [MemoryMarkSummary],
}

/// Stores a checked proposal inside the caller's transaction (D192 (2), (3)). A memory with the
/// given `uid` already in the store is returned with `false`, and so is a non-rejected memory of the
/// same kind and key among `place.among`; nothing is written then. Otherwise the memory is inserted
/// under `uid`, or under a new uid, and returned with `true`. A `carried` history replaces the one a
/// new memory gets (D193 (8)).
pub(crate) fn propose_in(
    conn: &Connection,
    new: &NewMemory<'_>,
    checked: &Checked,
    place: &Placement<'_>,
    uid: Option<&str>,
    carried: Option<&Carried<'_>>,
) -> Result<(Memory, bool), StoreError> {
    if let Some(uid) = uid
        && let Some(id) = id_by_uid(conn, uid)?
    {
        return Ok((memory_by_id(conn, id)?, false));
    }
    if place.threads {
        for source in checked
            .sources
            .iter()
            .filter(|source| source.kind == MemorySourceKind::Thread)
        {
            thread_id(conn, &source.reference).map_err(|error| match error {
                StoreError::NotFound => invalid("a thread source names a stored thread"),
                other => other,
            })?;
        }
    }
    if let Some(existing) = find_duplicate(conn, new.kind, &checked.key, None, place.among)? {
        return Ok((memory_by_id(conn, existing)?, false));
    }
    let accepted = new.status == MemoryStatus::Accepted;
    let uid = uid.map_or_else(new_uid, str::to_owned);
    let (proposed_by, model, created_at, decided_at, decided_by, retrieval) = match carried {
        Some(kept) => (
            kept.proposed_by,
            kept.model,
            kept.created_at,
            kept.decided_at,
            kept.decided_by,
            (kept.retrieval_count, kept.last_retrieved_at),
        ),
        None => (
            Some(new.origin),
            None,
            new.at,
            accepted.then_some(new.at),
            accepted.then_some(HUMAN),
            (0, None),
        ),
    };
    conn.execute(
        "INSERT INTO memory (uid, kind, text, basis, status, origin, proposed_by, model, created_at,
            decided_at, decided_by, retrieval_count, last_retrieved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            uid,
            kind_text(new.kind),
            checked.text,
            basis_text(new.basis),
            status_text(new.status),
            new.origin,
            proposed_by,
            model,
            created_at,
            decided_at,
            decided_by,
            i64::from(retrieval.0),
            retrieval.1,
        ],
    )?;
    let id = conn.last_insert_rowid();
    insert_sources(conn, id, &checked.sources)?;
    if let Some(kept) = carried {
        insert_marks(conn, &uid, kept.marks)?;
    }
    Ok((memory_by_id(conn, id)?, true))
}

/// Accepts a memory inside the caller's transaction (a human's action, D192 (3) and (4)). On a
/// proposed memory this is the acceptance; on an accepted one it is an edit, or a supersede. `text`
/// replaces the text when it differs (the origin becomes `human`; `proposed_by` stays), and a key
/// that `place.among` holds for another memory is refused. `supersedes` must each name an accepted
/// memory other than this one; they become `superseded` with a `memory_rel` row. `blobs` gives the
/// blob of each file source at `HEAD` (`None` when the file is absent) and is recorded only when the
/// memory first becomes accepted: accepting an accepted memory again keeps its status, `decided_at`,
/// `decided_by` and the recorded blobs (D192 (4)), so "files changed since" still counts from the
/// first acceptance.
pub(crate) fn accept_in(
    conn: &Connection,
    place: &Placement<'_>,
    id: i64,
    text: Option<&str>,
    supersedes: &[i64],
    blobs: &[(String, BlobRead)],
    at: i64,
) -> Result<Memory, StoreError> {
    let (kind, stored_text, before) = core_of(conn, id)?;
    next_status(before, Transition::Accept).map_err(|message| invalid(&message))?;
    if let Some(text) = text {
        let text = check_text(text)?;
        if text != stored_text {
            if let Some(other) =
                find_duplicate(conn, kind, &dedup_key(kind, &text), Some(id), place.among)?
            {
                return Err(invalid(&format!("memory {other} already says this")));
            }
            conn.execute(
                "UPDATE memory SET text = ?1, origin = 'human' WHERE id = ?2",
                params![text, id],
            )?;
        }
    }
    // Accepting an accepted memory again keeps its decision and its recorded blobs (D192 (4)).
    if before != MemoryStatus::Accepted {
        conn.execute(
            "UPDATE memory SET status = 'accepted', decided_at = ?1, decided_by = 'human'
             WHERE id = ?2",
            params![at, id],
        )?;
        for (path, read) in blobs {
            conn.execute(
                "UPDATE memory_source SET blob_oid = ?1, blob_read = ?2
                 WHERE memory_id = ?3 AND kind = 'file' AND ref = ?4",
                params![read.oid(), read.was_read(), id, path],
            )?;
        }
    }
    for &old in supersedes {
        if old == id {
            return Err(invalid("a memory cannot supersede itself"));
        }
        let (_, _, old_status) = core_of(conn, old).map_err(|error| match error {
            StoreError::NotFound => invalid(&format!("memory {old} does not exist")),
            other => other,
        })?;
        next_status(old_status, Transition::Supersede)
            .map_err(|message| invalid(&format!("memory {old}: {message}")))?;
        conn.execute(
            "UPDATE memory SET status = 'superseded', decided_at = ?1, decided_by = 'human'
             WHERE id = ?2",
            params![at, old],
        )?;
        conn.execute(
            "INSERT INTO memory_rel (from_memory, to_memory, kind, origin, at)
             VALUES (?1, ?2, 'supersedes', 'human', ?3)",
            params![id, old, at],
        )?;
    }
    memory_by_id(conn, id)
}

/// Rejects a proposed memory inside the caller's transaction (a human's action).
pub(crate) fn reject_in(conn: &Connection, id: i64, at: i64) -> Result<Memory, StoreError> {
    let (_, _, before) = core_of(conn, id)?;
    next_status(before, Transition::Reject).map_err(|message| invalid(&message))?;
    conn.execute(
        "UPDATE memory SET status = 'rejected', decided_at = ?1, decided_by = 'human'
         WHERE id = ?2",
        params![at, id],
    )?;
    memory_by_id(conn, id)
}

/// Deletes a memory with its sources and its links (a human's action). `NotFound` when missing.
pub(crate) fn delete_in(conn: &Connection, id: i64) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM memory_mark WHERE memory_uid = (SELECT uid FROM memory WHERE id = ?1)",
        params![id],
    )?;
    if conn.execute("DELETE FROM memory WHERE id = ?1", params![id])? == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

/// The repository-relative paths of a memory's file sources, by path.
pub(crate) fn file_source_paths(conn: &Connection, id: i64) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT ref FROM memory_source WHERE memory_id = ?1 AND kind = 'file' ORDER BY ref",
    )?;
    let paths = statement
        .query_map(params![id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(paths)
}

/// The row id of the memory with `uid`, when there is one.
pub(crate) fn id_by_uid(conn: &Connection, uid: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM memory WHERE uid = ?1",
            params![uid],
            |row| row.get(0),
        )
        .optional()?)
}

/// The row id of the memory with `uid`, or `NotFound`.
pub(crate) fn id_of_uid(conn: &Connection, uid: &str) -> Result<i64, StoreError> {
    id_by_uid(conn, uid)?.ok_or(StoreError::NotFound)
}

/// A memory by uid, with its sources and relations. `NotFound` when missing.
pub(crate) fn memory_by_uid(conn: &Connection, uid: &str) -> Result<Memory, StoreError> {
    memory_by_id(conn, id_of_uid(conn, uid)?)
}

/// A memory a list finds, before its page is read: its row, its uid, its creation time and its
/// `bm25` score (0.0 without a query).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub id: i64,
    pub uid: String,
    pub created_at: i64,
    pub score: f64,
}

/// Every memory that `query` and `filter` select, restricted to `within` (a SQL condition on `m.id`,
/// or empty), unpaged. With a query they are ordered by relevance (the D128 builder over
/// `memory_fts`), otherwise newest first; ties are ordered by uid, so every store lists alike (D193
/// (6)). A query with no searchable word selects nothing.
pub(crate) fn list_ids(
    conn: &Connection,
    query: &str,
    filter: &MemoryFilter,
    within: &str,
) -> Result<Vec<Found>, StoreError> {
    let fts = if query.trim().is_empty() {
        None
    } else {
        let Some(expression) = build_fts_query(query) else {
            return Ok(Vec::new());
        };
        Some(expression)
    };
    let sql = match fts {
        Some(_) => format!(
            "SELECT m.id, m.uid, m.created_at, bm25(memory_fts) FROM memory m
             JOIN memory_fts ON memory_fts.rowid = m.id
             WHERE memory_fts MATCH ?1{MEMORY_FILTER_SQL}{within}
             ORDER BY bm25(memory_fts), m.uid"
        ),
        None => format!(
            "SELECT m.id, m.uid, m.created_at, 0.0 FROM memory m WHERE 1 = 1{MEMORY_FILTER_SQL}{within}
             ORDER BY m.created_at DESC, m.uid"
        ),
    };
    let kind = filter.kind.map(kind_text);
    let status = filter.status.map(status_text);
    let path = filter.path.as_deref().filter(|path| !path.is_empty());
    let author = filter.author.as_deref().filter(|author| !author.is_empty());
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(params![fts, kind, status, path, author], |row| {
            Ok(Found {
                id: row.get(0)?,
                uid: row.get(1)?,
                created_at: row.get(2)?,
                score: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// How many memories are proposed and inside `within`, an SQL restriction on `m` that starts with
/// ` AND` (empty for the whole store). One SQL count (D193 (6)).
pub(crate) fn count_proposed(conn: &Connection, within: &str) -> Result<u32, StoreError> {
    let sql = format!("SELECT COUNT(*) FROM memory m WHERE m.status = ?1{within}");
    let count: i64 = conn.query_row(&sql, params![status_text(MemoryStatus::Proposed)], |row| {
        row.get(0)
    })?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The uids of the proposed memories inside `within` (see `count_proposed`).
pub(crate) fn proposed_uids(conn: &Connection, within: &str) -> Result<Vec<String>, StoreError> {
    let sql = format!("SELECT m.uid FROM memory m WHERE m.status = ?1{within}");
    let mut statement = conn.prepare(&sql)?;
    let uids = statement
        .query_map(params![status_text(MemoryStatus::Proposed)], |row| {
            row.get(0)
        })?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(uids)
}

/// Counts one retrieval of memory `uid`, at `at` (D201 (4)): one UPDATE. `false` when no memory of
/// that uid is stored here. Not a command of its own: the caller decides what may be counted.
pub(crate) fn count_retrieval(conn: &Connection, uid: &str, at: i64) -> Result<bool, StoreError> {
    let changed = conn.execute(
        "UPDATE memory SET retrieval_count = retrieval_count + 1, last_retrieved_at = ?2
         WHERE uid = ?1",
        params![uid, at],
    )?;
    Ok(changed > 0)
}

/// The uid and the characters of the text of each accepted memory inside `within` (see
/// `count_proposed`). `length` counts characters, not bytes, as the size line does (D201 (5)).
pub(crate) fn accepted_text(
    conn: &Connection,
    within: &str,
) -> Result<Vec<(String, u64)>, StoreError> {
    let sql = format!("SELECT m.uid, length(m.text) FROM memory m WHERE m.status = ?1{within}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(params![status_text(MemoryStatus::Accepted)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(uid, characters)| (uid, u64::try_from(characters).unwrap_or(0)))
        .collect())
}

/// The size of accepted memories given as `(uid, characters)`: how many there are and the
/// characters of their text together. A memory that two stores hold is counted once (D201 (5)).
#[must_use]
pub fn accepted_size(rows: impl IntoIterator<Item = (String, u64)>) -> (u32, u64) {
    let mut seen = HashSet::new();
    let mut count = 0_u32;
    let mut characters = 0_u64;
    for (uid, length) in rows {
        if seen.insert(uid) {
            count = count.saturating_add(1);
            characters = characters.saturating_add(length);
        }
    }
    (count, characters)
}

/// How many of `uids` are proposed memories. Chunked, so a long list stays under SQLite's
/// parameter limit.
pub(crate) fn count_proposed_among(conn: &Connection, uids: &[String]) -> Result<u32, StoreError> {
    const CHUNK: usize = 500;
    let mut total = 0_u32;
    for chunk in uids.chunks(CHUNK) {
        // ?1 is the status; the uids follow from ?2.
        let placeholders = (2..2 + chunk.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT COUNT(*) FROM memory m WHERE m.status = ?1 AND m.uid IN ({placeholders})"
        );
        let proposed = status_text(MemoryStatus::Proposed);
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&proposed];
        values.extend(chunk.iter().map(|uid| uid as &dyn rusqlite::ToSql));
        let count: i64 = conn.query_row(&sql, values.as_slice(), |row| row.get(0))?;
        total = total.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
    }
    Ok(total)
}

impl ThreadStore {
    /// Stores a memory, or returns the memory that holds its text: a non-rejected memory of the same
    /// kind with the same duplicate key is returned with `false`, and nothing is written (D192 (2)).
    /// Otherwise the memory is inserted and returned with `true`.
    pub fn propose_memory(&mut self, new: &NewMemory<'_>) -> Result<(Memory, bool), StoreError> {
        let checked = check_proposal(new)?;
        let tx = begin(&mut self.conn)?;
        let placed = propose_in(
            &tx,
            new,
            &checked,
            &Placement {
                threads: true,
                among: None,
            },
            None,
            None,
        )?;
        tx.commit()?;
        Ok(placed)
    }

    /// Accepts a memory (a human's action, D192 (3) and (4)); see `accept_in`. Every change is one
    /// transaction: a refusal writes nothing.
    pub fn accept_memory(
        &mut self,
        id: i64,
        text: Option<&str>,
        supersedes: &[i64],
        blobs: &[(String, BlobRead)],
        at: i64,
    ) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let memory = accept_in(
            &tx,
            &Placement {
                threads: false,
                among: None,
            },
            id,
            text,
            supersedes,
            blobs,
            at,
        )?;
        tx.commit()?;
        Ok(memory)
    }

    /// Rejects a proposed memory (a human's action).
    pub fn reject_memory(&mut self, id: i64, at: i64) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let memory = reject_in(&tx, id, at)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Deletes a memory with its sources and its links (a human's action). `NotFound` when missing.
    pub fn delete_memory(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        delete_in(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Records `mark` of `actor` on the accepted memory `uid` (D202), in one transaction. The bool is
    /// false when the actor's latest mark already is `mark`, and nothing is written then.
    pub(crate) fn mark_memory(
        &mut self,
        uid: &str,
        actor: &str,
        mark: MemoryMark,
        reason: Option<&str>,
        at: i64,
    ) -> Result<(Memory, bool), StoreError> {
        let tx = begin(&mut self.conn)?;
        let marked = mark_in(&tx, uid, actor, mark, reason, at)?;
        tx.commit()?;
        Ok(marked)
    }

    /// Every mark of memory `uid`, oldest first (see `mark_history`).
    pub(crate) fn memory_mark_history(
        &self,
        uid: &str,
    ) -> Result<Vec<MemoryMarkSummary>, StoreError> {
        mark_history(&self.conn, uid)
    }

    /// A memory with its sources and its supersedes links. `NotFound` when missing.
    pub fn read_memory(&self, id: i64) -> Result<Memory, StoreError> {
        memory_by_id(&self.conn, id)
    }

    /// A memory by uid, with its sources and its supersedes links. `NotFound` when missing.
    pub fn read_memory_by_uid(&self, uid: &str) -> Result<Memory, StoreError> {
        memory_by_uid(&self.conn, uid)
    }

    /// The row id of the memory with `uid`. `NotFound` when missing.
    pub fn memory_id_of(&self, uid: &str) -> Result<i64, StoreError> {
        id_of_uid(&self.conn, uid)
    }

    /// The repository-relative paths of a memory's file sources, by path.
    pub fn memory_file_sources(&self, id: i64) -> Result<Vec<String>, StoreError> {
        file_source_paths(&self.conn, id)
    }

    /// Every memory `query` and `filter` select, unpaged, for the engine to merge with the
    /// memories of the contexts (D193 (6)).
    pub(crate) fn find_memories(
        &self,
        query: &str,
        filter: &MemoryFilter,
    ) -> Result<Vec<Found>, StoreError> {
        list_ids(&self.conn, query, filter, "")
    }

    /// How many proposed memories this store holds (see `count_proposed`).
    pub(crate) fn count_proposed(&self) -> Result<u32, StoreError> {
        count_proposed(&self.conn, "")
    }

    /// Counts one retrieval of memory `uid` now (see `count_retrieval`).
    pub(crate) fn count_retrieval(&self, uid: &str, at: i64) -> Result<bool, StoreError> {
        count_retrieval(&self.conn, uid, at)
    }

    /// The accepted memories of this store as `(uid, characters)` (see `accepted_text`).
    pub(crate) fn accepted_text(&self) -> Result<Vec<(String, u64)>, StoreError> {
        accepted_text(&self.conn, "")
    }

    /// How many of `uids` are proposed memories of this store.
    pub(crate) fn count_proposed_among(&self, uids: &[String]) -> Result<u32, StoreError> {
        count_proposed_among(&self.conn, uids)
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
        let digest = list_digest(query, filter, REPOSITORY_SCOPE);
        let offset = match cursor {
            Some(cursor) => decode_offset(cursor, &digest)?,
            None => 0,
        };
        let found = list_ids(&self.conn, query, filter, "")?;
        let total = found.len();
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
        let end = start
            .saturating_add(usize::try_from(limit.max(1)).unwrap_or(usize::MAX))
            .min(total);
        let next = (end < total).then(|| format!("{digest}.{end}"));
        let mut memories = Vec::with_capacity(end - start);
        for hit in &found[start..end] {
            memories.push(memory_by_id(&self.conn, hit.id)?);
        }
        Ok((memories, next))
    }
}

/// The scope a cursor of the repository's own memories names (D193 (6)).
pub(crate) const REPOSITORY_SCOPE: &str = "repository";

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
        blob_read: source.blob_read,
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

/// The id of a non-rejected memory of `kind` with duplicate key `key`, other than `except`, among
/// `among` when it is given.
fn find_duplicate(
    conn: &Connection,
    kind: MemoryKind,
    key: &str,
    except: Option<i64>,
    among: Option<&[i64]>,
) -> Result<Option<i64>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, text FROM memory WHERE kind = ?1 AND status != 'rejected' ORDER BY id",
    )?;
    let rows = statement.query_map(params![kind_text(kind)], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, text) = row?;
        let listed = among.is_none_or(|ids| ids.contains(&id));
        if Some(id) != except && listed && dedup_key(kind, &text) == key {
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
            "INSERT INTO memory_source
                (memory_id, kind, ref, repo_id, entry_from, entry_to, blob_oid, blob_read)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                source_kind_text(source.kind),
                source.reference,
                source.repo_id,
                source.entry_from,
                source.entry_to,
                source.blob_oid,
                source.blob_read,
            ],
        )?;
    }
    Ok(())
}

/// A memory with its sources and relations, or `NotFound`.
pub(crate) fn memory_by_id(conn: &Connection, id: i64) -> Result<Memory, StoreError> {
    let sql = format!("SELECT {MEMORY_COLUMNS} FROM memory m WHERE m.id = ?1");
    let mut memory = conn
        .query_row(&sql, params![id], memory_row)
        .optional()?
        .ok_or(StoreError::NotFound)??;
    memory.sources = sources_of(conn, id)?;
    memory.supersedes = related_uids(
        conn,
        "SELECT m.uid FROM memory_rel r JOIN memory m ON m.id = r.to_memory
         WHERE r.from_memory = ?1 AND r.kind = 'supersedes' ORDER BY m.uid",
        id,
    )?;
    memory.superseded_by = related_uids(
        conn,
        "SELECT m.uid FROM memory_rel r JOIN memory m ON m.id = r.from_memory
         WHERE r.to_memory = ?1 AND r.kind = 'supersedes' ORDER BY m.uid",
        id,
    )?;
    memory.marks = mark_summaries(conn, &memory.uid)?;
    Ok(memory)
}

/// Writes `mark` of `actor` (its origin, `human` or `agent:<client>`) on memory `uid`, inside the
/// caller's transaction (D202). Only an accepted memory takes a mark. A mark equal to the actor's
/// latest mark writes nothing. Returns the memory as it now is and whether a row was written.
pub(crate) fn mark_in(
    conn: &Connection,
    uid: &str,
    actor: &str,
    mark: MemoryMark,
    reason: Option<&str>,
    at: i64,
) -> Result<(Memory, bool), StoreError> {
    let id = id_of_uid(conn, uid)?;
    let status: String = conn.query_row(
        "SELECT status FROM memory WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let status =
        parse_status(&status).ok_or_else(|| invalid("a memory holds an unknown status"))?;
    check_markable(status).map_err(|message| invalid(&message))?;
    if is_repeat(latest_mark(conn, uid, actor)?, mark) {
        return Ok((memory_by_id(conn, id)?, false));
    }
    conn.execute(
        "INSERT INTO memory_mark (memory_uid, actor, mark, reason, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![uid, actor, mark_text(mark), reason, at],
    )?;
    Ok((memory_by_id(conn, id)?, true))
}

/// The latest mark of `actor` on memory `uid`, if it has marked it.
fn latest_mark(
    conn: &Connection,
    uid: &str,
    actor: &str,
) -> Result<Option<MemoryMark>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT mark FROM memory_mark WHERE memory_uid = ?1 AND actor = ?2
             ORDER BY id DESC LIMIT 1",
            params![uid, actor],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| parse_mark(&text).ok_or_else(|| invalid("a memory mark holds an unknown mark")))
        .transpose()
}

/// The latest mark of each actor on memory `uid`, by actor (D202 (2)). Earlier marks are not here.
pub(crate) fn mark_summaries(
    conn: &Connection,
    uid: &str,
) -> Result<Vec<MemoryMarkSummary>, StoreError> {
    mark_rows(
        conn,
        "SELECT m.actor, m.mark, m.at, m.reason FROM memory_mark m
         WHERE m.memory_uid = ?1 AND m.id = (SELECT MAX(x.id) FROM memory_mark x
            WHERE x.memory_uid = m.memory_uid AND x.actor = m.actor)
         ORDER BY m.actor",
        uid,
    )
}

/// Every mark memory `uid` has, oldest first (D202 (2)): the history the marks keep.
pub(crate) fn mark_history(
    conn: &Connection,
    uid: &str,
) -> Result<Vec<MemoryMarkSummary>, StoreError> {
    mark_rows(
        conn,
        "SELECT actor, mark, at, reason FROM memory_mark WHERE memory_uid = ?1 ORDER BY id",
        uid,
    )
}

fn mark_rows(
    conn: &Connection,
    sql: &str,
    uid: &str,
) -> Result<Vec<MemoryMarkSummary>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![uid], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut marks = Vec::new();
    for row in rows {
        let (actor, mark, at, reason) = row?;
        let mark =
            parse_mark(&mark).ok_or_else(|| invalid("a memory mark holds an unknown mark"))?;
        marks.push(MemoryMarkSummary {
            actor,
            mark,
            at,
            reason,
        });
    }
    Ok(marks)
}

/// Stores carried marks under memory `uid` (D202), in the order they were made.
pub(crate) fn insert_marks(
    conn: &Connection,
    uid: &str,
    marks: &[MemoryMarkSummary],
) -> Result<(), StoreError> {
    for mark in marks {
        conn.execute(
            "INSERT INTO memory_mark (memory_uid, actor, mark, reason, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                uid,
                mark.actor,
                mark_text(mark.mark),
                mark.reason,
                mark.at
            ],
        )?;
    }
    Ok(())
}

/// A memory row without its sources and relations. A stored value that is not a known kind or
/// status is an error of the row, not of the query.
fn memory_row(row: &Row<'_>) -> rusqlite::Result<Result<Memory, StoreError>> {
    let kind: String = row.get(2)?;
    let status: String = row.get(4)?;
    let basis: String = row.get(11)?;
    let retrieval_count: i64 = row.get(12)?;
    let (Some(kind), Some(status), Some(basis)) = (
        parse_kind(&kind),
        parse_status(&status),
        parse_basis(&basis),
    ) else {
        return Ok(Err(invalid(
            "a memory holds an unknown kind, status or basis",
        )));
    };
    Ok(Ok(Memory {
        id: row.get(0)?,
        uid: row.get(1)?,
        kind,
        text: row.get(3)?,
        basis,
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
        scope: String::new(),
        contexts: Vec::new(),
        retrieval_count: u32::try_from(retrieval_count).unwrap_or(0),
        last_retrieved_at: row.get(13)?,
        marks: Vec::new(),
    }))
}

fn sources_of(conn: &Connection, id: i64) -> Result<Vec<MemorySource>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT kind, ref, repo_id, entry_from, entry_to, blob_oid, blob_read FROM memory_source
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
            row.get::<_, bool>(6)?,
        ))
    })?;
    let mut sources = Vec::new();
    for row in rows {
        let (kind, reference, repo_id, entry_from, entry_to, blob_oid, blob_read) = row?;
        let kind = parse_source_kind(&kind)
            .ok_or_else(|| invalid("a memory source holds an unknown kind"))?;
        sources.push(MemorySource {
            kind,
            reference,
            repo_id,
            entry_from,
            entry_to,
            blob_oid,
            blob_read,
        });
    }
    Ok(sources)
}

/// The uids a query selects for memory `id` (its supersedes links, either way).
fn related_uids(conn: &Connection, sql: &str, id: i64) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let uids = statement
        .query_map(params![id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(uids)
}

/// The digest a cursor carries: the first 16 hex digits of SHA-256 over the query, the filter and the
/// scope the list covers (D193 (6)).
pub(crate) fn list_digest(query: &str, filter: &MemoryFilter, scope: &str) -> String {
    let material = format!(
        "{query}\0{:?}\0{:?}\0{:?}\0{:?}\0{scope}",
        filter.kind, filter.status, filter.path, filter.author
    );
    hex_prefix(&Sha256::digest(material.as_bytes()))
}

/// The offset a cursor names, when the cursor was made for this query, filter and scope.
pub(crate) fn decode_offset(cursor: &str, digest: &str) -> Result<i64, StoreError> {
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
    hex_of(digest, UID_HEX)
}

/// The first `digits` lowercase hex digits of `digest`.
fn hex_of(digest: &[u8], digits: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(digits + 1);
    for byte in digest.iter().take(digits.div_ceil(2)) {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text.truncate(digits);
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
            blob_read: false,
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
            blob_read: false,
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
            basis: MemoryBasis::Observed,
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
            basis: MemoryBasis::Observed,
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
    fn accepted_size_counts_each_memory_once_and_its_characters_together() {
        assert_eq!(accepted_size(Vec::new()), (0, 0));
        let rows = vec![
            ("a".to_owned(), 10),
            ("b".to_owned(), 5),
            // The same memory from the other store: counted once.
            ("a".to_owned(), 10),
        ];
        assert_eq!(accepted_size(rows), (2, 15));
    }

    #[test]
    fn a_basis_is_one_of_three_exact_words_and_any_other_is_refused_naming_them() {
        for (basis, word) in [
            (MemoryBasis::Observed, "observed"),
            (MemoryBasis::Tested, "tested"),
            (MemoryBasis::Unconfirmed, "unconfirmed"),
        ] {
            assert_eq!(basis_text(basis), word);
            assert_eq!(check_basis(word), Ok(basis));
        }
        for bad in ["", "guessed", "Observed", " tested", "belief"] {
            let error = check_basis(bad).expect_err("refused");
            assert!(
                error.starts_with("basis must be observed, tested or unconfirmed"),
                "{error}"
            );
        }
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
            basis: MemoryBasis::Observed,
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
            scope: String::new(),
            contexts: Vec::new(),
            retrieval_count: 0,
            last_retrieved_at: None,
            marks: Vec::new(),
        };
        let source = |path: &str, blob: Option<&str>| MemorySource {
            kind: MemorySourceKind::File,
            reference: path.to_owned(),
            repo_id: None,
            entry_from: None,
            entry_to: None,
            blob_oid: blob.map(str::to_owned),
            blob_read: true,
        };
        // a: same blob; b: edited; c: deleted; d: appeared; e: absent both times; f: git fails;
        // g: its repository is not on this machine.
        memory.sources = vec![
            source("c.rs", Some("c1")),
            source("a.rs", Some("a1")),
            source("b.rs", Some("b1")),
            source("d.rs", None),
            source("e.rs", None),
            source("f.rs", Some("f1")),
            source("g.rs", Some("g1")),
            MemorySource {
                kind: MemorySourceKind::Commit,
                ..source("0123", Some("x"))
            },
        ];
        let now = |source: &MemorySource| match source.reference.as_str() {
            "a.rs" => Ok(Some("a1".to_owned())),
            "b.rs" => Ok(Some("b2".to_owned())),
            "c.rs" | "e.rs" => Ok(None),
            "d.rs" => Ok(Some("d1".to_owned())),
            "f.rs" => Err(Lookup::Failed),
            "g.rs" => Err(Lookup::Unavailable),
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
            FileChange {
                path: "g.rs".into(),
                state: FileChangeState::Unavailable,
            },
        ];
        assert_eq!(changes, want);

        memory.status = MemoryStatus::Superseded;
        assert_eq!(files_changed_since(&memory, &now), want, "superseded too");

        memory.status = MemoryStatus::Proposed;
        assert!(files_changed_since(&memory, &now).is_empty());
    }

    #[test]
    fn a_file_whose_blob_was_not_read_at_acceptance_is_unavailable_without_a_lookup() {
        let mut memory = Memory {
            id: 1,
            uid: "u".into(),
            kind: MemoryKind::Rule,
            text: "t".into(),
            basis: MemoryBasis::Observed,
            status: MemoryStatus::Accepted,
            origin: "human".into(),
            proposed_by: None,
            model: None,
            created_at: 1,
            decided_at: Some(2),
            decided_by: Some("human".into()),
            reviewed: true,
            sources: vec![MemorySource {
                kind: MemorySourceKind::File,
                reference: "src/lib.rs".into(),
                repo_id: Some("other".into()),
                entry_from: None,
                entry_to: None,
                blob_oid: None,
                blob_read: false,
            }],
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
            scope: String::new(),
            contexts: Vec::new(),
            retrieval_count: 0,
            last_retrieved_at: None,
            marks: Vec::new(),
        };
        // The file is at HEAD now, but nothing was read to compare it with: never `Changed`.
        let lookup = |_: &MemorySource| -> Result<Option<String>, Lookup> {
            panic!("a source not read at acceptance is not looked up")
        };
        let want = vec![FileChange {
            path: "src/lib.rs".into(),
            state: FileChangeState::Unavailable,
        }];
        assert_eq!(files_changed_since(&memory, &lookup), want);
        memory.status = MemoryStatus::Superseded;
        assert_eq!(
            files_changed_since(&memory, &lookup),
            want,
            "superseded too"
        );
    }

    #[test]
    fn blob_read_tells_a_blob_an_absent_file_and_a_failed_read_apart() {
        assert_eq!(
            BlobRead::of::<()>(Ok(Some("abc".to_owned()))),
            BlobRead::Blob("abc".to_owned())
        );
        assert_eq!(BlobRead::of::<()>(Ok(None)), BlobRead::Absent);
        assert_eq!(
            BlobRead::of(Err::<Option<String>, _>(())),
            BlobRead::NotRead
        );
        assert_eq!(BlobRead::Blob("abc".into()).oid(), Some("abc"));
        assert_eq!(BlobRead::Absent.oid(), None);
        assert_eq!(BlobRead::NotRead.oid(), None);
        assert!(BlobRead::Absent.was_read());
        assert!(!BlobRead::NotRead.was_read());
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
            blob_read: false,
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
            basis: MemoryBasis::Observed,
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
        assert_eq!(old.superseded_by, vec![new.uid.clone()]);
        let new = store.read_memory(new.id).expect("new");
        assert_eq!(new.supersedes, vec![old.uid.clone()]);
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
            &[file("src/a.rs"), file("src/b.rs"), file("src/c.rs")],
        );
        let blobs = [
            ("src/a.rs".to_owned(), BlobRead::Blob("abc123".to_owned())),
            ("src/b.rs".to_owned(), BlobRead::Absent),
            ("src/c.rs".to_owned(), BlobRead::NotRead),
        ];
        let accepted = store
            .accept_memory(memory.id, None, &[], &blobs, 20)
            .expect("accept");
        let source_of = |path: &str| {
            accepted
                .sources
                .iter()
                .find(|source| source.reference == path)
                .expect("the source")
        };
        assert_eq!(source_of("src/a.rs").blob_oid.as_deref(), Some("abc123"));
        assert!(source_of("src/a.rs").blob_read);
        assert_eq!(source_of("src/b.rs").blob_oid, None);
        assert!(source_of("src/b.rs").blob_read, "read, and absent");
        assert_eq!(source_of("src/c.rs").blob_oid, None);
        assert!(!source_of("src/c.rs").blob_read, "not read");
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
        let first = [("src/a.rs".to_owned(), BlobRead::Blob("abc123".to_owned()))];
        let accepted = store
            .accept_memory(memory.id, None, &[], &first, 20)
            .expect("accept");

        let changed = [("src/a.rs".to_owned(), BlobRead::Blob("def456".to_owned()))];
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
        let blobs = [("src/a.rs".to_owned(), BlobRead::Blob("abc123".to_owned()))];
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
        assert_eq!(superseded.supersedes, vec![old.uid.clone()]);
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
            basis: MemoryBasis::Observed,
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

    #[test]
    fn a_reason_is_trimmed_empty_or_at_most_two_hundred_characters() {
        assert_eq!(check_reason(None), Ok(None));
        assert_eq!(check_reason(Some("   ")), Ok(None));
        assert_eq!(
            check_reason(Some("  stale since the move  ")),
            Ok(Some("stale since the move".to_owned()))
        );
        let two_hundred = "é".repeat(REASON_CHARS);
        assert_eq!(check_reason(Some(&two_hundred)), Ok(Some(two_hundred)));
        let too_long = "é".repeat(REASON_CHARS + 1);
        assert!(
            check_reason(Some(&too_long)).is_err(),
            "a longer reason is refused, not cut"
        );
    }

    #[test]
    fn a_mark_is_useful_or_wrong_and_any_other_word_is_refused() {
        assert_eq!(check_mark("useful"), Ok(MemoryMark::Useful));
        assert_eq!(check_mark("wrong"), Ok(MemoryMark::Wrong));
        let refused = check_mark("helpful").expect_err("refused");
        assert!(refused.contains("useful or wrong"), "{refused}");
        assert_eq!(mark_text(MemoryMark::Wrong), "wrong");
    }

    #[test]
    fn only_an_accepted_memory_can_be_marked_and_a_repeat_is_the_latest_mark() {
        assert_eq!(check_markable(MemoryStatus::Accepted), Ok(()));
        for status in [
            MemoryStatus::Proposed,
            MemoryStatus::Rejected,
            MemoryStatus::Superseded,
        ] {
            let refused = check_markable(status).expect_err("refused");
            assert!(
                refused.starts_with("only an accepted memory can be marked"),
                "{refused}"
            );
        }
        assert!(is_repeat(Some(MemoryMark::Useful), MemoryMark::Useful));
        assert!(!is_repeat(Some(MemoryMark::Useful), MemoryMark::Wrong));
        assert!(!is_repeat(None, MemoryMark::Useful));
    }

    #[test]
    fn the_actors_whose_latest_mark_is_wrong_are_named_in_order() {
        let summary = |actor: &str, mark| MemoryMarkSummary {
            actor: actor.to_owned(),
            mark,
            at: 1,
            reason: None,
        };
        let marks = [
            summary("agent:a", MemoryMark::Wrong),
            summary("human", MemoryMark::Useful),
            summary("agent:b", MemoryMark::Wrong),
        ];
        assert_eq!(marked_wrong_by(&marks), ["agent:a", "agent:b"]);
        assert!(marked_wrong_by(&marks[1..2]).is_empty());
    }
}
