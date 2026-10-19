//! Where workflow runs live (docs/10, D79, D80): a SQLite file in the Git
//! common directory, `.git/gitmaster/state.db`. The workflow file holds the
//! definition; everything about execution is here. Each transition is one
//! SQLite transaction, so a crash resumes from the last recorded state.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::machine::Record;

/// Runs kept per workflow; older ones are pruned (D80).
pub const RETAINED_RUNS: usize = 200;
/// Outputs larger than this are stored as hash, size and head (D80).
pub const OUTPUT_LIMIT: usize = 64 * 1024;
/// How much of a large output is kept.
pub const OUTPUT_HEAD: usize = 4 * 1024;

/// Schema version written to `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 2;
/// How long a writer waits for another connection's lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

const SCHEMA: &str = "
CREATE TABLE workflow_run (
    id INTEGER PRIMARY KEY,
    workflow TEXT NOT NULL,
    definition_hash TEXT NOT NULL,
    definition_text TEXT NOT NULL,
    trigger_json TEXT NOT NULL,
    started_by TEXT NOT NULL,
    status TEXT NOT NULL,
    current_state TEXT,
    machine_json TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    end_reason TEXT
);
CREATE INDEX workflow_run_by_workflow ON workflow_run (workflow, id);

CREATE TABLE workflow_transition (
    run_id INTEGER NOT NULL REFERENCES workflow_run (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    from_state TEXT NOT NULL,
    outcome TEXT NOT NULL,
    to_state TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
);

CREATE TABLE workflow_state_output (
    run_id INTEGER NOT NULL REFERENCES workflow_run (id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    visit INTEGER NOT NULL,
    output_json TEXT,
    truncated INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    PRIMARY KEY (run_id, state, visit)
);

CREATE TABLE workflow_wait (
    run_id INTEGER NOT NULL REFERENCES workflow_run (id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    event TEXT NOT NULL,
    deadline INTEGER,
    PRIMARY KEY (run_id, state)
);
";

/// Version 1 to 2: each output row keeps the rendered input of its step.
const UPGRADE_1_TO_2: &str = "
ALTER TABLE workflow_state_output ADD COLUMN input_json TEXT;
";

/// Columns read back for a `RunRow`, in the order `raw_run` expects them.
const RUN_COLUMNS: &str = "id, workflow, definition_hash, definition_text, trigger_json, \
     started_by, status, current_state, machine_json, started_at, ended_at, end_reason";

pub type RunId = i64;

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

/// Stored as lowercase text: `running`, `waiting`, `awaiting_approval`,
/// `done`, `failed`, `blocked`, `cancelled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Waiting,
    AwaitingApproval,
    Done,
    Failed,
    Blocked,
    Cancelled,
}

/// What a new run starts with. `definition_text` is the file as it was when
/// the run started (D80: a run executes the version it started with).
#[derive(Clone, Debug, PartialEq)]
pub struct NewRun<'a> {
    pub workflow: &'a str,
    pub definition_hash: &'a str,
    pub definition_text: &'a str,
    pub trigger: &'a Value,
    /// `human`, `event:<name>` or `agent:<client>` (D64).
    pub started_by: &'a str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub id: RunId,
    pub workflow: String,
    pub definition_hash: String,
    pub definition_text: String,
    pub trigger: Value,
    pub started_by: String,
    pub status: RunStatus,
    /// Path of the state the run is in (`each/plan` inside a `for_each`).
    pub current_state: Option<String>,
    /// The serialized `machine::Run` after the last transition.
    pub machine: Option<Value>,
    /// Milliseconds since the Unix epoch, as the caller passed them.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionRow {
    /// 1, 2, 3… within the run.
    pub seq: u32,
    pub from_state: String,
    pub outcome: String,
    pub to_state: String,
    pub at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputRow {
    pub state: String,
    /// 1 for the first time the run entered the state, then 2, 3…
    pub visit: u32,
    /// `None` while the state has been entered and not answered yet.
    pub output: Option<Value>,
    /// The output was larger than `OUTPUT_LIMIT`: `output` is
    /// `{"sha256": .., "size": .., "head": ..}`.
    pub truncated: bool,
    /// What the step received, rendered. `None` for a state that takes no
    /// input, and for rows written before schema 2. Stored like `output`: an
    /// input above `OUTPUT_LIMIT` reads back as `{"sha256", "size", "head"}`.
    pub input: Option<Value>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating it and its parent directory when missing) and
    /// migrates the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError(format!("cannot create {}: {e}", parent.display())))?;
        }
        let mut conn = Connection::open(path)
            .map_err(|e| StoreError(format!("cannot open {}: {e}", path.display())))?;
        configure(&conn)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(StoreError::from)?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// A database that lives only in memory (tests, dry run).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory().map_err(StoreError::from)?;
        configure(&conn)?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn create_run(&mut self, new: &NewRun<'_>, now: i64) -> Result<RunId, StoreError> {
        let trigger = to_json(new.trigger)?;
        self.conn
            .execute(
                "INSERT INTO workflow_run
                    (workflow, definition_hash, definition_text, trigger_json,
                     started_by, status, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    new.workflow,
                    new.definition_hash,
                    new.definition_text,
                    trigger,
                    new.started_by,
                    RunStatus::Running.as_str(),
                    now,
                ],
            )
            .map_err(StoreError::from)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Records that the run is entering `state`, before its effect starts:
    /// a new output row with no output yet, the rendered `input` of the step
    /// (`None` for a state that takes none) and `current_state`.
    pub fn enter_state(
        &mut self,
        run: RunId,
        state: &str,
        input: Option<&Value>,
        now: i64,
    ) -> Result<(), StoreError> {
        let input_json = input
            .map(|value| stored_output(value).map(|(text, _)| text))
            .transpose()?;
        let tx = self.conn.transaction().map_err(StoreError::from)?;
        ensure_run(&tx, run)?;
        tx.execute(
            "INSERT INTO workflow_state_output
                (run_id, state, visit, output_json, truncated, started_at, input_json)
             VALUES (?1, ?2,
                (SELECT COALESCE(MAX(visit), 0) + 1 FROM workflow_state_output
                 WHERE run_id = ?1 AND state = ?2),
                NULL, 0, ?3, ?4)",
            params![run, state, now, input_json],
        )
        .map_err(StoreError::from)?;
        tx.execute(
            "UPDATE workflow_run SET current_state = ?2 WHERE id = ?1",
            params![run, state],
        )
        .map_err(StoreError::from)?;
        tx.commit().map_err(StoreError::from)
    }

    /// In ONE transaction: closes the open output row of `record.from` with
    /// `output`, appends the transition, saves `machine` and sets `status`.
    pub fn record_transition(
        &mut self,
        run: RunId,
        record: &Record,
        output: &Value,
        machine: &Value,
        status: RunStatus,
        now: i64,
    ) -> Result<(), StoreError> {
        let (output_json, truncated) = stored_output(output)?;
        let machine_json = to_json(machine)?;
        let current_state = if record.to.starts_with("end.") {
            None
        } else {
            Some(record.to.as_str())
        };

        let tx = self.conn.transaction().map_err(StoreError::from)?;
        ensure_run(&tx, run)?;

        let open_visit: Option<u32> = tx
            .query_row(
                "SELECT visit FROM workflow_state_output
                 WHERE run_id = ?1 AND state = ?2 AND ended_at IS NULL
                 ORDER BY visit DESC LIMIT 1",
                params![run, record.from],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)?;
        match open_visit {
            Some(visit) => {
                tx.execute(
                    "UPDATE workflow_state_output
                     SET output_json = ?3, truncated = ?4, ended_at = ?5
                     WHERE run_id = ?1 AND state = ?2 AND visit = ?6",
                    params![run, record.from, output_json, truncated, now, visit],
                )
                .map_err(StoreError::from)?;
            }
            None => {
                tx.execute(
                    "INSERT INTO workflow_state_output
                        (run_id, state, visit, output_json, truncated, started_at, ended_at)
                     VALUES (?1, ?2,
                        (SELECT COALESCE(MAX(visit), 0) + 1 FROM workflow_state_output
                         WHERE run_id = ?1 AND state = ?2),
                        ?3, ?4, ?5, ?5)",
                    params![run, record.from, output_json, truncated, now],
                )
                .map_err(StoreError::from)?;
            }
        }

        let seq: u32 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM workflow_transition WHERE run_id = ?1",
                params![run],
                |row| row.get(0),
            )
            .map_err(StoreError::from)?;
        tx.execute(
            "INSERT INTO workflow_transition
                (run_id, seq, from_state, outcome, to_state, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![run, seq, record.from, record.outcome, record.to, now],
        )
        .map_err(StoreError::from)?;

        tx.execute(
            "UPDATE workflow_run
             SET current_state = ?2, machine_json = ?3, status = ?4
             WHERE id = ?1",
            params![run, current_state, machine_json, status.as_str()],
        )
        .map_err(StoreError::from)?;
        tx.commit().map_err(StoreError::from)
    }

    /// Sets the status of a run that has not ended (waiting, awaiting approval).
    pub fn set_status(&mut self, run: RunId, status: RunStatus) -> Result<(), StoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE workflow_run SET status = ?2 WHERE id = ?1 AND ended_at IS NULL",
                params![run, status.as_str()],
            )
            .map_err(StoreError::from)?;
        if changed == 0 {
            return Err(StoreError(format!("run {run} has ended or does not exist")));
        }
        Ok(())
    }

    /// Ends the run: final status, reason and `ended_at`.
    pub fn finish_run(
        &mut self,
        run: RunId,
        status: RunStatus,
        reason: Option<&str>,
        now: i64,
    ) -> Result<(), StoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE workflow_run
                 SET status = ?2, ended_at = ?3, end_reason = ?4, current_state = NULL
                 WHERE id = ?1",
                params![run, status.as_str(), now, reason],
            )
            .map_err(StoreError::from)?;
        if changed == 0 {
            return Err(StoreError(format!("no run {run}")));
        }
        Ok(())
    }

    pub fn run(&self, run: RunId) -> Result<Option<RunRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM workflow_run WHERE id = ?1"),
                params![run],
                raw_run,
            )
            .optional()
            .map_err(StoreError::from)?;
        raw.map(RawRun::into_row).transpose()
    }

    /// Runs that have not ended (running, waiting, awaiting approval),
    /// oldest first: what the engine resumes after a restart.
    pub fn unfinished_runs(&self) -> Result<Vec<RunRow>, StoreError> {
        read_runs(
            &self.conn,
            &format!(
                "SELECT {RUN_COLUMNS} FROM workflow_run
                 WHERE status IN ('running', 'waiting', 'awaiting_approval')
                 ORDER BY started_at, id"
            ),
            params![],
        )
    }

    /// The state a running run entered and never answered: the crash point.
    pub fn open_state(&self, run: RunId) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT state FROM workflow_state_output
                 WHERE run_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC, visit DESC LIMIT 1",
                params![run],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// The last `limit` runs of `workflow`, newest first.
    pub fn runs(&self, workflow: &str, limit: usize) -> Result<Vec<RunRow>, StoreError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        read_runs(
            &self.conn,
            &format!(
                "SELECT {RUN_COLUMNS} FROM workflow_run
                 WHERE workflow = ?1 ORDER BY id DESC LIMIT ?2"
            ),
            params![workflow, limit],
        )
    }

    pub fn transitions(&self, run: RunId) -> Result<Vec<TransitionRow>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, from_state, outcome, to_state, at FROM workflow_transition
                 WHERE run_id = ?1 ORDER BY seq",
            )
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![run], |row| {
                Ok(TransitionRow {
                    seq: row.get(0)?,
                    from_state: row.get(1)?,
                    outcome: row.get(2)?,
                    to_state: row.get(3)?,
                    at: row.get(4)?,
                })
            })
            .map_err(StoreError::from)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    pub fn outputs(&self, run: RunId) -> Result<Vec<OutputRow>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT state, visit, output_json, truncated, started_at, ended_at, input_json
                 FROM workflow_state_output
                 WHERE run_id = ?1 ORDER BY started_at, visit",
            )
            .map_err(StoreError::from)?;
        let raw = stmt
            .query_map(params![run], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(StoreError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)?;
        raw.into_iter()
            .map(
                |(state, visit, output, truncated, started_at, ended_at, input)| -> Result<OutputRow, StoreError> {
                    Ok(OutputRow {
                        state,
                        visit,
                        output: output.as_deref().map(parse_json).transpose()?,
                        truncated,
                        input: input.as_deref().map(parse_json).transpose()?,
                        started_at,
                        ended_at,
                    })
                },
            )
            .collect()
    }

    /// Keeps the newest `keep` ended runs of each workflow and deletes the
    /// rest with their transitions and outputs. Returns how many runs went.
    pub fn prune(&mut self, keep: usize) -> Result<usize, StoreError> {
        let keep = i64::try_from(keep).unwrap_or(i64::MAX);
        self.conn
            .execute(
                "DELETE FROM workflow_run
                 WHERE ended_at IS NOT NULL AND id IN (
                    SELECT id FROM (
                        SELECT id, ROW_NUMBER() OVER (PARTITION BY workflow ORDER BY id DESC) AS place
                        FROM workflow_run
                        WHERE ended_at IS NOT NULL
                    ) WHERE place > ?1
                 )",
                params![keep],
            )
            .map_err(StoreError::from)
    }
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_text(text: &str) -> Result<Self, StoreError> {
        match text {
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "awaiting_approval" => Ok(Self::AwaitingApproval),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            "blocked" => Ok(Self::Blocked),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StoreError(format!(
                "unknown run status `{other}` in state.db"
            ))),
        }
    }
}

fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT).map_err(StoreError::from)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(StoreError::from)
}

/// Creates the schema of an empty database, upgrades an older one, each in one
/// transaction; refuses one written by a newer version of the app.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StoreError::from)?;
    match version {
        0 | 1 => {
            let tx = conn.transaction().map_err(StoreError::from)?;
            if version == 0 {
                tx.execute_batch(SCHEMA).map_err(StoreError::from)?;
            }
            tx.execute_batch(UPGRADE_1_TO_2).map_err(StoreError::from)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(StoreError::from)?;
            tx.commit().map_err(StoreError::from)
        }
        v if v == SCHEMA_VERSION => Ok(()),
        v if v > SCHEMA_VERSION => Err(StoreError(
            "state.db was written by a newer GitMaster".to_string(),
        )),
        v => Err(StoreError(format!(
            "state.db has unknown schema version {v}"
        ))),
    }
}

fn ensure_run(conn: &Connection, run: RunId) -> Result<(), StoreError> {
    let found: Option<RunId> = conn
        .query_row(
            "SELECT id FROM workflow_run WHERE id = ?1",
            params![run],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)?;
    if found.is_none() {
        return Err(StoreError(format!("no run {run}")));
    }
    Ok(())
}

/// The run columns as the database holds them; decoded by `into_row`.
struct RawRun {
    id: RunId,
    workflow: String,
    definition_hash: String,
    definition_text: String,
    trigger_json: String,
    started_by: String,
    status: String,
    current_state: Option<String>,
    machine_json: Option<String>,
    started_at: i64,
    ended_at: Option<i64>,
    end_reason: Option<String>,
}

