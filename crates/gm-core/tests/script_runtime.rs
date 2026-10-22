//! The host side of script runs, with a closure in place of the VM: journal, approval rules,
//! blocking calls and cancellation (design/workflow-scripts.md, D99 to D101).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gm_core::script::fake::FnEngine;
use gm_core::script::journal::{Journal, NewRun, RunStatus};
use gm_core::script::policy::StartedBy;
use gm_core::script::port::{Budgets, CtxHost, Failure, HostEnd, HostReply, RunEnd, ScriptEngine};
use gm_core::script::run::{
    EffectCall, EffectResult, Effects, RunSpec, Runtime, RuntimeConfig, ScriptEvent,
};
use serde_json::{Value, json};

/// Answers each effect from a table and counts the calls.
#[derive(Default)]
struct Canned {
    calls: Mutex<Vec<EffectCall>>,
    answers: Mutex<Vec<(&'static str, EffectResult)>>,
}

impl Canned {
    fn answer(&self, func: &'static str, result: EffectResult) {
        self.answers.lock().unwrap().push((func, result));
    }

    fn funcs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.func.clone())
            .collect()
    }
}

impl Effects for Canned {
    fn effect(&self, call: &EffectCall) -> EffectResult {
        self.calls.lock().unwrap().push(call.clone());
        self.answers
            .lock()
            .unwrap()
            .iter()
            .find(|(func, _)| *func == call.func)
            .map_or_else(|| EffectResult::done(Value::Null), |(_, r)| r.clone())
    }
}

struct Rig {
    runtime: Runtime,
    journal: Journal,
    effects: Arc<Canned>,
    events: Arc<Mutex<Vec<ScriptEvent>>>,
}

fn rig(engine: impl ScriptEngine + 'static) -> Rig {
    let journal = Journal::open_in_memory().unwrap();
    let effects = Arc::new(Canned::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let runtime = Runtime::new(RuntimeConfig {
        engine: Arc::new(engine),
        journal: journal.clone(),
        effects: Arc::clone(&effects) as Arc<dyn Effects>,
        events: Arc::new(move |event| sink.lock().unwrap().push(event)),
        clock: Arc::new(|| 1_700_000_000_000),
        cwd: std::env::temp_dir(),
        login_path: Arc::new(OnceLock::new()),
        budgets: Budgets::default(),
    });
    Rig {
        runtime,
        journal,
        effects,
        events,
    }
}

fn spec(started_by: StartedBy) -> RunSpec {
    RunSpec {
        workflow: "w".into(),
        chunk: "w.lua".into(),
        source: "-- script".into(),
        definition_hash: "h".into(),
        started_by,
        trigger: json!({}),
        input: json!({}),
        reviewed: true,
        auto_approve: false,
        seed: 7,
    }
}

impl Rig {
    fn wait_for(&self, what: &str, pred: impl Fn(&[ScriptEvent]) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if pred(&self.events.lock().unwrap()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "timed out waiting for {what}; events: {:?}",
            self.events.lock().unwrap()
        );
    }

    fn awaiting(&self) -> (i64, String) {
        self.wait_for("an approval request", |events| {
            events
                .iter()
                .any(|e| matches!(e, ScriptEvent::AwaitingApproval { .. }))
        });
        self.events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                ScriptEvent::AwaitingApproval { run, func, .. } => Some((*run, func.clone())),
                _ => None,
            })
            .unwrap()
    }

    fn finished(&self) -> (RunStatus, Option<String>, Option<Value>) {
        self.wait_for("the end of the run", |events| {
            events
                .iter()
                .any(|e| matches!(e, ScriptEvent::Finished { .. }))
        });
        self.runtime.join_all();
        self.events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                ScriptEvent::Finished {
                    status,
                    reason,
                    value,
                    ..
                } => Some((*status, reason.clone(), value.clone())),
                _ => None,
            })
            .unwrap()
    }
}

