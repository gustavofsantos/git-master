//! The MCP server (D135) end to end: JSON-RPC lines in, a real repository and its real
//! `.git/gitmaster/` stores behind it.

use std::path::Path;
use std::process::Command as Process;

use gm_core::threads::store::{Batch, MetaUpdate, NewEdit, NewEntry, ThreadStore};
use gm_schema::{
    Actor, Command, CommandKind, EditChange, EntryKind, LinkKind, LinkOrigin, OperationId,
    ThreadLink,
};
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
    let code = gm_mcp::command(&[], dir, None, input.as_bytes(), &mut output, &mut errors);
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
            "thread_read",
            "provenance",
            "labels_list",
            "label_get",
            "label_create",
            "label_attach",
            "thread_link"
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
fn resolving_returns_the_edit_gitmaster_observed_in_the_region() {
    let dir = repo();
    comment(dir.path(), 2, "rename b");
    std::fs::write(dir.path().join("f.txt"), "a\nbee\nc\n").unwrap();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "review_comment_resolve",
                &json!({ "id": 1, "summary": "renamed b to bee" }),
            ),
            call(3, "review_comments_list", &json!({ "status": "handled" })),
        ],
    );
    let resolved = result(&replies[1]);
    assert_eq!(resolved["status"], "handled");
    assert_eq!(resolved["untrusted"], true);
    assert_eq!(
        resolved["change"],
        json!({ "before": ["b"], "after": ["bee"], "changed": true, "before_start_line": 2, "after_start_line": 2 })
    );
    // The list stays as it was: the change is only on the resolve result.
    assert!(result(&replies[2])["comments"][0].get("change").is_none());
}

#[test]
fn resolving_without_an_edit_says_nothing_changed() {
    let dir = repo();
    comment(dir.path(), 2, "rename b");
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "review_comment_resolve",
                &json!({ "id": 1, "summary": "renamed b to bee" }),
            ),
        ],
    );
    assert_eq!(
        result(&replies[1])["change"],
        json!({ "before": ["b"], "after": ["b"], "changed": false, "before_start_line": 2, "after_start_line": 2 })
    );
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
    let code = gm_mcp::command(&[], dir.path(), None, &b""[..], Vec::new(), &mut errors);
    assert_eq!(code, 1);
    assert!(!errors.is_empty());
    let code = gm_mcp::command(
        &["--bogus".to_owned()],
        dir.path(),
        None,
        &b""[..],
        Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(code, 2);
}

fn new_entry(source: &str, kind: EntryKind, text: &str, at: i64) -> NewEntry {
    NewEntry {
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
    }
}

