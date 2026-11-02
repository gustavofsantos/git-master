//! Lua workflows through the engine's commands (design/workflow-scripts.md, D98, D101): the
//! listing, the review of a version, a manual run with its approvals and its cancel, and the
//! templates. Real repositories in temporary directories; this harness plays the engine thread
//! that carries a run's effects and events, as the runner does.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command as Process;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gm_core::git::SubprocessGit;
use gm_core::script::journal::{Journal, NewRun};
use gm_core::script::run::ScriptEvent;
use gm_core::script::trust::TrustStore;
use gm_core::{EffectRequest, Engine, ScriptLink};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowDiagnostic,
    WorkflowDryRun, WorkflowEnd, WorkflowInfo, WorkflowRunDetail, WorkflowRunStatus,
    WorkflowRunSummary,
};
use serde_json::json;
use sha2::{Digest, Sha256};

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

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
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
    fn open(repo: &Path, flows: &Path) -> Self {
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
        let mut harness = Self {
            engine,
            messages,
            last: 0,
        };
        harness.send_as(
            Actor::Human,
            CommandKind::OpenRepo {
                path: repo.display().to_string(),
            },
        );
        harness
    }

    fn send(&mut self, kind: CommandKind) -> Vec<Event> {
        self.send_as(Actor::Human, kind)
    }

    fn send_as(&mut self, actor: Actor, kind: CommandKind) -> Vec<Event> {
        self.last += 1;
        self.engine
            .handle(OperationId(self.last), &Command { actor, kind })
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
            match self.messages.recv_timeout(Duration::from_millis(20)) {
                Ok(Message::Effect(request)) => events.extend(self.engine.script_effect(request)),
                Ok(Message::Event(event)) => events.extend(self.engine.script_event(event)),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
        }
        events
    }

    /// Runs the workflow and plays the run to its end, approving every request when `approve`.
    fn run_to_end(
        &mut self,
        name: &str,
        inputs: &[(&str, &str)],
        approve: Option<bool>,
    ) -> Vec<Event> {
        let started = self.send(run(name, inputs));
        let mut events = started;
        let mut answered = 0;
        loop {
            events = self.pump_until(events, |events| finished(events) || asks(events) > answered);
            if finished(&events) {
                self.engine.script_runtime().unwrap().join_all();
                return events;
            }
            let run = events
                .iter()
                .find_map(|e| match e {
                    Event::WorkflowApprovalRequested { run, .. } => Some(*run),
                    _ => None,
                })
                .unwrap();
            answered += 1;
            match approve {
                Some(approved) => {
                    let answer = self.send(CommandKind::ApproveWorkflowStep { run, approved });
                    events.extend(answer);
                }
                None => return events,
            }
        }
    }

    /// Reviews the script `name` as the listing shows it, then runs it.
    fn review(&mut self, name: &str) {
        let hash = self
            .listing()
            .iter()
            .find(|w| w.name == name)
            .unwrap()
            .hash
            .clone();
        let events = self.send(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert_eq!(failure(&events), None);
    }

    fn listing(&mut self) -> Vec<WorkflowInfo> {
        listed(&self.send(CommandKind::ListWorkflows)).0
    }
}

fn run(name: &str, inputs: &[(&str, &str)]) -> CommandKind {
    CommandKind::RunWorkflow {
        name: name.to_owned(),
        inputs: inputs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
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

fn end_of(events: &[Event]) -> (WorkflowEnd, Option<String>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunFinished { end, reason, .. } => Some((*end, reason.clone())),
            _ => None,
        })
        .expect("the run finished")
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

/// The listing's workflows the person wrote, and what was not read. The built-in stack workflows
/// (`WF4b`) are listed too; these tests are about the files in the user's folder, so they are left out.
fn listed(events: &[Event]) -> (Vec<WorkflowInfo>, Vec<String>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowsListed {
                workflows, ignored, ..
            } => Some((
                workflows.iter().filter(|w| !w.builtin()).cloned().collect(),
                ignored.clone(),
            )),
            _ => None,
        })
        .expect("a listing")
}