/// How the VM raises a reply that is not a value.
fn raise(reply: HostReply) -> Result<Value, RunEnd> {
    match reply {
        HostReply::Value(v) => Ok(v),
        HostReply::Outcome {
            outcome, message, ..
        } => Err(RunEnd::Failed(Failure::Outcome {
            func: "x".into(),
            outcome,
            message,
        })),
        HostReply::Error(m) => Err(RunEnd::Failed(Failure::Script(m))),
        HostReply::End(HostEnd::Cancelled) => Err(RunEnd::Cancelled),
        HostReply::End(HostEnd::Blocked(r)) => Err(RunEnd::Blocked(r)),
        HostReply::End(HostEnd::Failed(r)) => Err(RunEnd::Failed(Failure::Reason(r))),
    }
}

macro_rules! step {
    ($host:expr, $func:expr, $args:expr) => {
        match raise($host.call($func, $args)) {
            Ok(v) => v,
            Err(end) => return end,
        }
    };
}

#[test]
fn a_run_is_journaled_from_the_calls_themselves() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "notify", json!({ "text": "hello" }));
        let now = step!(host, "now", json!({}));
        let echoed = step!(host, "exec", json!({ "argv": ["echo", "hi"] }));
        RunEnd::Returned(json!({ "now": now, "text": echoed["text"] }))
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (status, reason, value) = rig.finished();
    assert_eq!(status, RunStatus::Done, "{reason:?}");
    assert_eq!(
        value,
        Some(json!({ "now": 1_700_000_000_000_i64, "text": "hi" }))
    );

    let calls = rig.journal.calls(id).unwrap();
    let shape: Vec<(u32, &str, &str, Option<&str>)> = calls
        .iter()
        .map(|c| {
            (
                c.seq,
                c.func.as_str(),
                c.level.as_str(),
                c.outcome.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (1, "notify", "read", Some("done")),
            (2, "now", "helper", Some("done")),
            (3, "exec", "read", Some("done")),
        ]
    );
    assert_eq!(calls[0].args, json!({ "text": "hello" }));
    assert_eq!(calls[2].result.as_ref().unwrap()["exit_code"], 0);
    let row = rig.journal.run(id).unwrap().unwrap();
    assert_eq!(row.status, RunStatus::Done);
    assert_eq!(row.started_by, "human");
    assert_eq!(row.definition_text, "-- script");
    assert!(rig.runtime.active().is_empty());
}

#[test]
fn a_manual_run_applies_without_asking() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(
            host,
            "branch.create",
            json!({ "name": "feat/x", "checkout": true })
        );
        RunEnd::Returned(Value::Null)
    }));
    rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (status, _, _) = rig.finished();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(rig.effects.funcs(), ["branch.create"]);
}

#[test]
fn an_event_run_asks_and_applies_only_after_the_answer() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "stage.all", json!({}));
        RunEnd::Returned(Value::Null)
    }));
    let id = rig
        .runtime
        .start(spec(StartedBy::Event("commit.created".into())))
        .unwrap();
    let (run, func) = rig.awaiting();
    assert_eq!((run, func.as_str()), (id, "stage.all"));
    assert!(
        rig.effects.funcs().is_empty(),
        "nothing applies before the answer"
    );
    assert_eq!(
        rig.journal.run(id).unwrap().unwrap().status,
        RunStatus::AwaitingApproval
    );
    assert!(rig.runtime.approve(id, true));
    let (status, _, _) = rig.finished();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(rig.effects.funcs(), ["stage.all"]);
}

#[test]
fn a_rejected_apply_ends_the_run_and_never_runs_the_effect() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "stage.all", json!({}));
        RunEnd::Returned(Value::Null)
    }));
    let id = rig
        .runtime
        .start(spec(StartedBy::Agent("claude".into())))
        .unwrap();
    rig.awaiting();
    rig.runtime.approve(id, false);
    let (status, reason, _) = rig.finished();
    assert_eq!(status, RunStatus::Failed);
    assert!(reason.unwrap().contains("rejected"));
    assert!(rig.effects.funcs().is_empty());
    let calls = rig.journal.calls(id).unwrap();
    assert_eq!(calls[0].outcome.as_deref(), Some("rejected"));
}

