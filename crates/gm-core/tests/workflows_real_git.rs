//! Manual workflows end to end (docs/10): real repositories in temporary
//! directories, workflow files in a directory of their own, and the engine
//! driven directly. Commands go in, the `exec` jobs are taken and run here, and
//! each report goes back, as the runner does on its threads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, WorkflowEnd, WorkflowInfo,
    WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary,
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
            "new-workflow",
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

fn list_runs(workflow: &str, limit: u32) -> CommandKind {
    CommandKind::ListWorkflowRuns {
        workflow: workflow.to_owned(),
        limit,
    }
}

fn load_run(run: i64) -> CommandKind {
    CommandKind::LoadWorkflowRun { run }
}

fn save(path: &Path, source: &str, expected_hash: Option<&str>) -> CommandKind {
    CommandKind::SaveWorkflowFile {
        path: path.display().to_string(),
        source: source.to_owned(),
        expected_hash: expected_hash.map(str::to_owned),
    }
}

fn delete(path: &Path, expected_hash: &str) -> CommandKind {
    CommandKind::DeleteWorkflowFile {
        path: path.display().to_string(),
        expected_hash: expected_hash.to_owned(),
    }
}

fn runs_listed(events: &[Event]) -> Vec<WorkflowRunSummary> {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowRunsListed { runs, .. } => Some(runs.clone()),
            _ => None,
        })
        .expect("a runs listing")
}

fn run_loaded(events: &[Event]) -> WorkflowRunDetail {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowRunLoaded { detail, .. } => Some((**detail).clone()),
            _ => None,
        })
        .expect("a loaded run")
}

/// The path and hash a `WorkflowFileSaved` answer names.
fn file_saved(events: &[Event]) -> (String, String) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowFileSaved { path, hash, .. } => Some((path.clone(), hash.clone())),
            _ => None,
        })
        .expect("a saved event")
}

/// The path and trash location a `WorkflowFileDeleted` answer names.
fn file_deleted(events: &[Event]) -> (String, String) {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowFileDeleted {
                path, trashed_to, ..
            } => Some((path.clone(), trashed_to.clone())),
            _ => None,
        })
        .expect("a deleted event")
}

/// The hash the listing gives the workflow file `name`.
fn hash_listed(events: &[Event], name: &str) -> String {
    listing(events)
        .0
        .iter()
        .find(|workflow| workflow.name == name)
        .expect("the file is listed")
        .hash
        .clone()
}

fn is_failure_containing(events: &[Event], text: &str) -> bool {
    failure(events).is_some_and(|detail| detail.contains(text))
}

#[test]
fn a_run_view_lists_each_step_with_its_rendered_input_output_and_outcome() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let mut harness = Harness::open(repo.path(), flows.path());
    let run = run_id(&harness.send(run("suggest")));
    harness.run_jobs();

    let detail = run_loaded(&harness.send(load_run(run)));

    let summary = &detail.summary;
    assert_eq!(summary.run, run);
    assert_eq!(summary.workflow, "suggest");
    assert_eq!(summary.status, WorkflowRunStatus::Done);
    assert_eq!(summary.started_by, "human");
    assert!(summary.ended_at.is_some());
    assert_eq!(summary.end_reason, None);
    assert_eq!(summary.current_state, None);
    assert_eq!(detail.definition, SUGGEST);
    assert_eq!(detail.inputs, json!({}));
    assert_eq!(detail.trigger, json!({"on": "manual"}));
    assert_eq!(
        Path::new(detail.repo["root"].as_str().expect("the root")),
        repo.path()
    );

    assert_eq!(detail.steps.len(), 2);
    let ask = &detail.steps[0];
    assert_eq!(ask.state, "ask");
    assert_eq!(ask.visit, 1);
    assert_eq!(ask.action.as_deref(), Some("exec"));
    assert_eq!(
        ask.input,
        Some(json!({"argv": ["printf", "feat: from workflow"]}))
    );
    assert_eq!(
        ask.output.as_ref().and_then(|output| output.get("text")),
        Some(&json!("feat: from workflow"))
    );
    assert!(!ask.truncated);
    assert_eq!(ask.outcome.as_deref(), Some("done"));
    assert_eq!(ask.next.as_deref(), Some("suggest"));
    assert!(ask.ended_at.is_some());

    let suggest = &detail.steps[1];
    assert_eq!(suggest.state, "suggest");
    assert_eq!(suggest.action.as_deref(), Some("commit.suggest_message"));
    assert_eq!(
        suggest.input,
        Some(json!({"message": "feat: from workflow"}))
    );
    assert_eq!(
        suggest.output,
        Some(json!({"message": "feat: from workflow"}))
    );
    assert_eq!(suggest.outcome.as_deref(), Some("done"));
    assert_eq!(suggest.next.as_deref(), Some("end.done"));

    // A read is not in the command log (D28).
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| matches!(entry.kind, CommandKind::LoadWorkflowRun { .. }))
    );
}

