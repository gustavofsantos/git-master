//! Scorecards (D219) through the MCP server end to end: JSON-RPC lines in, a real repository and
//! its `.git/gitmaster/` store behind them. A publish is refused as a whole when a check fails,
//! a stale scorecard is listed only when asked, and a comment's label gives its kind.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_mcp::Session;
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A repository on `main` with `f.txt` (five lines), checked out on `feat`.
fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    git(dir, &["checkout", "-q", "-b", "feat"]);
}

/// Runs the server on `requests` for the repository at `repo`, and returns its replies.
fn session(repo: &Path, requests: &[Value]) -> Vec<Value> {
    let engine = Engine::new(Box::new(SubprocessGit::default()));
    let mut server = Session::open(engine, &repo.to_string_lossy()).expect("the repository opens");
    let input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = Vec::new();
    gm_mcp::serve(&mut server, input.as_bytes(), &mut output).expect("the output is written");
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } })
}

fn initialize() -> Vec<Value> {
    vec![
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                            "clientInfo": { "name": "test-client", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    ]
}

/// Runs `calls` after the handshake, and returns the replies to them.
fn run(dir: &Path, calls: Vec<Value>) -> Vec<Value> {
    let mut requests = initialize();
    requests.extend(calls);
    session(dir, &requests).into_iter().skip(1).collect()
}

fn result(reply: &Value) -> &Value {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    &reply["result"]["structuredContent"]
}

fn refusal(reply: &Value) -> String {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn publish_args(head: &str) -> Value {
    json!({
        "name": "security",
        "branch": "feat",
        "head": head,
        "status": "needs_work",
        "summary": {
            "label": "issue",
            "decorations": ["blocking", "security"],
            "subject": "Unchecked input",
            "body": "The parser trusts its input."
        },
        "comments": [{
            "path": "f.txt",
            "start_line": 2,
            "end_line": 2,
            "label": "issue",
            "decorations": ["blocking"],
            "body": "The line is not checked.",
            "suggested_fix": "check the length first"
        }]
    })
}

#[test]
fn the_scorecard_tools_are_listed_and_the_verdict_tools_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = session(
        dir.path(),
        &[
            initialize(),
            vec![json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })],
        ]
        .concat(),
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for name in [
        "scorecard_publish",
        "scorecards_list",
        "review_comment_create",
    ] {
        assert!(names.contains(&name), "{name} is listed: {names:?}");
    }
    assert!(
        names
            .iter()
            .all(|name| !name.contains("verdict") && !name.contains("reviewer")),
        "the verdict and reviewer tools are gone: {names:?}"
    );
    let by_name = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
    assert_eq!(
        by_name("scorecards_list")["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(
        by_name("scorecard_publish")["annotations"]["readOnlyHint"],
        false
    );
    assert_eq!(
        by_name("scorecard_publish")["annotations"]["destructiveHint"],
        false
    );
    assert_eq!(
        by_name("scorecard_publish")["inputSchema"]["required"],
        json!(["name", "branch", "head", "status", "summary"])
    );
}

#[test]
fn a_publish_writes_the_scorecard_and_its_comment_and_returns_their_ids() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let replies = run(
        dir.path(),
        vec![
            call(2, "scorecard_publish", &publish_args(&head)),
            call(3, "review_comments_list", &json!({ "author": "any" })),
        ],
    );
    let published = result(&replies[0]);
    let scorecard = &published["scorecard"];
    assert_eq!(scorecard["name"], "security");
    assert_eq!(scorecard["status"], "needs_work");
    assert_eq!(scorecard["head"], head);
    assert_eq!(scorecard["stale"], false);
    assert_eq!(scorecard["blocking_open"], 1);
    assert_eq!(scorecard["origin"], "agent:test-client");
    assert_eq!(scorecard["untrusted"], true);
    assert_eq!(published["comment_ids"].as_array().unwrap().len(), 1);

    let listed = result(&replies[1])["comments"].as_array().unwrap().clone();
    assert_eq!(listed.len(), 1, "{listed:?}");
    let comment = &listed[0];
    assert_eq!(comment["label"], "issue");
    assert_eq!(comment["decorations"], json!(["blocking"]));
    assert_eq!(comment["scorecard"], scorecard["id"]);
    assert_eq!(comment["kind"], "flag");
    assert_eq!(comment["severity"], "high");
    assert_eq!(comment["suggested_fix"], "check the length first");
    assert_eq!(comment["author"], "agent:test-client");
}

#[test]
fn a_publish_on_a_head_that_is_not_the_tip_is_refused_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let older = git(dir.path(), &["rev-parse", "HEAD"]);
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    let replies = run(
        dir.path(),
        vec![
            call(2, "scorecard_publish", &publish_args(&older)),
            call(3, "scorecards_list", &json!({ "include_stale": true })),
        ],
    );
    let reason = refusal(&replies[0]);
    assert!(reason.contains("is not the tip of feat"), "{reason}");
    assert_eq!(result(&replies[1])["scorecards"], json!([]));
}

