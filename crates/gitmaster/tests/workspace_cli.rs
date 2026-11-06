//! Runs the built `gitmaster` binary for the one-shot `workspace` commands
//! (docs/reference/cli.md, "Workspaces"). The commands themselves are covered with real Git in
//! `gm-core/tests/workspace_cli_real_git.rs`; this checks that the binary routes to them.

use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_gitmaster");

fn gitmaster(args: &[&str], home: &std::path::Path) -> Output {
    Command::new(BINARY)
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env_remove("WORKTREES_HOME")
        .env("GIT_CONFIG_GLOBAL", home.join("no-global-config"))
        .output()
        .expect("run gitmaster")
}

#[test]
fn the_binary_creates_and_lists_a_workspace() {
    let base = tempfile::tempdir().expect("scratch dir");
    let base = base.path().canonicalize().expect("canonical");
    let root = base.join("ws");
    let root_arg = root.to_str().expect("utf-8 path");

    let created = gitmaster(&["workspace", "create", "alpha", "--root", root_arg], &base);
    assert_eq!(created.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&created.stdout),
        format!("{}\n", root.join("alpha").display())
    );

    let listed = gitmaster(
        &["workspace", "list", "--root", root_arg, "--format", "names"],
        &base,
    );
    assert_eq!(listed.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "alpha\n");
}

#[test]
fn the_binary_reports_a_wrong_command_line_with_status_two() {
    let base = tempfile::tempdir().expect("scratch dir");
    let output = gitmaster(&["workspace", "nope"], base.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: gitmaster workspace"));
}

#[test]
fn the_binary_prints_usage_with_status_two_for_no_arguments() {
    let base = tempfile::tempdir().expect("scratch dir");
    let output = gitmaster(&["workspace"], base.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: gitmaster workspace"));
}