#[test]
fn a_run_that_does_not_exist_fails_to_load() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());

    let refused = harness.send(load_run(999));

    assert_eq!(last_state(&refused), OperationState::Failed);
    assert_eq!(failure(&refused).as_deref(), Some("no run 999"));
}

#[test]
fn runs_are_listed_newest_first_and_only_those_of_the_named_workflow() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path(), "a.txt", "two\n");
    write(flows.path(), "suggest.toml", SUGGEST);
    write(flows.path(), "commit-all.toml", COMMIT_ALL);
    let mut harness = Harness::open(repo.path(), flows.path());
    let first = run_id(&harness.send(run("suggest")));
    harness.run_jobs();
    let other = run_id(&harness.send(run("commit-all")));
    let second = run_id(&harness.send(run("suggest")));
    harness.run_jobs();

    let listed = runs_listed(&harness.send(list_runs("suggest", 10)));

    let ids: Vec<i64> = listed.iter().map(|summary| summary.run).collect();
    assert_eq!(ids, [second, first]);
    assert!(!ids.contains(&other));
    assert!(listed.iter().all(|summary| summary.workflow == "suggest"));
    assert!(
        listed
            .iter()
            .all(|summary| summary.status == WorkflowRunStatus::Done)
    );

    let newest = runs_listed(&harness.send(list_runs("suggest", 1)));
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].run, second);
    assert!(runs_listed(&harness.send(list_runs("nobody", 10))).is_empty());
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| matches!(entry.kind, CommandKind::ListWorkflowRuns { .. }))
    );
}

#[test]
fn a_staged_diff_over_4_kib_is_cut_in_the_run_view() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path(), "big.txt", &"a line of text\n".repeat(600));
    git(repo.path(), &["add", "big.txt"]);
    write(flows.path(), "suggest.toml", SUGGEST);
    let mut harness = Harness::open(repo.path(), flows.path());
    let run = run_id(&harness.send(run("suggest")));
    harness.run_jobs();

    let detail = run_loaded(&harness.send(load_run(run)));

    let full = git(
        repo.path(),
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ],
    );
    let diff = detail.repo["staged_diff"].as_str().expect("the diff");
    let (head, rest) = diff.split_once("\n… (").expect("the cut marker");
    let size: usize = rest
        .strip_suffix(" bytes)")
        .and_then(|size| size.parse().ok())
        .expect("the size");
    assert_eq!(head.len(), 4 * 1024);
    assert!(full.starts_with(head));
    assert_eq!(size, full.len());
}

#[test]
fn a_saved_file_with_the_hash_it_was_read_at_is_rewritten_and_relisted() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let path = flows.path().join("suggest.toml");
    let mut harness = Harness::open(repo.path(), flows.path());
    let hash = hash_listed(&harness.send(CommandKind::ListWorkflows), "suggest");
    let edited = format!("{SUGGEST}\n# edited\n");

    let answer = harness.send(save(&path, &edited, Some(&hash)));

    assert_eq!(last_state(&answer), OperationState::Done);
    let (saved_path, new_hash) = file_saved(&answer);
    assert_eq!(saved_path, path.display().to_string());
    assert_eq!(new_hash, format!("{:x}", Sha256::digest(edited.as_bytes())));
    assert_ne!(new_hash, hash);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    assert_eq!(hash_listed(&answer, "suggest"), new_hash);
    // A save writes no Git state: it is not in the command log.
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| matches!(entry.kind, CommandKind::SaveWorkflowFile { .. }))
    );
}

#[test]
fn a_save_against_a_file_that_changed_on_disk_is_refused() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let path = flows.path().join("suggest.toml");
    let mut harness = Harness::open(repo.path(), flows.path());
    let hash = hash_listed(&harness.send(CommandKind::ListWorkflows), "suggest");
    let elsewhere = "schema = 1\n# changed by someone else\n";
    write(flows.path(), "suggest.toml", elsewhere);

    let refused = harness.send(save(&path, "mine", Some(&hash)));

    assert_eq!(last_state(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused).as_deref(),
        Some("the file changed on disk; reload it")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), elsewhere);
}

