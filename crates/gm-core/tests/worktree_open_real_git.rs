//! Opening a linked worktree the way its window does: `OpenRepo` on its folder.
//! The status the window then shows must be the status `git -C <worktree>`
//! reports. Temporary directories are removed when the test ends.

use std::path::Path;
use std::process::Command;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, Command as EngineCommand, CommandKind, Event, OperationId};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_linked_worktree_opens_on_its_own_branch_and_status() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    let wt = base.path().join("repo-feature");
    git_in(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature/x",
            wt.to_str().unwrap(),
        ],
    );
    // A change only in the new worktree: the main checkout must not show it.
    std::fs::write(wt.join("a.txt"), "two\n").unwrap();
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();

    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let events = engine.handle(
        OperationId(1),
        &EngineCommand {
            actor: Actor::Human,
            kind: CommandKind::OpenRepo {
                path: wt.to_str().unwrap().into(),
            },
        },
    );
    let status = events
        .iter()
        .find_map(|event| match event {
            Event::StatusLoaded { status, .. } => Some(status.clone()),
            _ => None,
        })
        .expect("opening the worktree loads its status");

    // The porcelain paths start after the two status letters and a space.
    let git_status = git_in(&wt, &["status", "--porcelain"]);
    let mut git_paths: Vec<&str> = git_status.lines().map(|line| &line[3..]).collect();
    git_paths.sort_unstable();
    let mut paths: Vec<&str> = status.entries.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(status.branch.head.as_deref(), Some("feature/x"));
    assert_eq!(
        paths, git_paths,
        "git status in the worktree: {git_status:?}"
    );
    assert_eq!(paths, ["a.txt", "new.txt"]);
    assert_eq!(
        Path::new(&status.root).canonicalize().unwrap(),
        wt.canonicalize().unwrap()
    );
}
