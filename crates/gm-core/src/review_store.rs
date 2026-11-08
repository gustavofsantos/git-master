//! Where review comments live (B4, D96, D97): a SQLite file in the Git common
//! directory, `.git/gitmaster/review.db`, beside the workflow store. The anchor
//! is stored as JSON. The location is not stored: it is computed on read, from
//! the file as it is now (see `review`).
//!
//! A reply is a row of the same table with a `parent`: one id names a comment or
//! a reply, and a reply belongs to a root comment (one level deep). A reply copies
//! its root's path and anchor and has status `open`; only its `body`, `author` and
//! `created_at` are its own. Every write to a comment names a root.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use gm_schema::{ReviewAnchor, ReviewComment, ReviewReply, ReviewStatus};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Schema version written to `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 4;
/// How long a writer waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

const SCHEMA: &str = "
CREATE TABLE comments (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    anchor_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    snapshot TEXT,
    summary TEXT,
    after_snapshot TEXT
);
";

/// Version 1 to 2 (D135): the agent's summary of a handled comment.
const V2: &str = "ALTER TABLE comments ADD COLUMN summary TEXT;";

/// Version 2 to 3 (D140): the snapshot taken when an agent marks a comment handled.
const V3: &str = "ALTER TABLE comments ADD COLUMN after_snapshot TEXT;";

/// Version 3 to 4: replies. A reply is a row with a `parent` and an `author`. Each
/// summary becomes a reply by `agent`, timed at the resolve time when there is one (the
/// handled time was not stored) and at the creation time otherwise. Then the column goes.
const V4: &str = "
ALTER TABLE comments ADD COLUMN parent INTEGER REFERENCES comments(id) ON DELETE CASCADE;
ALTER TABLE comments ADD COLUMN author TEXT;
INSERT INTO comments (path, body, status, anchor_json, created_at, parent, author)
    SELECT path, summary, 'open', anchor_json, COALESCE(resolved_at, created_at), id, 'agent'
    FROM comments WHERE summary IS NOT NULL;
ALTER TABLE comments DROP COLUMN summary;
";

/// The upgrades after the first version, in order. Each runs when the database is at its
/// `from` version, so a database several versions behind runs every step after it.
const UPGRADES: [(i64, &str); 3] = [(1, V2), (2, V3), (3, V4)];

/// Columns read back for a root `ReviewComment`, in the order `raw_comment` expects.
const COLUMNS: &str = "id, path, body, status, anchor_json, created_at, resolved_at, snapshot, \
     after_snapshot";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

/// What a reply did: refused when the root is resolved, or added, and whether it
/// reopened a handled root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Resolved,
    Added { reopened: bool },
}

/// One root row as SQLite gives it: id, path, body, status, anchor JSON, created,
/// resolved, snapshot and the "after" snapshot.
type RawComment = (
    i64,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
);

pub struct ReviewStore {
    conn: Connection,
}

