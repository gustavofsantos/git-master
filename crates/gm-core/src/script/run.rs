//! The host side of a run (design/workflow-scripts.md, D99, D100, D101): one thread per run, the
//! journal written from the calls themselves, the approval rules, and the blocking calls
//! (`approve`, `wait`, `exec`) that a cancel wakes at once. The VM sits behind `ScriptEngine`, and
//! every effect other than `notify`, `exec` and the helpers goes to `Effects`, which the engine
//! implements over its own Commands, so no second path to Git exists.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use gm_schema::RestackPlan;
use serde_json::{Value, json};

use super::journal::{Journal, NewCall, NewRun, RunId, RunStatus};
use super::policy::{AskReason, Gate, StartedBy, gate_apply};
use super::port::{
    Budgets, CtxHost, Failure, HostEnd, HostReply, RunEnd, RunRequest, ScriptEngine,
};
use super::surface::{CtxFn, CtxSurface};
use crate::script::catalog::Level;
use crate::script::exec;

/// Milliseconds since the Unix epoch. A function so tests and `ctx.now()` can use a fixed clock.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Where a run's events go. Called on the run's thread, so it must not block.
pub type EventSink = Arc<dyn Fn(ScriptEvent) + Send + Sync>;

/// What the front ends hear about a run. The engine turns these into `gm_schema::Event`s.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptEvent {
    Changed {
        run: RunId,
        workflow: String,
        status: RunStatus,
    },
    /// `ctx.notify`. Marked as coming from the script in the Runs tab.
    Notify { run: RunId, text: String },
    /// `ctx.commit.suggest_message`: the composer shows `message` as a suggestion.
    SuggestMessage { run: RunId, message: String },
    /// The run reached an applying call, before any approval of it (`WF3a`): a trigger may no
    /// longer cancel it. Not shown to the person.
    Applying { run: RunId },
    /// The run waits for `Runtime::approve`. `summary` is what the person reads; `args` are the
    /// resolved arguments of the call (D101). `plan` is the restack an apply of a restack or a
    /// reparent would move, so the person sees what they approve (`WF4c`).
    AwaitingApproval {
        run: RunId,
        seq: u32,
        func: String,
        summary: String,
        args: Value,
        reason: Option<AskReason>,
        plan: Option<RestackPlan>,
    },
    Finished {
        run: RunId,
        workflow: String,
        status: RunStatus,
        reason: Option<String>,
        /// What `execute` returned; the field value of a `transform` run.
        value: Option<Value>,
    },
}

/// One call that changes or asks the engine for something: the effects of `ctx`.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectCall {
    pub run: RunId,
    /// The catalog name, such as `commit.create`.
    pub func: String,
    pub args: Value,
    pub level: Level,
}

/// How an effect ended. `outcome` is one of the outcomes the catalog declares for the action.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectResult {
    pub outcome: String,
    pub value: Value,
    /// Why, for an outcome other than `done`.
    pub message: String,
}

impl EffectResult {
    /// The answer to an effect posted by a run that was cancelled meanwhile: nothing was written.
    #[must_use]
    pub fn cancelled() -> Self {
        Self {
            outcome: "cancelled".to_owned(),
            value: Value::Null,
            message: "cancelled".to_owned(),
        }
    }

    #[must_use]
    pub fn done(value: Value) -> Self {
        Self {
            outcome: "done".to_owned(),
            value,
            message: String::new(),
        }
    }

    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            outcome: "failed".to_owned(),
            value: Value::Null,
            message: message.into(),
        }
    }
}

/// Carries out the effects of a run. Called on the run's thread, and it may block until the
/// engine thread has done the work. The engine's implementation sends the call to the engine
/// thread, where it runs as a Command (D81).
pub trait Effects: Send + Sync {
    fn effect(&self, call: &EffectCall) -> EffectResult;
}

/// Everything a run needs from its caller.
#[derive(Clone, Debug)]
pub struct RunSpec {
    pub workflow: String,
    /// What errors cite: the file name.
    pub chunk: String,
    pub source: String,
    pub definition_hash: String,
    pub started_by: StartedBy,
    pub trigger: Value,
    /// The `input` value: `inputs`, `trigger` and `repo`.
    pub input: Value,
    /// The person has reviewed this version of the script (D101). A run of a version that was not
    /// reviewed does not start.
    pub reviewed: bool,
    /// The grant of automatic approval for event runs, tied to the reviewed hash (D101).
    pub auto_approve: bool,
    /// The seed of `ctx.random()`.
    pub seed: u64,
}

