//! Where review comments live (B4, D96, D97): a SQLite file in the Git common
//! directory, `.git/gitmaster/review.db`, beside the workflow store. The anchor
//! is stored as JSON. The location is not stored: it is computed on read, from
//! the file as it is now (see `review`).
//!
//! A scorecard (D219) is what a review published on a branch: one row of `scorecards`, and its
//! comments, which are agent-started root comments that name it. They are written in one
//! transaction, the newest 20 scorecards per name and branch are kept, and the comments of a
//! scorecard do not count against the agent-thread limits.
//!
//! A reply is a row of the same table with a `parent`: one id names a comment or
//! a reply, and a reply belongs to a root comment (one level deep). A reply copies
//! its root's path and anchor and has status `open`; only its `body`, `author` and
//! `created_at` are its own. Every write to a comment names a root.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use gm_schema::{
    CommentLabel, RETAINED_PER_NAME_AND_BRANCH, ReviewAnchor, ReviewComment, ReviewKind,
    ReviewReply, ReviewSeverity, ReviewStatus, Scorecard, ScorecardStatus,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::review;

/// Schema version written to `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 11;
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

/// Version 4 to 5 (D200): an agent's thread has a kind and, for a flag, a severity. Its
/// author is the root's `author`, which version 4 already has. Existing rows keep `NULL`.
const V5: &str = "
ALTER TABLE comments ADD COLUMN kind TEXT;
ALTER TABLE comments ADD COLUMN severity TEXT;
";

/// Version 5 to 6 (D212 (6)): verdicts, and the findings a verdict produced. A finding is a root
/// comment with the verdict's id; `ON DELETE SET NULL` keeps the comment when retention deletes
/// its verdict.
const V6: &str = "
CREATE TABLE verdicts (id INTEGER PRIMARY KEY, reviewer TEXT NOT NULL, branch TEXT NOT NULL,
    head TEXT NOT NULL, signal TEXT NOT NULL, value TEXT NOT NULL, rationale TEXT NOT NULL,
    origin TEXT NOT NULL, model TEXT, run_thread TEXT, at INTEGER NOT NULL);
CREATE INDEX verdicts_by_reviewer ON verdicts(reviewer, branch, id);
ALTER TABLE comments ADD COLUMN verdict_id INTEGER REFERENCES verdicts(id) ON DELETE SET NULL;
";

/// Version 6 to 7 (D215 (2)): the approval of a branch's tip, one row per branch. `head` is the
/// commit approved; the approval is stale when the branch has moved on, derived on read.
const V7: &str = "
CREATE TABLE approvals (branch TEXT PRIMARY KEY, head TEXT NOT NULL, note TEXT,
    origin TEXT NOT NULL, at INTEGER NOT NULL);
";

/// Version 7 to 8 (D212 (6), D214 (1)): a finding keeps the branch of its verdict. Retention
/// unlinks an old verdict's findings (`verdict_id` becomes `NULL`), and a finding that is still
/// open must keep blocking the `findings` gate of its branch, so the branch is stored with it.
/// Existing findings take the branch of the verdict they were linked to.
const V8: &str = "
ALTER TABLE comments ADD COLUMN finding_branch TEXT;
UPDATE comments SET finding_branch = (SELECT branch FROM verdicts WHERE verdicts.id = comments.verdict_id)
    WHERE verdict_id IS NOT NULL;
";

/// Version 8 to 9 (D212 (4), R2h): the audit of bypasses, one row per bypass, written before the
/// bypassed command runs, so it survives the app. A finding also keeps the commit it was made on
/// (`finding_head`), so a rename of its branch does not let it escape the `findings` gate: a
/// finding whose commit is an ancestor of the branch's tip counts. Existing findings take the
/// commit of the verdict they were linked to; a finding that retention unlinked keeps `NULL`.
const V9: &str = "
CREATE TABLE bypasses (id INTEGER PRIMARY KEY, branch TEXT NOT NULL, head TEXT NOT NULL,
    operation TEXT NOT NULL, gates TEXT NOT NULL, reason TEXT NOT NULL, actor TEXT NOT NULL,
    at INTEGER NOT NULL);
ALTER TABLE comments ADD COLUMN finding_head TEXT;
UPDATE comments SET finding_head = (SELECT head FROM verdicts WHERE verdicts.id = comments.verdict_id)
    WHERE verdict_id IS NOT NULL;
";

/// Version 9 to 10 (D214 (3)): proposed reviewers and gates. A proposal is a row, never a file:
/// `kind` is `reviewer` or `gate`, `body` the JSON of the definition, and `status` is `proposed`,
/// `denied` or `accepted`. The `suggested_fix` column of a finding is used by the next slice (D215 (3)).
const V10: &str = "
CREATE TABLE proposals (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
    body TEXT NOT NULL, reason TEXT NOT NULL, origin TEXT NOT NULL, status TEXT NOT NULL,
    at INTEGER NOT NULL, decided_at INTEGER);
ALTER TABLE comments ADD COLUMN suggested_fix TEXT;
";

