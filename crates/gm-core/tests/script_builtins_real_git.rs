//! Per-repository switches and the built-in stack workflows (design/workflow-scripts.md, `WF4b`,
//! D164): a switched-off workflow does not run in its repository, a built-in one runs only when
//! switched on there, the restack, fetch and reparent built-ins act on real stacks, and Copy to
//! Edit writes an unreviewed copy. Real repositories in temporary directories; this harness plays
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
    RestackPlan, RestackRecord, WorkflowEnd, WorkflowInfo, WorkflowRunStatus, WorkflowRunSummary,
};

const RESTACK_CHILDREN: &str = "restack-children-on-parent-move";
const RESTACK_STALE: &str = "restack-stale-after-fetch";
const REPARENT: &str = "reparent-to-grandparent-on-merge";

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
}

impl Harness {
    /// Opens `repo` with the user's workflows in `flows`, the way the app does: triggers on.
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
        };
        let opened = harness.send(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        assert!(failure(&opened).is_none(), "open: {opened:#?}");
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

    /// The listing, as the Workflows page has it.
    fn listing(&mut self) -> Vec<WorkflowInfo> {
        let events = self.send(CommandKind::ListWorkflows);
        events
            .iter()
            .find_map(|e| match e {
                Event::WorkflowsListed { workflows, .. } => Some(workflows.clone()),
                _ => None,
            })
            .expect("a listing")
    }

    /// Reviews the workflow `name` as the listing shows it, with the grant if `auto_approve`.
    fn review(&mut self, name: &str, auto_approve: bool) {
        let hash = self
            .listing()
            .into_iter()
            .find(|w| w.name == name)
            .unwrap_or_else(|| panic!("no workflow named {name}"))
            .hash;
        let events = self.send(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
    }

    /// Switches the workflow `name` on or off in this repository.
    fn switch(&mut self, name: &str, enabled: bool) -> Vec<Event> {
        let events = self.send(CommandKind::SetWorkflowEnabled {
            name: name.to_owned(),
            enabled,
        });
        assert!(failure(&events).is_none(), "switch refused: {events:#?}");
        events
    }

    /// Switches a built-in on with the grant, as the Switch On dialog does when ticked.
    fn switch_on_with_grant(&mut self, name: &str) {
        self.switch(name, true);
        self.review(name, true);
    }

    /// Writes `file`, stages only that file and commits it, as the person does in the window.
    fn commit_file(&mut self, file: &str, text: &str, message: &str) -> Vec<Event> {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
        let events = self.send(CommandKind::Commit {
            message: message.to_owned(),
        });
        assert!(
            events.iter().any(|e| matches!(e, Event::Committed { .. })),
            "no commit: {events:#?}"
        );
        events
    }

    /// Runs `Fetch` the way the window does: the job runs on this thread, then its report.
    fn fetch(&mut self) -> Vec<Event> {
        let mut events = self.send(CommandKind::Fetch);
        for job in self.engine.take_remote_jobs() {
            let report = job.run();
            events.extend(self.engine.remote_report(report));
        }
        events
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

    /// The runs of the workflow `name`, as the Runs tab lists them.
    fn runs(&mut self, name: &str) -> Vec<WorkflowRunSummary> {
        let events = self.send(CommandKind::ListWorkflowRuns {
            workflow: name.into(),
            limit: 10,
        });
        events
            .iter()
            .find_map(|e| match e {
                Event::WorkflowRunsListed { runs, .. } => Some(runs.clone()),
                _ => None,
            })
            .expect("a list of runs")
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
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::WorkflowRunFinished { .. }))
}

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    let flows = dir.path().join("flows");
    std::fs::create_dir_all(&flows).unwrap();
    (dir, repo, flows)
}

/// `main <- feat`: `feat` has its own file and its recorded parent and base.
fn stacked_repo(repo: &Path) -> String {
    let base = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("feat.txt"), "feat\n").unwrap();
    git(repo, &["add", "feat.txt"]);
    git(repo, &["commit", "-q", "-m", "feat"]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["config", "branch.feat.stackParent", "main"]);
    git(repo, &["config", "branch.feat.stackParentBase", &base]);
    base
}

/// `main <- feat`, where `feat` changes `a.txt` and `main` will change it too.
fn conflicting_stack(repo: &Path) {
    let base = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("a.txt"), "from feat\n").unwrap();
    git(repo, &["commit", "-q", "-am", "feat: a.txt"]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["config", "branch.feat.stackParent", "main"]);
    git(repo, &["config", "branch.feat.stackParentBase", &base]);
}