fn raw_run(row: &Row<'_>) -> rusqlite::Result<RawRun> {
    Ok(RawRun {
        id: row.get(0)?,
        workflow: row.get(1)?,
        definition_hash: row.get(2)?,
        definition_text: row.get(3)?,
        trigger_json: row.get(4)?,
        started_by: row.get(5)?,
        status: row.get(6)?,
        current_state: row.get(7)?,
        machine_json: row.get(8)?,
        started_at: row.get(9)?,
        ended_at: row.get(10)?,
        end_reason: row.get(11)?,
    })
}

impl RawRun {
    fn into_row(self) -> Result<RunRow, StoreError> {
        Ok(RunRow {
            id: self.id,
            workflow: self.workflow,
            definition_hash: self.definition_hash,
            definition_text: self.definition_text,
            trigger: parse_json(&self.trigger_json)?,
            started_by: self.started_by,
            status: RunStatus::from_text(&self.status)?,
            current_state: self.current_state,
            machine: self.machine_json.as_deref().map(parse_json).transpose()?,
            started_at: self.started_at,
            ended_at: self.ended_at,
            end_reason: self.end_reason,
        })
    }
}

fn read_runs<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<RunRow>, StoreError> {
    let mut stmt = conn.prepare(sql).map_err(StoreError::from)?;
    let raw = stmt
        .query_map(params, raw_run)
        .map_err(StoreError::from)?
        .collect::<rusqlite::Result<Vec<RawRun>>>()
        .map_err(StoreError::from)?;
    raw.into_iter().map(RawRun::into_row).collect()
}