#[test]
fn after_an_exec_even_a_manual_apply_asks() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let out = step!(host, "exec", json!({ "argv": ["echo", "feat/y"] }));
        step!(host, "branch.create", json!({ "name": out["text"] }));
        RunEnd::Returned(Value::Null)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (_, func) = rig.awaiting();
    assert_eq!(func, "branch.create");
    let asked = rig.events.lock().unwrap().iter().find_map(|e| match e {
        ScriptEvent::AwaitingApproval { args, .. } => Some(args.clone()),
        _ => None,
    });
    assert_eq!(
        asked,
        Some(json!({ "name": "feat/y" })),
        "the dialog shows the resolved arguments"
    );
    rig.runtime.approve(id, true);
    rig.finished();
    assert_eq!(rig.effects.funcs(), ["branch.create"]);
}

#[test]
fn the_grant_lets_an_event_run_apply_but_not_after_an_exec() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "stage.all", json!({}));
        RunEnd::Returned(Value::Null)
    }));
    let mut granted = spec(StartedBy::Event("commit.created".into()));
    granted.auto_approve = true;
    rig.runtime.start(granted).unwrap();
    let (status, _, _) = rig.finished();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(rig.effects.funcs(), ["stage.all"]);
}

#[test]
fn cancel_wakes_a_blocked_approve_at_once() {
    let after_cancel = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&after_cancel);
    let rig = rig(FnEngine(move |host: &mut dyn CtxHost| {
        step!(host, "approve", json!({ "summary": "ok?" }));
        counter.fetch_add(1, Ordering::SeqCst);
        RunEnd::Returned(Value::Null)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    rig.awaiting();
    let started = Instant::now();
    assert!(rig.runtime.cancel(id));
    let (status, _, _) = rig.finished();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(status, RunStatus::Cancelled);
    assert_eq!(
        after_cancel.load(Ordering::SeqCst),
        0,
        "no script code runs after the cancel"
    );
    assert_eq!(
        rig.journal.calls(id).unwrap()[0].outcome.as_deref(),
        Some("cancelled")
    );
}

#[test]
fn approve_returns_the_answer_to_the_script() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let answer = step!(host, "approve", json!({}));
        RunEnd::Returned(answer)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    rig.awaiting();
    rig.runtime.approve(id, false);
    let (status, _, value) = rig.finished();
    assert_eq!((status, value), (RunStatus::Done, Some(json!(false))));
    assert_eq!(
        rig.journal.calls(id).unwrap()[0].outcome.as_deref(),
        Some("rejected")
    );
}

#[test]
fn a_stray_approve_is_refused() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "exec", json!({ "argv": ["sleep", "0.3"] }));
        RunEnd::Returned(Value::Null)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    assert!(
        !rig.runtime.approve(id, true),
        "the run waits for no approval"
    );
    rig.finished();
    assert!(!rig.runtime.approve(id, true), "the run is over");
}

#[test]
fn cancel_kills_a_running_exec() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(
            host,
            "exec",
            json!({ "argv": ["sleep", "30"], "timeout": "1m" })
        );
        RunEnd::Returned(Value::Null)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    rig.runtime.cancel(id);
    let (status, _, _) = rig.finished();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(status, RunStatus::Cancelled);
}

#[test]
fn an_exec_timeout_or_failure_is_an_outcome_not_a_crash() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let reply = host.call("exec", json!({ "argv": ["sh", "-c", "exit 3"] }));
        RunEnd::Returned(match reply {
            HostReply::Outcome { outcome, value, .. } => {
                json!({ "outcome": outcome, "code": value["exit_code"] })
            }
            other => json!(format!("{other:?}")),
        })
    }));
    rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (_, _, value) = rig.finished();
    assert_eq!(value, Some(json!({ "outcome": "failed", "code": 3 })));
}

