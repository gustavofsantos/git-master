//! Proposed reviewers and gates (D214 (3)) through the MCP server end to end: JSON-RPC lines in, a
//! real repository behind them. The person's reviewers and gates are read from a temporary
//! configuration directory, never from the real one.

use std::path::Path;
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::{Engine, reviewers};
use gm_mcp::Session;
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) {
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
}

/// A repository on `main` with `f.txt`, checked out on `feat` with one commit of its own.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    git(dir, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(dir.join("f.txt"), "one\nthree\n").unwrap();
    git(dir, &["commit", "-q", "-am", "feat change"]);
}

/// Runs the server on `requests` for the repository at `repo`, with the person's reviewers and
/// gates in `config`.
fn session(repo: &Path, config: &Path, requests: &[Value]) -> Vec<Value> {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_reviewer_user_dir(Some(reviewers::user_dir(config)));
    engine.set_gates_user_dir(Some(config.to_path_buf()));
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

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "test-client", "version": "1" } } })
}

fn tools_list(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list" })
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

fn reviewer_args(name: &str) -> Value {
    json!({
        "name": name,
        "description": "Knows the billing rules.",
        "instructions": "Check the rounding of invoices against the billing rules.",
        "signal": "level",
        "direction": "lower_is_better",
        "subject": "branch",
        "reason": "the billing rules are not in the code"
    })
}

fn gate_args(id: &str) -> Value {
    json!({
        "id": id,
        "kind": "up_to_date",
        "on": ["push"],
        "reason": "billing changes need a check"
    })
}

#[test]
fn the_proposal_tools_are_listed_as_writes_and_no_tool_accepts_or_denies_one() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = session(dir.path(), config.path(), &[initialize(), tools_list(2)]);
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let by_name = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
    for name in ["reviewer_propose", "gate_propose"] {
        assert_eq!(
            by_name(name)["annotations"]["readOnlyHint"],
            false,
            "{name}"
        );
        let required = &by_name(name)["inputSchema"]["required"];
        assert!(
            required.as_array().unwrap().contains(&json!("reason")),
            "{name}"
        );
    }
    let gate_fields = &by_name("gate_propose")["inputSchema"]["properties"];
    for field in [
        "id",
        "kind",
        "reviewers",
        "severity",
        "reviewer",
        "max",
        "min",
        "on",
        "when",
    ] {
        assert!(
            gate_fields.get(field).is_some(),
            "gate_propose takes {field}"
        );
    }
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(
        names.iter().all(|name| !name.contains("accept")
            && !name.contains("deny")
            && !name.contains("approve")),
        "{names:?}"
    );
}

#[test]
fn an_agent_proposes_a_reviewer_and_reviewers_list_does_not_show_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = session(
        dir.path(),
        config.path(),
        &[
            initialize(),
            call(2, "reviewer_propose", &reviewer_args("billing")),
            call(3, "reviewers_list", &json!({})),
            call(4, "reviewers_list", &json!({ "name": "billing" })),
        ],
    );
    let proposal = &result(&replies[1])["proposal"];
    assert_eq!(proposal["status"], "proposed");
    assert_eq!(proposal["kind"], "reviewer");
    assert_eq!(proposal["origin"], "agent:test-client");
    assert_eq!(proposal["updates"], false);
    assert_eq!(proposal["reviewer"]["name"], "billing");
    assert_eq!(proposal["reviewer"]["signal"], "level");
    assert_eq!(proposal["untrusted"], true, "the agent's text is untrusted");
    let listed = result(&replies[2])["reviewers"].as_array().unwrap();
    assert!(
        listed.iter().all(|reviewer| reviewer["name"] != "billing"),
        "a proposal is not a reviewer: {listed:?}"
    );
    assert!(
        result(&replies[3])["reviewers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !dir.path()
            .join(".git/gitmaster/reviewers/billing.md")
            .exists()
    );
}