/// Version 10 to 11 (D219): scorecards replace verdicts, approvals, bypasses and proposals. A
/// scorecard is a row, and its comments link to it by `scorecard_id`; `ON DELETE SET NULL` keeps a
/// comment when retention deletes its scorecard. `comments` is rebuilt without the foreign key to
/// `verdicts`, which the drop would otherwise leave dangling: SQLite then refuses every insert into
/// `comments`. The rebuild runs with foreign keys off (see `prepare`), so the self-reference of a
/// reply does not cascade. The rows of the four dropped tables are not migrated. Their comments stay
/// as review comments, with their `verdict_id`, `finding_branch` and `finding_head` values kept, unused.
const V11: &str = "
CREATE TABLE scorecards (id INTEGER PRIMARY KEY, name TEXT NOT NULL, branch TEXT NOT NULL,
    head TEXT NOT NULL, status TEXT NOT NULL, label TEXT NOT NULL, decorations TEXT NOT NULL,
    subject TEXT NOT NULL, body TEXT NOT NULL, origin TEXT NOT NULL, model TEXT, run_thread TEXT,
    workflow_run TEXT, created_at INTEGER NOT NULL);
CREATE INDEX scorecards_by_name ON scorecards(name, branch, id);
CREATE TABLE comments_v11 (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    anchor_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    snapshot TEXT,
    after_snapshot TEXT,
    parent INTEGER REFERENCES comments(id) ON DELETE CASCADE,
    author TEXT,
    kind TEXT,
    severity TEXT,
    finding_branch TEXT,
    finding_head TEXT,
    suggested_fix TEXT,
    verdict_id INTEGER,
    scorecard_id INTEGER REFERENCES scorecards(id) ON DELETE SET NULL,
    label TEXT,
    decorations TEXT
);
INSERT INTO comments_v11 (id, path, body, status, anchor_json, created_at, resolved_at, snapshot,
    after_snapshot, parent, author, kind, severity, finding_branch, finding_head, suggested_fix,
    verdict_id)
    SELECT id, path, body, status, anchor_json, created_at, resolved_at, snapshot, after_snapshot,
    parent, author, kind, severity, finding_branch, finding_head, suggested_fix, verdict_id
    FROM comments;
DROP TABLE comments;
ALTER TABLE comments_v11 RENAME TO comments;
DROP TABLE verdicts;
DROP TABLE approvals;
DROP TABLE bypasses;
DROP TABLE proposals;
";

/// The upgrades after the first version, in order. Each runs when the database is at its
/// `from` version, so a database several versions behind runs every step after it.
const UPGRADES: [(i64, &str); 10] = [
    (1, V2),
    (2, V3),
    (3, V4),
    (4, V5),
    (5, V6),
    (6, V7),
    (7, V8),
    (8, V9),
    (9, V10),
    (10, V11),
];

/// Columns read back for a root `ReviewComment`, in the order `raw_comment` expects.
const COLUMNS: &str = "id, path, body, status, anchor_json, created_at, resolved_at, snapshot, \
     after_snapshot, author, kind, severity, suggested_fix, label, decorations, scorecard_id";

/// The select of a scorecard, with the open comments that block counted (D219 (4)). The order
/// of its columns is the one `raw_scorecard` expects.
const SCORECARD_SELECT: &str = "SELECT s.id, s.name, s.branch, s.head, s.status, s.label,
    s.decorations, s.subject, s.body, s.origin, s.model, s.run_thread, s.workflow_run,
    s.created_at,
    (SELECT COUNT(*) FROM comments c WHERE c.scorecard_id = s.id AND c.parent IS NULL
     AND c.status = 'open'
     AND (c.severity = 'high' OR EXISTS (SELECT 1 FROM json_each(c.decorations)
                                         WHERE json_each.value = 'blocking')))
    FROM scorecards s";

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

/// What an agent starts a thread with (D200): its author (`agent:<client>`), what it
/// says, and for a flag how serious it is. Its label and decorations are D219 (5).
pub struct AgentThread<'a> {
    pub author: &'a str,
    pub kind: ReviewKind,
    pub severity: Option<ReviewSeverity>,
    pub label: Option<CommentLabel>,
    pub decorations: &'a [String],
    pub body: &'a str,
}

