//! Listing and deleting the local branches merged into the base branch, through
//! the engine, against real repositories in temporary directories. Each test
//! removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, BranchFailure, Command, CommandKind, Event, MERGED_LIST_CHANGED, OperationId,
    OperationState,
};

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

/// A repository with one commit on `main`.
fn repo(base: &Path) -> PathBuf {
    repo_on(base, "main")
}

/// A repository with one commit on `branch`, the only branch it has.
fn repo_on(base: &Path, branch: &str) -> PathBuf {
    let dir = base.join(format!("repo-{branch}"));
    std::fs::create_dir(&dir).unwrap();
    git_in(&dir, &["init", "-q", "-b", branch]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git_in(&dir, &["add", "."]);
    git_in(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

fn commit(dir: &Path, subject: &str) {
    git_in(dir, &["commit", "-q", "--allow-empty", "-m", subject]);
}

fn command(actor: Actor, kind: CommandKind) -> Command {
    Command { actor, kind }
}

fn human(kind: CommandKind) -> Command {
    command(Actor::Human, kind)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn logged_engine(root: &Path, log: &CommandLog) -> Engine {
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo { path: text(root) }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn delete_merged(names: &[&str]) -> CommandKind {
    CommandKind::DeleteMergedBranches {
        branches: names.iter().map(|name| (*name).to_owned()).collect(),
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

/// The base and the listed branch names of the list answer.
fn listed(events: &[Event]) -> (Option<String>, Vec<String>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::MergedBranchesListed { base, branches, .. } => Some((
                base.clone(),
                branches.iter().map(|b| b.name.clone()).collect(),
            )),
            _ => None,
        })
        .expect("the list answers")
}

fn report(events: &[Event]) -> (Vec<String>, Vec<BranchFailure>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::MergedBranchesDeleted {
                deleted, failures, ..
            } => Some((deleted.clone(), failures.clone())),
            _ => None,
        })
        .expect("the delete reports")
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

fn list(engine: &mut Engine, id: u64) -> Vec<Event> {
    engine.handle(OperationId(id), &human(CommandKind::ListMergedBranches))
}

#[test]
fn a_merged_branch_is_listed_and_an_unmerged_one_is_not() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "merged"]);
    git_in(&main, &["checkout", "-q", "-b", "wip"]);
    commit(&main, "only here");
    git_in(&main, &["checkout", "-q", "main"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);

    let events = list(&mut engine, 2);

    assert!(done(&events), "{events:?}");
    let (base_used, names) = listed(&events);
    assert_eq!(base_used.as_deref(), Some("main"));
    assert_eq!(names, ["merged"]);
    let tip = git_in(&main, &["rev-parse", "--short", "merged"]);
    let shown = events
        .iter()
        .find_map(|e| match e {
            Event::MergedBranchesListed { branches, .. } => branches.first().cloned(),
            _ => None,
        })
        .unwrap();
    assert_eq!(shown.tip, tip);
    assert_eq!(shown.subject, "init");
    assert!(log.entries().is_empty(), "a list is a read, not a command");
}

#[test]
fn the_current_branch_is_never_listed_even_when_merged() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["checkout", "-q", "-b", "here"]);
    git_in(&main, &["branch", "other"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    let (base_used, names) = listed(&list(&mut engine, 2));

    assert_eq!(base_used.as_deref(), Some("main"));
    assert_eq!(names, ["other"], "here is checked out and stays");
}

#[test]
fn protected_names_are_never_listed_but_a_longer_name_is() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    for name in ["master", "trunk", "develop", "dev", "staging", "production"] {
        git_in(&main, &["branch", name]);
    }
    git_in(&main, &["branch", "release/1.0"]);
    git_in(&main, &["branch", "main-old"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    let (_, names) = listed(&list(&mut engine, 2));

    assert_eq!(names, ["main-old", "release/1.0"]);
}

#[test]
fn the_exact_name_release_is_protected() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "release"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    let (base_used, names) = listed(&list(&mut engine, 2));

    assert_eq!(base_used.as_deref(), Some("main"));
    assert!(names.is_empty(), "{names:?}");
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_never_listed() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "feature"]);
    git_in(&main, &["branch", "free"]);
    let target = base.path().join("repo-feature");
    git_in(
        &main,
        &["worktree", "add", "-q", target.to_str().unwrap(), "feature"],
    );
    let mut engine = logged_engine(&main, &CommandLog::default());

    let (_, names) = listed(&list(&mut engine, 2));

    assert_eq!(names, ["free"]);
}