#[test]
fn an_agent_proposes_a_gate_and_it_blocks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = session(
        dir.path(),
        config.path(),
        &[
            initialize(),
            call(2, "gate_propose", &gate_args("billing-gate")),
            call(3, "gates_status", &json!({})),
        ],
    );
    let answer = result(&replies[1]);
    assert_eq!(answer["proposal"]["status"], "proposed");
    assert_eq!(answer["proposal"]["kind"], "gate");
    assert_eq!(answer["proposal"]["gate"]["id"], "billing-gate");
    assert_eq!(answer["proposal"]["gate"]["kind"], "up_to_date");
    assert_eq!(
        answer["proposal"]["untrusted"], true,
        "the agent's text is untrusted"
    );
    let status = result(&replies[2]);
    assert_eq!(status["ready"], true, "{status}");
    assert!(status["gates"].as_array().unwrap().is_empty(), "{status}");
}

#[test]
fn a_gate_proposal_takes_the_settings_of_its_kind_and_refuses_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    repo(dir.path());
    let replies = session(
        dir.path(),
        config.path(),
        &[
            initialize(),
            call(
                2,
                "gate_propose",
                &json!({
                    "id": "billing-verdict",
                    "kind": "fresh_verdict",
                    "reviewers": ["billing"],
                    "on": ["push", "restack"],
                    "when": { "paths": ["services/billing/**"] },
                    "reason": "billing changes need the billing reviewer"
                }),
            ),
            call(
                3,
                "gate_propose",
                &json!({
                    "id": "scores",
                    "kind": "signal",
                    "reviewer": "security",
                    "max": 40,
                    "on": ["push"],
                    "reason": "keep the score low"
                }),
            ),
            call(
                4,
                "gate_propose",
                &json!({
                    "id": "wrong-setting",
                    "kind": "findings",
                    "reviewers": ["billing"],
                    "severity": "high",
                    "on": ["push"],
                    "reason": "why"
                }),
            ),
            call(
                5,
                "gate_propose",
                &json!({ "id": "bad-kind", "kind": "bogus", "on": ["push"], "reason": "why" }),
            ),
            call(
                6,
                "gate_propose",
                &json!({
                    "id": "bad-when",
                    "kind": "up_to_date",
                    "on": ["push"],
                    "when": { "files": ["x"] },
                    "reason": "why"
                }),
            ),
        ],
    );
    let first = result(&replies[1]);
    assert_eq!(first["proposal"]["gate"]["reviewers"][0], "billing");
    assert_eq!(
        first["proposal"]["gate"]["when"]["paths"][0],
        "services/billing/**"
    );
    assert_eq!(first["proposal"]["gate"]["on"], json!(["push", "restack"]));
    let signal = result(&replies[2]);
    assert_eq!(signal["proposal"]["gate"]["max"], 40);
    assert!(refusal(&replies[3]).contains("reviewers is not a setting of a findings gate"));
    assert!(refusal(&replies[4]).contains("kind must be"));
    assert!(refusal(&replies[5]).contains("files is not a field of when"));
}

#[test]
fn refusals_come_back_as_tool_errors_and_the_server_keeps_answering() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    repo(dir.path());
    let mut accepting = reviewer_args("billing");
    accepting["accept"] = json!(true);
    let mut no_reason = reviewer_args("billing");
    no_reason.as_object_mut().unwrap().remove("reason");
    let replies = session(
        dir.path(),
        config.path(),
        &[
            initialize(),
            call(2, "reviewer_propose", &reviewer_args("Bad Name")),
            call(3, "reviewer_propose", &accepting),
            call(4, "reviewer_propose", &no_reason),
            call(5, "reviewer_propose", &reviewer_args("billing")),
            call(6, "reviewer_propose", &reviewer_args("billing")),
            call(7, "reviewers_list", &json!({})),
        ],
    );
    assert!(
        refusal(&replies[1]).contains("not a reviewer name"),
        "{}",
        replies[1]
    );
    assert!(refusal(&replies[2]).contains("accept is not an argument here"));
    assert!(refusal(&replies[3]).contains("reason is required"));
    result(&replies[4]);
    let again = refusal(&replies[5]);
    assert!(again.contains("already proposed as proposal 1"), "{again}");
    assert!(result(&replies[6])["reviewers"].is_array());
}
