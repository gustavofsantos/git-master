//! Lua scripts end to end (design/workflow-scripts.md): the real Luau sandbox, the real runtime,
//! the engine's real `Actions` over `git` in a temporary repository. A run's effects are posted
//! to the engine thread; this harness plays that thread, as the runner does.

use std::path::Path;
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::journal::{Journal, RunId, RunStatus};
use gm_core::script::policy::StartedBy;
use gm_core::script::run::{RunSpec, ScriptEvent};
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{Actor, Command, CommandKind, OperationId};
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository on `main` with one commit. Its own config holds the author.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// How the harness answers an approval request.
#[derive(Clone, Copy)]
enum Answer {
    Approve,
    Reject,
    /// Cancel the run instead of answering.
    Cancel,
}

struct Harness {
    engine: Engine,
    requests: Receiver<EffectRequest>,
    events: Arc<Mutex<Vec<ScriptEvent>>>,
    last: u64,
    answered: usize,
}

impl Harness {
    fn open(repo: &Path) -> Self {
        let (tx, requests) = mpsc::channel::<EffectRequest>();
        let tx = Mutex::new(tx);
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_script_link(ScriptLink {
            effects: Arc::new(move |request| {
                let _ = tx.lock().unwrap().send(request);
            }),
            events: Arc::new(move |event| sink.lock().unwrap().push(event)),
        });
        let mut harness = Self {
            engine,
            requests,
            events,
            last: 0,
            answered: 0,
        };
        harness.send(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        harness
    }

    fn send(&mut self, kind: CommandKind) {
        self.last += 1;
        self.engine.handle(
            OperationId(self.last),
            &Command {
                actor: Actor::Human,
                kind,
            },
        );
    }

    fn start(&mut self, source: &str, started_by: StartedBy) -> RunId {
        self.last += 1;
        self.engine
            .start_script(
                OperationId(self.last),
                RunSpec {
                    workflow: "t".into(),
                    chunk: "t.lua".into(),
                    source: source.into(),
                    definition_hash: "h".into(),
                    started_by,
                    trigger: json!({}),
                    input: json!({ "inputs": {}, "trigger": {}, "repo": {} }),
                    reviewed: true,
                    auto_approve: false,
                    seed: 1,
                },
            )
            .unwrap()
    }

    /// Plays the engine thread until the run ends: answers its effects and its approvals.
    fn finish(&mut self, answer: Answer) -> (RunStatus, Option<String>, Option<Value>) {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                Instant::now() < end,
                "the run did not end: {:?}",
                self.events.lock().unwrap()
            );
            match self.requests.recv_timeout(Duration::from_millis(20)) {
                Ok(request) => {
                    self.engine.script_effect(request);
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            let snapshot = self.events.lock().unwrap().clone();
            let asks: Vec<RunId> = snapshot
                .iter()
                .filter_map(|e| match e {
                    ScriptEvent::AwaitingApproval { run, .. } => Some(*run),
                    _ => None,
                })
                .collect();
            while self.answered < asks.len() {
                let runtime = self.engine.script_runtime().unwrap();
                match answer {
                    Answer::Cancel => {
                        runtime.cancel(asks[self.answered]);
                    }
                    Answer::Approve | Answer::Reject => {
                        runtime.approve(asks[self.answered], matches!(answer, Answer::Approve));
                    }
                }
                self.answered += 1;
            }
            if let Some(done) = snapshot.iter().find_map(|e| match e {
                ScriptEvent::Finished {
                    status,
                    reason,
                    value,
                    ..
                } => Some((*status, reason.clone(), value.clone())),
                _ => None,
            }) {
                self.engine.script_runtime().unwrap().join_all();
                return done;
            }
        }
    }

    fn journal(repo: &Path) -> Journal {
        Journal::open(&repo.join(".git").join("gitmaster").join("state.db")).unwrap()
    }
}

fn run_script(
    repo: &Path,
    source: &str,
    started_by: StartedBy,
    answer: Answer,
) -> (Harness, RunId, (RunStatus, Option<String>, Option<Value>)) {
    let mut harness = Harness::open(repo);
    let run = harness.start(source, started_by);
    let end = harness.finish(answer);
    (harness, run, end)
}

#[test]
fn a_manual_script_creates_and_checks_out_a_branch() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.branch.create({ name = "feat/from-script", checkout = true })
  ctx.notify("created")
end
return M
"#;
    let (harness, run, (status, reason, _)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done, "{reason:?}");
    assert_eq!(
        git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "feat/from-script"
    );
    // The write is in the command log, under the operation that started the run (D81).
    assert!(harness.engine.log().iter().any(|e| matches!(
        &e.kind,
        CommandKind::CreateBranch { name, checkout: true, .. } if name == "feat/from-script"
    )));
    let calls = Harness::journal(repo.path()).calls(run).unwrap();
    let names: Vec<&str> = calls.iter().map(|c| c.func.as_str()).collect();
    assert_eq!(names, ["branch.create", "notify"]);
    assert_eq!(calls[0].outcome.as_deref(), Some("done"));
    assert_eq!(
        calls[0].args,
        json!({ "name": "feat/from-script", "checkout": true })
    );
}

