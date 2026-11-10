//! Review workflows (D219, S4) against real repositories in temporary directories: `ctx.change()`
//! on a stack parent, an upstream and no base; the repository's `.git/gitmaster/workflows` folder,
//! its listing, its worktrees and its shadowing of the user's folder; the review picker's filter;
//! and `GITMASTER_WORKFLOW_RUN` in the environment of `ctx.exec`. This harness plays the engine
//! thread that carries a run's effects and events, as the runner does.

use std::collections::BTreeMap;
use std::path::Path;
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
    WorkflowLocation, WorkflowRole, review_workflows,
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

fn commit_file(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// `main` with `a.txt`, and nothing else.
fn init_main(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    commit_file(dir, "a.txt", "one\n");
}

/// A manual workflow `name` whose body is `body`, with the given `M.meta` extras.
fn manual(name: &str, extra: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{ name = \"{name}\", trigger = {{ on = \"manual\" }}{extra} }}\n\
         function M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

fn write(dir: &Path, file: &str, source: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file), source).unwrap();
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
    /// Opens `repo`, with the user's workflows in `user` (which may not exist).
    fn open(repo: &Path, user: &Path) -> Self {
        let (tx, messages) = mpsc::channel();
        let effects_tx = Mutex::new(tx.clone());
        let events_tx = Mutex::new(tx);
        let mut engine = Engine::new(Box::new(SubprocessGit::default()));
        engine.set_workflow_user_dir(Some(user.to_path_buf()));
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
        let (id, events) = harness.human(CommandKind::OpenRepo { path: s(repo) });
        assert_eq!(
            state_of(&events, id),
            Some(OperationState::Done),
            "{events:?}"
        );
        harness
    }

    fn human(&mut self, kind: CommandKind) -> (OperationId, Vec<Event>) {
        self.last += 1;
        let id = OperationId(self.last);
        let mut events = self.engine.handle(
            id,
            &Command {
                actor: Actor::Human,
                kind,
            },
        );
        events.extend(self.jobs());
        (id, events)
    }

    /// Runs the Git jobs the engine queued, until none is left.
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

    fn step(&mut self, wait: Duration) -> Vec<Event> {
        let mut events = match self.messages.recv_timeout(wait) {
            Ok(Message::Effect(request)) => self.engine.script_effect(request),
            Ok(Message::Event(event)) => self.engine.script_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Vec::new(),
        };
        events.extend(self.jobs());
        events
    }

    fn listing(&mut self) -> Vec<WorkflowInfo> {
        let (_, events) = self.human(CommandKind::ListWorkflows);
        events
            .into_iter()
            .find_map(|e| match e {
                Event::WorkflowsListed { workflows, .. } => Some(workflows),
                _ => None,
            })
            .expect("the workflows are listed")
    }

    /// The listed workflow `name`.
    fn info(&mut self, name: &str) -> WorkflowInfo {
        self.listing()
            .into_iter()
            .find(|w| w.name == name)
            .unwrap_or_else(|| panic!("no workflow named {name}"))
    }

    /// Reviews the workflow `name` as the listing shows it, so it may run.
    fn review(&mut self, name: &str) {
        let hash = self.info(name).hash;
        let (_, events) = self.human(CommandKind::ReviewWorkflow {
            name: name.to_owned(),
            hash,
            auto_approve: false,
        });
        assert!(failure(&events).is_none(), "review refused: {events:#?}");
    }

    /// Runs the manual workflow `name` and returns its events once the operation ended and the
    /// run finished.
    fn run(&mut self, name: &str) -> Vec<Event> {
        let (id, mut events) = self.human(CommandKind::RunWorkflow {
            name: name.to_owned(),
            inputs: BTreeMap::new(),
        });
        let end = Instant::now() + Duration::from_secs(30);
        let done = |events: &[Event]| {
            state_of(events, id).is_some_and(OperationState::is_terminal)
                && !ends(events).is_empty()
        };
        while !done(&events) {
            assert!(Instant::now() < end, "timed out; events: {events:#?}");
            events.extend(self.step(Duration::from_millis(20)));
        }
        events
    }
}

fn state_of(events: &[Event], id: OperationId) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged {
            id: found, state, ..
        } if *found == id && state.is_terminal() => Some(*state),
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

/// The run id of the run that finished, the one the workflow's events carry.
fn run_id(events: &[Event]) -> i64 {
    events
        .iter()
        .find_map(|e| match e {
            Event::WorkflowRunFinished { run, .. } => Some(*run),
            _ => None,
        })
        .expect("the run finished")
}

/// A probe that reports what `ctx.change()` gave it.
const CHANGE_PROBE: &str = r#"local c = ctx.change()
ctx.notify("base=" .. (c.base or "nil") .. " files=" .. table.concat(c.files, ",") .. " description=" .. (c.description or "nil") .. " truncated=" .. tostring(c.truncated))"#;

fn change_notice(h: &mut Harness, workflow_dir: &Path) -> String {
    write(
        workflow_dir,
        "probe.lua",
        &manual("probe-change", "", CHANGE_PROBE),
    );
    h.review("probe-change");
    let events = h.run("probe-change");
    assert_eq!(ends(&events), [WorkflowEnd::Done], "{events:#?}");
    let texts = notices(&events);
    assert_eq!(texts.len(), 1, "{texts:?}");
    texts[0].clone()
}

#[test]
fn ctx_change_measures_a_branch_against_its_stack_parent_with_its_description() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    git(&repo, &["branch", "base"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit_file(&repo, "b.txt", "two\n");
    git(&repo, &["config", "branch.feat.stackparent", "base"]);
    git(
        &repo,
        &["config", "branch.feat.description", "Adds the b file"],
    );
    let user = dir.path().join("user");
    let mut h = Harness::open(&repo, &user);
    assert_eq!(
        change_notice(&mut h, &user),
        "base=base files=b.txt description=Adds the b file truncated=false"
    );
}

#[test]
fn ctx_change_falls_back_to_the_upstream_when_there_is_no_stack_parent() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", &s(&origin)],
    );
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    git(&repo, &["remote", "add", "origin", &s(&origin)]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    git(&repo, &["branch", "--set-upstream-to=origin/main", "feat"]);
    commit_file(&repo, "b.txt", "two\n");
    let user = dir.path().join("user");
    let mut h = Harness::open(&repo, &user);
    assert_eq!(
        change_notice(&mut h, &user),
        "base=origin/main files=b.txt description=nil truncated=false"
    );
}

#[test]
fn ctx_change_has_no_base_and_no_files_when_nothing_is_known() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "feat"]);
    commit_file(&repo, "a.txt", "one\n");
    let user = dir.path().join("user");
    let mut h = Harness::open(&repo, &user);
    assert_eq!(
        change_notice(&mut h, &user),
        "base=nil files= description=nil truncated=false"
    );
}

