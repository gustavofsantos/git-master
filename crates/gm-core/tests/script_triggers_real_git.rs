//! Event triggers of the workflows (design/workflow-scripts.md, `WF3a`, D155): a commit, a checkout,
//! a repository opening and a ref change start the scripts that name them, with `match`, the
//! concurrency rules and the no-cascade rule. Real repositories in temporary directories; this
//! harness plays the engine thread that carries a run's effects and events, as the runner does.
//! The watcher's signal is sent by hand: `Engine::refs_changed` is what it calls.

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
    Actor, Command, CommandKind, Event, OperationId, WorkflowEnd, WorkflowRunStatus,
    WorkflowRunSummary,
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

    /// The watcher saw refs move after its quiet period: the check the runner sends the engine.
    fn refs_seen(&mut self) -> Vec<Event> {
        self.last += 1;
        self.engine.refs_changed(OperationId(self.last))
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
    fn review(&mut self, name: &str, auto_approve: bool) {
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
            auto_approve,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
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

/// The workflows that wait for a review, with the trigger that asked (`WF3b`).
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

fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: gm_schema::OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

fn runs_listed(events: &[Event]) -> Vec<WorkflowRunSummary> {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunsListed { runs, .. } => Some(runs.clone()),
            _ => None,
        })
        .expect("a list of runs")
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::WorkflowRunFinished { .. }))
}

const ON_COMMIT: &str = r#"
local M = {}
M.meta = { name = "on-commit", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.notify("commit " .. input.trigger.oid .. " on " .. input.trigger.branch)
end
return M
"#;

const ON_REFS_FEAT: &str = r#"
local M = {}
M.meta = { name = "on-refs-feat", trigger = { on = "refs.changed", match = "branch:feat/*" } }
function M.execute(ctx, input)
  ctx.notify("moved " .. input.trigger.branch)
end
return M
"#;

const ON_REFS_ALL: &str = r#"
local M = {}
M.meta = { name = "on-refs-all", trigger = { on = "refs.changed" } }
function M.execute(ctx, input)
  local old = input.trigger.old or "none"
  local new = input.trigger.new or "none"
  ctx.notify(input.trigger.branch .. " " .. old .. " -> " .. new)
end
return M
"#;

const ON_CHECKOUT: &str = r#"
local M = {}
M.meta = { name = "on-checkout", trigger = { on = "checkout.done" } }
function M.execute(ctx, input)
  ctx.notify("checkout " .. input.trigger.branch)
end
return M
"#;

const ON_OPEN: &str = r#"
local M = {}
M.meta = { name = "on-open", trigger = { on = "repo.opened" } }
function M.execute(ctx, input)
  ctx.notify("opened " .. input.trigger.root)
end
return M
"#;

/// Commits from the run itself: it stages the change the person left and commits it, then
/// reports. A cascade would run `on-commit` again.
const FOLLOW_UP: &str = r#"
local M = {}
M.meta = { name = "follow-up", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.notify("ran on " .. input.trigger.oid)
  ctx.stage.all()
  ctx.commit.create({ message = "wf: follow-up" })
end
return M
"#;

/// Holds its run at an approval, so the test can start more triggers while it is active.
fn hold(concurrency: &str) -> String {
    format!(
        r#"
local M = {{}}
M.meta = {{ name = "hold", trigger = {{ on = "commit.created" }}, concurrency = "{concurrency}" }}
function M.execute(ctx, input)
  ctx.notify("start " .. input.trigger.oid)
  ctx.approve("hold " .. input.trigger.oid)
  ctx.notify("end " .. input.trigger.oid)
end
return M
"#
    )
}

/// Applies a change while it runs: an event run waits at this call until the person approves it.
const STAGER: &str = r#"
local M = {}
M.meta = { name = "stager", trigger = { on = "commit.created" }, concurrency = "replace" }
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
fn a_commit_starts_a_commit_created_workflow_that_sees_the_oid_and_branch() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("on-commit", false);
    let (events, oid) = h.commit_file("b.txt", "b\n", "feat: b");
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec![format!("commit {oid} on main")]);
    let runs = runs_listed(&h.send(CommandKind::ListWorkflowRuns {
        workflow: "on-commit".into(),
        limit: 10,
    }));
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].started_by, "event:commit.created");
    assert_eq!(runs[0].status, WorkflowRunStatus::Done);
}

#[test]
fn a_script_that_is_not_reviewed_does_not_start_and_a_notice_says_why() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows, true);
    let (events, _) = h.commit_file("b.txt", "b\n", "feat: b");
    assert_eq!(
        wants_review(&events),
        vec![("on-commit".to_owned(), "commit.created".to_owned())]
    );
    assert!(notices(&events).is_empty(), "{events:#?}");
    let later = h.settle(Duration::from_millis(200));
    assert!(started_runs(&events).is_empty());
    assert!(started_runs(&later).is_empty());
}

