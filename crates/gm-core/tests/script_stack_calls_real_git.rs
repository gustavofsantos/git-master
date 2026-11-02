//! The stack calls of workflow scripts (`WF4a`, D68, D142, D154): `ctx.stack.*`, `ctx.restack.*`,
//! `ctx.reparent.*`, `ctx.fetch` and `ctx.push.lease`, run by the engine against real repositories in
//! temporary directories. This harness plays the engine thread that carries a run's effects, as the
//! runner does. Each test removes its directories when it ends.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink, ScriptReport};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowEnd, WorkflowInfo,
};

/// Writes an executable script from a child process. Writing it here and running it at once
/// races with other test threads' forks, which inherit the open file and make the exec fail
/// with "Text file busy" (ETXTBSY).
#[cfg(unix)]
fn write_script(path: &Path, text: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sh");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository on `main` with one commit that adds `a.txt`.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    write_and_commit(dir, "a.txt", "one\n", "init");
}

fn write_and_commit(dir: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn rev(dir: &Path, name: &str) -> String {
    git(dir, &["rev-parse", name])
}

fn config(dir: &Path, key: &str) -> String {
    git(dir, &["config", "--local", key])
}

/// The commit `name` is on `main` (an ancestor of it or the same).
fn descends_from(dir: &Path, name: &str, ancestor: &str) -> bool {
    Process::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, name])
        .current_dir(dir)
        .status()
        .unwrap()
        .success()
}

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
    /// A Git command a run waits for ended, as the runner's script thread reports it.
    Remote(ScriptReport),
}

/// The engine of one repository, with a channel that carries its runs' effects and events.
struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    tx: mpsc::Sender<Message>,
    last: u64,
}

impl Harness {
    fn open(repo: &Path, flows: &Path) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx.clone());
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(flows.to_path_buf()));
        engine.set_trust_store(TrustStore::open_in_memory().unwrap());
        engine.set_script_link(ScriptLink {
            effects: Arc::new(move |request| {
                let _ = effects_tx.lock().unwrap().send(Message::Effect(request));
            }),
            events: Arc::new(move |event| {
                let _ = events_tx.lock().unwrap().send(Message::Event(event));
            }),
        });
        engine.set_triggers(true);
        let mut harness = Self {
            engine,
            messages,
            tx,
            last: 0,
        };
        let events = harness.send(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        assert_eq!(failure(&events), None, "open: {events:?}");
        harness
    }

    fn send(&mut self, kind: CommandKind) -> Vec<Event> {
        self.last += 1;
        self.engine.handle(
            OperationId(self.last),
            &Command {
                actor: Actor::Human,
                kind,
            },
        )
    }

    /// A Git command the person runs by hand, between runs.
    fn ok(&mut self, kind: CommandKind) {
        let events = self.send(kind);
        assert_eq!(failure(&events), None, "{events:?}");
    }

    /// Starts the Git commands the engine queued for runs, each on a thread of its own, as the
    /// runner does (`WF4a`).
    fn spawn_jobs(&mut self) {
        for job in self.engine.take_script_jobs() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Message::Remote(job.run()));
            });
        }
    }

    fn handle(&mut self, message: Message) -> Vec<Event> {
        match message {
            Message::Effect(request) => {
                let events = self.engine.script_effect(request);
                self.spawn_jobs();
                events
            }
            Message::Event(event) => self.engine.script_event(event),
            Message::Remote(report) => self.engine.script_remote_report(report),
        }
    }

    /// Plays the engine thread until `done` says the events so far are enough, or panics.
    fn pump_until(
        &mut self,
        mut events: Vec<Event>,
        done: impl Fn(&[Event]) -> bool,
    ) -> Vec<Event> {
        let end = Instant::now() + Duration::from_secs(30);
        while !done(&events) {
            assert!(Instant::now() < end, "timed out; events: {events:#?}");
            if let Ok(message) = self.messages.recv_timeout(Duration::from_millis(20)) {
                events.extend(self.handle(message));
            }
        }
        events
    }

    /// Plays the engine thread until `file` exists, or panics.
    fn pump_until_file(&mut self, events: &mut Vec<Event>, file: &Path) {
        let end = Instant::now() + Duration::from_secs(30);
        while !file.exists() {
            assert!(
                Instant::now() < end,
                "timed out waiting for {}",
                file.display()
            );
            if let Ok(message) = self.messages.recv_timeout(Duration::from_millis(5)) {
                events.extend(self.handle(message));
            }
        }
    }

    fn listing(&mut self) -> Vec<WorkflowInfo> {
        listed(&self.send(CommandKind::ListWorkflows))
    }

    /// Reviews the script `name` as the listing shows it, without automatic approval.
    fn review(&mut self, name: &str) {
        let hash = self
            .listing()
            .iter()
            .find(|w| w.name == name)
            .unwrap_or_else(|| panic!("no workflow {name}"))
            .hash
            .clone();
        let events = self.send(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert_eq!(failure(&events), None, "{events:?}");
    }

    /// Runs `name` by hand with `inputs`, answering each approval with `approve` (`None` leaves the
    /// run waiting at its first approval). Returns the operation that started the run, and the events.
    fn run(
        &mut self,
        name: &str,
        inputs: &[(&str, &str)],
        approve: Option<bool>,
    ) -> (u64, Vec<Event>) {
        let op = self.last + 1;
        let events = self.send(CommandKind::RunWorkflow {
            name: name.to_owned(),
            inputs: inputs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        });
        let events = self.play(events, approve);
        (op, events)
    }

    /// Plays the runs until they finish, or until an approval is left unanswered (`approve` is `None`).
    fn play(&mut self, mut events: Vec<Event>, approve: Option<bool>) -> Vec<Event> {
        let mut answered = 0;
        loop {
            events = self.pump_until(events, |events| finished(events) || asks(events) > answered);
            if finished(&events) {
                self.engine.script_runtime().unwrap().join_all();
                return events;
            }
            answered += 1;
            let Some(approved) = approve else {
                return events;
            };
            let run = events
                .iter()
                .rev()
                .find_map(|e| match e {
                    Event::WorkflowApprovalRequested { run, .. } => Some(*run),
                    _ => None,
                })
                .unwrap();
            let answer = self.send(CommandKind::ApproveWorkflowStep { run, approved });
            events.extend(answer);
        }
    }
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::WorkflowRunFinished { .. }))
}