#[test]
fn a_repository_workflow_is_listed_from_its_folder_and_runs_once_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    let folder = repo.join(".git").join("gitmaster").join("workflows");
    write(
        &folder,
        "probe-task.lua",
        &manual("probe-task", ", review = true", "ctx.notify(\"task ran\")"),
    );
    let user = dir.path().join("user");
    let mut h = Harness::open(&repo, &user);

    let info = h.info("probe-task");
    assert_eq!(info.location, WorkflowLocation::Repository);
    assert_eq!(info.role, WorkflowRole::Review);
    assert!(!info.reviewed, "unreviewed until approved");
    assert_eq!(info.error, None);

    h.review("probe-task");
    let events = h.run("probe-task");
    assert_eq!(notices(&events), ["task ran"], "{events:#?}");
    assert_eq!(ends(&events), [WorkflowEnd::Done]);
    assert!(h.info("probe-task").reviewed);
    // Nothing was written into the working tree.
    assert!(!repo.join("probe-task.lua").exists());
}

#[test]
fn a_worktree_of_the_repository_sees_the_same_workflow_folder() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    write(
        &repo.join(".git").join("gitmaster").join("workflows"),
        "probe-task.lua",
        &manual("probe-task", ", review = true", "ctx.notify(\"task ran\")"),
    );
    let worktree = dir.path().join("wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "other", &s(&worktree)],
    );
    let user = dir.path().join("user");
    let mut h = Harness::open(&worktree, &user);
    let info = h.info("probe-task");
    assert_eq!(info.location, WorkflowLocation::Repository);
}

#[test]
fn a_repository_workflow_shadows_the_user_one_of_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    write(
        &repo.join(".git").join("gitmaster").join("workflows"),
        "dup.lua",
        &manual("dup", "", "ctx.notify(\"repository\")"),
    );
    let user = dir.path().join("user");
    write(&user, "dup.lua", &manual("dup", "", "ctx.notify(\"user\")"));
    let mut h = Harness::open(&repo, &user);
    let listing = h.listing();
    let winner: Vec<&WorkflowInfo> = listing
        .iter()
        .filter(|w| w.name == "dup" && w.error.is_none())
        .collect();
    assert_eq!(winner.len(), 1, "{listing:#?}");
    assert_eq!(winner[0].location, WorkflowLocation::Repository);
    let shadowed = listing
        .iter()
        .find(|w| w.location == WorkflowLocation::User && w.name == "dup")
        .expect("the user one is still listed");
    assert!(
        shadowed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("shadowed")),
        "{shadowed:?}"
    );
}

#[test]
fn the_review_picker_lists_only_the_workflows_marked_review() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    let user = dir.path().join("user");
    write(&user, "rev-a.lua", &manual("rev-a", ", review = true", ""));
    write(&user, "plain.lua", &manual("plain", "", ""));
    write(
        &repo.join(".git").join("gitmaster").join("workflows"),
        "rev-b.lua",
        &manual("rev-b", ", review = true", ""),
    );
    write(
        &user,
        "bad-review.lua",
        "local M = {}\nM.meta = { name = \"bad-review\", trigger = { on = \"commit.created\" }, review = true }\nfunction M.execute(ctx, input) end\nreturn M\n",
    );
    let mut h = Harness::open(&repo, &user);
    let listing = h.listing();
    let mut names: Vec<&str> = review_workflows(&listing)
        .iter()
        .map(|w| w.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["rev-a", "rev-b"]);
    let bad = listing.iter().find(|w| w.name == "bad-review").unwrap();
    assert!(bad.error.is_some(), "a review on an event does not load");
}

#[test]
fn the_run_id_reaches_the_program_that_a_review_starts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_main(&repo);
    let user = dir.path().join("user");
    let out = dir.path().join("run-id.txt");
    let body = format!(
        "ctx.exec({{ \"sh\", \"-c\", \"printf %s \\\"$GITMASTER_WORKFLOW_RUN\\\" > \\\"$0\\\"\", \"{}\" }}, {{ timeout = \"10s\" }})",
        s(&out)
    );
    write(
        &user,
        "probe-env.lua",
        &manual("probe-env", ", review = true", &body),
    );
    let mut h = Harness::open(&repo, &user);
    h.review("probe-env");
    let events = h.run("probe-env");
    assert_eq!(ends(&events), [WorkflowEnd::Done], "{events:#?}");
    let written = std::fs::read_to_string(&out).expect("the program wrote its environment");
    assert_eq!(written, run_id(&events).to_string());
}
