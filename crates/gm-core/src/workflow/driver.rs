//! Drives workflow runs (docs/10, D64, D79, D80): feeds outcomes to the pure
//! transition function, carries out each effect, and records every step in
//! the store before and after it runs. Quick actions run inline; `exec` runs
//! on a thread the caller starts (`ExecJob`); `approve` pauses for the human.
//! Git actions go through `GitActions`, so the driver is tested with a fake.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::catalog::{self, Level};
use super::exec::{self, ExecResult, ExecSpec};
use super::loader::Entry;
use super::machine::{self, Context, Effect};
use super::model::{Definition, End, StateKind};
use super::store::{NewRun, RETAINED_RUNS, RunId, RunRow, RunStatus, Store};

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
    /// Cancels the `exec` job in flight. A fresh flag for each job.
    cancel: Arc<AtomicBool>,
    pending: Pending,
}

/// What an active run waits for, besides the engine's own steps.
#[derive(Debug, PartialEq, Eq)]
enum Pending {
    Nothing,
    /// The report of the `exec` job of this state.
    Exec(String),
    /// The human's answer to the `approve` state the run is in.
    Approval,
}

/// One state about to run: its id, its action and its rendered input.
struct Call {
    state: String,
    action: String,
    input: Value,
}

pub struct Driver {
    store: Store,
    /// Where `exec` runs: the repository root.
    cwd: std::path::PathBuf,
    /// The login-shell PATH `exec` steps run with; empty until it is read.
    login_path: Arc<std::sync::OnceLock<String>>,
    active: BTreeMap<RunId, Active>,
}

impl Driver {
    pub fn new(store: Store, cwd: std::path::PathBuf) -> Self {
        Self {
            store,
            cwd,
            login_path: Arc::default(),
            active: BTreeMap::new(),
        }
    }