/// A thread stored the way the capture hook stores it, in the repository's `threads.db`.
fn store_thread(dir: &Path) -> String {
    let entry = new_entry;
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

const COMMIT: &str = "abcdef1234567890abcdef1234567890abcdef12";

fn new_batch(
    dir: &Path,
    session: &str,
    model: &str,
    branch: &str,
    entries: Vec<NewEntry>,
) -> Batch {
    let started_at = entries.first().map_or(0, |entry| entry.at);
    let at = entries.last().map_or(0, |entry| entry.at);
    Batch {
        harness: "claude-code".to_owned(),
        session_id: session.to_owned(),
        worktree: dir.to_string_lossy().into_owned(),
        branch: Some(branch.to_owned()),
        head: None,
        model: Some(model.to_owned()),
        harness_version: None,
        transcript: None,
        cursor: None,
        started_at,
        at,
        ended: None,
        entries,
        gaps: Vec::new(),
    }
}

/// Two threads for the search tests, returned as their ids. `a` ran on `main` with
/// `claude-opus-4`, edited `f.txt`, is starred, labelled `auth` and linked to a commit. `b` ran
/// later on `feature` with `claude-haiku` and edited nothing.
fn store_search_threads(dir: &Path) -> (String, String) {
    let mut edited = new_entry("a2", EntryKind::ToolCall, "Edit f.txt", 1002);
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
    let a = new_batch(
        dir,
        "a",
        "claude-opus-4",
        "main",
        vec![
            new_entry(
                "a0",
                EntryKind::User,
                "why did we pick jittered backoff",
                1000,
            ),
            new_entry(
                "a1",
                EntryKind::Assistant,
                "jittered backoff avoids thundering herds",
                1001,
            ),
            edited,
            new_entry("a3", EntryKind::Assistant, "done", 1003),
        ],
    );
    let b = new_batch(
        dir,
        "b",
        "claude-haiku",
        "feature",
        vec![
            new_entry("b0", EntryKind::User, "keep the backoff short", 2000),
            new_entry("b1", EntryKind::Assistant, "a fixed delay is enough", 2001),
        ],
    );
    let mut store = ThreadStore::open(&dir.join(".git")).unwrap();
    store.apply(&a).unwrap();
    store.apply(&b).unwrap();
    let uid_a = ThreadStore::thread_uid("claude-code", "a");
    let uid_b = ThreadStore::thread_uid("claude-code", "b");
    store
        .update_meta(
            &uid_a,
            &MetaUpdate {
                starred: Some(true),
                ..MetaUpdate::default()
            },
        )
        .unwrap();
    store
        .attach_label(
            "auth",
            gm_schema::threads::AttachKind::Thread,
            &uid_a,
            None,
            "human",
            1,
        )
        .unwrap();
    store
        .link(
            &uid_a,
            &ThreadLink {
                kind: LinkKind::Commit,
                reference: COMMIT.to_owned(),
                origin: LinkOrigin::Human,
                resolved: None,
                state: None,
            },
        )
        .unwrap();
    (uid_a, uid_b)
}

/// Runs each search in one session, after the handshake, and returns the reply to each.
fn searches(dir: &Path, arguments: &[Value]) -> Vec<Value> {
    let mut requests = vec![initialize()];
    requests.extend(
        (2_u64..)
            .zip(arguments)
            .map(|(id, args)| call(id, "threads_search", args)),
    );
    session(dir, &requests).split_off(1)
}

/// The thread ids of a reply, sorted, for a search whose equal hits have no fixed order.
fn thread_set(reply: &Value) -> Vec<String> {
    let mut ids = thread_ids(reply);
    ids.sort();
    ids
}

fn hits_of(reply: &Value) -> Vec<Value> {
    result(reply)["hits"].as_array().unwrap().clone()
}

fn thread_ids(reply: &Value) -> Vec<String> {
    hits_of(reply)
        .iter()
        .map(|hit| hit["thread"].as_str().unwrap().to_owned())
        .collect()
}

/// The count the diagnosis gives for removing `filter`, when it is named.
fn removed_by(reply: &Value, filter: &str) -> Option<i64> {
    result(reply)["diagnosis"]
        .as_array()?
        .iter()
        .find(|entry| entry["filter"] == filter)?["count"]
        .as_i64()
}

#[test]
fn the_search_filters_by_role_turn_thread_and_context() {
    let dir = repo();
    let (a, b) = store_search_threads(dir.path());
    let replies = searches(
        dir.path(),
        &[
            json!({ "query": "backoff", "group": "entry", "role": "user" }),
            json!({ "query": "backoff", "group": "entry", "role": "assistant" }),
            json!({ "query": "backoff", "group": "entry", "role": "any" }),
            json!({ "query": "backoff", "group": "entry", "thread": b }),
            json!({ "query": "backoff", "group": "entry", "turn_from": 50 }),
            json!({ "query": "backoff", "group": "entry", "role": "assistant", "context": 2 }),
        ],
    );

    assert_eq!(result(&replies[0])["filters"]["role"], "user");
    let user = hits_of(&replies[0]);
    assert_eq!(user.len(), 2, "one prompt of each thread");
    assert!(user.iter().all(|hit| hit["seq"] == 0), "{user:?}");

    let assistant = hits_of(&replies[1]);
    assert_eq!(assistant.len(), 1, "{assistant:?}");
    assert_eq!(assistant[0]["thread"], a.as_str());
    assert_eq!(assistant[0]["seq"], 1);

    assert_eq!(hits_of(&replies[2]).len(), 3);

    assert_eq!(thread_ids(&replies[3]), vec![b.clone()]);

    assert!(hits_of(&replies[4]).is_empty());
    assert_eq!(removed_by(&replies[4], "turn_range"), Some(3));

    let context = hits_of(&replies[5]);
    let seqs: Vec<i64> = context[0]["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(
        seqs,
        [0, 3],
        "the prompt before the hit and the reply after it"
    );
    assert_eq!(context[0]["context"][0]["kind"], "user");
}

#[test]
fn the_search_filters_by_thread_metadata_paths_commits_and_times() {
    let dir = repo();
    let (a, b) = store_search_threads(dir.path());
    let replies = searches(
        dir.path(),
        &[
            json!({ "query": "backoff", "harness": "claude-code" }),
            json!({ "query": "backoff", "harness": "codex" }),
            json!({ "query": "backoff", "model": "claude-haiku" }),
            json!({ "query": "backoff", "branch": "main" }),
            json!({ "query": "backoff", "label": "auth" }),
            json!({ "query": "backoff", "label": "missing" }),
            json!({ "query": "backoff", "starred": true }),
            json!({ "query": "backoff", "starred": false }),
            json!({ "query": "backoff", "edited_only": true }),
            json!({ "query": "backoff", "path": "f.txt" }),
            json!({ "query": "backoff", "path": "*.txt" }),
            json!({ "query": "backoff", "path": "*.rs" }),
            json!({ "query": "backoff", "commit": "abcdef1" }),
            json!({ "query": "backoff", "commit": "1111111" }),
            json!({ "query": "backoff", "since": "7d" }),
            json!({ "query": "backoff", "until": "7d" }),
            json!({ "query": "backoff", "since": 1500 }),
            json!({ "query": "backoff", "until": "1970-01-01T00:00:02Z" }),
        ],
    );
    let mut both = vec![a.clone(), b.clone()];
    both.sort();
    assert_eq!(thread_set(&replies[0]), both);
    assert!(hits_of(&replies[1]).is_empty());
    assert_eq!(removed_by(&replies[1], "harness"), Some(2));
    assert_eq!(thread_ids(&replies[2]), vec![b.clone()]);
    assert_eq!(thread_ids(&replies[3]), vec![a.clone()]);
    assert_eq!(thread_ids(&replies[4]), vec![a.clone()]);
    assert!(hits_of(&replies[5]).is_empty());
    assert_eq!(removed_by(&replies[5], "label"), Some(2));
    assert_eq!(thread_ids(&replies[6]), vec![a.clone()]);
    assert_eq!(thread_ids(&replies[7]), vec![b.clone()]);
    assert_eq!(thread_ids(&replies[8]), vec![a.clone()]);
    assert_eq!(thread_ids(&replies[9]), vec![a.clone()]);
    assert_eq!(thread_ids(&replies[10]), vec![a.clone()]);
    assert!(hits_of(&replies[11]).is_empty());
    assert_eq!(removed_by(&replies[11], "path"), Some(2));
    assert_eq!(thread_ids(&replies[12]), vec![a.clone()]);
    assert!(hits_of(&replies[13]).is_empty());
    assert_eq!(removed_by(&replies[13], "commit"), Some(2));
    assert!(
        hits_of(&replies[14]).is_empty(),
        "nothing is newer than 7 days"
    );
    assert_eq!(removed_by(&replies[14], "since"), Some(2));
    assert_eq!(thread_set(&replies[15]), both);
    assert_eq!(thread_ids(&replies[16]), vec![b.clone()]);
    assert_eq!(thread_ids(&replies[17]), vec![a.clone()]);
}

#[test]
fn the_search_orders_counts_and_lists_threads() {
    let dir = repo();
    let (a, b) = store_search_threads(dir.path());
    let replies = searches(
        dir.path(),
        &[
            json!({ "query": "backoff", "order": "recent" }),
            json!({ "query": "backoff", "facets": true }),
            json!({ "query": "backoff" }),
            json!({ "branch": "main" }),
            json!({ "query": "" }),
            json!({ "query": "backoff", "label": "auth", "since": "1970-01-01T00:00:01Z", "edited_only": true }),
        ],
    );

    assert_eq!(
        thread_ids(&replies[0]),
        vec![b.clone(), a.clone()],
        "newest first"
    );

    let facets = &result(&replies[1])["facets"];
    assert!(
        facets["branch"]
            .as_array()
            .unwrap()
            .contains(&json!({ "value": "main", "count": 1 }))
    );
    assert!(
        facets["branch"]
            .as_array()
            .unwrap()
            .contains(&json!({ "value": "feature", "count": 1 }))
    );
    assert_eq!(
        facets["agent"],
        json!([{ "value": "claude-code", "count": 2 }])
    );
    assert_eq!(facets["path"], json!([{ "value": "f.txt", "count": 1 }]));
    assert!(
        result(&replies[2])["facets"].is_null(),
        "facets only when asked"
    );

    let listed = hits_of(&replies[3]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["thread"], a.as_str());
    assert_eq!(listed[0]["seq"], 0, "a listing hit has no entry");
    assert_eq!(listed[0]["agent"], "claude-code");
    assert_eq!(listed[0]["branch"], "main");
    assert_eq!(listed[0]["edit_count"], 1);
    assert_eq!(listed[0]["labels"], json!(["auth"]));
    assert_eq!(listed[0]["untrusted"], true);
    assert_eq!(
        thread_ids(&replies[4]).len(),
        2,
        "an empty query lists by filter alone"
    );

    let answer = result(&replies[5]);
    assert_eq!(answer["filters"]["label"], "auth");
    assert_eq!(answer["filters"]["edited_only"], true);
    assert_eq!(answer["filters"]["since"], 1000);
    assert!(
        answer["filters"].get("harness").is_none(),
        "unset filters are not echoed"
    );
    assert_eq!(thread_ids(&replies[5]), vec![a.clone()]);
    assert_eq!(answer["has_more"], false);
    assert!(answer["next_cursor"].is_null());
}

#[test]
fn the_search_pages_with_a_cursor_until_no_more_hits() {
    let dir = repo();
    store_search_threads(dir.path());
    let mut cursor: Option<String> = None;
    let mut seen: Vec<(String, u64)> = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 5, "the paging does not end");
        let mut args = json!({ "query": "backoff", "group": "entry", "limit": 1 });
        if let Some(text) = &cursor {
            args["cursor"] = json!(text);
        }
        let reply = searches(dir.path(), &[args]).remove(0);
        for hit in hits_of(&reply) {
            seen.push((
                hit["thread"].as_str().unwrap().to_owned(),
                hit["seq"].as_u64().unwrap(),
            ));
        }
        let answer = result(&reply);
        if answer["has_more"] == true {
            cursor = Some(answer["next_cursor"].as_str().unwrap().to_owned());
        } else {
            assert!(answer["next_cursor"].is_null());
            break;
        }
    }
    assert_eq!(pages, 3, "one hit a page");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no hit repeats: {seen:?}");
}

