//! Creating and deleting local branches through the engine, against real
//! repositories in temporary directories. Each test removes its directory when
//! it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

fn git_in(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Git's own stderr for `args`, which must fail.
fn git_refusal(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!output.status.success(), "git {args:?} should fail");
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// A repository with one commit on `main` and a tag `v1` on it.
fn repo(base: &Path) -> PathBuf {
    let main = base.join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    git_in(&main, &["tag", "v1"]);
    main
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn command(actor: Actor, kind: CommandKind) -> Command {
    Command { actor, kind }
}

fn logged_engine(root: &Path, log: &CommandLog) -> Engine {
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &command(Actor::Human, CommandKind::OpenRepo { path: text(root) }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn create(name: &str, start_point: Option<&str>, checkout: bool) -> CommandKind {
    CommandKind::CreateBranch {
        name: name.into(),
        start_point: start_point.map(str::to_owned),
        checkout,
    }
}

fn delete(name: &str, force: bool) -> CommandKind {
    CommandKind::DeleteBranch {
        name: name.into(),
        force,
    }
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

fn branch_exists(dir: &Path, name: &str) -> bool {
    Process::new("git")
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{name}"))
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success()
}

fn current(dir: &Path) -> String {
    git_in(dir, &["branch", "--show-current"])
}

#[test]
fn a_new_branch_starts_at_head_and_head_stays_put() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let head = git_in(&main, &["rev-parse", "HEAD"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);

    let events = engine.handle(
        OperationId(2),
        &command(Actor::Human, create("topic", None, false)),
    );

    assert!(done(&events), "{events:?}");
    assert_eq!(git_in(&main, &["rev-parse", "topic"]), head);
    assert_eq!(current(&main), "main");
    let listed = events.iter().find_map(|e| match e {
        Event::BranchesLoaded { branches, .. } => Some(branches.clone()),
        _ => None,
    });
    let names: Vec<String> = listed
        .expect("the branch list follows the change")
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"topic".to_owned()), "{names:?}");
    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].succeeded());
    assert_eq!(entries[0].args, ["branch", "topic"]);
}

#[test]
fn a_new_branch_starts_at_a_tag_or_another_branch() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["commit", "-q", "--allow-empty", "-m", "second"]);
    git_in(&main, &["branch", "side"]);
    let tag = git_in(&main, &["rev-parse", "v1^{commit}"]);
    let side = git_in(&main, &["rev-parse", "side"]);
    let head = git_in(&main, &["rev-parse", "HEAD"]);
    assert_ne!(tag, head);
    let mut engine = logged_engine(&main, &CommandLog::default());

    assert!(done(&engine.handle(
        OperationId(2),
        &command(Actor::Human, create("from-tag", Some("v1"), false)),
    )));
    assert!(done(&engine.handle(
        OperationId(3),
        &command(Actor::Human, create("from-side", Some("side"), false)),
    )));

    assert_eq!(git_in(&main, &["rev-parse", "from-tag"]), tag);
    assert_eq!(git_in(&main, &["rev-parse", "from-side"]), side);

    let first = git_in(&main, &["rev-parse", "HEAD~1"]);
    assert!(done(&engine.handle(
        OperationId(4),
        &command(Actor::Human, create("from-commit", Some(&first), false)),
    )));
    assert_eq!(git_in(&main, &["rev-parse", "from-commit"]), first);
}

#[test]
fn a_new_branch_with_checkout_is_checked_out_at_once() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);

    let events = engine.handle(
        OperationId(2),
        &command(Actor::Human, create("topic", Some("v1"), true)),
    );

    assert!(done(&events), "{events:?}");
    assert_eq!(current(&main), "topic");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StatusLoaded { status, .. } if status.branch.head.as_deref() == Some("topic"))),
        "the status shows the new branch"
    );
    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].args, ["checkout", "-b", "topic", "v1"]);
}