    /// Gives `exec` steps the login-shell PATH git runs with (see `GitPort::login_path`).
    pub fn set_login_path(&mut self, path: Arc<std::sync::OnceLock<String>>) {
        self.login_path = path;
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Starts a manual run of `entry` (D64(a): a human's manual run executes
    /// its apply steps). Refuses an entry that did not load. `repo` is the
    /// snapshot expressions read as `repo.*` (`root`, `branch`, `head`,
    /// `staged_diff`).
    ///
    /// The whole serialized `Context` is stored as the run's `trigger`, so
    /// `resume` can rebuild it after a restart.
    pub fn start(
        &mut self,
        entry: &Entry,
        inputs: Value,
        repo: Value,
        git: &mut dyn GitActions,
        now: i64,
    ) -> Result<(RunId, Outcome), String> {
        let (definition, _) = entry
            .result
            .as_ref()
            .map_err(|_| format!("workflow `{}` does not load", entry.name))?;
        if definition.trigger.on != "manual" {
            return Err("only manual workflows run in this version".to_owned());
        }
        for input in &definition.inputs {
            match inputs.get(&input.name) {
                None => return Err(format!("missing input `{}`", input.name)),
                Some(value) if !value.is_string() => {
                    return Err(format!("input `{}` must be a string", input.name));
                }
                Some(_) => {}
            }
        }

        let context = Context {
            inputs,
            trigger: json!({"on": "manual"}),
            repo,
        };
        let trigger = serde_json::to_value(&context).map_err(|e| e.to_string())?;
        let id = self
            .store
            .create_run(
                &NewRun {
                    workflow: &entry.name,
                    definition_hash: &entry.hash,
                    definition_text: &entry.source,
                    trigger: &trigger,
                    started_by: "human",
                },
                now,
            )
            .map_err(|e| e.to_string())?;

        let mut out = Outcome::default();
        match machine::start(definition, &context) {
            Ok((run, effect)) => {
                self.active.insert(
                    id,
                    Active {
                        workflow: entry.name.clone(),
                        definition: definition.clone(),
                        context,
                        run,
                        cancel: Arc::new(AtomicBool::new(false)),
                        pending: Pending::Nothing,
                    },
                );
                self.drive(id, effect, git, now, &mut out);
            }
            Err(error) => {
                self.close_failed(id, entry.name.clone(), error.to_string(), now, &mut out);
            }
        }
        Ok((id, out))
    }

    /// The answer of an `exec` job. A report for a run that is no longer
    /// active (cancelled) is dropped.
    pub fn exec_report(
        &mut self,
        report: ExecReport,
        git: &mut dyn GitActions,
        now: i64,
    ) -> Outcome {
        let mut out = Outcome::default();
        let ExecReport { run, state, result } = report;
        let Some(active) = self.active.get_mut(&run) else {
            return out;
        };
        if active.pending != Pending::Exec(state) {
            return out;
        }
        active.pending = Pending::Nothing;

        match self.transition(run, result.outcome, &result.output, now) {
            Ok(effect) => self.drive(run, effect, git, now, &mut out),
            Err(error) => self.fail(run, error, now, &mut out),
        }
        out
    }

    /// The human's answer to an `approve` state.
    pub fn approve(
        &mut self,
        run: RunId,
        approved: bool,
        git: &mut dyn GitActions,
        now: i64,
    ) -> Result<Outcome, String> {
        let waiting = self
            .active
            .get(&run)
            .is_some_and(|active| active.pending == Pending::Approval);
        if !waiting {
            return Err(format!("run {run} is not awaiting approval"));
        }
        self.store
            .set_status(run, RunStatus::Running)
            .map_err(|e| e.to_string())?;
        if let Some(active) = self.active.get_mut(&run) {
            active.pending = Pending::Nothing;
        }

        let outcome = if approved { "approved" } else { "rejected" };
        let mut out = Outcome::default();
        match self.transition(run, outcome, &json!({}), now) {
            Ok(effect) => self.drive(run, effect, git, now, &mut out),
            Err(error) => self.fail(run, error, now, &mut out),
        }
        Ok(out)
    }

    /// Cancels an active run: kills its `exec` process, if any, and ends it
    /// `cancelled`.
    pub fn cancel(&mut self, run: RunId, now: i64) -> Result<Outcome, String> {
        let active = self
            .active
            .get(&run)
            .ok_or_else(|| format!("run {run} is not active"))?;
        active.cancel.store(true, Ordering::SeqCst);
        let workflow = active.workflow.clone();
        self.store
            .finish_run(run, RunStatus::Cancelled, Some("cancelled"), now)
            .map_err(|e| e.to_string())?;
        self.active.remove(&run);
        Ok(Outcome {
            events: vec![
                RunEvent::Finished {
                    run,
                    end: End::Failed,
                    reason: Some("cancelled".to_owned()),
                    value: None,
                },
                RunEvent::Changed {
                    run,
                    workflow,
                    status: RunStatus::Cancelled,
                    state: None,
                },
            ],
            jobs: Vec::new(),
        })
    }

    /// After a restart: picks up every unfinished run from the store (D80).
    pub fn resume(&mut self, git: &mut dyn GitActions, now: i64) -> Outcome {
        let _ = self.store.prune(RETAINED_RUNS);
        let mut out = Outcome::default();
        let Ok(rows) = self.store.unfinished_runs() else {
            return out;
        };
        for row in &rows {
            self.resume_run(row, git, now, &mut out);
        }
        out
    }

    /// Rebuilds one unfinished run from its row and carries on from its
    /// position.
    fn resume_run(&mut self, row: &RunRow, git: &mut dyn GitActions, now: i64, out: &mut Outcome) {
        let id = row.id;
        let workflow = row.workflow.clone();
        let Ok((definition, _)) = super::load(&row.definition_text) else {
            self.close_failed(
                id,
                workflow,
                "the workflow no longer loads".to_owned(),
                now,
                out,
            );
            return;
        };
        let context = match serde_json::from_value::<Context>(row.trigger.clone()) {
            Ok(context) => context,
            Err(error) => {
                let reason = format!("the run's context does not decode: {error}");
                self.close_failed(id, workflow, reason, now, out);
                return;
            }
        };
        let (run, effect) = match rebuild(&definition, &context, row.machine.as_ref()) {
            Ok(pair) => pair,
            Err(error) => {
                self.close_failed(id, workflow, error, now, out);
                return;
            }
        };

        self.active.insert(
            id,
            Active {
                workflow,
                definition,
                context,
                run,
                cancel: Arc::new(AtomicBool::new(false)),
                pending: Pending::Nothing,
            },
        );
        match row.status {
            RunStatus::AwaitingApproval => {
                if let Some(active) = self.active.get_mut(&id) {
                    active.pending = Pending::Approval;
                }
                out.events.push(RunEvent::AwaitingApproval {
                    run: id,
                    state: row.current_state.clone().unwrap_or_default(),
                    summary: String::new(),
                });
            }
            RunStatus::Waiting => {}
            _ => self.resume_running(id, effect, git, now, out),
        }
    }

    /// A running run: a crashed `apply` step is never run again (D80); any
    /// other step runs again from its effect.
    fn resume_running(
        &mut self,
        id: RunId,
        effect: Effect,
        git: &mut dyn GitActions,
        now: i64,
        out: &mut Outcome,
    ) {
        let open = match self.store.open_state(id) {
            Ok(open) => open,
            Err(error) => {
                self.fail(id, error.to_string(), now, out);
                return;
            }
        };
        if let Some(state) = open
            && let Some(outcomes) = self.apply_outcomes(id, &state)
        {
            let error = json!({"error": "the engine stopped while this step ran"});
            let outcome = if outcomes.contains(&"interrupted") {
                "interrupted"
            } else {
                "failed"
            };
            match self.transition(id, outcome, &error, now) {
                Ok(effect) => self.drive(id, effect, git, now, out),
                Err(error) => self.fail(id, error, now, out),
            }
            return;
        }
        self.drive(id, effect, git, now, out);
    }

    /// Carries out effects until the run waits for something or ends.
    fn drive(
        &mut self,
        id: RunId,
        first: Effect,
        git: &mut dyn GitActions,
        now: i64,
        out: &mut Outcome,
    ) {
        let mut effect = first;
        loop {
            let (outcome, output) = match effect {
                Effect::Run {
                    state,
                    action,
                    input,
                    ..
                } => {
                    let call = Call {
                        state,
                        action,
                        input,
                    };
                    match self.run_action(id, &call, git, now, out) {
                        Ok(Some(done)) => done,
                        // `exec` waits for its report, `approve` for the human.
                        Ok(None) => return,
                        Err(error) => {
                            self.fail(id, error, now, out);
                            return;
                        }
                    }
                }
                Effect::Wait { state, .. } => {
                    if let Err(error) = self.store.set_status(id, RunStatus::Waiting) {
                        self.fail(id, error.to_string(), now, out);
                    } else {
                        out.events
                            .push(self.changed(id, RunStatus::Waiting, Some(&state)));
                    }
                    return;
                }
                Effect::Finish { end, reason, value } => {
                    self.finish(id, end, reason, value, now, out);
                    return;
                }
            };
            effect = match self.transition(id, outcome, &output, now) {
                Ok(next) => next,
                Err(error) => {
                    self.fail(id, error, now, out);
                    return;
                }
            };
        }
    }

    /// Enters the state of `call`, then carries out its action. `Ok(None)`
    /// when the run now waits (an exec job or the human); `Ok(Some)` with the
    /// outcome and output otherwise. `Err` is a store failure.
    fn run_action(
        &mut self,
        id: RunId,
        call: &Call,
        git: &mut dyn GitActions,
        now: i64,
        out: &mut Outcome,
    ) -> Result<Option<(&'static str, Value)>, String> {
        self.store
            .enter_state(id, &call.state, now)
            .map_err(|e| e.to_string())?;
        out.events
            .push(self.changed(id, RunStatus::Running, Some(&call.state)));

        match call.action.as_str() {
            "exec" => return self.start_exec(id, call, out),
            "approve" => return self.await_approval(id, call, out),
            _ => {}
        }

        let result = match call.action.as_str() {
            "noop" => Ok(json!({})),
            "notify" => text_field(&call.input, "text").map(|text| {
                out.events.push(RunEvent::Notify {
                    run: id,
                    text: text.to_owned(),
                });
                json!({})
            }),
            "commit.suggest_message" => text_field(&call.input, "message").map(|message| {
                out.events.push(RunEvent::SuggestMessage {
                    run: id,
                    message: message.to_owned(),
                });
                json!({"message": message})
            }),
            "set" => Ok(call.input.clone()),
            "stage.all" => git.stage_all().map(|()| json!({})),
            "commit.create" => text_field(&call.input, "message")
                .and_then(|message| git.commit(message))
                .map(|commit| json!({"commit": commit})),
            "branch.create" => branch_create(&call.input, git),
            "branch.checkout" => text_field(&call.input, "branch")
                .and_then(|branch| git.checkout(branch))
                .map(|()| json!({})),
            other => Err(format!("action `{other}` is not available yet")),
        };
        Ok(Some(match result {
            Ok(output) => ("done", output),
            Err(error) => ("failed", json!({"error": error})),
        }))
    }

    /// Queues the `exec` job of `call` and leaves the run waiting for its report.
    fn start_exec(
        &mut self,
        id: RunId,
        call: &Call,
        out: &mut Outcome,
    ) -> Result<Option<(&'static str, Value)>, String> {
        let spec = match exec::spec(&call.input, &self.cwd) {
            Ok(mut spec) => {
                spec.path = self.login_path.get().cloned();
                spec
            }
            Err(error) => return Ok(Some(("failed", json!({"error": error})))),
        };
        let active = self
            .active
            .get_mut(&id)
            .ok_or_else(|| format!("run {id} is not active"))?;
        active.cancel = Arc::new(AtomicBool::new(false));
        active.pending = Pending::Exec(call.state.clone());
        out.jobs.push(ExecJob {
            run: id,
            state: call.state.clone(),
            spec,
            cancel: Arc::clone(&active.cancel),
        });
        Ok(None)
    }

    /// Pauses the run for the human, with the `summary` of the state.
    fn await_approval(
        &mut self,
        id: RunId,
        call: &Call,
        out: &mut Outcome,
    ) -> Result<Option<(&'static str, Value)>, String> {
        self.store
            .set_status(id, RunStatus::AwaitingApproval)
            .map_err(|e| e.to_string())?;
        let summary = call
            .input
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Some(active) = self.active.get_mut(&id) {
            active.pending = Pending::Approval;
        }
        out.events.push(RunEvent::AwaitingApproval {
            run: id,
            state: call.state.clone(),
            summary,
        });
        out.events
            .push(self.changed(id, RunStatus::AwaitingApproval, Some(&call.state)));
        Ok(None)
    }

    /// Records the outcome of the current state, then returns the effect of
    /// the next one. A machine or store failure is returned as the reason.
    fn transition(
        &mut self,
        id: RunId,
        outcome: &str,
        output: &Value,
        now: i64,
    ) -> Result<Effect, String> {
        let step = {
            let active = self
                .active
                .get(&id)
                .ok_or_else(|| format!("run {id} is not active"))?;
            machine::next(
                &active.definition,
                &active.context,
                &active.run,
                outcome,
                output.clone(),
            )
            .map_err(|e| e.to_string())?
        };
        let machine_json = serde_json::to_value(&step.run).map_err(|e| e.to_string())?;
        self.store
            .record_transition(
                id,
                &step.record,
                output,
                &machine_json,
                RunStatus::Running,
                now,
            )
            .map_err(|e| e.to_string())?;
        if let Some(active) = self.active.get_mut(&id) {
            active.run = step.run;
        }
        Ok(step.effect)
    }

    /// Ends the run as the effect says, and tells the front-ends.
    fn finish(
        &mut self,
        id: RunId,
        end: End,
        reason: Option<String>,
        value: Option<Value>,
        now: i64,
        out: &mut Outcome,
    ) {
        let status = status_of(end);
        if let Err(error) = self.store.finish_run(id, status, reason.as_deref(), now) {
            self.fail(id, error.to_string(), now, out);
            return;
        }
        let workflow = self.workflow_of(id);
        self.active.remove(&id);
        out.events.push(RunEvent::Finished {
            run: id,
            end,
            reason,
            value,
        });
        out.events.push(RunEvent::Changed {
            run: id,
            workflow,
            status,
            state: None,
        });
    }

    /// Ends an active run failed, because of `reason`.
    fn fail(&mut self, id: RunId, reason: String, now: i64, out: &mut Outcome) {
        let workflow = self.workflow_of(id);
        self.close_failed(id, workflow, reason, now, out);
    }

    /// Ends a run failed and forgets it. Takes the workflow name, so a run
    /// that cannot be rebuilt can be closed too. A second store error is
    /// ignored: the first one is the reason.
    fn close_failed(
        &mut self,
        id: RunId,
        workflow: String,
        reason: String,
        now: i64,
        out: &mut Outcome,
    ) {
        let _ = self
            .store
            .finish_run(id, RunStatus::Failed, Some(&reason), now);
        self.active.remove(&id);
        out.events.push(RunEvent::Finished {
            run: id,
            end: End::Failed,
            reason: Some(reason),
            value: None,
        });
        out.events.push(RunEvent::Changed {
            run: id,
            workflow,
            status: RunStatus::Failed,
            state: None,
        });
    }

    fn workflow_of(&self, id: RunId) -> String {
        self.active
            .get(&id)
            .map(|active| active.workflow.clone())
            .unwrap_or_default()
    }

    fn changed(&self, id: RunId, status: RunStatus, state: Option<&str>) -> RunEvent {
        RunEvent::Changed {
            run: id,
            workflow: self.workflow_of(id),
            status,
            state: state.map(str::to_owned),
        }
    }

    /// The outcomes of `path` when its state is an `apply` action; `None`
    /// otherwise.
    fn apply_outcomes(&self, id: RunId, path: &str) -> Option<&'static [&'static str]> {
        let definition = &self.active.get(&id)?.definition;
        let action = action_at(definition, path)?;
        let spec = catalog::action(action)?;
        (spec.level == Level::Apply).then_some(spec.outcomes)
    }
}

/// The run as the engine stores it: rebuilt from its saved position, or
/// started again when it never recorded a transition.
fn rebuild(
    definition: &Definition,
    context: &Context,
    saved: Option<&Value>,
) -> Result<(machine::Run, Effect), String> {
    match saved {
        None => machine::start(definition, context).map_err(|e| e.to_string()),
        Some(value) => {
            let run: machine::Run = serde_json::from_value(value.clone())
                .map_err(|e| format!("cannot read the run's position: {e}"))?;
            let effect =
                machine::current_effect(definition, context, &run).map_err(|e| e.to_string())?;
            Ok((run, effect))
        }
    }
}

/// The action of the state at `path` (`each/plan` inside a `for_each`).
fn action_at<'d>(definition: &'d Definition, path: &str) -> Option<&'d str> {
    let state = match path.split_once('/') {
        None => definition.state(path)?,
        Some((owner, nested)) => match &definition.state(owner)?.kind {
            StateKind::ForEach { states, .. } => states.iter().find(|state| state.id == nested)?,
            _ => return None,
        },
    };
    match &state.kind {
        StateKind::Action { action, .. } => Some(action.as_str()),
        _ => None,
    }
}