#[test]
fn invalid_search_values_are_refused_with_the_valid_ones() {
    let dir = repo();
    store_search_threads(dir.path());
    let cases = [
        (
            json!({ "query": "x", "role": "bogus" }),
            "one of user, assistant, any",
        ),
        (
            json!({ "query": "x", "group": "pages" }),
            "one of thread, entry",
        ),
        (
            json!({ "query": "x", "order": "random" }),
            "one of relevance, recent",
        ),
        (
            json!({ "query": "x", "context": 4 }),
            "between 0 and 3, not 4",
        ),
        (
            json!({ "query": "x", "limit": 0 }),
            "between 1 and 50, not 0",
        ),
        (
            json!({ "query": "x", "turn_from": 0 }),
            "must be between 1 and",
        ),
        (
            json!({ "query": "x", "turn_from": 3, "turn_to": 1 }),
            "after turn_to",
        ),
        (
            json!({ "query": "x", "since": "yesterday" }),
            "is not a time",
        ),
        (json!({ "query": "x", "until": true }), "must be a time"),
        (json!({ "query": "x", "harness": "" }), "must not be empty"),
        (json!({ "query": "x", "starred": "yes" }), "true or false"),
        (json!({ "query": "x", "facets": 1 }), "true or false"),
    ];
    let arguments: Vec<Value> = cases.iter().map(|(args, _)| args.clone()).collect();
    let replies = searches(dir.path(), &arguments);
    assert_eq!(replies.len(), cases.len());
    for ((_, expected), reply) in cases.iter().zip(&replies) {
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        let message = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            message.contains(expected),
            "{expected:?} not in {message:?}"
        );
    }
}