const ON_COMMIT: &str = r#"
local M = {}
M.meta = { name = "on-commit", trigger = { on = "commit.created" } }
function M.execute(ctx, input)
  ctx.notify("commit " .. input.trigger.oid)
end
return M
"#;

#[test]
fn a_switched_off_workflow_does_not_run_here_and_runs_in_another_repository() {
    let (dir, repo, flows) = setup();
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    init_repo(&other);
    write_script(&flows, "on-commit.lua", ON_COMMIT);
    let mut here = Harness::new(&repo, &flows);
    let mut there = Harness::new(&other, &flows);
    here.review("on-commit", false);
    there.review("on-commit", false);
    here.switch("on-commit", false);

    let events = here.commit_file("b.txt", "b\n", "here");
    let settled = here.settle(Duration::from_millis(300));
    assert!(notices(&events).is_empty() && notices(&settled).is_empty());
    assert!(here.runs("on-commit").is_empty(), "switched off here");

    let events = there.commit_file("b.txt", "b\n", "there");
    let events = there.pump_until(events, |e| notices(e).len() == 1);
    assert_eq!(notices(&events).len(), 1, "{events:#?}");
    assert!(notices(&events)[0].starts_with("commit "));
}

const BY_HAND: &str = r#"
local M = {}
M.meta = { name = "by-hand", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.notify("ran by hand")
end
return M
"#;

#[test]
fn a_switched_off_workflow_still_runs_by_hand() {
    let (_dir, repo, flows) = setup();
    write_script(&flows, "by-hand.lua", BY_HAND);
    let mut h = Harness::new(&repo, &flows);
    h.review("by-hand", false);
    h.switch("by-hand", false);
    let listed = h.listing();
    assert!(!listed.iter().find(|w| w.name == "by-hand").unwrap().enabled);
    let events = h.send(CommandKind::RunWorkflow {
        name: "by-hand".into(),
        inputs: BTreeMap::new(),
    });
    let events = h.pump_until(events, finished);
    assert_eq!(notices(&events), vec!["ran by hand".to_owned()]);
}

#[test]
fn a_built_in_workflow_is_listed_as_built_in_and_is_off_until_switched_on() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    let mut h = Harness::new(&repo, &flows);
    let listed = h.listing();
    let built: Vec<&WorkflowInfo> = listed.iter().filter(|w| w.builtin()).collect();
    assert_eq!(built.len(), 3, "{listed:#?}");
    let restack = built.iter().find(|w| w.name == RESTACK_CHILDREN).unwrap();
    assert!(!restack.enabled && restack.reviewed);
    assert_eq!(
        restack.title.as_deref(),
        Some("Restack children when parent moves")
    );
    assert!(restack.path.starts_with("builtin:"));
    assert!(!flows.join("restack-children-on-parent-move.lua").exists());

    // Off: a commit on main restacks nothing.
    h.commit_file("elsewhere.txt", "x\n", "main moves");
    let events = h.refs_seen();
    let later = h.settle(Duration::from_millis(300));
    assert!(started_runs(&events).is_empty() && started_runs(&later).is_empty());
    assert!(h.runs(RESTACK_CHILDREN).is_empty());
    assert_eq!(
        git(&repo, &["merge-base", "main", "feat"]),
        git(&repo, &["rev-parse", "main~1"]),
        "feat was not moved"
    );
}

#[test]
fn a_commit_on_the_parent_restacks_the_child_with_the_grant_and_no_approval() {
    let (_dir, repo, flows) = setup();
    let base = stacked_repo(&repo);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);
    let events = h.commit_file("elsewhere.txt", "x\n", "main moves");
    assert!(approvals(&events).is_empty());
    let new_main = git(&repo, &["rev-parse", "main"]);
    assert_ne!(new_main, base);
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    assert!(
        approvals(&events).is_empty(),
        "the grant applies: {events:#?}"
    );
    assert_eq!(ends(&events).len(), 1);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(
        notices(&events),
        vec!["Restacked 1 branch onto main".to_owned()]
    );

    // The child now sits on the new tip of main, and its base is that tip.
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
    assert_eq!(
        git(&repo, &["config", "branch.feat.stackParentBase"]),
        new_main
    );
    assert_eq!(h.runs(RESTACK_CHILDREN)[0].status, WorkflowRunStatus::Done);
}