#[test]
fn with_the_switch_off_no_workflow_runs_on_an_event() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows, false);
    h.review("on-commit", false);
    let (events, _) = h.commit_file("b.txt", "b\n", "feat: b");
    let later = h.settle(Duration::from_millis(300));
    assert!(notices(&events).is_empty());
    assert!(started_runs(&events).is_empty());
    assert!(started_runs(&later).is_empty());
    assert!(notices(&later).is_empty());
}

#[test]
fn a_match_runs_for_the_branch_it_names_and_not_for_the_others() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-refs-feat.lua", ON_REFS_FEAT);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("on-refs-feat", false);

    // A new commit on main is a change that matches nothing.
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "on main"]);
    let events = h.refs_seen();
    let quiet = [events, h.settle(Duration::from_millis(300))].concat();
    assert!(started_runs(&quiet).is_empty(), "main matches nothing");
    assert!(notices(&quiet).is_empty());

    // A feat branch matches, and runs once.
    git(&repo, &["branch", "feat/x"]);
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["moved feat/x".to_owned()]);
}

#[test]
fn each_changed_branch_fires_once_with_its_old_and_new_tip() {
    let (_dir, repo, flows) = setup();
    git(&repo, &["branch", "keep"]);
    git(&repo, &["branch", "moved"]);
    let init = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let two = git(&repo, &["rev-parse", "HEAD"]);
    write_script(&flows, "on-refs-all.lua", ON_REFS_ALL);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("on-refs-all", false);

    let mut seen = Vec::new();
    for change in [
        vec!["branch", "-f", "moved", two.as_str()],
        vec!["branch", "created", two.as_str()],
        vec!["branch", "-D", "keep"],
    ] {
        git(&repo, &change);
        let events = h.refs_seen();
        let events = h.pump_until(events, finished);
        seen.extend(notices(&events));
    }
    assert_eq!(
        seen,
        vec![
            format!("moved {init} -> {two}"),
            format!("created none -> {two}"),
            format!("keep {init} -> none"),
        ]
    );
}

#[test]
fn a_run_triggers_nothing_not_even_the_ref_change_it_makes() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "follow-up.lua", FOLLOW_UP);
    write_script(&flows, "on-refs-all.lua", ON_REFS_ALL);
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("follow-up", true);
    h.review("on-refs-all", false);
    h.review("on-commit", false);

    // The person commits `b.txt`. `a.txt` stays changed and unstaged, so the run has work to commit.
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    let (events, oid) = h.commit_file("b.txt", "b\n", "feat: b");
    let mut log = h.pump_until(events, |e| ends(e).len() == 2);
    // The watcher reports the branch the run moved, after its quiet period.
    log.extend(h.refs_seen());
    log.extend(h.settle(Duration::from_millis(300)));

    let all = notices(&log);
    assert_eq!(
        all.iter().filter(|n| n.starts_with("commit ")).count(),
        1,
        "on-commit ran once, for the person's commit: {all:#?}"
    );
    assert!(all.contains(&format!("commit {oid} on main")));
    assert_eq!(
        all.iter().filter(|n| n.starts_with("ran on")).count(),
        1,
        "the run's own commit started no second follow-up: {all:#?}"
    );
    assert!(
        all.iter().all(|n| !n.contains("->")),
        "no ref change fired for the run's commit: {all:#?}"
    );
    assert_eq!(started_runs(&log).len(), 2, "two runs only: {log:#?}");
    let subjects = git(&repo, &["log", "--format=%s"]);
    assert!(
        subjects.contains("wf: follow-up"),
        "the run committed: {subjects}"
    );
}

#[test]
fn a_checkout_starts_a_checkout_done_workflow() {
    let (_dir, repo, flows) = setup();
    git(&repo, &["branch", "other"]);
    write_script(&flows, "on-checkout.lua", ON_CHECKOUT);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("on-checkout", false);
    let events = h.send(CommandKind::Checkout {
        branch: "other".into(),
    });
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["checkout other".to_owned()]);
}

#[test]
fn opening_a_repository_starts_a_repo_opened_workflow() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "on-open.lua", ON_OPEN);
    let mut h = Harness::new(&repo, &flows, true);
    assert_eq!(
        wants_review(&h.opened),
        vec![("on-open".to_owned(), "repo.opened".to_owned())]
    );
    h.review("on-open", false);
    let events = h.open();
    let events = h.pump_until(events, finished);
    let root = std::fs::canonicalize(&repo).unwrap();
    assert_eq!(notices(&events), vec![format!("opened {}", root.display())]);
}

