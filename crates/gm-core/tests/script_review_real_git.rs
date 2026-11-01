//! Review and automatic approval of event runs (design/workflow-scripts.md, `WF3b`, D101, D155(3),
//! D157(2)). A version the person has not reviewed does not run on an event: the run waits for the
//! review behind a request, and the review starts it. The grant of the review lets event runs apply
//! without asking; an edit or a revoke takes both back. Real repositories in temporary directories;
//! this harness plays the engine thread that carries a run's effects and events, as the runner does.

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
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowEnd, WorkflowInfo,
    WorkflowRunStatus,
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
    /// The events of opening the repository.
    opened: Vec<Event>,
}

impl Harness {
    /// Opens `repo` with the workflows of `flows`, with the triggers on as the app turns them on.
    fn new(repo: &Path, flows: &Path) -> Self {
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
        engine.set_triggers(true);
        let mut harness = Self {
            engine,
            messages,
            repo: repo.to_path_buf(),
            last: 0,
            opened: Vec::new(),
        };
        harness.opened = harness.open();
        harness
    }

    fn open(&mut self) -> Vec<Event> {
        self.send(CommandKind::OpenRepo {
            path: self.repo.display().to_string(),
        })
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

    /// The workflow `name` as the listing shows it.
    fn info(&mut self, name: &str) -> WorkflowInfo {
        let events = self.send(CommandKind::ListWorkflows);
        listed(&events)
            .into_iter()
            .find(|w| w.name == name)
            .unwrap_or_else(|| panic!("no workflow named {name}"))
    }

    /// Reviews the version of `name` the listing shows, with its grant. Returns the answer.
    fn review(&mut self, name: &str, auto_approve: bool) -> Vec<Event> {
        let hash = self.info(name).hash;
        let events = self.send(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
        events
    }

    /// Writes `file`, stages only that file and commits it, as the person does in the window.
    fn commit_file(&mut self, file: &str, text: &str, message: &str) -> (Vec<Event>, String) {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
        self.commit(message)
    }

    fn commit(&mut self, message: &str) -> (Vec<Event>, String) {
        let events = self.send(CommandKind::Commit {
            message: message.to_owned(),
        });
        let oid = events
            .iter()
            .find_map(|e| match e {
                Event::Committed { oid, .. } => Some(oid.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no commit: {events:#?}"));
        (events, oid)
    }

    /// Commits, then plays the engine until the commit's own events are in, and returns the
    /// events of the commit and of its runs.
    fn commit_and_settle(&mut self, file: &str, text: &str, message: &str) -> (Vec<Event>, String) {
        let (events, oid) = self.commit_file(file, text, message);
        let mut all = events;
        all.extend(self.settle(Duration::from_millis(300)));
        (all, oid)
    }
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

fn notices(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The workflows that wait for a review, with the trigger that asked.
fn wants_review(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowWantsReview {
                workflow, trigger, ..
            } => Some((workflow.clone(), trigger.clone())),
            _ => None,
        })
        .collect()
}

/// The pending requests that ended, by workflow.
fn dropped(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRequestDropped { workflow } => Some(workflow.clone()),
            _ => None,
        })
        .collect()
}

fn started_runs(events: &[Event]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRunChanged {
                run,
                status: WorkflowRunStatus::Running,
                ..
            } => Some(*run),
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

fn ends(events: &[Event]) -> Vec<(i64, WorkflowEnd)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRunFinished { run, end, .. } => Some((*run, *end)),
            _ => None,
        })
        .collect()
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::WorkflowRunFinished { .. }))
}

fn committed(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::Committed { .. }))
}