#[test]
fn without_the_grant_the_restack_waits_for_approval_at_apply() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    let mut h = Harness::new(&repo, &flows);
    h.switch(RESTACK_CHILDREN, true);
    let events = h.commit_file("elsewhere.txt", "x\n", "main moves");
    assert!(approvals(&events).is_empty());
    let events = h.refs_seen();
    let events = h.pump_until(events, |e| !approvals(e).is_empty());
    let run = approvals(&events)[0];
    assert_eq!(
        h.runs(RESTACK_CHILDREN)[0].status,
        WorkflowRunStatus::AwaitingApproval
    );
    let events = h.send(CommandKind::ApproveWorkflowStep {
        run,
        approved: true,
    });
    assert!(failure(&events).is_none(), "{events:#?}");
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
}

#[test]
fn a_restack_conflict_waits_for_the_resolution_and_then_restacks_the_rest() {
    let (_dir, repo, flows) = setup();
    conflicting_stack(&repo);
    // A child of feat with its own file: it has to follow feat once feat is resolved.
    let feat_tip = git(&repo, &["rev-parse", "feat"]);
    git(&repo, &["checkout", "-q", "-b", "sub", "feat"]);
    std::fs::write(repo.join("sub.txt"), "sub\n").unwrap();
    git(&repo, &["add", "sub.txt"]);
    git(&repo, &["commit", "-q", "-m", "sub"]);
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["config", "branch.sub.stackParent", "feat"]);
    git(&repo, &["config", "branch.sub.stackParentBase", &feat_tip]);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);

    let events = h.commit_file("a.txt", "from main\n", "main moves");
    assert!(approvals(&events).is_empty());
    let mut events = h.refs_seen();
    events.extend(h.settle(Duration::from_secs(1)));
    assert!(!finished(&events), "the wait ended early: {events:#?}");
    assert_eq!(
        h.runs(RESTACK_CHILDREN)[0].status,
        WorkflowRunStatus::Waiting
    );

    // Resolve the conflict of feat and finish it, as the restack dialog does.
    let plan = h.plan("main");
    let events = h.send(CommandKind::StartRestackResolution {
        branch: "main".into(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
        conflicted: "feat".into(),
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
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(
        notices(&events),
        vec!["Restacked 2 branches onto main".to_owned()],
        "the run re-planned after the resume: feat and sub moved"
    );

    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
    git(&repo, &["merge-base", "--is-ancestor", "feat", "sub"]);
    assert_eq!(git(&repo, &["show", "feat:a.txt"]), "resolved");
}

#[test]
fn restack_stale_after_fetch_restacks_a_stale_stack_from_a_local_remote() {
    let (dir, repo, flows) = setup();
    let remote = dir.path().join("origin.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);
    stacked_repo(&repo);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_STALE);
    // The parent moves while the child waits: the stack is stale before the fetch.
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "main moves"],
    );
    let old_base = git(&repo, &["config", "branch.feat.stackParentBase"]);
    let events = h.fetch();
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(
        notices(&events),
        vec!["Restacked 1 branch after the fetch".to_owned()]
    );
    assert_ne!(
        git(&repo, &["config", "branch.feat.stackParentBase"]),
        old_base
    );
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
}

