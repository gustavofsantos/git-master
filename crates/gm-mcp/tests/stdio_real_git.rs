//! The MCP server (D135) end to end: JSON-RPC lines in, a real repository and its real
//! `.git/gitmaster/` stores behind it.

use std::path::Path;
use std::process::Command as Process;

use gm_schema::{Actor, Command, CommandKind, OperationId};
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

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("f.txt"), "a\nb\nc\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

/// Runs `gitmaster mcp` on `dir` with `requests` as its input, and returns its replies.
fn session(dir: &Path, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let code = gm_mcp::command(&[], dir, input.as_bytes(), &mut output, &mut errors);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&errors));
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

/// A comment as the app writes it: through the engine, as the person.
fn comment(dir: &Path, line: u32, body: &str) {
    let mut engine = gm_core::Engine::new(Box::new(gm_core::git::SubprocessGit::default()));
    let human = |kind| Command {
        actor: Actor::Human,
        kind,
    };
    engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        }),
    );
    engine.handle(
        OperationId(2),
        &human(CommandKind::AddReviewComment {
            path: "f.txt".into(),
            start_line: line,
            end_line: line,
            body: body.into(),
        }),
    );
}

fn result(reply: &Value) -> &Value {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    &reply["result"]["structuredContent"]
}

#[test]
fn the_handshake_lists_only_the_tools_the_agent_does_not_have() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    );
    assert_eq!(replies.len(), 2, "a notification is not answered");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "gitmaster");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "review_comments_list",
            "review_comment_resolve",
            "threads_search",
            "thread_get",
            "thread_read"
        ]
    );
}

#[test]
fn an_agent_lists_a_comment_handles_it_and_the_person_still_has_to_resolve_it() {
    let dir = repo();
    comment(dir.path(), 2, "rename b");
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "review_comments_list", &json!({ "status": "open" })),
            call(
                3,
                "review_comment_resolve",
                &json!({ "id": 1, "summary": "renamed b to bee" }),
            ),
            call(4, "review_comments_list", &json!({ "status": "open" })),
            call(5, "review_comments_list", &json!({ "status": "handled" })),
            call(
                6,
                "review_comment_resolve",
                &json!({ "id": 1, "summary": " " }),
            ),
        ],
    );
    let open = result(&replies[1])["comments"].as_array().unwrap().clone();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["path"], "f.txt");
    assert_eq!(open[0]["start_line"], 2);
    assert_eq!(open[0]["body"], "rename b");
    assert_eq!(open[0]["untrusted"], true);

    assert_eq!(result(&replies[2])["status"], "handled");
    assert!(
        result(&replies[3])["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        result(&replies[4])["comments"][0]["summary"],
        "renamed b to bee"
    );
    assert_eq!(replies[5]["result"]["isError"], true);

    // The record is in the repository's store: the app reads it from there.
    assert!(dir.path().join(".git/gitmaster/review.db").is_file());
}

#[test]
fn errors_are_answered_not_fatal() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "no_such_tool", &json!({})),
            call(3, "thread_get", &json!({ "id": "0123456789abcdef" })),
            call(
                4,
                "review_comment_resolve",
                &json!({ "id": 99, "summary": "x" }),
            ),
            json!({ "jsonrpc": "2.0", "id": 5, "method": "nope" }),
            json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
        ],
    );
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(replies[2]["result"]["isError"], true);
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!(replies[4]["error"]["code"], -32601);
    assert_eq!(replies[5]["result"], json!({}));
}

#[test]
fn threads_of_an_empty_store_search_to_nothing() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "threads_search", &json!({ "query": "backoff" })),
        ],
    );
    assert!(result(&replies[1])["hits"].as_array().unwrap().is_empty());
}

#[test]
fn a_directory_that_is_not_a_repository_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut errors = Vec::new();
    let code = gm_mcp::command(&[], dir.path(), &b""[..], Vec::new(), &mut errors);
    assert_eq!(code, 1);
    assert!(!errors.is_empty());
    let code = gm_mcp::command(
        &["--bogus".to_owned()],
        dir.path(),
        &b""[..],
        Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(code, 2);
}

/// A thread stored the way the capture hook stores it, in the repository's `threads.db`.
fn store_thread(dir: &Path) -> String {
    use gm_core::threads::store::{Batch, NewEdit, NewEntry, ThreadStore};
    use gm_schema::{EditChange, EntryKind};

    let entry = |source: &str, kind, text: &str, at| NewEntry {
        source_id: source.to_owned(),
        kind,
        parent_id: None,
        tool: None,
        text: text.to_owned(),
        args_json: None,
        is_error: false,
        truncated: false,
        orig_bytes: None,
        redactions: 0,
        hits: Vec::new(),
        at,
        edit: None,
    };
    let mut edited = entry("e2", EntryKind::ToolCall, "Edit f.txt", 3);
    edited.edit = Some(NewEdit {
        path: "f.txt".to_owned(),
        old_path: None,
        change: EditChange::Modify,
        patch: "@@ -1 +1 @@\n-a\n+b\n".to_owned(),
        added: 1,
        removed: 1,
        pre_oid: None,
        post_oid: None,
        from_input: false,
        hits: Vec::new(),
    });
    let batch = Batch {
        harness: "claude-code".to_owned(),
        session_id: "s1".to_owned(),
        worktree: dir.to_string_lossy().into_owned(),
        branch: Some("main".to_owned()),
        head: None,
        model: None,
        harness_version: None,
        transcript: None,
        cursor: None,
        started_at: 1,
        at: 5,
        ended: None,
        entries: vec![
            entry("e0", EntryKind::User, "why did we pick jittered backoff", 1),
            entry(
                "e1",
                EntryKind::Assistant,
                "jittered backoff avoids thundering herds",
                2,
            ),
            edited,
            entry("e3", EntryKind::Assistant, "done", 4),
        ],
        gaps: Vec::new(),
    };
    let mut store = ThreadStore::open(&dir.join(".git")).unwrap();
    store.apply(&batch).unwrap();
    ThreadStore::thread_uid("claude-code", "s1")
}

#[test]
fn an_agent_searches_a_thread_then_reads_it_in_pages() {
    let dir = repo();
    let uid = store_thread(dir.path());
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "threads_search",
                &json!({ "query": "backoff", "limit": 5 }),
            ),
            call(3, "thread_get", &json!({ "id": uid })),
            call(
                4,
                "thread_read",
                &json!({ "id": uid, "from": 1, "limit": 2 }),
            ),
        ],
    );
    let hits = result(&replies[1])["hits"].as_array().unwrap().clone();
    assert!(!hits.is_empty());
    assert_eq!(hits[0]["thread"], uid.as_str());
    assert_eq!(hits[0]["untrusted"], true);

    let thread = result(&replies[2]);
    assert_eq!(thread["thread"]["branch"], "main");
    assert_eq!(thread["edits"][0]["path"], "f.txt");
    assert!(
        thread["edits"][0].get("patch").is_none(),
        "a summary stays small"
    );

    let page = result(&replies[3]);
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["seq"], 1);
    assert_eq!(entries[0]["kind"], "assistant");
    assert_eq!(entries[0]["untrusted"], true);
    assert_eq!(page["has_more"], true);
}