const BRANCH: &str = r#"
local M = {}
M.meta = { name = "make-branch", description = "Make a branch", trigger = { on = "manual" },
  inputs = { name = { type = "string", prompt = "Branch name" } } }
function M.execute(ctx, input)
  ctx.branch.create({ name = input.inputs.name, checkout = true })
end
return M
"#;

const COMMIT_ALL: &str = r#"
local M = {}
M.meta = { name = "commit-all", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.stage.all()
  ctx.commit.create({ message = "wf: commit" })
end
return M
"#;

const SUGGEST: &str = r#"
local M = {}
M.meta = { name = "suggest", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.exec({ "printf", "feat: from workflow" })
  ctx.commit.suggest_message({ message = r.text })
end
return M
"#;

const SLOW: &str = r#"
local M = {}
M.meta = { name = "slow", trigger = { on = "manual" } }
function M.execute(ctx, input)
  ctx.exec({ "sleep", "30" }, { timeout = "1m" })
end
return M
"#;

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

#[test]
fn the_listing_shows_scripts_their_meta_and_what_was_not_read() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    write(flows.path(), "old.toml", "schema = 1\nname = \"old\"\n");
    write(
        flows.path(),
        "broken.lua",
        "local M = {}\nM.meta = {\nreturn M\n",
    );
    let mut harness = Harness::open(repo.path(), flows.path());

    let (workflows, ignored) = listed(&harness.send(CommandKind::ListWorkflows));

    let names: Vec<&str> = workflows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        ["broken", "make-branch"],
        "sorted by file name; a broken file by its stem"
    );
    let good = &workflows[1];
    assert_eq!(good.trigger, "manual");
    assert_eq!(good.inputs.len(), 1);
    assert_eq!(good.inputs[0].name, "name");
    assert_eq!(good.inputs[0].prompt.as_deref(), Some("Branch name"));
    assert_eq!(good.hash, sha256(BRANCH));
    assert_eq!(good.source, BRANCH);
    assert_eq!(good.error, None);
    assert!(
        workflows[0].error.as_deref().unwrap().starts_with("line "),
        "{:?}",
        workflows[0].error
    );
    assert!(
        ignored
            .iter()
            .any(|line| line.contains("old.toml") && line.contains("no longer supported")),
        "{ignored:?}"
    );
}

#[test]
fn the_api_reference_is_written_next_to_the_scripts_once() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.send(CommandKind::ListWorkflows);
    for file in ["API.md", "ctx.d.luau", "SKILL.md", "VERSION"] {
        assert!(flows.path().join(".api").join(file).exists(), "{file}");
    }
    // A repository is never written to.
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    assert!(!repo.path().join(".api").exists());
}

#[test]
fn a_directory_inside_the_repository_is_not_read() {
    let repo = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let inside = repo.path().join("flows");
    std::fs::create_dir(&inside).unwrap();
    write(&inside, "inside.lua", BRANCH);
    write(
        outside.path(),
        "outside.lua",
        &BRANCH.replace("make-branch", "outside"),
    );
    git(
        repo.path(),
        &[
            "config",
            "--add",
            "gitmaster.workflowPath",
            &inside.display().to_string(),
        ],
    );
    let mut harness = Harness::open(repo.path(), outside.path());

    let (workflows, ignored) = listed(&harness.send(CommandKind::ListWorkflows));

    assert_eq!(
        workflows
            .iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>(),
        ["outside"]
    );
    assert!(
        ignored
            .iter()
            .any(|line| line.contains("inside the repository")),
        "{ignored:?}"
    );
}

#[test]
fn an_unreviewed_script_asks_for_a_review_and_does_not_start() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());

    let events = harness.send(run("make-branch", &[("name", "feat/x")]));

    let request = events.iter().find_map(|e| match e {
        Event::WorkflowReviewRequested {
            name,
            hash,
            source,
            inputs,
            ..
        } => Some((name.clone(), hash.clone(), source.clone(), inputs.clone())),
        _ => None,
    });
    let (name, hash, source, inputs) = request.expect("a review request");
    assert_eq!(
        (name.as_str(), hash.as_str(), source.as_str()),
        ("make-branch", sha256(BRANCH).as_str(), BRANCH)
    );
    assert_eq!(
        inputs.get("name").map(String::as_str),
        Some("feat/x"),
        "the run can be sent again"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WorkflowRunChanged { .. }))
    );
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/x"]), "");
}