#[test]
fn origin_head_chooses_the_base_and_without_it_main_does() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    // develop is one commit ahead of main; dev-work sits on that commit, so it is
    // merged into develop only; old sits on the first commit, merged into both.
    git_in(&main, &["branch", "old"]);
    git_in(&main, &["checkout", "-q", "-b", "develop"]);
    commit(&main, "ahead");
    git_in(&main, &["branch", "dev-work"]);
    git_in(&main, &["checkout", "-q", "main"]);
    let origin = base.path().join("origin.git");
    git_in(
        base.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git_in(
        &main,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&main, &["push", "-q", "origin", "main", "develop"]);
    git_in(&main, &["remote", "set-head", "origin", "develop"]);
    let mut engine = logged_engine(&main, &CommandLog::default());

    let (base_used, names) = listed(&list(&mut engine, 2));

    assert_eq!(base_used.as_deref(), Some("develop"));
    assert_eq!(names, ["dev-work", "old"]);

    git_in(&main, &["remote", "set-head", "origin", "--delete"]);
    let (base_used, names) = listed(&list(&mut engine, 3));
    assert_eq!(base_used.as_deref(), Some("main"), "the fallback chain");
    assert_eq!(names, ["old"]);
}

#[test]
fn the_fallback_chain_takes_the_first_existing_name_then_the_current_branch() {
    let base = tempfile::tempdir().unwrap();
    // (the only branch, the base the list must report)
    for (branch, expected) in [
        ("master", "master"),
        ("trunk", "trunk"),
        ("develop", "develop"),
    ] {
        let dir = repo_on(base.path(), branch);
        let mut engine = logged_engine(&dir, &CommandLog::default());
        let (base_used, _) = listed(&list(&mut engine, 2));
        assert_eq!(base_used.as_deref(), Some(expected), "{branch}");
    }
    // No name of the chain: the current branch is the base, and its other
    // merged branches are what is listed.
    let dir = repo_on(base.path(), "topic");
    git_in(&dir, &["branch", "other"]);
    let mut engine = logged_engine(&dir, &CommandLog::default());
    let (base_used, names) = listed(&list(&mut engine, 2));
    assert_eq!(base_used.as_deref(), Some("topic"));
    assert_eq!(names, ["other"]);
    // Detached, with no name of the chain: there is no base and nothing is listed.
    git_in(&dir, &["checkout", "-q", "--detach"]);
    let events = list(&mut engine, 3);
    assert_eq!(listed(&events), (None, Vec::new()));
}

#[test]
fn deleting_removes_exactly_the_confirmed_branches() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "a"]);
    git_in(&main, &["branch", "b"]);
    git_in(&main, &["branch", "c"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);
    let (_, names) = listed(&list(&mut engine, 2));
    assert_eq!(names, ["a", "b", "c"]);

    // Confirming part of the list is not the list that was shown: refused.
    let part = engine.handle(OperationId(3), &human(delete_merged(&["a", "b"])));
    assert_eq!(refusal(&part).as_deref(), Some(MERGED_LIST_CHANGED));
    assert!(branch_exists(&main, "a"), "a refusal deletes nothing");

    let events = engine.handle(OperationId(4), &human(delete_merged(&["a", "b", "c"])));

    assert!(done(&events), "{events:?}");
    for name in ["a", "b", "c"] {
        assert!(!branch_exists(&main, name), "{name} is gone");
    }
    let (deleted, failures) = report(&events);
    assert_eq!(deleted, ["a", "b", "c"]);
    assert!(failures.is_empty());
    let entries = log.entries();
    assert_eq!(
        entries.len(),
        3,
        "the list and the refusal were not logged: {entries:?}"
    );
    assert_eq!(entries[0].args, ["branch", "-d", "a"]);
    assert_eq!(entries[1].args, ["branch", "-d", "b"]);
    assert_eq!(entries[2].args, ["branch", "-d", "c"]);
}

