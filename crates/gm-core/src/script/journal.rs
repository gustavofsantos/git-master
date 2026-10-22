//! The run journal (design/workflow-scripts.md, D80, D100): what the host observed of each run,
//! written by the host from the calls themselves, never from what a script says. It is history for
//! the Runs tab and the evidence a restart needs to mark a run `interrupted`; it is not a way to
//! resume (Q23). Lives in `state.db`, next to the tables of the TOML engine.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The last runs kept per workflow (D80).
pub const RETAINED_RUNS: usize = 200;
/// A result larger than this is stored as its hash, its size and its first `RESULT_HEAD` bytes.
pub const RESULT_LIMIT: usize = 64 * 1024;
pub const RESULT_HEAD: usize = 4 * 1024;

pub type RunId = i64;

/// Schema version in `script_meta`. Separate from `PRAGMA user_version`, which the old store owns.
const SCHEMA_VERSION: &str = "1";
/// How long a writer waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
/// The reason written on every run a restart found in progress.
const INTERRUPTED_REASON: &str = "the app stopped while the run was in progress";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS script_meta (
    key TEXT PRIMARY KEY,
    value TEXT
);
CREATE TABLE IF NOT EXISTS script_run (
    id INTEGER PRIMARY KEY,
    workflow TEXT NOT NULL,
    definition_hash TEXT NOT NULL,
    definition_text TEXT NOT NULL,
    trigger_json TEXT NOT NULL,
    started_by TEXT NOT NULL,
    seed INTEGER NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    end_reason TEXT,
    value_json TEXT
);
CREATE INDEX IF NOT EXISTS script_run_by_workflow ON script_run (workflow, id);
CREATE TABLE IF NOT EXISTS script_call (
    run_id INTEGER NOT NULL REFERENCES script_run (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    func TEXT NOT NULL,
    level TEXT NOT NULL,
    args_json TEXT NOT NULL,
    result_json TEXT,
    outcome TEXT,
    message TEXT,
    started_at INTEGER NOT NULL,
    duration_ms INTEGER,
    PRIMARY KEY (run_id, seq)
);
";

/// Columns read back for a `RunRow`, in the order `raw_run` expects them.
const RUN_COLUMNS: &str = "id, workflow, definition_hash, definition_text, trigger_json, \
     started_by, seed, status, started_at, ended_at, end_reason, value_json";
/// Columns read back for a `CallRow`, in the order `raw_call` expects them.
const CALL_COLUMNS: &str = "run_id, seq, func, level, args_json, result_json, outcome, message, \
     started_at, duration_ms";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalError(pub String);

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JournalError {}

impl From<rusqlite::Error> for JournalError {
    fn from(e: rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

/// Where a run is. Stored as lowercase text with underscores: `awaiting_approval`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    AwaitingApproval,
    Waiting,
    Done,
    Failed,
    Blocked,
    Cancelled,
    /// The app stopped while the run was in progress (D100).
    Interrupted,
}

impl RunStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "running" => Self::Running,
            "awaiting_approval" => Self::AwaitingApproval,
            "waiting" => Self::Waiting,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "blocked" => Self::Blocked,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    /// Whether the run is over.
    #[must_use]
    pub fn is_final(self) -> bool {
        !matches!(self, Self::Running | Self::AwaitingApproval | Self::Waiting)
    }
}

/// What a new run starts with. `definition_text` is the file as it was when the run started, so
/// the Runs tab shows what ran even if the file changed after.
#[derive(Clone, Debug)]
pub struct NewRun<'a> {
    pub workflow: &'a str,
    pub definition_hash: &'a str,
    pub definition_text: &'a str,
    pub trigger: &'a Value,
    /// `human`, `event:<name>` or `agent:<client>` (D64).
    pub started_by: &'a str,
    /// The seed of `ctx.random()`, so a run's random values can be told apart from its journal.
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub id: RunId,
    pub workflow: String,
    pub definition_hash: String,
    pub definition_text: String,
    pub trigger: Value,
    pub started_by: String,
    pub seed: u64,
    pub status: RunStatus,
    /// Milliseconds since the Unix epoch, as the caller passed them.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
    /// What `execute` returned, for a run that returned a value.
    pub value: Option<Value>,
}

