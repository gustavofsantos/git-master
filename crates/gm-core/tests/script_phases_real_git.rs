//! Phases of the operations (design/workflow-scripts.md, `WF3c`, D157): a `before.<op>` gate runs
//! before a commit, a restack, a branch creation or a conflict resolution and can veto it, and an
//! `after.<op>` observer runs after it succeeded. Real repositories in temporary directories; this
//! harness plays the engine thread that carries a run's effects and events, as the runner does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, RestackPlan, WorkflowEnd,
};

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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// A commit on `branch` with `file`, leaving `main` checked out.
fn commit_on(repo: &Path, branch: &str, file: &str, content: &str) {
    git(repo, &["checkout", "-q", branch]);
    std::fs::write(repo.join(file), content).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", &format!("{file}: {content}")]);
    git(repo, &["checkout", "-q", "main"]);
}

fn refs(repo: &Path) -> String {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    )
}

/// A script that runs `body` for the phase `on`, named `name`.
fn phase(name: &str, on: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\" }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn write_script(flows: &Path, file: &str, source: &str) {
    std::fs::create_dir_all(flows).unwrap();
    std::fs::write(flows.join(file), source).unwrap();
}

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
}

struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    repo: PathBuf,
    last: u64,
}

impl Harness {
    /// Opens `repo` with the workflows of `flows`. `triggers` is the switch the app turns on.
    fn new(repo: &Path, flows: &Path, triggers: bool) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
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
        engine.set_triggers(triggers);
        let mut harness = Self {
            engine,
            messages,
            repo: repo.to_path_buf(),
            last: 0,
        };
        let opened = harness.handle(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        assert_eq!(
            final_state(&opened.1, opened.0).map(|(state, _)| state),
            Some(OperationState::Done),
            "open: {:?}",
            opened.1
        );
        harness
    }

    /// Runs a command as the person does, and returns its id with the events so far.
    fn handle(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let events = self.engine.handle(
            id,
            &Command {
                actor: Actor::Human,
                kind,
            },
        );
        (id, events)
    }

    /// Runs a command and returns its events only.
    fn send(&mut self, kind: CommandKind) -> Vec<Event> {
        self.handle(kind).1
    }

    /// Plays the engine thread until the operation `id` ends, or panics.
    fn finish(&mut self, id: OperationId, events: Vec<Event>) -> Vec<Event> {
        self.pump_until(events, move |events| {
            final_state(events, id).is_some_and(|(state, _)| state.is_terminal())
        })
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
            events.extend(self.step(Duration::from_millis(20)));
        }
        events
    }

    /// Plays the engine thread for `span` and returns what came in.
    fn settle(&mut self, span: Duration) -> Vec<Event> {
        let end = Instant::now() + span;
        let mut events = Vec::new();
        while Instant::now() < end {
            events.extend(self.step(Duration::from_millis(20)));
        }
        events
    }

    fn step(&mut self, wait: Duration) -> Vec<Event> {
        match self.messages.recv_timeout(wait) {
            Ok(Message::Effect(request)) => self.engine.script_effect(request),
            Ok(Message::Event(event)) => self.engine.script_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        }
    }

    /// Reviews the script `name` as the listing shows it.
    fn review(&mut self, name: &str) {
        let events = self.send(CommandKind::ListWorkflows);
        let hash = events
            .iter()
            .find_map(|e| match e {
                Event::WorkflowsListed { workflows, .. } => workflows
                    .iter()
                    .find(|w| w.name == name)
                    .map(|w| w.hash.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no workflow named {name}"));
        let events = self.send(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
    }

    /// Writes `file` and stages it, as the person does in the window.
    fn stage(&mut self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
    }

    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }
}

/// The last terminal state of the operation `id`, with its detail.
fn final_state(events: &[Event], id: OperationId) -> Option<(OperationState, Option<String>)> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged {
            id: found,
            state,
            detail,
            ..
        } if *found == id && state.is_terminal() => Some((*state, detail.clone())),
        _ => None,
    })
}

fn committed(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::Committed { oid, .. } => Some(oid.clone()),
        _ => None,
    })
}