/// What the runtime needs besides the engine and the journal.
pub struct RuntimeConfig {
    pub engine: Arc<dyn ScriptEngine>,
    pub journal: Journal,
    pub effects: Arc<dyn Effects>,
    pub events: EventSink,
    pub clock: Clock,
    /// Where `exec` runs: the repository root.
    pub cwd: PathBuf,
    /// The login-shell PATH `exec` runs with; empty until it is read.
    pub login_path: Arc<OnceLock<String>>,
    pub budgets: Budgets,
}

/// An answer to a blocked call.
enum Signal {
    Approve(bool),
    /// An event a wait may be listening for, with how it ended (`resumed` or `aborted`) and the
    /// fields the wait returns beside `outcome`.
    Event {
        event: String,
        outcome: String,
        payload: Value,
    },
    Cancel,
}

/// What a blocked run waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Nothing,
    Approval,
    Wait(String),
}

struct Control {
    cancel: Arc<AtomicBool>,
    signals: mpsc::Sender<Signal>,
    pending: Pending,
}

struct Inner {
    engine: Arc<dyn ScriptEngine>,
    surface: CtxSurface,
    journal: Journal,
    effects: Arc<dyn Effects>,
    events: EventSink,
    clock: Clock,
    cwd: PathBuf,
    login_path: Arc<OnceLock<String>>,
    budgets: Budgets,
    controls: Mutex<HashMap<RunId, Control>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

/// Starts runs and answers the people and events that wake them. Cloneable.
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

impl Runtime {
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                engine: config.engine,
                surface: CtxSurface::from_catalog(),
                journal: config.journal,
                effects: config.effects,
                events: config.events,
                clock: config.clock,
                cwd: config.cwd,
                login_path: config.login_path,
                budgets: config.budgets,
                controls: Mutex::new(HashMap::new()),
                handles: Mutex::new(Vec::new()),
            }),
        }
    }

    /// For the open after the app stopped (D100): marks the runs left unfinished `interrupted`.
    /// Call it once, before the first `start`.
    pub fn recover(&self) -> Vec<RunId> {
        let now = (self.inner.clock)();
        let runs = self
            .inner
            .journal
            .recover_interrupted(now)
            .unwrap_or_default();
        for run in &runs {
            if let Ok(Some(row)) = self.inner.journal.run(*run) {
                self.inner.emit(ScriptEvent::Changed {
                    run: *run,
                    workflow: row.workflow,
                    status: RunStatus::Interrupted,
                });
            }
        }
        runs
    }

    /// Starts a run on its own thread and returns its id at once.
    pub fn start(&self, spec: RunSpec) -> Result<RunId, String> {
        if !spec.reviewed {
            return Err("this version of the script has not been reviewed".to_owned());
        }
        let inner = &self.inner;
        let now = (inner.clock)();
        let id = inner
            .journal
            .create_run(
                &NewRun {
                    workflow: &spec.workflow,
                    definition_hash: &spec.definition_hash,
                    definition_text: &spec.source,
                    trigger: &spec.trigger,
                    input: &spec.input,
                    started_by: &spec.started_by.label(),
                    seed: spec.seed,
                },
                now,
            )
            .map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        inner
            .controls
            .lock()
            .map_err(|_| "the run table is poisoned".to_owned())?
            .insert(
                id,
                Control {
                    cancel: Arc::clone(&cancel),
                    signals: tx,
                    pending: Pending::Nothing,
                },
            );
        inner.emit(ScriptEvent::Changed {
            run: id,
            workflow: spec.workflow.clone(),
            status: RunStatus::Running,
        });
        let inner_for_thread = Arc::clone(inner);
        let handle = std::thread::Builder::new()
            .name(format!("gm-script-{id}"))
            .spawn(move || run_thread(&inner_for_thread, id, spec, &cancel, &rx))
            .map_err(|e| format!("cannot start the run thread: {e}"))?;
        if let Ok(mut handles) = inner.handles.lock() {
            handles.retain(|h| !h.is_finished());
            handles.push(handle);
        }
        Ok(id)
    }

    /// Answers the approval a run waits for. `false` when the run is not waiting for one.
    pub fn approve(&self, run: RunId, approved: bool) -> bool {
        self.inner
            .signal(run, &Pending::Approval, Signal::Approve(approved))
    }

    /// Whether a cancel was asked for the run. `false` once the run has ended.
    #[must_use]
    pub fn is_cancelling(&self, run: RunId) -> bool {
        self.inner.controls.lock().is_ok_and(|controls| {
            controls
                .get(&run)
                .is_some_and(|control| control.cancel.load(Ordering::SeqCst))
        })
    }

    /// The branch a run's trigger names (`input.trigger.branch`), `None` when it names none or the
    /// run is not in the journal. `ctx.scorecards` reads its default branch from it (D219).
    #[must_use]
    pub fn trigger_branch(&self, run: RunId) -> Option<String> {
        let row = self.inner.journal.run(run).ok()??;
        row.trigger.get("branch")?.as_str().map(str::to_owned)
    }

    /// Cancels a run: stops the script, wakes a blocked call and kills the process of an `exec`.
    /// `false` when the run is not active.
    pub fn cancel(&self, run: RunId) -> bool {
        let Ok(controls) = self.inner.controls.lock() else {
            return false;
        };
        let Some(control) = controls.get(&run) else {
            return false;
        };
        control.cancel.store(true, Ordering::SeqCst);
        let _ = control.signals.send(Signal::Cancel);
        true
    }

    /// An event a wait may be listening for, such as `restack.resumed`, and how it ended
    /// (`resumed` or `aborted`). Wakes every run waiting on `event`; returns how many.
    pub fn deliver(&self, event: &str, outcome: &str) -> usize {
        self.deliver_with(event, outcome, &Value::Null)
    }

    /// As `deliver`, and the waits return the fields of `payload` (an object) beside `outcome`,
    /// as `ctx.wait` returns `{ outcome = ..., branch = ..., commit = ... }` (`WF3f`, D162(1)).
    pub fn deliver_with(&self, event: &str, outcome: &str, payload: &Value) -> usize {
        let Ok(controls) = self.inner.controls.lock() else {
            return 0;
        };
        controls
            .values()
            .filter(|c| c.pending == Pending::Wait(event.to_owned()))
            .filter(|c| {
                c.signals
                    .send(Signal::Event {
                        event: event.to_owned(),
                        outcome: outcome.to_owned(),
                        payload: payload.clone(),
                    })
                    .is_ok()
            })
            .count()
    }

    /// The runs that have not ended.
    #[must_use]
    pub fn active(&self) -> Vec<RunId> {
        let mut ids: Vec<RunId> = self
            .inner
            .controls
            .lock()
            .map(|c| c.keys().copied().collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// Waits for every run thread to end. For tests and for closing the app.
    pub fn join_all(&self) {
        let handles: Vec<JoinHandle<()>> = self
            .inner
            .handles
            .lock()
            .map(|mut h| std::mem::take(&mut *h))
            .unwrap_or_default();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Inner {
    fn emit(&self, event: ScriptEvent) {
        (self.events)(event);
    }

    fn signal(&self, run: RunId, expected: &Pending, signal: Signal) -> bool {
        let Ok(controls) = self.controls.lock() else {
            return false;
        };
        match controls.get(&run) {
            Some(control) if &control.pending == expected => control.signals.send(signal).is_ok(),
            _ => false,
        }
    }

    fn set_pending(&self, run: RunId, pending: Pending) {
        if let Ok(mut controls) = self.controls.lock()
            && let Some(control) = controls.get_mut(&run)
        {
            control.pending = pending;
        }
    }
}

fn run_thread(
    inner: &Arc<Inner>,
    id: RunId,
    spec: RunSpec,
    cancel: &Arc<AtomicBool>,
    signals: &mpsc::Receiver<Signal>,
) {
    let input = &spec.input;
    let mut host = RunHost {
        inner: Arc::clone(inner),
        run: id,
        workflow: spec.workflow.clone(),
        started_by: spec.started_by.clone(),
        auto_approve: spec.auto_approve,
        cancel: Arc::clone(cancel),
        signals,
        seq: 0,
        exec_seen: false,
        rng: spec.seed,
    };
    let request = RunRequest {
        chunk: &spec.chunk,
        source: &spec.source,
        input,
        surface: &inner.surface,
        budgets: inner.budgets,
        cancel: Arc::clone(cancel),
    };
    let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inner.engine.run(&request, &mut host)
    }))
    .unwrap_or_else(|_| RunEnd::Failed(Failure::Script("the script engine panicked".to_owned())));

    let (status, reason, value) = match end {
        RunEnd::Returned(value) => (RunStatus::Done, None, Some(value)),
        RunEnd::Blocked(reason) => (RunStatus::Blocked, Some(reason), None),
        RunEnd::Failed(failure) => (RunStatus::Failed, Some(failure.to_string()), None),
        RunEnd::Cancelled => (RunStatus::Cancelled, Some("cancelled".to_owned()), None),
    };
    let now = (inner.clock)();
    if let Err(e) = inner
        .journal
        .finish_run(id, status, now, reason.as_deref(), value.as_ref())
    {
        // The run is over either way; the front ends still hear how it ended.
        inner.emit(ScriptEvent::Notify {
            run: id,
            text: format!("could not write the end of the run to the journal: {e}"),
        });
    }
    if let Ok(mut controls) = inner.controls.lock() {
        controls.remove(&id);
    }
    inner.emit(ScriptEvent::Finished {
        run: id,
        workflow: spec.workflow,
        status,
        reason,
        value: value.filter(|v| !v.is_null()),
    });
}