#[test]
fn stage_and_commit_write_through_the_engine() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    std::fs::write(repo.path().join("b.txt"), "two\n").unwrap();
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.stage.all()
  local c = ctx.commit.create({ message = "feat: from a script" })
  return c.commit
end
return M
"#;
    let (_, _, (status, reason, value)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done, "{reason:?}");
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "feat: from a script"
    );
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    let head = git(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        value,
        Some(json!(head.trim())),
        "the script got the new commit id"
    );
}

#[test]
fn a_script_that_only_reads_leaves_the_repository_alone() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let before = git(repo.path(), &["status", "--porcelain=v2", "--branch"]);
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.exec({ "git", "rev-parse", "--abbrev-ref", "HEAD" })
  ctx.notify("on " .. r.text)
  return r.text
end
return M
"#;
    let (_, _, (status, _, value)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done);
    assert_eq!(value, Some(json!("main")));
    assert_eq!(
        git(repo.path(), &["status", "--porcelain=v2", "--branch"]),
        before
    );
    assert!(!repo.path().join(".gitmaster").exists());
}

#[test]
fn a_failed_git_call_stops_the_run_unless_the_script_tries_it() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    git(repo.path(), &["branch", "taken"]);

    let strict = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.branch.create({ name = "taken" })
  ctx.notify("not reached")
end
return M
"#;
    let (_, run, (status, reason, _)) =
        run_script(repo.path(), strict, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Failed);
    assert!(
        reason
            .unwrap()
            .contains("ctx.branch.create ended with outcome failed")
    );
    let names: Vec<String> = Harness::journal(repo.path())
        .calls(run)
        .unwrap()
        .into_iter()
        .map(|c| c.func)
        .collect();
    assert_eq!(names, ["branch.create"], "nothing runs after the failure");

    let lenient = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.try(function() return ctx.branch.create({ name = "taken" }) end)
  return r.outcome
end
return M
"#;
    let (_, _, (status, _, value)) =
        run_script(repo.path(), lenient, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done);
    assert_eq!(value, Some(json!("failed")));
}

#[test]
fn after_an_exec_the_apply_waits_for_the_person() {
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.exec({ "printf", "feat/asked" })
  ctx.branch.create({ name = r.text })
end
return M
"#;
    // Approved: the branch appears.
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let (_, _, (status, _, _)) = run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done);
    assert!(git(repo.path(), &["branch", "--list", "feat/asked"]).contains("feat/asked"));

    // Rejected: it does not, and the run fails with the rejection.
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let (_, run, (status, reason, _)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Reject);
    assert_eq!(status, RunStatus::Failed);
    assert!(reason.unwrap().contains("rejected"));
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/asked"]), "");
    let calls = Harness::journal(repo.path()).calls(run).unwrap();
    assert_eq!(calls[1].outcome.as_deref(), Some("rejected"));
}

#[test]
fn an_event_run_asks_before_every_apply() {
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.branch.create({ name = "from-event" })
end
return M
"#;
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let (harness, _, (status, _, _)) = run_script(
        repo.path(),
        script,
        StartedBy::Event("commit.created".into()),
        Answer::Reject,
    );
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(git(repo.path(), &["branch", "--list", "from-event"]), "");
    assert!(harness.events.lock().unwrap().iter().any(
        |e| matches!(e, ScriptEvent::AwaitingApproval { func, .. } if func == "branch.create")
    ));
}

#[test]
fn suggest_message_reaches_the_front_ends_and_writes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.commit.suggest_message({ message = "feat: suggested" })
end
return M
"#;
    let (harness, _, (status, _, _)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Done);
    assert!(harness.events.lock().unwrap().iter().any(
        |e| matches!(e, ScriptEvent::SuggestMessage { message, .. } if message == "feat: suggested")
    ));
    assert_eq!(git(repo.path(), &["log", "--format=%s"]).trim(), "init");
}

#[test]
fn stack_actions_are_declared_but_not_available_yet() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = "main" })
end
return M
"#;
    let (_, _, (status, reason, _)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Approve);
    assert_eq!(status, RunStatus::Failed);
    assert!(reason.unwrap().contains("not available"));
}

#[test]
fn cancelling_wakes_a_script_blocked_in_approve_and_runs_nothing_after_it() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let script = r#"
local M = {}
M.meta = { name = "t", trigger = { on = "manual" } }
function M.execute(ctx, input)
  if ctx.approve("Create the branch?") then
    ctx.branch.create({ name = "never" })
  end
  ctx.notify("not reached")
end
return M
"#;
    let started = Instant::now();
    let (harness, run, (status, _, _)) =
        run_script(repo.path(), script, StartedBy::Human, Answer::Cancel);
    assert_eq!(status, RunStatus::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(git(repo.path(), &["branch", "--list", "never"]), "");
    let calls = Harness::journal(repo.path()).calls(run).unwrap();
    assert_eq!(calls.len(), 1, "nothing was called after the approve");
    assert_eq!(calls[0].outcome.as_deref(), Some("cancelled"));
    drop(harness);
}