/// A call about to start. Written before the effect runs.
#[derive(Clone, Debug)]
pub struct NewCall<'a> {
    pub run: RunId,
    /// 1, 2, 3… within the run, in the order the script made the calls.
    pub seq: u32,
    /// The name without `ctx.`, such as `branch.create`.
    pub func: &'a str,
    /// `read`, `propose`, `apply`, `pause` or `helper`.
    pub level: &'a str,
    /// The resolved arguments, as the host received them.
    pub args: &'a Value,
    pub started_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallRow {
    pub run: RunId,
    pub seq: u32,
    pub func: String,
    pub level: String,
    pub args: Value,
    /// `None` until the call ends; stays `None` for a call a restart found unfinished.
    pub result: Option<Value>,
    /// `done`, `failed`, `conflict`, … or `interrupted` for a call a restart found unfinished.
    pub outcome: Option<String>,
    pub message: Option<String>,
    pub started_at: i64,
    pub duration_ms: Option<i64>,
}

/// Cloneable: every run thread writes through the same connection.
#[derive(Clone)]
pub struct Journal {
    conn: Arc<Mutex<Connection>>,
}

impl Journal {
    /// Opens (creating it and its parent directory when missing) and migrates the journal tables
    /// in the database at `path`. Other tables of the file are left alone.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = open_file(path)
            .map_err(|e| JournalError(format!("cannot open {}: {}", path.display(), e.0)))?;
        Ok(Self::wrap(conn))
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&mut conn)?;
        Ok(Self::wrap(conn))
    }

    fn wrap(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// A poisoned lock is an error for the caller, not a second panic in the run thread.
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, JournalError> {
        self.conn.lock().map_err(|_| {
            JournalError("the run journal is unavailable after a panic in another thread".into())
        })
    }

    /// Adds a run with status `running` and prunes the workflow's runs beyond `RETAINED_RUNS`,
    /// oldest first, with their calls. Returns the new id.
    pub fn create_run(&self, new: &NewRun<'_>, now: i64) -> Result<RunId, JournalError> {
        let trigger = serde_json::to_string(new.trigger)
            .map_err(|e| JournalError(format!("cannot encode the trigger: {e}")))?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO script_run (workflow, definition_hash, definition_text, trigger_json, \
             started_by, seed, status, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7)",
            params![
                new.workflow,
                new.definition_hash,
                new.definition_text,
                trigger,
                new.started_by,
                new.seed.cast_signed(),
                now,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "DELETE FROM script_run WHERE workflow = ?1 AND id NOT IN (\
             SELECT id FROM script_run WHERE workflow = ?1 ORDER BY id DESC LIMIT ?2)",
            params![new.workflow, to_i64(RETAINED_RUNS)],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Moves an unfinished run to `status`. A run already in a final status is left as it is.
    pub fn set_status(&self, run: RunId, status: RunStatus) -> Result<(), JournalError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE script_run SET status = ?2 WHERE id = ?1 \
             AND status IN ('running', 'awaiting_approval', 'waiting')",
            params![run, status.as_str()],
        )?;
        if changed == 0 {
            ensure_run(&conn, run)?;
        }
        Ok(())
    }

    /// Writes the start of a call. The row has no result until `finish_call`.
    pub fn begin_call(&self, call: &NewCall<'_>) -> Result<(), JournalError> {
        let args = stored_json(call.args)?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO script_call (run_id, seq, func, level, args_json, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                call.run,
                i64::from(call.seq),
                call.func,
                call.level,
                args,
                call.started_at,
            ],
        )?;
        Ok(())
    }

    /// Writes the end of a call. `result` larger than `RESULT_LIMIT` bytes of JSON is stored as
    /// `{"truncated": true, "sha256": <hex>, "size": <bytes>, "head": <first RESULT_HEAD bytes,
    /// cut at a character boundary>}`.
    pub fn finish_call(
        &self,
        run: RunId,
        seq: u32,
        result: &Value,
        outcome: &str,
        message: Option<&str>,
        duration_ms: i64,
    ) -> Result<(), JournalError> {
        let stored = stored_json(result)?;
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE script_call SET result_json = ?3, outcome = ?4, message = ?5, duration_ms = ?6 \
             WHERE run_id = ?1 AND seq = ?2",
            params![run, i64::from(seq), stored, outcome, message, duration_ms],
        )?;
        if changed == 0 {
            return Err(JournalError(format!("no call {seq} in run {run}")));
        }
        Ok(())
    }

    /// Ends the run with a final status. A run already in a final status is left as it is.
    pub fn finish_run(
        &self,
        run: RunId,
        status: RunStatus,
        now: i64,
        reason: Option<&str>,
        value: Option<&Value>,
    ) -> Result<(), JournalError> {
        if !status.is_final() {
            return Err(JournalError(format!(
                "a run ends with a final status, not {}",
                status.as_str()
            )));
        }
        let value = value.map(stored_json).transpose()?;
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE script_run SET status = ?2, ended_at = ?3, end_reason = ?4, \
             value_json = COALESCE(?5, value_json) WHERE id = ?1 \
             AND status IN ('running', 'awaiting_approval', 'waiting')",
            params![run, status.as_str(), now, reason, value],
        )?;
        if changed == 0 {
            ensure_run(&conn, run)?;
        }
        Ok(())
    }

    pub fn run(&self, run: RunId) -> Result<Option<RunRow>, JournalError> {
        let conn = self.lock()?;
        let raw = conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM script_run WHERE id = ?1"),
                params![run],
                raw_run,
            )
            .optional()?;
        raw.map(RawRun::decode).transpose()
    }

    /// The newest runs of a workflow, newest first.
    pub fn runs(&self, workflow: &str, limit: usize) -> Result<Vec<RunRow>, JournalError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM script_run WHERE workflow = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let raws = stmt
            .query_map(params![workflow, to_i64(limit)], raw_run)?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(RawRun::decode).collect()
    }

    /// The calls of a run, in `seq` order.
    pub fn calls(&self, run: RunId) -> Result<Vec<CallRow>, JournalError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CALL_COLUMNS} FROM script_call WHERE run_id = ?1 ORDER BY seq"
        ))?;
        let raws = stmt
            .query_map(params![run], raw_call)?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(RawCall::decode).collect()
    }

    /// For the next open after the app stopped (D100): every run still `running`,
    /// `awaiting_approval` or `waiting` becomes `interrupted` with `ended_at` = `now` and the
    /// reason `the app stopped while the run was in progress`, and every call of those runs without
    /// an outcome gets the outcome `interrupted`. Returns the runs it changed, oldest first.
    pub fn recover_interrupted(&self, now: i64) -> Result<Vec<RunId>, JournalError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM script_run \
                 WHERE status IN ('running', 'awaiting_approval', 'waiting') \
                 ORDER BY started_at, id",
            )?;
            stmt.query_map([], |row| row.get::<_, RunId>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE script_run SET status = 'interrupted', ended_at = ?2, end_reason = ?3 \
                 WHERE id = ?1",
                params![id, now, INTERRUPTED_REASON],
            )?;
            tx.execute(
                "UPDATE script_call SET outcome = 'interrupted' \
                 WHERE run_id = ?1 AND outcome IS NULL",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }
}