impl ReviewStore {
    /// Opens (creating it and its parent directory when missing) and migrates
    /// the database at `path`. A file that is not this database, or one written
    /// by a newer `GitMaster`, is an error and is left as it is.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError(format!("cannot create {}: {e}", parent.display())))?;
        }
        let mut conn = Connection::open(path)
            .map_err(|e| StoreError(format!("cannot open {}: {e}", path.display())))?;
        Self::prepare(&mut conn)
            .map_err(|e| StoreError(format!("cannot open {}: {}", path.display(), e.0)))?;
        Ok(Self { conn })
    }

    /// A database that lives only in memory (tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory().map_err(StoreError::from)?;
        Self::prepare(&mut conn)?;
        Ok(Self { conn })
    }

    fn prepare(conn: &mut Connection) -> Result<(), StoreError> {
        conn.busy_timeout(BUSY_TIMEOUT).map_err(StoreError::from)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::from)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(StoreError::from)?;
        migrate(conn)
    }

    /// Stores a new open comment on `path`, anchored at `anchor`, and returns its
    /// id. The snapshot is set later, once the row exists.
    pub fn add(
        &self,
        path: &str,
        body: &str,
        anchor: &ReviewAnchor,
        now: i64,
    ) -> Result<i64, StoreError> {
        let anchor_json = to_json(anchor)?;
        self.conn.execute(
            "INSERT INTO comments (path, body, status, anchor_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                path,
                body,
                status_text(ReviewStatus::Open),
                anchor_json,
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Every root comment, by path, then by the start line of its anchor, then by id,
    /// each with its replies, oldest first. `location` is `None` and `outdated` false:
    /// the caller computes them.
    pub fn list(&self) -> Result<Vec<ReviewComment>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM comments WHERE parent IS NULL
             ORDER BY path, json_extract(anchor_json, '$.start_line'), id"
        ))?;
        let rows = statement.query_map([], raw_comment)?;
        let mut comments: Vec<ReviewComment> =
            rows.map(|row| comment_of(row?)).collect::<Result<_, _>>()?;
        let mut replies = self.replies_by_root()?;
        for comment in &mut comments {
            comment.replies = replies.remove(&comment.id).unwrap_or_default();
        }
        Ok(comments)
    }

    /// The root comment `id`, with its replies; `None` for a reply or an unknown id.
    pub fn get(&self, id: i64) -> Result<Option<ReviewComment>, StoreError> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM comments WHERE id = ?1 AND parent IS NULL"),
                params![id],
                raw_comment,
            )
            .optional()?;
        let Some(raw) = row else {
            return Ok(None);
        };
        let mut comment = comment_of(raw)?;
        comment.replies = self.replies_of(id)?;
        Ok(Some(comment))
    }

    /// The root comment that `id` belongs to: `id` itself for a root, its parent for a
    /// reply. `None` when there is no such row.
    pub fn root_of(&self, id: i64) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT COALESCE(parent, id) FROM comments WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set_body(&self, id: i64, body: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET body = ?2 WHERE id = ?1 AND parent IS NULL",
                params![id, body],
            )?,
            id,
        )
    }

    /// Sets the status. `resolved_at` is `now` when the comment becomes resolved
    /// and cleared otherwise; the "after" snapshot is cleared when it is reopened
    /// (D140). The replies stay.
    pub fn set_status(&self, id: i64, status: ReviewStatus, now: i64) -> Result<(), StoreError> {
        let resolved_at = (status == ReviewStatus::Resolved).then_some(now);
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET status = ?2, resolved_at = ?3,
                 after_snapshot = CASE WHEN ?2 = 'open' THEN NULL ELSE after_snapshot END
                 WHERE id = ?1 AND parent IS NULL",
                params![id, status_text(status), resolved_at],
            )?,
            id,
        )
    }

    /// Marks the root comment handled, and adds `body` as a reply by `author` (D135), in
    /// one transaction. A comment the person resolved stays resolved, even when they
    /// resolved it a moment ago in another process; the check and the write are one
    /// statement. Any earlier "after" snapshot is dropped: the caller records the new
    /// one (D140). Returns whether it changed.
    pub fn handle(&self, id: i64, author: &str, body: &str, now: i64) -> Result<bool, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let rows = tx.execute(
            "UPDATE comments SET status = ?2, resolved_at = NULL, after_snapshot = NULL
             WHERE id = ?1 AND parent IS NULL AND status != ?3",
            params![
                id,
                status_text(ReviewStatus::Handled),
                status_text(ReviewStatus::Resolved)
            ],
        )?;
        if rows == 0 {
            let exists: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM comments WHERE id = ?1 AND parent IS NULL)",
                params![id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(StoreError(format!("no review comment {id}")));
            }
            return Ok(false);
        }
        insert_reply(&tx, id, author, body, now)?;
        tx.commit()?;
        Ok(true)
    }

    /// Adds `body` as a reply by `author` to the root comment `root`, in one
    /// transaction. A resolved root takes no reply. With `reopen`, a handled root
    /// becomes open again and its "after" snapshot is dropped, as a reopen does (D140).
    pub fn reply(
        &self,
        root: i64,
        author: &str,
        body: &str,
        now: i64,
        reopen: bool,
    ) -> Result<Reply, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let status: String = tx
            .query_row(
                "SELECT status FROM comments WHERE id = ?1 AND parent IS NULL",
                params![root],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError(format!("no review comment {root}")))?;
        if status == status_text(ReviewStatus::Resolved) {
            return Ok(Reply::Resolved);
        }
        insert_reply(&tx, root, author, body, now)?;
        let reopened = reopen && status == status_text(ReviewStatus::Handled);
        if reopened {
            tx.execute(
                "UPDATE comments SET status = ?2, resolved_at = NULL, after_snapshot = NULL
                 WHERE id = ?1",
                params![root, status_text(ReviewStatus::Open)],
            )?;
        }
        tx.commit()?;
        Ok(Reply::Added { reopened })
    }

    /// Records the snapshot object id taken when the comment was made.
    pub fn set_snapshot(&self, id: i64, oid: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET snapshot = ?2 WHERE id = ?1 AND parent IS NULL",
                params![id, oid],
            )?,
            id,
        )
    }

    /// Records the snapshot object id taken when an agent marked the comment
    /// handled (D140), in place of any earlier one.
    pub fn set_after_snapshot(&self, id: i64, oid: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET after_snapshot = ?2 WHERE id = ?1 AND parent IS NULL",
                params![id, oid],
            )?,
            id,
        )
    }

    /// Deletes the root comment and, with it, its replies.
    pub fn delete(&self, id: i64) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "DELETE FROM comments WHERE id = ?1 AND parent IS NULL",
                params![id],
            )?,
            id,
        )
    }

    /// The replies of every root, grouped by root and oldest first (by creation, then id).
    fn replies_by_root(&self) -> Result<BTreeMap<i64, Vec<ReviewReply>>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT parent, id, author, body, created_at FROM comments
             WHERE parent IS NOT NULL ORDER BY parent, created_at, id",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, reply_of(row)?)))?;
        let mut by_root: BTreeMap<i64, Vec<ReviewReply>> = BTreeMap::new();
        for row in rows {
            let (root, reply) = row?;
            by_root.entry(root).or_default().push(reply);
        }
        Ok(by_root)
    }

    /// The replies of root `root`, oldest first.
    fn replies_of(&self, root: i64) -> Result<Vec<ReviewReply>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT parent, id, author, body, created_at FROM comments
             WHERE parent = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![root], reply_of)?;
        rows.map(|row| row.map_err(StoreError::from)).collect()
    }

    /// Refuses a write that touched no row: the comment is not there.
    fn changed(rows: usize, id: i64) -> Result<(), StoreError> {
        if rows == 0 {
            Err(StoreError(format!("no review comment {id}")))
        } else {
            Ok(())
        }
    }
}

