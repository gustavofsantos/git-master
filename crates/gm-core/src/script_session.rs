//! The engine's side of script runs (design/workflow-scripts.md, D81, D100): the runtime of one
//! open repository, and the effects a run asks for. A run lives on its own thread, which cannot
//! touch the `GitPort`; it posts each effect to the engine thread and blocks for the answer, so
//! every Git write is a Command-shaped call in the command log under the operation that started
//! the run, as before (D81).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use gm_schema::{
    Event, OperationId, WorkflowCallRecord, WorkflowDryCall, WorkflowDryRun, WorkflowEnd,
    WorkflowInfo, WorkflowInput, WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary,
};
use serde_json::{Value, json};

use crate::gate_check::GateConfig;
use crate::port::{RemoteRun, ScriptRemote};
use crate::script::builtins;
use crate::script::dry::{self, DryRun};
use crate::script::journal::{CallRow, Journal, RunId, RunRow, RunStatus};
use crate::script::loader::{Loaded, ScriptEntry};
use crate::script::luau::LuauEngine;
use crate::script::port::{Budgets, ScriptDiagnostic, ScriptEngine};
use crate::script::run::{
    Clock, EffectCall, EffectResult, Effects, EventSink, Runtime, RuntimeConfig, ScriptEvent,
};
use crate::script_stacks::{self, PlanBook, Started};
use crate::script_verdicts;
use crate::workflow_session::{Actions, Effected, GitActions};
use crate::{GitError, GitPort, LogEntry};
use gm_schema::CommandKind;

/// A Git command a run waits for (`WF4a`), to run on a thread of its own. The runner calls `run`
/// and passes the report to `Engine::script_remote_report`.
pub struct ScriptJob {
    pub(crate) run: RunId,
    pub(crate) command: RemoteRun,
}

impl ScriptJob {
    /// Runs the command to its end.
    #[must_use]
    pub fn run(self) -> ScriptReport {
        ScriptReport {
            run: self.run,
            outcome: (self.command)().map_err(|GitError(message)| message),
        }
    }
}

/// The end of a `ScriptJob`.
pub struct ScriptReport {
    pub(crate) run: RunId,
    pub(crate) outcome: Result<(), String>,
}

/// A call that waits for its Git command, and the way to answer its run (`WF4a`).
pub(crate) struct Waiting {
    pub(crate) run: RunId,
    pub(crate) kind: CommandKind,
    pub(crate) remote: ScriptRemote,
    pub(crate) reply: mpsc::Sender<EffectResult>,
}

/// What `ScriptSession::execute` did with a request: answered, or left for its Git command.
pub(crate) enum Carried {
    Answered(Effected),
    Waiting(Box<Waiting>),
}

/// An effect posted by a run thread, with the way back.
pub struct EffectRequest {
    pub call: EffectCall,
    reply: mpsc::Sender<EffectResult>,
}

impl EffectRequest {
    /// A request that nothing waits for: the answer is dropped. For tests.
    #[must_use]
    pub fn detached(call: EffectCall) -> Self {
        Self {
            call,
            reply: mpsc::channel().0,
        }
    }
}

/// Where the engine's two streams come from: effect requests (answered by
/// `Engine::script_effect`) and events (for the front ends). The runner posts both onto the engine
/// thread's channel; tests collect them.
#[derive(Clone)]
pub struct ScriptLink {
    pub effects: Arc<dyn Fn(EffectRequest) + Send + Sync>,
    pub events: EventSink,
}

/// Sends a run's effects to the engine thread and waits for the answer.
struct PostedEffects {
    post: Arc<dyn Fn(EffectRequest) + Send + Sync>,
}

impl Effects for PostedEffects {
    fn effect(&self, call: &EffectCall) -> EffectResult {
        let (reply, answer) = mpsc::channel();
        (self.post)(EffectRequest {
            call: call.clone(),
            reply,
        });
        answer.recv().unwrap_or_else(|_| {
            EffectResult::failed("the engine stopped before it answered the call")
        })
    }
}

/// The script session of one open repository.
pub(crate) struct ScriptSession {
    pub(crate) root: PathBuf,
    pub(crate) runtime: Runtime,
    /// The operation that started each run: its Git writes are logged under it.
    origins: BTreeMap<RunId, OperationId>,
    default_origin: OperationId,
    events: EventSink,
    /// The temporary file that holds the whole staged diff of a run, until the run ends.
    diffs: BTreeMap<RunId, tempfile::NamedTempFile>,
    /// The stack plans the runs made, by id (`WF4a`).
    plans: PlanBook,
    /// The workflow each run runs, by run id (`WF4c`): a restack conflict names it.
    workflows: BTreeMap<RunId, String>,
    /// The gate directories a `ctx.push.lease` or `ctx.restack.apply` checks against (D212).
    pub(crate) gates: GateConfig,
}