fn gate_names(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::GateStarted { workflow, .. } => Some(workflow.clone()),
            _ => None,
        })
        .collect()
}

fn notices(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn approvals(events: &[Event]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowApprovalRequested { run, .. } => Some(*run),
            _ => None,
        })
        .collect()
}

fn ends(events: &[Event]) -> Vec<WorkflowEnd> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRunFinished { end, .. } => Some(*end),
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

fn plan_of(events: &[Event]) -> RestackPlan {
    events
        .iter()
        .find_map(|e| match e {
            Event::RestackPlanned { plan, .. } => Some(plan.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no plan: {events:#?}"))
}

/// A repository with `a.txt` staged and a change to commit.
fn staged_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

/// The commit's message on `HEAD`.
fn subject(repo: &Path) -> String {
    git(repo, &["log", "-1", "--format=%s"])
}

#[test]
fn a_blocking_gate_stops_the_commit_with_its_reason() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "freeze.lua",
        &phase("freeze", "before.commit", "ctx.block(\"no\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("freeze");
    let before = h.head();
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Vetoed,
            Some("freeze blocked the commit: no".into())
        )),
        "{events:#?}"
    );
    assert_eq!(committed(&events), None);
    assert_eq!(h.head(), before, "no commit was made");
}

#[test]
fn a_gate_that_raises_an_error_stops_the_commit() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "lint.lua",
        &phase("lint", "before.commit", "error(\"boom\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("lint");
    let before = h.head();
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    let (state, detail) = final_state(&events, id).expect("the operation ended");
    assert_eq!(state, OperationState::Vetoed, "{events:#?}");
    let detail = detail.unwrap_or_default();
    assert!(detail.starts_with("lint failed: "), "{detail}");
    assert!(detail.contains("boom"), "{detail}");
    assert_eq!(h.head(), before);
}

#[test]
fn two_gates_run_in_file_name_order_and_the_first_block_stops_the_rest() {
    let (_dir, repo, flows) = staged_repo();
    // Written out of order: the order the gates run in is the file name's, not the write order.
    write_script(
        &flows,
        "c-third.lua",
        &phase("c-third", "before.commit", "ctx.block(\"third\")"),
    );
    write_script(
        &flows,
        "b-second.lua",
        &phase("b-second", "before.commit", "ctx.block(\"second\")"),
    );
    write_script(
        &flows,
        "a-first.lua",
        &phase("a-first", "before.commit", "ctx.notify(\"first\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    for name in ["a-first", "b-second", "c-third"] {
        h.review(name);
    }
    let before = h.head();
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    assert_eq!(gate_names(&events), ["a-first", "b-second"], "{events:#?}");
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Vetoed,
            Some("b-second blocked the commit: second".into())
        ))
    );
    assert_eq!(h.head(), before);
}

#[test]
fn a_passing_gate_lets_the_commit_happen_with_its_normal_events() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "ok.lua",
        &phase(
            "ok",
            "before.commit",
            "ctx.notify(\"checked \" .. input.trigger.message)",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("ok");
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    assert_eq!(gate_names(&events), ["ok"]);
    let oid = committed(&events).expect("the commit was made");
    assert_eq!(oid, h.head());
    assert_eq!(subject(&repo), "fix");
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
    assert_eq!(notices(&events), ["checked fix"]);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn an_unreviewed_gate_stops_the_commit_with_the_review_text() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "freeze.lua",
        &phase("freeze", "before.commit", "ctx.block(\"no\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    let before = h.head();
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Vetoed,
            Some("freeze is not reviewed: review it, then try again.".into())
        )),
        "{events:#?}"
    );
    assert!(gate_names(&events).is_empty(), "the gate did not run");
    assert_eq!(h.head(), before);
}