#[test]
fn a_list_that_changed_is_refused_and_nothing_is_deleted() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "a"]);
    let mut engine = logged_engine(&main, &CommandLog::default());
    let (_, names) = listed(&list(&mut engine, 2));
    assert_eq!(names, ["a"]);

    // Someone merges one more branch after the list was shown.
    git_in(&main, &["branch", "late"]);
    let refused = engine.handle(OperationId(3), &human(delete_merged(&["a"])));

    assert_eq!(refusal(&refused).as_deref(), Some(MERGED_LIST_CHANGED));
    assert!(branch_exists(&main, "a"));
    assert!(branch_exists(&main, "late"));
}

#[test]
fn a_branch_checked_out_meanwhile_makes_the_list_change() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "a"]);
    let mut engine = logged_engine(&main, &CommandLog::default());
    let (_, names) = listed(&list(&mut engine, 2));
    assert_eq!(names, ["a"]);
    let target = base.path().join("repo-a");
    git_in(
        &main,
        &["worktree", "add", "-q", target.to_str().unwrap(), "a"],
    );

    let refused = engine.handle(OperationId(3), &human(delete_merged(&["a"])));

    assert_eq!(refusal(&refused).as_deref(), Some(MERGED_LIST_CHANGED));
    assert!(branch_exists(&main, "a"));
}

#[test]
fn an_unmerged_branch_is_never_deleted_even_when_it_is_named() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "a"]);
    git_in(&main, &["checkout", "-q", "-b", "wip"]);
    commit(&main, "only here");
    git_in(&main, &["checkout", "-q", "main"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);
    let (_, names) = listed(&list(&mut engine, 2));
    assert_eq!(names, ["a"]);

    let refused = engine.handle(OperationId(3), &human(delete_merged(&["a", "wip"])));

    assert_eq!(refusal(&refused).as_deref(), Some(MERGED_LIST_CHANGED));
    assert!(branch_exists(&main, "a"));
    assert!(branch_exists(&main, "wip"));
    assert!(log.entries().is_empty(), "git never ran");
}

#[test]
fn a_branch_git_refuses_is_reported_and_the_others_still_go() {
    let base = tempfile::tempdir().unwrap();
    // main moves on to c2; x sits on c2, so it is merged into main, but HEAD is
    // a side branch from c1 that does not contain c2, so `git branch -d x` refuses.
    let main = repo(base.path());
    commit(&main, "second");
    git_in(&main, &["branch", "x"]);
    git_in(&main, &["branch", "y", "main~1"]);
    git_in(&main, &["checkout", "-q", "-b", "side", "main~1"]);
    commit(&main, "side work");
    let mut engine = logged_engine(&main, &CommandLog::default());
    let (base_used, names) = listed(&list(&mut engine, 2));
    assert_eq!(base_used.as_deref(), Some("main"));
    assert_eq!(names, ["x", "y"]);

    let events = engine.handle(OperationId(3), &human(delete_merged(&["x", "y"])));

    assert!(
        done(&events),
        "a refused branch is a report, not a failed run"
    );
    assert!(!branch_exists(&main, "y"));
    assert!(branch_exists(&main, "x"));
    let (deleted, failures) = report(&events);
    assert_eq!(deleted, ["y"]);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name, "x");
    assert!(
        failures[0].message.contains("not fully merged"),
        "{failures:?}"
    );
}

#[test]
fn an_agent_cannot_delete_merged_branches() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "a"]);
    let log = CommandLog::default();
    let mut engine = logged_engine(&main, &log);
    let agent = Actor::Agent {
        name: "agent".into(),
    };

    let refused = engine.handle(OperationId(2), &command(agent, delete_merged(&["a"])));

    assert_eq!(
        refusal(&refused).as_deref(),
        Some("only a human can delete merged branches")
    );
    assert!(branch_exists(&main, "a"));
    assert!(log.entries().is_empty(), "git never ran");
}

#[test]
fn an_empty_base_list_gives_an_empty_list() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let mut engine = logged_engine(&main, &CommandLog::default());

    let events = list(&mut engine, 2);

    assert!(done(&events), "{events:?}");
    assert_eq!(listed(&events), (Some("main".to_owned()), Vec::new()));
}