fn asks(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::WorkflowApprovalRequested { .. }))
        .count()
}

/// The approval the run asked for, as its `state` (the call's name).
fn asked(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowApprovalRequested { state, .. } => Some(state.clone()),
            _ => None,
        })
        .collect()
}

fn end_of(events: &[Event]) -> WorkflowEnd {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunFinished { end, .. } => Some(*end),
            _ => None,
        })
        .expect("the run finished")
}

/// What the run said with `ctx.notify`, in order.
fn notes(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

fn listed(events: &[Event]) -> Vec<WorkflowInfo> {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowsListed { workflows, .. } => Some(workflows.clone()),
            _ => None,
        })
        .expect("a listing")
}

/// The command that starts the workflow `name` by hand with `inputs`.
fn run_command(name: &str, inputs: &[(&str, &str)]) -> CommandKind {
    CommandKind::RunWorkflow {
        name: name.to_owned(),
        inputs: inputs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

/// `main` with `a` under it (commit `a1`) and `b` under `a` (commit `b1`), HEAD on `main`.
fn stack(repo: &Path) -> Harness {
    init_repo(repo);
    let flows = repo.with_extension("flows");
    std::fs::create_dir_all(&flows).unwrap();
    let mut harness = Harness::open(repo, &flows);
    harness.ok(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    git(repo, &["checkout", "-q", "a"]);
    write_and_commit(repo, "a1.txt", "a1\n", "a1");
    harness.ok(CommandKind::CreateChildBranch {
        name: "b".into(),
        parent: "a".into(),
        checkout: false,
    });
    git(repo, &["checkout", "-q", "b"]);
    write_and_commit(repo, "b1.txt", "b1\n", "b1");
    git(repo, &["checkout", "-q", "main"]);
    harness
}

/// Moves `main` with a commit of its own, so `a` is stale.
fn move_main(repo: &Path) {
    write_and_commit(repo, "m1.txt", "m1\n", "m1");
}

fn flows_dir(repo: &Path) -> PathBuf {
    repo.with_extension("flows")
}

fn write_flow(repo: &Path, name: &str, text: &str) {
    write(&flows_dir(repo), &format!("{name}.lua"), text);
}

/// The script for `name`, with the inputs its `execute` reads.
const TREE: &str = r#"
local M = {}
M.meta = { name = "tree", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local t = ctx.stack.tree()
  local parts = {}
  for _, b in ipairs(t.branches) do
    parts[#parts + 1] = b.name .. "=" .. tostring(b.stale)
  end
  ctx.notify(table.concat(parts, " "))
  local s = ctx.stack.stale()
  ctx.notify("stale " .. table.concat(s.branches, ","))
end
return M
"#;

const RESTACK_A: &str = r#"
local M = {}
M.meta = { name = "restack-a", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = "a" })
  ctx.notify("plan " .. plan.plan_id .. " " .. plan.branch_count)
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  ctx.notify("outcome " .. r.outcome)
  if r.outcome ~= "done" then return ctx.fail("restack " .. r.outcome) end
end
return M
"#;

const APPLY_GIVEN: &str = r#"
local M = {}
M.meta = { name = "apply-given", trigger = { on = "manual" },
  inputs = { plan = { type = "string", prompt = "Plan" } } }
function M.execute(ctx, input)
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = input.inputs.plan }) end)
  ctx.notify("outcome " .. r.outcome)
end
return M
"#;

const UNDO_GIVEN: &str = r#"
local M = {}
M.meta = { name = "undo-given", trigger = { on = "manual" },
  inputs = { plan = { type = "string", prompt = "Plan" } } }
function M.execute(ctx, input)
  local r = ctx.try(function() return ctx.restack.undo({ plan_id = input.inputs.plan }) end)
  ctx.notify("outcome " .. r.outcome)
  if r.outcome ~= "done" then return ctx.fail("undo " .. r.outcome) end
end
return M
"#;

const PLAN_ROOT_MAIN: &str = r#"
local M = {}
M.meta = { name = "plan-main", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = "main" })
  ctx.notify("plan " .. plan.plan_id .. " " .. plan.branch_count)
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  ctx.notify("outcome " .. r.outcome)
end
return M
"#;