#[test]
fn before_restack_sees_the_root_and_blocks_a_real_restack_with_refs_unchanged() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "restack-freeze.lua",
        &phase(
            "restack-freeze",
            "before.restack",
            "ctx.block(\"frozen \" .. input.trigger.root .. \" \" .. input.trigger.branches[1])",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("restack-freeze");
    // Branch a, based on main, and then main moves on: a is stale and the restack moves it.
    h.send(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    commit_on(&repo, "a", "a.1", "a1");
    commit_on(&repo, "main", "m.1", "m1");
    let plan = plan_of(&h.send(CommandKind::PlanRestack {
        branch: "main".into(),
        skip: Vec::new(),
    }));
    assert!(plan.can_apply(), "{plan:?}");
    let before = refs(&repo);
    let (id, events) = h.handle(CommandKind::ApplyRestack {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Vetoed,
            Some("restack-freeze blocked the restack: frozen main a".into())
        )),
        "{events:#?}"
    );
    assert_eq!(refs(&repo), before, "no ref moved");
}

#[test]
fn an_observer_of_a_restack_sees_the_branches_it_moved() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "after-restack.lua",
        &phase(
            "after-restack",
            "after.restack",
            "ctx.notify(\"moved \" .. input.trigger.root .. \" \" .. input.trigger.moved[1])",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("after-restack");
    h.send(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    commit_on(&repo, "a", "a.1", "a1");
    commit_on(&repo, "main", "m.1", "m1");
    let plan = plan_of(&h.send(CommandKind::PlanRestack {
        branch: "main".into(),
        skip: Vec::new(),
    }));
    let (id, events) = h.handle(CommandKind::ApplyRestack {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done),
        "{events:#?}"
    );
    h.pump_until(events, |events| {
        notices(events).contains(&"moved main a".to_owned())
    });
}

#[test]
fn a_gate_on_branch_create_blocks_the_new_branch() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "branch-freeze.lua",
        &phase(
            "branch-freeze",
            "before.branch_create",
            "ctx.block(\"no branches\")",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("branch-freeze");
    let (id, events) = h.handle(CommandKind::CreateBranch {
        name: "feat".into(),
        start_point: None,
        checkout: false,
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id),
        Some((
            OperationState::Vetoed,
            Some("branch-freeze blocked the branch: no branches".into())
        ))
    );
    assert_eq!(git(&repo, &["branch", "--list", "feat"]), "");
}

#[test]
fn an_observer_runs_after_the_commit_and_sees_its_oid() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "after-commit.lua",
        &phase(
            "after-commit",
            "after.commit",
            "ctx.notify(\"after \" .. input.trigger.oid)",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("after-commit");
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    let oid = committed(&events).expect("the commit was made");
    let events = h.pump_until(events, |events| {
        notices(events).contains(&format!("after {oid}"))
    });
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
    assert!(gate_names(&events).is_empty());
}

#[test]
fn an_observer_of_a_branch_creation_sees_the_new_tip() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "after-branch.lua",
        &phase(
            "after-branch",
            "after.branch_create",
            "ctx.notify(\"made \" .. input.trigger.name .. \" \" .. input.trigger.oid)",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("after-branch");
    let (id, events) = h.handle(CommandKind::CreateBranch {
        name: "feat".into(),
        start_point: None,
        checkout: false,
    });
    let events = h.finish(id, events);
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done)
    );
    let head = h.head();
    h.pump_until(events, |events| {
        notices(events).contains(&format!("made feat {head}"))
    });
}

#[test]
fn a_failing_observer_leaves_the_commit_done_and_warns() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "after-commit.lua",
        &phase("after-commit", "after.commit", "error(\"boom\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("after-commit");
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.finish(id, events);
    assert!(committed(&events).is_some(), "the commit was made");
    let events = h.pump_until(events, |events| {
        notices(events)
            .iter()
            .any(|text| text.starts_with("after-commit failed after the commit: "))
    });
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done),
        "the operation keeps its outcome"
    );
    assert_eq!(subject(&repo), "fix");
}

