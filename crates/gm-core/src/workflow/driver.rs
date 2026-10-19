//! Drives workflow runs (docs/10, D64, D79, D80): feeds outcomes to the pure
//! transition function, carries out each effect, and records every step in
//! the store before and after it runs. Quick actions run inline; `exec` runs
//! on a thread the caller starts (`ExecJob`); `approve` pauses for the human.
//! Git actions go through `GitActions`, so the driver is tested with a fake.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use super::exec::{ExecResult, ExecSpec};
use super::loader::Entry;
use super::machine::{self, Context};
use super::model::{Definition, End};
use super::store::{RunId, RunStatus, Store};

/// The Git side effects of WF1 actions. The engine implements it over its
/// `GitPort`; each call is one side effect and goes to the command log.
pub trait GitActions {
    /// `stage.all`: stages every change of the working tree.
    fn stage_all(&mut self) -> Result<(), String>;
    /// `commit.create`: commits the index; returns the new commit id.
    fn commit(&mut self, message: &str) -> Result<String, String>;
    /// `branch.create`.
    fn create_branch(&mut self, name: &str, checkout: bool) -> Result<(), String>;
    /// `branch.checkout`.
    fn checkout(&mut self, branch: &str) -> Result<(), String>;
}

/// What the front-ends hear about a run. The engine turns these into
/// `gm_schema::Event`s.
#[derive(Clone, Debug, PartialEq)]
pub enum RunEvent {
    /// The run moved: its status and the state it is in now.
    Changed {
        run: RunId,
        workflow: String,
        status: RunStatus,
        state: Option<String>,
    },
    /// `commit.suggest_message`: the composer shows `message` as a suggestion.
    SuggestMessage { run: RunId, message: String },
    /// `notify`: a line for the user.
    Notify { run: RunId, text: String },
    /// `approve` reached: the run waits for `approve` with `approved` true or false.
    AwaitingApproval {
        run: RunId,
        state: String,
        summary: String,
    },
    /// The run ended. `value` is the last `set` of a transform run.
    Finished {
        run: RunId,
        end: End,
        reason: Option<String>,
        value: Option<Value>,
    },
}

/// An `exec` step to run off the engine thread: `ExecJob::run` blocks until
/// the process ends; its report goes back to `Driver::exec_report`.
pub struct ExecJob {
    pub run: RunId,
    pub state: String,
    pub spec: ExecSpec,
    pub cancel: Arc<AtomicBool>,
}

pub struct ExecReport {
    pub run: RunId,
    pub state: String,
    pub result: ExecResult,
}

impl ExecJob {
    pub fn run(self) -> ExecReport {
        let result = super::exec::run(&self.spec, &self.cancel);
        ExecReport {
            run: self.run,
            state: self.state,
            result,
        }
    }
}

/// Events to broadcast and exec jobs to start, from one call.
#[derive(Default)]
pub struct Outcome {
    pub events: Vec<RunEvent>,
    pub jobs: Vec<ExecJob>,
}

/// A run in memory between steps.
struct Active {
    workflow: String,
    definition: Definition,
    context: Context,
    run: machine::Run,
    cancel: Arc<AtomicBool>,
}

pub struct Driver {
    store: Store,
    /// Where `exec` runs: the repository root.
    cwd: std::path::PathBuf,
    active: BTreeMap<RunId, Active>,
}

impl Driver {
    pub fn new(store: Store, cwd: std::path::PathBuf) -> Self {
        Self {
            store,
            cwd,
            active: BTreeMap::new(),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Starts a manual run of `entry` (D64(a): a human's manual run executes
    /// its apply steps). Refuses an entry that did not load. `repo` is the
    /// snapshot expressions read as `repo.*` (`root`, `branch`, `head`,
    /// `staged_diff`).
    pub fn start(
        &mut self,
        _entry: &Entry,
        _inputs: Value,
        _repo: Value,
        _git: &mut dyn GitActions,
        _now: i64,
    ) -> Result<(RunId, Outcome), String> {
        todo!("WF1: driver::start")
    }

    /// The answer of an `exec` job. A report for a run that is no longer
    /// active (cancelled) is dropped.
    pub fn exec_report(
        &mut self,
        _report: ExecReport,
        _git: &mut dyn GitActions,
        _now: i64,
    ) -> Outcome {
        todo!("WF1: driver::exec_report")
    }

    /// The human's answer to an `approve` state.
    pub fn approve(
        &mut self,
        _run: RunId,
        _approved: bool,
        _git: &mut dyn GitActions,
        _now: i64,
    ) -> Result<Outcome, String> {
        todo!("WF1: driver::approve")
    }

    /// Cancels an active run: kills its `exec` process, if any, and ends it
    /// `cancelled`.
    pub fn cancel(&mut self, _run: RunId, _now: i64) -> Result<Outcome, String> {
        todo!("WF1: driver::cancel")
    }

    /// After a restart: picks up every unfinished run from the store (D80).
    pub fn resume(&mut self, _git: &mut dyn GitActions, _now: i64) -> Outcome {
        todo!("WF1: driver::resume")
    }
}