#[test]
fn a_bad_argument_or_an_unreadable_comment_refuses_the_whole_publish() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let mut calls = Vec::new();
    let mut bad_name = publish_args(&head);
    bad_name["name"] = json!("Security Review");
    calls.push(bad_name);
    let mut bad_status = publish_args(&head);
    bad_status["status"] = json!("needs-work");
    calls.push(bad_status);
    let mut bad_label = publish_args(&head);
    bad_label["summary"]["label"] = json!("nit");
    calls.push(bad_label);
    let mut bad_comment_label = publish_args(&head);
    bad_comment_label["comments"][0]["label"] = json!("nit");
    calls.push(bad_comment_label);
    let mut four_words = publish_args(&head);
    four_words["summary"]["decorations"] = json!(["auth", "api", "db", "cache"]);
    calls.push(four_words);
    let mut outside = publish_args(&head);
    outside["comments"][0]["start_line"] = json!(4);
    outside["comments"][0]["end_line"] = json!(9);
    calls.push(outside);
    let mut too_many = publish_args(&head);
    let one = too_many["comments"][0].clone();
    too_many["comments"] = json!(vec![one; 31]);
    calls.push(too_many);
    let mut valid_first = publish_args(&head);
    valid_first["comments"] = json!([
        publish_args(&head)["comments"][0].clone(),
        { "path": "f.txt", "start_line": 4, "end_line": 9, "label": "issue", "body": "x" }
    ]);
    calls.push(valid_first);

    let mut requests: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, args)| call(10 + index as u64, "scorecard_publish", args))
        .collect();
    requests.push(call(
        30,
        "scorecards_list",
        &json!({ "include_stale": true }),
    ));
    let replies = run(dir.path(), requests);
    let reasons: Vec<String> = replies[..calls.len()].iter().map(refusal).collect();
    assert!(
        reasons[0].contains("a scorecard name is 1 to 40"),
        "{reasons:?}"
    );
    assert!(
        reasons[1].contains("status must be pass, needs_work or fail"),
        "{reasons:?}"
    );
    assert!(reasons[2].contains("label must be one of"), "{reasons:?}");
    assert!(reasons[3].contains("label must be one of"), "{reasons:?}");
    assert!(
        reasons[4].contains("at most 3 decorations of your own"),
        "{reasons:?}"
    );
    assert!(reasons[5].contains("not in the file"), "{reasons:?}");
    assert!(reasons[6].contains("at most 30 comments"), "{reasons:?}");
    assert!(reasons[7].contains("not in the file"), "{reasons:?}");
    assert_eq!(result(replies.last().unwrap())["scorecards"], json!([]));
}

#[test]
fn a_comment_text_that_appears_twice_refuses_the_publish() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    std::fs::write(dir.path().join("f.txt"), "same\nother\nsame\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "repeat"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let mut args = publish_args(&head);
    args["comments"][0]["start_line"] = json!(1);
    args["comments"][0]["end_line"] = json!(1);
    let replies = run(dir.path(), vec![call(2, "scorecard_publish", &args)]);
    let reason = refusal(&replies[0]);
    assert!(reason.contains("f.txt"), "{reason}");
}

#[test]
fn a_stale_scorecard_is_listed_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    let replies = run(
        dir.path(),
        vec![
            call(2, "scorecard_publish", &publish_args(&head)),
            call(3, "scorecards_list", &json!({})),
            call(
                4,
                "scorecards_list",
                &json!({ "include_stale": true, "limit": 5 }),
            ),
        ],
    );
    assert!(refusal(&replies[0]).contains("is not the tip of feat"));
    assert_eq!(result(&replies[1])["scorecards"], json!([]));
}

#[test]
fn a_published_scorecard_goes_stale_after_a_commit_and_shows_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let published = run(
        dir.path(),
        vec![call(2, "scorecard_publish", &publish_args(&head))],
    );
    assert!(published[0]["result"].is_object(), "{published:?}");
    let mut requests = initialize();
    requests.push(call(3, "scorecards_list", &json!({ "name": "security" })));
    let before = session(dir.path(), &requests);
    assert_eq!(
        result(&before[1])["scorecards"].as_array().unwrap().len(),
        1
    );
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "second"]);
    let after = run(
        dir.path(),
        vec![
            call(4, "scorecards_list", &json!({})),
            call(5, "scorecards_list", &json!({ "include_stale": true })),
        ],
    );
    assert_eq!(result(&after[0])["scorecards"], json!([]));
    let stale = &result(&after[1])["scorecards"][0];
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["head"], head);
}

#[test]
fn an_agent_marks_a_scorecard_comment_handled_and_never_resolves_it() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let replies = run(
        dir.path(),
        vec![
            call(2, "scorecard_publish", &publish_args(&head)),
            call(3, "review_comments_list", &json!({ "author": "any" })),
        ],
    );
    let id = result(&replies[0])["comment_ids"][0].clone();
    let handled = run(
        dir.path(),
        vec![call(
            4,
            "review_comment_resolve",
            &json!({ "id": id, "summary": "checked the length" }),
        )],
    );
    assert_eq!(result(&handled[0])["status"], "handled");
    assert_eq!(
        result(&handled[0])["replies"][0]["body"],
        "checked the length"
    );
}

#[test]
fn a_review_comment_with_only_a_label_takes_its_kind_and_severity() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = run(
        dir.path(),
        vec![
            call(
                2,
                "review_comment_create",
                &json!({
                    "path": "f.txt",
                    "anchor_text": "four",
                    "body": "Rename this to keep the naming consistent.",
                    "label": "nitpick",
                    "decorations": ["non-blocking", "naming"]
                }),
            ),
            call(
                3,
                "review_comment_create",
                &json!({ "path": "f.txt", "anchor_text": "four", "body": "no kind and no label" }),
            ),
            call(
                4,
                "review_comment_create",
                &json!({
                    "path": "f.txt",
                    "anchor_text": "four",
                    "body": "too many words",
                    "label": "todo",
                    "decorations": ["a", "b", "c", "d"]
                }),
            ),
        ],
    );
    let created = result(&replies[0]);
    assert_eq!(created["label"], "nitpick");
    assert_eq!(created["kind"], "flag");
    assert_eq!(created["severity"], "low");
    assert_eq!(created["decorations"], json!(["non-blocking", "naming"]));
    assert!(refusal(&replies[1]).contains("a kind"));
    assert!(refusal(&replies[2]).contains("at most 3 decorations"));
}