const REPARENT_B: &str = r#"
local M = {}
M.meta = { name = "reparent-b", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local plan = ctx.reparent.plan({ branch = "b", parent = "main" })
  ctx.notify("plan " .. plan.plan_id .. " " .. plan.branch_count)
  local r = ctx.try(function() return ctx.reparent.apply({ plan_id = plan.plan_id }) end)
  ctx.notify("outcome " .. r.outcome)
  if r.outcome ~= "done" then return ctx.fail("reparent " .. r.outcome) end
end
return M
"#;

const FETCH: &str = r#"
local M = {}
M.meta = { name = "fetch", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.try(function() return ctx.fetch({ remote = "--upload-pack=x" }) end)
  ctx.notify("dash " .. r.outcome)
  ctx.fetch({})
end
return M
"#;

const PUSH_LEASE: &str = r#"
local M = {}
M.meta = { name = "push-feat", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.push.lease({ branch = "feat" })
end
return M
"#;

const RESTACK_ON_COMMIT: &str = r#"
local M = {}
M.meta = { name = "restack-on-commit", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = "main" })
  ctx.restack.apply({ plan_id = plan.plan_id })
end
return M
"#;

#[test]
fn stack_tree_and_stale_read_the_stacks_and_the_moved_parent() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    move_main(&repo);
    write_flow(&repo, "tree", TREE);
    harness.review("tree");

    let (_, events) = harness.run("tree", &[], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(
        notes(&events),
        ["main=false a=true b=false", "stale a"],
        "the tree is in stack order; only a moved its parent"
    );
}

#[test]
fn restack_plan_and_apply_move_a_and_b_and_are_logged_under_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    move_main(&repo);
    let old_a = rev(&repo, "a");
    let old_b = rev(&repo, "b");
    write_flow(&repo, "restack-a", RESTACK_A);
    harness.review("restack-a");

    let (op, events) = harness.run("restack-a", &[], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert!(
        notes(&events).contains(&"outcome done".to_owned()),
        "{:?}",
        notes(&events)
    );
    assert!(
        notes(&events)[0].ends_with(" 2"),
        "two branches move: {:?}",
        notes(&events)
    );
    assert_ne!(rev(&repo, "a"), old_a);
    assert_ne!(rev(&repo, "b"), old_b);
    assert!(
        descends_from(&repo, "a", "main"),
        "a sits on main's new tip"
    );
    assert!(descends_from(&repo, "b", "a"), "b sits on a's new tip");
    assert_eq!(
        config(&repo, "branch.a.stackParentBase"),
        rev(&repo, "main")
    );
    assert_eq!(config(&repo, "branch.b.stackParentBase"), rev(&repo, "a"));
    assert_eq!(git(&repo, &["log", "-1", "--format=%s", "b"]), "b1");
    let logged = harness.engine.log().iter().any(|entry| {
        entry.id == OperationId(op)
            && entry.actor == Actor::Human
            && matches!(entry.kind, CommandKind::ApplyRestack { .. })
    });
    assert!(
        logged,
        "the restack is in the command log under the run's operation (D81)"
    );
}

#[test]
fn an_apply_after_a_branch_moved_by_hand_ends_stale_and_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    move_main(&repo);
    write_flow(&repo, "plan-main", PLAN_ROOT_MAIN);
    write_flow(&repo, "apply-given", APPLY_GIVEN);
    harness.review("plan-main");
    harness.review("apply-given");
    // Plan only: the plan's id is in the first run's notes, and nothing moves.
    let (_, events) = harness.run("plan-main", &[], Some(true));
    let plan = notes(&events)[0].split(' ').nth(1).unwrap().to_owned();
    let old_a = rev(&repo, "a");
    // A branch moves by hand after the plan was made.
    git(&repo, &["checkout", "-q", "a"]);
    write_and_commit(&repo, "a2.txt", "a2\n", "a2");
    git(&repo, &["checkout", "-q", "main"]);
    let moved_a = rev(&repo, "a");
    assert_ne!(moved_a, old_a);

    let (_, events) = harness.run("apply-given", &[("plan", &plan)], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(notes(&events), ["outcome stale"]);
    assert_eq!(
        rev(&repo, "a"),
        moved_a,
        "a stays where it was moved by hand"
    );
}

#[test]
fn an_unknown_plan_id_ends_stale() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    write_flow(&repo, "apply-given", APPLY_GIVEN);
    harness.review("apply-given");

    let (_, events) = harness.run("apply-given", &[("plan", "0123456789abcdef")], Some(true));

    assert_eq!(notes(&events), ["outcome stale"]);
}

#[test]
fn a_conflict_moves_the_clean_sibling_and_leaves_the_conflicting_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    let flows = flows_dir(&repo);
    std::fs::create_dir_all(&flows).unwrap();
    let mut harness = Harness::open(&repo, &flows);
    // a changes a.txt; c, a sibling of a under main, adds its own file.
    harness.ok(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    git(&repo, &["checkout", "-q", "a"]);
    write_and_commit(&repo, "a.txt", "a-side\n", "a1");
    harness.ok(CommandKind::CreateChildBranch {
        name: "b".into(),
        parent: "a".into(),
        checkout: false,
    });
    git(&repo, &["checkout", "-q", "b"]);
    write_and_commit(&repo, "b1.txt", "b1\n", "b1");
    git(&repo, &["checkout", "-q", "main"]);
    harness.ok(CommandKind::CreateChildBranch {
        name: "c".into(),
        parent: "main".into(),
        checkout: false,
    });
    git(&repo, &["checkout", "-q", "c"]);
    write_and_commit(&repo, "c1.txt", "c1\n", "c1");
    git(&repo, &["checkout", "-q", "main"]);
    // main changes the same line, so a's commit conflicts when it is replayed.
    write_and_commit(&repo, "a.txt", "main-side\n", "m1");
    let (old_a, old_b, old_c) = (rev(&repo, "a"), rev(&repo, "b"), rev(&repo, "c"));
    write_flow(&repo, "plan-main", PLAN_ROOT_MAIN);
    harness.review("plan-main");

    let (_, events) = harness.run("plan-main", &[], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert!(
        notes(&events).iter().any(|n| n == "outcome conflict"),
        "{:?}",
        notes(&events)
    );
    assert_eq!(rev(&repo, "a"), old_a, "the conflicting branch stays");
    assert_eq!(rev(&repo, "b"), old_b, "and the branch under it");
    assert_ne!(rev(&repo, "c"), old_c, "the clean sibling moved");
    assert!(descends_from(&repo, "c", "main"));
    assert_eq!(
        config(&repo, "branch.c.stackParentBase"),
        rev(&repo, "main")
    );
}

#[test]
fn restack_undo_with_the_right_plan_id_restores_and_a_wrong_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    move_main(&repo);
    let (old_a, old_b) = (rev(&repo, "a"), rev(&repo, "b"));
    let old_base_a = config(&repo, "branch.a.stackParentBase");
    write_flow(&repo, "restack-a", RESTACK_A);
    write_flow(&repo, "undo-given", UNDO_GIVEN);
    harness.review("restack-a");
    harness.review("undo-given");
    let (_, events) = harness.run("restack-a", &[], Some(true));
    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    let plan = notes(&events)[0].split(' ').nth(1).unwrap().to_owned();
    let applied_a = rev(&repo, "a");

    let (_, events) = harness.run("undo-given", &[("plan", "0123456789abcdef")], Some(true));
    assert_eq!(
        end_of(&events),
        WorkflowEnd::Failed,
        "a wrong id fails: {events:?}"
    );
    assert_eq!(rev(&repo, "a"), applied_a, "nothing is undone");

    let (_, events) = harness.run("undo-given", &[("plan", &plan)], Some(true));
    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(rev(&repo, "a"), old_a);
    assert_eq!(rev(&repo, "b"), old_b);
    assert_eq!(config(&repo, "branch.a.stackParentBase"), old_base_a);
    assert_eq!(config(&repo, "branch.a.stackParent"), "main");
}

#[test]
fn reparent_moves_b_from_a_onto_main_and_undo_puts_the_old_parent_back() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    let (old_b, old_a) = (rev(&repo, "b"), rev(&repo, "a"));
    let old_base_b = config(&repo, "branch.b.stackParentBase");
    assert_eq!(config(&repo, "branch.b.stackParent"), "a");
    write_flow(&repo, "reparent-b", REPARENT_B);
    write_flow(&repo, "undo-given", UNDO_GIVEN);
    harness.review("reparent-b");
    harness.review("undo-given");

    let (_, events) = harness.run("reparent-b", &[], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(config(&repo, "branch.b.stackParent"), "main");
    assert_eq!(
        config(&repo, "branch.b.stackParentBase"),
        rev(&repo, "main")
    );
    assert!(descends_from(&repo, "b", "main"));
    assert_eq!(git(&repo, &["log", "--format=%s", "main..b"]), "b1");
    assert_eq!(rev(&repo, "a"), old_a, "a is not moved");
    let plan = notes(&events)[0].split(' ').nth(1).unwrap().to_owned();

    let (_, events) = harness.run("undo-given", &[("plan", &plan)], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(rev(&repo, "b"), old_b);
    assert_eq!(
        config(&repo, "branch.b.stackParent"),
        "a",
        "the old parent is back"
    );
    assert_eq!(config(&repo, "branch.b.stackParentBase"), old_base_b);
}

#[test]
fn fetch_updates_the_remote_tracking_branches_and_refuses_an_option_as_a_remote() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("repo");
    let other = dir.path().join("other");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_and_commit(&other, "x.txt", "x\n", "remote work");
    git(&other, &["push", "-q", "origin", "main"]);
    let remote_tip = rev(&other, "main");
    assert_ne!(rev(&repo, "origin/main"), remote_tip);
    let flows = flows_dir(&repo);
    std::fs::create_dir_all(&flows).unwrap();
    let mut harness = Harness::open(&repo, &flows);
    write_flow(&repo, "fetch", FETCH);
    harness.review("fetch");

    let (_, events) = harness.run("fetch", &[], Some(true));

    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(notes(&events), ["dash failed"]);
    assert_eq!(rev(&repo, "origin/main"), remote_tip);
}

#[test]
fn push_lease_asks_every_time_and_pushes_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("repo");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    write_and_commit(&repo, "f1.txt", "f1\n", "f1");
    git(&repo, &["push", "-q", "-u", "origin", "feat"]);
    write_and_commit(&repo, "f2.txt", "f2\n", "f2");
    git(&repo, &["checkout", "-q", "main"]);
    let local = rev(&repo, "feat");
    let flows = flows_dir(&repo);
    std::fs::create_dir_all(&flows).unwrap();
    let mut harness = Harness::open(&repo, &flows);
    write_flow(&repo, "push-feat", PUSH_LEASE);
    harness.review("push-feat");

    let (_, events) = harness.run("push-feat", &[], Some(true));

    assert_eq!(asked(&events), ["push.lease"], "push always asks");
    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(rev(&origin, "feat"), local, "the remote has the new commit");
}