fn open_file(path: &Path) -> Result<Connection, JournalError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| JournalError(format!("cannot create {}: {e}", parent.display())))?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), JournalError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// Creates the journal tables of an empty database and refuses a journal written by another
/// version of the app. The check and the creation run in one transaction.
fn migrate(conn: &mut Connection) -> Result<(), JournalError> {
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    let stored: Option<String> = tx
        .query_row(
            "SELECT value FROM script_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match stored {
        None => {
            tx.execute(
                "INSERT INTO script_meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION],
            )?;
        }
        Some(version) if version == SCHEMA_VERSION => {}
        Some(version) => {
            return Err(JournalError(format!(
                "the run journal is version {version}; this app reads version {SCHEMA_VERSION}"
            )));
        }
    }
    tx.commit()?;
    Ok(())
}

/// Text stored for a JSON value. A value over `RESULT_LIMIT` bytes becomes its digest and head.
fn stored_json(value: &Value) -> Result<String, JournalError> {
    let text = serde_json::to_string(&canonical(value))
        .map_err(|e| JournalError(format!("cannot encode JSON: {e}")))?;
    if text.len() <= RESULT_LIMIT {
        return Ok(text);
    }
    let mut cut = RESULT_HEAD.min(text.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    Ok(canonical(&json!({
        "truncated": true,
        "sha256": hex(&Sha256::digest(text.as_bytes())),
        "size": text.len(),
        "head": &text[..cut],
    }))
    .to_string())
}

/// The value with every object's keys in sorted order. `serde_json` keeps the order of insertion
/// when any crate of the build turns on `preserve_order`, so the same call would otherwise be
/// stored as different text from one build to another.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&map[key])))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            // Writing to a String cannot fail.
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn parse_json(text: &str) -> Result<Value, JournalError> {
    serde_json::from_str(text)
        .map_err(|e| JournalError(format!("a stored JSON value is not valid: {e}")))
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn ensure_run(conn: &Connection, run: RunId) -> Result<(), JournalError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM script_run WHERE id = ?1)",
        params![run],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(JournalError(format!("no run {run}")))
    }
}

