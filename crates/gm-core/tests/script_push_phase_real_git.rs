//! The `push` phase (D157, D212 (3)) and `ctx.verdicts` (D212 (5)) against real repositories in
//! temporary directories. The remote is a local bare repository, so nothing needs the network. The
//! harness plays the engine thread: it carries a run's effects and events, and runs the Git jobs of
//! a push, as the runner does.

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
    Actor, Command, CommandKind, Event, FindingInput, OperationId, OperationState, ReviewSeverity,
    Reviewer, ReviewerDirection, ReviewerSignal, ReviewerSource, ReviewerSubject, WorkflowDryRun,
    WorkflowEnd,
};
use serde_json::json;

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

/// `origin` (bare, with `main`), and `repo` on `feat` with one commit that is not pushed. `f.txt`
/// reads `one`, `three` on `feat`, so line 2 is `three`.
struct Fixture {
    _dir: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    user: PathBuf,
}

fn fixture() -> Fixture {
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
    Fixture {
        _dir: dir,
        origin,
        repo,
        user,
    }
}

fn commit(dir: &Path, text: &str, message: &str) {
    std::fs::write(dir.join("f.txt"), text).unwrap();
    git(dir, &["commit", "-q", "-am", message]);
}

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

fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"])
}

/// A workflow `name` for `on`, optionally with a `match` rule, whose body runs in `execute`.
fn gate_source(name: &str, on: &str, filter: Option<&str>, body: &str) -> String {
    let rule = filter
        .map(|f| format!(", match = \"{f}\""))
        .unwrap_or_default();
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\"{rule} }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn write_workflow(flows: &Path, name: &str, source: &str) {
    std::fs::write(flows.join(format!("{name}.lua")), source).unwrap();
}

fn write_repo_gates(repo: &Path, text: &str) {
    let dir = repo.join(".git").join("gitmaster");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("gates.json"), text).unwrap();
}

const APPROVED_PUSH: &str =
    r#"{"gates":[{"id":"approved","kind":"human_approval","on":["push"]}]}"#;

fn reviewer(name: &str) -> Reviewer {
    Reviewer {
        name: name.to_owned(),
        description: format!("The {name} reviewer."),
        instructions: format!("Judge the change for {name}."),
        signal: ReviewerSignal::Level,
        direction: ReviewerDirection::LowerIsBetter,
        subject: ReviewerSubject::Branch,
        model: None,
        source: ReviewerSource::Repository,
        paths: Vec::new(),
        branches: Vec::new(),
        members: Vec::new(),
    }
}

fn save_reviewer(repo: &Path, name: &str) {
    reviewers::save(&reviewers::repo_dir(&repo.join(".git")), &reviewer(name)).unwrap();
}

fn finding(severity: ReviewSeverity, start: u32, body: &str) -> FindingInput {
    FindingInput {
        severity,
        path: "f.txt".to_owned(),
        start_line: start,
        end_line: start,
        body: body.to_owned(),
        suggested_fix: None,
    }
}

/// The agent records `value` of `reviewer` on `feat` at its tip, with `findings`.
fn record(reviewer: &str, value: &str, findings: Vec<FindingInput>) -> CommandKind {
    CommandKind::RecordVerdict {
        reviewer: reviewer.to_owned(),
        branch: "feat".to_owned(),
        head: "HEAD".to_owned(),
        value: value.to_owned(),
        rationale: "Checked the change.".to_owned(),
        findings,
    }
}

fn agent() -> Actor {
    Actor::Agent {
        name: "agent".into(),
    }
}

enum Message {
    Effect(EffectRequest),
    Event(ScriptEvent),
}

struct Harness {
    engine: Engine,
    messages: Receiver<Message>,
    last: u64,
}

impl Harness {
    /// Opens `fx.repo` with the workflows and the gates of `fx.user`, and the switch the app turns
    /// on for triggers.
    fn open(fx: &Fixture) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(fx.user.join("flows")));
        engine.set_gates_user_dir(Some(fx.user.clone()));
        engine.set_reviewer_user_dir(Some(reviewers::user_dir(&fx.user)));
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
        let (id, events) = harness.human(CommandKind::OpenRepo { path: s(&fx.repo) });
        assert_eq!(
            state_of(&events, id),
            Some(OperationState::Done),
            "{events:?}"
        );
        harness
    }

    /// Runs a command as `actor` and returns its id with the events, the Git jobs it started
    /// included.
    fn handle(&mut self, actor: Actor, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let mut events = self.engine.handle(id, &Command { actor, kind });
        events.extend(self.jobs());
        (id, events)
    }

    fn human(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.handle(Actor::Human, kind)
    }

    fn agent(&mut self, kind: CommandKind) -> Vec<Event> {
        self.handle(agent(), kind).1
    }

    /// Runs the Git jobs the engine queued, and their follow-ups, until none is left (the runner's
    /// part). A push runs here once its gates passed.
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

    /// Plays the engine thread until operation `id` ends.
    fn finish(&mut self, id: OperationId, events: Vec<Event>) -> Vec<Event> {
        self.pump_until(events, move |events| {
            state_of(events, id).is_some_and(OperationState::is_terminal)
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

    fn step(&mut self, wait: Duration) -> Vec<Event> {
        let mut events = match self.messages.recv_timeout(wait) {
            Ok(Message::Effect(request)) => self.engine.script_effect(request),
            Ok(Message::Event(event)) => self.engine.script_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        };
        events.extend(self.jobs());
        events
    }

    /// Reviews the workflow `name` as the listing shows it.
    fn review(&mut self, name: &str) {
        let (_, events) = self.human(CommandKind::ListWorkflows);
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
        let (_, events) = self.human(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
    }
}

/// The state the operation `id` ended in, if it ended.
fn state_of(events: &[Event], id: OperationId) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged {
            id: found, state, ..
        } if *found == id && state.is_terminal() => Some(*state),
        _ => None,
    })
}