#[test]
fn push_lease_fails_when_the_remote_moved_since_the_last_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("repo");
    let other = dir.path().join("other");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    write_and_commit(&repo, "f1.txt", "f1\n", "f1");
    git(&repo, &["push", "-q", "-u", "origin", "feat"]);
    git(&repo, &["checkout", "-q", "main"]);
    // Someone else pushes to feat; this repository does not fetch it.
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            "-b",
            "feat",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_and_commit(&other, "theirs.txt", "theirs\n", "theirs");
    git(&other, &["push", "-q", "origin", "feat"]);
    let theirs = rev(&other, "feat");
    // This repository commits on feat too.
    git(&repo, &["checkout", "-q", "feat"]);
    write_and_commit(&repo, "f2.txt", "f2\n", "f2");
    git(&repo, &["checkout", "-q", "main"]);
    let flows = flows_dir(&repo);
    std::fs::create_dir_all(&flows).unwrap();
    let mut harness = Harness::open(&repo, &flows);
    write_flow(&repo, "push-feat", PUSH_LEASE);
    harness.review("push-feat");

    let (_, events) = harness.run("push-feat", &[], Some(true));

    assert_eq!(asked(&events), ["push.lease"]);
    assert_eq!(end_of(&events), WorkflowEnd::Failed, "{events:?}");
    assert_eq!(rev(&origin, "feat"), theirs, "the lease kept their commit");
}

