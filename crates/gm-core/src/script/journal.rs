//! The run journal (design/workflow-scripts.md, D80, D100): what the host observed of each run,
//! written by the host from the calls themselves, never from what a script says. It is history for
//! the Runs tab and the evidence a restart needs to mark a run `interrupted`; it is not a way to
//! resume (Q23). Lives in `state.db`, next to the tables of the TOML engine.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::Value;

/// The last runs kept per workflow (D80).
pub const RETAINED_RUNS: usize = 200;
/// A result larger than this is stored as its hash, its size and its first `RESULT_HEAD` bytes.
pub const RESULT_LIMIT: usize = 64 * 1024;
pub const RESULT_HEAD: usize = 4 * 1024;

pub type RunId = i64;

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
    pub fn open(_path: &Path) -> Result<Self, JournalError> {
        todo!()
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        todo!()
    }

    /// Adds a run with status `running` and prunes the workflow's runs beyond `RETAINED_RUNS`,
    /// oldest first, with their calls. Returns the new id.
    pub fn create_run(&self, _new: &NewRun<'_>, _now: i64) -> Result<RunId, JournalError> {
        todo!()
    }

    pub fn set_status(&self, _run: RunId, _status: RunStatus) -> Result<(), JournalError> {
        todo!()
    }

    /// Writes the start of a call. The row has no result until `finish_call`.
    pub fn begin_call(&self, _call: &NewCall<'_>) -> Result<(), JournalError> {
        todo!()
    }

    /// Writes the end of a call. `result` larger than `RESULT_LIMIT` bytes of JSON is stored as
    /// `{"truncated": true, "sha256": <hex>, "size": <bytes>, "head": <first RESULT_HEAD bytes,
    /// cut at a character boundary>}`.
    pub fn finish_call(
        &self,
        _run: RunId,
        _seq: u32,
        _result: &Value,
        _outcome: &str,
        _message: Option<&str>,
        _duration_ms: i64,
    ) -> Result<(), JournalError> {
        todo!()
    }

    /// Ends the run with a final status.
    pub fn finish_run(
        &self,
        _run: RunId,
        _status: RunStatus,
        _now: i64,
        _reason: Option<&str>,
        _value: Option<&Value>,
    ) -> Result<(), JournalError> {
        todo!()
    }

    pub fn run(&self, _run: RunId) -> Result<Option<RunRow>, JournalError> {
        todo!()
    }

    /// The newest runs of a workflow, newest first.
    pub fn runs(&self, _workflow: &str, _limit: usize) -> Result<Vec<RunRow>, JournalError> {
        todo!()
    }

    /// The calls of a run, in `seq` order.
    pub fn calls(&self, _run: RunId) -> Result<Vec<CallRow>, JournalError> {
        todo!()
    }

    /// For the next open after the app stopped (D100): every run still `running`,
    /// `awaiting_approval` or `waiting` becomes `interrupted` with `ended_at` = `now` and the
    /// reason `the app stopped while the run was in progress`, and every call of those runs without
    /// an outcome gets the outcome `interrupted`. Returns the runs it changed, oldest first.
    pub fn recover_interrupted(&self, _now: i64) -> Result<Vec<RunId>, JournalError> {
        todo!()
    }
}
