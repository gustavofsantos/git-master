//! Manual workflows end to end (docs/10): real repositories in temporary
//! directories, workflow files in a directory of their own, and the engine
//! driven directly. Commands go in, the `exec` jobs are taken and run here, and
//! each report goes back, as the runner does on its threads.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowEnd, WorkflowInfo,
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository on `main` with one commit. Its own config holds the author, so
/// the engine's commits work; nothing is changed in the working tree.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

const SUGGEST: &str = r#"
schema = 1
name = "suggest"

[trigger]
on = "manual"

[[state]]
id = "ask"
action = "exec"
with = { argv = ["printf", "feat: from workflow"] }
on.timeout = "end.failed"

[[state]]
id = "suggest"
action = "commit.suggest_message"
with = { message = "${states.ask.output.text}" }
"#;

const COMMIT_ALL: &str = r#"
schema = 1
name = "commit-all"

[trigger]
on = "manual"

[[state]]
id = "stage"
action = "stage.all"
with = {}

[[state]]
id = "commit"
action = "commit.create"
with = { message = "wf: commit" }
"#;

const SLOW: &str = r#"
schema = 1
name = "slow"

[trigger]
on = "manual"

[[state]]
id = "wait"
action = "exec"
with = { argv = ["sleep", "5"] }
on.timeout = "end.failed"
"#;

/// Not TOML: the file does not parse, so it is listed by its file name.
const BROKEN: &str = "schema = 1\nname = \"broken\"\n[[state\nid = \"x\"\n";

/// An open repository, the workflow directory set, and the operation ids counted.
struct Harness {
    engine: Engine,
    last: u64,
}

impl Harness {
    fn open(repo: &Path, flows: &Path) -> Self {
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(flows.to_path_buf()));
        let mut harness = Self { engine, last: 0 };
        harness.send(CommandKind::OpenRepo {
            path: repo.display().to_string(),
        });
        harness
    }

    /// Sends a human's command and returns the events it produced.
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

    /// Runs every `exec` job taken so far and feeds each report back, until no
    /// job is left. Returns the events the reports produced.
    fn run_jobs(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let jobs = self.engine.take_workflow_jobs();
            if jobs.is_empty() {
                return events;
            }
            for job in jobs {
                events.extend(self.engine.workflow_report(job.run()));
            }
        }
    }
}

fn run(name: &str) -> CommandKind {
    CommandKind::RunWorkflow {
        name: name.to_owned(),
        inputs: BTreeMap::new(),
    }
}

/// The run a start answered with: the run id of its first run event.
fn run_id(events: &[Event]) -> i64 {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowRunChanged { run, .. } => Some(*run),
            _ => None,
        })
        .expect("a run event")
}

fn last_state(events: &[Event]) -> OperationState {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, .. } => Some(*state),
            _ => None,
        })
        .expect("an operation event")
}

/// The detail of the operation that failed, if one did.
fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|event| match event {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

fn listing(events: &[Event]) -> (Vec<WorkflowInfo>, Vec<String>) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowsListed {
                workflows, ignored, ..
            } => Some((workflows.clone(), ignored.clone())),
            _ => None,
        })
        .expect("a listing")
}

fn status(repo: &Path) -> String {
    git(repo, &["status", "--porcelain"])
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

#[test]
fn a_manual_workflow_suggests_the_text_its_exec_step_printed() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let mut harness = Harness::open(repo.path(), flows.path());

    let started = harness.send(run("suggest"));
    let run = run_id(&started);
    assert_eq!(last_state(&started), OperationState::Done);
    let events = harness.run_jobs();

    assert!(events.contains(&Event::WorkflowMessageSuggested {
        run,
        message: "feat: from workflow".to_owned(),
    }));
    assert!(events.contains(&Event::WorkflowRunFinished {
        run,
        end: WorkflowEnd::Done,
        reason: None,
    }));
    // The run wrote no commit and no file of the working tree; its state lives in .git.
    assert_eq!(status(repo.path()), "");
    assert!(repo.path().join(".git/gitmaster/state.db").exists());
}

