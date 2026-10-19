//! Where workflow runs live (docs/10, D79, D80): a SQLite file in the Git
//! common directory, `.git/gitmaster/state.db`. The workflow file holds the
//! definition; everything about execution is here. Each transition is one
//! SQLite transaction, so a crash resumes from the last recorded state.

use std::path::Path;

use serde_json::Value;

use super::machine::Record;

/// Runs kept per workflow; older ones are pruned (D80).
pub const RETAINED_RUNS: usize = 200;
/// Outputs larger than this are stored as hash, size and head (D80).
pub const OUTPUT_LIMIT: usize = 64 * 1024;
/// How much of a large output is kept.
pub const OUTPUT_HEAD: usize = 4 * 1024;

pub type RunId = i64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

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
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

pub struct Store {
    conn: rusqlite::Connection,
}

impl Store {
    /// Opens (creating it and its parent directory when missing) and
    /// migrates the database at `path`.
    pub fn open(_path: &Path) -> Result<Self, StoreError> {
        todo!("WF1: store::open")
    }

    /// A database that lives only in memory (tests, dry run).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        todo!("WF1: store::open_in_memory")
    }

    pub fn create_run(&mut self, _new: &NewRun<'_>, _now: i64) -> Result<RunId, StoreError> {
        todo!("WF1: store::create_run")
    }

    /// Records that the run is entering `state`, before its effect starts:
    /// a new output row with no output yet, and `current_state`.
    pub fn enter_state(&mut self, _run: RunId, _state: &str, _now: i64) -> Result<(), StoreError> {
        todo!("WF1: store::enter_state")
    }

    /// In ONE transaction: closes the open output row of `record.from` with
    /// `output`, appends the transition, saves `machine` and sets `status`.
    pub fn record_transition(
        &mut self,
        _run: RunId,
        _record: &Record,
        _output: &Value,
        _machine: &Value,
        _status: RunStatus,
        _now: i64,
    ) -> Result<(), StoreError> {
        todo!("WF1: store::record_transition")
    }

    /// Sets the status of a run that has not ended (waiting, awaiting approval).
    pub fn set_status(&mut self, _run: RunId, _status: RunStatus) -> Result<(), StoreError> {
        todo!("WF1: store::set_status")
    }

    /// Ends the run: final status, reason and `ended_at`.
    pub fn finish_run(
        &mut self,
        _run: RunId,
        _status: RunStatus,
        _reason: Option<&str>,
        _now: i64,
    ) -> Result<(), StoreError> {
        todo!("WF1: store::finish_run")
    }

    pub fn run(&self, _run: RunId) -> Result<Option<RunRow>, StoreError> {
        todo!("WF1: store::run")
    }

    /// Runs that have not ended (running, waiting, awaiting approval),
    /// oldest first: what the engine resumes after a restart.
    pub fn unfinished_runs(&self) -> Result<Vec<RunRow>, StoreError> {
        todo!("WF1: store::unfinished_runs")
    }

    /// The state a running run entered and never answered: the crash point.
    pub fn open_state(&self, _run: RunId) -> Result<Option<String>, StoreError> {
        todo!("WF1: store::open_state")
    }

    /// The last `limit` runs of `workflow`, newest first.
    pub fn runs(&self, _workflow: &str, _limit: usize) -> Result<Vec<RunRow>, StoreError> {
        todo!("WF1: store::runs")
    }

    pub fn transitions(&self, _run: RunId) -> Result<Vec<TransitionRow>, StoreError> {
        todo!("WF1: store::transitions")
    }

    pub fn outputs(&self, _run: RunId) -> Result<Vec<OutputRow>, StoreError> {
        todo!("WF1: store::outputs")
    }

    /// Keeps the newest `keep` ended runs of each workflow and deletes the
    /// rest with their transitions and outputs. Returns how many runs went.
    pub fn prune(&mut self, _keep: usize) -> Result<usize, StoreError> {
        todo!("WF1: store::prune")
    }
}