#[test]
fn the_search_schema_declares_every_filter_and_option() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let search = tools
        .iter()
        .find(|tool| tool["name"] == "threads_search")
        .unwrap();
    let schema = &search["inputSchema"];
    assert!(schema.get("required").is_none(), "query is optional");
    for name in [
        "query",
        "role",
        "thread",
        "turn_from",
        "turn_to",
        "since",
        "until",
        "branch",
        "commit",
        "path",
        "harness",
        "model",
        "label",
        "starred",
        "edited_only",
        "group",
        "order",
        "limit",
        "cursor",
        "context",
        "facets",
    ] {
        assert!(
            schema["properties"].get(name).is_some(),
            "{name} is not declared"
        );
    }
    assert_eq!(schema["properties"]["group"]["default"], "thread");
}

/// Several servers on one repository at the same time, in its main worktree and in a linked
/// one, starting on a repository that has no store yet. None may fail, and every comment ends
/// handled exactly once.
#[test]
fn parallel_servers_share_one_repository_and_its_worktrees() {
    const SERVERS: usize = 8;
    let dir = repo();
    let linked = dir.path().join("linked");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "other",
        ],
    );
    let worktrees = [dir.path().to_path_buf(), linked];

    // All servers start on a fresh repository, so the stores are created in a race.
    let start = std::sync::Barrier::new(SERVERS);
    std::thread::scope(|scope| {
        for n in 0..SERVERS {
            let (start, worktree) = (&start, &worktrees[n % 2]);
            scope.spawn(move || {
                start.wait();
                let replies = session(
                    worktree,
                    &[
                        initialize(),
                        call(2, "review_comments_list", &json!({})),
                        call(3, "threads_search", &json!({ "query": "anything" })),
                    ],
                );
                assert_eq!(result(&replies[1])["comments"], json!([]));
                assert_eq!(result(&replies[2])["hits"], json!([]));
            });
        }
    });

    for line in 1..=3 {
        comment(dir.path(), line, &format!("comment {line}"));
    }
    store_thread(dir.path());

    // Each server handles every comment; they overlap, so the same comment is handled by
    // several of them, and the last summary wins.
    let start = std::sync::Barrier::new(SERVERS);
    std::thread::scope(|scope| {
        for n in 0..SERVERS {
            let (start, worktree) = (&start, &worktrees[n % 2]);
            scope.spawn(move || {
                start.wait();
                let mut requests = vec![initialize()];
                for id in 1..=3 {
                    requests.push(call(
                        10 + id,
                        "review_comment_resolve",
                        &json!({ "id": id, "summary": format!("server {n}") }),
                    ));
                    requests.push(call(
                        20 + id,
                        "threads_search",
                        &json!({ "query": "backoff" }),
                    ));
                }
                for reply in &session(worktree, &requests)[1..] {
                    assert_eq!(reply["result"]["isError"], false, "{reply}");
                }
            });
        }
    });

    let replies = session(
        &worktrees[1],
        &[
            initialize(),
            call(2, "review_comments_list", &json!({ "status": "handled" })),
        ],
    );
    assert_eq!(result(&replies[1])["comments"].as_array().unwrap().len(), 3);
}

#[test]
fn a_server_started_outside_a_repository_falls_back_to_the_project_directory() {
    let outside = tempfile::tempdir().unwrap();
    let project = repo();
    let input = format!("{}\n", call(1, "review_comments_list", &json!({})));
    let mut output = Vec::new();
    let code = gm_mcp::command(
        &[],
        outside.path(),
        Some(project.path()),
        input.as_bytes(),
        &mut output,
        &mut Vec::new(),
    );
    assert_eq!(code, 0);
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("\"comments\":[]")
    );
}