/// Inserts a reply to `root`, anchored where the root is: a copy of its path and anchor.
fn insert_reply(
    conn: &Connection,
    root: i64,
    author: &str,
    body: &str,
    now: i64,
) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO comments (path, body, status, anchor_json, created_at, parent, author)
         SELECT path, ?2, ?3, anchor_json, ?4, id, ?5 FROM comments WHERE id = ?1",
        params![root, body, status_text(ReviewStatus::Open), now, author],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Creates the schema of an empty database, or brings an older one up to date; refuses one
/// written by a newer version of the app. Several processes can open the same file at once
/// (the app and any number of `gitmaster mcp`), so the version is read inside a write
/// transaction: the second to arrive waits, sees the finished work and changes nothing.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match version {
        0 => {
            tx.execute_batch(SCHEMA)?;
            version = 3;
        }
        1..=3 => {}
        v if v == SCHEMA_VERSION => return Ok(()),
        v if v > SCHEMA_VERSION => {
            return Err(StoreError(
                "review.db was written by a newer GitMaster".to_owned(),
            ));
        }
        v => {
            return Err(StoreError(format!(
                "review.db has unknown schema version {v}"
            )));
        }
    }
    for (from, sql) in UPGRADES {
        if version == from {
            tx.execute_batch(sql)?;
            version = from + 1;
        }
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit().map_err(StoreError::from)
}

