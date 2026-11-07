//! `contexts.db`: the contexts, the repositories in them and the memories attached to them
//! (design/threads.md, "Contexts"; D191, D193). It has its own schema version, the pragmas of
//! `threads.db` and the memory rules of `threads::memories`, which both stores share. A read never
//! creates the file (D193 (2)). The engine decides who acts; this store checks what the rules check.

use std::path::{Path, PathBuf};

use gm_schema::threads::{Context, ContextRepo, Memory, MemoryFilter, MemoryStatus};
use rusqlite::{Connection, OptionalExtension, params};

use super::visibility::{self, ContextNode};
use crate::threads::memories::{self, BlobRead, Carried, Found, NewMemory, Placement};
use crate::threads::store::{BUSY_TIMEOUT, StoreError, begin, create_private, invalid, retry_busy};

/// Migrations in order; `PRAGMA user_version` counts how many have run, so the first is version 1.
const MIGRATIONS: &[&str] = &[SCHEMA_V1, SCHEMA_V2];
/// Characters of a context name (D193 (3)).
const NAME_CHARS: usize = 64;

/// Schema 1 (D191, D193): contexts, their repositories, the memories of the contexts with their
/// sources, links and full-text index (the shapes and triggers of `threads.db`, schema 7), and the
/// attachments of memories to contexts.
const SCHEMA_V1: &str = "
CREATE TABLE context (
    id         INTEGER PRIMARY KEY,
    uid        TEXT UNIQUE NOT NULL,
    name       TEXT NOT NULL,
    parent_id  INTEGER REFERENCES context(id) ON DELETE RESTRICT,
    created_at INTEGER NOT NULL,
    UNIQUE(parent_id, name));
CREATE TABLE context_repo (
    context_id INTEGER NOT NULL REFERENCES context(id) ON DELETE CASCADE,
    repo_id    TEXT NOT NULL,
    path       TEXT NOT NULL,
    name       TEXT NOT NULL,
    added_at   INTEGER NOT NULL,
    PRIMARY KEY(context_id, repo_id));
CREATE INDEX context_repo_repo ON context_repo (repo_id);
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
CREATE TABLE memory_context (
    memory_id  INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
    context_id INTEGER NOT NULL REFERENCES context(id) ON DELETE CASCADE,
    PRIMARY KEY(memory_id, context_id));
CREATE INDEX memory_context_context ON memory_context (context_id);
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

/// Schema 2 (D193 (9)): a file source records whether its blob was read at acceptance. The rows
/// accepted under schema 1 were read, so they count as read; a proposal is not read yet.
const SCHEMA_V2: &str = "
ALTER TABLE memory_source ADD COLUMN blob_read INTEGER NOT NULL DEFAULT 0;
UPDATE memory_source SET blob_read = 1
    WHERE kind = 'file' AND memory_id IN (
        SELECT id FROM memory WHERE status IN ('accepted','superseded'));
";

/// The contexts store: one connection to `contexts.db`.
pub struct ContextStore {
    conn: Connection,
}