/// The full id of `rev` in `dir`, as blame and the commit links store it.
fn rev(dir: &Path, rev: &str) -> String {
    let output = Process::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "rev-parse {rev}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A `f.txt` edit with the given patch. `post_oid` is what the edit wrote, when it is known.
fn file_edit(patch: &str, added: u32, post_oid: Option<String>, from_input: bool) -> NewEdit {
    NewEdit {
        path: "f.txt".to_owned(),
        old_path: None,
        change: EditChange::Modify,
        patch: patch.to_owned(),
        added,
        removed: 0,
        pre_oid: None,
        post_oid,
        from_input,
        hits: Vec::new(),
    }
}

/// A thread of session `session` that explains `f.txt`: a prompt, a reply, the edit and a
/// closing reply. Its entries are stamped before the commit, so the edit counts as earlier.
fn store_provenance_thread(dir: &Path, session: &str, edit: NewEdit) -> String {
    let mut edited = new_entry(
        &format!("{session}2"),
        EntryKind::ToolCall,
        "Edit f.txt",
        12,
    );
    edited.edit = Some(edit);
    let batch = new_batch(
        dir,
        session,
        "claude-opus-4",
        "main",
        vec![
            new_entry(
                &format!("{session}0"),
                EntryKind::User,
                "why is the loop written this way",
                10,
            ),
            new_entry(
                &format!("{session}1"),
                EntryKind::Assistant,
                "because the retry loop needs it",
                11,
            ),
            edited,
            new_entry(&format!("{session}3"), EntryKind::Assistant, "done", 13),
        ],
    );
    let mut store = ThreadStore::open(&dir.join(".git")).unwrap();
    store.apply(&batch).unwrap();
    ThreadStore::thread_uid("claude-code", session)
}

/// The text of a refused call, which must be an error.
fn refusal(reply: &Value) -> String {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The three lines of `f.txt` as a run of added lines, which a line match needs.
const ABC_PATCH: &str = "@@ -0,0 +1,3 @@\n+a\n+b\n+c\n";

#[test]
fn a_blob_match_is_the_strongest_evidence_and_its_entries_are_untrusted() {
    let dir = repo();
    let head = rev(dir.path(), "HEAD");
    let blob = rev(dir.path(), "HEAD:f.txt");
    let uid = store_provenance_thread(
        dir.path(),
        "blob",
        file_edit(ABC_PATCH, 3, Some(blob), false),
    );
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 3 }),
            ),
        ],
    );
    let answer = result(&replies[1]);
    assert_eq!(answer["untrusted"], true);
    assert_eq!(answer["complete"], true);
    assert!(answer["cuts"].as_array().unwrap().is_empty());
    let commits = answer["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["id"], head.as_str());
    assert_eq!(commits[0]["subject"], "init");
    assert_eq!(commits[0]["author"], "t");
    assert_eq!(commits[0]["lines"], json!([{ "from": 1, "to": 3 }]));
    let thread = &commits[0]["threads"][0];
    assert_eq!(thread["thread"], uid.as_str());
    assert_eq!(thread["ambiguous"], false);
    assert_eq!(thread["evidence"][0], "blob_match");
    let edit = &thread["edits"][0];
    assert_eq!(edit["path"], "f.txt");
    assert_eq!(edit["evidence"][0], "blob_match");
    assert!(edit["unverifiable"].is_null());
    let entries = edit["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|entry| entry["untrusted"] == true));
    assert!(
        entries
            .iter()
            .any(|entry| entry["text"] == "because the retry loop needs it")
    );
}

#[test]
fn a_line_match_is_found_when_the_edit_did_not_write_the_file_as_it_is() {
    let dir = repo();
    store_provenance_thread(dir.path(), "line", file_edit(ABC_PATCH, 3, None, false));
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 3 }),
            ),
            call(3, "provenance", &json!({ "path": "f.txt", "from_line": 2 })),
        ],
    );
    let thread = &result(&replies[1])["commits"][0]["threads"][0];
    // The edit also added lines the commit owns, which is the weaker hunk overlap.
    assert_eq!(thread["evidence"], json!(["line_match", "hunk_overlap"]));
    assert_eq!(
        thread["edits"][0]["evidence"],
        json!(["line_match", "hunk_overlap"])
    );
    let single = result(&replies[2]);
    assert_eq!(single["to_line"], 2, "to_line defaults to from_line");
    assert_eq!(
        single["commits"][0]["lines"],
        json!([{ "from": 2, "to": 2 }])
    );
}

#[test]
fn several_matching_edits_are_all_listed_and_marked_ambiguous() {
    let dir = repo();
    let first = store_provenance_thread(dir.path(), "x", file_edit(ABC_PATCH, 3, None, false));
    let second = store_provenance_thread(dir.path(), "y", file_edit(ABC_PATCH, 3, None, false));
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 3 }),
            ),
        ],
    );
    let threads = result(&replies[1])["commits"][0]["threads"]
        .as_array()
        .unwrap()
        .clone();
    let mut ids: Vec<&str> = threads
        .iter()
        .map(|t| t["thread"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    let mut expected = [first.as_str(), second.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert!(threads.iter().all(|thread| thread["ambiguous"] == true));
}

#[test]
fn an_edit_that_cannot_be_compared_is_unverifiable_not_no_match() {
    let dir = repo();
    store_provenance_thread(dir.path(), "input", file_edit(ABC_PATCH, 3, None, true));
    store_provenance_thread(
        dir.path(),
        "redacted",
        file_edit(
            "@@ -0,0 +1,3 @@\n+[REDACTED:token]\n+b\n+c\n",
            3,
            None,
            false,
        ),
    );
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 3 }),
            ),
        ],
    );
    let threads = result(&replies[1])["commits"][0]["threads"]
        .as_array()
        .unwrap()
        .clone();
    let mut reasons: Vec<String> = threads
        .iter()
        .map(|thread| {
            assert_eq!(thread["evidence"], json!([]), "no evidence is claimed");
            thread["edits"][0]["unverifiable"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    reasons.sort();
    assert_eq!(reasons, ["from_input", "redacted"]);
}

#[test]
fn a_cut_answer_says_which_bound_cut_it() {
    let dir = repo();
    let head = rev(dir.path(), "HEAD");
    let mut store = ThreadStore::open(&dir.path().join(".git")).unwrap();
    for index in 0..6 {
        let session = format!("c{index}");
        let batch = new_batch(
            dir.path(),
            &session,
            "claude-opus-4",
            "main",
            vec![new_entry(
                &format!("{session}0"),
                EntryKind::User,
                "why",
                10 + index,
            )],
        );
        store.apply(&batch).unwrap();
        let uid = ThreadStore::thread_uid("claude-code", &session);
        store
            .link(
                &uid,
                &ThreadLink {
                    kind: LinkKind::Commit,
                    reference: head.clone(),
                    origin: LinkOrigin::Human,
                    resolved: None,
                    state: None,
                },
            )
            .unwrap();
    }
    drop(store);
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 3 }),
            ),
        ],
    );
    let answer = result(&replies[1]);
    assert_eq!(answer["complete"], false);
    let cut = &answer["cuts"][0];
    assert_eq!(cut["bound"], "threads");
    assert_eq!(cut["limit"], 5);
    assert_eq!(cut["commit"], head.as_str());
    assert!(cut["text"].as_str().unwrap().starts_with("only 5 threads"));
    assert_eq!(answer["commits"][0]["threads"].as_array().unwrap().len(), 5);
}