/// The detail of the last terminal event of any operation.
fn detail(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, detail, .. } if state.is_terminal() => detail.clone(),
        _ => None,
    })
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

fn ends(events: &[Event]) -> Vec<WorkflowEnd> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkflowRunFinished { end, .. } => Some(*end),
            _ => None,
        })
        .collect()
}

fn dry_run_of(events: &[Event]) -> WorkflowDryRun {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowDryRun { dry_run, .. } => Some((**dry_run).clone()),
            _ => None,
        })
        .expect("a dry run")
}

const VERDICT_GATE: &str = "for _, verdict in ipairs(ctx.verdicts({ reviewer = \"security\" })) do\n\
    if not verdict.stale then\n\
    for _, finding in ipairs(verdict.findings) do\n\
    if finding.severity == \"high\" then\n\
    ctx.block(\"security: \" .. finding.path .. \":\" .. tostring(finding.start_line) .. \" is high\")\n\
    end\nend\nend\nend";

#[test]
fn a_blocking_before_push_gate_stops_the_push_and_the_remote_does_not_move() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "freeze",
        &gate_source("freeze", "before.push", None, "ctx.block(\"no\")"),
    );
    let mut h = Harness::open(&fx);
    h.review("freeze");

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Vetoed),
        "{events:?}"
    );
    assert_eq!(
        detail(&events).as_deref(),
        Some("freeze blocked the push: no")
    );
    assert_eq!(gate_names(&events), ["freeze"]);
    assert!(
        remote_tip(&fx.origin, "feat").is_empty(),
        "the push went out"
    );
}

#[test]
fn a_before_push_gate_that_passes_lets_the_push_run_and_sees_the_branch() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "pass",
        &gate_source(
            "pass",
            "before.push",
            None,
            "ctx.notify(\"gate \" .. input.trigger.branch)",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("pass");

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert_eq!(notices(&events), ["gate feat"]);
    assert_eq!(remote_tip(&fx.origin, "feat"), head(&fx.repo));
}

#[test]
fn an_after_push_observer_runs_after_a_successful_push_and_sees_the_pushed_oid() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "seen",
        &gate_source(
            "seen",
            "after.push",
            None,
            "ctx.notify(\"after \" .. input.trigger.branch .. \" \" .. input.trigger.oid)",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("seen");

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.pump_until(events, |events| {
        state_of(events, id).is_some() && !ends(events).is_empty()
    });

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert!(gate_names(&events).is_empty(), "an observer is not a gate");
    assert_eq!(notices(&events), [format!("after feat {}", head(&fx.repo))]);
}

#[test]
fn an_after_push_observer_does_not_run_when_the_push_fails() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "seen",
        &gate_source("seen", "after.push", None, "ctx.notify(\"after\")"),
    );
    let mut h = Harness::open(&fx);
    h.review("seen");
    // The remote is gone, so the push fails.
    git(
        &fx.repo,
        &["remote", "set-url", "origin", "/no/such/remote.git"],
    );

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Failed),
        "{events:?}"
    );
    assert!(ends(&events).is_empty(), "no observer ran: {events:?}");
}

#[test]
fn a_before_push_gate_for_another_branch_does_not_run_for_feat() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "other",
        &gate_source(
            "other",
            "before.push",
            Some("branch:other/*"),
            "ctx.block(\"no\")",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("other");

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert!(gate_names(&events).is_empty(), "{events:?}");
    assert_eq!(remote_tip(&fx.origin, "feat"), head(&fx.repo));
}