fn status_of(end: End) -> RunStatus {
    match end {
        End::Done => RunStatus::Done,
        End::Failed => RunStatus::Failed,
        End::Blocked => RunStatus::Blocked,
    }
}

/// A string input of an action; missing or not a string is an error naming it.
fn text_field<'a>(input: &'a Value, name: &str) -> Result<&'a str, String> {
    input
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("input `{name}` is missing or not a string"))
}

/// `branch.create`: `checkout` is optional and false by default.
fn branch_create(input: &Value, git: &mut dyn GitActions) -> Result<Value, String> {
    let name = text_field(input, "name")?;
    let checkout = match input.get("checkout") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "input `checkout` must be true or false".to_owned())?,
    };
    git.create_branch(name, checkout).map(|()| json!({}))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sha2::{Digest, Sha256};

    use super::*;

    const SUGGEST: &str = r#"
schema = 1
name = "suggest"

[trigger]
on = "manual"

[[state]]
id = "ask"
action = "exec"
with = { argv = ["printf", "feat: x"] }
on.timeout = "end.failed"

[[state]]
id = "suggest"
action = "commit.suggest_message"
with = { message = "${states.ask.output.text}" }
"#;

    const TICKET: &str = r#"
schema = 1
name = "branch-from-ticket"

[inputs]
ticket = { type = "string", prompt = "Ticket key" }