#[test]
fn an_invalid_name_is_refused_before_git_runs_and_an_existing_one_gets_gits_message() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);

    let spaced = engine.handle(
        OperationId(2),
        &command(Actor::Human, create("two words", None, false)),
    );
    assert_eq!(
        refusal(&spaced).as_deref(),
        Some("A branch name cannot contain spaces.")
    );
    let dashed = engine.handle(
        OperationId(3),
        &command(Actor::Human, create("-x", None, true)),
    );
    assert_eq!(
        refusal(&dashed).as_deref(),
        Some("A branch name cannot start with '-'.")
    );
    assert!(
        log.entries().is_empty(),
        "no git command ran for an invalid name"
    );

    let existing = engine.handle(
        OperationId(4),
        &command(Actor::Human, create("main", None, false)),
    );
    let gits = git_refusal(&main, &["branch", "main"]);
    assert_eq!(refusal(&existing).as_deref(), Some(gits.as_str()));
    assert!(gits.contains("already exists"), "{gits}");
    assert_eq!(current(&main), "main");
}

#[test]
fn deleting_a_merged_branch_removes_it() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "merged"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);

    let events = engine.handle(
        OperationId(2),
        &command(Actor::Human, delete("merged", false)),
    );

    assert!(done(&events), "{events:?}");
    assert!(!branch_exists(&main, "merged"));
    assert_eq!(log.entries()[0].args, ["branch", "-d", "merged"]);
}

#[test]
fn an_unmerged_branch_survives_a_safe_delete_and_goes_on_force() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["checkout", "-q", "-b", "wip"]);
    git_in(&main, &["commit", "-q", "--allow-empty", "-m", "only here"]);
    git_in(&main, &["checkout", "-q", "main"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    let safe = engine.handle(OperationId(2), &command(Actor::Human, delete("wip", false)));

    let message = refusal(&safe).expect("git refuses an unmerged branch");
    assert!(message.contains("not fully merged"), "{message}");
    assert!(branch_exists(&main, "wip"), "the branch survives");

    let forced = engine.handle(OperationId(3), &command(Actor::Human, delete("wip", true)));

    assert!(done(&forced), "{forced:?}");
    assert!(!branch_exists(&main, "wip"));
}

#[test]
fn the_checked_out_branch_cannot_be_deleted_even_with_force() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let mut engine = logged_engine(&main, &CommandLog::default());

    let events = engine.handle(OperationId(2), &command(Actor::Human, delete("main", true)));

    assert_eq!(
        refusal(&events).as_deref(),
        Some("Cannot delete the checked-out branch main.")
    );
    assert!(branch_exists(&main, "main"));
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_refused_naming_its_path() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "feature"]);
    let target = base.path().join("repo-feature");
    git_in(
        &main,
        &["worktree", "add", "-q", target.to_str().unwrap(), "feature"],
    );
    let folder = target.canonicalize().unwrap();
    let mut engine = logged_engine(&main, &CommandLog::default());

    let events = engine.handle(
        OperationId(2),
        &command(Actor::Human, delete("feature", true)),
    );

    let message = refusal(&events).expect("the delete is refused");
    assert_eq!(
        message,
        format!(
            "Cannot delete feature: it is checked out in the worktree at {}.",
            folder.display()
        )
    );
    assert!(branch_exists(&main, "feature"));
}

#[test]
fn an_agent_cannot_create_or_delete_a_branch_in_a_real_repository() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);
    let agent = Actor::Agent {
        name: "agent".into(),
    };

    let created = engine.handle(
        OperationId(2),
        &command(agent.clone(), create("x", None, false)),
    );
    let deleted = engine.handle(OperationId(3), &command(agent, delete("main", true)));

    assert_eq!(
        refusal(&created).as_deref(),
        Some("only a human can create a branch")
    );
    assert_eq!(
        refusal(&deleted).as_deref(),
        Some("only a human can delete a branch")
    );
    assert!(!branch_exists(&main, "x"));
    assert!(log.entries().is_empty(), "git never ran");
}

#[test]
fn the_engine_log_records_both_commands_with_their_outcome() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "gone"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    engine.handle(
        OperationId(2),
        &command(Actor::Human, create("topic", None, false)),
    );
    engine.handle(
        OperationId(3),
        &command(Actor::Human, delete("gone", false)),
    );

    let kinds: Vec<(OperationId, OperationState)> = engine
        .log()
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                CommandKind::CreateBranch { .. } | CommandKind::DeleteBranch { .. }
            )
        })
        .filter(|entry| entry.state == OperationState::Done)
        .map(|entry| (entry.id, entry.state))
        .collect();
    assert_eq!(
        kinds,
        [
            (OperationId(2), OperationState::Done),
            (OperationId(3), OperationState::Done)
        ]
    );
}