#[test]
fn bad_provenance_arguments_are_refused_with_what_to_change() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "provenance",
                &json!({ "path": "/etc/passwd", "from_line": 1 }),
            ),
            call(
                3,
                "provenance",
                &json!({ "path": "../f.txt", "from_line": 1 }),
            ),
            call(4, "provenance", &json!({ "from_line": 1 })),
            call(5, "provenance", &json!({ "path": "f.txt" })),
            call(
                6,
                "provenance",
                &json!({ "path": "f.txt", "from_line": "3" }),
            ),
            call(
                7,
                "provenance",
                &json!({ "path": "f.txt", "from_line": -1 }),
            ),
            call(8, "provenance", &json!({ "path": "f.txt", "from_line": 0 })),
            call(
                9,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 3, "to_line": 2 }),
            ),
            call(
                10,
                "provenance",
                &json!({ "path": "f.txt", "from_line": 1, "to_line": 1001 }),
            ),
        ],
    );
    let texts: Vec<String> = replies[1..].iter().map(refusal).collect();
    assert!(
        texts[0].contains("relative to the repository root"),
        "{}",
        texts[0]
    );
    assert!(
        texts[1].contains("stay inside the repository"),
        "{}",
        texts[1]
    );
    assert_eq!(texts[2], "path is required");
    assert_eq!(texts[3], "from_line is required");
    assert_eq!(texts[4], "from_line must be an integer");
    assert!(
        texts[5].contains("from_line must be a line number"),
        "{}",
        texts[5]
    );
    assert_eq!(texts[6], "not a line range: 0 to 0");
    assert_eq!(texts[7], "not a line range: 3 to 2");
    assert_eq!(texts[8], "the range is longer than 1000 lines");
}

#[test]
fn the_provenance_schema_is_read_only_and_names_its_arguments() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    );
    let tool = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "provenance")
        .expect("provenance is listed")
        .clone();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["annotations"]["destructiveHint"], false);
    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["path", "from_line"])
    );
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    for key in ["path", "from_line", "to_line"] {
        assert!(
            tool["inputSchema"]["properties"].get(key).is_some(),
            "{key}"
        );
    }
    let description = tool["description"].as_str().unwrap();
    assert!(description.contains("complete"), "a partial answer says so");
    assert!(description.contains("blob_match"));
}

/// The label named `name` as the store holds it, read the way the app reads it.
fn stored_label(dir: &Path, name: &str) -> gm_schema::threads::LabelDetail {
    let store = ThreadStore::open(&dir.join(".git")).unwrap();
    let label = store
        .list_labels(name, 10)
        .unwrap()
        .into_iter()
        .find(|label| label.name == name)
        .expect("the label is stored");
    store.read_label(label.id).unwrap()
}

/// The reply's text must say `words`, so the agent knows what to change.
fn refused_with(reply: &Value, words: &str) {
    let text = refusal(reply);
    assert!(text.contains(words), "{text}");
}

#[test]
fn an_agent_creates_a_label_attaches_it_and_links_threads_and_the_store_keeps_its_origin() {
    let dir = repo();
    let (uid_a, uid_b) = store_search_threads(dir.path());
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "label_create",
                &json!({ "name": "needs-review", "description": "A second look", "color": "#3B82F6" }),
            ),
            call(
                3,
                "label_attach",
                &json!({ "label": "needs-review", "kind": "thread", "ref": uid_a }),
            ),
            call(
                4,
                "thread_link",
                &json!({ "from": uid_b, "to": uid_a, "kind": "continues" }),
            ),
        ],
    );
    let created = result(&replies[1]);
    assert_eq!(created["created"], true);
    assert_eq!(created["label"]["name"], "needs-review");
    assert_eq!(
        created["label"]["color"], "#3b82f6",
        "the colour is stored in lower case"
    );
    assert_eq!(created["origin"], "agent:test-client");
    let attached = result(&replies[2]);
    assert_eq!(attached["attachment"]["kind"], "thread");
    assert_eq!(attached["attachment"]["ref"], uid_a);
    assert_eq!(attached["attachment"]["origin"], "agent:test-client");
    let link = result(&replies[3]);
    assert_eq!(link["from"], uid_b);
    assert_eq!(link["to"], uid_a);
    assert_eq!(link["kind"], "continues");
    assert_eq!(link["origin"], "agent:test-client");
    let stored = stored_label(dir.path(), "needs-review");
    assert_eq!(stored.label.origin, "agent:test-client");
    assert_eq!(stored.label.description, "A second look");
    assert_eq!(stored.attachments.len(), 1);
    assert_eq!(stored.attachments[0].origin, "agent:test-client");
}