[trigger]
on = "manual"

[[state]]
id = "name"
action = "exec"
with = { argv = ["printf", "feat/${inputs.ticket}"] }
on.timeout = "end.failed"

[[state]]
id = "confirm"
action = "approve"
with = { summary = "Create branch ${states.name.output.text}?" }
on.approved = "create"
on.rejected = "end.done"

[[state]]
id = "create"
action = "branch.create"
with = { name = "${states.name.output.text}", checkout = true }
"#;

    const COMMIT_NOW: &str = r#"
schema = 1
name = "commit-now"

[trigger]
on = "manual"

[[state]]
id = "say"
action = "notify"
with = { text = "Staging" }

[[state]]
id = "stage"
action = "stage.all"
with = {}

[[state]]
id = "commit"
action = "commit.create"
with = { message = "feat: x" }
"#;

    const COMMIT_ONLY: &str = r#"
schema = 1
name = "commit-only"

[trigger]
on = "manual"

[[state]]
id = "stage"
action = "stage.all"
with = {}

[[state]]
id = "commit"
action = "commit.create"
with = { message = "feat: x" }
"#;

    const RESTACK_BY_HAND: &str = r#"
schema = 1
name = "restack-by-hand"

[trigger]
on = "manual"

[[state]]
id = "plan"
action = "restack.plan"
with = { root = "main" }
"#;

    const SLOW: &str = r#"