#[test]
fn reparent_to_grandparent_moves_a_branch_off_a_deleted_parent() {
    let (_dir, repo, flows) = setup();
    // main <- b <- c
    git(&repo, &["checkout", "-q", "-b", "b"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&repo, &["add", "b.txt"]);
    git(&repo, &["commit", "-q", "-m", "b"]);
    let b_base = git(&repo, &["rev-parse", "main"]);
    git(&repo, &["config", "branch.b.stackParent", "main"]);
    git(&repo, &["config", "branch.b.stackParentBase", &b_base]);
    git(&repo, &["checkout", "-q", "-b", "c"]);
    std::fs::write(repo.join("c.txt"), "c\n").unwrap();
    git(&repo, &["add", "c.txt"]);
    git(&repo, &["commit", "-q", "-m", "c"]);
    git(&repo, &["config", "branch.c.stackParent", "b"]);
    git(
        &repo,
        &[
            "config",
            "branch.c.stackParentBase",
            &git(&repo, &["rev-parse", "b"]),
        ],
    );
    git(&repo, &["checkout", "-q", "main"]);

    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(REPARENT);
    // b is merged and its branch deleted: c is left with a parent that is gone.
    git(&repo, &["branch", "-D", "b"]);
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(notices(&events), vec!["Reparented c onto main".to_owned()]);
    assert_eq!(git(&repo, &["config", "branch.c.stackParent"]), "main");
    assert_eq!(git(&repo, &["config", "branch.c.stackParentBase"]), b_base);
    assert_eq!(git(&repo, &["log", "--format=%s", "main..c"]), "c");
}

#[test]
fn copy_to_edit_writes_an_unreviewed_copy_under_a_new_name() {
    let (_dir, repo, flows) = setup();
    let mut h = Harness::new(&repo, &flows);
    let events = h.send(CommandKind::CopyBuiltinWorkflow {
        name: RESTACK_CHILDREN.into(),
    });
    assert!(failure(&events).is_none(), "{events:#?}");
    let (path, name) = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowCopied { path, name, .. } => Some((path.clone(), name.clone())),
            _ => None,
        })
        .expect("a copy");
    assert_eq!(name, "restack-children-on-parent-move-custom");
    assert_eq!(PathBuf::from(&path), flows.join(format!("{name}.lua")));

    let listed = h.listing();
    let copy = listed.iter().find(|w| w.name == name).expect("listed");
    assert!(!copy.builtin() && !copy.reviewed && copy.enabled);
    assert_eq!(copy.trigger, "stack.parent_moved");

    // A second copy takes the next name and never overwrites the first.
    let events = h.send(CommandKind::CopyBuiltinWorkflow {
        name: RESTACK_CHILDREN.into(),
    });
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkflowCopied { name, .. } if name == "restack-children-on-parent-move-custom-2"
    )));
}

const ON_PARENT_MOVED: &str = r#"
local M = {}
M.meta = { name = "on-parent-moved", trigger = { on = "stack.parent_moved" } }
function M.execute(ctx, input)
  ctx.notify("moved " .. input.trigger.branch)
end
return M
"#;

#[test]
fn a_branch_already_stale_when_the_restack_is_switched_on_is_restacked_by_the_next_parent_commit() {
    let (_dir, repo, flows) = setup();
    let base = stacked_repo(&repo);
    // main moves before the repository opens, so feat is stale from the start.
    std::fs::write(repo.join("elsewhere.txt"), "x\n").unwrap();
    git(&repo, &["add", "elsewhere.txt"]);
    git(&repo, &["commit", "-q", "-m", "main moves"]);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);
    let settled = h.settle(Duration::from_millis(300));
    assert!(
        h.runs(RESTACK_CHILDREN).is_empty() && started_runs(&settled).is_empty(),
        "switching on fires nothing: {settled:#?}"
    );

    // Another commit on main moves its tip again: feat is still stale, and its parent moved.
    let mut events = h.commit_file("elsewhere2.txt", "y\n", "main moves again");
    events.extend(h.refs_seen());
    let events = h.pump_until(events, finished);
    assert!(
        approvals(&events).is_empty(),
        "the grant applies: {events:#?}"
    );
    assert_eq!(ends(&events).len(), 1, "{events:#?}");
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(
        notices(&events),
        vec!["Restacked 1 branch onto main".to_owned()]
    );
    let new_main = git(&repo, &["rev-parse", "main"]);
    assert_ne!(new_main, base);
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
    assert_eq!(
        git(&repo, &["config", "branch.feat.stackParentBase"]),
        new_main
    );
    let runs = h.runs(RESTACK_CHILDREN);
    assert_eq!(runs.len(), 1, "{runs:#?}");
    assert_eq!(runs[0].status, WorkflowRunStatus::Done);
    assert_eq!(runs[0].started_by, "event:stack.parent_moved");

    // A refresh with nothing moved fires nothing.
    let mut events = h.refs_seen();
    events.extend(h.settle(Duration::from_millis(300)));
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert_eq!(h.runs(RESTACK_CHILDREN).len(), 1);
}