#[test]
fn wait_resumes_on_the_event_and_returns_the_outcome() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let w = step!(
            host,
            "wait",
            json!({ "event": "restack.resumed", "timeout": "1m" })
        );
        RunEnd::Returned(w)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    rig.wait_for("the wait", |events| {
        events.iter().any(|e| {
            matches!(
                e,
                ScriptEvent::Changed {
                    status: RunStatus::Waiting,
                    ..
                }
            )
        })
    });
    assert_eq!(rig.runtime.deliver("other.event", "resumed"), 0);
    assert_eq!(rig.runtime.deliver("restack.resumed", "resumed"), 1);
    let (status, _, value) = rig.finished();
    assert_eq!(
        (status, value),
        (RunStatus::Done, Some(json!({ "outcome": "resumed" })))
    );
    assert_eq!(
        rig.journal.calls(id).unwrap()[0].outcome.as_deref(),
        Some("resumed")
    );
}

#[test]
fn wait_times_out_without_raising() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let w = step!(
            host,
            "wait",
            json!({ "event": "restack.resumed", "timeout": "1s" })
        );
        RunEnd::Returned(w)
    }));
    let started = Instant::now();
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (status, _, value) = rig.finished();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(
        (status, value),
        (RunStatus::Done, Some(json!({ "outcome": "timeout" })))
    );
    assert_eq!(
        rig.journal.calls(id).unwrap()[0].outcome.as_deref(),
        Some("timeout")
    );
}

#[test]
fn cancel_wakes_a_wait() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        step!(host, "wait", json!({ "event": "restack.resumed" }));
        RunEnd::Returned(Value::Null)
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    rig.wait_for("the wait", |events| {
        events.iter().any(|e| {
            matches!(
                e,
                ScriptEvent::Changed {
                    status: RunStatus::Waiting,
                    ..
                }
            )
        })
    });
    rig.runtime.cancel(id);
    assert_eq!(rig.finished().0, RunStatus::Cancelled);
}

#[test]
fn a_version_that_was_not_reviewed_does_not_start() {
    let rig = rig(FnEngine(|_: &mut dyn CtxHost| {
        RunEnd::Returned(Value::Null)
    }));
    let mut unreviewed = spec(StartedBy::Human);
    unreviewed.reviewed = false;
    let error = rig.runtime.start(unreviewed).unwrap_err();
    assert!(error.contains("not been reviewed"), "{error}");
    assert!(rig.journal.runs("w", 10).unwrap().is_empty());
}

#[test]
fn an_outcome_the_catalog_does_not_declare_becomes_failed() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let reply = host.call("branch.create", json!({ "name": "x" }));
        RunEnd::Returned(json!(format!("{reply:?}")))
    }));
    rig.effects.answer(
        "branch.create",
        EffectResult {
            outcome: "conflict".into(),
            value: Value::Null,
            message: "boom".into(),
        },
    );
    rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (_, _, value) = rig.finished();
    let text = value.unwrap();
    assert!(
        text.as_str().unwrap().contains("does not declare"),
        "{text}"
    );
}

#[test]
fn a_declared_non_done_outcome_reaches_the_script() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let reply = host.call("restack.apply", json!({ "plan_id": "p" }));
        RunEnd::Returned(match reply {
            HostReply::Outcome {
                outcome, message, ..
            } => json!([outcome, message]),
            other => json!(format!("{other:?}")),
        })
    }));
    rig.effects.answer(
        "restack.apply",
        EffectResult {
            outcome: "conflict".into(),
            value: Value::Null,
            message: "src/a.rs".into(),
        },
    );
    rig.runtime.start(spec(StartedBy::Human)).unwrap();
    assert_eq!(rig.finished().2, Some(json!(["conflict", "src/a.rs"])));
}