/// A run row as SQLite returns it; decoded once the lock is released.
struct RawRun {
    id: RunId,
    workflow: String,
    definition_hash: String,
    definition_text: String,
    trigger_json: String,
    started_by: String,
    seed: i64,
    status: String,
    started_at: i64,
    ended_at: Option<i64>,
    end_reason: Option<String>,
    value_json: Option<String>,
}

fn raw_run(row: &Row<'_>) -> rusqlite::Result<RawRun> {
    Ok(RawRun {
        id: row.get(0)?,
        workflow: row.get(1)?,
        definition_hash: row.get(2)?,
        definition_text: row.get(3)?,
        trigger_json: row.get(4)?,
        started_by: row.get(5)?,
        seed: row.get(6)?,
        status: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        end_reason: row.get(10)?,
        value_json: row.get(11)?,
    })
}

impl RawRun {
    fn decode(self) -> Result<RunRow, JournalError> {
        let status = RunStatus::parse(&self.status).ok_or_else(|| {
            JournalError(format!(
                "run {} has an unknown status {:?}",
                self.id, self.status
            ))
        })?;
        Ok(RunRow {
            id: self.id,
            workflow: self.workflow,
            definition_hash: self.definition_hash,
            definition_text: self.definition_text,
            trigger: parse_json(&self.trigger_json)?,
            started_by: self.started_by,
            seed: self.seed.cast_unsigned(),
            status,
            started_at: self.started_at,
            ended_at: self.ended_at,
            end_reason: self.end_reason,
            value: self.value_json.as_deref().map(parse_json).transpose()?,
        })
    }
}

/// A call row as SQLite returns it; decoded once the lock is released.
struct RawCall {
    run: RunId,
    seq: i64,
    func: String,
    level: String,
    args_json: String,
    result_json: Option<String>,
    outcome: Option<String>,
    message: Option<String>,
    started_at: i64,
    duration_ms: Option<i64>,
}

fn raw_call(row: &Row<'_>) -> rusqlite::Result<RawCall> {
    Ok(RawCall {
        run: row.get(0)?,
        seq: row.get(1)?,
        func: row.get(2)?,
        level: row.get(3)?,
        args_json: row.get(4)?,
        result_json: row.get(5)?,
        outcome: row.get(6)?,
        message: row.get(7)?,
        started_at: row.get(8)?,
        duration_ms: row.get(9)?,
    })
}