#[test]
fn stage_all_then_commit_commits_the_working_tree_and_is_logged() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path(), "a.txt", "two\n");
    write(flows.path(), "commit-all.toml", COMMIT_ALL);
    let mut harness = Harness::open(repo.path(), flows.path());

    let started = harness.send(run("commit-all"));
    let run = run_id(&started);
    let operation = OperationId(harness.last);

    assert!(started.contains(&Event::WorkflowRunFinished {
        run,
        end: WorkflowEnd::Done,
        reason: None,
    }));
    assert!(
        started
            .iter()
            .any(|event| matches!(event, Event::StatusLoaded { .. }))
    );
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "wf: commit"
    );
    assert_eq!(status(repo.path()), "");

    let log = harness.engine.log();
    assert!(log.iter().any(|entry| entry.id == operation
        && entry.actor == Actor::Human
        && entry.kind == CommandKind::StageAll
        && entry.state == OperationState::Done));
    assert!(log.iter().any(|entry| entry.id == operation
        && entry.kind
            == CommandKind::Commit {
                message: "wf: commit".to_owned(),
            }
        && entry.state == OperationState::Done));
}

#[test]
fn a_broken_workflow_is_listed_with_its_error_and_refuses_to_run() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    write(flows.path(), "broken.toml", BROKEN);
    let mut harness = Harness::open(repo.path(), flows.path());

    let listed = harness.send(CommandKind::ListWorkflows);
    let (workflows, ignored) = listing(&listed);

    assert!(ignored.is_empty());
    let broken = workflows
        .iter()
        .find(|workflow| workflow.name == "broken")
        .expect("the broken file is listed");
    assert!(
        broken
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("line "))
    );
    let suggest = workflows
        .iter()
        .find(|workflow| workflow.name == "suggest")
        .expect("the good file is listed");
    assert_eq!(suggest.error, None);
    assert_eq!(suggest.trigger, "manual");
    assert_eq!(suggest.hash.len(), 64);
    // A read is not in the command log (D28).
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| entry.kind == CommandKind::ListWorkflows)
    );

    let refused = harness.send(run("broken"));
    assert_eq!(
        failure(&refused).as_deref(),
        Some("workflow `broken` does not load")
    );
}

#[test]
fn a_workflow_directory_inside_the_repository_is_ignored_and_one_outside_is_read() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let inside = repo.path().join("flows-in-repo");
    std::fs::create_dir(&inside).unwrap();
    write(&inside, "hidden.toml", SLOW);
    write(extra.path(), "slow.toml", SLOW);
    git(
        repo.path(),
        &[
            "config",
            "--add",
            "gitmaster.workflowPath",
            &inside.display().to_string(),
        ],
    );
    git(
        repo.path(),
        &[
            "config",
            "--add",
            "gitmaster.workflowPath",
            &extra.path().display().to_string(),
        ],
    );
    let mut harness = Harness::open(repo.path(), flows.path());

    let (workflows, ignored) = listing(&harness.send(CommandKind::ListWorkflows));

    let names: Vec<&str> = workflows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["slow"]);
    assert!(ignored.iter().any(|line| {
        line.starts_with(&format!("{}: ", inside.display()))
            && line.contains("inside the repository")
    }));
}

#[test]
fn a_run_waiting_on_an_exec_step_is_resumed_by_the_next_engine() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "slow.toml", SLOW);

    let mut first = Harness::open(repo.path(), flows.path());
    let run = run_id(&first.send(run("slow")));
    let jobs = first.engine.take_workflow_jobs();
    assert_eq!(jobs.iter().map(|job| job.run).collect::<Vec<_>>(), [run]);
    // The engine goes away before its job runs; the job is never run.
    drop(jobs);
    drop(first);

    let mut second = Harness::open(repo.path(), flows.path());
    second.send(CommandKind::ListWorkflows);
    let resumed: Vec<i64> = second
        .engine
        .take_workflow_jobs()
        .iter()
        .map(|job| job.run)
        .collect();

    assert_eq!(resumed, [run]);
    assert!(repo.path().join(".git/gitmaster/state.db").exists());
    assert_eq!(status(repo.path()), "");
}

