//! Worktree listing against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;

fn git_in(dir: &Path, args: &[&str]) {
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
}

/// A repository with one commit on `main`.
fn repo(dir: &Path) {
    git_in(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-q", "-m", "init"]);
}

fn canonical(path: &Path) -> String {
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn lists_the_main_checkout_and_a_linked_worktree_with_a_space_in_its_path() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("main repo");
    std::fs::create_dir(&main).unwrap();
    repo(&main);
    let linked = base.path().join("linked wt");
    git_in(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );

    let entries = SubprocessGit::default().worktrees(&main).unwrap();

    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(entries[0].is_main);
    assert_eq!(entries[0].path, canonical(&main));
    assert_eq!(entries[0].branch.as_deref(), Some("main"));
    assert!(!entries[1].is_main);
    assert_eq!(entries[1].path, canonical(&linked));
    assert_eq!(entries[1].branch.as_deref(), Some("feature"));
    assert!(
        entries
            .iter()
            .all(|e| e.head.is_some() && e.locked.is_none() && e.prunable.is_none())
    );
}

#[test]
fn reports_a_detached_locked_and_prunable_worktree() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("main");
    std::fs::create_dir(&main).unwrap();
    repo(&main);
    let detached = base.path().join("detached");
    let locked = base.path().join("locked");
    let gone = base.path().join("gone");
    for path in [&detached, &locked, &gone] {
        git_in(
            &main,
            &["worktree", "add", "-q", "--detach", path.to_str().unwrap()],
        );
    }
    git_in(
        &main,
        &[
            "worktree",
            "lock",
            "--reason",
            "on a USB stick",
            locked.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&gone).unwrap();

    let entries = SubprocessGit::default().worktrees(&main).unwrap();

    let by_path = |path: &Path| {
        entries
            .iter()
            .find(|e| e.path == canonical(path))
            .unwrap_or_else(|| panic!("{} not listed: {entries:?}", path.display()))
    };
    let d = by_path(&detached);
    assert!(d.detached && d.branch.is_none());
    let l = by_path(&locked);
    assert_eq!(l.locked.as_deref(), Some("on a USB stick"));
    let g = entries
        .iter()
        .find(|e| e.path.ends_with("gone"))
        .expect("the removed worktree is still listed");
    assert!(g.prunable.is_some(), "{g:?}");
}

#[test]
fn a_repository_outside_git_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SubprocessGit::default().worktrees(dir.path()).is_err());
}