impl RawCall {
    fn decode(self) -> Result<CallRow, JournalError> {
        let seq = u32::try_from(self.seq).map_err(|_| {
            JournalError(format!(
                "call {} of run {} has an invalid seq",
                self.seq, self.run
            ))
        })?;
        Ok(CallRow {
            run: self.run,
            seq,
            func: self.func,
            level: self.level,
            args: parse_json(&self.args_json)?,
            result: self.result_json.as_deref().map(parse_json).transpose()?,
            outcome: self.outcome,
            message: self.message,
            started_at: self.started_at,
            duration_ms: self.duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::store::Store;
    use sha2::{Digest, Sha256};

    fn fresh() -> Journal {
        Journal::open_in_memory().expect("open in memory")
    }

    fn new_run<'a>(workflow: &'a str, trigger: &'a Value) -> NewRun<'a> {
        NewRun {
            workflow,
            definition_hash: "sha256:abc",
            definition_text: "return {}",
            trigger,
            started_by: "human",
            seed: 7,
        }
    }

    fn start(j: &Journal, workflow: &str, now: i64) -> RunId {
        j.create_run(&new_run(workflow, &json!(null)), now)
            .expect("create")
    }

    fn call(run: RunId, seq: u32, args: &Value) -> NewCall<'_> {
        NewCall {
            run,
            seq,
            func: "branch.create",
            level: "apply",
            args,
            started_at: i64::from(seq),
        }
    }

    fn all_statuses() -> [RunStatus; 8] {
        [
            RunStatus::Running,
            RunStatus::AwaitingApproval,
            RunStatus::Waiting,
            RunStatus::Done,
            RunStatus::Failed,
            RunStatus::Blocked,
            RunStatus::Cancelled,
            RunStatus::Interrupted,
        ]
    }

    #[test]
    fn run_round_trips_every_field() {
        let j = fresh();
        let trigger = json!({"event": "push", "count": 3});
        let id = j
            .create_run(
                &NewRun {
                    workflow: "review",
                    definition_hash: "h1",
                    definition_text: "text of the file",
                    trigger: &trigger,
                    started_by: "event:push",
                    seed: 42,
                },
                1000,
            )
            .expect("create");
        assert_eq!(
            j.run(id).expect("read"),
            Some(RunRow {
                id,
                workflow: "review".into(),
                definition_hash: "h1".into(),
                definition_text: "text of the file".into(),
                trigger: trigger.clone(),
                started_by: "event:push".into(),
                seed: 42,
                status: RunStatus::Running,
                started_at: 1000,
                ended_at: None,
                end_reason: None,
                value: None,
            })
        );
        j.finish_run(
            id,
            RunStatus::Done,
            2000,
            Some("ok"),
            Some(&json!({"a": [1, 2]})),
        )
        .expect("finish");
        let row = j.run(id).expect("read").expect("exists");
        assert_eq!(row.status, RunStatus::Done);
        assert_eq!(row.ended_at, Some(2000));
        assert_eq!(row.end_reason.as_deref(), Some("ok"));
        assert_eq!(row.value, Some(json!({"a": [1, 2]})));
        assert_eq!(row.trigger, trigger);
        assert!(j.run(id + 1).expect("read").is_none());
    }

    #[test]
    fn every_status_round_trips() {
        let j = fresh();
        for status in all_statuses() {
            assert_eq!(RunStatus::parse(status.as_str()), Some(status));
            let id = start(&j, "statuses", 1);
            j.set_status(id, status).expect("set");
            assert_eq!(j.run(id).expect("read").expect("row").status, status);
        }
        assert_eq!(RunStatus::parse("Running"), None);
    }

    #[test]
    fn seed_round_trips_at_the_u64_limits() {
        let j = fresh();
        for seed in [0, 42, u64::MAX] {
            let trigger = json!(null);
            let id = j
                .create_run(
                    &NewRun {
                        seed,
                        ..new_run("seeds", &trigger)
                    },
                    1,
                )
                .expect("create");
            assert_eq!(j.run(id).expect("read").expect("row").seed, seed);
        }
    }

    #[test]
    fn call_lifecycle_and_order() {
        let j = fresh();
        let id = start(&j, "calls", 1);
        let args = json!({"branch": "main"});
        j.begin_call(&call(id, 2, &args)).expect("begin 2");
        j.begin_call(&call(id, 1, &args)).expect("begin 1");

        let calls = j.calls(id).expect("calls");
        assert_eq!(calls.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(calls[0].args, args);
        assert_eq!(calls[0].result, None);
        assert_eq!(calls[0].outcome, None);
        assert_eq!(calls[0].message, None);
        assert_eq!(calls[0].duration_ms, None);
        assert_eq!(calls[0].func, "branch.create");
        assert_eq!(calls[0].level, "apply");

        j.finish_call(id, 1, &json!({"ok": true}), "done", Some("created"), 12)
            .expect("finish");
        let done = j.calls(id).expect("calls").remove(0);
        assert_eq!(done.result, Some(json!({"ok": true})));
        assert_eq!(done.outcome.as_deref(), Some("done"));
        assert_eq!(done.message.as_deref(), Some("created"));
        assert_eq!(done.duration_ms, Some(12));

        assert!(j.finish_call(id, 9, &json!(null), "done", None, 0).is_err());
    }

    #[test]
    fn a_begin_for_an_unknown_run_is_refused() {
        let j = fresh();
        let args = json!(null);
        assert!(j.begin_call(&call(99, 1, &args)).is_err());
    }

    #[test]
    fn a_final_status_is_not_moved() {
        let j = fresh();
        let id = start(&j, "final", 1);
        j.finish_run(id, RunStatus::Done, 2000, Some("ok"), None)
            .expect("finish");

        j.finish_run(
            id,
            RunStatus::Cancelled,
            3000,
            Some("late cancel"),
            Some(&json!(1)),
        )
        .expect("refused quietly");
        j.set_status(id, RunStatus::Running)
            .expect("refused quietly");
        j.set_status(id, RunStatus::Failed)
            .expect("refused quietly");

        let row = j.run(id).expect("read").expect("row");
        assert_eq!(row.status, RunStatus::Done);
        assert_eq!(row.ended_at, Some(2000));
        assert_eq!(row.end_reason.as_deref(), Some("ok"));
        assert_eq!(row.value, None);
    }

    #[test]
    fn a_run_cannot_end_with_a_status_that_is_not_final() {
        let j = fresh();
        let id = start(&j, "final", 1);
        assert!(j.finish_run(id, RunStatus::Waiting, 2, None, None).is_err());
        assert_eq!(
            j.run(id).expect("read").expect("row").status,
            RunStatus::Running
        );
    }

    #[test]
    fn retention_keeps_the_newest_runs_of_each_workflow() {
        let j = fresh();
        let total = RETAINED_RUNS + 5;
        let mut alpha = Vec::new();
        for i in 0..i64::try_from(total).expect("fits") {
            let id = start(&j, "alpha", i);
            j.begin_call(&call(id, 1, &json!(i))).expect("call");
            alpha.push(id);
        }
        let beta: Vec<RunId> = (0..3)
            .map(|i| {
                let id = start(&j, "beta", i);
                j.begin_call(&call(id, 1, &json!(i))).expect("call");
                id
            })
            .collect();

        let kept = j.runs("alpha", usize::MAX).expect("runs");
        assert_eq!(kept.len(), RETAINED_RUNS);
        let expected: Vec<RunId> = alpha[total - RETAINED_RUNS..]
            .iter()
            .rev()
            .copied()
            .collect();
        assert_eq!(kept.iter().map(|r| r.id).collect::<Vec<_>>(), expected);

        assert_eq!(j.runs("beta", 10).expect("runs").len(), 3);
        assert_eq!(
            j.runs("beta", 10)
                .expect("runs")
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            beta.iter().rev().copied().collect::<Vec<_>>()
        );

        for pruned in &alpha[..total - RETAINED_RUNS] {
            assert!(j.run(*pruned).expect("read").is_none());
            assert!(j.calls(*pruned).expect("calls").is_empty());
        }
        assert_eq!(j.calls(alpha[total - 1]).expect("calls").len(), 1);

        let guard = j.lock().expect("lock");
        let calls: i64 = guard
            .query_row("SELECT COUNT(*) FROM script_call", [], |row| row.get(0))
            .expect("count");
        assert_eq!(calls, i64::try_from(RETAINED_RUNS + 3).expect("fits"));
    }

    #[test]
    fn runs_limit_returns_the_newest_first() {
        let j = fresh();
        let ids: Vec<RunId> = (0..5).map(|i| start(&j, "limit", i)).collect();
        let got: Vec<RunId> = j
            .runs("limit", 2)
            .expect("runs")
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(got, vec![ids[4], ids[3]]);
    }

    #[test]
    fn a_large_result_is_stored_as_its_digest_and_head() {
        let j = fresh();
        let id = start(&j, "big", 1);
        let big = Value::String("x".repeat(100_000));
        let text = serde_json::to_string(&big).expect("text");
        j.begin_call(&call(id, 1, &json!(null))).expect("begin");
        j.finish_call(id, 1, &big, "done", None, 1).expect("finish");

        let record = j
            .calls(id)
            .expect("calls")
            .remove(0)
            .result
            .expect("result");
        assert_eq!(record["truncated"], json!(true));
        assert_eq!(record["size"], json!(text.len()));
        assert_eq!(
            record["sha256"],
            json!(hex(&Sha256::digest(text.as_bytes())))
        );
        let head = record["head"].as_str().expect("head");
        assert_eq!(head.len(), RESULT_HEAD);
        assert_eq!(head, &text[..RESULT_HEAD]);
    }

    #[test]
    fn a_head_is_cut_back_to_a_character_boundary() {
        let j = fresh();
        let id = start(&j, "utf8", 1);
        // Each "é" is two bytes, so byte 4096 of the JSON text falls inside one.
        let value = Value::String("é".repeat(50_000));
        let text = serde_json::to_string(&value).expect("text");
        j.begin_call(&call(id, 1, &json!(null))).expect("begin");
        j.finish_call(id, 1, &value, "done", None, 1)
            .expect("finish");

        let record = j
            .calls(id)
            .expect("calls")
            .remove(0)
            .result
            .expect("result");
        let head = record["head"].as_str().expect("head");
        assert_eq!(head.len(), RESULT_HEAD - 1);
        assert_eq!(head, &text[..RESULT_HEAD - 1]);
        assert_eq!(record["size"], json!(text.len()));
    }

    #[test]
    fn a_result_at_the_limit_is_kept_whole() {
        let j = fresh();
        let id = start(&j, "limit", 1);
        let value = Value::String("a".repeat(RESULT_LIMIT - 2));
        j.begin_call(&call(id, 1, &json!(null))).expect("begin");
        j.finish_call(id, 1, &value, "done", None, 1)
            .expect("finish");
        assert_eq!(j.calls(id).expect("calls").remove(0).result, Some(value));
    }

    #[test]
    fn large_args_are_truncated_too() {
        let j = fresh();
        let id = start(&j, "args", 1);
        let args = Value::String("y".repeat(100_000));
        j.begin_call(&call(id, 1, &args)).expect("begin");
        let stored = j.calls(id).expect("calls").remove(0).args;
        assert_eq!(stored["truncated"], json!(true));
    }

    #[test]
    fn recovery_marks_only_the_runs_in_progress() {
        let j = fresh();
        let running = start(&j, "recover", 1);
        let awaiting = start(&j, "recover", 2);
        j.set_status(awaiting, RunStatus::AwaitingApproval)
            .expect("set");
        let waiting = start(&j, "recover", 3);
        j.set_status(waiting, RunStatus::Waiting).expect("set");
        let done = start(&j, "recover", 4);
        j.finish_run(done, RunStatus::Done, 2000, Some("ok"), None)
            .expect("finish");
        let failed = start(&j, "recover", 5);
        j.finish_run(failed, RunStatus::Failed, 2000, None, None)
            .expect("finish");
        let cancelled = start(&j, "recover", 6);
        j.finish_run(cancelled, RunStatus::Cancelled, 2000, None, None)
            .expect("finish");

        let args = json!(null);
        j.begin_call(&call(running, 1, &args)).expect("begin");
        j.finish_call(running, 1, &json!("kept"), "done", None, 1)
            .expect("finish");
        j.begin_call(&call(running, 2, &args)).expect("begin");
        j.begin_call(&call(awaiting, 1, &args)).expect("begin");
        j.begin_call(&call(done, 1, &args)).expect("begin");

        let changed = j.recover_interrupted(9000).expect("recover");
        assert_eq!(changed, vec![running, awaiting, waiting]);

        for id in [running, awaiting, waiting] {
            let row = j.run(id).expect("read").expect("row");
            assert_eq!(row.status, RunStatus::Interrupted);
            assert_eq!(row.ended_at, Some(9000));
            assert_eq!(row.end_reason.as_deref(), Some(INTERRUPTED_REASON));
        }
        assert_eq!(
            INTERRUPTED_REASON,
            "the app stopped while the run was in progress"
        );

        let run_calls = j.calls(running).expect("calls");
        assert_eq!(run_calls[0].outcome.as_deref(), Some("done"));
        assert_eq!(run_calls[0].result, Some(json!("kept")));
        assert_eq!(run_calls[1].outcome.as_deref(), Some("interrupted"));
        assert_eq!(run_calls[1].result, None);
        assert_eq!(
            j.calls(awaiting).expect("calls")[0].outcome.as_deref(),
            Some("interrupted")
        );

        let untouched = j.run(done).expect("read").expect("row");
        assert_eq!(untouched.status, RunStatus::Done);
        assert_eq!(untouched.ended_at, Some(2000));
        assert_eq!(j.calls(done).expect("calls")[0].outcome, None);
        assert_eq!(
            j.run(failed).expect("read").expect("row").status,
            RunStatus::Failed
        );
        assert_eq!(
            j.run(cancelled).expect("read").expect("row").status,
            RunStatus::Cancelled
        );

        assert!(j.recover_interrupted(9500).expect("again").is_empty());
        assert_eq!(
            j.run(running).expect("read").expect("row").ended_at,
            Some(9000)
        );
    }

    #[test]
    fn a_file_journal_survives_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("state.db");
        let trigger = json!({"n": 1});
        let id = {
            let j = Journal::open(&path).expect("open creates the parent");
            let id = j
                .create_run(&new_run("persist", &trigger), 5)
                .expect("create");
            j.begin_call(&call(id, 1, &json!("arg"))).expect("begin");
            j.finish_call(id, 1, &json!("out"), "done", None, 3)
                .expect("finish");
            id
        };
        let j = Journal::open(&path).expect("reopen");
        let row = j.run(id).expect("read").expect("row");
        assert_eq!(row.workflow, "persist");
        assert_eq!(row.trigger, trigger);
        assert_eq!(row.status, RunStatus::Running);
        let calls = j.calls(id).expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].result, Some(json!("out")));
    }

