//! The gates of a push and a restack reached from a workflow (`ctx.push.lease`, `ctx.restack.apply`),
//! the native restack of a whole stack, and the check of a push's tip before it is sent (D212 (2),
//! (3), (4), (6a), R2h), against real repositories in temporary directories. The remotes are local
//! bare repositories, and the user's gate, reviewer and workflow directories are temporary ones.
//! The harness plays the engine thread: it carries a run's effects and events and runs the Git
//! jobs, as the runner does. Each test removes its directory when it ends.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink, reviewers};
use gm_schema::{
    Actor, Command, CommandKind, Event, FindingInput, GateOp, OperationId, OperationState,
    RestackPlan, ReviewSeverity, Reviewer, ReviewerDirection, ReviewerSignal, ReviewerSource,
    ReviewerSubject, WorkflowEnd,
};

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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The tip of `branch` in the bare remote, empty when the remote has no such branch.
fn remote_tip(origin: &Path, branch: &str) -> String {
    git(
        origin,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/heads/{branch}"),
        ],
    )
}

fn tip(repo: &Path, branch: &str) -> String {
    git(repo, &["rev-parse", branch])
}

/// Writes `text` to `f.txt` and commits it on the branch checked out in `dir`.
fn commit(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("f.txt"), text).unwrap();
    git(dir, &["commit", "-q", "-am", message]);
}

/// Commits `file` on `branch`, then leaves `main` checked out.
fn commit_on(repo: &Path, branch: &str, file: &str, content: &str) {
    git(repo, &["checkout", "-q", branch]);
    std::fs::write(repo.join(file), content).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", &format!("{file}: {content}")]);
    git(repo, &["checkout", "-q", "main"]);
}

/// `origin` (bare, with `main`), and `repo` on `feat` with one commit that is not pushed.
struct Push {
    _dir: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    user: PathBuf,
}

fn push_fixture() -> Push {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", &s(&origin)],
    );
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["remote", "add", "origin", &s(&origin)]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit(&repo, "one\nthree\n", "feat change");
    let user = dir.path().join("user");
    std::fs::create_dir_all(user.join("flows")).unwrap();
    Push {
        _dir: dir,
        origin,
        repo,
        user,
    }
}

/// `main` -> `a` -> `b` -> `c`, each with one commit, then `main` moves on. The restack of `a`
/// moves the three of them. The repository is on `main`.
struct Chain {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    user: PathBuf,
}

fn chain_fixture() -> Chain {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let user = dir.path().join("user");
    std::fs::create_dir_all(user.join("flows")).unwrap();
    let chain = Chain {
        _dir: dir,
        repo,
        user,
    };
    let mut session = Session::open(&chain.repo, &chain.user);
    for (name, parent) in [("a", "main"), ("b", "a"), ("c", "b")] {
        let events = session.human(CommandKind::CreateChildBranch {
            name: name.into(),
            parent: parent.into(),
            checkout: false,
        });
        assert_eq!(state(&events), OperationState::Done, "{events:?}");
        commit_on(&chain.repo, name, &format!("{name}.txt"), name);
    }
    commit_on(&chain.repo, "main", "m.txt", "m");
    chain
}

fn write_repo_gates(dir: &Path, text: &str) {
    let gitmaster = dir.join(".git").join("gitmaster");
    std::fs::create_dir_all(&gitmaster).unwrap();
    std::fs::write(gitmaster.join("gates.json"), text).unwrap();
}

/// The gate that needs a person's approval of the tip, on push and restack alike.
const APPROVED_PUSH: &str = r#"{"id":"approved","kind":"human_approval","on":["push"]}"#;
/// The gate that needs a person's approval of the branch `b` only, on restack (D214 (1)).
const APPROVED_B: &str =
    r#"{"id":"b-approved","kind":"human_approval","on":["restack"],"when":{"branches":["b"]}}"#;
/// The findings gate that blocks on any unresolved high finding.
const NO_HIGH: &str = r#"{"id":"no-high","kind":"findings","severity":"high","on":["push"]}"#;

