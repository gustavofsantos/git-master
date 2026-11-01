//! Waits and stack events (design/workflow-scripts.md, `WF3f`, D162): a `ctx.wait` on
//! `restack.resumed` ends when a restack conflict is finished or cancelled, and the stack events
//! `stack.parent_moved` and `stack.parent_merged` fire on the changes of the stacks, once each,
//! without a run's own changes. Real repositories in temporary directories; this harness plays
//! the engine thread that carries a run's effects and events, as the runner does. The watcher's
//! signal is sent by hand: `Engine::refs_changed` is what it calls.

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
    Actor, Command, CommandKind, ConflictSite, Event, OperationId, OperationState, RestackBlock,
    RestackPlan, WorkflowRunStatus, WorkflowRunSummary,
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

    /// The watcher saw refs move after its quiet period: the check the runner sends the engine.
    fn refs_seen(&mut self) -> Vec<Event> {
        self.last += 1;
        self.engine.refs_changed(OperationId(self.last))
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
    fn commit_file(&mut self, file: &str, text: &str, message: &str) -> Vec<Event> {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
        let events = self.send(CommandKind::Commit {
            message: message.to_owned(),
        });
        assert!(failure(&events).is_none(), "commit refused: {events:#?}");
        events
    }

    /// Starts the manual workflow `name`, and returns the events of the start.
    fn start(&mut self, name: &str) -> Vec<Event> {
        self.send(CommandKind::RunWorkflow {
            name: name.to_owned(),
            inputs: BTreeMap::new(),
        })
    }

    /// The runs of the workflow `name`, as the Runs tab lists them.
    fn runs(&mut self, name: &str) -> Vec<WorkflowRunSummary> {
        runs_listed(&self.send(CommandKind::ListWorkflowRuns {
            workflow: name.into(),
            limit: 10,
        }))
    }

    /// The restack plan of the branch `from`.
    fn plan(&mut self, from: &str) -> RestackPlan {
        let events = self.send(CommandKind::PlanRestack {
            branch: from.into(),
            skip: Vec::new(),
        });
        events
            .iter()
            .find_map(|e| match e {
                Event::RestackPlanned { plan, .. } => Some(plan.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no plan: {events:#?}"))
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

fn wants_review(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowWantsReview { workflow, .. } => Some(workflow.clone()),
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
            state: OperationState::Failed,
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

/// The id of the run that `events` started, which the test then waits for.
fn run_of(events: &[Event]) -> i64 {
    *started_runs(events)
        .first()
        .unwrap_or_else(|| panic!("no run started: {events:#?}"))
}

/// Waits for `restack.resumed` and says how it ended, with the payload's fields.
const WAIT_RESTACK: &str = r#"
local M = {}
M.meta = { name = "wait-restack", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local w = ctx.wait("restack.resumed", { timeout = "10m" })
  ctx.notify(w.outcome .. " " .. (w.branch or "-") .. " " .. (w.commit or "-"))
end
return M
"#;

/// Waits for a restack that never comes, for one second.
const WAIT_SHORT: &str = r#"
local M = {}
M.meta = { name = "wait-short", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local w = ctx.wait("restack.resumed", { timeout = "1s" })
  ctx.notify(w.outcome)
end
return M
"#;

const PARENT_MOVED: &str = r#"
local M = {}
M.meta = { name = "on-parent-moved", trigger = { on = "stack.parent_moved" } }
function M.execute(ctx, input)
  ctx.notify("moved " .. input.trigger.branch .. " " .. input.trigger.parent)
end
return M
"#;

const PARENT_MOVED_OTHER: &str = r#"
local M = {}
M.meta = { name = "on-other-moved", trigger = { on = "stack.parent_moved", match = "branch:other/*" } }
function M.execute(ctx, input)
  ctx.notify("other " .. input.trigger.branch)
end
return M
"#;

const PARENT_MERGED: &str = r#"
local M = {}
M.meta = { name = "on-parent-merged", trigger = { on = "stack.parent_merged" } }
function M.execute(ctx, input)
  ctx.notify("gone " .. input.trigger.branch .. " " .. input.trigger.parent)
end
return M
"#;

/// Commits a file the run stages itself: a commit made by a run on the parent.
const COMMIT_ELSEWHERE: &str = r#"
local M = {}
M.meta = { name = "commit-elsewhere", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.stage.all()
  ctx.commit.create({ message = "wf: elsewhere" })
  ctx.notify("committed")
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

// The restack conflict of D154, through the engine: `a` carries a commit on `a.txt`, and `main`
// then changes `a.txt` too, so replaying `a` onto `main` conflicts.

/// Makes the conflict and returns the commit of `a` that conflicts.
fn conflicting_chain(h: &mut Harness) -> String {
    h.send(CommandKind::CreateChildBranch {
        name: "a".into(),
        parent: "main".into(),
        checkout: false,
    });
    git(&h.repo, &["checkout", "-q", "a"]);
    std::fs::write(h.repo.join("a.txt"), "from a\n").unwrap();
    git(&h.repo, &["add", "a.txt"]);
    git(&h.repo, &["commit", "-q", "-m", "a.txt: from a"]);
    let commit = git(&h.repo, &["rev-parse", "HEAD"]);
    git(&h.repo, &["checkout", "-q", "main"]);
    std::fs::write(h.repo.join("a.txt"), "from main\n").unwrap();
    git(&h.repo, &["commit", "-q", "-am", "main moves"]);
    commit
}

/// Starts the resolution of `a` in the plan `plan_id`, resolves its one file and finishes it.
fn resolve_and_finish(h: &mut Harness, plan_id: &str) -> Vec<Event> {
    let events = h.send(CommandKind::StartRestackResolution {
        branch: "main".into(),
        plan_id: plan_id.into(),
        skip: Vec::new(),
        conflicted: "a".into(),
    });
    assert!(failure(&events).is_none(), "start refused: {events:#?}");
    let events = h.send(CommandKind::ResolveConflict {
        path: "a.txt".into(),
        text: "resolved\n".into(),
        origins: Vec::new(),
        site: ConflictSite::Restack,
    });
    assert!(failure(&events).is_none(), "resolve refused: {events:#?}");
    let events = h.send(CommandKind::FinishRestackResolution);
    assert!(failure(&events).is_none(), "finish refused: {events:#?}");
    events
}

/// The waits of the restack conflict: a run waits for `restack.resumed` and the conflict is
/// finished or cancelled.
fn waiting_on_a_conflict(name: &str, script: &str) -> (tempfile::TempDir, Harness, i64, String) {
    let (dir, repo, flows) = setup();
    write_script(&flows, &format!("{name}.lua"), script);
    let mut h = Harness::new(&repo, &flows);
    h.review(name, false);
    let conflict = conflicting_chain(&mut h);
    let mut events = h.start(name);
    events.extend(h.settle(Duration::from_millis(300)));
    assert!(!finished(&events), "the wait ended early: {events:#?}");
    let run = run_of(&events);
    assert_eq!(h.runs(name)[0].status, WorkflowRunStatus::Waiting);
    (dir, h, run, conflict)
}

#[test]
fn a_finished_restack_conflict_resumes_the_run_with_the_branch_and_commit() {
    let (_dir, mut h, run, conflict) = waiting_on_a_conflict("wait-restack", WAIT_RESTACK);
    let plan = h.plan("main");
    let planned = plan
        .branches
        .iter()
        .find(|entry| entry.name == "a")
        .expect("a is planned");
    assert!(matches!(planned.block, Some(RestackBlock::Conflict { .. })));
    let events = resolve_and_finish(&mut h, &plan.plan_id);
    let events = h.pump_until(events, finished);
    assert_eq!(
        notices(&events),
        vec![format!("resumed a {conflict}")],
        "the wait returns the outcome, the branch and the original commit"
    );
    assert_eq!(h.runs("wait-restack")[0].status, WorkflowRunStatus::Done);
    assert_eq!(h.runs("wait-restack")[0].run, run);
}

#[test]
fn a_cancelled_restack_conflict_ends_the_wait_with_aborted() {
    let (_dir, mut h, _run, conflict) = waiting_on_a_conflict("wait-restack", WAIT_RESTACK);
    let plan = h.plan("main");
    let started = h.send(CommandKind::StartRestackResolution {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
        conflicted: "a".into(),
    });
    assert!(failure(&started).is_none(), "start refused: {started:#?}");
    let events = h.send(CommandKind::CancelRestackResolution);
    assert!(failure(&events).is_none(), "cancel refused: {events:#?}");
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec![format!("aborted a {conflict}")]);
    assert_eq!(h.runs("wait-restack")[0].status, WorkflowRunStatus::Done);
}

#[test]
fn starting_and_resolving_a_conflict_does_not_wake_the_wait() {
    // Only finishing or cancelling the resolution ends a wait. `ctx.wait` accepts no other name
    // (the catalog declares `restack.resumed` alone), so the other names are the runtime's test.
    let (_dir, mut h, run, _conflict) = waiting_on_a_conflict("wait-restack", WAIT_RESTACK);
    let plan = h.plan("main");
    let started = h.send(CommandKind::StartRestackResolution {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
        conflicted: "a".into(),
    });
    assert!(failure(&started).is_none(), "start refused: {started:#?}");
    let resolved = h.send(CommandKind::ResolveConflict {
        path: "a.txt".into(),
        text: "resolved\n".into(),
        origins: Vec::new(),
        site: ConflictSite::Restack,
    });
    assert!(
        failure(&resolved).is_none(),
        "resolve refused: {resolved:#?}"
    );
    let later = h.settle(Duration::from_millis(300));
    assert!(notices(&started).is_empty() && notices(&resolved).is_empty());
    assert!(
        notices(&later).is_empty() && !finished(&later),
        "{later:#?}"
    );
    assert_eq!(h.runs("wait-restack")[0].status, WorkflowRunStatus::Waiting);

    // Cancelling the run itself ends the wait at once, as the docs say.
    let events = h.send(CommandKind::CancelWorkflowRun { run });
    let events = h.pump_until(events, finished);
    assert!(notices(&events).is_empty(), "{events:#?}");
    assert_eq!(
        h.runs("wait-restack")[0].status,
        WorkflowRunStatus::Cancelled
    );
}

#[test]
fn a_wait_with_a_timeout_still_times_out_when_no_event_comes() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "wait-short.lua", WAIT_SHORT);
    let mut h = Harness::new(&repo, &flows);
    h.review("wait-short", false);
    let events = h.start("wait-short");
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["timeout".to_owned()]);
    assert_eq!(h.runs("wait-short")[0].status, WorkflowRunStatus::Done);
}

/// `feat` is a child of `main`, based on the tip of `main` (its stack links are kept in the
/// config, as `set_parent` writes them).
fn stacked_repo(repo: &Path) {
    let base = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("feat.txt"), "feat\n").unwrap();
    git(repo, &["add", "feat.txt"]);
    git(repo, &["commit", "-q", "-m", "feat"]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["config", "branch.feat.stackParent", "main"]);
    git(repo, &["config", "branch.feat.stackParentBase", &base]);
}

#[test]
fn opening_a_repository_with_a_stale_branch_fires_nothing() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    // `main` moves after `feat` was based on it: `feat` is stale when the repository opens.
    std::fs::write(repo.join("elsewhere.txt"), "x\n").unwrap();
    git(&repo, &["add", "elsewhere.txt"]);
    git(&repo, &["commit", "-q", "-m", "main moves"]);
    write_script(&flows, "on-parent-moved.lua", PARENT_MOVED);
    let mut h = Harness::new(&repo, &flows);
    assert!(wants_review(&h.opened).is_empty(), "{:#?}", h.opened);
    assert!(started_runs(&h.opened).is_empty());
    h.review("on-parent-moved", false);
    let events = h.refs_seen();
    let later = h.settle(Duration::from_millis(300));
    assert!(notices(&events).is_empty() && notices(&later).is_empty());
    assert!(started_runs(&events).is_empty() && started_runs(&later).is_empty());
}

#[test]
fn a_commit_on_the_parent_fires_parent_moved_once_with_the_branch_and_parent() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    write_script(&flows, "on-parent-moved.lua", PARENT_MOVED);
    write_script(&flows, "on-other-moved.lua", PARENT_MOVED_OTHER);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-parent-moved", false);
    h.review("on-other-moved", false);
    // The person commits on `main`, as the window does; the watcher then sees the ref move.
    h.commit_file("elsewhere.txt", "x\n", "main moves");
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    let later = h.settle(Duration::from_millis(300));
    assert_eq!(notices(&events), vec!["moved feat main".to_owned()]);
    assert!(notices(&later).is_empty(), "{later:#?}");
    assert_eq!(h.runs("on-parent-moved").len(), 1);
    assert!(
        h.runs("on-other-moved").is_empty(),
        "match filters branch:other/*"
    );

    // Nothing moved since: another check fires nothing.
    let events = h.refs_seen();
    let later = h.settle(Duration::from_millis(300));
    assert!(started_runs(&events).is_empty() && started_runs(&later).is_empty());
    assert_eq!(h.runs("on-parent-moved").len(), 1);
}

#[test]
fn a_deleted_parent_fires_parent_merged() {
    let (_dir, repo, flows) = setup();
    // main <- base <- top
    let main_tip = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "base"]);
    std::fs::write(repo.join("base.txt"), "base\n").unwrap();
    git(&repo, &["add", "base.txt"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["config", "branch.base.stackParent", "main"]);
    git(&repo, &["config", "branch.base.stackParentBase", &main_tip]);
    let base_tip = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "top"]);
    std::fs::write(repo.join("top.txt"), "top\n").unwrap();
    git(&repo, &["add", "top.txt"]);
    git(&repo, &["commit", "-q", "-m", "top"]);
    git(&repo, &["config", "branch.top.stackParent", "base"]);
    git(&repo, &["config", "branch.top.stackParentBase", &base_tip]);
    git(&repo, &["checkout", "-q", "main"]);

    write_script(&flows, "on-parent-merged.lua", PARENT_MERGED);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-parent-merged", false);
    git(&repo, &["branch", "-D", "base"]);
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["gone top base".to_owned()]);
    let later = h.refs_seen();
    let later = h
        .settle(Duration::from_millis(300))
        .into_iter()
        .chain(later);
    assert!(notices(&later.collect::<Vec<_>>()).is_empty());
    assert_eq!(h.runs("on-parent-merged").len(), 1);
}

#[test]
fn a_commit_made_by_a_run_on_the_parent_fires_nothing() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    write_script(&flows, "on-parent-moved.lua", PARENT_MOVED);
    write_script(&flows, "commit-elsewhere.lua", COMMIT_ELSEWHERE);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-parent-moved", false);
    h.review("commit-elsewhere", false);
    std::fs::write(repo.join("elsewhere.txt"), "x\n").unwrap();
    let events = h.start("commit-elsewhere");
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["committed".to_owned()]);
    // The run moved `main`; the watcher's check after it sees no change to report.
    let events = h.refs_seen();
    let later = h.settle(Duration::from_millis(300));
    assert!(notices(&events).is_empty() && notices(&later).is_empty());
    assert!(started_runs(&events).is_empty() && started_runs(&later).is_empty());
    assert!(h.runs("on-parent-moved").is_empty());
}