#[test]
fn a_save_outside_the_workflow_directories_or_not_as_toml_is_refused() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let inside = repo.path().join("flow.toml");
    let outside = elsewhere.path().join("flow.toml");
    let not_toml = flows.path().join("notes.txt");

    let in_repo = harness.send(save(&inside, SUGGEST, None));
    let not_listed = harness.send(save(&outside, SUGGEST, None));
    let wrong_name = harness.send(save(&not_toml, SUGGEST, None));

    assert!(is_failure_containing(&in_repo, "is inside the repository"));
    assert!(is_failure_containing(
        &not_listed,
        "is not a workflows directory"
    ));
    assert!(is_failure_containing(&wrong_name, "must end in .toml"));
    assert!(!inside.exists());
    assert!(!outside.exists());
    assert!(!not_toml.exists());
}

#[test]
fn a_new_file_is_created_in_the_user_directory_and_not_overwritten_without_a_hash() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    // The user directory does not exist yet: the first save creates it.
    let user_dir = flows.path().join("not-yet-there");
    let mut harness = Harness::open(repo.path(), &user_dir);
    let path = user_dir.join("new.toml");

    let created = harness.send(save(&path, SUGGEST, None));

    assert_eq!(last_state(&created), OperationState::Done);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SUGGEST);
    assert!(
        listing(&created)
            .0
            .iter()
            .any(|workflow| workflow.name == "suggest")
    );

    let again = harness.send(save(&path, "schema = 1\n", None));

    assert_eq!(last_state(&again), OperationState::Failed);
    assert_eq!(
        failure(&again),
        Some(format!("a file already exists at {}", path.display()))
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SUGGEST);
}

#[test]
fn a_deleted_file_is_moved_to_the_trash_and_no_longer_listed() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let path = flows.path().join("suggest.toml");
    let trash = data.path().join("gitmaster").join("workflow-trash");
    let mut harness = Harness::open(repo.path(), flows.path());
    harness.engine.set_workflow_trash_dir(Some(trash.clone()));
    let hash = hash_listed(&harness.send(CommandKind::ListWorkflows), "suggest");

    let answer = harness.send(delete(&path, &hash));

    assert_eq!(last_state(&answer), OperationState::Done);
    let (deleted_path, trashed_to) = file_deleted(&answer);
    assert_eq!(deleted_path, path.display().to_string());
    let trashed = PathBuf::from(&trashed_to);
    assert_eq!(trashed.parent(), Some(trash.as_path()));
    let name = trashed.file_name().unwrap().to_str().unwrap();
    let (stamp, original) = name.split_once('-').expect("a time stamp first");
    assert!(stamp.parse::<u64>().is_ok());
    assert_eq!(original, "suggest.toml");
    assert_eq!(std::fs::read_to_string(&trashed).unwrap(), SUGGEST);
    assert!(!path.exists());
    assert!(
        !listing(&answer)
            .0
            .iter()
            .any(|workflow| workflow.name == "suggest")
    );
}

#[test]
fn a_delete_against_a_file_that_changed_on_disk_is_refused() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let path = flows.path().join("suggest.toml");
    let mut harness = Harness::open(repo.path(), flows.path());
    harness
        .engine
        .set_workflow_trash_dir(Some(data.path().join("trash")));
    let hash = hash_listed(&harness.send(CommandKind::ListWorkflows), "suggest");
    write(
        flows.path(),
        "suggest.toml",
        "schema = 1\n# edited elsewhere\n",
    );

    let refused = harness.send(delete(&path, &hash));

    assert_eq!(last_state(&refused), OperationState::Failed);
    assert_eq!(
        failure(&refused).as_deref(),
        Some("the file changed on disk; reload it")
    );
    assert!(path.exists());
    assert!(!data.path().join("trash").exists());
}

#[test]
fn an_agent_cannot_save_or_delete_a_workflow_file() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(flows.path(), "suggest.toml", SUGGEST);
    let path = flows.path().join("suggest.toml");
    let mut harness = Harness::open(repo.path(), flows.path());
    harness
        .engine
        .set_workflow_trash_dir(Some(data.path().join("trash")));
    let hash = hash_listed(&harness.send(CommandKind::ListWorkflows), "suggest");
    let agent = Actor::Agent {
        name: "agent".into(),
    };

    let saved = harness.engine.handle(
        OperationId(100),
        &Command {
            actor: agent.clone(),
            kind: save(&path, "agent wrote this", Some(&hash)),
        },
    );
    let deleted = harness.engine.handle(
        OperationId(101),
        &Command {
            actor: agent,
            kind: delete(&path, &hash),
        },
    );

    assert_eq!(
        failure(&saved).as_deref(),
        Some("only a human can save a workflow file")
    );
    assert_eq!(
        failure(&deleted).as_deref(),
        Some("only a human can delete a workflow file")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SUGGEST);
    assert!(!data.path().join("trash").exists());
}