#[test]
fn queue_keeps_one_pending_run_and_the_last_trigger_wins() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "hold.lua", &hold("queue"));
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold", false);
    let mut log = Vec::new();

    let (events, one) = h.commit_file("1.txt", "1\n", "one");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 1);
    let first = approvals(&log)[0];

    let (events, _) = h.commit_file("2.txt", "2\n", "two");
    log.extend(events);
    let (events, three) = h.commit_file("3.txt", "3\n", "three");
    log.extend(events);
    log.extend(h.settle(Duration::from_millis(200)));
    assert_eq!(
        notices(&log),
        vec![format!("start {one}")],
        "the second and third triggers wait"
    );

    h.send(CommandKind::ApproveWorkflowStep {
        run: first,
        approved: true,
    });
    log = h.pump_until(log, |e| approvals(e).len() == 2);
    let second = approvals(&log)[1];
    assert_ne!(first, second);
    h.send(CommandKind::ApproveWorkflowStep {
        run: second,
        approved: true,
    });
    log = h.pump_until(log, |e| ends(e).len() == 2);

    assert_eq!(
        notices(&log),
        vec![
            format!("start {one}"),
            format!("end {one}"),
            format!("start {three}"),
            format!("end {three}"),
        ],
        "the pending run starts with the last payload, and no run starts for the second commit"
    );
    assert_eq!(
        ends(&log),
        vec![(first, WorkflowEnd::Done), (second, WorkflowEnd::Done)]
    );
}

#[test]
fn skip_drops_a_trigger_while_a_run_is_active() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "hold.lua", &hold("skip"));
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold", false);
    let mut log = Vec::new();

    let (events, one) = h.commit_file("1.txt", "1\n", "one");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 1);
    let first = approvals(&log)[0];

    let (events, _) = h.commit_file("2.txt", "2\n", "two");
    log.extend(events);
    h.send(CommandKind::ApproveWorkflowStep {
        run: first,
        approved: true,
    });
    log = h.pump_until(log, |e| ends(e).len() == 1);
    log.extend(h.settle(Duration::from_millis(300)));
    assert_eq!(
        notices(&log),
        vec![format!("start {one}"), format!("end {one}")],
        "the second trigger was dropped, and nothing ran after the first ended"
    );

    let (events, three) = h.commit_file("3.txt", "3\n", "three");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 2);
    let third = approvals(&log)[1];
    h.send(CommandKind::ApproveWorkflowStep {
        run: third,
        approved: true,
    });
    log = h.pump_until(log, |e| ends(e).len() == 2);
    assert_eq!(
        notices(&log),
        vec![
            format!("start {one}"),
            format!("end {one}"),
            format!("start {three}"),
            format!("end {three}"),
        ],
        "a trigger after the run ended starts again"
    );
}

#[test]
fn replace_cancels_the_active_run_when_it_has_not_applied() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "hold.lua", &hold("replace"));
    let mut h = Harness::new(&repo, &flows, true);
    h.review("hold", false);
    let mut log = Vec::new();

    let (events, one) = h.commit_file("1.txt", "1\n", "one");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 1);
    let first = approvals(&log)[0];

    let (events, two) = h.commit_file("2.txt", "2\n", "two");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 2);
    let second = approvals(&log)[1];
    assert!(
        ends(&log).contains(&(first, WorkflowEnd::Cancelled)),
        "the first run was cancelled: {log:#?}"
    );
    h.send(CommandKind::ApproveWorkflowStep {
        run: second,
        approved: true,
    });
    log = h.pump_until(log, |e| ends(e).len() == 2);

    assert_eq!(
        notices(&log),
        vec![
            format!("start {one}"),
            format!("start {two}"),
            format!("end {two}"),
        ],
        "the cancelled run never reached its end"
    );
    assert_eq!(
        ends(&log),
        vec![(first, WorkflowEnd::Cancelled), (second, WorkflowEnd::Done)]
    );
}

#[test]
fn replace_queues_behind_a_run_that_reached_an_applying_call() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "stager.lua", STAGER);
    let mut h = Harness::new(&repo, &flows, true);
    h.review("stager", false);
    let mut log = Vec::new();

    let (events, one) = h.commit_file("1.txt", "1\n", "one");
    log.extend(events);
    log = h.pump_until(log, |e| approvals(e).len() == 1);
    let first = approvals(&log)[0];

    // The first run waits at its `stage.all`, an applying call: it is not cancelled.
    let (events, two) = h.commit_file("2.txt", "2\n", "two");
    log.extend(events);
    log.extend(h.settle(Duration::from_millis(200)));
    assert!(!ends(&log).iter().any(|(run, _)| *run == first));

    h.send(CommandKind::ApproveWorkflowStep {
        run: first,
        approved: true,
    });
    log = h.pump_until(log, |e| approvals(e).len() == 2);
    let second = approvals(&log)[1];
    h.send(CommandKind::ApproveWorkflowStep {
        run: second,
        approved: true,
    });
    log = h.pump_until(log, |e| ends(e).len() == 2);

    assert_eq!(
        notices(&log),
        vec![
            format!("start {one}"),
            format!("end {one}"),
            format!("start {two}"),
            format!("end {two}"),
        ]
    );
    assert_eq!(
        ends(&log),
        vec![(first, WorkflowEnd::Done), (second, WorkflowEnd::Done)]
    );
}