#[test]
fn a_reviewed_version_runs_and_an_edit_makes_it_unreviewed_again() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());

    harness.review("make-branch");
    let events = harness.run_to_end("make-branch", &[("name", "feat/x")], None);
    assert_eq!(end_of(&events), (WorkflowEnd::Done, None));
    assert_eq!(
        git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "feat/x"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkflowRunChanged { status: WorkflowRunStatus::Running, workflow, .. } if workflow == "make-branch"
    )));

    // One changed character is a new version.
    write(
        flows.path(),
        "make-branch.lua",
        &format!("{BRANCH}-- edited\n"),
    );
    let events = harness.send(run("make-branch", &[("name", "feat/y")]));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::WorkflowReviewRequested { .. }))
    );
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/y"]), "");
}

#[test]
fn a_review_of_a_version_that_changed_since_it_was_shown_is_refused() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    let shown = sha256(BRANCH);
    write(
        flows.path(),
        "make-branch.lua",
        &format!("{BRANCH}-- an agent edited it\n"),
    );

    let events = harness.send(CommandKind::ReviewWorkflow {
        name: "make-branch".into(),
        hash: shown,
        auto_approve: false,
    });

    assert!(
        failure(&events)
            .unwrap()
            .contains("changed after it was shown")
    );
    let events = harness.send(run("make-branch", &[("name", "feat/z")]));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::WorkflowReviewRequested { .. })),
        "still unreviewed"
    );
}

#[test]
fn inputs_are_checked_before_the_review_or_the_run() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("make-branch");

    let missing = harness.send(run("make-branch", &[]));
    assert!(failure(&missing).unwrap().contains("`name` is required"));
    let extra = harness.send(run("make-branch", &[("name", "x"), ("other", "y")]));
    assert!(failure(&extra).unwrap().contains("no input named `other`"));
    let unknown = harness.send(run("nope", &[]));
    assert!(
        failure(&unknown)
            .unwrap()
            .contains("no workflow named `nope`")
    );
}

#[test]
fn an_agent_cannot_run_review_approve_or_cancel() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    let agent = Actor::Agent {
        name: "claude".into(),
    };
    for kind in [
        run("make-branch", &[("name", "x")]),
        CommandKind::ReviewWorkflow {
            name: "make-branch".into(),
            hash: sha256(BRANCH),
            auto_approve: true,
        },
        CommandKind::ApproveWorkflowStep {
            run: 1,
            approved: true,
        },
        CommandKind::CancelWorkflowRun { run: 1 },
    ] {
        let events = harness.send_as(agent.clone(), kind.clone());
        assert!(failure(&events).is_some(), "{kind:?}");
    }
    assert_eq!(git(repo.path(), &["branch", "--list", "x"]), "");
}

#[test]
fn stage_and_commit_are_logged_under_the_operation_that_started_the_run() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    std::fs::write(repo.path().join("b.txt"), "two\n").unwrap();
    write(flows.path(), "commit-all.lua", COMMIT_ALL);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("commit-all");

    let before = harness.last;
    let events = harness.run_to_end("commit-all", &[], None);

    assert_eq!(end_of(&events).0, WorkflowEnd::Done);
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "wf: commit"
    );
    let started_by = OperationId(before + 1);
    let logged: Vec<&CommandKind> = harness
        .engine
        .log()
        .iter()
        .filter(|entry| entry.id == started_by)
        .map(|entry| &entry.kind)
        .collect();
    assert!(
        logged.iter().any(|k| matches!(k, CommandKind::StageAll)),
        "{logged:?}"
    );
    assert!(
        logged
            .iter()
            .any(|k| matches!(k, CommandKind::Commit { message } if message == "wf: commit")),
        "{logged:?}"
    );
}