    #[test]
    fn a_file_journal_and_the_old_store_coexist_in_either_order() {
        for store_first in [true, false] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("state.db");
            let trigger = json!(null);
            let id = if store_first {
                let _store = Store::open(&path).expect("store");
                let j = Journal::open(&path).expect("journal");
                j.create_run(&new_run("both", &trigger), 1).expect("create")
            } else {
                let j = Journal::open(&path).expect("journal");
                let id = j.create_run(&new_run("both", &trigger), 1).expect("create");
                let _store = Store::open(&path).expect("store");
                id
            };
            let raw = Connection::open(&path).expect("raw");
            let user_version: i64 = raw
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("user_version");
            assert_eq!(user_version, 2, "the old store keeps its version");
            let version: String = raw
                .query_row(
                    "SELECT value FROM script_meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0),
                )
                .expect("own version");
            assert_eq!(version, SCHEMA_VERSION);
            drop(raw);

            let j = Journal::open(&path).expect("reopen journal");
            assert!(j.run(id).expect("read").is_some());
        }
    }

    #[test]
    fn a_journal_written_by_another_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        drop(Journal::open(&path).expect("create"));
        {
            let raw = Connection::open(&path).expect("raw");
            raw.execute(
                "UPDATE script_meta SET value = '9' WHERE key = 'schema_version'",
                [],
            )
            .expect("bump");
        }
        let error = Journal::open(&path).err().expect("refused");
        assert!(error.0.contains("version 9"), "{error}");
    }

    #[test]
    fn an_open_error_names_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write");
        let path = blocker.join("state.db");
        let error = Journal::open(&path).err().expect("refused");
        assert!(error.0.contains(&blocker.display().to_string()), "{error}");
    }

    #[test]
    fn concurrent_writers_lose_nothing() {
        let j = fresh();
        let handles: Vec<_> = (0..8_i64)
            .map(|t| {
                let j = j.clone();
                std::thread::spawn(move || {
                    let trigger = json!(null);
                    let id = j
                        .create_run(&new_run("parallel", &trigger), t)
                        .expect("create");
                    for seq in 1..=50_u32 {
                        let args = json!({"n": seq});
                        j.begin_call(&call(id, seq, &args)).expect("begin");
                        j.finish_call(id, seq, &json!(seq), "done", None, 1)
                            .expect("finish");
                    }
                    id
                })
            })
            .collect();
        let ids: Vec<RunId> = handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .collect();
        assert_eq!(j.runs("parallel", 100).expect("runs").len(), 8);
        for id in ids {
            let calls = j.calls(id).expect("calls");
            assert_eq!(calls.len(), 50);
            assert!(calls.iter().all(|c| c.outcome.as_deref() == Some("done")));
        }
    }

    #[test]
    fn json_is_stored_with_sorted_keys_whatever_the_order_it_was_built_in() {
        // With `preserve_order` (on in this crate's tests, and in a whole-workspace build) a map
        // keeps the order of insertion; the stored text must not depend on it.
        let j = fresh();
        let run = start(&j, "w", 1);
        let mut args = serde_json::Map::new();
        args.insert("name".into(), json!("x"));
        args.insert(
            "checkout".into(),
            json!({ "z": 1, "a": [{ "y": 2, "b": 3 }] }),
        );
        let args = Value::Object(args);
        j.begin_call(&NewCall {
            run,
            seq: 1,
            func: "branch.create",
            level: "apply",
            args: &args,
            started_at: 1,
        })
        .unwrap();
        let mut result = serde_json::Map::new();
        result.insert("text".into(), json!("t"));
        result.insert("exit_code".into(), json!(0));
        j.finish_call(run, 1, &Value::Object(result), "done", None, 1)
            .unwrap();
        let call = &j.calls(run).unwrap()[0];
        assert_eq!(
            call.args.to_string(),
            r#"{"checkout":{"a":[{"b":3,"y":2}],"z":1},"name":"x"}"#
        );
        assert_eq!(
            call.result.as_ref().unwrap().to_string(),
            r#"{"exit_code":0,"text":"t"}"#
        );
    }

    #[test]
    fn a_poisoned_lock_is_an_error() {
        let j = fresh();
        let shared = j.conn.clone();
        let handle = std::thread::spawn(move || {
            let _guard = shared.lock().expect("lock");
            panic!("poison the journal");
        });
        assert!(handle.join().is_err());
        assert!(j.run(1).is_err());
        assert!(j.create_run(&new_run("x", &json!(null)), 1).is_err());
    }

    #[test]
    fn an_unknown_status_in_the_database_is_an_error() {
        let j = fresh();
        let id = start(&j, "odd", 1);
        {
            let guard = j.lock().expect("lock");
            guard
                .execute(
                    "UPDATE script_run SET status = 'mystery' WHERE id = ?1",
                    params![id],
                )
                .expect("corrupt");
        }
        assert!(j.run(id).is_err());
        assert!(j.runs("odd", 10).is_err());
    }
}
