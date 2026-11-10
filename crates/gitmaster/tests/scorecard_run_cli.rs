//! The run that starts an agent is linked to the scorecards the agent publishes (D219): a
//! `gitmaster mcp` process started with `GITMASTER_WORKFLOW_RUN` stores it on each scorecard, one
//! started without it, or with a malformed value, stores none. Runs the real binary over stdio.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_gitmaster");

fn git(dir: &Path, args: &[&str]) -> String {
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
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// `main` with one commit and `feat` checked out, the tip being the commit a scorecard judges.
fn repo(dir: &Path) -> String {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    git(dir, &["checkout", "-q", "-b", "feat"]);
    git(dir, &["rev-parse", "HEAD"])
}

fn publish(head: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "scorecard_publish",
        "arguments": {
            "name": "security",
            "branch": "feat",
            "head": head,
            "status": "pass",
            "summary": { "label": "note", "decorations": [], "subject": "Looks fine", "body": "Checked." },
        }
    }})
}

/// Starts `gitmaster mcp` on `repo` with `run` as `GITMASTER_WORKFLOW_RUN` (or none), publishes one
/// scorecard, and returns the workflow run the scorecard stored.
fn stored_run(repo: &Path, home: &Path, run: Option<&str>) -> Value {
    let head = git(repo, &["rev-parse", "HEAD"]);
    let mut command = Command::new(BINARY);
    command
        .args(["mcp", "--repo"])
        .arg(repo)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("no-global-config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env_remove("GITMASTER_WORKFLOW_RUN");
    if let Some(run) = run {
        command.env("GITMASTER_WORKFLOW_RUN", run);
    }
    let mut child = command.spawn().expect("run gitmaster mcp");
    let requests = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        publish(&head),
    ];
    {
        let mut input = child.stdin.take().unwrap();
        for request in requests {
            writeln!(input, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    let reply = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|reply| reply["id"] == 2)
        .expect("a reply to the publish");
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    reply["result"]["structuredContent"]["scorecard"]["workflow_run"].clone()
}

#[test]
fn a_scorecard_published_by_a_run_links_to_that_run() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    repo(&repo_dir);
    assert_eq!(stored_run(&repo_dir, &home, Some("abc123")), "abc123");
}

#[test]
fn a_scorecard_published_without_a_run_stores_none() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    repo(&repo_dir);
    assert!(stored_run(&repo_dir, &home, None).is_null());
}

#[test]
fn a_malformed_run_id_is_dropped_and_the_publish_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    repo(&repo_dir);
    assert!(stored_run(&repo_dir, &home, Some("a;rm -rf /")).is_null());
}