#[test]
fn an_event_run_waits_for_approval_before_it_restacks() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut harness = stack(&repo);
    let old_a = rev(&repo, "a");
    write_flow(&repo, "restack-on-commit", RESTACK_ON_COMMIT);
    harness.review("restack-on-commit");
    // A commit on main from the window starts the event run.
    write(&repo, "m1.txt", "m1\n");
    git(&repo, &["add", "-A"]);
    let events = harness.send(CommandKind::Commit {
        message: "m1".into(),
    });
    assert_eq!(failure(&events), None, "{events:?}");

    let events = harness.play(events, None);

    assert_eq!(
        asked(&events),
        ["restack.apply"],
        "an event run waits (D101)"
    );
    assert_eq!(rev(&repo, "a"), old_a, "nothing moves before the approval");
}

/// The script that fetches and then says so.
const FETCH_SLOW: &str = r#"
local M = {}
M.meta = { name = "fetch-slow", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.fetch({})
  ctx.notify("fetched")
end
return M
"#;

/// A remote whose fetch takes two seconds: `repo` tracks `origin`, whose upload-pack is a wrapper that
/// touches `started` and then sleeps. `other` pushed a commit, `tip`, that the fetch brings in.
#[cfg(unix)]
struct SlowRemote {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    flows: PathBuf,
    started: PathBuf,
    tip: String,
}

#[cfg(unix)]
fn slow_remote() -> SlowRemote {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("repo");
    let other = dir.path().join("other");
    let started = dir.path().join("started");
    let flows = dir.path().join("flows");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write_and_commit(&other, "x.txt", "x\n", "remote work");
    git(&other, &["push", "-q", "origin", "main"]);
    let tip = rev(&other, "main");
    let wrapper = dir.path().join("slow-upload-pack");
    write_script(
        &wrapper,
        &format!(
            "#!/bin/sh\ntouch '{}'\nsleep 2\nexec git upload-pack \"$@\"\n",
            started.display()
        ),
    );
    git(
        &repo,
        &[
            "config",
            "remote.origin.uploadpack",
            wrapper.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&flows).unwrap();
    SlowRemote {
        _dir: dir,
        repo,
        flows,
        started,
        tip,
    }
}

#[cfg(unix)]
#[test]
fn a_fetch_is_answered_when_git_ends_and_a_read_meanwhile_is_answered_at_once() {
    let slow = slow_remote();
    let mut harness = Harness::open(&slow.repo, &slow.flows);
    write(&slow.flows, "fetch-slow.lua", FETCH_SLOW);
    harness.review("fetch-slow");
    let before = rev(&slow.repo, "origin/main");

    let mut events = harness.send(run_command("fetch-slow", &[]));
    harness.pump_until_file(&mut events, &slow.started);

    // The fetch is in flight: a read is answered at once, and the run has not finished.
    let asked = Instant::now();
    let answer = harness.send(CommandKind::RefreshStatus);
    let took = asked.elapsed();
    assert_eq!(failure(&answer), None, "{answer:?}");
    assert!(took < Duration::from_secs(1), "the read waited {took:?}");
    assert!(!finished(&events), "the run waits for its fetch");
    assert_eq!(
        rev(&slow.repo, "origin/main"),
        before,
        "the fetch has not finished"
    );

    let events = harness.pump_until(events, finished);
    assert_eq!(end_of(&events), WorkflowEnd::Done, "{events:?}");
    assert_eq!(notes(&events), ["fetched"]);
    assert_eq!(rev(&slow.repo, "origin/main"), slow.tip);
}

#[cfg(unix)]
#[test]
fn cancelling_a_run_during_a_fetch_stops_git_and_ends_the_run_promptly() {
    let slow = slow_remote();
    let mut harness = Harness::open(&slow.repo, &slow.flows);
    write(&slow.flows, "fetch-slow.lua", FETCH_SLOW);
    harness.review("fetch-slow");
    let before = rev(&slow.repo, "origin/main");

    let op = harness.last + 1;
    let mut events = harness.send(run_command("fetch-slow", &[]));
    harness.pump_until_file(&mut events, &slow.started);
    let run_id = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunChanged { run, .. } => Some(*run),
            _ => None,
        })
        .expect("the run is shown");

    let cancelled_at = Instant::now();
    let answer = harness.send(CommandKind::CancelWorkflowRun { run: run_id });
    assert_eq!(failure(&answer), None, "{answer:?}");
    let events = harness.pump_until(events, finished);
    let took = cancelled_at.elapsed();

    assert_eq!(end_of(&events), WorkflowEnd::Cancelled, "{events:?}");
    // The fetch sleeps two seconds before Git answers; the cancel did not wait for it.
    assert!(took < Duration::from_secs(1), "the cancel took {took:?}");
    assert_eq!(
        rev(&slow.repo, "origin/main"),
        before,
        "nothing was fetched"
    );
    // Git would have fetched once its two seconds passed: the stop must have killed it.
    let grace = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        rev(&slow.repo, "origin/main"),
        before,
        "the stopped fetch never completed"
    );
    let logged = harness.engine.log().iter().any(|entry| {
        entry.id == OperationId(op)
            && matches!(entry.kind, CommandKind::Fetch)
            && entry.state == OperationState::Failed
    });
    assert!(
        logged,
        "the stopped fetch is in the command log under the run (D81)"
    );
}