/// One root row as SQLite gives it: id, path, body, status, anchor JSON, created,
/// resolved, snapshot, the "after" snapshot, author, kind, severity, the suggested fix, the
/// label, the decorations JSON and the scorecard.
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
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
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
        // Off while migrating: a rebuild of `comments` must not cascade through its own foreign
        // keys. Foreign keys are on for every other use of the connection.
        conn.pragma_update(None, "foreign_keys", false)
            .map_err(StoreError::from)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(StoreError::from)?;
        migrate(conn)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::from)
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

    /// Stores a new open thread that an agent started on `path` (D200), and returns its id.
    /// The open agent threads of the repository and of the file are counted, and the row is
    /// written, inside one write transaction: a thread over either limit is refused with the
    /// reason, and nothing is written. Another process cannot add a thread between the count
    /// and the insert, because the write lock is taken first.
    pub fn add_agent(
        &self,
        path: &str,
        thread: &AgentThread,
        anchor: &ReviewAnchor,
        now: i64,
    ) -> Result<i64, StoreError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let written: Result<i64, StoreError> = (|| {
            let in_repository = open_agent_count(&self.conn, None)?;
            let on_file = open_agent_count(&self.conn, Some(path))?;
            review::check_agent_limits(in_repository, on_file).map_err(StoreError)?;
            insert_agent_root(
                &self.conn,
                &AgentRoot {
                    path,
                    body: thread.body,
                    anchor,
                    now,
                    author: thread.author,
                    kind: thread.kind,
                    severity: thread.severity,
                    label: thread.label,
                    decorations: thread.decorations,
                    scorecard: None,
                    suggested_fix: None,
                },
            )
        })();
        let outcome = written.and_then(|id| {
            self.conn.execute_batch("COMMIT")?;
            Ok(id)
        });
        if outcome.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        outcome
    }

    /// Publishes a scorecard and its comments (D219 (6)) in one write transaction, then keeps the
    /// newest `RETAINED_PER_NAME_AND_BRANCH` scorecards of its name and branch. The older ones are
    /// deleted in the same transaction, and their comments stay, unlinked. The comments are not
    /// counted against the agent-thread limits.
    pub fn record_scorecard(
        &mut self,
        new: &NewScorecard,
        comments: &[NewScorecardComment],
    ) -> Result<RecordedScorecard, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO scorecards (name, branch, head, status, label, decorations, subject, body,
             origin, model, run_thread, workflow_run, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                new.name,
                new.branch,
                new.head,
                new.status.as_str(),
                new.label.as_str(),
                json_list(&new.decorations)?,
                new.subject,
                new.body,
                new.origin,
                new.model,
                new.run_thread,
                new.workflow_run,
                new.at,
            ],
        )?;
        let id = tx.last_insert_rowid();
        let mut ids = Vec::with_capacity(comments.len());
        for comment in comments {
            ids.push(insert_agent_root(
                &tx,
                &AgentRoot {
                    path: &comment.path,
                    body: &comment.body,
                    anchor: &comment.anchor,
                    now: new.at,
                    author: &new.origin,
                    kind: comment.kind,
                    severity: comment.severity,
                    label: Some(comment.label),
                    decorations: &comment.decorations,
                    scorecard: Some(id),
                    suggested_fix: comment.suggested_fix.as_deref(),
                },
            )?);
        }
        let keep = i64::try_from(RETAINED_PER_NAME_AND_BRANCH).unwrap_or(i64::MAX);
        tx.execute(
            "DELETE FROM scorecards WHERE name = ?1 AND branch = ?2 AND id NOT IN (
                 SELECT id FROM scorecards WHERE name = ?1 AND branch = ?2
                 ORDER BY id DESC LIMIT ?3)",
            params![new.name, new.branch, keep],
        )?;
        tx.commit()?;
        Ok(RecordedScorecard { id, comments: ids })
    }

    /// The scorecard `id`, with its open blocking comments counted; `None` when there is none.
    /// `stale` is false: the engine derives it from the branch's tip.
    pub fn get_scorecard(&self, id: i64) -> Result<Option<Scorecard>, StoreError> {
        let row = self
            .conn
            .query_row(
                &format!("{SCORECARD_SELECT} WHERE s.id = ?1"),
                params![id],
                raw_scorecard,
            )
            .optional()?;
        row.map(scorecard_of).transpose()
    }

    /// The scorecards of `name` and `branch` (each optional), newest first, at most `limit`.
    /// `stale` is false: the engine derives it.
    pub fn list_scorecards(
        &self,
        name: Option<&str>,
        branch: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Scorecard>, StoreError> {
        let limit = i64::try_from(limit).map_err(|e| StoreError(e.to_string()))?;
        let mut statement = self.conn.prepare(&format!(
            "{SCORECARD_SELECT} WHERE (?1 IS NULL OR s.name = ?1) AND (?2 IS NULL OR s.branch = ?2)
             ORDER BY s.id DESC LIMIT ?3"
        ))?;
        let rows = statement.query_map(params![name, branch, limit], raw_scorecard)?;
        rows.map(|row| scorecard_of(row?)).collect()
    }

    /// How many agent-started threads are not resolved in the repository (D200).
    pub fn open_agent_threads(&self) -> Result<usize, StoreError> {
        open_agent_count(&self.conn, None)
    }

    /// How many agent-started threads are not resolved on `path` (D200).
    pub fn open_agent_threads_on(&self, path: &str) -> Result<usize, StoreError> {
        open_agent_count(&self.conn, Some(path))
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

/// The open agent-started threads of the repository, or of `path` (D200): roots whose author
/// is an agent and whose status is not resolved. A scorecard's comments are not threads (D219),
/// and neither are the findings of the verdicts that version 11 removed (D212 (6)).
fn open_agent_count(conn: &Connection, path: Option<&str>) -> Result<usize, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM comments WHERE parent IS NULL AND author LIKE 'agent:%'
         AND finding_branch IS NULL AND scorecard_id IS NULL AND status != ?1
         AND (?2 IS NULL OR path = ?2)",
        params![status_text(ReviewStatus::Resolved), path],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|e| StoreError(e.to_string()))
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
        1..SCHEMA_VERSION => {}
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

/// What an agent-started root row holds (D200, D212 (6)).
struct AgentRoot<'a> {
    path: &'a str,
    body: &'a str,
    anchor: &'a ReviewAnchor,
    now: i64,
    author: &'a str,
    kind: ReviewKind,
    severity: Option<ReviewSeverity>,
    /// The label the agent gave the thread (D219 (5)); `None` when it gave none.
    label: Option<CommentLabel>,
    /// The decorations the agent gave the thread, in order; empty for none (D219 (5)).
    decorations: &'a [String],
    /// The scorecard the comment belongs to (D219 (6)); `None` for a thread an agent starts.
    scorecard: Option<i64>,
    /// The fix a reviewer suggested, redacted (D215 (3)); `None` for a thread.
    suggested_fix: Option<&'a str>,
}