/// The `CtxHost` of one run.
struct RunHost<'a> {
    inner: Arc<Inner>,
    run: RunId,
    workflow: String,
    started_by: StartedBy,
    auto_approve: bool,
    cancel: Arc<AtomicBool>,
    signals: &'a mpsc::Receiver<Signal>,
    seq: u32,
    /// An `exec` started in this run: every later `apply` asks (D101).
    exec_seen: bool,
    /// State of the `ctx.random()` generator.
    rng: u64,
}

/// How a call ended, for the script and for the journal.
struct Done {
    reply: HostReply,
    /// What the journal keeps as the result.
    result: Value,
    outcome: String,
    message: String,
}

impl Done {
    fn value(value: Value) -> Self {
        Self {
            reply: HostReply::Value(value.clone()),
            result: value,
            outcome: "done".to_owned(),
            message: String::new(),
        }
    }

    /// A value the script reads, ending with an outcome of its own (`approved`, `resumed`…).
    fn value_with(value: Value, outcome: &str) -> Self {
        Self {
            outcome: outcome.to_owned(),
            ..Self::value(value)
        }
    }

    fn outcome(outcome: &str, value: Value, message: String) -> Self {
        Self {
            reply: HostReply::Outcome {
                outcome: outcome.to_owned(),
                value: value.clone(),
                message: message.clone(),
            },
            result: value,
            outcome: outcome.to_owned(),
            message,
        }
    }