#[test]
fn a_label_an_agent_created_is_listed_and_found_by_the_threads_search_label_filter() {
    let dir = repo();
    let (_, uid_b) = store_search_threads(dir.path());
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "label_create", &json!({ "name": "agent-tag" })),
            call(
                3,
                "label_attach",
                &json!({ "label": "agent-tag", "kind": "thread", "ref": uid_b }),
            ),
            call(4, "labels_list", &json!({})),
            call(5, "threads_search", &json!({ "label": "agent-tag" })),
        ],
    );
    let listing = result(&replies[3]);
    let tag = listing["labels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|label| label["name"] == "agent-tag")
        .expect("the label is listed");
    assert_eq!(tag["attachment_count"], 1);
    assert_eq!(tag["origin"], "agent:test-client");
    assert_eq!(
        tag["untrusted"], true,
        "a label name is text an agent wrote"
    );
    assert_eq!(listing["has_more"], false);
    assert_eq!(thread_ids(&replies[4]), vec![uid_b.clone()]);
    let labels = hits_of(&replies[4])[0]["labels"].clone();
    assert!(
        labels
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label.as_str() == Some("agent-tag")),
        "{labels}"
    );
}

#[test]
fn attaching_a_pair_again_returns_the_same_attachment_and_keeps_the_first_origin() {
    let dir = repo();
    let (uid_a, _) = store_search_threads(dir.path());
    // The person attached `auth` to thread a before (store_search_threads).
    let auth = stored_label(dir.path(), "auth").label.id;
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "label_attach",
                &json!({ "label": "auth", "kind": "thread", "ref": uid_a }),
            ),
            call(
                3,
                "label_attach",
                &json!({ "label": "reviewed", "kind": "file", "ref": "f.txt" }),
            ),
            call(
                4,
                "label_attach",
                &json!({ "label": "reviewed", "kind": "file", "ref": "f.txt" }),
            ),
            call(
                5,
                "label_attach",
                &json!({ "label": "reviewed", "kind": "label", "ref": "auth" }),
            ),
        ],
    );
    let first = result(&replies[1]);
    assert_eq!(
        first["attachment"]["origin"], "human",
        "the person's origin stays"
    );
    assert_eq!(first["label"]["attachment_count"], 1);
    let (once, again) = (result(&replies[2]), result(&replies[3]));
    assert_eq!(once["attachment"]["at"], again["attachment"]["at"]);
    assert_eq!(again["label"]["attachment_count"], 1);
    assert_eq!(again["origin"], "agent:test-client");
    let stored = stored_label(dir.path(), "reviewed");
    assert_eq!(
        stored.attachments.len(),
        2,
        "the file and the label attachment"
    );
    let onto_label = result(&replies[4]);
    assert_eq!(onto_label["attachment"]["kind"], "label");
    assert_eq!(onto_label["attachment"]["ref"], auth.to_string());
}

#[test]
fn a_commit_is_attached_by_a_short_id_and_stored_with_its_snapshot() {
    let dir = repo();
    let head = rev(dir.path(), "HEAD");
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(
                2,
                "label_attach",
                &json!({ "label": "baseline", "kind": "commit", "ref": &head[..7] }),
            ),
        ],
    );
    let attachment = &result(&replies[1])["attachment"];
    assert_eq!(attachment["kind"], "commit");
    assert_eq!(attachment["ref"], head, "stored by its full id");
    assert_eq!(attachment["subject"], "init");
    assert_eq!(attachment["origin"], "agent:test-client");
}

#[test]
fn labels_list_and_label_get_read_the_labels_with_what_is_attached() {
    let dir = repo();
    let (uid_a, _) = store_search_threads(dir.path());
    let auth = stored_label(dir.path(), "auth").label.id;
    ThreadStore::open(&dir.path().join(".git"))
        .unwrap()
        .create_label("docs", "", None, "human", 2)
        .unwrap();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "labels_list", &json!({ "query": "AU" })),
            call(3, "label_get", &json!({ "label": "auth" })),
            call(4, "label_get", &json!({ "label": auth })),
            call(5, "label_get", &json!({ "label": "missing" })),
            call(6, "labels_list", &json!({ "limit": 0 })),
            call(7, "labels_list", &json!({ "limit": 1 })),
        ],
    );
    let names: Vec<&str> = result(&replies[1])["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| label["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["auth"], "the query matches in any case");
    for reply in [&replies[2], &replies[3]] {
        let detail = result(reply);
        assert_eq!(detail["label"]["name"], "auth");
        assert_eq!(detail["label"]["origin"], "human");
        let attachment = &detail["attachments"][0];
        assert_eq!(attachment["kind"], "thread");
        assert_eq!(attachment["ref"], uid_a);
        assert_eq!(attachment["origin"], "human");
        assert_eq!(attachment["untrusted"], true);
    }
    refused_with(&replies[4], "no label named \"missing\"");
    refused_with(&replies[5], "between 1 and 50");
    let listing = result(&replies[6]);
    assert_eq!(listing["labels"].as_array().unwrap().len(), 1);
    assert_eq!(
        listing["has_more"], true,
        "a row past the page says more follow"
    );
}

