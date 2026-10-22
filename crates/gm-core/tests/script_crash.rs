//! D100: the app stops in the middle of an `apply`. The next open shows the run as `interrupted`,
//! and nothing runs it again. The test kills a real process, so the journal has to survive a
//! SIGKILL, not an orderly shutdown.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gm_core::script::fake::FnEngine;
use gm_core::script::journal::{Journal, RunStatus};
use gm_core::script::policy::StartedBy;
use gm_core::script::port::{Budgets, CtxHost, HostReply, RunEnd};
use gm_core::script::run::{EffectCall, EffectResult, Effects, RunSpec, Runtime, RuntimeConfig};
use serde_json::{Value, json};

const CHILD_ENV: &str = "GM_SCRIPT_CRASH_DB";

/// An effect that never returns: the engine is "in the middle of" the write when it is killed.
struct Stuck {
    marker: PathBuf,
}

impl Effects for Stuck {
    fn effect(&self, _call: &EffectCall) -> EffectResult {
        std::fs::write(&self.marker, "started").unwrap();
        std::thread::sleep(Duration::from_secs(120));
        EffectResult::done(Value::Null)
    }
}

/// An effect that must not be reached after the restart.
struct Forbidden;

impl Effects for Forbidden {
    fn effect(&self, call: &EffectCall) -> EffectResult {
        panic!("{} ran again after the restart", call.func);
    }
}

fn runtime(db: &Path, effects: Arc<dyn Effects>, script: bool) -> Runtime {
    let engine = FnEngine(move |host: &mut dyn CtxHost| {
        if script {
            let _ = host.call("stage.all", json!({}));
            if let HostReply::End(_) = host.call("commit.create", json!({ "message": "m" })) {
                return RunEnd::Cancelled;
            }
        }
        RunEnd::Returned(Value::Null)
    });
    Runtime::new(RuntimeConfig {
        engine: Arc::new(engine),
        journal: Journal::open(db).unwrap(),
        effects,
        events: Arc::new(|_| {}),
        clock: Arc::new(|| 1_700_000_000_000),
        cwd: std::env::temp_dir(),
        login_path: Arc::new(OnceLock::new()),
        budgets: Budgets::default(),
    })
}

fn spec() -> RunSpec {
    RunSpec {
        workflow: "w".into(),
        chunk: "w.lua".into(),
        source: String::new(),
        definition_hash: "h".into(),
        started_by: StartedBy::Human,
        trigger: json!({}),
        input: json!({}),
        reviewed: true,
        auto_approve: false,
        seed: 1,
    }
}

#[test]
fn killing_the_engine_in_the_middle_of_an_apply_leaves_the_run_interrupted() {
    // Child: start the run and block inside the apply until the parent kills this process.
    if let Ok(db) = std::env::var(CHILD_ENV) {
        let db = PathBuf::from(db);
        let marker = db.with_extension("started");
        let rt = runtime(&db, Arc::new(Stuck { marker }), true);
        rt.start(spec()).unwrap();
        std::thread::sleep(Duration::from_secs(120));
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let marker = db.with_extension("started");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "killing_the_engine_in_the_middle_of_an_apply_leaves_the_run_interrupted",
            "--exact",
            "--nocapture",
        ])
        .env(CHILD_ENV, &db)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < end, "the child never reached the apply");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    // The next open.
    let rt = runtime(&db, Arc::new(Forbidden), false);
    let recovered = rt.recover();
    assert_eq!(recovered.len(), 1);
    let journal = Journal::open(&db).unwrap();
    let run = journal.run(recovered[0]).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Interrupted);
    assert_eq!(
        run.end_reason.as_deref(),
        Some("the app stopped while the run was in progress")
    );
    let calls = journal.calls(run.id).unwrap();
    assert_eq!(
        calls.len(),
        1,
        "the apply that never ended is the only call"
    );
    assert_eq!(calls[0].func, "stage.all");
    assert_eq!(calls[0].level, "apply");
    assert_eq!(calls[0].outcome.as_deref(), Some("interrupted"));
    assert_eq!(calls[0].result, None, "no result was recorded for it");

    // Nothing starts again by itself, and a second open changes nothing.
    assert!(rt.active().is_empty());
    assert!(rt.recover().is_empty());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(journal.calls(run.id).unwrap().len(), 1);
}
