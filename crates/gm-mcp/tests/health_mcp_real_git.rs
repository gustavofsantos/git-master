//! The code health tools (docs/guide/code-health.md, D225) end to end: JSON-RPC lines in, a real
//! repository with a real history behind them, through the same session the server uses.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_mcp::Session;
use serde_json::{Value, json};

const FILES: [&str; 6] = [
    "hot.rs",
    "cold.rs",
    "pair_a.rs",
    "pair_b.rs",
    "src/a/x.rs",
    "src/ab/y.rs",
];

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

fn write(dir: &Path, path: &str, version: u32) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, format!("{path} version {version}\n")).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A repository whose history gives: `hot.rs` 10 commits, `pair_a.rs` and `pair_b.rs` 6 commits
/// each, all six together, and the other three files once each.
fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for file in FILES {
        write(dir, file, 0);
    }
    commit(dir, "init");
    for version in 1..=5 {
        write(dir, "pair_a.rs", version);
        write(dir, "pair_b.rs", version);
        commit(dir, &format!("pair {version}"));
    }
    for version in 1..=9 {
        write(dir, "hot.rs", version);
        commit(dir, &format!("hot {version}"));
    }
}

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

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "test-client", "version": "1" } } })
}

/// One call on a fresh session: the structured result of a call that succeeds.
fn ask(repo: &Path, name: &str, arguments: &Value) -> Value {
    let replies = session(repo, &[initialize(), call(2, name, arguments)]);
    result(&replies[1]).clone()
}

fn result(reply: &Value) -> &Value {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    &reply["result"]["structuredContent"]
}

/// The text of a call that is refused.
fn refusal(repo: &Path, name: &str, arguments: &Value) -> String {
    let replies = session(repo, &[initialize(), call(2, name, arguments)]);
    assert_eq!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    replies[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn paths_of(files: &Value) -> Vec<String> {
    files
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn health_context_answers_a_known_file_and_an_unknown_one() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let answer = ask(
        &repo,
        "health_context",
        &json!({ "paths": ["hot.rs", "nope/unknown.rs"] }),
    );
    assert_eq!(answer["complete"], true, "{answer}");
    assert!(answer.get("note").is_none(), "{answer}");
    assert_eq!(answer["rev"].as_str().unwrap().len(), 40, "{answer}");
    assert!(answer["config_hash"].is_string(), "{answer}");
    let hot = &answer["paths"][0];
    assert_eq!(hot["path"], "hot.rs");
    assert_eq!(hot["known"], true, "{hot}");
    assert_eq!(hot["heat"]["rank"], 1, "{hot}");
    assert_eq!(hot["heat"]["of"], FILES.len(), "{hot}");
    assert!(
        ["rising", "steady", "falling"].contains(&hot["heat"]["trend"].as_str().unwrap()),
        "{hot}"
    );
    assert!(hot["heat"]["value"].as_f64().unwrap() > 0.0, "{hot}");
    assert!(hot["constraints"].is_array(), "{hot}");
    assert!(hot["changes_together"].is_array(), "{hot}");
    assert_eq!(hot["ownership"]["authors"], 1, "{hot}");
    let unknown = &answer["paths"][1];
    assert_eq!(unknown["path"], "nope/unknown.rs");
    assert_eq!(unknown["known"], false, "{unknown}");
    assert_eq!(unknown["constraints"], json!([]), "{unknown}");
    assert!(unknown.get("heat").is_none(), "{unknown}");
}

#[test]
fn health_context_lists_the_file_that_changes_together_with_the_one_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let answer = ask(&repo, "health_context", &json!({ "paths": ["pair_a.rs"] }));
    let partners = answer["paths"][0]["changes_together"].as_array().unwrap();
    let partner = partners
        .iter()
        .find(|partner| partner["path"] == "pair_b.rs")
        .unwrap_or_else(|| panic!("pair_b.rs is listed: {answer}"));
    assert_eq!(partner["shared_commits"], 6, "{partner}");
    assert!(partner["degree"].as_f64().unwrap() >= 0.3, "{partner}");
}

#[test]
fn hotspots_list_pages_through_with_the_cursor_and_ends_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let mut paths = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= FILES.len(), "the listing ends");
        let mut arguments = json!({ "limit": 1 });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = ask(&repo, "hotspots_list", &arguments);
        assert_eq!(page["files"].as_array().unwrap().len(), 1, "{page}");
        assert_eq!(page["files"][0]["rank"], pages, "{page}");
        paths.extend(paths_of(&page["files"]));
        if page["has_more"] == false {
            assert!(page["next_cursor"].is_null(), "{page}");
            break;
        }
        cursor = Some(page["next_cursor"].as_str().unwrap().to_owned());
    }
    assert_eq!(pages, FILES.len());
    assert_eq!(paths[0], "hot.rs");
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        FILES.len(),
        "no file is listed twice: {paths:?}"
    );
}

#[test]
fn hotspots_list_scope_takes_whole_segments_of_the_folder_only() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let inside = ask(&repo, "hotspots_list", &json!({ "scope": "src/a" }));
    assert_eq!(paths_of(&inside["files"]), vec!["src/a/x.rs"], "{inside}");
    assert_eq!(inside["has_more"], false, "{inside}");
    let slashed = ask(&repo, "hotspots_list", &json!({ "scope": "/src/a/" }));
    assert_eq!(paths_of(&slashed["files"]), vec!["src/a/x.rs"], "{slashed}");
    let sibling = ask(&repo, "hotspots_list", &json!({ "scope": "src/ab" }));
    assert_eq!(
        paths_of(&sibling["files"]),
        vec!["src/ab/y.rs"],
        "{sibling}"
    );
    let nothing = ask(&repo, "hotspots_list", &json!({ "scope": "nowhere" }));
    assert_eq!(nothing["files"], json!([]), "{nothing}");
    assert!(nothing["next_cursor"].is_null(), "{nothing}");
    let whole = ask(&repo, "hotspots_list", &json!({}));
    assert_eq!(
        whole["files"].as_array().unwrap().len(),
        FILES.len(),
        "{whole}"
    );
}

