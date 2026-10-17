//! The history read against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::{History, RefKind};

/// Runs `git` in `dir` with a fixed identity; `when` sets both dates, so the
/// commits order by time without ties.
fn git_at(dir: &Path, when: &str, args: &[&str]) {
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
        .env("GIT_AUTHOR_DATE", when)
        .env("GIT_COMMITTER_DATE", when)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn git(dir: &Path, args: &[&str]) {
    git_at(dir, "2024-01-01T10:00:00+00:00", args);
}

fn commit(dir: &Path, when: &str, file: &str, subject: &str) {
    std::fs::write(dir.join(file), format!("{subject}\n")).unwrap();
    git(dir, &["add", file]);
    git_at(dir, when, &["commit", "-q", "-m", subject]);
}

fn history_of(dir: &Path) -> History {
    let root = SubprocessGit::default().resolve_root(dir).unwrap();
    SubprocessGit::default().history(&root).unwrap()
}

fn subjects(history: &History) -> Vec<&str> {
    history.commits.iter().map(|c| c.subject.as_str()).collect()
}

#[test]
fn lists_a_linear_history_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    commit(dir.path(), "2024-01-03T10:00:00+00:00", "c", "third");

    let history = history_of(dir.path());

    assert!(!history.unborn);
    assert_eq!(subjects(&history), ["third", "second", "first"]);
    let head = &history.commits[0];
    assert_eq!(head.oid.len(), 40);
    assert!(head.oid.starts_with(&head.short));
    assert_eq!(head.author_time, 1_704_276_000);
    assert_eq!(head.refs.len(), 2);
    assert_eq!(head.refs[0].name, "HEAD");
    assert_eq!(head.refs[0].kind, RefKind::Head);
    assert_eq!(head.refs[1].name, "main");
    assert_eq!(head.refs[1].kind, RefKind::Branch);
    assert!(history.commits[1].refs.is_empty());
}

#[test]
fn shows_a_merge_with_both_parents_in_the_list() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "base");
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "on feature");
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), "2024-01-03T10:00:00+00:00", "c", "on main");
    git_at(
        dir.path(),
        "2024-01-04T10:00:00+00:00",
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );

    let history = history_of(dir.path());

    assert_eq!(
        subjects(&history),
        ["merge feature", "on main", "on feature", "base"]
    );
    assert_eq!(history.commits[0].refs[0].kind, RefKind::Head);
}

#[test]
fn a_root_commit_is_listed_last() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "only");

    let history = history_of(dir.path());

    assert_eq!(subjects(&history), ["only"]);
    assert!(!history.unborn);
}

#[test]
fn an_unborn_head_is_reported_with_no_commits() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);

    let history = history_of(dir.path());

    assert!(history.unborn);
    assert!(history.commits.is_empty());
}

#[test]
fn a_detached_head_is_labelled_head_on_its_commit() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    git(dir.path(), &["checkout", "-q", "--detach", "HEAD~1"]);

    let history = history_of(dir.path());

    // HEAD is on "first", so the list is that commit alone, labelled HEAD only.
    assert_eq!(subjects(&history), ["first"]);
    let head = &history.commits[0];
    assert_eq!(head.refs.len(), 1);
    assert_eq!(head.refs[0].name, "HEAD");
    assert_eq!(head.refs[0].kind, RefKind::Head);
}

#[test]
fn a_unicode_subject_comes_through_whole() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(
        dir.path(),
        "2024-01-01T10:00:00+00:00",
        "a",
        "Ajusta arredondamento de moeda: çãé 日本語 🚀 and a very long tail of words that goes on",
    );

    let history = history_of(dir.path());

    assert_eq!(
        history.commits[0].subject,
        "Ajusta arredondamento de moeda: çãé 日本語 🚀 and a very long tail of words that goes on"
    );
}

#[test]
fn a_tag_is_a_label_of_its_commit() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "release");
    git(dir.path(), &["tag", "v1.0"]);

    let history = history_of(dir.path());

    let tag = history.commits[0]
        .refs
        .iter()
        .find(|r| r.kind == RefKind::Tag)
        .unwrap();
    assert_eq!(tag.name, "v1.0");
}

#[test]
fn reading_the_history_records_nothing_and_writes_no_commit_graph() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    let log = gm_core::command_log::CommandLog::default();
    let git_port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let root = git_port.resolve_root(dir.path()).unwrap();

    git_port.history(&root).unwrap();

    assert!(log.entries().is_empty());
    assert!(!dir.path().join(".git/objects/info/commit-graph").exists());
}
