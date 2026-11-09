//! The change of a branch (D214 (1)) against real repositories in temporary directories: the base
//! is the stack parent, else the upstream unless it is the branch's own remote counterpart, else
//! `origin/HEAD`, else the first default branch that exists; the default branch has no change; and
//! a rename lists both of its paths. Each test removes its directory when it ends.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::change::{base_of, change_of};
use gm_core::git::SubprocessGit;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A repository on `main` with `a.txt` and `f.txt`, checked out on `feat` with `f.txt` changed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    std::fs::write(dir.path().join("f.txt"), "two\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "feat change"]);
    dir
}

/// Adds `origin` with `refs/remotes/origin/<name>` at the tip of local `<name>`.
fn remote_has(dir: &Path, name: &str) {
    git(dir, &["remote", "add", "origin", "../unused-remote"]);
    git(
        dir,
        &["update-ref", &format!("refs/remotes/origin/{name}"), name],
    );
}

fn base(dir: &Path, branch: &str) -> Option<String> {
    base_of(&SubprocessGit::default(), dir, branch).unwrap()
}

#[test]
fn the_stack_parent_wins_over_the_upstream_and_origin_head() {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "main"]);
    git(dir.path(), &["checkout", "-q", "-b", "parent"]);
    git(dir.path(), &["checkout", "-q", "feat"]);
    remote_has(dir.path(), "main");
    git(
        dir.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    SubprocessGit::default()
        .write_stack_link(dir.path(), "feat", Some(("parent", "main")))
        .unwrap();
    assert_eq!(base(dir.path(), "feat").as_deref(), Some("parent"));
    let change = change_of(&SubprocessGit::default(), dir.path(), "feat")
        .unwrap()
        .expect("a change");
    assert_eq!(change.base_ref, "parent");
    assert_eq!(change.paths, Some(vec!["f.txt".to_owned()]));
}

#[test]
fn an_upstream_that_is_the_own_remote_counterpart_is_ignored() {
    let dir = repo();
    // The branch was pushed: its own remote branch is its upstream, and points at its tip.
    remote_has(dir.path(), "feat");
    git(
        dir.path(),
        &["branch", "-q", "--set-upstream-to=origin/feat", "feat"],
    );
    assert_eq!(base(dir.path(), "feat").as_deref(), Some("main"));
    let change = change_of(&SubprocessGit::default(), dir.path(), "feat")
        .unwrap()
        .expect("a change against main");
    assert_eq!(change.base_ref, "main");
    assert_eq!(change.paths, Some(vec!["f.txt".to_owned()]));
}

#[test]
fn an_upstream_on_another_branch_is_the_base() {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "main"]);
    git(dir.path(), &["checkout", "-q", "-b", "other"]);
    git(dir.path(), &["checkout", "-q", "feat"]);
    remote_has(dir.path(), "other");
    git(
        dir.path(),
        &["branch", "-q", "--set-upstream-to=origin/other", "feat"],
    );
    assert_eq!(base(dir.path(), "feat").as_deref(), Some("origin/other"));
}

#[test]
fn origin_head_is_used_when_nothing_nearer_is_known() {
    let dir = repo();
    remote_has(dir.path(), "main");
    git(
        dir.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    assert_eq!(base(dir.path(), "feat").as_deref(), Some("origin/main"));
}

#[test]
fn main_is_the_fallback_when_there_is_no_remote() {
    let dir = repo();
    assert_eq!(base(dir.path(), "feat").as_deref(), Some("main"));
}

#[test]
fn the_default_branch_itself_has_no_base_and_no_change() {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "main"]);
    assert_eq!(base(dir.path(), "main"), None);
    assert!(
        change_of(&SubprocessGit::default(), dir.path(), "main")
            .unwrap()
            .is_none()
    );
    // Even with origin/HEAD naming the branch's own remote counterpart, it is not its own base.
    remote_has(dir.path(), "main");
    git(
        dir.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    assert_eq!(base(dir.path(), "main"), None);
    git(dir.path(), &["checkout", "-q", "feat"]);
    git(
        dir.path(),
        &["branch", "-q", "--set-upstream-to=origin/main", "main"],
    );
    assert_eq!(base(dir.path(), "main"), None);
}

#[test]
fn a_rename_lists_both_of_its_paths_in_the_change() {
    let dir = repo();
    git(dir.path(), &["mv", "a.txt", "renamed.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "rename"]);
    let change = change_of(&SubprocessGit::default(), dir.path(), "feat")
        .unwrap()
        .expect("a change");
    assert_eq!(
        change.paths,
        Some(vec![
            "a.txt".to_owned(),
            "f.txt".to_owned(),
            "renamed.txt".to_owned()
        ])
    );
}