#[test]
fn a_stale_branch_fires_parent_moved_once_per_parent_move_and_not_on_a_refresh() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    // main moves before the repository opens: feat is stale from the start.
    std::fs::write(repo.join("elsewhere.txt"), "x\n").unwrap();
    git(&repo, &["add", "elsewhere.txt"]);
    git(&repo, &["commit", "-q", "-m", "main moves"]);
    write_script(&flows, "on-parent-moved.lua", ON_PARENT_MOVED);
    let mut h = Harness::new(&repo, &flows);
    h.review("on-parent-moved", false);

    // The opening snapshot is the baseline, and a refresh with no move fires nothing.
    let mut events = h.refs_seen();
    events.extend(h.settle(Duration::from_millis(300)));
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert!(h.runs("on-parent-moved").is_empty());

    // The parent moves again: feat is still stale and its parent tip differs, so it fires once.
    let mut events = h.commit_file("elsewhere2.txt", "y\n", "main moves again");
    events.extend(h.refs_seen());
    let events = h.pump_until(events, finished);
    assert_eq!(
        notices(&events),
        vec!["moved feat".to_owned()],
        "{events:#?}"
    );
    let events = h.settle(Duration::from_millis(300));
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert_eq!(h.runs("on-parent-moved").len(), 1);

    // feat stays stale and nothing moves: a refresh fires nothing.
    let mut events = h.refs_seen();
    events.extend(h.settle(Duration::from_millis(300)));
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert_eq!(h.runs("on-parent-moved").len(), 1);
}

/// The plan an approval asked for, when the approval is an apply of a restack (`WF4c`).
fn approval_plan(events: &[Event]) -> Option<RestackPlan> {
    events.iter().find_map(|e| match e {
        Event::WorkflowApprovalRequested { plan, .. } => plan.clone(),
        _ => None,
    })
}

/// The restack conflicts the window was told about: the run, its workflow and the branch the
/// plan after the apply blocks on a conflict (`WF4c`).
fn conflicts(events: &[Event]) -> Vec<(i64, String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRestackConflict {
                run,
                workflow,
                plan,
            } => {
                let branch = plan
                    .branches
                    .iter()
                    .find(|b| matches!(b.block, Some(RestackBlock::Conflict { .. })))
                    .map_or_else(String::new, |b| b.name.clone());
                Some((*run, workflow.clone(), branch))
            }
            _ => None,
        })
        .collect()
}

/// The record each restack that ran reported; `None` for an undo (`WF4c`).
fn restack_records(events: &[Event]) -> Vec<Option<RestackRecord>> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::RestackFinished { record, .. } => Some(record.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_approval_of_a_restack_apply_carries_the_plan_that_moves_the_branches() {
    let (_dir, repo, flows) = setup();
    let base = stacked_repo(&repo);
    let mut h = Harness::new(&repo, &flows);
    h.switch(RESTACK_CHILDREN, true);
    let events = h.commit_file("elsewhere.txt", "x\n", "main moves");
    assert!(approvals(&events).is_empty());
    let events = h.refs_seen();
    let events = h.pump_until(events, |e| !approvals(e).is_empty());
    let plan = approval_plan(&events).expect("the approval carries the plan");
    // The plan moves feat onto main, replaying its one commit, and holds nothing back.
    assert_eq!(plan.from, "feat", "{plan:#?}");
    assert_eq!(plan.branches.len(), 1, "{plan:#?}");
    let feat = &plan.branches[0];
    assert_eq!((feat.name.as_str(), feat.parent.as_str()), ("feat", "main"));
    assert_eq!(feat.commits.len(), 1);
    assert_eq!(feat.block, None);
    assert!(feat.new_tip.is_some());
    // Nothing moved yet: the approval holds the apply.
    assert_eq!(git(&repo, &["config", "branch.feat.stackParentBase"]), base);

    let run = approvals(&events)[0];
    let events = h.send(CommandKind::ApproveWorkflowStep {
        run,
        approved: true,
    });
    assert!(failure(&events).is_none(), "{events:#?}");
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
}

#[test]
fn a_restack_conflict_in_a_run_notifies_with_the_branch_and_resolves_from_its_plan() {
    let (_dir, repo, flows) = setup();
    conflicting_stack(&repo);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);

    let events = h.commit_file("a.txt", "from main\n", "main moves");
    assert!(conflicts(&events).is_empty());
    let events = h.refs_seen();
    let events = h.pump_until(events, |e| !conflicts(e).is_empty());
    let found = conflicts(&events);
    assert_eq!(found.len(), 1, "{events:#?}");
    let (run, workflow, branch) = found[0].clone();
    assert_eq!(workflow, RESTACK_CHILDREN);
    assert_eq!(branch, "feat");
    // The run reaches its wait a moment after the apply reported the conflict.
    let until = Instant::now() + Duration::from_secs(10);
    while h.runs(RESTACK_CHILDREN)[0].status != WorkflowRunStatus::Waiting {
        assert!(
            Instant::now() < until,
            "the run never waited for the resolution"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!finished(&events), "the run did not end: {events:#?}");

    // The notice opens the restack dialog on the plan's root; Resolve conflict starts from that
    // plan, by its id (D154).
    let plan = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRestackConflict { plan, .. } => Some(plan.clone()),
            _ => None,
        })
        .expect("the conflict plan");
    assert_eq!(
        plan.from, "feat",
        "the run's restack starts at its trigger branch"
    );
    let events = h.send(CommandKind::StartRestackResolution {
        branch: plan.from.clone(),
        plan_id: plan.plan_id.clone(),
        skip: Vec::new(),
        conflicted: branch.clone(),
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
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].0, run);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_eq!(
        notices(&events),
        vec!["Restacked 1 branch onto main".to_owned()],
        "the run re-planned after the resume"
    );
    assert!(
        approvals(&events).is_empty(),
        "the grant covers the re-planned apply"
    );
    // The apply after the resume moved feat, so the window can undo it (WF4c).
    let records = restack_records(&events);
    assert!(
        records.iter().any(|r| r
            .as_ref()
            .is_some_and(|r| r.moves.iter().any(|m| m.name == "feat"))),
        "{records:#?}"
    );
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
    assert_eq!(git(&repo, &["show", "feat:a.txt"]), "resolved");
}