#[test]
fn a_script_that_execs_and_then_applies_asks_and_a_rejection_changes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(
        flows.path(),
        "ask.lua",
        r#"
local M = {}
M.meta = { name = "ask", trigger = { on = "manual" } }
function M.execute(ctx, input)
  local r = ctx.exec({ "printf", "feat/asked" })
  ctx.branch.create({ name = r.text })
end
return M
"#,
    );
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("ask");

    let events = harness.run_to_end("ask", &[], Some(false));

    let summary = events.iter().find_map(|e| match e {
        Event::WorkflowApprovalRequested { summary, state, .. } => {
            Some((summary.clone(), state.clone()))
        }
        _ => None,
    });
    let (summary, state) = summary.expect("an approval request");
    assert_eq!(state, "branch.create");
    assert!(
        summary.contains("feat/asked"),
        "the dialog shows the resolved arguments: {summary}"
    );
    let (end, reason) = end_of(&events);
    assert_eq!(end, WorkflowEnd::Failed);
    assert!(reason.unwrap().contains("rejected"));
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/asked"]), "");

    let events = harness.run_to_end("ask", &[], Some(true));
    assert_eq!(end_of(&events).0, WorkflowEnd::Done);
    assert!(git(repo.path(), &["branch", "--list", "feat/asked"]).contains("feat/asked"));
}

#[test]
fn cancelling_a_run_kills_its_program_and_ends_it() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "slow.lua", SLOW);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("slow");
    let events = harness.send(run("slow", &[]));
    let events = harness.pump_until(events, |events| {
        events
            .iter()
            .any(|e| matches!(e, Event::WorkflowRunChanged { .. }))
    });
    let run_id = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunChanged { run, .. } => Some(*run),
            _ => None,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let started = Instant::now();
    let cancel = harness.send(CommandKind::CancelWorkflowRun { run: run_id });
    assert_eq!(failure(&cancel), None);
    let events = harness.pump_until(events, finished);

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkflowRunChanged {
            status: WorkflowRunStatus::Cancelled,
            ..
        }
    )));
    assert_eq!(
        end_of(&events),
        (WorkflowEnd::Cancelled, Some("cancelled".to_owned()))
    );
    let again = harness.send(CommandKind::CancelWorkflowRun { run: run_id });
    assert!(
        failure(&again).is_some(),
        "a run that ended cannot be cancelled"
    );
}

#[test]
fn a_suggested_message_reaches_the_window() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.lua", SUGGEST);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("suggest");

    let events = harness.run_to_end("suggest", &[], None);

    assert!(events.iter().any(|e| matches!(
        e,
        Event::WorkflowMessageSuggested { message, .. } if message == "feat: from workflow"
    )));
    assert_eq!(git(repo.path(), &["log", "--format=%s"]).trim(), "init");
}

#[test]
fn a_template_is_copied_as_lua_into_the_user_directory_and_never_overwritten() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let (_, _) = listed(&harness.send(CommandKind::ListWorkflows));
    let first = harness.send(CommandKind::CreateWorkflowFromTemplate {
        template: "commit-staged".into(),
    });
    let second = harness.send(CommandKind::CreateWorkflowFromTemplate {
        template: "commit-staged".into(),
    });

    let path_of = |events: &[Event]| {
        events.iter().find_map(|e| match e {
            Event::WorkflowCreated { path, name, .. } => Some((path.clone(), name.clone())),
            _ => None,
        })
    };
    let (first_path, first_name) = path_of(&first).unwrap();
    let (second_path, second_name) = path_of(&second).unwrap();
    assert!(first_path.ends_with("commit-staged.lua"), "{first_path}");
    assert!(
        second_path.ends_with("commit-staged-2.lua"),
        "{second_path}"
    );
    assert_eq!(
        (first_name.as_str(), second_name.as_str()),
        ("commit-staged", "commit-staged-2")
    );
    let names: Vec<String> = harness.listing().into_iter().map(|w| w.name).collect();
    // Files are listed by file name, and `-` sorts before `.`.
    assert_eq!(names, ["commit-staged-2", "commit-staged"]);
    let unknown = harness.send(CommandKind::CreateWorkflowFromTemplate {
        template: "nope".into(),
    });
    assert!(failure(&unknown).unwrap().contains("no template named"));
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn every_template_loads_without_a_problem() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let events = harness.send(CommandKind::ListWorkflows);
    let Some(templates) = events.iter().find_map(|e| match e {
        Event::WorkflowsListed { templates, .. } => Some(templates.clone()),
        _ => None,
    }) else {
        panic!("a listing");
    };
    assert!(templates.len() >= 4, "{templates:?}");
    for template in templates {
        harness.send(CommandKind::CreateWorkflowFromTemplate { template });
    }
    for workflow in harness.listing() {
        assert_eq!(workflow.error, None, "{}", workflow.name);
        assert_eq!(workflow.trigger, "manual", "{}", workflow.name);
    }
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

