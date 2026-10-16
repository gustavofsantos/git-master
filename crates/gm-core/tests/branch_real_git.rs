//! Branch listing and checkout against real repositories in temporary
//! directories. The remote is a second repository on disk, cloned to get
//! `origin`.

use std::path::{Path, PathBuf};
use std::process::Command;

use gm_core::GitPort;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::BranchEntry;

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
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

fn current(dir: &Path) -> String {
    git_in(dir, &["branch", "--show-current"])
        .trim_end()
        .to_owned()
}

/// A repository with one commit on `main` holding `a.txt`.
fn origin_repo(dir: &Path) {
    git_in(dir, &["init", "-q", "-b", "main"]);
    write(dir, "a.txt", "one\n");
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-q", "-m", "init"]);
}

/// `origin` has `main` and `remote-only`. The clone, returned as its path,
/// has `main`, a local `wip`, and `origin/main` plus `origin/remote-only`.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let base = tempfile::tempdir().unwrap();
    let origin = base.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    origin_repo(&origin);
    git_in(&origin, &["branch", "remote-only"]);

    let clone = base.path().join("clone");
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .unwrap();
    assert!(status.success());
    git_in(&clone, &["branch", "wip"]);
    git_in(&clone, &["fetch", "-q", "origin"]);
    (base, clone)
}

fn entry(name: &str, is_remote: bool, is_current: bool, upstream: Option<&str>) -> BranchEntry {
    BranchEntry {
        name: name.into(),
        is_remote,
        is_current,
        upstream: upstream.map(str::to_owned),
    }
}

fn logged_port() -> (SubprocessGit, CommandLog) {
    let log = CommandLog::default();
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    (port, log)
}

#[test]
fn lists_local_then_remote_branches_and_flags_the_current_one() {
    let (_base, clone) = fixture();

    let branches = SubprocessGit::default().branches(&clone).unwrap();

    assert_eq!(
        branches,
        [
            entry("main", false, true, Some("origin/main")),
            entry("wip", false, false, None),
            entry("origin/main", true, false, None),
            entry("origin/remote-only", true, false, None),
        ]
    );
}

#[test]
fn switches_to_a_local_branch() {
    let (_base, clone) = fixture();

    SubprocessGit::default().checkout(&clone, "wip").unwrap();

    assert_eq!(current(&clone), "wip");
}

#[test]
fn a_remote_only_branch_gets_a_local_tracking_branch() {
    let (_base, clone) = fixture();

    SubprocessGit::default()
        .checkout(&clone, "origin/remote-only")
        .unwrap();

    assert_eq!(current(&clone), "remote-only");
    let upstream = git_in(
        &clone,
        &["rev-parse", "--abbrev-ref", "remote-only@{upstream}"],
    );
    assert_eq!(upstream.trim_end(), "origin/remote-only");
}

#[test]
fn a_remote_branch_with_a_local_namesake_switches_to_the_local_one() {
    let (_base, clone) = fixture();
    git_in(&clone, &["branch", "remote-only", "origin/main"]);

    SubprocessGit::default()
        .checkout(&clone, "origin/remote-only")
        .unwrap();

    assert_eq!(current(&clone), "remote-only");
    assert_eq!(
        git_in(&clone, &["rev-parse", "HEAD"]),
        git_in(&clone, &["rev-parse", "origin/main"])
    );
}

#[test]
fn a_refused_checkout_returns_git_error_and_leaves_head_alone() {
    let (_base, clone) = fixture();
    // `wip` changes a.txt; `main` gets an uncommitted change to the same file.
    git_in(&clone, &["switch", "-q", "wip"]);
    write(&clone, "a.txt", "wip\n");
    git_in(&clone, &["commit", "-q", "-am", "wip change"]);
    git_in(&clone, &["switch", "-q", "main"]);
    write(&clone, "a.txt", "dirty\n");

    let error = SubprocessGit::default()
        .checkout(&clone, "wip")
        .unwrap_err();

    assert!(error.0.contains("would be overwritten"), "{error}");
    assert_eq!(current(&clone), "main");
    assert_eq!(
        std::fs::read_to_string(clone.join("a.txt")).unwrap(),
        "dirty\n"
    );
    assert!(!clone.join(".git/index.lock").exists());
}

#[test]
fn a_name_starting_with_a_dash_is_rejected_before_git_runs() {
    let (_base, clone) = fixture();
    let (port, log) = logged_port();

    let error = port.checkout(&clone, "--detach").unwrap_err();

    assert!(error.0.contains("invalid branch name"), "{error}");
    assert_eq!(current(&clone), "main");
    assert!(log.entries().is_empty());
}

#[test]
fn an_empty_name_is_rejected() {
    let (_base, clone) = fixture();
    assert!(SubprocessGit::default().checkout(&clone, "").is_err());
}

#[test]
fn listing_records_nothing_and_checkout_is_logged_as_switch() {
    let (_base, clone) = fixture();
    let (port, log) = logged_port();

    port.branches(&clone).unwrap();
    assert!(log.entries().is_empty());

    port.checkout(&clone, "wip").unwrap();

    let entries = log.entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].args[0], "switch");
    assert!(entries[0].succeeded());
}

#[cfg(unix)]
#[test]
fn checkout_runs_the_post_checkout_hook() {
    use std::os::unix::fs::PermissionsExt;
    let (_base, clone) = fixture();
    let hook = clone.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        "#!/bin/sh\ntouch \"$(git rev-parse --git-dir)/hook-ran\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    SubprocessGit::default().checkout(&clone, "wip").unwrap();

    assert!(clone.join(".git/hook-ran").exists());
}