impl ScriptSession {
    pub(crate) fn open(
        root: PathBuf,
        state_db: &std::path::Path,
        link: &ScriptLink,
        git: &dyn GitPort,
        clock: Clock,
        opened_by: OperationId,
        gates: GateConfig,
    ) -> Result<Self, String> {
        let journal = Journal::open(state_db).map_err(|e| e.0)?;
        let engine: Arc<dyn ScriptEngine> = Arc::new(LuauEngine::new());
        let runtime = Runtime::new(RuntimeConfig {
            engine,
            journal,
            effects: Arc::new(PostedEffects {
                post: Arc::clone(&link.effects),
            }),
            events: Arc::clone(&link.events),
            clock,
            cwd: root.clone(),
            login_path: git.login_path(),
            budgets: Budgets::default(),
        });
        // D100: whatever the last session left unfinished is over now.
        runtime.recover();
        Ok(Self {
            root,
            runtime,
            origins: BTreeMap::new(),
            default_origin: opened_by,
            events: Arc::clone(&link.events),
            diffs: BTreeMap::new(),
            plans: PlanBook::default(),
            workflows: BTreeMap::new(),
            gates,
        })
    }

    pub(crate) fn note_origin(&mut self, run: RunId, origin: OperationId) {
        self.origins.insert(run, origin);
    }

    pub(crate) fn note_workflow(&mut self, run: RunId, workflow: String) {
        self.workflows.insert(run, workflow);
    }

    /// The name of the workflow `run` runs, when the run is known to this session.
    pub(crate) fn workflow(&self, run: RunId) -> Option<&str> {
        self.workflows.get(&run).map(String::as_str)
    }

    /// Keeps the file with the whole staged diff alive while the run lasts.
    pub(crate) fn keep_diff(&mut self, run: RunId, file: tempfile::NamedTempFile) {
        self.diffs.insert(run, file);
    }

    /// The run ended: its diff file goes.
    pub(crate) fn drop_diff(&mut self, run: RunId) {
        self.diffs.remove(&run);
    }

    pub(crate) fn origin(&self, run: RunId) -> OperationId {
        self.origins
            .get(&run)
            .copied()
            .unwrap_or(self.default_origin)
    }

    /// Carries out one effect, or starts the Git command it waits for (`WF4a`). An effect that
    /// runs on the engine thread is answered here, and returns what it changed, so the caller reads
    /// the status (and the branches and stacks, if refs moved) again.
    pub(crate) fn execute(
        &mut self,
        git: &dyn GitPort,
        log: &mut Vec<LogEntry>,
        request: EffectRequest,
    ) -> Carried {
        let EffectRequest { call, reply } = request;
        // A run cancelled while its effect waited for the engine writes nothing (WF3a).
        if self.runtime.is_cancelling(call.run) {
            let _ = reply.send(EffectResult::cancelled());
            return Carried::Answered(Effected::default());
        }
        let origin = self.origin(call.run);
        let started = match call.func.as_str() {
            "fetch" => Some(script_stacks::fetch(git, &self.root, remote_arg(&call))),
            "push.lease" => Some(script_stacks::push_lease(
                git,
                &self.root,
                &self.gates,
                text_arg(&call, "branch"),
                remote_arg(&call),
            )),
            "verdicts" => Some(Started::Ended(script_verdicts::read(
                git,
                &self.root,
                &call.args,
                self.runtime.trigger_branch(call.run),
            ))),
            _ => None,
        };
        if let Some(started) = started {
            return match started {
                Started::Ended(result) => {
                    let _ = reply.send(result);
                    Carried::Answered(Effected::default())
                }
                Started::Waiting { remote } => Carried::Waiting(Box::new(Waiting {
                    run: call.run,
                    kind: if call.func == "fetch" {
                        CommandKind::Fetch
                    } else {
                        CommandKind::PushWithLease
                    },
                    remote,
                    reply,
                })),
            };
        }
        let mut actions = Actions::new(git, &self.root, log, origin);
        let result = effect(
            &mut actions,
            &mut self.plans,
            &self.gates,
            &call,
            &self.events,
        );
        let effected = actions.finish();
        // The run may be gone (cancelled while it waited for the engine); nobody needs the answer.
        let _ = reply.send(result);
        Carried::Answered(effected)
    }
}

