//! `ListRepoBranches` (D195) against real repositories: the branches of a repository the window has
//! not opened, a remote-only branch among them, and a path that is not a repository. The engine has
//! no repository open in any of these, so the read does not depend on the window's repository.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, BranchEntry, Command, CommandKind, Event, OperationId, OperationState};

fn git_in(dir: &Path, args: &[&str]) {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
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
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A repository `<base>/<name>` on `main` with one commit and a local `feat` branch.
fn repo(base: &Path, name: &str) -> PathBuf {
    let repo = base.join(name);
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    git_in(&repo, &["branch", "feat"]);
    repo
}

/// A repository whose `origin/<branch>` exists and whose local `<branch>` does not, as a clone of
/// a remote that has `branch` has.
fn repo_with_remote_branch(base: &Path, name: &str, branch: &str) -> PathBuf {
    let repo = repo(base, name);
    let origin = base.join(format!("origin-{name}.git"));
    git_in(base, &["init", "-q", "--bare", &text(&origin)]);
    git_in(&repo, &["remote", "add", "origin", &text(&origin)]);
    git_in(&repo, &["push", "-q", "origin", "main"]);
    git_in(&repo, &["branch", branch]);
    git_in(&repo, &["push", "-q", "origin", branch]);
    git_in(&repo, &["branch", "-q", "-D", branch]);
    repo
}

fn list(engine: &mut Engine, id: u64, repo: &str) -> Vec<Event> {
    let command = Command {
        actor: Actor::Human,
        kind: CommandKind::ListRepoBranches { repo: repo.into() },
    };
    engine.handle(OperationId(id), &command)
}

fn listed(events: &[Event]) -> Option<(String, Vec<BranchEntry>)> {
    events.iter().find_map(|event| match event {
        Event::RepoBranchesLoaded { repo, branches, .. } => Some((repo.clone(), branches.clone())),
        _ => None,
    })
}

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn names(branches: &[BranchEntry]) -> Vec<&str> {
    branches.iter().map(|branch| branch.name.as_str()).collect()
}

#[test]
fn lists_the_branches_of_a_repository_the_window_has_not_opened() {
    let base = tempfile::tempdir().unwrap();
    let repo = repo(&base.path().canonicalize().unwrap(), "billing");
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));

    let events = list(&mut engine, 2, &text(&repo));

    let (echoed, branches) = listed(&events).expect("a listing");
    assert_eq!(echoed, text(&repo), "the answer names its repository");
    assert_eq!(
        names(&branches),
        ["feat", "main"],
        "local branches in name order"
    );
    let main = branches.iter().find(|b| b.name == "main").unwrap();
    assert!(main.is_current, "main is checked out");
    assert_eq!(state_of(&events), Some(OperationState::Done));
    assert!(
        engine.log().is_empty(),
        "a read is not recorded in the command log (D28)"
    );
}

#[test]
fn a_remote_only_branch_is_listed_under_its_remote_name() {
    let base = tempfile::tempdir().unwrap();
    let repo = repo_with_remote_branch(&base.path().canonicalize().unwrap(), "pay", "pay-v2");
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));

    let events = list(&mut engine, 2, &text(&repo));

    let (_, branches) = listed(&events).expect("a listing");
    let remote = branches
        .iter()
        .find(|b| b.name == "origin/pay-v2")
        .expect("the remote-only branch");
    assert!(remote.is_remote);
    assert!(
        !branches.iter().any(|b| !b.is_remote && b.name == "pay-v2"),
        "there is no local pay-v2"
    );
}

#[test]
fn a_folder_that_is_not_a_repository_fails_with_a_reason() {
    let base = tempfile::tempdir().unwrap();
    let folder = base.path().canonicalize().unwrap().join("plain");
    std::fs::create_dir(&folder).unwrap();
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));

    let events = list(&mut engine, 2, &text(&folder));

    assert!(listed(&events).is_none(), "no listing for a plain folder");
    assert_eq!(state_of(&events), Some(OperationState::Failed));
}

#[test]
fn an_empty_path_is_refused_before_git_runs() {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));

    let events = list(&mut engine, 2, "   ");

    assert!(listed(&events).is_none());
    assert_eq!(state_of(&events), Some(OperationState::Failed));
}
