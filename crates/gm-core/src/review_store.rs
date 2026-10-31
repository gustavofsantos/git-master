//! Where review comments live (B4, D96, D97): a SQLite file in the Git common
//! directory, `.git/gitmaster/review.db`, beside the workflow store. The anchor
//! is stored as JSON. The location is not stored: it is computed on read, from
//! the file as it is now (see `review`).

use std::path::Path;
use std::time::Duration;

use gm_schema::{ReviewAnchor, ReviewComment, ReviewStatus};
use rusqlite::{Connection, OptionalExtension, params};

/// Schema version written to `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 2;
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
    summary TEXT
);
";

/// Version 1 to 2 (D135): the agent's summary of a handled comment.
const V2: &str = "ALTER TABLE comments ADD COLUMN summary TEXT;";

/// Columns read back for a `ReviewComment`, in the order `raw_comment` expects.
const COLUMNS: &str =
    "id, path, body, status, anchor_json, created_at, resolved_at, snapshot, summary";

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

/// One row as SQLite gives it: id, path, body, status, anchor JSON, created,
/// resolved, snapshot and summary.
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

    /// Every comment, by path, then by the start line of its anchor, then by id.
    /// `location` is `None` and `outdated` false: the caller computes them.
    pub fn list(&self) -> Result<Vec<ReviewComment>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM comments
             ORDER BY path, json_extract(anchor_json, '$.start_line'), id"
        ))?;
        let rows = statement.query_map([], raw_comment)?;
        rows.map(|row| comment_of(row?)).collect()
    }

    pub fn get(&self, id: i64) -> Result<Option<ReviewComment>, StoreError> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM comments WHERE id = ?1"),
                params![id],
                raw_comment,
            )
            .optional()?;
        row.map(comment_of).transpose()
    }

    pub fn set_body(&self, id: i64, body: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET body = ?2 WHERE id = ?1",
                params![id, body],
            )?,
            id,
        )
    }

    /// Sets the status. `resolved_at` is `now` when the comment becomes resolved
    /// and cleared otherwise; the agent's summary is cleared when it is reopened.
    pub fn set_status(&self, id: i64, status: ReviewStatus, now: i64) -> Result<(), StoreError> {
        let resolved_at = (status == ReviewStatus::Resolved).then_some(now);
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET status = ?2, resolved_at = ?3,
                 summary = CASE WHEN ?2 = 'open' THEN NULL ELSE summary END WHERE id = ?1",
                params![id, status_text(status), resolved_at],
            )?,
            id,
        )
    }

    /// Marks the comment handled by an agent, with its `summary` (D135).
    pub fn set_handled(&self, id: i64, summary: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET status = ?2, resolved_at = NULL, summary = ?3 WHERE id = ?1",
                params![id, status_text(ReviewStatus::Handled), summary],
            )?,
            id,
        )
    }

    /// Records the snapshot object id taken when the comment was made.
    pub fn set_snapshot(&self, id: i64, oid: &str) -> Result<(), StoreError> {
        Self::changed(
            self.conn.execute(
                "UPDATE comments SET snapshot = ?2 WHERE id = ?1",
                params![id, oid],
            )?,
            id,
        )
    }

    pub fn delete(&self, id: i64) -> Result<(), StoreError> {
        Self::changed(
            self.conn
                .execute("DELETE FROM comments WHERE id = ?1", params![id])?,
            id,
        )
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

/// Creates the schema of an empty database; refuses one written by another
/// version of the app. Each step is one transaction.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match version {
        0 => {
            let tx = conn.transaction()?;
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit().map_err(StoreError::from)
        }
        1 => {
            let tx = conn.transaction()?;
            tx.execute_batch(V2)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit().map_err(StoreError::from)
        }
        v if v == SCHEMA_VERSION => Ok(()),
        v if v > SCHEMA_VERSION => Err(StoreError(
            "review.db was written by a newer GitMaster".to_owned(),
        )),
        v => Err(StoreError(format!(
            "review.db has unknown schema version {v}"
        ))),
    }
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

fn comment_of(raw: RawComment) -> Result<ReviewComment, StoreError> {
    let (id, path, body, status, anchor_json, created_at, resolved_at, snapshot, summary) = raw;
    Ok(ReviewComment {
        id,
        path,
        body,
        status: parse_status(&status)?,
        anchor: serde_json::from_str(&anchor_json)
            .map_err(|e| StoreError(format!("cannot decode the anchor of comment {id}: {e}")))?,
        created_at,
        resolved_at,
        summary,
        snapshot,
        location: None,
        outdated: false,
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
        store.set_handled(id, "done").expect("handled");
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.status, ReviewStatus::Handled);
        assert_eq!(comment.summary.as_deref(), Some("done"));
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
}