fn create(template: &str) -> CommandKind {
    CommandKind::CreateWorkflowFromTemplate {
        template: template.to_owned(),
    }
}

/// The workflow file a `WorkflowCreated` answer names, and its name.
fn created(events: &[Event]) -> (String, String) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowCreated { path, name, .. } => Some((path.clone(), name.clone())),
            _ => None,
        })
        .expect("a workflow created event")
}

fn templates_listed(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowsListed { templates, .. } => Some(templates.clone()),
            _ => None,
        })
        .expect("a listing")
}

fn template_source(name: &str) -> &'static str {
    gm_core::workflow::templates::all()
        .iter()
        .find(|template| template.name == name)
        .expect("a built-in template")
        .source
}

#[test]
fn a_template_is_copied_into_the_user_directory_and_listed() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let user_dir = flows.path().join("not-yet-there");
    let mut harness = Harness::open(repo.path(), &user_dir);

    let answer = harness.send(create("commit-staged"));
    let operation = OperationId(harness.last);

    assert_eq!(last_state(&answer), OperationState::Done);
    let (path, name) = created(&answer);
    assert_eq!(name, "commit-staged");
    assert_eq!(Path::new(&path), user_dir.join("commit-staged.toml"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        template_source("commit-staged")
    );
    // Creating a file is not a Git operation: it is not in the command log.
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| entry.id == operation)
    );

    let listed = harness.send(CommandKind::ListWorkflows);
    let (workflows, _) = listing(&listed);
    let copy = workflows
        .iter()
        .find(|workflow| workflow.name == "commit-staged")
        .expect("the copy is listed");
    assert_eq!(copy.error, None);
    assert_eq!(
        templates_listed(&listed),
        [
            "suggest-commit-message",
            "branch-from-ticket",
            "commit-staged"
        ]
    );
}

#[test]
fn a_second_copy_gets_a_suffixed_file_and_a_suffixed_name() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let first = harness.send(create("branch-from-ticket"));
    let second = harness.send(create("branch-from-ticket"));

    assert_eq!(created(&first).1, "branch-from-ticket");
    let (path, name) = created(&second);
    assert_eq!(name, "branch-from-ticket-2");
    assert_eq!(
        Path::new(&path),
        flows.path().join("branch-from-ticket-2.toml")
    );
    let copy = std::fs::read_to_string(&path).unwrap();
    assert!(copy.contains("name = \"branch-from-ticket-2\"\n"));
    assert!(!copy.contains("name = \"branch-from-ticket\"\n"));
    // The first copy is untouched.
    assert_eq!(
        std::fs::read_to_string(flows.path().join("branch-from-ticket.toml")).unwrap(),
        template_source("branch-from-ticket")
    );

    let (workflows, _) = listing(&harness.send(CommandKind::ListWorkflows));
    let names: Vec<&str> = workflows.iter().map(|w| w.name.as_str()).collect();
    assert!(names.contains(&"branch-from-ticket"));
    assert!(names.contains(&"branch-from-ticket-2"));
    assert!(workflows.iter().all(|w| w.error.is_none()));
}

#[test]
fn an_agent_cannot_create_a_workflow_from_a_template() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let refused = harness.engine.handle(
        OperationId(100),
        &Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: create("commit-staged"),
        },
    );

    assert_eq!(last_state(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused).as_deref(),
        Some("only a human can create a workflow from a template")
    );
    assert_eq!(std::fs::read_dir(flows.path()).unwrap().count(), 0);
}

#[test]
fn an_unknown_template_fails_and_writes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let refused = harness.send(create("nope"));

    assert_eq!(last_state(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused).as_deref(),
        Some("no template named `nope`")
    );
    assert_eq!(std::fs::read_dir(flows.path()).unwrap().count(), 0);
}

#[test]
fn without_a_user_directory_a_template_is_refused() {
    let repo = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_workflow_user_dir(None);
    engine.handle(
        OperationId(1),
        &Command {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: repo.path().display().to_string(),
            },
        },
    );

    let refused = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Human,
            kind: create("commit-staged"),
        },
    );

    assert_eq!(
        failure(&refused).as_deref(),
        Some("there is no user workflows directory (HOME is not set)")
    );
}
