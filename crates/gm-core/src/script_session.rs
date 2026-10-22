//! The engine's side of script runs (design/workflow-scripts.md, D81, D100): the runtime of one
//! open repository, and the effects a run asks for. A run lives on its own thread, which cannot
//! touch the `GitPort`; it posts each effect to the engine thread and blocks for the answer, so
//! every Git write is a Command-shaped call in the command log under the operation that started
//! the run, as before (D81).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use gm_schema::OperationId;
use serde_json::{Value, json};

use crate::script::journal::{Journal, RunId};
use crate::script::luau::LuauEngine;
use crate::script::port::{Budgets, ScriptEngine};
use crate::script::run::{
    Clock, EffectCall, EffectResult, Effects, EventSink, Runtime, RuntimeConfig, ScriptEvent,
};
use crate::workflow::driver::GitActions;
use crate::workflow_session::Actions;
use crate::{GitPort, LogEntry};

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
}

impl ScriptSession {
    pub(crate) fn open(
        root: PathBuf,
        state_db: &std::path::Path,
        link: &ScriptLink,
        git: &dyn GitPort,
        clock: Clock,
        opened_by: OperationId,
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
        })
    }

    pub(crate) fn note_origin(&mut self, run: RunId, origin: OperationId) {
        self.origins.insert(run, origin);
    }

    fn origin(&self, run: RunId) -> OperationId {
        self.origins
            .get(&run)
            .copied()
            .unwrap_or(self.default_origin)
    }

    /// Carries out one effect on the engine thread. Returns the answer and whether Git was
    /// written, so the caller reads the status again.
    pub(crate) fn execute(
        &self,
        git: &dyn GitPort,
        log: &mut Vec<LogEntry>,
        request: EffectRequest,
    ) -> bool {
        let EffectRequest { call, reply } = request;
        let mut actions = Actions::new(git, &self.root, log, self.origin(call.run));
        let result = effect(&mut actions, &call, &self.events);
        let wrote = actions.wrote();
        // The run may be gone (cancelled while it waited for the engine); nobody needs the answer.
        let _ = reply.send(result);
        wrote
    }
}

fn text_arg<'a>(call: &'a EffectCall, key: &str) -> &'a str {
    call.args
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The effect of one `ctx` call. Arguments were checked against the catalog before the call got
/// here.
fn effect(actions: &mut Actions<'_>, call: &EffectCall, events: &EventSink) -> EffectResult {
    let into = |result: Result<Value, String>| match result {
        Ok(value) => EffectResult::done(value),
        Err(message) => EffectResult::failed(message),
    };
    match call.func.as_str() {
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