fn text_arg<'a>(call: &'a EffectCall, key: &str) -> &'a str {
    call.args
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The remote of a call, `origin` when it names none.
fn remote_arg(call: &EffectCall) -> &str {
    call.args
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or("origin")
}

/// The effect of one `ctx` call. Arguments were checked against the catalog before the call got
/// here.
fn effect(
    actions: &mut Actions<'_>,
    plans: &mut PlanBook,
    gates: &GateConfig,
    call: &EffectCall,
    events: &EventSink,
) -> EffectResult {
    let into = |result: Result<Value, String>| match result {
        Ok(value) => EffectResult::done(value),
        Err(message) => EffectResult::failed(message),
    };
    match call.func.as_str() {
        "stack.tree" => into(script_stacks::tree(actions.git(), actions.root())),
        "stack.stale" => into(script_stacks::stale(actions.git(), actions.root())),
        "restack.plan" => into(script_stacks::plan_restack(
            actions.git(),
            actions.root(),
            plans,
            text_arg(call, "root"),
        )),
        "reparent.plan" => into(script_stacks::plan_reparent(
            actions.git(),
            actions.root(),
            plans,
            text_arg(call, "branch"),
            text_arg(call, "parent"),
        )),
        "restack.preview" => script_stacks::preview(
            actions.git(),
            actions.root(),
            plans,
            text_arg(call, "plan_id"),
        ),
        "restack.apply" => {
            script_stacks::apply_restack(actions, plans, gates, text_arg(call, "plan_id"))
        }
        "reparent.apply" => {
            script_stacks::apply_reparent(actions, plans, text_arg(call, "plan_id"))
        }
        "restack.undo" => script_stacks::undo(actions, text_arg(call, "plan_id")),
        "stage.all" => into(actions.stage_all().map(|()| Value::Null)),
        "commit.create" => into(
            actions
                .commit(text_arg(call, "message"))
                .map(|commit| json!({ "commit": commit })),
        ),
        "branch.create" => {
            let checkout = call
                .args
                .get("checkout")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            into(
                actions
                    .create_branch(text_arg(call, "name"), checkout)
                    .map(|()| Value::Null),
            )
        }
        "branch.checkout" => into(
            actions
                .checkout(text_arg(call, "branch"))
                .map(|()| Value::Null),
        ),
        "commit.suggest_message" => {
            events(ScriptEvent::SuggestMessage {
                run: call.run,
                message: text_arg(call, "message").to_owned(),
            });
            EffectResult::done(Value::Null)
        }
        other => EffectResult::failed(format!(
            "ctx.{other} is not available in this version of GitMaster yet"
        )),
    }
}

/// One script as the front ends list it. A script that does not load carries its first problem.
pub(crate) fn workflow_info(entry: &ScriptEntry) -> WorkflowInfo {
    let (trigger, key, inputs, error) = match &entry.meta {
        Ok(meta) => (
            meta.trigger.on.clone(),
            meta.trigger.key.clone(),
            meta.inputs
                .iter()
                .map(|input| WorkflowInput {
                    name: input.name.clone(),
                    prompt: Some(input.prompt.clone()),
                })
                .collect(),
            None,
        ),
        Err(problems) => (
            String::new(),
            None,
            Vec::new(),
            problems.first().map(|p| {
                if p.line == 0 {
                    p.message.clone()
                } else {
                    format!("line {}: {}", p.line, p.message)
                }
            }),
        ),
    };
    WorkflowInfo {
        name: entry.name.clone(),
        path: entry.path.display().to_string(),
        trigger,
        key,
        inputs,
        hash: entry.hash.clone(),
        error,
        warnings: Vec::new(),
        source: entry.source.clone(),
        reviewed: false,
        auto_approve: false,
        // The engine sets these from the switches and the reviews it holds.
        enabled: false,
        title: builtins::get(&entry.name)
            .filter(|_| entry.builtin)
            .map(|builtin| builtin.title.to_owned()),
    }
}

/// What the listing did not read, as `<path>: <reason>`.
pub(crate) fn ignored_lines(loaded: &Loaded) -> Vec<String> {
    loaded
        .ignored
        .iter()
        .map(|ignored| format!("{}: {}", ignored.path.display(), ignored.reason))
        .collect()
}