const ON_COMMIT: &str = r#"
local M = {}
M.meta = { name = "on-commit", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.notify("ran on " .. input.trigger.oid)
end
return M
"#;

/// Applies a change on its event: it waits at the apply call unless the version is granted.
const STAGER: &str = r#"
local M = {}
M.meta = { name = "stager", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.notify("start " .. input.trigger.oid)
  ctx.stage.all()
  ctx.notify("end " .. input.trigger.oid)
end
return M
"#;

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

#[test]
fn an_unreviewed_commit_asks_for_the_review_and_does_not_run() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    let (events, _) = h.commit_and_settle("b.txt", "b\n", "feat: b");
    assert_eq!(
        wants_review(&events),
        vec![("on-commit".to_owned(), "commit.created".to_owned())]
    );
    assert!(notices(&events).is_empty(), "{events:#?}");
    assert!(started_runs(&events).is_empty(), "{events:#?}");
}

#[test]
fn reviewing_the_waiting_version_starts_its_run_for_the_commit_it_waits_for() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    let (_, oid) = h.commit_and_settle("b.txt", "b\n", "feat: b");
    let events = h.review("on-commit", false);
    assert_eq!(dropped(&events), vec!["on-commit".to_owned()]);
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec![format!("ran on {oid}")]);
    let runs = started_runs(&events);
    assert_eq!(runs.len(), 1, "{events:#?}");
    let end = ends(&events);
    assert_eq!(end.len(), 1);
    assert_eq!(end[0].1, WorkflowEnd::Done);
}

#[test]
fn two_commits_before_the_review_keep_one_request_for_the_latest_commit() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    let (_, _first) = h.commit_and_settle("b.txt", "b\n", "feat: b");
    let (_, second) = h.commit_and_settle("c.txt", "c\n", "feat: c");
    let events = h.review("on-commit", false);
    let events = h.pump_until(events, finished);
    let settled = h.settle(Duration::from_millis(300));
    let all: Vec<Event> = events.into_iter().chain(settled).collect();
    assert_eq!(notices(&all), vec![format!("ran on {second}")]);
    assert_eq!(started_runs(&all).len(), 1, "{all:#?}");
}

#[test]
fn dismissing_the_request_drops_it_and_a_later_review_starts_nothing() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    h.commit_and_settle("b.txt", "b\n", "feat: b");
    let events = h.send(CommandKind::DismissWorkflowRequest {
        workflow: "on-commit".into(),
    });
    assert_eq!(dropped(&events), vec!["on-commit".to_owned()]);
    let events = h.review("on-commit", false);
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    let later = h.settle(Duration::from_millis(300));
    assert!(started_runs(&later).is_empty(), "{later:#?}");
    assert!(notices(&later).is_empty(), "{later:#?}");
}

#[test]
fn with_the_grant_an_event_run_applies_without_asking() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "stager.lua", STAGER);
    let mut h = Harness::new(&repo, &flows);
    h.review("stager", true);
    assert!(h.info("stager").auto_approve);
    let (events, oid) = h.commit_file("b.txt", "b\n", "feat: b");
    let events = h.pump_until(events, finished);
    assert!(approvals(&events).is_empty(), "{events:#?}");
    assert_eq!(
        notices(&events),
        vec![format!("start {oid}"), format!("end {oid}")]
    );
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done);
}

#[test]
fn without_the_grant_an_event_run_pauses_for_approval_until_the_person_approves() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "stager.lua", STAGER);
    let mut h = Harness::new(&repo, &flows);
    h.review("stager", false);
    assert!(!h.info("stager").auto_approve);
    let (events, _) = h.commit_file("b.txt", "b\n", "feat: b");
    let events = h.pump_until(events, |events| !approvals(events).is_empty());
    let run = approvals(&events)[0];
    assert!(ends(&events).is_empty(), "{events:#?}");
    let answer = h.send(CommandKind::ApproveWorkflowStep {
        run,
        approved: true,
    });
    assert!(failure(&answer).is_none(), "{answer:#?}");
    let events = h.pump_until(answer, finished);
    assert_eq!(ends(&events), vec![(run, WorkflowEnd::Done)]);
}

#[test]
fn an_edit_after_the_review_asks_again_and_takes_the_grant_back() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-commit", true);
    let (events, _) = h.commit_and_settle("b.txt", "b\n", "feat: b");
    assert!(started_runs(&events).len() == 1, "{events:#?}");
    write_script(
        &flows,
        "on-commit.lua",
        &format!("{ON_COMMIT}\n-- edited\n"),
    );
    let info = h.info("on-commit");
    assert!(!info.reviewed, "{info:#?}");
    assert!(!info.auto_approve, "{info:#?}");
    let (events, _) = h.commit_and_settle("c.txt", "c\n", "feat: c");
    assert_eq!(
        wants_review(&events),
        vec![("on-commit".to_owned(), "commit.created".to_owned())]
    );
    assert!(started_runs(&events).is_empty(), "{events:#?}");
}