fn status_text(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Open => "open",
        ReviewStatus::Handled => "handled",
        ReviewStatus::Resolved => "resolved",
    }
}

fn parse_status(text: &str) -> Result<ReviewStatus, StoreError> {
    match text {
        "open" => Ok(ReviewStatus::Open),
        "handled" => Ok(ReviewStatus::Handled),
        "resolved" => Ok(ReviewStatus::Resolved),
        other => Err(StoreError(format!("unknown review status {other:?}"))),
    }
}

fn raw_comment(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawComment> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

/// A reply from a row selected as `parent, id, author, body, created_at`.
fn reply_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewReply> {
    Ok(ReviewReply {
        id: row.get(1)?,
        author: row.get(2)?,
        body: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn comment_of(raw: RawComment) -> Result<ReviewComment, StoreError> {
    let (id, path, body, status, anchor_json, created_at, resolved_at, snapshot, after_snapshot) =
        raw;
    Ok(ReviewComment {
        id,
        path,
        body,
        status: parse_status(&status)?,
        anchor: serde_json::from_str(&anchor_json)
            .map_err(|e| StoreError(format!("cannot decode the anchor of comment {id}: {e}")))?,
        created_at,
        resolved_at,
        snapshot,
        after_snapshot,
        location: None,
        outdated: false,
        replies: Vec::new(),
    })
}

fn to_json(anchor: &ReviewAnchor) -> Result<String, StoreError> {
    serde_json::to_string(anchor).map_err(|e| StoreError(format!("cannot encode JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(text: &str, start_line: u32) -> ReviewAnchor {
        ReviewAnchor {
            text: text.to_owned(),
            before: vec!["above".to_owned()],
            after: Vec::new(),
            start_line,
        }
    }

    fn user_version(store: &ReviewStore) -> i64 {
        store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version")
    }

    #[test]
    fn a_version_one_database_gains_the_summary_column() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "CREATE TABLE comments (id INTEGER PRIMARY KEY, path TEXT NOT NULL,
                 body TEXT NOT NULL, status TEXT NOT NULL, anchor_json TEXT NOT NULL,
                 created_at INTEGER NOT NULL, resolved_at INTEGER, snapshot TEXT);
                 PRAGMA user_version = 1;",
            )
            .expect("v1");
        }
        let store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        let id = store.add("a", "b", &anchor("x", 1), 1).expect("add");
        assert!(store.handle(id, "agent:test", "done", 2).expect("handled"));
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.status, ReviewStatus::Handled);
        assert_eq!(comment.replies.len(), 1);
        assert_eq!(comment.replies[0].body, "done");
    }

    #[test]
    fn a_version_two_database_gains_the_after_snapshot_column() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "CREATE TABLE comments (id INTEGER PRIMARY KEY, path TEXT NOT NULL,
                 body TEXT NOT NULL, status TEXT NOT NULL, anchor_json TEXT NOT NULL,
                 created_at INTEGER NOT NULL, resolved_at INTEGER, snapshot TEXT,
                 summary TEXT);
                 INSERT INTO comments (path, body, status, anchor_json, created_at, summary)
                 VALUES ('a', 'b', 'handled', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 1, 'done');
                 PRAGMA user_version = 2;",
            )
            .expect("v2");
        }
        let store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        let kept = store.list().expect("list").remove(0);
        assert_eq!(kept.replies.len(), 1);
        assert_eq!(kept.replies[0].body, "done");
        assert_eq!(kept.after_snapshot, None);
        store
            .set_after_snapshot(kept.id, &"d".repeat(40))
            .expect("after");
        let comment = store.get(kept.id).expect("get").expect("found");
        assert_eq!(comment.after_snapshot, Some("d".repeat(40)));
    }

    #[test]
    fn reopening_drops_the_after_snapshot_and_resolving_keeps_it() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store.add("a", "b", &anchor("x", 1), 1).expect("add");
        assert!(store.handle(id, "agent:t", "done", 2).expect("handled"));
        store.set_after_snapshot(id, "first").expect("after");
        assert!(
            store
                .handle(id, "agent:t", "again", 3)
                .expect("handled again")
        );
        assert_eq!(
            store.get(id).expect("get").expect("found").after_snapshot,
            None,
            "handling again drops the old snapshot"
        );
        store.set_after_snapshot(id, "second").expect("again");
        assert_eq!(
            store.get(id).expect("get").expect("found").after_snapshot,
            Some("second".to_owned())
        );
        store
            .set_status(id, ReviewStatus::Resolved, 4)
            .expect("resolve");
        assert_eq!(
            store.get(id).expect("get").expect("found").after_snapshot,
            Some("second".to_owned())
        );
        store.set_status(id, ReviewStatus::Open, 5).expect("reopen");
        let reopened = store.get(id).expect("get").expect("found");
        assert_eq!(reopened.after_snapshot, None);
        assert_eq!(reopened.replies.len(), 2, "reopening keeps the replies");
        assert_eq!(
            store.set_after_snapshot(99, "x").expect_err("refused").0,
            "no review comment 99"
        );
    }

    #[test]
    fn a_resolved_comment_is_not_marked_handled_and_takes_no_reply() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store.add("a", "b", &anchor("x", 1), 1).expect("add");
        store
            .set_status(id, ReviewStatus::Resolved, 2)
            .expect("resolve");
        assert!(!store.handle(id, "agent:t", "late", 3).expect("handled"));
        assert_eq!(
            store.reply(id, "human", "why", 4, true).expect("reply"),
            Reply::Resolved
        );
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.status, ReviewStatus::Resolved);
        assert!(comment.replies.is_empty());
        assert!(store.handle(99, "agent:t", "x", 5).is_err());
    }

    #[test]
    fn a_fresh_database_is_created_at_the_current_version() {
        let store = ReviewStore::open_in_memory().expect("open");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
    }

    #[test]
    fn a_comment_round_trips_with_its_anchor_and_status() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store
            .add("src/a.rs", "rename this", &anchor("let x = 1;", 7), 1_000)
            .expect("add");
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.path, "src/a.rs");
        assert_eq!(comment.body, "rename this");
        assert_eq!(comment.status, ReviewStatus::Open);
        assert_eq!(comment.anchor, anchor("let x = 1;", 7));
        assert_eq!(comment.created_at, 1_000);
        assert_eq!(comment.resolved_at, None);
        assert_eq!(comment.snapshot, None);
        assert_eq!(comment.location, None);
        assert!(!comment.outdated);
        assert_eq!(store.get(id + 1).expect("get"), None);
    }

    #[test]
    fn the_list_is_ordered_by_path_then_start_line_then_id() {
        let store = ReviewStore::open_in_memory().expect("open");
        let late = store.add("b.rs", "late", &anchor("x", 30), 1).expect("add");
        let early = store.add("b.rs", "early", &anchor("x", 3), 2).expect("add");
        let first_file = store.add("a.rs", "top", &anchor("x", 99), 3).expect("add");
        let tie = store.add("b.rs", "tie", &anchor("x", 3), 4).expect("add");
        let order: Vec<i64> = store
            .list()
            .expect("list")
            .iter()
            .map(|comment| comment.id)
            .collect();
        assert_eq!(order, vec![first_file, early, tie, late]);
    }

    #[test]
    fn the_body_status_and_snapshot_change_in_place() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store
            .add("a.rs", "first", &anchor("x", 1), 10)
            .expect("add");
        store.set_body(id, "second").expect("body");
        store.set_snapshot(id, &"c".repeat(40)).expect("snapshot");
        store
            .set_status(id, ReviewStatus::Resolved, 20)
            .expect("resolve");
        let resolved = store.get(id).expect("get").expect("found");
        assert_eq!(resolved.body, "second");
        assert_eq!(resolved.snapshot, Some("c".repeat(40)));
        assert_eq!(resolved.status, ReviewStatus::Resolved);
        assert_eq!(resolved.resolved_at, Some(20));
        assert_eq!(resolved.created_at, 10);

        store
            .set_status(id, ReviewStatus::Open, 30)
            .expect("reopen");
        let reopened = store.get(id).expect("get").expect("found");
        assert_eq!(reopened.status, ReviewStatus::Open);
        assert_eq!(reopened.resolved_at, None);
    }

    #[test]
    fn a_write_to_a_missing_comment_is_an_error() {
        let store = ReviewStore::open_in_memory().expect("open");
        let message = |result: Result<(), StoreError>| result.expect_err("refused").0;
        assert_eq!(message(store.set_body(9, "x")), "no review comment 9");
        assert_eq!(
            message(store.set_status(9, ReviewStatus::Resolved, 1)),
            "no review comment 9"
        );
        assert_eq!(message(store.set_snapshot(9, "abc")), "no review comment 9");
        assert_eq!(message(store.delete(9)), "no review comment 9");
    }

    #[test]
    fn a_deleted_comment_is_gone() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store.add("a.rs", "x", &anchor("x", 1), 1).expect("add");
        store.delete(id).expect("delete");
        assert_eq!(store.get(id).expect("get"), None);
        assert!(store.list().expect("list").is_empty());
    }

    #[test]
    fn comments_survive_reopening_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gitmaster").join("review.db");
        let id = {
            let store = ReviewStore::open(&path).expect("create");
            store.add("a.rs", "kept", &anchor("x", 2), 5).expect("add")
        };
        let store = ReviewStore::open(&path).expect("reopen");
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.body, "kept");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused_and_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("review.db");
        ReviewStore::open(&path).expect("create");
        {
            let conn = Connection::open(&path).expect("raw open");
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .expect("bump");
        }
        let error = ReviewStore::open(&path).err().expect("refused");
        assert!(
            error
                .0
                .ends_with(": review.db was written by a newer GitMaster"),
            "{}",
            error.0
        );
        let conn = Connection::open(&path).expect("raw open");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION + 1);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_refused_and_not_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("review.db");
        let junk = b"this is not a sqlite file, it must not be overwritten".repeat(64);
        std::fs::write(&path, &junk).expect("write junk");
        let error = ReviewStore::open(&path).err().expect("refused");
        assert!(error.0.starts_with("cannot open "), "{}", error.0);
        assert_eq!(std::fs::read(&path).expect("read back"), junk);
    }

    fn v3_database(path: &Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "CREATE TABLE comments (id INTEGER PRIMARY KEY, path TEXT NOT NULL,
             body TEXT NOT NULL, status TEXT NOT NULL, anchor_json TEXT NOT NULL,
             created_at INTEGER NOT NULL, resolved_at INTEGER, snapshot TEXT,
             summary TEXT, after_snapshot TEXT);
             INSERT INTO comments (id, path, body, status, anchor_json, created_at,
                 resolved_at, snapshot, summary, after_snapshot) VALUES
             (1, 'a.rs', 'open one', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 10, NULL, 'c1', NULL, NULL),
             (2, 'a.rs', 'handled one', 'handled', '{\"text\":\"y\",\"before\":[],\"after\":[],\"start_line\":2}', 20, NULL, 'c2', 'did two', 'a2'),
             (3, 'a.rs', 'resolved one', 'resolved', '{\"text\":\"z\",\"before\":[],\"after\":[],\"start_line\":3}', 30, 77, 'c3', 'did three', 'a3');
             PRAGMA user_version = 3;",
        )
        .expect("v3");
    }

    #[test]
    fn a_version_three_database_moves_each_summary_into_an_agent_reply() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        v3_database(&path);
        let store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);

        let columns: Vec<String> = store
            .conn
            .prepare("PRAGMA table_info(comments)")
            .expect("prepare")
            .query_map([], |row| row.get(1))
            .expect("query")
            .map(|name| name.expect("name"))
            .collect();
        assert!(!columns.iter().any(|name| name == "summary"), "{columns:?}");
        assert!(columns.iter().any(|name| name == "parent"), "{columns:?}");

        let listed = store.list().expect("list");
        let ids: Vec<i64> = listed.iter().map(|comment| comment.id).collect();
        assert_eq!(ids, vec![1, 2, 3], "replies are not roots");
        assert!(listed[0].replies.is_empty());
        assert_eq!(listed[1].status, ReviewStatus::Handled);
        assert_eq!(listed[1].replies.len(), 1);
        let reply = &listed[1].replies[0];
        assert_eq!(reply.author, "agent");
        assert_eq!(reply.body, "did two");
        assert_eq!(reply.created_at, 20, "no resolve time: the creation time");
        assert_eq!(listed[1].after_snapshot.as_deref(), Some("a2"));
        assert_eq!(listed[2].status, ReviewStatus::Resolved);
        assert_eq!(listed[2].resolved_at, Some(77));
        assert_eq!(listed[2].replies[0].body, "did three");
        assert_eq!(listed[2].replies[0].created_at, 77, "the resolve time");

        let got = store.get(2).expect("get").expect("found");
        assert_eq!(got.replies, listed[1].replies);
        assert_eq!(store.get(4).expect("get"), None);
    }

    #[test]
    fn a_reply_attaches_to_its_root_and_a_reply_is_not_a_root() {
        let store = ReviewStore::open_in_memory().expect("open");
        let root = store
            .add("a.rs", "rename", &anchor("x", 1), 1)
            .expect("add");
        assert_eq!(
            store
                .reply(root, "human", "first", 2, false)
                .expect("reply"),
            Reply::Added { reopened: false }
        );
        let first = store.get(root).expect("get").expect("root").replies[0].id;
        assert_eq!(store.root_of(first).expect("root of"), Some(root));
        assert_eq!(store.root_of(root).expect("root of"), Some(root));
        assert_eq!(store.root_of(99).expect("root of"), None);
        assert_eq!(
            store.get(first).expect("get"),
            None,
            "a reply is not a comment"
        );
        for refused in [
            store.set_body(first, "x").err(),
            store.set_status(first, ReviewStatus::Resolved, 3).err(),
            store.set_snapshot(first, "x").err(),
            store.delete(first).err(),
        ] {
            assert_eq!(
                refused.expect("refused").0,
                format!("no review comment {first}")
            );
        }
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn deleting_a_comment_deletes_its_replies() {
        let store = ReviewStore::open_in_memory().expect("open");
        let root = store
            .add("a.rs", "rename", &anchor("x", 1), 1)
            .expect("add");
        store.reply(root, "human", "one", 2, false).expect("reply");
        store.delete(root).expect("delete");
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM comments", [], |row| row.get(0))
            .expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_person_reply_reopens_a_handled_comment_and_an_agent_reply_does_not() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store
            .add("a.rs", "rename", &anchor("x", 1), 1)
            .expect("add");
        assert!(store.handle(id, "agent:t", "done", 2).expect("handled"));
        store.set_after_snapshot(id, "after").expect("after");
        assert_eq!(
            store.reply(id, "agent:t", "more", 3, false).expect("agent"),
            Reply::Added { reopened: false }
        );
        assert_eq!(
            store.get(id).expect("get").expect("found").status,
            ReviewStatus::Handled
        );
        assert_eq!(
            store
                .reply(id, "human", "no, again", 4, true)
                .expect("human"),
            Reply::Added { reopened: true }
        );
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.status, ReviewStatus::Open);
        assert_eq!(comment.after_snapshot, None);
        let bodies: Vec<&str> = comment.replies.iter().map(|r| r.body.as_str()).collect();
        assert_eq!(bodies, ["done", "more", "no, again"]);
        let authors: Vec<&str> = comment.replies.iter().map(|r| r.author.as_str()).collect();
        assert_eq!(authors, ["agent:t", "agent:t", "human"]);
    }

    #[test]
    fn replies_keep_their_order_when_they_share_a_millisecond() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store
            .add("a.rs", "rename", &anchor("x", 1), 1)
            .expect("add");
        store.reply(id, "human", "first", 5, false).expect("first");
        store
            .reply(id, "human", "second", 5, false)
            .expect("second");
        store.reply(id, "human", "third", 4, false).expect("third");
        let bodies: Vec<String> = store
            .list()
            .expect("list")
            .remove(0)
            .replies
            .into_iter()
            .map(|reply| reply.body)
            .collect();
        assert_eq!(bodies, ["third", "first", "second"]);
    }
}