fn save_security(dir: &Path) {
    let reviewer = Reviewer {
        name: "security".to_owned(),
        description: "Looks for unchecked input.".to_owned(),
        instructions: "Read the change and flag unchecked input.".to_owned(),
        signal: ReviewerSignal::Level,
        direction: ReviewerDirection::LowerIsBetter,
        subject: ReviewerSubject::Branch,
        model: None,
        source: ReviewerSource::Repository,
        paths: Vec::new(),
        branches: Vec::new(),
    };
    reviewers::save(&reviewers::repo_dir(&dir.join(".git")), &reviewer).unwrap();
}

fn high_finding() -> FindingInput {
    FindingInput {
        severity: ReviewSeverity::High,
        path: "f.txt".to_owned(),
        start_line: 2,
        end_line: 2,
        body: "unchecked input".to_owned(),
        suggested_fix: None,
    }
}

/// The agent's verdict of `security` on `feat` at its tip, with one high finding.
fn security_with_finding() -> CommandKind {
    CommandKind::RecordVerdict {
        reviewer: "security".to_owned(),
        branch: "feat".to_owned(),
        head: "HEAD".to_owned(),
        value: "low".to_owned(),
        rationale: "Checked the change.".to_owned(),
        findings: vec![high_finding()],
    }
}

/// A workflow `name` for a manual run, whose `execute` body is `body`.
fn flow(name: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"manual\" }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

/// A workflow for `on` (a phase such as `before.push`) whose `execute` body is `body`.
fn phase(name: &str, on: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\" }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

const PUSH_FEAT: &str = r#"  ctx.push.lease({ branch = "feat" })"#;

const RESTACK_A: &str = r#"  local plan = ctx.restack.plan({ root = "a" })
  ctx.restack.apply({ plan_id = plan.plan_id })"#;

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
}

/// The engine of one repository, with the channel its runs' effects and events come through.
struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    last: u64,
}