/// The output (or input) as stored: the value itself, or, above `OUTPUT_LIMIT`,
/// its hash, size and head. The flag tells which one it is.
fn stored_output(output: &Value) -> Result<(String, bool), StoreError> {
    let text = to_json(output)?;
    if text.len() <= OUTPUT_LIMIT {
        return Ok((text, false));
    }
    let digest = Sha256::digest(text.as_bytes());
    let wrapper = json!({
        "sha256": hex(&digest),
        "size": text.len(),
        "head": prefix(&text, OUTPUT_HEAD),
    });
    Ok((to_json(&wrapper)?, true))
}

/// The first `limit` bytes of `text`, cut back to a char boundary.
pub(crate) fn prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn to_json(value: &Value) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError(format!("cannot encode JSON: {e}")))
}

fn parse_json(text: &str) -> Result<Value, StoreError> {
    serde_json::from_str(text)
        .map_err(|e| StoreError(format!("cannot decode JSON in state.db: {e}")))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;

    fn start(store: &mut Store, workflow: &str) -> RunId {
        let trigger = json!({"kind": "manual"});
        store
            .create_run(
                &NewRun {
                    workflow,
                    definition_hash: "hash-1",
                    definition_text: "[workflow]",
                    trigger: &trigger,
                    started_by: "human",
                },
                1000,
            )
            .expect("create run")
    }

    fn record(from: &str, outcome: &str, to: &str) -> Record {
        Record {
            from: from.to_string(),
            outcome: outcome.to_string(),
            to: to.to_string(),
        }
    }

    fn count(store: &Store, table: &str) -> i64 {
        store
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count")
    }

    #[test]
    fn round_trip_from_start_to_finish() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "lint");

        let row = store.run(run).expect("read").expect("exists");
        assert_eq!(row.status, RunStatus::Running);
        assert_eq!(row.current_state, None);
        assert_eq!(row.trigger, json!({"kind": "manual"}));
        assert_eq!(row.definition_text, "[workflow]");

        store
            .enter_state(run, "a", Some(&json!({"text": "x"})), 1100)
            .expect("enter a");
        store
            .record_transition(
                run,
                &record("a", "ok", "b"),
                &json!({"n": 1}),
                &json!({"at": "b"}),
                RunStatus::Running,
                1200,
            )
            .expect("a to b");
        let row = store.run(run).expect("read").expect("exists");
        assert_eq!(row.current_state.as_deref(), Some("b"));

        store.enter_state(run, "b", None, 1300).expect("enter b");
        store
            .record_transition(
                run,
                &record("b", "ok", "end.done"),
                &json!("fine"),
                &json!({"end": true}),
                RunStatus::Done,
                1400,
            )
            .expect("b to end");
        store
            .finish_run(run, RunStatus::Done, None, 1500)
            .expect("finish");

        let row = store.run(run).expect("read").expect("exists");
        assert_eq!(row.status, RunStatus::Done);
        assert_eq!(row.current_state, None);
        assert_eq!(row.ended_at, Some(1500));
        assert_eq!(row.started_at, 1000);
        assert_eq!(row.machine, Some(json!({"end": true})));

        let transitions = store.transitions(run).expect("transitions");
        assert_eq!(
            transitions,
            vec![
                TransitionRow {
                    seq: 1,
                    from_state: "a".into(),
                    outcome: "ok".into(),
                    to_state: "b".into(),
                    at: 1200,
                },
                TransitionRow {
                    seq: 2,
                    from_state: "b".into(),
                    outcome: "ok".into(),
                    to_state: "end.done".into(),
                    at: 1400,
                },
            ]
        );

        let outputs = store.outputs(run).expect("outputs");
        assert_eq!(
            outputs,
            vec![
                OutputRow {
                    state: "a".into(),
                    visit: 1,
                    output: Some(json!({"n": 1})),
                    truncated: false,
                    input: Some(json!({"text": "x"})),
                    started_at: 1100,
                    ended_at: Some(1200),
                },
                OutputRow {
                    state: "b".into(),
                    visit: 1,
                    output: Some(json!("fine")),
                    truncated: false,
                    input: None,
                    started_at: 1300,
                    ended_at: Some(1400),
                },
            ]
        );
        assert!(store.unfinished_runs().expect("list").is_empty());
    }

    #[test]
    fn each_entry_into_a_state_is_a_new_visit() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "loop");

        for visit in 1..=3 {
            store
                .enter_state(run, "a", None, 1000 + visit)
                .expect("enter a");
            assert_eq!(store.open_state(run).expect("open"), Some("a".to_string()));
            store
                .record_transition(
                    run,
                    &record("a", "again", "a"),
                    &json!(visit),
                    &json!({}),
                    RunStatus::Running,
                    2000 + visit,
                )
                .expect("a to a");
        }

        let visits: Vec<u32> = store
            .outputs(run)
            .expect("outputs")
            .iter()
            .map(|row| row.visit)
            .collect();
        assert_eq!(visits, vec![1, 2, 3]);
        assert_eq!(store.open_state(run).expect("open"), None);
    }

    #[test]
    fn large_output_is_stored_as_hash_size_and_head() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "big");
        let output = json!({ "text": "x".repeat(100 * 1024) });
        let text = serde_json::to_string(&output).expect("encode");

        store.enter_state(run, "a", None, 1100).expect("enter");
        store
            .record_transition(
                run,
                &record("a", "ok", "end.done"),
                &output,
                &json!({}),
                RunStatus::Done,
                1200,
            )
            .expect("record");

        let rows = store.outputs(run).expect("outputs");
        assert!(rows[0].truncated);
        let stored = rows[0].output.clone().expect("output");
        assert_eq!(
            stored["sha256"],
            json!(hex(&Sha256::digest(text.as_bytes())))
        );
        assert_eq!(stored["size"], json!(text.len()));
        let head = stored["head"].as_str().expect("head");
        assert!(head.len() <= OUTPUT_HEAD);
        assert!(text.starts_with(head));
    }

    #[test]
    fn large_input_is_stored_as_hash_size_and_head() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "big-input");
        let input = json!({ "text": "y".repeat(100 * 1024) });
        let text = serde_json::to_string(&input).expect("encode");

        store
            .enter_state(run, "a", Some(&input), 1100)
            .expect("enter");

        let rows = store.outputs(run).expect("outputs");
        assert!(!rows[0].truncated, "the flag is the output's");
        let stored = rows[0].input.clone().expect("input");
        assert_eq!(
            stored["sha256"],
            json!(hex(&Sha256::digest(text.as_bytes())))
        );
        assert_eq!(stored["size"], json!(text.len()));
        assert!(text.starts_with(stored["head"].as_str().expect("head")));
    }

    #[test]
    fn large_multibyte_output_is_cut_at_a_char_boundary() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "accents");
        let output = json!({ "text": "é".repeat(60 * 1024) });
        let text = serde_json::to_string(&output).expect("encode");

        store.enter_state(run, "a", None, 1100).expect("enter");
        store
            .record_transition(
                run,
                &record("a", "ok", "end.done"),
                &output,
                &json!({}),
                RunStatus::Done,
                1200,
            )
            .expect("record");

        let rows = store.outputs(run).expect("outputs");
        let stored = rows[0].output.clone().expect("output");
        let head = stored["head"].as_str().expect("head");
        assert!(head.len() <= OUTPUT_HEAD);
        assert!(text.starts_with(head));
    }

    #[test]
    fn crash_after_entering_a_state_is_visible_after_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("state.db");
        {
            let mut store = Store::open(&path).expect("open creates the parent");
            let run = start(&mut store, "sync");
            store.enter_state(run, "apply", None, 2000).expect("enter");
            // Dropped here without recording: the process went away.
        }

        let store = Store::open(&path).expect("reopen");
        let mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(mode, "wal");

        let unfinished = store.unfinished_runs().expect("list");
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].workflow, "sync");
        assert_eq!(unfinished[0].current_state.as_deref(), Some("apply"));
        assert_eq!(
            store.open_state(unfinished[0].id).expect("open state"),
            Some("apply".to_string())
        );
    }

    #[test]
    fn a_failed_transition_leaves_nothing_behind() {
        let mut store = Store::open_in_memory().expect("open");
        let run = start(&mut store, "atomic");
        store.enter_state(run, "a", None, 1100).expect("enter");
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER fail_transition BEFORE INSERT ON workflow_transition
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
            )
            .expect("trigger");

        let result = store.record_transition(
            run,
            &record("a", "ok", "b"),
            &json!({"n": 1}),
            &json!({"at": "b"}),
            RunStatus::Done,
            1200,
        );
        assert!(result.is_err());

        assert_eq!(store.open_state(run).expect("open"), Some("a".to_string()));
        assert_eq!(count(&store, "workflow_transition"), 0);
        let row = store.run(run).expect("read").expect("exists");
        assert_eq!(row.status, RunStatus::Running);
        assert_eq!(row.current_state.as_deref(), Some("a"));
        assert_eq!(row.machine, None);
    }

    #[test]
    fn recording_for_a_missing_run_writes_nothing() {
        let mut store = Store::open_in_memory().expect("open");
        let result = store.record_transition(
            999,
            &record("a", "ok", "b"),
            &json!({}),
            &json!({}),
            RunStatus::Running,
            1,
        );
        assert_eq!(result, Err(StoreError("no run 999".to_string())));
        assert_eq!(count(&store, "workflow_state_output"), 0);
        assert_eq!(count(&store, "workflow_transition"), 0);
    }

    #[test]
    fn statuses_round_trip_and_ended_runs_refuse_set_status() {
        let mut store = Store::open_in_memory().expect("open");
        let first = start(&mut store, "wf");
        let second = start(&mut store, "wf");

        store.set_status(first, RunStatus::Waiting).expect("wait");
        let unfinished = store.unfinished_runs().expect("list");
        assert_eq!(unfinished.len(), 2);
        assert_eq!(unfinished[0].status, RunStatus::Waiting);

        store
            .set_status(first, RunStatus::AwaitingApproval)
            .expect("approval");
        store
            .finish_run(first, RunStatus::Cancelled, Some("user cancelled"), 1500)
            .expect("finish");
        let row = store.run(first).expect("read").expect("exists");
        assert_eq!(row.status, RunStatus::Cancelled);
        assert_eq!(row.end_reason.as_deref(), Some("user cancelled"));
        assert!(store.set_status(first, RunStatus::Running).is_err());

        let listed = store.runs("wf", 1).expect("runs");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, second);
        assert_eq!(store.runs("wf", 10).expect("runs")[1].id, first);

        for status in [
            RunStatus::Running,
            RunStatus::Waiting,
            RunStatus::AwaitingApproval,
            RunStatus::Done,
            RunStatus::Failed,
            RunStatus::Blocked,
            RunStatus::Cancelled,
        ] {
            assert_eq!(RunStatus::from_text(status.as_str()), Ok(status));
        }
        assert!(RunStatus::from_text("nope").is_err());
    }

    #[test]
    fn prune_keeps_the_newest_ended_runs_and_never_unfinished_ones() {
        let mut store = Store::open_in_memory().expect("open");
        let unfinished: Vec<RunId> = (0..3).map(|_| start(&mut store, "daily")).collect();
        let other = start(&mut store, "other");
        store
            .finish_run(other, RunStatus::Done, None, 3000)
            .expect("finish other");

        let first_ended = store.conn.last_insert_rowid() + 1;
        for i in 0..205 {
            let id = start(&mut store, "daily");
            if i == 0 {
                store.enter_state(id, "a", None, 1100).expect("enter");
                store
                    .record_transition(
                        id,
                        &record("a", "ok", "end.done"),
                        &json!({}),
                        &json!({}),
                        RunStatus::Done,
                        1200,
                    )
                    .expect("record");
            }
            store
                .finish_run(id, RunStatus::Done, None, 3000)
                .expect("finish");
        }

        let removed = store.prune(RETAINED_RUNS).expect("prune");
        assert_eq!(removed, 5);

        let daily = store.runs("daily", 1000).expect("runs");
        assert_eq!(daily.len(), 203);
        assert_eq!(
            daily.iter().filter(|row| row.ended_at.is_some()).count(),
            RETAINED_RUNS
        );
        for id in &unfinished {
            let row = store.run(*id).expect("read").expect("kept");
            assert_eq!(row.status, RunStatus::Running);
        }
        assert!(store.run(first_ended).expect("read").is_none());
        assert!(store.run(first_ended + 4).expect("read").is_none());
        assert!(store.run(first_ended + 5).expect("read").is_some());
        assert_eq!(store.runs("other", 10).expect("runs").len(), 1);
        assert!(store.run(other).expect("read").is_some());
        assert_eq!(count(&store, "workflow_transition"), 0);
    }

    fn user_version(store: &Store) -> i64 {
        store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version")
    }

    #[test]
    fn a_fresh_database_is_created_at_the_current_version() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(user_version(&store), 2);
        let columns: Vec<String> = store
            .conn
            .prepare("SELECT name FROM pragma_table_info('workflow_state_output')")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("columns");
        assert!(columns.contains(&"input_json".to_string()));
    }

    #[test]
    fn a_version_1_database_is_upgraded_and_keeps_its_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            // Built by hand, as schema 1 wrote it.
            let conn = Connection::open(&path).expect("raw open");
            conn.execute_batch(SCHEMA).expect("schema 1");
            conn.pragma_update(None, "user_version", 1)
                .expect("version 1");
            conn.execute_batch(
                "INSERT INTO workflow_run
                    (id, workflow, definition_hash, definition_text, trigger_json,
                     started_by, status, started_at, ended_at)
                 VALUES (1, 'old', 'h', '[workflow]', '{}', 'human', 'done', 1000, 1500);
                 INSERT INTO workflow_state_output
                    (run_id, state, visit, output_json, truncated, started_at, ended_at)
                 VALUES (1, 'a', 1, '{\"n\":1}', 0, 1100, 1200);",
            )
            .expect("rows of schema 1");
        }

        let store = Store::open(&path).expect("upgrade");
        assert_eq!(user_version(&store), 2);
        let rows = store.outputs(1).expect("outputs");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].output, Some(json!({"n": 1})));
        assert_eq!(rows[0].input, None);
        assert_eq!(store.run(1).expect("read").expect("run").workflow, "old");
        drop(store);

        // Reopening a current database changes nothing.
        let reopened = Store::open(&path).expect("reopen");
        assert_eq!(user_version(&reopened), 2);
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        Store::open(&path).expect("create");
        {
            let conn = Connection::open(&path).expect("raw open");
            conn.pragma_update(None, "user_version", 3).expect("bump");
        }
        let error = Store::open(&path).err().expect("refused");
        assert_eq!(
            error.0,
            "state.db was written by a newer GitMaster".to_string()
        );
    }
}