impl ContextStore {
    /// Opens `contexts.db` at `path`, creating it and its folder when missing. The file and its WAL
    /// and shared-memory files are private (mode `0600`). The pragmas are those of `threads.db`, and
    /// an open of a file another process is creating is retried for a short time.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        for file in [
            path.to_path_buf(),
            sidecar(path, "-wal"),
            sidecar(path, "-shm"),
        ] {
            create_private(&file)?;
        }
        let conn = retry_busy(|| -> Result<Connection, StoreError> {
            let mut conn = Connection::open(path)?;
            prepare(&mut conn)?;
            Ok(conn)
        })?;
        Ok(Self { conn })
    }

    /// Opens `contexts.db` when it exists, and returns `None` when it does not: a read never creates
    /// it, so with no file there are no contexts (D193 (2)).
    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        if !path.is_file() {
            return Ok(None);
        }
        Self::open(path).map(Some)
    }

    /// A store that lives only in memory (tests).
    pub fn open_memory() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()?;
        prepare(&mut conn)?;
        Ok(Self { conn })
    }

    /// Every context as the visibility rules read it: id, name and parent.
    pub fn nodes(&self) -> Result<Vec<ContextNode>, StoreError> {
        nodes_of(&self.conn)
    }

    /// Every context, with its path, repositories and counts, in id order.
    pub fn list_contexts(&self) -> Result<Vec<Context>, StoreError> {
        let nodes = nodes_of(&self.conn)?;
        nodes
            .iter()
            .map(|node| context_with(&self.conn, node.id, &nodes))
            .collect()
    }

    /// One context with its path, repositories and counts. `NotFound` when it does not exist.
    pub fn read_context(&self, id: i64) -> Result<Context, StoreError> {
        context_by_id(&self.conn, id)
    }

    /// Creates a context under `parent` (the root when `None`). The name is trimmed and has 1 to 64
    /// characters, and it is unique among its siblings: a root context is checked in code too,
    /// because SQLite's UNIQUE does not hold for two NULL parents.
    pub fn create_context(
        &mut self,
        name: &str,
        parent: Option<i64>,
        at: i64,
    ) -> Result<Context, StoreError> {
        let name = check_name(name)?;
        let tx = begin(&mut self.conn)?;
        if let Some(parent) = parent {
            context_row(&tx, parent).map_err(|error| match error {
                StoreError::NotFound => invalid("the parent context does not exist"),
                other => other,
            })?;
        }
        if sibling_named(&tx, parent, &name, None)? {
            return Err(duplicate(&name));
        }
        tx.execute(
            "INSERT INTO context (uid, name, parent_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![memories::new_uid(), name, parent, at],
        )?;
        let id = tx.last_insert_rowid();
        let context = context_by_id(&tx, id)?;
        tx.commit()?;
        Ok(context)
    }

    /// Renames a context and moves it under another parent (`Some(None)` moves it to the root). A
    /// name already used by a sibling is refused, and so is a parent that would make a cycle.
    pub fn update_context(
        &mut self,
        id: i64,
        name: Option<&str>,
        parent: Option<Option<i64>>,
    ) -> Result<Context, StoreError> {
        let tx = begin(&mut self.conn)?;
        let (current_name, current_parent) = context_row(&tx, id)?;
        let name = match name {
            Some(name) => check_name(name)?,
            None => current_name,
        };
        let parent = parent.unwrap_or(current_parent);
        if let Some(new_parent) = parent
            && !exists(&tx, new_parent)?
        {
            return Err(invalid("the parent context does not exist"));
        }
        if visibility::would_cycle(&nodes_of(&tx)?, id, parent) {
            return Err(invalid(
                "a context cannot move under itself or under one of its sub-contexts",
            ));
        }
        if sibling_named(&tx, parent, &name, Some(id))? {
            return Err(duplicate(&name));
        }
        tx.execute(
            "UPDATE context SET name = ?1, parent_id = ?2 WHERE id = ?3",
            params![name, parent, id],
        )?;
        let context = context_by_id(&tx, id)?;
        tx.commit()?;
        Ok(context)
    }

    /// Deletes a context, refused while it has sub-contexts. The memories attached to no other
    /// context belong to it alone: when there are any and `delete_memories` is false the delete is
    /// refused with their count; otherwise they are deleted with it. Returns that count. Memories
    /// attached to another context stay, with their attachment to that context.
    pub fn delete_context(&mut self, id: i64, delete_memories: bool) -> Result<u32, StoreError> {
        let tx = begin(&mut self.conn)?;
        context_row(&tx, id)?;
        let children: i64 = tx.query_row(
            "SELECT COUNT(*) FROM context WHERE parent_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 {
            return Err(invalid(
                "the context has sub-contexts; delete or move them first",
            ));
        }
        let alone = memories_only_in(&tx, id)?;
        let count = u32::try_from(alone.len()).unwrap_or(u32::MAX);
        if !alone.is_empty() && !delete_memories {
            return Err(invalid(&format!(
                "{count} memories belong to this context alone; delete them with it, or attach \
                 them to another context first"
            )));
        }
        for memory in alone {
            tx.execute("DELETE FROM memory WHERE id = ?1", params![memory])?;
        }
        tx.execute("DELETE FROM context WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(count)
    }

    /// Adds a repository to a context. Adding one that is already there is idempotent: its name is
    /// updated, and its path is set on every context the repository is in, since a clone has one
    /// path (D193 (4)).
    pub fn add_repo(
        &mut self,
        context: i64,
        repo_id: &str,
        path: &str,
        name: &str,
        at: i64,
    ) -> Result<(), StoreError> {
        if repo_id.is_empty() {
            return Err(invalid("a repository has an id"));
        }
        let tx = begin(&mut self.conn)?;
        context_row(&tx, context)?;
        tx.execute(
            "INSERT INTO context_repo (context_id, repo_id, path, name, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(context_id, repo_id) DO UPDATE SET path = excluded.path, name = excluded.name",
            params![context, repo_id, path, name, at],
        )?;
        tx.execute(
            "UPDATE context_repo SET path = ?1 WHERE repo_id = ?2",
            params![path, repo_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a repository from a context. `NotFound` when it is not there. The memories stay.
    pub fn remove_repo(&mut self, context: i64, repo_id: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let removed = tx.execute(
            "DELETE FROM context_repo WHERE context_id = ?1 AND repo_id = ?2",
            params![context, repo_id],
        )?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// The contexts a repository belongs to, by id.
    pub fn repo_contexts(&self, repo_id: &str) -> Result<Vec<i64>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT context_id FROM context_repo WHERE repo_id = ?1 ORDER BY context_id",
        )?;
        let ids = statement
            .query_map(params![repo_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// The last known path of a repository: the path it was added or last updated with.
    pub fn repo_path(&self, repo_id: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT path FROM context_repo WHERE repo_id = ?1
                 ORDER BY added_at DESC, rowid DESC LIMIT 1",
                params![repo_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Sets the path of a repository in every context it is in. `NotFound` when it is in none.
    pub fn update_repo_path(&mut self, repo_id: &str, path: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let updated = tx.execute(
            "UPDATE context_repo SET path = ?1 WHERE repo_id = ?2",
            params![path, repo_id],
        )?;
        if updated == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a repository from every context, for a new repository id (D193 (4)).
    pub fn forget_repo(&mut self, repo_id: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        tx.execute(
            "DELETE FROM context_repo WHERE repo_id = ?1",
            params![repo_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Stores a memory proposed at `contexts`, and attaches it to all of them when it is new. The
    /// duplicate key is looked for among the memories attached to any of those contexts, as in
    /// `threads.db` (D192 (2)). With a `uid` that is already stored, that memory is returned with
    /// `false` and nothing is written: a retried move finds its memory (D193 (8)). Every source must
    /// name its repository.
    pub fn propose_context_memory(
        &mut self,
        new: &NewMemory<'_>,
        contexts: &[i64],
        uid: Option<&str>,
    ) -> Result<(Memory, bool), StoreError> {
        let checked = memories::check_proposal(new)?;
        self.store_in_contexts(new, &checked, None, contexts, uid)
    }

    /// Moves a repository memory into `contexts` (D193 (8)): stored under its own uid, with its
    /// status, origin, history and sources (the caller has set each source's `repo_id`). The
    /// duplicate key is looked for as in a proposal. A memory already stored under that uid is
    /// returned with `false` and nothing is written, so a move interrupted after this write is
    /// finished by a retry.
    pub fn move_memory_in(
        &mut self,
        memory: &Memory,
        contexts: &[i64],
    ) -> Result<(Memory, bool), StoreError> {
        if !matches!(
            memory.status,
            MemoryStatus::Proposed | MemoryStatus::Accepted
        ) {
            return Err(invalid("only a proposed or accepted memory can move"));
        }
        let new = NewMemory {
            kind: memory.kind,
            text: &memory.text,
            status: memory.status,
            origin: &memory.origin,
            sources: &memory.sources,
            at: memory.created_at,
        };
        let checked = memories::check_content(&new)?;
        let carried = Carried {
            proposed_by: memory.proposed_by.as_deref(),
            model: memory.model.as_deref(),
            created_at: memory.created_at,
            decided_at: memory.decided_at,
            decided_by: memory.decided_by.as_deref(),
        };
        self.store_in_contexts(&new, &checked, Some(&carried), contexts, Some(&memory.uid))
    }

    /// Stores a checked memory attached to `contexts`, inside one transaction (D193 (7), (8)).
    fn store_in_contexts(
        &mut self,
        new: &NewMemory<'_>,
        checked: &memories::Checked,
        carried: Option<&Carried<'_>>,
        contexts: &[i64],
        uid: Option<&str>,
    ) -> Result<(Memory, bool), StoreError> {
        if contexts.is_empty() {
            return Err(invalid("a memory is proposed at a context"));
        }
        if checked
            .sources
            .iter()
            .any(|source| source.repo_id.is_none())
        {
            return Err(invalid("a source of a context memory names its repository"));
        }
        let tx = begin(&mut self.conn)?;
        for &context in contexts {
            context_row(&tx, context)?;
        }
        let among = members_of(&tx, contexts)?;
        let place = Placement {
            threads: false,
            among: Some(&among),
        };
        let (memory, created) = memories::propose_in(&tx, new, checked, &place, uid, carried)?;
        if created {
            for &context in contexts {
                attach(&tx, memory.id, context)?;
            }
        }
        tx.commit()?;
        Ok((memory, created))
    }

    /// Accepts a memory by uid (a human's action, D192 (3) and (4)); see `threads::memories`. A text
    /// edit's duplicate key is looked for among the memories that share a context with it.
    /// `supersedes` are uids of this store's memories.
    pub fn accept_memory(
        &mut self,
        uid: &str,
        text: Option<&str>,
        supersedes: &[String],
        blobs: &[(String, BlobRead)],
        at: i64,
    ) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let id = memories::id_of_uid(&tx, uid)?;
        let peers = peers_of(&tx, id)?;
        let mut old = Vec::with_capacity(supersedes.len());
        for other in supersedes {
            old.push(
                memories::id_of_uid(&tx, other).map_err(|error| match error {
                    StoreError::NotFound => invalid(&format!("memory {other} does not exist")),
                    other => other,
                })?,
            );
        }
        let place = Placement {
            threads: false,
            among: Some(&peers),
        };
        let memory = memories::accept_in(&tx, &place, id, text, &old, blobs, at)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Rejects a proposed memory by uid (a human's action).
    pub fn reject_memory(&mut self, uid: &str, at: i64) -> Result<Memory, StoreError> {
        let tx = begin(&mut self.conn)?;
        let id = memories::id_of_uid(&tx, uid)?;
        let memory = memories::reject_in(&tx, id, at)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Deletes a memory by uid with its sources and links (a human's action). `NotFound` when missing.
    pub fn delete_memory(&mut self, uid: &str) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let id = memories::id_of_uid(&tx, uid)?;
        memories::delete_in(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Attaches a memory to a context, which it then shares with the other contexts it is in.
    /// Attaching it again changes nothing.
    pub fn attach_memory(&mut self, uid: &str, context: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let id = memories::id_of_uid(&tx, uid)?;
        context_row(&tx, context)?;
        attach(&tx, id, context)?;
        tx.commit()?;
        Ok(())
    }

    /// Detaches a memory from a context. `NotFound` when it is not attached there; refused when it
    /// is the memory's last context (move or delete the memory instead).
    pub fn detach_memory(&mut self, uid: &str, context: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let id = memories::id_of_uid(&tx, uid)?;
        let contexts = contexts_of(&tx, id)?;
        if !contexts.contains(&context) {
            return Err(StoreError::NotFound);
        }
        if contexts.len() <= 1 {
            return Err(invalid(
                "a memory keeps at least one context; move or delete it instead",
            ));
        }
        tx.execute(
            "DELETE FROM memory_context WHERE memory_id = ?1 AND context_id = ?2",
            params![id, context],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The contexts a memory is attached to, by id.
    pub fn memory_contexts(&self, uid: &str) -> Result<Vec<i64>, StoreError> {
        let id = memories::id_of_uid(&self.conn, uid)?;
        contexts_of(&self.conn, id)
    }

    /// A memory by uid, with its sources and relations. `NotFound` when missing.
    pub fn read_memory_by_uid(&self, uid: &str) -> Result<Memory, StoreError> {
        memories::memory_by_uid(&self.conn, uid)
    }

    /// The memories attached to any of `contexts`, each once, with their `bm25` score (0.0 without a
    /// query), up to `limit`. With a query they are ordered by relevance, otherwise newest first.
    pub fn list_memories_in(
        &self,
        contexts: &[i64],
        query: &str,
        filter: &MemoryFilter,
        limit: u32,
    ) -> Result<Vec<(Memory, f64)>, StoreError> {
        let found = self.find_memories_in(contexts, query, filter)?;
        let mut memories = Vec::new();
        for hit in found
            .iter()
            .take(usize::try_from(limit.max(1)).unwrap_or(usize::MAX))
        {
            memories.push((memories::memory_by_id(&self.conn, hit.id)?, hit.score));
        }
        Ok(memories)
    }

    /// Every memory attached to any of `contexts` that `query` and `filter` select, each once and
    /// unpaged, in the order of the list (D193 (6)). The engine merges them with the repository's.
    pub(crate) fn find_memories_in(
        &self,
        contexts: &[i64],
        query: &str,
        filter: &MemoryFilter,
    ) -> Result<Vec<Found>, StoreError> {
        if contexts.is_empty() {
            return Ok(Vec::new());
        }
        let listed = contexts
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let within = format!(
            " AND m.id IN (SELECT memory_id FROM memory_context WHERE context_id IN ({listed}))"
        );
        memories::list_ids(&self.conn, query, filter, &within)
    }
}

/// Connection pragmas of `threads.db` (D122), then the migrations of this file.
fn prepare(conn: &mut Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "secure_delete", true)?;
    migrate(conn)
}

/// Runs the migrations the file has not seen, in one immediate transaction so two processes
/// opening a new file do not both create the schema.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = begin(conn)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let latest = MIGRATIONS.len();
    let from = usize::try_from(version)
        .ok()
        .filter(|from| *from <= latest)
        .ok_or_else(|| invalid("contexts.db was written by a newer GitMaster"))?;
    if from == latest {
        return Ok(());
    }
    for sql in &MIGRATIONS[from..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(
        None,
        "user_version",
        i64::try_from(latest).unwrap_or(i64::MAX),
    )?;
    tx.commit()?;
    Ok(())
}

/// `path` with `suffix` added to its name, the WAL and shared-memory files of SQLite.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// A context name, trimmed; it has 1 to 64 characters (D193 (3)).
fn check_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_CHARS {
        return Err(invalid("a context name has 1 to 64 characters"));
    }
    Ok(name.to_owned())
}

fn duplicate(name: &str) -> StoreError {
    invalid(&format!(
        "a context named \"{name}\" already exists under the same parent"
    ))
}

/// Whether a context other than `except` under `parent` (the root when `None`) has `name`.
fn sibling_named(
    conn: &Connection,
    parent: Option<i64>,
    name: &str,
    except: Option<i64>,
) -> Result<bool, StoreError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT id FROM context
             WHERE parent_id IS ?1 AND name = ?2 AND (?3 IS NULL OR id != ?3) LIMIT 1",
            params![parent, name, except],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn exists(conn: &Connection, id: i64) -> Result<bool, StoreError> {
    let found: Option<i64> = conn
        .query_row("SELECT id FROM context WHERE id = ?1", params![id], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(found.is_some())
}

/// The name and parent of a context, or `NotFound`.
fn context_row(conn: &Connection, id: i64) -> Result<(String, Option<i64>), StoreError> {
    conn.query_row(
        "SELECT name, parent_id FROM context WHERE id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

fn nodes_of(conn: &Connection) -> Result<Vec<ContextNode>, StoreError> {
    let mut statement = conn.prepare("SELECT id, name, parent_id FROM context ORDER BY id")?;
    let nodes = statement
        .query_map([], |row| {
            Ok(ContextNode {
                id: row.get(0)?,
                name: row.get(1)?,
                parent: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(nodes)
}

fn context_by_id(conn: &Connection, id: i64) -> Result<Context, StoreError> {
    let nodes = nodes_of(conn)?;
    context_with(conn, id, &nodes)
}

/// A context with its path (from `nodes`), repositories and counts.
fn context_with(conn: &Connection, id: i64, nodes: &[ContextNode]) -> Result<Context, StoreError> {
    let (name, parent) = context_row(conn, id)?;
    let uid: String = conn.query_row(
        "SELECT uid FROM context WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT repo_id, path, name FROM context_repo WHERE context_id = ?1 ORDER BY name, repo_id",
    )?;
    let repos = statement
        .query_map(params![id], |row| {
            Ok(ContextRepo {
                repo_id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let accepted: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_context mc JOIN memory m ON m.id = mc.memory_id
         WHERE mc.context_id = ?1 AND m.status = 'accepted'",
        params![id],
        |row| row.get(0),
    )?;
    let subs = nodes.iter().filter(|node| node.parent == Some(id)).count();
    Ok(Context {
        id,
        uid,
        name,
        parent,
        path: visibility::context_path(nodes, id),
        repos,
        accepted_memories: u32::try_from(accepted).unwrap_or(u32::MAX),
        sub_contexts: u32::try_from(subs).unwrap_or(u32::MAX),
    })
}

/// The memories attached to context `id` and to no other context, by id.
fn memories_only_in(conn: &Connection, id: i64) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT m.id FROM memory m JOIN memory_context mc ON mc.memory_id = m.id
         WHERE mc.context_id = ?1
           AND NOT EXISTS (SELECT 1 FROM memory_context o
                           WHERE o.memory_id = m.id AND o.context_id != ?1)
         ORDER BY m.id",
    )?;
    let ids = statement
        .query_map(params![id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// The contexts a memory is attached to, by id.
fn contexts_of(conn: &Connection, memory: i64) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT context_id FROM memory_context WHERE memory_id = ?1 ORDER BY context_id",
    )?;
    let ids = statement
        .query_map(params![memory], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// The memories attached to any of `contexts`, each once, by id.
fn members_of(conn: &Connection, contexts: &[i64]) -> Result<Vec<i64>, StoreError> {
    let mut ids = Vec::new();
    let mut statement =
        conn.prepare("SELECT memory_id FROM memory_context WHERE context_id = ?1")?;
    for &context in contexts {
        let found = statement
            .query_map(params![context], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.extend(found);
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// The memories that share at least one context with memory `id`, `id` included.
fn peers_of(conn: &Connection, id: i64) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT o.memory_id FROM memory_context m
         JOIN memory_context o ON o.context_id = m.context_id
         WHERE m.memory_id = ?1 ORDER BY o.memory_id",
    )?;
    let ids = statement
        .query_map(params![id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn attach(conn: &Connection, memory: i64, context: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO memory_context (memory_id, context_id) VALUES (?1, ?2)",
        params![memory, context],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::threads::{MemoryKind, MemorySource, MemorySourceKind, MemoryStatus};

    const REPO: &str = "0123456789abcdef0123456789abcdef";

    fn file(path: &str, repo: Option<&str>) -> MemorySource {
        MemorySource {
            kind: MemorySourceKind::File,
            reference: path.to_owned(),
            repo_id: repo.map(str::to_owned),
            entry_from: None,
            entry_to: None,
            blob_oid: None,
            blob_read: false,
        }
    }

    /// A rule an agent proposes from `REPO`.
    fn proposal<'a>(text: &'a str, sources: &'a [MemorySource]) -> NewMemory<'a> {
        NewMemory {
            kind: MemoryKind::Rule,
            text,
            status: MemoryStatus::Proposed,
            origin: "agent:x",
            sources,
            at: 10,
        }
    }

    fn store() -> ContextStore {
        ContextStore::open_memory().expect("open")
    }

    fn root(store: &mut ContextStore, name: &str) -> i64 {
        store.create_context(name, None, 1).expect("create").id
    }

    /// A memory proposed at `contexts` from `REPO`, with `uid` when given.
    fn propose(
        store: &mut ContextStore,
        text: &str,
        contexts: &[i64],
        uid: Option<&str>,
    ) -> (Memory, bool) {
        let sources = [file("src/lib.rs", Some(REPO))];
        store
            .propose_context_memory(&proposal(text, &sources), contexts, uid)
            .expect("propose")
    }

    fn all(store: &ContextStore, contexts: &[i64]) -> Vec<Memory> {
        store
            .list_memories_in(contexts, "", &MemoryFilter::default(), 50)
            .expect("list")
            .into_iter()
            .map(|(memory, _)| memory)
            .collect()
    }

    #[test]
    fn the_file_is_migrated_to_the_latest_version_and_reopening_keeps_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("contexts.db");
        for _ in 0..2 {
            let store = ContextStore::open(&path).expect("open");
            let version: i64 = store
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("version");
            assert_eq!(version, 2);
        }
    }

    /// Schema 2 (D193 (9)) from a version 1 file: the file sources of accepted and superseded
    /// memories were read, a proposal's was not, and a commit source is not a file.
    #[test]
    fn version_one_gains_blob_read_and_marks_the_accepted_file_sources_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("contexts.db");
        {
            let conn = Connection::open(&path).expect("raw");
            conn.execute_batch(SCHEMA_V1).expect("schema 1");
            conn.execute_batch(
                "INSERT INTO memory (id, uid, kind, text, status, origin, created_at) VALUES
                    (1, 'accepted1', 'rule', 'a', 'accepted', 'human', 1),
                    (2, 'proposed1', 'rule', 'p', 'proposed', 'agent:x', 1),
                    (3, 'superseded1', 'rule', 's', 'superseded', 'human', 1);
                 INSERT INTO memory_source (memory_id, kind, ref, repo_id, blob_oid) VALUES
                    (1, 'file', 'src/a.rs', NULL, 'abc'),
                    (1, 'commit', '0123456', NULL, NULL),
                    (2, 'file', 'src/b.rs', NULL, NULL),
                    (3, 'file', 'src/c.rs', NULL, 'def');",
            )
            .expect("rows");
            conn.pragma_update(None, "user_version", 1)
                .expect("version");
        }
        let store = ContextStore::open(&path).expect("migrate");
        let mut statement = store
            .conn
            .prepare(
                "SELECT m.uid, s.ref, s.blob_read FROM memory_source s
                 JOIN memory m ON m.id = s.memory_id ORDER BY m.uid, s.kind, s.ref",
            )
            .expect("prepare");
        let rows: Vec<(String, String, bool)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                ("accepted1".to_owned(), "0123456".to_owned(), false),
                ("accepted1".to_owned(), "src/a.rs".to_owned(), true),
                ("proposed1".to_owned(), "src/b.rs".to_owned(), false),
                ("superseded1".to_owned(), "src/c.rs".to_owned(), true),
            ]
        );
    }

    #[test]
    fn the_file_is_private_and_a_missing_one_is_not_created_by_a_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("app");
        let path = folder.join("contexts.db");
        assert!(
            ContextStore::open_existing(&path).expect("read").is_none(),
            "no file, no contexts"
        );
        assert!(!folder.exists(), "a read creates neither file nor folder");
        drop(ContextStore::open(&path).expect("open"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(
            ContextStore::open_existing(&path).expect("read").is_some(),
            "once written, it opens"
        );
    }

    #[test]
    fn two_handles_on_one_file_can_both_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("contexts.db");
        let mut first = ContextStore::open(&path).expect("first");
        let mut second = ContextStore::open(&path).expect("second");
        first
            .create_context("Platform", None, 1)
            .expect("first write");
        second
            .create_context("Partners", None, 2)
            .expect("second write after the first commit");
        assert_eq!(first.list_contexts().expect("list").len(), 2);
    }

    #[test]
    fn a_sibling_name_is_unique_and_a_root_name_too() {
        let mut store = store();
        let platform = root(&mut store, "Platform");
        assert!(
            store.create_context(" Platform ", None, 2).is_err(),
            "a trimmed duplicate at the root is refused"
        );
        store
            .create_context("Billing", Some(platform), 2)
            .expect("child");
        assert!(store.create_context("Billing", Some(platform), 3).is_err());
        assert!(
            store.create_context("Billing", None, 3).is_ok(),
            "the same name under another parent is fine"
        );
        let partners = root(&mut store, "Partners");
        assert!(
            store
                .update_context(partners, Some("Platform"), None)
                .is_err(),
            "a rename to a root sibling's name is refused"
        );
        assert!(matches!(
            store.create_context("X", Some(999), 1),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.create_context("   ", None, 1).is_err());
        assert!(store.create_context(&"x".repeat(65), None, 1).is_err());
        assert!(store.create_context(&"x".repeat(64), None, 1).is_ok());
    }

    #[test]
    fn update_refuses_a_cycle_and_moves_and_renames_a_context() {
        let mut store = store();
        let platform = root(&mut store, "Platform");
        let billing = store
            .create_context("Billing", Some(platform), 2)
            .expect("billing")
            .id;
        let invoices = store
            .create_context("Invoices", Some(billing), 3)
            .expect("invoices")
            .id;
        assert!(
            store
                .update_context(platform, None, Some(Some(invoices)))
                .is_err(),
            "a grandchild cannot become the parent"
        );
        assert!(
            store
                .update_context(platform, None, Some(Some(platform)))
                .is_err(),
            "a context cannot be its own parent"
        );
        let moved = store
            .update_context(billing, Some("Invoicing"), Some(None))
            .expect("move and rename");
        assert_eq!(moved.parent, None);
        assert_eq!(moved.name, "Invoicing");
        assert_eq!(moved.path, "Invoicing");
        assert!(matches!(
            store.update_context(999, Some("X"), None),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn delete_is_refused_while_there_are_sub_contexts() {
        let mut store = store();
        let platform = root(&mut store, "Platform");
        let billing = store
            .create_context("Billing", Some(platform), 2)
            .expect("billing")
            .id;
        assert!(store.delete_context(platform, false).is_err());
        assert_eq!(store.delete_context(billing, false).expect("leaf"), 0);
        assert_eq!(store.delete_context(platform, false).expect("now"), 0);
    }

    #[test]
    fn delete_counts_the_memories_of_the_context_alone_and_keeps_shared_ones() {
        let mut store = store();
        let platform = root(&mut store, "Platform");
        let billing = store
            .create_context("Billing", Some(platform), 2)
            .expect("billing")
            .id;
        let (alone, _) = propose(&mut store, "only in billing", &[billing], None);
        let (shared, _) = propose(&mut store, "in billing and platform", &[billing], None);
        store
            .attach_memory(&shared.uid, platform)
            .expect("attach to platform");
        let refused = store.delete_context(billing, false).expect_err("refused");
        assert!(
            matches!(&refused, StoreError::Invalid(message) if message.contains("1 memories")),
            "{refused}"
        );
        assert_eq!(store.delete_context(billing, true).expect("deleted"), 1);
        assert!(matches!(
            store.read_memory_by_uid(&alone.uid),
            Err(StoreError::NotFound)
        ));
        assert_eq!(
            store.memory_contexts(&shared.uid).expect("kept"),
            vec![platform]
        );
    }

    #[test]
    fn add_repo_is_idempotent_and_a_clone_has_one_path() {
        let mut store = store();
        let a = root(&mut store, "A");
        let b = root(&mut store, "B");
        store.add_repo(a, REPO, "/work/api", "api", 1).expect("add");
        store
            .add_repo(a, REPO, "/work/api-moved", "api-2", 2)
            .expect("again");
        let contexts = store.list_contexts().expect("list");
        let repos: Vec<_> = contexts
            .iter()
            .flat_map(|context| context.repos.iter())
            .collect();
        assert_eq!(repos.len(), 1, "the second add updated the first row");
        assert_eq!(repos[0].path, "/work/api-moved");
        assert_eq!(repos[0].name, "api-2");
        store
            .add_repo(b, REPO, "/work/api-moved", "api", 3)
            .expect("join b");
        assert_eq!(store.repo_contexts(REPO).expect("contexts"), vec![a, b]);
        assert_eq!(
            store.repo_path(REPO).expect("path").as_deref(),
            Some("/work/api-moved")
        );
        store
            .update_repo_path(REPO, "/elsewhere")
            .expect("moved clone");
        assert_eq!(
            store.repo_path(REPO).expect("path").as_deref(),
            Some("/elsewhere")
        );
        store.remove_repo(a, REPO).expect("remove");
        assert!(matches!(
            store.remove_repo(a, REPO),
            Err(StoreError::NotFound)
        ));
        store.forget_repo(REPO).expect("forget");
        assert!(store.repo_contexts(REPO).expect("contexts").is_empty());
        assert_eq!(store.repo_path(REPO).expect("path"), None);
        assert!(matches!(
            store.update_repo_path(REPO, "/x"),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn a_proposal_needs_a_context_a_known_one_and_a_repository_per_source() {
        let mut store = store();
        let a = root(&mut store, "A");
        let sources = [file("src/lib.rs", Some(REPO))];
        let new = proposal("rule", &sources);
        assert!(store.propose_context_memory(&new, &[], None).is_err());
        assert!(store.propose_context_memory(&new, &[999], None).is_err());
        let unnamed = [file("src/lib.rs", None)];
        assert!(
            store
                .propose_context_memory(&proposal("rule", &unnamed), &[a], None)
                .is_err(),
            "a source without its repository is refused"
        );
        let (memory, created) = store
            .propose_context_memory(&new, &[a], None)
            .expect("propose");
        assert!(created);
        assert_eq!(memory.status, MemoryStatus::Proposed);
        assert_eq!(
            store.memory_contexts(&memory.uid).expect("contexts"),
            vec![a]
        );
    }

    #[test]
    fn a_duplicate_is_looked_for_among_the_memories_of_the_given_contexts_only() {
        let mut store = store();
        let a = root(&mut store, "A");
        let b = root(&mut store, "B");
        let (first, created) = propose(&mut store, "Use the cache", &[a], None);
        assert!(created);
        let (again, created) = propose(&mut store, "use  the CACHE", &[a, b], None);
        assert!(!created);
        assert_eq!(
            again.uid, first.uid,
            "the same text in a shared context is found"
        );
        let (other, created) = propose(&mut store, "use the cache", &[b], None);
        assert!(created, "b holds no such memory, so this one is new");
        assert_ne!(other.uid, first.uid);
    }

    #[test]
    fn a_proposal_with_a_stored_uid_returns_that_memory_and_writes_nothing() {
        let mut store = store();
        let a = root(&mut store, "A");
        let uid = "abcdef0123456789";
        let (memory, created) = propose(&mut store, "Use the cache", &[a], Some(uid));
        assert!(created);
        assert_eq!(memory.uid, uid);
        let (again, created) = propose(&mut store, "a different text", &[a], Some(uid));
        assert!(!created, "the uid is already stored");
        assert_eq!(again.text, "Use the cache");
        assert_eq!(all(&store, &[a]).len(), 1, "nothing was written");
    }

    #[test]
    fn attach_is_idempotent_and_the_last_context_cannot_be_detached() {
        let mut store = store();
        let a = root(&mut store, "A");
        let b = root(&mut store, "B");
        let (memory, _) = propose(&mut store, "Shared rule", &[a], None);
        store.attach_memory(&memory.uid, b).expect("attach");
        store.attach_memory(&memory.uid, b).expect("again");
        assert_eq!(
            store.memory_contexts(&memory.uid).expect("contexts"),
            vec![a, b]
        );
        store.detach_memory(&memory.uid, b).expect("detach b");
        assert!(matches!(
            store.detach_memory(&memory.uid, a),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.detach_memory(&memory.uid, b),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn list_memories_in_returns_each_memory_once_and_filters_by_kind_and_status() {
        let mut store = store();
        let a = root(&mut store, "A");
        let b = root(&mut store, "B");
        let (both, _) = propose(&mut store, "alpha rule", &[a, b], None);
        let lesson_sources = [file("src/x.rs", Some(REPO))];
        let lesson = NewMemory {
            kind: MemoryKind::Lesson,
            ..proposal("beta lesson", &lesson_sources)
        };
        let (only_b, _) = store
            .propose_context_memory(&lesson, &[b], None)
            .expect("lesson");
        let human_sources = [file("src/y.rs", Some(REPO))];
        let accepted = NewMemory {
            status: MemoryStatus::Accepted,
            origin: "human",
            ..proposal("gamma rule", &human_sources)
        };
        let (only_a, _) = store
            .propose_context_memory(&accepted, &[a], None)
            .expect("accepted");

        let in_both = ids(&all(&store, &[a, b]));
        assert_eq!(in_both.len(), 3, "each memory once: {in_both:?}");
        let rules = MemoryFilter {
            kind: Some(MemoryKind::Rule),
            ..MemoryFilter::default()
        };
        let rule_ids = store
            .list_memories_in(&[a, b], "", &rules, 50)
            .expect("rules")
            .into_iter()
            .map(|(memory, _)| memory.id)
            .collect::<Vec<_>>();
        assert_eq!(rule_ids.len(), 2);
        assert!(rule_ids.contains(&both.id) && rule_ids.contains(&only_a.id));
        let accepted_only = MemoryFilter {
            status: Some(MemoryStatus::Accepted),
            ..MemoryFilter::default()
        };
        let accepted_ids = store
            .list_memories_in(&[a, b], "", &accepted_only, 50)
            .expect("accepted")
            .into_iter()
            .map(|(memory, _)| memory.id)
            .collect::<Vec<_>>();
        assert_eq!(accepted_ids, vec![only_a.id]);
        let b_ids = ids(&all(&store, &[b]));
        assert_eq!(b_ids.len(), 2, "b holds the lesson and the shared rule");
        assert!(b_ids.contains(&only_b.id) && b_ids.contains(&both.id));
        let found = store
            .list_memories_in(&[a, b], "rule", &MemoryFilter::default(), 50)
            .expect("query");
        let found_ids: Vec<i64> = found.iter().map(|(memory, _)| memory.id).collect();
        assert!(found_ids.contains(&both.id) && found_ids.contains(&only_a.id));
        assert!(!found_ids.contains(&only_b.id), "the lesson does not match");
        assert!(
            store
                .list_memories_in(&[], "", &MemoryFilter::default(), 50)
                .expect("none")
                .is_empty()
        );
        let counts = store.list_contexts().expect("list");
        let a_counts = counts.iter().find(|context| context.id == a).expect("a");
        assert_eq!(a_counts.accepted_memories, 1);
    }

    fn ids(memories: &[Memory]) -> Vec<i64> {
        memories.iter().map(|memory| memory.id).collect()
    }

    #[test]
    fn a_move_interrupted_after_its_insert_is_finished_by_a_retry_with_one_copy() {
        let mut store = store();
        let a = root(&mut store, "A");
        let stored = Memory {
            id: 7,
            uid: "abcdef0123456789".into(),
            kind: MemoryKind::Rule,
            text: "Use the cache".into(),
            status: MemoryStatus::Accepted,
            origin: "human".into(),
            proposed_by: Some("agent:x".into()),
            model: Some("model-1".into()),
            created_at: 5,
            decided_at: Some(9),
            decided_by: Some("human".into()),
            reviewed: true,
            sources: vec![MemorySource {
                blob_oid: Some("b1".into()),
                blob_read: true,
                ..file("src/lib.rs", Some(REPO))
            }],
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            files_changed_since: Vec::new(),
            scope: String::new(),
        };
        // The crash: the insert into contexts.db happens, the delete from threads.db does not.
        let (first, created) = store.move_memory_in(&stored, &[a]).expect("insert");
        assert!(created);
        // The retry finds the uid and writes nothing new.
        let (again, created) = store.move_memory_in(&stored, &[a]).expect("retry");
        assert!(!created, "the copy is already there");
        assert_eq!(again.uid, stored.uid);
        assert_eq!(all(&store, &[a]).len(), 1, "one copy");
        assert_eq!(first.created_at, 5, "the creation time is kept");
        assert_eq!(first.decided_at, Some(9));
        assert_eq!(first.decided_by.as_deref(), Some("human"));
        assert_eq!(first.proposed_by.as_deref(), Some("agent:x"));
        assert_eq!(first.model.as_deref(), Some("model-1"));
        assert_eq!(first.origin, "human");
        assert_eq!(first.sources[0].blob_oid.as_deref(), Some("b1"));
        assert_eq!(first.sources[0].repo_id.as_deref(), Some(REPO));
        let superseded = Memory {
            status: MemoryStatus::Superseded,
            ..stored
        };
        assert!(
            store.move_memory_in(&superseded, &[a]).is_err(),
            "a superseded memory does not move"
        );
    }

    #[test]
    fn an_accepted_memory_accepted_again_keeps_its_decision_and_an_edit_to_a_held_key_is_refused() {
        let mut store = store();
        let a = root(&mut store, "A");
        let (first, _) = propose(&mut store, "one", &[a], None);
        let (second, _) = propose(&mut store, "two", &[a], None);
        let accepted = store
            .accept_memory(&first.uid, None, &[], &[], 20)
            .expect("accept");
        assert_eq!(accepted.status, MemoryStatus::Accepted);
        let again = store
            .accept_memory(&first.uid, None, &[], &[], 30)
            .expect("again");
        assert_eq!(again.decided_at, Some(20), "the first decision stays");
        let held = store.accept_memory(&second.uid, Some("ONE"), &[], &[], 40);
        assert!(
            matches!(&held, Err(StoreError::Invalid(message)) if message.contains(&format!("memory {}", first.id))),
            "{held:?}"
        );
        let rejected = store.reject_memory(&second.uid, 50).expect("reject");
        assert_eq!(rejected.status, MemoryStatus::Rejected);
        store.delete_memory(&second.uid).expect("delete");
        assert!(matches!(
            store.delete_memory(&second.uid),
            Err(StoreError::NotFound)
        ));
    }
}