impl Harness {
    /// Opens `repo` with the workflows, gates and reviewers of `user`, and the trigger switch on.
    fn open(repo: &Path, user: &Path) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(user.join("flows")));
        engine.set_gates_user_dir(Some(user.to_path_buf()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(user)));
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
            last: 0,
        };
        let events = harness.human(CommandKind::OpenRepo { path: s(repo) });
        assert_eq!(state(&events), OperationState::Done, "open: {events:?}");
        harness
    }

    /// Runs a command as `actor` and returns its events, the Git jobs it started included.
    fn send(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.last += 1;
        let mut events = self
            .engine
            .handle(OperationId(self.last), &Command { actor, kind });
        events.extend(self.jobs());
        events
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.send(Actor::Human, kind)
    }

    fn agent(&mut self, kind: CommandKind) -> Vec<Event> {
        self.send(agent(), kind)
    }

    /// Runs the Git jobs the engine queued, and their follow-ups, until none is left.
    fn jobs(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let remote = self.engine.take_remote_jobs();
            let scripts = self.engine.take_script_jobs();
            if remote.is_empty() && scripts.is_empty() {
                return events;
            }
            for job in remote {
                let report = job.run();
                events.extend(self.engine.remote_report(report));
            }
            for job in scripts {
                let report = job.run();
                events.extend(self.engine.script_remote_report(report));
            }
        }
    }

    /// Plays the engine thread for one message, if one comes within a moment.
    fn step(&mut self) -> Vec<Event> {
        let mut events = match self.messages.recv_timeout(Duration::from_millis(20)) {
            Ok(Message::Effect(request)) => self.engine.script_effect(request),
            Ok(Message::Event(event)) => self.engine.script_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        };
        events.extend(self.jobs());
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
            events.extend(self.step());
        }
        events
    }

    /// Reviews the workflow `name` as the listing shows it, without automatic approval.
    fn review(&mut self, name: &str) {
        let events = self.human(CommandKind::ListWorkflows);
        let hash = events
            .iter()
            .find_map(|event| match event {
                Event::WorkflowsListed { workflows, .. } => workflows
                    .iter()
                    .find(|w| w.name == name)
                    .map(|w| w.hash.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no workflow named {name}"));
        let events = self.human(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert_eq!(failure(&events), None, "review refused: {events:#?}");
    }

    /// Plays the engine thread until `done` says the events so far are enough, answering each
    /// approval a run asks for with `approve`. Returns the events, `done`'s last ones included.
    fn settle(
        &mut self,
        mut events: Vec<Event>,
        approve: bool,
        done: impl Fn(&[Event]) -> bool,
    ) -> Vec<Event> {
        let mut answered = 0;
        loop {
            events = self.pump_until(events, |events| done(events) || asks(events) > answered);
            if done(&events) {
                return events;
            }
            answered += 1;
            let run = events
                .iter()
                .rev()
                .find_map(|event| match event {
                    Event::WorkflowApprovalRequested { run, .. } => Some(*run),
                    _ => None,
                })
                .expect("an approval request");
            let answer = self.human(CommandKind::ApproveWorkflowStep {
                run,
                approved: approve,
            });
            events.extend(answer);
        }
    }

    /// Runs the workflow `name` by hand, answering each approval it asks with `approve`, until the
    /// run finishes. Returns the events, the run's end included.
    fn run(&mut self, name: &str, approve: bool) -> Vec<Event> {
        let events = self.human(CommandKind::RunWorkflow {
            name: name.to_owned(),
            inputs: BTreeMap::new(),
        });
        self.settle(events, approve, finished)
    }
}

fn agent() -> Actor {
    Actor::Agent {
        name: "agent".into(),
    }
}

/// An engine with a repository open, with the user's gate, reviewer and workflow directories.
struct Session {
    engine: Engine,
    next: u64,
}

impl Session {
    fn open(repo: &Path, user: &Path) -> Self {
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(user)));
        engine.set_gates_user_dir(Some(user.to_path_buf()));
        let mut session = Self { engine, next: 1 };
        let events = session.human(CommandKind::OpenRepo { path: s(repo) });
        assert_eq!(state(&events), OperationState::Done, "open: {events:?}");
        session
    }

    fn human(&mut self, kind: CommandKind) -> Vec<Event> {
        self.next += 1;
        self.engine.handle(
            OperationId(self.next),
            &Command {
                actor: Actor::Human,
                kind,
            },
        )
    }

    /// Plans the restack of `branch` and applies it, as the window does.
    fn restack(&mut self, branch: &str) -> Vec<Event> {
        let plan = self.plan(branch);
        self.human(CommandKind::ApplyRestack {
            branch: branch.into(),
            plan_id: plan.plan_id,
            skip: Vec::new(),
        })
    }

    fn plan(&mut self, branch: &str) -> RestackPlan {
        let events = self.human(CommandKind::PlanRestack {
            branch: branch.into(),
            skip: Vec::new(),
        });
        events
            .iter()
            .find_map(|event| match event {
                Event::RestackPlanned { plan, .. } => Some(plan.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no plan: {events:?}"))
    }
}

/// The state the operation ended in, and the detail of that event.
fn ended(events: &[Event]) -> (OperationState, Option<String>) {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, detail, .. } if state.is_terminal() => {
                Some((*state, detail.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no terminal event: {events:?}"))
}

fn state(events: &[Event]) -> OperationState {
    ended(events).0
}

/// The state and detail an operation ended with, when it has ended.
fn ended_of(events: &[Event], id: OperationId) -> Option<(OperationState, Option<String>)> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged {
            id: found,
            state,
            detail,
            ..
        } if *found == id && state.is_terminal() => Some((*state, detail.clone())),
        _ => None,
    })
}

/// The detail of the last failed operation, if any.
fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| match event {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

/// The blocked gates of a native check, as the event lists them: the operation, the branch, the
/// gate ids and the file problems.
fn blocked(events: &[Event]) -> (GateOp, String, Vec<String>, Vec<String>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::GatesBlocked {
                operation,
                branch,
                gates,
                errors,
                ..
            } => Some((
                *operation,
                branch.clone(),
                gates.iter().map(|gate| gate.id.clone()).collect(),
                errors.clone(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no GatesBlocked in {events:?}"))
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::WorkflowRunFinished { .. }))
}

fn asks(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::WorkflowApprovalRequested { .. }))
        .count()
}

/// How the workflow run ended, and why when it failed.
fn run_end(events: &[Event]) -> (WorkflowEnd, Option<String>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowRunFinished { end, reason, .. } => Some((*end, reason.clone())),
            _ => None,
        })
        .expect("the run finished")
}

#[test]
fn a_workflow_push_is_stopped_by_a_failing_human_approval_gate_until_the_branch_is_approved() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut harness = Harness::open(&fx.repo, &fx.user);
    write_flow_file(&fx.user, "push-feat", &flow("push-feat", PUSH_FEAT));
    harness.review("push-feat");

    let vetoed = harness.run("push-feat", true);
    let (end, reason) = run_end(&vetoed);
    assert_eq!(end, WorkflowEnd::Failed, "{vetoed:?}");
    assert!(
        reason
            .as_deref()
            .unwrap_or_default()
            .contains("Blocked by gate approved: "),
        "{reason:?}"
    );
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the remote moved"
    );

    let tip = tip(&fx.repo, "feat");
    let approved = harness.human(CommandKind::ApproveBranch {
        branch: "feat".into(),
        head: tip.clone(),
        note: None,
    });
    assert_eq!(state(&approved), OperationState::Done, "{approved:?}");
    let pushed = harness.run("push-feat", true);
    assert_eq!(run_end(&pushed).0, WorkflowEnd::Done, "{pushed:?}");
    assert_eq!(remote_tip(&fx.origin, "feat"), tip);
}