#[test]
fn a_workflow_restack_reports_its_record_and_undo_puts_the_branches_back() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    let feat_before = git(&repo, &["rev-parse", "feat"]);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);
    h.commit_file("elsewhere.txt", "x\n", "main moves");
    let events = h.refs_seen();
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    let records = restack_records(&events);
    let record = records
        .iter()
        .flatten()
        .next()
        .unwrap_or_else(|| panic!("the run reported no record: {events:#?}"));
    assert_eq!(record.from, "feat");
    assert_eq!(record.moves.len(), 1, "{record:#?}");
    assert_eq!(record.moves[0].name, "feat");
    assert_ne!(git(&repo, &["rev-parse", "feat"]), feat_before);

    // Undo, as the window's restack dialog does: the record goes, and feat is where it was.
    let events = h.send(CommandKind::UndoRestack);
    assert!(failure(&events).is_none(), "undo refused: {events:#?}");
    assert_eq!(restack_records(&events), vec![None]);
    assert_eq!(git(&repo, &["rev-parse", "feat"]), feat_before);
}

#[test]
fn undo_of_a_restack_is_a_baseline_and_the_next_parent_move_restacks_again() {
    let (_dir, repo, flows) = setup();
    stacked_repo(&repo);
    let feat_before = git(&repo, &["rev-parse", "feat"]);
    let mut h = Harness::new(&repo, &flows);
    h.switch_on_with_grant(RESTACK_CHILDREN);

    // main moves: the run restacks feat.
    let mut events = h.commit_file("elsewhere.txt", "x\n", "main moves");
    events.extend(h.refs_seen());
    let mut events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    assert_ne!(git(&repo, &["rev-parse", "feat"]), feat_before);
    events.extend(h.settle(Duration::from_millis(300)));
    assert_eq!(h.runs(RESTACK_CHILDREN).len(), 1);

    // Undo from the window: feat is back where it was, and the undo starts no run.
    let mut events = h.send(CommandKind::UndoRestack);
    assert!(failure(&events).is_none(), "undo refused: {events:#?}");
    events.extend(h.refs_seen());
    events.extend(h.settle(Duration::from_millis(500)));
    assert_eq!(git(&repo, &["rev-parse", "feat"]), feat_before);
    assert!(started_runs(&events).is_empty(), "{events:#?}");
    assert_eq!(h.runs(RESTACK_CHILDREN).len(), 1, "no new run after undo");

    // The next move of main fires as usual and restacks feat again.
    let mut events = h.commit_file("elsewhere2.txt", "y\n", "main moves again");
    events.extend(h.refs_seen());
    let events = h.pump_until(events, finished);
    assert_eq!(ends(&events)[0].1, WorkflowEnd::Done, "{events:#?}");
    git(&repo, &["merge-base", "--is-ancestor", "main", "feat"]);
    assert_eq!(
        git(&repo, &["config", "branch.feat.stackParentBase"]),
        git(&repo, &["rev-parse", "main"])
    );
    assert_eq!(h.runs(RESTACK_CHILDREN).len(), 2);
}