/// Stages, runs an `exec` and commits. The commit message is a literal: an
/// assumed `exec` output would not render in a dry run.
const DRY_COMMIT: &str = r#"
schema = 1
name = "dry-commit"

[trigger]
on = "manual"

[[state]]
id = "stage"
action = "stage.all"
with = {}

[[state]]
id = "say"
action = "exec"
with = { argv = ["printf", "%s", "feat: dry"] }
on.timeout = "end.failed"

[[state]]
id = "commit"
action = "commit.create"
with = { message = "wf: dry" }
"#;

fn dry_run_of(source: &str, inputs: serde_json::Value) -> CommandKind {
    CommandKind::DryRunWorkflow {
        source: source.to_owned(),
        inputs,
    }
}

fn dry_run_listed(events: &[Event]) -> gm_schema::WorkflowDryRun {
    events
        .iter()
        .find_map(|event| match event {
            Event::WorkflowDryRun { dry_run, .. } => Some((**dry_run).clone()),
            _ => None,
        })
        .expect("a dry run")
}

#[test]
fn a_dry_run_shows_each_step_and_performs_none_of_them() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path(), "a.txt", "two\n");
    let mut harness = Harness::open(repo.path(), flows.path());
    let before = status(repo.path());
    assert_eq!(before, " M a.txt\n");

    let events = harness.send(dry_run_of(DRY_COMMIT, json!({})));
    let dry = dry_run_listed(&events);

    assert_eq!(last_state(&events), OperationState::Done);
    assert_eq!(dry.error, None);
    assert!(dry.missing_inputs.is_empty());
    assert_eq!(dry.stop, "end.done");
    let names: Vec<(&str, &str)> = dry
        .steps
        .iter()
        .map(|step| (step.state.as_str(), step.action.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("stage", "stage.all"),
            ("say", "exec"),
            ("commit", "commit.create"),
        ]
    );
    let say = &dry.steps[1];
    assert_eq!(say.input, json!({"argv": ["printf", "%s", "feat: dry"]}));
    assert_eq!(say.level, "read");
    assert!(!say.executed);
    assert!(say.assumed);
    let commit = &dry.steps[2];
    assert_eq!(commit.input, json!({"message": "wf: dry"}));
    assert_eq!(commit.level, "apply");
    assert!(!commit.executed);
    assert!(commit.assumed);

    // Nothing was done: the working tree, the history and the store are as they were.
    assert_eq!(status(repo.path()), before);
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]).trim(),
        "init"
    );
    assert!(!repo.path().join(".git/gitmaster/state.db").exists());
    assert!(runs_listed(&harness.send(list_runs("dry-commit", 10))).is_empty());
    assert!(
        !harness
            .engine
            .log()
            .iter()
            .any(|entry| matches!(entry.kind, CommandKind::DryRunWorkflow { .. }))
    );
}

#[test]
fn a_dry_run_fills_a_missing_input_with_an_empty_string_and_lists_it() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let source = r#"
schema = 1
name = "named"

[inputs]
ticket = { prompt = "Ticket key" }
title = {}

[trigger]
on = "manual"

[[state]]
id = "say"
action = "exec"
with = { argv = ["printf", "%s", "${inputs.ticket}"] }
on.timeout = "end.failed"
"#;

    let events = harness.send(dry_run_of(source, json!({"ticket": "GM-7"})));
    let dry = dry_run_listed(&events);

    assert_eq!(dry.missing_inputs, ["title"]);
    assert_eq!(
        dry.steps[0].input,
        json!({"argv": ["printf", "%s", "GM-7"]})
    );
    assert_eq!(dry.stop, "end.done");
}

#[test]
fn a_dry_run_of_a_text_that_does_not_load_reports_its_errors() {
    let repo = tempfile::tempdir().unwrap();
    let flows = tempfile::tempdir().unwrap();
    init_repo(repo.path());
    let mut harness = Harness::open(repo.path(), flows.path());
    let source = DRY_COMMIT.replace("action = \"commit.create\"", "action = \"commit.nope\"");

    let events = harness.send(dry_run_of(&source, json!({})));
    let dry = dry_run_listed(&events);

    assert_eq!(last_state(&events), OperationState::Done);
    let error = dry.error.expect("the text does not load");
    assert!(error.contains("unknown action `commit.nope`"), "{error}");
    assert!(dry.steps.is_empty());
    assert_eq!(dry.stop, "");
}