fn loaded(events: &[Event]) -> WorkflowRunDetail {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunLoaded { detail, .. } => Some((**detail).clone()),
            _ => None,
        })
        .expect("a loaded run")
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

/// The problems a check answered, in the order it found them.
fn checked_of(events: &[Event]) -> Vec<WorkflowDiagnostic> {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowChecked { diagnostics, .. } => Some(diagnostics.clone()),
            _ => None,
        })
        .expect("a check answer")
}

#[test]
fn a_check_of_a_good_script_finds_nothing_and_writes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let events = harness.send(CommandKind::CheckWorkflowScript {
        source: DRY.to_owned(),
    });

    assert!(checked_of(&events).is_empty());
    assert!(harness.listing().is_empty(), "a check saves nothing");
}

#[test]
fn a_check_of_a_syntax_error_points_at_its_line() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let events = harness.send(CommandKind::CheckWorkflowScript {
        source: "local M = {}\nM.meta = {\nreturn M\n".into(),
    });

    let problems = checked_of(&events);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].line >= 2, "{problems:?}");
    assert!(!problems[0].message.is_empty());
}

#[test]
fn a_check_of_a_bad_meta_points_at_the_field() {
    let flows = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let source = r#"local M = {}
M.meta = {
  name = "w",
  trigger = { on = "nope" },
}
function M.execute(ctx, input)
end
return M
"#;

    let problems = checked_of(&harness.send(CommandKind::CheckWorkflowScript {
        source: source.to_owned(),
    }));

    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].line, 4, "{problems:?}");
    assert!(problems[0].message.contains("unknown trigger `nope`"));
}

const DRY: &str = r#"
local M = {}
M.meta = { name = "dry", trigger = { on = "manual" },
  inputs = { name = { type = "string", prompt = "Branch name" } } }
function M.execute(ctx, input)
  ctx.exec({ "touch", "MARKER" })
  ctx.branch.create({ name = input.inputs.name, checkout = true })
  return "ok"
end
return M
"#;

#[test]
fn a_finished_run_is_listed_and_loads_with_its_script_input_and_calls() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("make-branch");
    let events = harness.run_to_end("make-branch", &[("name", "feat/x")], None);
    assert_eq!(end_of(&events).0, WorkflowEnd::Done);

    let listed = runs_listed(&harness.send(CommandKind::ListWorkflowRuns {
        workflow: "make-branch".into(),
        limit: 50,
    }));
    assert_eq!(listed.len(), 1);
    let summary = &listed[0];
    assert_eq!(summary.workflow, "make-branch");
    assert_eq!(summary.status, WorkflowRunStatus::Done);
    assert_eq!(summary.started_by, "human");
    assert!(summary.ended_at.is_some());

    let detail = loaded(&harness.send(CommandKind::LoadWorkflowRun { run: summary.run }));
    assert_eq!(detail.summary, *summary);
    assert_eq!(detail.definition, BRANCH);
    assert_eq!(detail.trigger, json!({ "on": "manual" }));
    assert_eq!(detail.input["inputs"], json!({ "name": "feat/x" }));
    assert!(detail.input["repo"].is_object(), "{:?}", detail.input);
    assert_eq!(detail.calls.len(), 1, "{:#?}", detail.calls);
    let call = &detail.calls[0];
    assert_eq!(call.seq, 1);
    assert_eq!(call.func, "branch.create");
    assert_eq!(call.level, "apply");
    assert_eq!(call.args, json!({ "name": "feat/x", "checkout": true }));
    assert_eq!(call.outcome.as_deref(), Some("done"));
    assert!(call.duration_ms.is_some());
}