#[test]
fn a_forged_cursor_or_one_of_another_scope_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let forged = refusal(
        &repo,
        "hotspots_list",
        &json!({ "cursor": "1:000000000000:deadbeef:" }),
    );
    assert!(forged.contains("another listing"), "{forged}");
    assert!(forged.contains("without a cursor"), "{forged}");
    let whole = ask(&repo, "hotspots_list", &json!({ "limit": 1 }));
    let cursor = whole["next_cursor"].as_str().unwrap();
    let other_scope = refusal(
        &repo,
        "hotspots_list",
        &json!({ "limit": 1, "scope": "src", "cursor": cursor }),
    );
    assert!(other_scope.contains("another listing"), "{other_scope}");
}

#[test]
fn health_context_refuses_an_empty_or_oversized_path_list_and_keeps_answering() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let empty = refusal(&repo, "health_context", &json!({ "paths": [] }));
    assert!(empty.contains("1 to 50"), "{empty}");
    let too_many: Vec<String> = (0..51).map(|index| format!("f{index}.rs")).collect();
    let many = refusal(&repo, "health_context", &json!({ "paths": too_many }));
    assert!(many.contains("not 51"), "{many}");
    let escaping = refusal(
        &repo,
        "health_context",
        &json!({ "paths": ["../outside.rs"] }),
    );
    assert!(escaping.contains(".."), "{escaping}");
    let absolute = refusal(
        &repo,
        "health_context",
        &json!({ "paths": ["/etc/passwd"] }),
    );
    assert!(absolute.contains("relative"), "{absolute}");
    // The same session keeps answering after a refusal.
    let replies = session(
        &repo,
        &[
            initialize(),
            call(2, "health_context", &json!({ "paths": [] })),
            call(3, "health_context", &json!({ "paths": ["cold.rs"] })),
        ],
    );
    assert_eq!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    assert_eq!(
        result(&replies[2])["paths"][0]["known"],
        true,
        "{}",
        replies[2]
    );
}

#[test]
fn both_health_tools_are_listed_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let replies = session(
        &repo,
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    for name in ["health_context", "hotspots_list", "churn_summary"] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed"));
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
        assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");
    }
    let context = tools
        .iter()
        .find(|tool| tool["name"] == "health_context")
        .unwrap();
    assert_eq!(context["inputSchema"]["required"], json!(["paths"]));
    assert!(context["description"].as_str().unwrap().contains("BEFORE"));
}

/// The churn of a summary has its counts and its shares to two decimals, at every age.
fn assert_churn_shape(churn: &Value) {
    if churn.is_null() {
        return;
    }
    assert!(churn["lines_added"].is_u64(), "{churn}");
    assert!(churn["rewritten_14"].is_u64(), "{churn}");
    let share = churn["rewritten_share"].as_f64().unwrap();
    assert!((0.0..=1.0).contains(&share), "{churn}");
    assert!(
        (share * 100.0 - (share * 100.0).round()).abs() < 1e-9,
        "{churn}"
    );
    let survival = churn["survival"].as_object().unwrap();
    assert_eq!(survival.len(), 6, "{churn}");
    for age in ["7", "14", "30", "60", "90", "180"] {
        let share = survival[age]
            .as_f64()
            .unwrap_or_else(|| panic!("{age}: {churn}"));
        assert!((0.0..=1.0).contains(&share), "{churn}");
    }
}

#[test]
fn churn_summary_answers_the_whole_repository_and_a_scope_with_churn_or_null() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let whole = ask(&repo, "churn_summary", &json!({}));
    assert_eq!(whole["complete"], true, "{whole}");
    assert!(whole.get("note").is_none(), "{whole}");
    assert_eq!(whole["rev"].as_str().unwrap().len(), 40, "{whole}");
    assert!(whole["config_hash"].is_string(), "{whole}");
    assert_eq!(whole["scope"], "", "{whole}");
    assert_churn_shape(&whole["churn"]);
    let folder = ask(&repo, "churn_summary", &json!({ "scope": "/src/a/" }));
    assert_eq!(folder["scope"], "src/a", "{folder}");
    assert_churn_shape(&folder["churn"]);
}

#[test]
fn churn_summary_of_an_unknown_scope_has_no_churn() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    make_repo(&repo);
    let unknown = ask(&repo, "churn_summary", &json!({ "scope": "nowhere" }));
    assert!(unknown["churn"].is_null(), "{unknown}");
    assert_eq!(unknown["scope"], "nowhere", "{unknown}");
    let sibling = ask(&repo, "churn_summary", &json!({ "scope": "src/a" }));
    let prefix = ask(&repo, "churn_summary", &json!({ "scope": "src" }));
    assert_eq!(sibling["scope"], "src/a", "{sibling}");
    assert_eq!(prefix["scope"], "src", "{prefix}");
    let bad = refusal(&repo, "churn_summary", &json!({ "scope": "../x" }));
    assert!(!bad.is_empty());
}