#[test]
fn bad_arguments_are_a_catchable_error_and_no_effect_runs() {
    let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
        let reply = host.call("branch.create", json!({ "nme": "x" }));
        RunEnd::Returned(match reply {
            HostReply::Error(message) => json!(message),
            other => json!(format!("{other:?}")),
        })
    }));
    let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (_, _, value) = rig.finished();
    let message = value.unwrap();
    assert!(
        message.as_str().unwrap().contains("unknown field `nme`"),
        "{message}"
    );
    assert!(rig.effects.funcs().is_empty());
    assert_eq!(
        rig.journal.calls(id).unwrap()[0].outcome.as_deref(),
        Some("error")
    );
}

#[test]
fn block_and_fail_end_the_run_with_their_reason() {
    for (func, expected) in [("block", RunStatus::Blocked), ("fail", RunStatus::Failed)] {
        let rig = rig(FnEngine(move |host: &mut dyn CtxHost| {
            step!(host, func, json!({ "reason": "because" }));
            RunEnd::Returned(Value::Null)
        }));
        rig.runtime.start(spec(StartedBy::Human)).unwrap();
        let (status, reason, _) = rig.finished();
        assert_eq!(
            (status, reason.as_deref()),
            (expected, Some("because")),
            "{func}"
        );
    }
}

#[test]
fn a_panicking_engine_fails_the_run_instead_of_taking_the_app_down() {
    let rig = rig(FnEngine(|_: &mut dyn CtxHost| -> RunEnd {
        panic!("vm bug")
    }));
    rig.runtime.start(spec(StartedBy::Human)).unwrap();
    let (status, reason, _) = rig.finished();
    assert_eq!(status, RunStatus::Failed);
    assert!(reason.unwrap().contains("panicked"));
}

#[test]
fn random_is_seeded_and_repeatable() {
    let run = |seed: u64| {
        let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
            let a = step!(host, "random", json!({}));
            let b = step!(host, "random", json!({}));
            RunEnd::Returned(json!([a, b]))
        }));
        let mut s = spec(StartedBy::Human);
        s.seed = seed;
        rig.runtime.start(s).unwrap();
        rig.finished().2.unwrap()
    };
    assert_eq!(run(1), run(1));
    assert_ne!(run(1), run(2));
    for x in run(3).as_array().unwrap() {
        let x = x.as_f64().unwrap();
        assert!((0.0..1.0).contains(&x));
    }
}

#[test]
fn the_same_script_with_the_same_results_makes_the_same_calls() {
    let play = || {
        let rig = rig(FnEngine(|host: &mut dyn CtxHost| {
            let a = step!(host, "exec", json!({ "argv": ["echo", "a"] }));
            let t = step!(host, "now", json!({}));
            step!(
                host,
                "commit.suggest_message",
                json!({ "message": a["text"] })
            );
            step!(host, "notify", json!({ "text": t.to_string() }));
            RunEnd::Returned(Value::Null)
        }));
        rig.effects.answer(
            "commit.suggest_message",
            EffectResult::done(json!({ "ok": 1 })),
        );
        let id = rig.runtime.start(spec(StartedBy::Human)).unwrap();
        rig.finished();
        rig.journal
            .calls(id)
            .unwrap()
            .into_iter()
            .map(|c| (c.seq, c.func, c.args, c.result, c.outcome))
            .collect::<Vec<_>>()
    };
    assert_eq!(play(), play());
}

#[test]
fn a_restart_marks_unfinished_runs_interrupted() {
    let rig = rig(FnEngine(|_: &mut dyn CtxHost| {
        RunEnd::Returned(Value::Null)
    }));
    let stale = rig
        .journal
        .create_run(
            &NewRun {
                workflow: "w",
                definition_hash: "h",
                definition_text: "",
                trigger: &json!({}),
                started_by: "human",
                seed: 1,
            },
            1,
        )
        .unwrap();
    assert_eq!(rig.runtime.recover(), vec![stale]);
    assert_eq!(
        rig.journal.run(stale).unwrap().unwrap().status,
        RunStatus::Interrupted
    );
    assert!(rig.events.lock().unwrap().iter().any(|e| matches!(
        e,
        ScriptEvent::Changed { run, status: RunStatus::Interrupted, .. } if *run == stale
    )));
}