#[test]
fn an_unreviewed_before_push_gate_vetoes_the_push_with_the_review_text() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "freeze",
        &gate_source("freeze", "before.push", None, "ctx.block(\"no\")"),
    );
    let mut h = Harness::open(&fx);

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Vetoed),
        "{events:?}"
    );
    assert_eq!(
        detail(&events).as_deref(),
        Some("freeze is not reviewed: review it, then try again.")
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::WorkflowGateUnreviewed { workflow, .. } if workflow == "freeze"
        )),
        "{events:?}"
    );
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_failing_native_gate_vetoes_before_any_workflow_gate_runs() {
    let fx = fixture();
    write_repo_gates(&fx.repo, APPROVED_PUSH);
    write_workflow(
        &fx.user.join("flows"),
        "pass",
        &gate_source("pass", "before.push", None, "ctx.notify(\"ran\")"),
    );
    let mut h = Harness::open(&fx);
    h.review("pass");

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Vetoed),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::GatesBlocked { .. })),
        "{events:?}"
    );
    assert!(
        gate_names(&events).is_empty(),
        "a workflow gate ran: {events:?}"
    );
    assert!(notices(&events).is_empty());
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_gate_reads_the_recorded_verdict_and_blocks_a_push_on_a_high_finding() {
    let fx = fixture();
    save_reviewer(&fx.repo, "security");
    write_workflow(
        &fx.user.join("flows"),
        "verdict-gate",
        &gate_source("verdict-gate", "before.push", None, VERDICT_GATE),
    );
    let mut h = Harness::open(&fx);
    h.review("verdict-gate");
    let recorded = h.agent(record(
        "security",
        "high",
        vec![finding(ReviewSeverity::High, 2, "Unchecked input.")],
    ));
    assert_eq!(
        state_of(&recorded, OperationId(h.last)),
        Some(OperationState::Done),
        "{recorded:?}"
    );

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Vetoed),
        "{events:?}"
    );
    assert_eq!(
        detail(&events).as_deref(),
        Some("verdict-gate blocked the push: security: f.txt:2 is high")
    );
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_verdict_is_fresh_at_its_tip_and_stale_after_a_commit_and_its_line_moves() {
    let fx = fixture();
    save_reviewer(&fx.repo, "security");
    write_workflow(
        &fx.user.join("flows"),
        "show",
        &gate_source(
            "show",
            "before.push",
            None,
            "for _, v in ipairs(ctx.verdicts()) do\n\
             for _, f in ipairs(v.findings) do\n\
             ctx.notify(\"stale=\" .. tostring(v.stale) .. \" line=\" .. tostring(f.start_line) .. \
             \" sev=\" .. f.severity .. \" status=\" .. f.status .. \" path=\" .. f.path)\n\
             end\nend",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("show");
    h.agent(record(
        "security",
        "high",
        vec![finding(ReviewSeverity::High, 2, "Unchecked input.")],
    ));

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);
    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert_eq!(
        notices(&events),
        ["stale=false line=2 sev=high status=open path=f.txt"]
    );

    // A line above `three` moves it to line 3; the verdict is for the old tip.
    std::fs::write(fx.repo.join("f.txt"), "new\none\nthree\n").unwrap();
    git(&fx.repo, &["commit", "-q", "-am", "add a line"]);
    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);
    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert_eq!(
        notices(&events),
        ["stale=true line=3 sev=high status=open path=f.txt"]
    );
}

#[test]
fn ctx_verdicts_filters_by_reviewer_and_lists_newest_first() {
    let fx = fixture();
    save_reviewer(&fx.repo, "security");
    save_reviewer(&fx.repo, "style");
    write_workflow(
        &fx.user.join("flows"),
        "count",
        &gate_source(
            "count",
            "before.push",
            None,
            "local style = ctx.verdicts({ reviewer = \"style\" })\n\
             ctx.notify(\"all=\" .. #ctx.verdicts() .. \" style=\" .. #style .. \" first=\" .. style[1].reviewer)",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("count");
    h.agent(record("security", "high", Vec::new()));
    h.agent(record("style", "low", Vec::new()));

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert_eq!(notices(&events), ["all=2 style=1 first=style"]);
}

#[test]
fn a_dry_run_of_a_script_that_reads_verdicts_gets_an_empty_list() {
    let fx = fixture();
    let mut h = Harness::open(&fx);
    let source = "local M = {}\nM.meta = { name = \"count\", trigger = { on = \"manual\" } }\n\
                  function M.execute(ctx, input)\n\
                  local found = ctx.verdicts({ reviewer = \"security\" })\n\
                  return tostring(#found)\nend\nreturn M\n"
        .to_owned();

    let (_, events) = h.human(CommandKind::DryRunWorkflow {
        source,
        inputs: json!({}),
    });
    let dry = dry_run_of(&events);

    assert_eq!(dry.error, None, "{events:?}");
    assert_eq!(dry.calls[0].func, "verdicts");
    assert_eq!(dry.calls[0].level, "read");
    assert_eq!(dry.stop, "returned: \"0\"");
}
