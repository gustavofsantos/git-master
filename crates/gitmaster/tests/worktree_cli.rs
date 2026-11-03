//! Runs the built `gitmaster` binary for the one-shot `worktree` commands
//! (docs/reference/cli.md, "Worktrees"). The commands themselves are covered with real Git in
//! `gm-core/tests/worktree_cli_real_git.rs`; this checks that the binary routes to them.

use std::path::Path;
use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_gitmaster");

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn gitmaster(args: &[&str], dir: &Path, home: &Path) -> Output {
    Command::new(BINARY)
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env_remove("WORKTREES_HOME")
        .env("GIT_CONFIG_GLOBAL", home.join("no-global-config"))
        .output()
        .expect("run gitmaster")
}

#[test]
fn the_binary_adds_and_lists_a_worktree() {
    let base = tempfile::tempdir().expect("scratch dir");
    let base = base.path().canonicalize().expect("canonical");
    let repo = base.join("repo");
    std::fs::create_dir(&repo).expect("repo dir");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let added = gitmaster(&["worktree", "add", "feature/x"], &repo, &base);
    assert_eq!(added.status.code(), Some(0));
    let path = base.join("repo-feature-x");
    assert_eq!(
        String::from_utf8_lossy(&added.stdout),
        format!("{}\n", path.display())
    );

    let listed = gitmaster(&["worktree", "list", "--format", "branches"], &repo, &base);
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "main\nfeature/x\n");
}

#[test]
fn the_binary_reports_a_wrong_command_line_with_status_two() {
    let base = tempfile::tempdir().expect("scratch dir");
    let output = gitmaster(&["worktree", "nope"], base.path(), base.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: gitmaster worktree"));
}
