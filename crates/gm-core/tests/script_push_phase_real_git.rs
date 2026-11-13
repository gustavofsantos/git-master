//! The `push` phase (D157) and `ctx.scorecards` (D219) against real repositories in
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
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{
    Actor, Command, CommandKind, CommentLabel, Event, OperationId, OperationState, ScorecardStatus,
    ScorecardSummary, WorkflowDryRun, WorkflowEnd,
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
    let rule = filter.map_or_default(|f| format!(", match = \"{f}\""));
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"{on}\"{rule} }} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn write_workflow(flows: &Path, name: &str, source: &str) {
    std::fs::write(flows.join(format!("{name}.lua")), source).unwrap();
}

/// The agent publishes a scorecard `name` on `feat` at its tip, with `status` and no comments.
fn publish(name: &str, status: ScorecardStatus) -> CommandKind {
    CommandKind::PublishScorecard {
        name: name.to_owned(),
        branch: "feat".to_owned(),
        head: "HEAD".to_owned(),
        status,
        summary: ScorecardSummary {
            label: CommentLabel::Issue,
            decorations: Vec::new(),
            subject: "Unchecked input".to_owned(),
            body: "Checked the change.".to_owned(),
        },
        comments: Vec::new(),
        member: None,
        workflow_run: None,
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
    /// Opens `fx.repo` with the workflows of `fx.user`, and the switch the app
    /// turns on for triggers.
    fn open(fx: &Fixture) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(fx.user.join("flows")));
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
    /// part). A push runs here once its `before.push` workflows passed.
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

/// The gate of a review: the scorecard of the pushed head must pass and have no open blocking
/// comment, or the push stops.
const SCORECARD_GATE: &str = r#"for _, row in ipairs(ctx.scorecards({ name = "security", branch = input.repo.branch })) do
    if row.head == input.repo.head and not row.stale then
        if row.status ~= "pass" then return ctx.block("security: " .. row.status) end
        if row.blocking_open > 0 then return ctx.block(row.blocking_open .. " blocking comments are open") end
        return
    end
end
return ctx.block("no security scorecard for this commit")"#;

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
fn a_before_push_gate_reads_a_fail_scorecard_and_stops_the_push() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "scorecard-gate",
        &gate_source("scorecard-gate", "before.push", None, SCORECARD_GATE),
    );
    let mut h = Harness::open(&fx);
    h.review("scorecard-gate");
    let published = h.agent(publish("security", ScorecardStatus::Fail));
    assert_eq!(
        state_of(&published, OperationId(h.last)),
        Some(OperationState::Done),
        "{published:?}"
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
        Some("scorecard-gate blocked the push: security: fail")
    );
    assert!(remote_tip(&fx.origin, "feat").is_empty());
}

#[test]
fn a_passing_scorecard_for_the_pushed_head_lets_the_push_through() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "scorecard-gate",
        &gate_source("scorecard-gate", "before.push", None, SCORECARD_GATE),
    );
    let mut h = Harness::open(&fx);
    h.review("scorecard-gate");
    h.agent(publish("security", ScorecardStatus::Pass));

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);

    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert!(
        !remote_tip(&fx.origin, "feat").is_empty(),
        "the push went out"
    );
}

#[test]
fn a_scorecard_is_fresh_at_its_tip_and_stale_after_a_commit_for_ctx_scorecards() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "show",
        &gate_source(
            "show",
            "before.push",
            None,
            "for _, row in ipairs(ctx.scorecards()) do\n\
             ctx.notify(\"stale=\" .. tostring(row.stale) .. \" status=\" .. row.status .. \
             \" blocking=\" .. row.blocking_open .. \" label=\" .. row.label)\n\
             end",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("show");
    h.agent(publish("security", ScorecardStatus::NeedsWork));

    let (id, events) = h.human(CommandKind::PushWithLease);
    let events = h.finish(id, events);
    assert_eq!(
        state_of(&events, id),
        Some(OperationState::Done),
        "{events:?}"
    );
    assert_eq!(
        notices(&events),
        ["stale=false status=needs_work blocking=0 label=issue"]
    );

    // A commit moves the branch; the scorecard stays as it was, and is stale now.
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
        ["stale=true status=needs_work blocking=0 label=issue"]
    );
}

#[test]
fn ctx_scorecards_filters_by_name_and_lists_newest_first() {
    let fx = fixture();
    write_workflow(
        &fx.user.join("flows"),
        "count",
        &gate_source(
            "count",
            "before.push",
            None,
            "local style = ctx.scorecards({ name = \"style\" })\n\
             ctx.notify(\"all=\" .. #ctx.scorecards() .. \" style=\" .. #style .. \" first=\" .. style[1].name)",
        ),
    );
    let mut h = Harness::open(&fx);
    h.review("count");
    h.agent(publish("security", ScorecardStatus::Fail));
    h.agent(publish("style", ScorecardStatus::Pass));

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
fn a_dry_run_of_a_script_that_reads_scorecards_gets_an_empty_list() {
    let fx = fixture();
    let mut h = Harness::open(&fx);
    let source = "local M = {}\nM.meta = { name = \"count\", trigger = { on = \"manual\" } }\n\
                  function M.execute(ctx, input)\n\
                  local found = ctx.scorecards({ name = \"security\" })\n\
                  return tostring(#found)\nend\nreturn M\n"
        .to_owned();

    let (_, events) = h.human(CommandKind::DryRunWorkflow {
        source,
        inputs: json!({}),
    });
    let dry = dry_run_of(&events);

    assert_eq!(dry.error, None, "{events:?}");
    assert_eq!(dry.calls[0].func, "scorecards");
    assert_eq!(dry.calls[0].level, "read");
    assert_eq!(dry.stop, "returned: \"0\"");
}