schema = 1
name = "slow"

[trigger]
on = "manual"

[[state]]
id = "wait"
action = "exec"
with = { argv = ["sleep", "5"] }
on.timeout = "end.failed"
"#;

    /// Records the git calls and fails the method named by `failing`.
    #[derive(Default)]
    struct FakeGit {
        calls: Vec<String>,
        failing: Option<&'static str>,
    }

    impl FakeGit {
        fn record(&mut self, method: &'static str, call: String) -> Result<(), String> {
            self.calls.push(call);
            if self.failing == Some(method) {
                Err(format!("{method} failed"))
            } else {
                Ok(())
            }
        }
    }

    impl GitActions for FakeGit {
        fn stage_all(&mut self) -> Result<(), String> {
            self.record("stage_all", "stage_all".to_owned())
        }

        fn commit(&mut self, message: &str) -> Result<String, String> {
            self.record("commit", format!("commit {message}"))?;
            Ok("abc123".to_owned())
        }

        fn create_branch(&mut self, name: &str, checkout: bool) -> Result<(), String> {
            self.record("create_branch", format!("create_branch {name} {checkout}"))
        }

        fn checkout(&mut self, branch: &str) -> Result<(), String> {
            self.record("checkout", format!("checkout {branch}"))
        }
    }

    fn hash(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }

    /// An entry as the loader makes it, from TOML text.
    fn entry(source: &str) -> Entry {
        let result = crate::workflow::load(source);
        let name = match &result {
            Ok((definition, _)) => definition.name.clone(),
            Err(_) => "broken".to_owned(),
        };
        Entry {
            path: PathBuf::from("test.toml"),
            name,
            hash: hash(source),
            source: source.to_owned(),
            result,
        }
    }

    fn driver() -> Driver {
        Driver::new(
            Store::open_in_memory().expect("store"),
            std::env::temp_dir(),
        )
    }

    fn ticket_inputs() -> Value {
        json!({"ticket": "T-1"})
    }

    /// Starts the ticket workflow and answers its `exec`, so it waits for
    /// the approval.
    fn start_ticket(driver: &mut Driver, git: &mut FakeGit) -> RunId {
        let (run, out) = driver
            .start(&entry(TICKET), ticket_inputs(), json!({}), git, 1000)
            .expect("start");
        let report = out.jobs.into_iter().next().expect("exec job").run();
        let out = driver.exec_report(report, git, 1100);
        assert!(out.events.contains(&RunEvent::AwaitingApproval {
            run,
            state: "confirm".to_owned(),
            summary: "Create branch feat/T-1?".to_owned(),
        }));
        run
    }

    fn finished(run: RunId, end: End, reason: Option<&str>) -> RunEvent {
        RunEvent::Finished {
            run,
            end,
            reason: reason.map(str::to_owned),
            value: None,
        }
    }

    #[test]
    fn suggest_commit_message_shows_the_printed_text_and_ends_done() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let (run, mut out) = driver
            .start(&entry(SUGGEST), json!({}), json!({}), &mut git, 1000)
            .expect("start");
        assert_eq!(out.jobs.len(), 1);
        let report = out.jobs.remove(0).run();

        let out = driver.exec_report(report, &mut git, 1100);
        assert!(out.events.contains(&RunEvent::SuggestMessage {
            run,
            message: "feat: x".to_owned(),
        }));
        assert!(out.events.contains(&finished(run, End::Done, None)));

        assert_eq!(
            driver.store().transitions(run).expect("transitions").len(),
            2
        );
        let row = driver.store().run(run).expect("read").expect("row");
        assert_eq!(row.status, RunStatus::Done);
        assert!(git.calls.is_empty());
    }

    #[test]
    fn branch_from_ticket_creates_the_branch_when_approved() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let run = start_ticket(&mut driver, &mut git);
        let out = driver.approve(run, true, &mut git, 1200).expect("approve");
        assert_eq!(git.calls, ["create_branch feat/T-1 true"]);
        assert!(out.events.contains(&finished(run, End::Done, None)));
        assert!(driver.approve(run, true, &mut git, 1300).is_err());
    }

    #[test]
    fn branch_from_ticket_creates_nothing_when_rejected() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let run = start_ticket(&mut driver, &mut git);
        let out = driver.approve(run, false, &mut git, 1200).expect("reject");
        assert!(git.calls.is_empty());
        assert!(out.events.contains(&finished(run, End::Done, None)));
    }

    #[test]
    fn notify_stage_and_commit_run_in_order() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let (run, out) = driver
            .start(&entry(COMMIT_NOW), json!({}), json!({}), &mut git, 1000)
            .expect("start");
        assert!(out.events.contains(&RunEvent::Notify {
            run,
            text: "Staging".to_owned(),
        }));
        assert!(out.events.contains(&finished(run, End::Done, None)));
        assert_eq!(git.calls, ["stage_all", "commit feat: x"]);
    }

    #[test]
    fn a_failing_commit_ends_the_run_failed() {
        let mut driver = driver();
        let mut git = FakeGit {
            failing: Some("commit"),
            ..FakeGit::default()
        };
        let (run, out) = driver
            .start(&entry(COMMIT_NOW), json!({}), json!({}), &mut git, 1000)
            .expect("start");
        assert!(out.events.iter().any(|event| matches!(
            event,
            RunEvent::Finished { run: r, end: End::Failed, reason: Some(_), .. } if *r == run
        )));
        assert_eq!(
            driver.store().run(run).expect("read").expect("row").status,
            RunStatus::Failed
        );
    }

    #[test]
    fn start_refuses_what_this_version_does_not_run() {
        let mut driver = driver();
        let mut git = FakeGit::default();

        let broken = entry(include_str!(
            "../../tests/workflows/invalid/unknown-action.toml"
        ));
        let error = driver
            .start(&broken, json!({}), json!({}), &mut git, 1)
            .err()
            .expect("refused");
        assert!(error.contains("does not load"), "{error}");

        let before_restack = entry(include_str!(
            "../../tests/workflows/block-restack-on-protected.toml"
        ));
        let error = driver
            .start(&before_restack, json!({}), json!({}), &mut git, 1)
            .err()
            .expect("refused");
        assert_eq!(error, "only manual workflows run in this version");

        let error = driver
            .start(&entry(TICKET), json!({}), json!({}), &mut git, 1)
            .err()
            .expect("refused");
        assert_eq!(error, "missing input `ticket`");

        let error = driver
            .start(&entry(TICKET), json!({"ticket": 5}), json!({}), &mut git, 1)
            .err()
            .expect("refused");
        assert_eq!(error, "input `ticket` must be a string");

        assert!(driver.store().unfinished_runs().expect("list").is_empty());
    }

    #[test]
    fn cancel_during_an_exec_ends_the_run_and_drops_the_late_report() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let (run, mut out) = driver
            .start(&entry(SLOW), json!({}), json!({}), &mut git, 1000)
            .expect("start");
        let job = out.jobs.remove(0);

        let cancelled = driver.cancel(run, 1100).expect("cancel");
        assert!(cancelled.events.contains(&RunEvent::Finished {
            run,
            end: End::Failed,
            reason: Some("cancelled".to_owned()),
            value: None,
        }));
        assert!(cancelled.events.contains(&RunEvent::Changed {
            run,
            workflow: "slow".to_owned(),
            status: RunStatus::Cancelled,
            state: None,
        }));

        let late = driver.exec_report(job.run(), &mut git, 1200);
        assert!(late.events.is_empty());
        assert!(late.jobs.is_empty());
    }

    #[test]
    fn resume_runs_a_read_step_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        let mut git = FakeGit::default();

        let run = {
            let mut driver = Driver::new(Store::open(&path).expect("open"), std::env::temp_dir());
            let (run, out) = driver
                .start(&entry(SUGGEST), json!({}), json!({}), &mut git, 1000)
                .expect("start");
            assert_eq!(out.jobs.len(), 1);
            // Dropped without reporting: the process went away.
            run
        };

        let mut driver = Driver::new(Store::open(&path).expect("reopen"), std::env::temp_dir());
        let mut out = driver.resume(&mut git, 2000);
        assert_eq!(out.jobs.len(), 1);
        let job = out.jobs.remove(0);
        assert_eq!(job.run, run);
        assert_eq!(job.state, "ask");
        assert_eq!(job.spec.argv, ["printf", "feat: x"]);

        let report = job.run();
        let out = driver.exec_report(report, &mut git, 2100);
        assert!(out.events.contains(&RunEvent::SuggestMessage {
            run,
            message: "feat: x".to_owned(),
        }));
        assert!(out.events.contains(&finished(run, End::Done, None)));
    }

    #[test]
    fn resume_never_runs_a_crashed_apply_step_again() {
        let context = Context {
            inputs: json!({}),
            trigger: json!({"on": "manual"}),
            repo: json!({}),
        };
        let trigger = serde_json::to_value(&context).expect("encode");
        let (definition, _) = crate::workflow::load(COMMIT_ONLY).expect("loads");

        let mut store = Store::open_in_memory().expect("open");
        let run = store
            .create_run(
                &NewRun {
                    workflow: "commit-only",
                    definition_hash: "hash",
                    definition_text: COMMIT_ONLY,
                    trigger: &trigger,
                    started_by: "human",
                },
                1000,
            )
            .expect("create");
        let (first, _) = machine::start(&definition, &context).expect("start");
        let step = machine::next(&definition, &context, &first, "done", json!({})).expect("next");
        store
            .record_transition(
                run,
                &step.record,
                &json!({}),
                &serde_json::to_value(&step.run).expect("encode"),
                RunStatus::Running,
                1100,
            )
            .expect("record");
        store.enter_state(run, "commit", 1200).expect("enter");

        let mut driver = Driver::new(store, std::env::temp_dir());
        let mut git = FakeGit::default();
        let out = driver.resume(&mut git, 2000);

        assert!(git.calls.is_empty());
        assert!(out.events.iter().any(|event| matches!(
            event,
            RunEvent::Finished { run: r, end: End::Failed, reason: Some(_), .. } if *r == run
        )));
        assert_eq!(
            driver.store().run(run).expect("read").expect("row").status,
            RunStatus::Failed
        );
    }

    #[test]
    fn an_action_this_version_lacks_fails_the_step() {
        let mut driver = driver();
        let mut git = FakeGit::default();
        let (run, out) = driver
            .start(
                &entry(RESTACK_BY_HAND),
                json!({}),
                json!({}),
                &mut git,
                1000,
            )
            .expect("start");
        assert!(out.events.iter().any(|event| matches!(
            event,
            RunEvent::Finished { run: r, end: End::Failed, reason: Some(_), .. } if *r == run
        )));

        let outputs = driver.store().outputs(run).expect("outputs");
        assert_eq!(
            outputs[0].output,
            Some(json!({"error": "action `restack.plan` is not available yet"}))
        );
    }
}