#[test]
fn a_workflow_push_is_stopped_by_a_findings_gate_until_the_finding_is_resolved() {
    let fx = push_fixture();
    save_security(&fx.repo);
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{NO_HIGH}]}}"#));
    let mut harness = Harness::open(&fx.repo, &fx.user);
    let recorded = harness.agent(security_with_finding());
    let comment = recorded
        .iter()
        .find_map(|event| match event {
            Event::VerdictRecorded { comment_ids, .. } => comment_ids.first().copied(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no finding comment in {recorded:?}"));
    write_flow_file(&fx.user, "push-feat", &flow("push-feat", PUSH_FEAT));
    harness.review("push-feat");

    let vetoed = harness.run("push-feat", true);
    let (end, reason) = run_end(&vetoed);
    assert_eq!(end, WorkflowEnd::Failed, "{vetoed:?}");
    assert!(
        reason
            .as_deref()
            .unwrap_or_default()
            .contains("Blocked by gate no-high: "),
        "{reason:?}"
    );
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the remote moved"
    );

    let resolved = harness.human(CommandKind::ResolveReviewComment { id: comment });
    assert_eq!(state(&resolved), OperationState::Done, "{resolved:?}");
    let pushed = harness.run("push-feat", true);
    assert_eq!(run_end(&pushed).0, WorkflowEnd::Done, "{pushed:?}");
    assert_eq!(remote_tip(&fx.origin, "feat"), tip(&fx.repo, "feat"));
}

#[test]
fn a_workflow_restack_is_stopped_by_a_gate_on_the_child_and_moves_nothing_until_it_passes() {
    let chain = chain_fixture();
    write_repo_gates(&chain.repo, &format!(r#"{{"gates":[{APPROVED_B}]}}"#));
    let mut harness = Harness::open(&chain.repo, &chain.user);
    let before = [
        tip(&chain.repo, "a"),
        tip(&chain.repo, "b"),
        tip(&chain.repo, "c"),
    ];
    write_flow_file(&chain.user, "restack-a", &flow("restack-a", RESTACK_A));
    harness.review("restack-a");

    let vetoed = harness.run("restack-a", true);
    let (end, reason) = run_end(&vetoed);
    assert_eq!(end, WorkflowEnd::Failed, "{vetoed:?}");
    // The gate fails on `b`, a child of the root: the reason names it, since three branches were
    // judged (R2h).
    assert!(
        reason
            .as_deref()
            .unwrap_or_default()
            .contains("b: Blocked by gate b-approved: "),
        "{reason:?}"
    );
    let unmoved = [
        tip(&chain.repo, "a"),
        tip(&chain.repo, "b"),
        tip(&chain.repo, "c"),
    ];
    assert_eq!(unmoved, before, "a branch moved");

    let approved = harness.human(CommandKind::ApproveBranch {
        branch: "b".into(),
        head: tip(&chain.repo, "b"),
        note: None,
    });
    assert_eq!(state(&approved), OperationState::Done, "{approved:?}");
    let restacked = harness.run("restack-a", true);
    assert_eq!(run_end(&restacked).0, WorkflowEnd::Done, "{restacked:?}");
    assert_ne!(tip(&chain.repo, "a"), before[0], "a did not move");
}

#[test]
fn the_native_restack_of_the_stack_is_vetoed_by_the_child_gate_and_a_bypass_naming_it_runs_it() {
    let chain = chain_fixture();
    write_repo_gates(&chain.repo, &format!(r#"{{"gates":[{APPROVED_B}]}}"#));
    let mut session = Session::open(&chain.repo, &chain.user);
    let before = tip(&chain.repo, "a");

    let vetoed = session.restack("a");
    assert_eq!(state(&vetoed), OperationState::Vetoed, "{vetoed:?}");
    let (operation, branch, gates, errors) = blocked(&vetoed);
    assert_eq!(operation, GateOp::Restack);
    assert_eq!(branch, "a", "the event names the root");
    assert_eq!(gates, ["b-approved"]);
    assert!(errors.is_empty());
    let (_, detail) = ended(&vetoed);
    assert!(
        detail
            .unwrap_or_default()
            .starts_with("b: Blocked by gate b-approved: "),
        "the veto names the child"
    );
    assert_eq!(tip(&chain.repo, "a"), before, "a moved");

    let plan = session.plan("a");
    let bypassed = session.human(CommandKind::BypassGates {
        gates: vec!["b-approved".into()],
        reason: "the stack is reviewed elsewhere".into(),
        then: Box::new(CommandKind::ApplyRestack {
            branch: "a".into(),
            plan_id: plan.plan_id,
            skip: Vec::new(),
        }),
    });
    assert_eq!(state(&bypassed), OperationState::Done, "{bypassed:?}");
    assert_ne!(tip(&chain.repo, "a"), before, "a did not move");
}

#[test]
fn a_broken_gates_file_makes_a_workflow_push_fail_closed() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, "{ not json");
    let mut harness = Harness::open(&fx.repo, &fx.user);
    write_flow_file(&fx.user, "push-feat", &flow("push-feat", PUSH_FEAT));
    harness.review("push-feat");

    let failed = harness.run("push-feat", true);
    let (end, reason) = run_end(&failed);
    assert_eq!(end, WorkflowEnd::Failed, "{failed:?}");
    assert!(
        reason
            .as_deref()
            .unwrap_or_default()
            .contains("Gate file problem:"),
        "{reason:?}"
    );
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the remote moved"
    );
}

#[test]
fn a_push_whose_branch_moves_while_its_gates_run_is_vetoed_and_pushes_nothing() {
    let fx = push_fixture();
    write_repo_gates(&fx.repo, &format!(r#"{{"gates":[{APPROVED_PUSH}]}}"#));
    let mut harness = Harness::open(&fx.repo, &fx.user);
    // The gate of the push commits what is staged on the branch, so the branch moves after the
    // native check judged its tip (R2h).
    write_flow_file(
        &fx.user,
        "commit-on-push",
        &phase(
            "commit-on-push",
            "before.push",
            "  ctx.stage.all()\n  ctx.commit.create({ message = \"moved by a gate\" })",
        ),
    );
    harness.review("commit-on-push");
    std::fs::write(fx.repo.join("g.txt"), "g\n").unwrap();
    let judged = tip(&fx.repo, "feat");
    let approved = harness.human(CommandKind::ApproveBranch {
        branch: "feat".into(),
        head: judged.clone(),
        note: None,
    });
    assert_eq!(state(&approved), OperationState::Done, "{approved:?}");

    // The push waits for its gate run, which asks for approval of each write and then reports
    // through the engine thread (`WF3c`). The push is the operation after the last one sent.
    let push = OperationId(harness.last + 1);
    let events = harness.human(CommandKind::PushWithLease);
    let events = harness.settle(events, true, |events| ended_of(events, push).is_some());
    let (end_state, detail) = ended_of(&events, push).expect("the push ended");
    assert_eq!(end_state, OperationState::Vetoed, "{events:?}");
    let moved = tip(&fx.repo, "feat");
    assert_ne!(moved, judged, "the gate did not commit");
    assert_eq!(
        detail.unwrap_or_default(),
        format!(
            "the branch moved while the gates ran: {} is now {}; run it again",
            &judged[..7],
            &moved[..7]
        )
    );
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the remote moved"
    );
}

fn write_flow_file(user: &Path, name: &str, text: &str) {
    std::fs::write(user.join("flows").join(format!("{name}.lua")), text).unwrap();
}