#[test]
fn invalid_label_and_link_arguments_are_refused_with_what_to_change() {
    let dir = repo();
    let (uid_a, _) = store_search_threads(dir.path());
    let long_name = "n".repeat(65);
    let long_description = "d".repeat(501);
    let replies = session(
        dir.path(),
        &[
            initialize(),
            call(2, "label_create", &json!({ "name": long_name })),
            call(3, "label_create", &json!({ "name": "   " })),
            call(
                4,
                "label_create",
                &json!({ "name": "x", "description": long_description }),
            ),
            call(5, "label_create", &json!({ "name": "x", "color": "red" })),
            call(
                6,
                "label_attach",
                &json!({ "label": "x", "kind": "folder", "ref": "f.txt" }),
            ),
            call(
                7,
                "label_attach",
                &json!({ "label": "x", "kind": "file", "ref": "/etc/passwd" }),
            ),
            call(
                8,
                "label_attach",
                &json!({ "label": "x", "kind": "file", "ref": "../outside.txt" }),
            ),
            call(
                9,
                "label_attach",
                &json!({ "label": "x", "kind": "commit", "ref": "HEAD" }),
            ),
            call(
                10,
                "label_attach",
                &json!({ "label": "x", "kind": "thread", "ref": "0123456789abcdef" }),
            ),
            call(
                11,
                "label_attach",
                &json!({ "label": "x", "kind": "label", "ref": "no-such-label" }),
            ),
            call(
                12,
                "thread_link",
                &json!({ "from": uid_a, "to": uid_a, "kind": "relates" }),
            ),
            call(
                13,
                "thread_link",
                &json!({ "from": uid_a, "to": "0123456789abcdef", "kind": "relates" }),
            ),
            call(
                14,
                "thread_link",
                &json!({ "from": uid_a, "to": uid_a, "kind": "duplicates" }),
            ),
            call(15, "label_get", &json!({})),
            call(16, "label_get", &json!({ "label": 42 })),
            call(17, "label_get", &json!({ "label": true })),
        ],
    );
    refused_with(&replies[1], "at most 64 characters");
    refused_with(&replies[2], "name is required");
    refused_with(&replies[3], "at most 500 characters");
    refused_with(&replies[4], "#rrggbb");
    refused_with(&replies[5], "thread, file, commit, label");
    refused_with(&replies[6], "relative");
    refused_with(&replies[7], "remove the `..`");
    refused_with(&replies[8], "7 to 40 hex digits");
    refused_with(&replies[9], "threads_search");
    refused_with(&replies[10], "no label named \"no-such-label\"");
    refused_with(&replies[11], "cannot be linked to itself");
    refused_with(&replies[12], "threads_search");
    refused_with(&replies[13], "continues, relates, supersedes");
    refused_with(&replies[14], "label is required");
    refused_with(&replies[15], "no label 42");
    refused_with(&replies[16], "must be a label's name or its integer id");
    assert!(
        !stored_label_names(dir.path()).contains(&"x".to_owned()),
        "a refused label is not stored"
    );
}

/// The names of every label in the store.
fn stored_label_names(dir: &Path) -> Vec<String> {
    let store = ThreadStore::open(&dir.join(".git")).unwrap();
    store
        .list_labels("", 100)
        .unwrap()
        .into_iter()
        .map(|label| label.name)
        .collect()
}

#[test]
fn the_record_tools_are_not_read_only_and_the_reads_are() {
    let dir = repo();
    let replies = session(
        dir.path(),
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is listed"))
            .clone()
    };
    for name in ["labels_list", "label_get"] {
        assert_eq!(tool(name)["annotations"]["readOnlyHint"], true, "{name}");
    }
    for name in ["label_create", "label_attach", "thread_link"] {
        let listed = tool(name);
        assert_eq!(listed["annotations"]["readOnlyHint"], false, "{name}");
        assert_eq!(listed["annotations"]["destructiveHint"], false, "{name}");
        assert!(listed.get("requiresUserInteraction").is_none(), "{name}");
        assert_eq!(
            listed["inputSchema"]["additionalProperties"], false,
            "{name}"
        );
    }
    assert_eq!(
        tool("label_attach")["inputSchema"]["required"],
        json!(["label", "kind", "ref"])
    );
    assert_eq!(
        tool("label_attach")["inputSchema"]["properties"]["kind"]["enum"],
        json!(["thread", "file", "commit", "label"])
    );
    assert_eq!(
        tool("thread_link")["inputSchema"]["properties"]["kind"]["enum"],
        json!(["continues", "relates", "supersedes"])
    );
    assert_eq!(
        tool("label_create")["inputSchema"]["properties"]["name"]["maxLength"],
        64
    );
    assert_eq!(
        tool("label_create")["inputSchema"]["properties"]["description"]["maxLength"],
        500
    );
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for human_only in [
        "label_rename",
        "label_delete",
        "label_detach",
        "thread_delete",
    ] {
        assert!(
            !names.contains(&human_only),
            "{human_only} is for the person"
        );
    }
}