    fn end(end: HostEnd, outcome: &str, message: &str) -> Self {
        Self {
            reply: HostReply::End(end),
            result: Value::Null,
            outcome: outcome.to_owned(),
            message: message.to_owned(),
        }
    }
}

enum Blocked {
    Signal(Signal),
    Timeout,
}

impl RunHost<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn now(&self) -> i64 {
        (self.inner.clock)()
    }

    fn set_status(&self, status: RunStatus, pending: Pending) {
        self.inner.set_pending(self.run, pending);
        let _ = self.inner.journal.set_status(self.run, status);
        self.inner.emit(ScriptEvent::Changed {
            run: self.run,
            workflow: self.workflow.clone(),
            status,
        });
    }

    /// Blocks until a signal arrives, the deadline passes or the run is cancelled. A cancel sends a
    /// signal, so this wakes at once.
    fn block(&self, deadline: Option<Instant>) -> Blocked {
        if self.cancelled() {
            return Blocked::Signal(Signal::Cancel);
        }
        let received = match deadline {
            None => self
                .signals
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
            Some(deadline) => self
                .signals
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
        };
        match received {
            Ok(signal) => Blocked::Signal(signal),
            Err(RecvTimeoutError::Timeout) => Blocked::Timeout,
            // The runtime is gone: nobody can answer, so the run ends.
            Err(RecvTimeoutError::Disconnected) => Blocked::Signal(Signal::Cancel),
        }
    }

    /// Asks the person and waits. `None` when the run was cancelled meanwhile.
    fn ask(
        &self,
        func: &str,
        summary: String,
        args: &Value,
        reason: Option<AskReason>,
        plan: Option<RestackPlan>,
    ) -> Option<bool> {
        self.set_status(RunStatus::AwaitingApproval, Pending::Approval);
        self.inner.emit(ScriptEvent::AwaitingApproval {
            run: self.run,
            seq: self.seq,
            func: func.to_owned(),
            summary,
            args: args.clone(),
            reason,
            plan,
        });
        let answer = loop {
            match self.block(None) {
                Blocked::Signal(Signal::Approve(approved)) => break Some(approved),
                Blocked::Signal(Signal::Cancel) => break None,
                // A stale event or timeout is not an answer.
                Blocked::Signal(Signal::Event { .. }) | Blocked::Timeout => {}
            }
        };
        if answer.is_some() {
            self.set_status(RunStatus::Running, Pending::Nothing);
        }
        answer
    }

    fn random(&mut self) -> f64 {
        // splitmix64: small, and the same seed gives the same sequence.
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // The top 53 bits make an exact double in [0, 1).
        #[allow(clippy::cast_precision_loss)]
        let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
        unit
    }

    fn helper(&mut self, func: &str, args: &Value) -> Done {
        match func {
            "now" => Done::value(json!(self.now())),
            "random" => Done::value(json!(self.random())),
            "block" => {
                let reason = reason_of(args);
                Done::end(HostEnd::Blocked(reason.clone()), "blocked", &reason)
            }
            _ => {
                let reason = reason_of(args);
                Done::end(HostEnd::Failed(reason.clone()), "failed", &reason)
            }
        }
    }

    fn dispatch(&mut self, f: &CtxFn, args: &Value) -> Done {
        match f.path {
            "notify" => {
                let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
                self.inner.emit(ScriptEvent::Notify {
                    run: self.run,
                    text: text.to_owned(),
                });
                Done::value(Value::Null)
            }
            "approve" => {
                let summary = args
                    .get("summary")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Approve this step?")
                    .to_owned();
                match self.ask("approve", summary, args, None, None) {
                    Some(true) => Done::value_with(json!(true), "approved"),
                    Some(false) => Done::value_with(json!(false), "rejected"),
                    None => Done::end(HostEnd::Cancelled, "cancelled", "cancelled"),
                }
            }
            "exec" => self.exec(args),
            "wait" => self.wait(args),
            _ => self.effect(f, args),
        }
    }

    fn exec(&mut self, args: &Value) -> Done {
        let mut spec = match exec::spec(args, &self.inner.cwd) {
            Ok(spec) => spec,
            Err(message) => {
                return Done {
                    reply: HostReply::Error(format!("ctx.exec: {message}")),
                    result: Value::Null,
                    outcome: "error".to_owned(),
                    message,
                };
            }
        };
        spec.path = self.inner.login_path.get().cloned();
        // Set before the program starts: its output may shape what comes next (D101).
        self.exec_seen = true;
        let result = exec::run(&spec, &self.cancel);
        if self.cancelled() {
            return Done::end(HostEnd::Cancelled, "cancelled", "cancelled");
        }
        if result.outcome == "done" {
            return Done::value(result.output);
        }
        let message = result
            .output
            .get("error")
            .or_else(|| result.output.get("stderr"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Done::outcome(result.outcome, result.output, message)
    }

    fn wait(&mut self, args: &Value) -> Done {
        let event = args
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let deadline = args
            .get("timeout")
            .and_then(Value::as_str)
            .and_then(|t| exec::parse_timeout(t).ok())
            .map(|d| Instant::now() + d);
        self.set_status(RunStatus::Waiting, Pending::Wait(event.clone()));
        let outcome = loop {
            match self.block(deadline) {
                Blocked::Timeout => break Some(("timeout".to_owned(), Value::Null)),
                Blocked::Signal(Signal::Cancel) => break None,
                Blocked::Signal(Signal::Event {
                    event: got,
                    outcome,
                    payload,
                }) if got == event => {
                    break Some((outcome, payload));
                }
                Blocked::Signal(_) => {}
            }
        };
        match outcome {
            Some((outcome, payload)) => {
                self.set_status(RunStatus::Running, Pending::Nothing);
                // The outcome is the table's own field, whatever the payload holds.
                let mut value = match payload {
                    Value::Object(fields) => fields,
                    _ => serde_json::Map::new(),
                };
                value.insert("outcome".to_owned(), json!(outcome));
                Done::value_with(Value::Object(value), &outcome)
            }
            None => Done::end(HostEnd::Cancelled, "cancelled", "cancelled"),
        }
    }

    /// The plan an apply of a restack or a reparent would move, read from the engine before the
    /// person is asked (`WF4c`). Reads only: nothing is logged and nothing moves. `None` for the
    /// other calls, and when the engine cannot make the plan.
    fn restack_plan_of(&self, f: &CtxFn, args: &Value) -> Option<RestackPlan> {
        if !matches!(f.path, "restack.apply" | "reparent.apply") {
            return None;
        }
        let call = EffectCall {
            run: self.run,
            func: "restack.preview".to_owned(),
            args: args.clone(),
            level: Level::Read,
        };
        let result = self.inner.effects.effect(&call);
        serde_json::from_value(result.value).ok()
    }

    fn effect(&mut self, f: &CtxFn, args: &Value) -> Done {
        if f.level == Level::Apply {
            self.inner.emit(ScriptEvent::Applying { run: self.run });
            let gate = gate_apply(f.path, &self.started_by, self.auto_approve, self.exec_seen);
            if let Gate::Ask(reason) = gate {
                let summary = format!("{}\n{}", approval_title(f), reason.text());
                let plan = self.restack_plan_of(f, args);
                match self.ask(f.path, summary, args, Some(reason), plan) {
                    Some(true) => {}
                    Some(false) => {
                        return Done::outcome(
                            "rejected",
                            Value::Null,
                            "rejected by the user".to_owned(),
                        );
                    }
                    None => return Done::end(HostEnd::Cancelled, "cancelled", "cancelled"),
                }
            }
        }
        let call = EffectCall {
            run: self.run,
            func: f.path.to_owned(),
            args: args.clone(),
            level: f.level,
        };
        let result = self.inner.effects.effect(&call);
        if result.outcome == "cancelled" {
            return Done::end(HostEnd::Cancelled, "cancelled", "cancelled");
        }
        if result.outcome == "done" {
            return Done::value(result.value);
        }
        if !f.outcomes.contains(&result.outcome.as_str()) {
            let message = format!(
                "the effect ended with `{}`, which ctx.{} does not declare",
                result.outcome, f.path
            );
            return Done::outcome("failed", Value::Null, message);
        }
        Done::outcome(&result.outcome, result.value, result.message)
    }
}

impl CtxHost for RunHost<'_> {
    fn call(&mut self, func: &str, args: Value) -> HostReply {
        if self.cancelled() {
            return HostReply::End(HostEnd::Cancelled);
        }
        self.seq += 1;
        let started = Instant::now();
        let is_helper = super::surface::HOST_HELPERS.contains(&func);
        let function = self.inner.surface.get(func).cloned();
        if function.is_none() && !is_helper {
            return HostReply::Error(self.inner.surface.unknown_name_message("", func));
        }
        let level = function.as_ref().map_or("helper", |f| level_name(f.level));
        let begun = self.inner.journal.begin_call(&NewCall {
            run: self.run,
            seq: self.seq,
            func,
            level,
            args: &args,
            started_at: self.now(),
        });
        if let Err(e) = begun {
            // Without the record there is no observed history: do not act.
            return HostReply::End(HostEnd::Failed(format!(
                "cannot write the run journal: {e}"
            )));
        }

        let done = match &function {
            None => self.helper(func, &args),
            Some(f) => match super::args::validate(func, f.fields, &args) {
                Err(message) => Done {
                    reply: HostReply::Error(message.clone()),
                    result: Value::Null,
                    outcome: "error".to_owned(),
                    message,
                },
                Ok(()) => self.dispatch(f, &args),
            },
        };

        let millis = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        let message = (!done.message.is_empty()).then_some(done.message.as_str());
        // The effect has happened. If the journal cannot take its end, the run goes on: the start
        // row stays and a restart shows the call as unfinished.
        let _ = self.inner.journal.finish_call(
            self.run,
            self.seq,
            &done.result,
            &done.outcome,
            message,
            millis,
        );
        done.reply
    }
}

fn reason_of(args: &Value) -> String {
    args.get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "apply",
        Level::Pause => "pause",
    }
}

/// The first line of an approval dialog for an applying call.
fn approval_title(f: &CtxFn) -> String {
    format!("Allow ctx.{}?", f.path)
}
