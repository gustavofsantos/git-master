//! The subprocess adapter against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::Change;

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

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

#[test]
fn reports_staged_unstaged_and_untracked() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("new file.txt"), "x\n").unwrap();

    let root = SubprocessGit::default().resolve_root(dir.path()).unwrap();
    let status = SubprocessGit::default().status(&root).unwrap();

    assert_eq!(status.branch.head.as_deref(), Some("main"));
    let find = |p: &str| status.entries.iter().find(|e| e.path == p).unwrap();
    assert_eq!(find("a.txt").staged, Some(Change::Modified));
    assert_eq!(find("b.txt").unstaged, Some(Change::Modified));
    assert!(find("new file.txt").untracked);
}

#[test]
fn status_does_not_touch_the_index() {
    let dir = repo();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    let index = dir.path().join(".git/index");
    let before = std::fs::read(&index).unwrap();
    SubprocessGit::default().status(dir.path()).unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before);
    assert!(!dir.path().join(".git/index.lock").exists());
}

#[test]
fn a_plain_directory_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SubprocessGit::default().resolve_root(dir.path()).is_err());
}

/// A repository on `main` with `a.txt` and `b.txt`, then a branch `feat` with a rename of `a.txt`
/// and a delete of `b.txt` committed.
fn renamed_and_deleted() -> tempfile::TempDir {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    git(dir.path(), &["mv", "a.txt", "renamed.txt"]);
    git(dir.path(), &["rm", "-q", "b.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "rename and delete"]);
    dir
}

#[test]
fn changed_paths_lists_both_sides_of_a_rename_and_a_delete_sorted() {
    let dir = renamed_and_deleted();
    let adapter = SubprocessGit::default();
    let from = adapter.resolve_commit(dir.path(), "main").unwrap();
    let to = adapter.resolve_commit(dir.path(), "feat").unwrap();
    assert_eq!(
        adapter.changed_paths(dir.path(), &from, &to).unwrap(),
        Some(vec![
            "a.txt".to_owned(),
            "b.txt".to_owned(),
            "renamed.txt".to_owned()
        ])
    );
}

#[test]
fn merge_base_of_commits_is_the_fork_point() {
    let dir = renamed_and_deleted();
    let adapter = SubprocessGit::default();
    let main = adapter.resolve_commit(dir.path(), "main").unwrap();
    let feat = adapter.resolve_commit(dir.path(), "feat").unwrap();
    assert_eq!(
        adapter
            .merge_base_of_commits(dir.path(), &main, &feat)
            .unwrap(),
        Some(main.clone())
    );
}

#[test]
fn a_branch_description_round_trips_with_newlines_and_quotes_and_unsets() {
    let dir = repo();
    let adapter = SubprocessGit::default();
    assert_eq!(
        adapter.branch_description(dir.path(), "main").unwrap(),
        None
    );
    let text = "first line\nsecond \"quoted\" line\n-leading dash";
    adapter
        .set_branch_description(dir.path(), "main", Some(text))
        .unwrap();
    assert_eq!(
        adapter
            .branch_description(dir.path(), "main")
            .unwrap()
            .as_deref(),
        Some(text)
    );
    adapter
        .set_branch_description(dir.path(), "main", None)
        .unwrap();
    assert_eq!(
        adapter.branch_description(dir.path(), "main").unwrap(),
        None
    );
    // Unsetting a key that is not set is not an error, and neither is an empty text.
    adapter
        .set_branch_description(dir.path(), "main", None)
        .unwrap();
    adapter
        .set_branch_description(dir.path(), "main", Some(""))
        .unwrap();
    assert_eq!(
        adapter.branch_description(dir.path(), "main").unwrap(),
        None
    );
}

#[test]
fn a_description_of_a_branch_with_a_slash_is_kept_under_its_name() {
    let dir = repo();
    git(dir.path(), &["branch", "feat/billing.v2"]);
    let adapter = SubprocessGit::default();
    adapter
        .set_branch_description(dir.path(), "feat/billing.v2", Some("the intent"))
        .unwrap();
    assert_eq!(
        adapter
            .branch_description(dir.path(), "feat/billing.v2")
            .unwrap()
            .as_deref(),
        Some("the intent")
    );
    assert_eq!(
        adapter.branch_description(dir.path(), "main").unwrap(),
        None
    );
}

#[test]
fn a_description_needs_a_local_branch_that_exists() {
    let dir = repo();
    let error = SubprocessGit::default()
        .set_branch_description(dir.path(), "nope", Some("text"))
        .unwrap_err();
    assert!(
        error.0.contains("no local branch named nope"),
        "{}",
        error.0
    );
}

#[test]
fn a_branch_without_an_upstream_has_none_and_one_with_it_names_it() {
    let dir = repo();
    git(dir.path(), &["remote", "add", "origin", "../unused-remote"]);
    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    let adapter = SubprocessGit::default();
    assert_eq!(adapter.upstream_of(dir.path(), "main").unwrap(), None);
    git(
        dir.path(),
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    assert_eq!(
        adapter.upstream_of(dir.path(), "main").unwrap().as_deref(),
        Some("origin/main")
    );
    assert_eq!(adapter.upstream_of(dir.path(), "missing").unwrap(), None);
}