#[test]
fn a_run_left_unfinished_by_a_previous_session_is_listed_as_interrupted() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let journal =
        Journal::open(&repo.path().join(".git").join("gitmaster").join("state.db")).unwrap();
    let run = journal
        .create_run(
            &NewRun {
                workflow: "make-branch",
                definition_hash: "h",
                definition_text: BRANCH,
                trigger: &json!({ "on": "manual" }),
                input: &json!({ "inputs": { "name": "x" } }),
                started_by: "human",
                seed: 1,
            },
            1,
        )
        .unwrap();
    drop(journal);
    let mut harness = Harness::open(repo.path(), flows.path());

    let listed = runs_listed(&harness.send(CommandKind::ListWorkflowRuns {
        workflow: "make-branch".into(),
        limit: 50,
    }));
    assert_eq!(listed[0].run, run);
    assert_eq!(listed[0].status, WorkflowRunStatus::Interrupted);
    assert_eq!(
        listed[0].end_reason.as_deref(),
        Some("the app stopped while the run was in progress")
    );
}

#[test]
fn a_dry_run_of_the_edited_text_lists_its_calls_and_performs_none() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let marker_dir = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let marker = marker_dir.path().join("ran");
    let source = DRY.replace("MARKER", &marker.display().to_string());
    let mut harness = Harness::open(repo.path(), flows.path());

    let events = harness.send(CommandKind::DryRunWorkflow {
        source,
        inputs: json!({}),
    });
    let dry = dry_run_of(&events);

    assert_eq!(dry.error, None);
    assert_eq!(dry.missing_inputs, ["name"]);
    assert_eq!(dry.stop, "returned: \"ok\"");
    let funcs: Vec<&str> = dry.calls.iter().map(|c| c.func.as_str()).collect();
    assert_eq!(funcs, ["exec", "branch.create"]);
    assert_eq!(dry.calls[0].level, "read");
    assert!(dry.calls[0].assumed && !dry.calls[0].executed);
    assert_eq!(
        dry.calls[0].args["argv"],
        json!(["touch", marker.display().to_string()])
    );
    assert_eq!(dry.calls[1].level, "apply");
    assert!(dry.calls[1].assumed);
    assert_eq!(
        dry.calls[1].args["name"],
        json!(""),
        "a missing input is empty"
    );
    assert!(!marker.exists(), "exec did not run");
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/*"]), "");
}

#[test]
fn a_dry_run_of_a_script_with_a_syntax_error_returns_the_error() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let events = harness.send(CommandKind::DryRunWorkflow {
        source: "local M = {}\nM.meta = {\nreturn M\n".into(),
        inputs: json!({}),
    });
    let dry = dry_run_of(&events);

    let error = dry.error.expect("the text does not load");
    assert!(error.starts_with("line "), "{error}");
    assert!(dry.calls.is_empty());
    assert_eq!(dry.stop, "");
}

#[test]
fn saving_creates_updates_and_refuses_a_stale_hash_and_a_file_outside_the_folders() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let path = flows.path().join("new-flow.lua").display().to_string();

    // A new file passes no hash.
    let events = harness.send(CommandKind::SaveWorkflowFile {
        path: path.clone(),
        source: BRANCH.to_owned(),
        expected_hash: None,
    });
    assert_eq!(failure(&events), None);
    let saved_hash = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowFileSaved { hash, .. } => Some(hash.clone()),
            _ => None,
        })
        .expect("saved");
    assert_eq!(saved_hash, sha256(BRANCH));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BRANCH);
    let listing = harness.listing();
    assert!(
        listing
            .iter()
            .any(|w| w.path == path && w.hash == saved_hash)
    );

    // A creation over an existing file is refused.
    let again = harness.send(CommandKind::SaveWorkflowFile {
        path: path.clone(),
        source: "x".into(),
        expected_hash: None,
    });
    assert!(failure(&again).is_some());

    // The right hash replaces the file.
    let edited = format!("{BRANCH}-- edited\n");
    let events = harness.send(CommandKind::SaveWorkflowFile {
        path: path.clone(),
        source: edited.clone(),
        expected_hash: Some(saved_hash.clone()),
    });
    assert_eq!(failure(&events), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);

    // A stale hash is a conflict, and the file keeps what is on disk.
    let stale = harness.send(CommandKind::SaveWorkflowFile {
        path: path.clone(),
        source: "lost\n".into(),
        expected_hash: Some(saved_hash),
    });
    assert_eq!(
        failure(&stale).as_deref(),
        Some("the file changed on disk; reload it")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);

    // Only a .lua file in a workflows folder is written.
    let toml = flows.path().join("x.toml").display().to_string();
    let not_lua = harness.send(CommandKind::SaveWorkflowFile {
        path: toml,
        source: "a = 1\n".into(),
        expected_hash: None,
    });
    assert!(failure(&not_lua).unwrap().contains("must end in .lua"));
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = harness.send(CommandKind::SaveWorkflowFile {
        path: elsewhere.path().join("out.lua").display().to_string(),
        source: BRANCH.to_owned(),
        expected_hash: None,
    });
    assert!(
        failure(&outside)
            .unwrap()
            .contains("is not a workflows directory")
    );
    assert!(!elsewhere.path().join("out.lua").exists());
}

#[test]
fn a_saved_edit_is_unreviewed_until_it_is_reviewed_again() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.review("make-branch");
    let hash = harness
        .listing()
        .iter()
        .find(|w| w.name == "make-branch")
        .unwrap()
        .hash
        .clone();

    let path = flows.path().join("make-branch.lua").display().to_string();
    let edited = format!("{BRANCH}-- saved\n");
    let events = harness.send(CommandKind::SaveWorkflowFile {
        path,
        source: edited,
        expected_hash: Some(hash),
    });
    assert_eq!(failure(&events), None);

    let events = harness.send(run("make-branch", &[("name", "feat/z")]));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::WorkflowReviewRequested { .. })),
        "{events:#?}"
    );
    assert_eq!(git(repo.path(), &["branch", "--list", "feat/z"]), "");
}

#[test]
fn deleting_moves_the_file_to_the_trash_and_refuses_a_stale_hash() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let trash = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "make-branch.lua", BRANCH);
    let mut harness = Harness::open(repo.path(), flows.path());
    harness
        .engine
        .set_workflow_trash_dir(Some(trash.path().to_path_buf()));
    let path = flows.path().join("make-branch.lua").display().to_string();

    let stale = harness.send(CommandKind::DeleteWorkflowFile {
        path: path.clone(),
        expected_hash: sha256("other"),
    });
    assert_eq!(
        failure(&stale).as_deref(),
        Some("the file changed on disk; reload it")
    );
    assert!(flows.path().join("make-branch.lua").exists());

    let events = harness.send(CommandKind::DeleteWorkflowFile {
        path,
        expected_hash: sha256(BRANCH),
    });
    assert_eq!(failure(&events), None);
    let trashed = events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowFileDeleted { trashed_to, .. } => Some(trashed_to.clone()),
            _ => None,
        })
        .expect("deleted");
    assert!(
        std::path::Path::new(&trashed).starts_with(trash.path()),
        "{trashed}"
    );
    assert_eq!(std::fs::read_to_string(&trashed).unwrap(), BRANCH);
    assert!(!flows.path().join("make-branch.lua").exists());
    assert!(harness.listing().is_empty());
}

#[test]
fn a_toml_workflow_is_listed_as_ignored_with_the_reason() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "old.toml", "schema = 1\n");
    let mut harness = Harness::open(repo.path(), flows.path());

    let (workflows, ignored) = listed(&harness.send(CommandKind::ListWorkflows));

    assert!(workflows.is_empty());
    assert_eq!(
        ignored,
        [format!(
            "{}: .toml workflows are no longer supported; rewrite it as a .lua script",
            flows.path().join("old.toml").display()
        )]
    );
}