/// Inserts an open agent-started root comment and returns its id. The caller owns the
/// transaction and any limit check.
fn insert_agent_root(conn: &Connection, root: &AgentRoot) -> Result<i64, StoreError> {
    let anchor_json = to_json(root.anchor)?;
    let decorations = (!root.decorations.is_empty())
        .then(|| json_list(root.decorations))
        .transpose()?;
    conn.execute(
        "INSERT INTO comments (path, body, status, anchor_json, created_at, author, kind,
         severity, label, decorations, scorecard_id, suggested_fix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            root.path,
            root.body,
            status_text(ReviewStatus::Open),
            anchor_json,
            root.now,
            root.author,
            kind_text(root.kind),
            root.severity.map(severity_text),
            root.label.map(CommentLabel::as_str),
            decorations,
            root.scorecard,
            root.suggested_fix,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// One scorecard row as SQLite gives it, in `SCORECARD_SELECT` order, with its blocking count.
type RawScorecard = (
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

fn raw_scorecard(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawScorecard> {
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
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

fn scorecard_of(raw: RawScorecard) -> Result<Scorecard, StoreError> {
    let (
        id,
        name,
        branch,
        head,
        status,
        label,
        decorations,
        subject,
        body,
        origin,
        model,
        run_thread,
        workflow_run,
        created_at,
        blocking_open,
    ) = raw;
    Ok(Scorecard {
        id,
        status: ScorecardStatus::parse(&status)
            .ok_or_else(|| StoreError(format!("unknown scorecard status {status:?}")))?,
        label: parse_label(&label)?,
        decorations: serde_json::from_str(&decorations).map_err(|e| {
            StoreError(format!(
                "cannot decode the decorations of scorecard {id}: {e}"
            ))
        })?,
        name,
        branch,
        head,
        // Derived by the engine from the branch's tip; the store never sets it (D219 (4)).
        stale: false,
        blocking_open: u32::try_from(blocking_open).unwrap_or(u32::MAX),
        subject,
        body,
        origin,
        model,
        run_thread,
        workflow_run,
        created_at,
    })
}

/// The JSON array text of `items`, which is `[]` for none.
fn json_list(items: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(items).map_err(|e| StoreError(format!("cannot encode JSON: {e}")))
}

fn parse_label(text: &str) -> Result<CommentLabel, StoreError> {
    CommentLabel::parse(text).ok_or_else(|| StoreError(format!("unknown comment label {text:?}")))
}

fn status_text(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Open => "open",
        ReviewStatus::Handled => "handled",
        ReviewStatus::Resolved => "resolved",
    }
}

fn kind_text(kind: ReviewKind) -> &'static str {
    match kind {
        ReviewKind::Note => "note",
        ReviewKind::Question => "question",
        ReviewKind::Flag => "flag",
    }
}

fn parse_kind(text: &str) -> Result<ReviewKind, StoreError> {
    match text {
        "note" => Ok(ReviewKind::Note),
        "question" => Ok(ReviewKind::Question),
        "flag" => Ok(ReviewKind::Flag),
        other => Err(StoreError(format!("unknown review kind {other:?}"))),
    }
}

fn severity_text(severity: ReviewSeverity) -> &'static str {
    match severity {
        ReviewSeverity::Low => "low",
        ReviewSeverity::Medium => "medium",
        ReviewSeverity::High => "high",
    }
}

fn parse_severity(text: &str) -> Result<ReviewSeverity, StoreError> {
    match text {
        "low" => Ok(ReviewSeverity::Low),
        "medium" => Ok(ReviewSeverity::Medium),
        "high" => Ok(ReviewSeverity::High),
        other => Err(StoreError(format!("unknown review severity {other:?}"))),
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
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
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
    let (
        id,
        path,
        body,
        status,
        anchor_json,
        created_at,
        resolved_at,
        snapshot,
        after_snapshot,
        author,
        kind,
        severity,
        suggested_fix,
        label,
        decorations,
        scorecard,
    ) = raw;
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
        author,
        kind: kind.as_deref().map(parse_kind).transpose()?,
        severity: severity.as_deref().map(parse_severity).transpose()?,
        needs_answer: false,
        suggested_fix,
        label: label.as_deref().map(parse_label).transpose()?,
        decorations: match decorations {
            Some(text) => serde_json::from_str(&text).map_err(|e| {
                StoreError(format!(
                    "cannot decode the decorations of comment {id}: {e}"
                ))
            })?,
            None => Vec::new(),
        },
        scorecard,
        workspace: None,
    })
}

fn to_json(anchor: &ReviewAnchor) -> Result<String, StoreError> {
    serde_json::to_string(anchor).map_err(|e| StoreError(format!("cannot encode JSON: {e}")))
}

/// A scorecard to record (D219 (6)): the summary and the commit it judged. `origin` is the
/// author of its comments too.
pub struct NewScorecard {
    pub name: String,
    pub branch: String,
    pub head: String,
    pub status: ScorecardStatus,
    pub label: CommentLabel,
    pub decorations: Vec<String>,
    pub subject: String,
    pub body: String,
    /// `agent:<client>`.
    pub origin: String,
    pub model: Option<String>,
    pub run_thread: Option<String>,
    pub workflow_run: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub at: i64,
}

/// A comment to record with its scorecard: a root comment anchored like any agent's thread.
pub struct NewScorecardComment {
    pub path: String,
    pub anchor: ReviewAnchor,
    pub body: String,
    pub kind: ReviewKind,
    pub severity: Option<ReviewSeverity>,
    pub label: CommentLabel,
    pub decorations: Vec<String>,
    /// The redacted fix the reviewer suggested, when it gave one (D215 (3)).
    pub suggested_fix: Option<String>,
}

/// What `record_scorecard` wrote: the scorecard's id and the ids of its comments, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedScorecard {
    pub id: i64,
    pub comments: Vec<i64>,
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

    fn agent_thread(kind: ReviewKind, body: &str) -> AgentThread<'_> {
        AgentThread {
            author: "agent:test",
            kind,
            severity: None,
            label: None,
            decorations: &[],
            body,
        }
    }

    fn v4_database(path: &Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "CREATE TABLE comments (id INTEGER PRIMARY KEY, path TEXT NOT NULL,
             body TEXT NOT NULL, status TEXT NOT NULL, anchor_json TEXT NOT NULL,
             created_at INTEGER NOT NULL, resolved_at INTEGER, snapshot TEXT,
             after_snapshot TEXT, parent INTEGER REFERENCES comments(id) ON DELETE CASCADE,
             author TEXT);
             INSERT INTO comments (id, path, body, status, anchor_json, created_at,
                 snapshot, parent, author) VALUES
             (1, 'a.rs', 'rename this', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 10, 'c1', NULL, NULL),
             (2, 'a.rs', 'which one?', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 11, NULL, 1, 'human');
             PRAGMA user_version = 4;",
        )
        .expect("v4");
    }

    #[test]
    fn a_version_four_database_keeps_its_threads_and_gains_kind_and_severity() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        v4_database(&path);
        let store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        let kept = store.list().expect("list").remove(0);
        assert_eq!(kept.body, "rename this");
        assert_eq!(kept.snapshot.as_deref(), Some("c1"));
        assert_eq!(kept.author, None, "a person's thread has no author");
        assert_eq!(kept.kind, None);
        assert_eq!(kept.severity, None);
        assert_eq!(kept.replies.len(), 1);
        assert_eq!(kept.replies[0].author, "human");
        assert_eq!(store.open_agent_threads().expect("count"), 0);
        let id = store
            .add_agent(
                "a.rs",
                &AgentThread {
                    author: "agent:test",
                    kind: ReviewKind::Flag,
                    severity: Some(ReviewSeverity::High),
                    label: None,
                    decorations: &[],
                    body: "risk",
                },
                &anchor("x", 1),
                20,
            )
            .expect("agent thread after the upgrade");
        assert_eq!(store.open_agent_threads().expect("count"), 1);
        assert_eq!(
            store.get(id).expect("get").expect("found").kind,
            Some(ReviewKind::Flag)
        );
    }

    #[test]
    fn an_agent_thread_round_trips_its_author_kind_and_severity() {
        let store = ReviewStore::open_in_memory().expect("open");
        let id = store
            .add_agent(
                "src/a.rs",
                &AgentThread {
                    author: "agent:claude-code",
                    kind: ReviewKind::Flag,
                    severity: Some(ReviewSeverity::Medium),
                    label: None,
                    decorations: &[],
                    body: "guess",
                },
                &anchor("let x = 1;", 3),
                7,
            )
            .expect("add");
        let comment = store.get(id).expect("get").expect("found");
        assert_eq!(comment.path, "src/a.rs");
        assert_eq!(comment.body, "guess");
        assert_eq!(comment.status, ReviewStatus::Open);
        assert_eq!(comment.author.as_deref(), Some("agent:claude-code"));
        assert_eq!(comment.kind, Some(ReviewKind::Flag));
        assert_eq!(comment.severity, Some(ReviewSeverity::Medium));
        assert_eq!(comment.anchor, anchor("let x = 1;", 3));
        let person = store.add("a.rs", "mine", &anchor("x", 1), 8).expect("add");
        let person = store.get(person).expect("get").expect("found");
        assert_eq!(
            (person.author, person.kind, person.severity),
            (None, None, None)
        );
    }

    #[test]
    fn agent_threads_are_counted_per_repository_and_per_file_and_not_once_resolved() {
        let store = ReviewStore::open_in_memory().expect("open");
        let thread = agent_thread(ReviewKind::Note, "n");
        for _ in 0..5 {
            store
                .add_agent("a.rs", &thread, &anchor("x", 1), 1)
                .expect("add");
        }
        assert_eq!(store.open_agent_threads_on("a.rs").expect("count"), 5);
        assert_eq!(store.open_agent_threads().expect("count"), 5);
        let refused = store
            .add_agent("a.rs", &thread, &anchor("x", 1), 1)
            .expect_err("sixth on one file");
        assert!(
            refused.0.contains("this file already has 5"),
            "{}",
            refused.0
        );
        assert_eq!(
            store.open_agent_threads().expect("count"),
            5,
            "nothing written"
        );

        store
            .add_agent("b.rs", &thread, &anchor("x", 1), 1)
            .expect("other file");
        let first = store.list().expect("list")[0].id;
        store
            .set_status(first, ReviewStatus::Resolved, 2)
            .expect("resolve");
        assert_eq!(store.open_agent_threads_on("a.rs").expect("count"), 4);
        store
            .add_agent("a.rs", &thread, &anchor("x", 1), 3)
            .expect("room again");

        let person = store.add("a.rs", "mine", &anchor("x", 1), 4).expect("add");
        store
            .reply(person, "agent:test", "reply", 5, false)
            .expect("reply");
        assert_eq!(store.open_agent_threads_on("a.rs").expect("count"), 5);
        assert_eq!(store.open_agent_threads().expect("count"), 6);
    }

    #[test]
    fn the_repository_limit_refuses_a_thread_on_a_new_file() {
        let store = ReviewStore::open_in_memory().expect("open");
        let thread = agent_thread(ReviewKind::Question, "q");
        for file in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            for _ in 0..5 {
                store
                    .add_agent(file, &thread, &anchor("x", 1), 1)
                    .expect("add");
            }
        }
        let refused = store
            .add_agent("e.rs", &thread, &anchor("x", 1), 1)
            .expect_err("twenty-first");
        assert!(
            refused.0.contains("the repository already has 20"),
            "{}",
            refused.0
        );
        assert_eq!(store.open_agent_threads().expect("count"), 20);
    }

    fn v5_database(path: &Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "CREATE TABLE comments (id INTEGER PRIMARY KEY, path TEXT NOT NULL,
             body TEXT NOT NULL, status TEXT NOT NULL, anchor_json TEXT NOT NULL,
             created_at INTEGER NOT NULL, resolved_at INTEGER, snapshot TEXT,
             after_snapshot TEXT, parent INTEGER REFERENCES comments(id) ON DELETE CASCADE,
             author TEXT, kind TEXT, severity TEXT);
             INSERT INTO comments (id, path, body, status, anchor_json, created_at, snapshot,
                 parent, author, kind, severity) VALUES
             (1, 'a.rs', 'rename this', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 10, 'c1', NULL, NULL, NULL, NULL),
             (2, 'a.rs', 'guess', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":2}', 11, NULL, NULL, 'agent:claude-code', 'flag', 'high'),
             (3, 'a.rs', 'which one?', 'open', '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":1}', 12, NULL, 1, 'human', NULL, NULL);
             PRAGMA user_version = 5;",
        )
        .expect("v5");
    }

    /// The shape of the comments and scorecards tables, their foreign keys, the indexes and the
    /// tables that exist.
    fn shape(store: &ReviewStore) -> Vec<String> {
        let mut shape = Vec::new();
        let queries = [
            "SELECT 'comments ' || name || ' ' || type || ' ' || \"notnull\" || ' ' || quote(dflt_value) || ' ' || pk FROM pragma_table_info('comments')",
            "SELECT 'scorecards ' || name || ' ' || type || ' ' || \"notnull\" || ' ' || quote(dflt_value) || ' ' || pk FROM pragma_table_info('scorecards')",
            "SELECT 'fk ' || \"table\" || ' ' || \"from\" || ' ' || \"to\" || ' ' || on_delete FROM pragma_foreign_key_list('comments')",
            "SELECT 'index ' || name || ' on ' || tbl_name FROM sqlite_master WHERE type = 'index' ORDER BY name",
            "SELECT 'table ' || name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        ];
        for sql in queries {
            let rows: Vec<String> = store
                .conn
                .prepare(sql)
                .expect("prepare")
                .query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect();
            shape.extend(rows);
        }
        shape
    }

    fn anchor_of(text: &str, start_line: u32) -> ReviewAnchor {
        anchor(text, start_line)
    }

    fn new_scorecard(name: &str, branch: &str, at: i64) -> NewScorecard {
        NewScorecard {
            name: name.to_owned(),
            branch: branch.to_owned(),
            head: "a".repeat(40),
            status: ScorecardStatus::NeedsWork,
            label: CommentLabel::Issue,
            decorations: vec!["security".to_owned()],
            subject: "Unchecked input".to_owned(),
            body: "looks risky".to_owned(),
            origin: "agent:claude-code".to_owned(),
            model: Some("haiku".to_owned()),
            run_thread: Some("thread-1".to_owned()),
            workflow_run: None,
            at,
        }
    }

    fn new_comment(
        text: &str,
        label: CommentLabel,
        severity: Option<ReviewSeverity>,
        decorations: &[&str],
    ) -> NewScorecardComment {
        NewScorecardComment {
            path: "a.rs".to_owned(),
            anchor: anchor_of(text, 1),
            body: format!("about {text}"),
            kind: gm_schema::kind_of_label(label),
            severity,
            label,
            decorations: decorations.iter().map(|d| (*d).to_owned()).collect(),
            suggested_fix: None,
        }
    }

    fn count(store: &ReviewStore, sql: &str) -> i64 {
        store
            .conn
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn a_version_five_database_keeps_its_comments_and_takes_a_scorecard() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        v5_database(&path);
        let mut store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        let kept = store.get(1).expect("get").expect("kept");
        assert_eq!(kept.replies.len(), 1);
        assert_eq!(kept.replies[0].body, "which one?");
        let flag = store.get(2).expect("get").expect("kept");
        assert_eq!(flag.kind, Some(ReviewKind::Flag));
        assert_eq!(flag.severity, Some(ReviewSeverity::High));
        let recorded = store
            .record_scorecard(
                &new_scorecard("security", "feat", 20),
                &[new_comment("x", CommentLabel::Nitpick, None, &[])],
            )
            .expect("record after the upgrade");
        assert_eq!(recorded.comments.len(), 1);
    }

    #[test]
    fn an_upgraded_database_has_the_same_shape_as_a_fresh_one() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        v5_database(&path);
        let upgraded = ReviewStore::open(&path).expect("migrate");
        let fresh = ReviewStore::open_in_memory().expect("fresh");
        let fresh_shape = shape(&fresh);
        for line in [
            "comments scorecard_id INTEGER 0 NULL 0",
            "comments label TEXT 0 NULL 0",
            "comments decorations TEXT 0 NULL 0",
            "index scorecards_by_name on scorecards",
            "table scorecards",
            "fk comments parent id CASCADE",
            "fk scorecards scorecard_id id SET NULL",
        ] {
            assert!(
                fresh_shape.iter().any(|l| l == line),
                "{line} in {fresh_shape:?}"
            );
        }
        assert!(
            !fresh_shape
                .iter()
                .any(|l| l == "table verdicts" || l.contains("verdicts")),
            "{fresh_shape:?}"
        );
        assert_eq!(shape(&upgraded), fresh_shape);
    }

    fn v10_database(path: &Path) {
        v5_database(path);
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(&format!("{V6}{V7}{V8}{V9}{V10}"))
            .expect("v6 to v10");
        conn.execute(
            "INSERT INTO verdicts (reviewer, branch, head, signal, value, rationale, origin, at)
             VALUES ('security', 'feat', ?1, 'level', 'high', 'unchecked input', 'agent:x', 5)",
            params!["a".repeat(40)],
        )
        .expect("verdict");
        conn.execute(
            "INSERT INTO comments (path, body, status, anchor_json, created_at, author, kind,
             severity, verdict_id, finding_branch, finding_head)
             VALUES ('a.rs', 'unchecked', 'open',
             '{\"text\":\"x\",\"before\":[],\"after\":[],\"start_line\":4}', 6, 'agent:x',
             'flag', 'high', 1, 'feat', ?1)",
            params!["a".repeat(40)],
        )
        .expect("finding of the verdict");
        conn.execute(
            "INSERT INTO approvals (branch, head, origin, at) VALUES ('feat', 'b', 'human', 1)",
            [],
        )
        .expect("approval");
        conn.pragma_update(None, "user_version", 10)
            .expect("version");
    }

    #[test]
    fn a_version_ten_database_keeps_its_comments_and_drops_the_verdict_tables() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("review.db");
        v10_database(&path);
        let store = ReviewStore::open(&path).expect("migrate");
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        for table in ["verdicts", "approvals", "bypasses", "proposals"] {
            assert_eq!(
                count(
                    &store,
                    &format!(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
                    )
                ),
                0,
                "{table} is dropped"
            );
        }
        let finding = store.get(4).expect("get").expect("kept");
        assert_eq!(finding.body, "unchecked");
        assert_eq!(finding.kind, Some(ReviewKind::Flag));
        assert_eq!(finding.label, None);
        assert!(finding.decorations.is_empty());
        assert_eq!(finding.scorecard, None);
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM comments WHERE id = 4 AND verdict_id = 1 AND finding_branch = 'feat'"
            ),
            1,
            "the old finding keeps its verdict and branch values, unused"
        );
        assert_eq!(
            store.open_agent_threads().expect("count"),
            1,
            "only the version 5 agent thread counts; the old finding does not"
        );
    }

    #[test]
    fn a_scorecard_and_its_comments_are_written_together_and_read_back() {
        let mut store = ReviewStore::open_in_memory().expect("open");
        let decorations = vec!["blocking".to_owned(), "auth".to_owned()];
        let mut blocking = new_comment(
            "fn a() {}",
            CommentLabel::Issue,
            Some(ReviewSeverity::High),
            &["blocking", "auth"],
        );
        blocking.decorations = decorations.clone();
        let recorded = store
            .record_scorecard(
                &new_scorecard("security", "feat", 100),
                &[
                    blocking,
                    new_comment("fn b() {}", CommentLabel::Praise, None, &[]),
                ],
            )
            .expect("record");
        assert_eq!(recorded.comments.len(), 2);
        let scorecard = store.get_scorecard(recorded.id).expect("get").expect("row");
        assert_eq!(scorecard.name, "security");
        assert_eq!(scorecard.status, ScorecardStatus::NeedsWork);
        assert_eq!(scorecard.label, CommentLabel::Issue);
        assert_eq!(scorecard.decorations, ["security"]);
        assert_eq!(scorecard.model.as_deref(), Some("haiku"));
        assert_eq!(scorecard.run_thread.as_deref(), Some("thread-1"));
        assert_eq!(scorecard.created_at, 100);
        assert!(!scorecard.stale, "the engine derives staleness");
        assert_eq!(scorecard.blocking_open, 1);
        let first = store
            .get(recorded.comments[0])
            .expect("get")
            .expect("comment");
        assert_eq!(first.scorecard, Some(recorded.id));
        assert_eq!(first.label, Some(CommentLabel::Issue));
        assert_eq!(first.decorations, decorations);
        assert_eq!(first.kind, Some(ReviewKind::Flag));
        assert_eq!(first.severity, Some(ReviewSeverity::High));
        assert_eq!(first.author.as_deref(), Some("agent:claude-code"));
        let praise = store
            .get(recorded.comments[1])
            .expect("get")
            .expect("comment");
        assert_eq!(praise.kind, Some(ReviewKind::Note));
        assert_eq!(praise.severity, None);
        assert!(praise.decorations.is_empty());
        assert_eq!(
            store.list_scorecards(None, None, 20).expect("list").len(),
            1
        );
    }

    #[test]
    fn a_scorecard_comment_is_not_counted_against_the_agent_thread_limits() {
        let mut store = ReviewStore::open_in_memory().expect("open");
        let comments: Vec<NewScorecardComment> = (0..25)
            .map(|_| new_comment("x", CommentLabel::Issue, None, &[]))
            .collect();
        store
            .record_scorecard(&new_scorecard("security", "feat", 1), &comments)
            .expect("record");
        assert_eq!(store.open_agent_threads().expect("repo"), 0);
        assert_eq!(store.open_agent_threads_on("a.rs").expect("file"), 0);
        let thread = agent_thread(ReviewKind::Note, "still allowed");
        store
            .add_agent("a.rs", &thread, &anchor("x", 1), 2)
            .expect("an agent thread is not refused");
        assert_eq!(store.open_agent_threads_on("a.rs").expect("file"), 1);
    }

    #[test]
    fn a_comment_keeps_the_label_and_decorations_its_agent_gave() {
        let store = ReviewStore::open_in_memory().expect("open");
        let decorations = vec!["non-blocking".to_owned(), "naming".to_owned()];
        let thread = AgentThread {
            author: "agent:test",
            kind: ReviewKind::Flag,
            severity: Some(ReviewSeverity::Low),
            label: Some(CommentLabel::Nitpick),
            decorations: &decorations,
            body: "rename",
        };
        let id = store
            .add_agent("a.rs", &thread, &anchor("x", 1), 3)
            .expect("add");
        let comment = store.get(id).expect("get").expect("row");
        assert_eq!(comment.label, Some(CommentLabel::Nitpick));
        assert_eq!(comment.decorations, decorations);
        assert_eq!(comment.scorecard, None);
    }

    #[test]
    fn retention_keeps_the_newest_twenty_per_name_and_branch_and_unlinks_the_rest() {
        let mut store = ReviewStore::open_in_memory().expect("open");
        let mut first = None;
        for at in 1..=22 {
            let recorded = store
                .record_scorecard(
                    &new_scorecard("security", "feat", at),
                    &[new_comment("x", CommentLabel::Issue, None, &[])],
                )
                .expect("record");
            first.get_or_insert(recorded.clone());
        }
        store
            .record_scorecard(&new_scorecard("security", "main", 30), &[])
            .expect("other branch");
        store
            .record_scorecard(&new_scorecard("code-review", "feat", 31), &[])
            .expect("other name");
        let kept: Vec<i64> = store
            .list_scorecards(Some("security"), Some("feat"), 50)
            .expect("list")
            .iter()
            .map(|scorecard| scorecard.id)
            .collect();
        assert_eq!(kept.len(), RETAINED_PER_NAME_AND_BRANCH);
        let first = first.expect("first");
        assert!(!kept.contains(&first.id));
        assert!(store.get_scorecard(first.id).expect("get").is_none());
        let orphan = store.get(first.comments[0]).expect("get").expect("kept");
        assert_eq!(orphan.scorecard, None, "the comment stays, unlinked");
        assert_eq!(
            store
                .list_scorecards(Some("security"), Some("main"), 50)
                .expect("list")
                .len(),
            1
        );
        assert_eq!(
            store
                .list_scorecards(Some("code-review"), None, 50)
                .expect("list")
                .len(),
            1
        );
    }

    #[test]
    fn blocking_open_counts_only_open_comments_that_block_or_are_high() {
        let mut store = ReviewStore::open_in_memory().expect("open");
        let recorded = store
            .record_scorecard(
                &new_scorecard("security", "feat", 1),
                &[
                    new_comment("a", CommentLabel::Nitpick, None, &["blocking"]),
                    new_comment("b", CommentLabel::Issue, Some(ReviewSeverity::High), &[]),
                    new_comment("c", CommentLabel::Issue, Some(ReviewSeverity::Medium), &[]),
                    new_comment("d", CommentLabel::Nitpick, None, &["blocking"]),
                ],
            )
            .expect("record");
        assert_eq!(
            store
                .get_scorecard(recorded.id)
                .expect("get")
                .expect("row")
                .blocking_open,
            3
        );
        store
            .handle(recorded.comments[0], "agent:claude-code", "fixed", 5)
            .expect("handle");
        assert_eq!(
            store
                .get_scorecard(recorded.id)
                .expect("get")
                .expect("row")
                .blocking_open,
            2,
            "a handled comment no longer counts"
        );
    }

    #[test]
    fn list_filters_by_name_and_branch_and_is_newest_first() {
        let mut store = ReviewStore::open_in_memory().expect("open");
        let first = store
            .record_scorecard(&new_scorecard("security", "feat", 1), &[])
            .expect("first")
            .id;
        let second = store
            .record_scorecard(&new_scorecard("code-review", "feat", 2), &[])
            .expect("second")
            .id;
        let third = store
            .record_scorecard(&new_scorecard("security", "main", 3), &[])
            .expect("third")
            .id;
        let ids = |rows: Vec<Scorecard>| -> Vec<i64> { rows.iter().map(|row| row.id).collect() };
        assert_eq!(
            ids(store.list_scorecards(None, None, 20).expect("all")),
            [third, second, first]
        );
        assert_eq!(
            ids(store
                .list_scorecards(Some("security"), None, 20)
                .expect("name")),
            [third, first]
        );
        assert_eq!(
            ids(store
                .list_scorecards(None, Some("feat"), 20)
                .expect("branch")),
            [second, first]
        );
        assert_eq!(
            ids(store.list_scorecards(None, None, 2).expect("limit")),
            [third, second]
        );
        assert!(
            store
                .list_scorecards(Some("nope"), None, 20)
                .expect("none")
                .is_empty()
        );
    }
}