#[test]
fn cancelling_while_a_gate_waits_ends_the_operation_and_commits_nothing() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "hold.lua",
        &phase(
            "hold",
            "before.commit",
            "ctx.approve(\"hold the commit\")\nctx.notify(\"released\")",
        ),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold");
    let before = h.head();
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    let events = h.pump_until(events, |events| !approvals(events).is_empty());
    let (_, cancel_events) = h.handle(CommandKind::CancelOperation { op: id });
    assert_eq!(
        final_state(&cancel_events, id).map(|(state, _)| state),
        Some(OperationState::Cancelled),
        "{cancel_events:#?}"
    );
    let late = h.settle(Duration::from_millis(300));
    assert_eq!(committed(&events), None);
    assert_eq!(
        committed(&late),
        None,
        "the gate's late end commits nothing"
    );
    assert_eq!(h.head(), before);
    assert!(
        notices(&late).is_empty(),
        "the cancelled gate did not go on"
    );
}

#[test]
fn a_second_gated_command_is_refused_while_one_waits() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "hold.lua",
        &phase("hold", "before.commit", "ctx.approve(\"hold the commit\")"),
    );
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold");
    h.stage("a.txt", "two\n");
    let (first, events) = h.handle(CommandKind::Commit {
        message: "first".into(),
    });
    h.pump_until(events, |events| !approvals(events).is_empty());
    let (second, refused) = h.handle(CommandKind::Commit {
        message: "second".into(),
    });
    let (state, detail) = final_state(&refused, second).expect("the second command ended");
    assert_eq!(state, OperationState::Failed);
    assert!(
        detail
            .unwrap_or_default()
            .contains("waiting for its workflow gates"),
        "{refused:#?}"
    );
    let (_, cancel_events) = h.handle(CommandKind::CancelOperation { op: first });
    assert_eq!(
        final_state(&cancel_events, first).map(|(state, _)| state),
        Some(OperationState::Cancelled)
    );
}

#[test]
fn a_commit_made_by_a_run_is_not_gated_and_runs_no_observer() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "freeze.lua",
        &phase("freeze", "before.commit", "ctx.block(\"no\")"),
    );
    write_script(
        &flows,
        "after-commit.lua",
        &phase("after-commit", "after.commit", "ctx.notify(\"observed\")"),
    );
    write_script(
        &flows,
        "make-commit.lua",
        "local M = {}\nM.meta = { name = \"make-commit\", trigger = { on = \"manual\" } }\n\
         function M.execute(ctx, input)\n  ctx.stage.all()\n  ctx.commit.create({ message = \"from run\" })\nend\nreturn M\n",
    );
    let mut h = Harness::new(&repo, &flows, true);
    for name in ["freeze", "after-commit", "make-commit"] {
        h.review(name);
    }
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    let (_, events) = h.handle(CommandKind::RunWorkflow {
        name: "make-commit".into(),
        inputs: BTreeMap::new(),
    });
    let events = h.pump_until(events, |events| !ends(events).is_empty());
    let late = h.settle(Duration::from_millis(300));
    assert_eq!(ends(&events), [WorkflowEnd::Done], "{events:#?}");
    assert_eq!(subject(&repo), "from run", "the run's commit was made");
    assert!(gate_names(&events).is_empty() && gate_names(&late).is_empty());
    assert!(
        notices(&events)
            .iter()
            .chain(notices(&late).iter())
            .all(|text| text != "observed"),
        "an observer ran for the run's commit"
    );
}

#[test]
fn with_the_switch_off_a_gate_does_not_run_and_the_commit_happens() {
    let (_dir, repo, flows) = staged_repo();
    write_script(
        &flows,
        "freeze.lua",
        &phase("freeze", "before.commit", "ctx.block(\"no\")"),
    );
    write_script(
        &flows,
        "after-commit.lua",
        &phase("after-commit", "after.commit", "ctx.notify(\"observed\")"),
    );
    let mut h = Harness::new(&repo, &flows, false);
    h.stage("a.txt", "two\n");
    let (id, events) = h.handle(CommandKind::Commit {
        message: "fix".into(),
    });
    assert!(gate_names(&events).is_empty());
    assert_eq!(
        final_state(&events, id).map(|(state, _)| state),
        Some(OperationState::Done),
        "{events:#?}"
    );
    assert_eq!(subject(&repo), "fix");
    let late = h.settle(Duration::from_millis(200));
    assert!(
        late.is_empty(),
        "no workflow runs with the switch off: {late:#?}"
    );
}