/// The events a front end gets for what a run reported.
pub(crate) fn to_events(event: ScriptEvent) -> Vec<Event> {
    match event {
        // A run left over from a session that stopped (D100) is not news in the window.
        ScriptEvent::Changed {
            status: RunStatus::Interrupted,
            ..
        }
        | ScriptEvent::Applying { .. } => Vec::new(),
        ScriptEvent::Changed {
            run,
            workflow,
            status,
        } => vec![Event::WorkflowRunChanged {
            run,
            workflow,
            status: run_status(status),
            state: None,
        }],
        ScriptEvent::Notify { run, text } => vec![Event::WorkflowNotice { run, text }],
        ScriptEvent::SuggestMessage { run, message } => {
            vec![Event::WorkflowMessageSuggested { run, message }]
        }
        ScriptEvent::AwaitingApproval {
            run,
            func,
            summary,
            args,
            plan,
            ..
        } => {
            // The person approves the concrete call, so the dialog carries its arguments (D101).
            let shown = if func == "approve" {
                summary
            } else {
                let args = serde_json::to_string_pretty(&args).unwrap_or_default();
                format!("{summary}\n\nctx.{func}({args})")
            };
            vec![Event::WorkflowApprovalRequested {
                run,
                state: func,
                summary: shown,
                plan,
            }]
        }
        ScriptEvent::Finished {
            run,
            workflow,
            status,
            reason,
            ..
        } => {
            let end = match status {
                RunStatus::Done => WorkflowEnd::Done,
                RunStatus::Blocked => WorkflowEnd::Blocked,
                RunStatus::Cancelled => WorkflowEnd::Cancelled,
                _ => WorkflowEnd::Failed,
            };
            vec![
                Event::WorkflowRunChanged {
                    run,
                    workflow,
                    status: run_status(status),
                    state: None,
                },
                Event::WorkflowRunFinished { run, end, reason },
            ]
        }
    }
}

fn run_status(status: RunStatus) -> WorkflowRunStatus {
    match status {
        RunStatus::Running => WorkflowRunStatus::Running,
        RunStatus::AwaitingApproval => WorkflowRunStatus::AwaitingApproval,
        RunStatus::Waiting => WorkflowRunStatus::Waiting,
        RunStatus::Done => WorkflowRunStatus::Done,
        RunStatus::Blocked => WorkflowRunStatus::Blocked,
        RunStatus::Cancelled => WorkflowRunStatus::Cancelled,
        RunStatus::Failed => WorkflowRunStatus::Failed,
        RunStatus::Interrupted => WorkflowRunStatus::Interrupted,
    }
}

/// A run of the journal as the history lists it.
pub(crate) fn run_summary(row: &RunRow) -> WorkflowRunSummary {
    WorkflowRunSummary {
        run: row.id,
        workflow: row.workflow.clone(),
        status: run_status(row.status),
        started_by: row.started_by.clone(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        end_reason: row.end_reason.clone(),
        current_state: None,
    }
}

/// A run with its script as it ran, its input, trigger and every `ctx` call (the Runs tab).
pub(crate) fn run_detail(row: &RunRow, calls: &[CallRow]) -> WorkflowRunDetail {
    WorkflowRunDetail {
        summary: run_summary(row),
        definition: row.definition_text.clone(),
        trigger: row.trigger.clone(),
        input: row.input.clone(),
        calls: calls.iter().map(call_record).collect(),
    }
}

fn call_record(call: &CallRow) -> WorkflowCallRecord {
    WorkflowCallRecord {
        seq: call.seq,
        func: call.func.clone(),
        level: call.level.clone(),
        args: call.args.clone(),
        result: call.result.clone(),
        outcome: call.outcome.clone(),
        message: call.message.clone().filter(|m| !m.is_empty()),
        started_at: call.started_at,
        duration_ms: call.duration_ms,
    }
}

/// A dry run that found the text does not load: the problems, one per line.
pub(crate) fn dry_run_error(problems: &[ScriptDiagnostic]) -> WorkflowDryRun {
    let error = problems
        .iter()
        .map(|p| {
            if p.line == 0 {
                p.message.clone()
            } else {
                format!("line {}: {}", p.line, p.message)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    WorkflowDryRun {
        error: Some(error),
        missing_inputs: Vec::new(),
        calls: Vec::new(),
        stop: String::new(),
    }
}

/// What a dry run made, for the window.
pub(crate) fn dry_run_result(missing_inputs: Vec<String>, run: DryRun) -> WorkflowDryRun {
    WorkflowDryRun {
        error: None,
        missing_inputs,
        calls: run
            .calls
            .into_iter()
            .map(|call| WorkflowDryCall {
                func: call.func,
                level: call.level,
                args: call.args,
                executed: call.executed,
                assumed: call.assumed,
                outcome: call.outcome,
                note: call.note,
            })
            .collect(),
        stop: dry::end_line(&run.end),
    }
}