#[test]
fn an_edit_drops_the_request_of_the_version_it_replaces() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    h.commit_and_settle("b.txt", "b\n", "feat: b");
    write_script(
        &flows,
        "on-commit.lua",
        &format!("{ON_COMMIT}\n-- edited\n"),
    );
    let events = h.send(CommandKind::ListWorkflows);
    assert_eq!(
        dropped(&events),
        vec!["on-commit".to_owned()],
        "{events:#?}"
    );
    let events = h.review("on-commit", false);
    assert!(started_runs(&events).is_empty(), "{events:#?}");
}

#[test]
fn revoking_the_review_takes_the_review_and_the_grant_back() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-commit", true);
    let hash = h.info("on-commit").hash;
    let events = h.send(CommandKind::RevokeWorkflowReview { hash });
    let info = listed(&events)
        .into_iter()
        .find(|w| w.name == "on-commit")
        .unwrap();
    assert!(!info.reviewed && !info.auto_approve, "{info:#?}");
    let (events, _) = h.commit_and_settle("b.txt", "b\n", "feat: b");
    assert_eq!(
        wants_review(&events),
        vec![("on-commit".to_owned(), "commit.created".to_owned())]
    );
    assert!(started_runs(&events).is_empty(), "{events:#?}");
}

#[test]
fn an_unreviewed_observer_of_a_commit_waits_and_runs_when_reviewed() {
    let (_dir, repo, flows) = setup();
    write_script(
        &flows,
        "after-commit.lua",
        r#"
local M = {}
M.meta = { name = "after-commit", trigger = { on = "after.commit" } }
function M.execute(ctx, input)
  ctx.notify("observed " .. input.trigger.oid)
end
return M
"#,
    );
    let mut h = Harness::new(&repo, &flows);
    let (events, _) = h.commit_file("b.txt", "b\n", "feat: b");
    let events = h.pump_until(events, committed);
    let settled = h.settle(Duration::from_millis(200));
    let all: Vec<Event> = events.into_iter().chain(settled).collect();
    assert_eq!(
        wants_review(&all),
        vec![("after-commit".to_owned(), "after.commit".to_owned())]
    );
    assert!(started_runs(&all).is_empty(), "{all:#?}");
    let oid = all
        .iter()
        .find_map(|e| match e {
            Event::Committed { oid, .. } => Some(oid.clone()),
            _ => None,
        })
        .unwrap();
    let events = h.review("after-commit", false);
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec![format!("observed {oid}")]);
}

const GATE: &str = r#"
local M = {}
M.meta = { name = "freeze", trigger = { on = "before.commit" } }
function M.execute(ctx, input)
  ctx.notify("checked")
end
return M
"#;

#[test]
fn an_unreviewed_gate_still_vetoes_and_the_same_commit_goes_through_after_the_review() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "freeze.lua", GATE);
    let mut h = Harness::new(&repo, &flows);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&repo, &["add", "b.txt"]);
    let (id, events) = {
        let id = OperationId(h.last + 1);
        (
            id,
            h.send(CommandKind::Commit {
                message: "feat: b".into(),
            }),
        )
    };
    let vetoed = events.iter().find_map(|e| match e {
        Event::OperationChanged {
            id: op,
            state: OperationState::Vetoed,
            detail,
            ..
        } if *op == id => detail.clone(),
        _ => None,
    });
    assert_eq!(
        vetoed.as_deref(),
        Some("freeze is not reviewed: review it, then try again.")
    );
    let gate = events.iter().find_map(|e| match e {
        Event::WorkflowGateUnreviewed {
            id: op,
            workflow,
            hash,
        } if *op == id => Some((workflow.clone(), hash.clone())),
        _ => None,
    });
    let info = h.info("freeze");
    assert_eq!(gate, Some(("freeze".to_owned(), info.hash.clone())));
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert!(
        wants_review(&events).is_empty(),
        "a gate waits for no request"
    );

    h.review("freeze", false);
    // The gate runs in its own run now, so the commit lands after it: the answer is not final.
    let events = h.send(CommandKind::Commit {
        message: "feat: b".into(),
    });
    let events = h.pump_until(events, committed);
    assert!(committed(&events), "{events:#?}");
    assert_eq!(notices(&events), vec!["checked".to_owned()]);
}
