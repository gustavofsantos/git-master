//! A workspace's review comments seen from its member through the MCP server (D135, D174, D200):
//! two stdio sessions, one on the workspace and one on the member repository, against a real
//! workspace whose member is a linked worktree of a clone. Each test removes its directory when
//! it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_schema::{Actor, Command, CommandKind, OperationId};
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
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

/// A workspace `pay` whose member `api` is the worktree of a clone on `feat`, where `f.txt` reads
/// "one" and "three". The clone's main checkout stays on `main`. The workspace and its member are
/// made by `gitmaster workspace`, which also writes the pointer the member's repository reads.
struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    member: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let identity = base.join("identity.gitconfig");
    std::fs::write(
        &identity,
        "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
    )
    .unwrap();
    let repo = base.join("api");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "f.txt"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    git(&repo, &["checkout", "-q", "main"]);

    let root = base.join("workspaces");
    let ws = root.join("pay");
    let member = ws.join("api");
    let words = |items: &[&str]| -> Vec<String> { items.iter().map(|w| (*w).to_owned()).collect() };
    let git_config = SubprocessGit {
        global_config: Some(identity.clone()),
        ..SubprocessGit::default()
    };
    let env = Env {
        cwd: base.clone(),
        home: Some(base.join("home")),
        worktrees_home: None,
    };
    let run = |items: &[&str]| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words(items),
            &env,
            &git_config,
            Some(&identity),
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0, "{items:?}: {}", String::from_utf8_lossy(&err));
    };
    run(&["create", "pay", "--root", root.to_str().unwrap()]);
    run(&[
        "add",
        repo.to_str().unwrap(),
        "feat",
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    Fixture {
        _dir: dir,
        ws,
        member,
    }
}

/// Runs `gitmaster mcp` with `words` in `cwd` on `requests`, and returns its replies.
fn session_at(words: &[String], cwd: &Path, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let code = gm_mcp::command(words, cwd, None, input.as_bytes(), &mut output, &mut errors);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&errors));
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// A session on the workspace: its own store.
fn workspace_session(ws: &Path, requests: &[Value]) -> Vec<Value> {
    session_at(&[], ws, requests)
}

/// A session on the member repository, opened with `--repo` as an agent's client starts it.
fn member_session(member: &Path, requests: &[Value]) -> Vec<Value> {
    session_at(
        &["--repo".to_owned(), member.to_string_lossy().into_owned()],
        member,
        requests,
    )
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

/// A comment the person writes in the workspace, through the engine, as the app does.
fn person_comment(ws: &Path, path: &str, text: &str, body: &str) {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let mut human = |id: u64, kind| {
        engine.handle(
            OperationId(id),
            &Command {
                actor: Actor::Human,
                kind,
            },
        )
    };
    human(
        1,
        CommandKind::OpenRepo {
            path: ws.to_string_lossy().into_owned(),
        },
    );
    let events = human(
        2,
        CommandKind::AddReviewCommentByText {
            label: None,
            path: path.into(),
            text: text.into(),
            body: body.into(),
        },
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            gm_schema::Event::OperationChanged {
                state: gm_schema::OperationState::Done,
                ..
            }
        )),
        "{events:?}"
    );
}

/// The comment of `comments` with `body`, from a listing result.
fn with_body<'a>(comments: &'a [Value], body: &str) -> &'a Value {
    comments
        .iter()
        .find(|comment| comment["body"] == body)
        .unwrap_or_else(|| panic!("no comment {body:?} in {comments:?}"))
}

#[test]
fn the_workspace_agents_comments_are_listed_to_the_member_agent_under_composite_ids() {
    let fixture = fixture();
    let replies = workspace_session(
        &fixture.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "three", "kind": "note", "body": "three is read once" }),
            ),
            call(
                3,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "question", "body": "is one the first line?" }),
            ),
            call(4, "review_comments_list", &json!({ "author": "any" })),
        ],
    );
    assert_eq!(result(&replies[1])["path"], "api/f.txt");
    assert_eq!(result(&replies[2])["path"], "api/f.txt");
    assert_eq!(
        result(&replies[3])["comments"].as_array().unwrap().len(),
        2,
        "the workspace lists its own two"
    );

    let member = member_session(
        &fixture.member,
        &[
            initialize(),
            call(2, "review_comments_list", &json!({ "author": "any" })),
            call(3, "review_comments_list", &json!({})),
        ],
    );
    let seen = result(&member[1])["comments"].as_array().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for comment in &seen {
        assert_eq!(comment["path"], "f.txt");
        assert_eq!(comment["workspace"], "pay");
        assert_eq!(comment["author"], "agent:test-client");
        assert_eq!(comment["outdated"], false, "{comment}");
        assert!(
            comment["id"].as_i64().unwrap() >= 1 << 40,
            "a composite id: {comment}"
        );
    }
    let question = with_body(&seen, "is one the first line?");
    assert_eq!(question["kind"], "question");
    assert_eq!(question["needs_answer"], true);
    assert_eq!(question["start_line"], 1);
    // The default list holds the person's threads only, so an agent's own notes are not in it.
    assert!(
        result(&member[2])["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_member_agent_hands_off_a_person_comment_and_the_workspace_sees_it_handled() {
    let fixture = fixture();
    person_comment(&fixture.ws, "api/f.txt", "three", "say why three matters");
    workspace_session(
        &fixture.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note", "body": "one is read once" }),
            ),
        ],
    );

    // The member agent finds the person's comment by its composite id.
    let listed = member_session(
        &fixture.member,
        &[
            initialize(),
            call(2, "review_comments_list", &json!({ "author": "any" })),
        ],
    );
    let comments = result(&listed[1])["comments"].as_array().unwrap().clone();
    let person = with_body(&comments, "say why three matters")["id"]
        .as_i64()
        .unwrap();
    let agent_note = with_body(&comments, "one is read once")["id"]
        .as_i64()
        .unwrap();

    let replies = member_session(
        &fixture.member,
        &[
            initialize(),
            call(
                2,
                "review_comment_resolve",
                &json!({ "id": person, "summary": "the third line is the last one" }),
            ),
            call(
                3,
                "review_comment_resolve",
                &json!({ "id": agent_note, "summary": "closing my own note" }),
            ),
        ],
    );
    let handed = result(&replies[1]);
    assert_eq!(handed["id"], person);
    assert_eq!(handed["status"], "handled");
    assert_eq!(
        handed["replies"].as_array().unwrap().last().unwrap()["body"],
        "the third line is the last one"
    );
    assert!(
        handed["change"].is_object(),
        "the change is observed: {handed}"
    );
    // The agent cannot hand off a thread it started, as in the repository (D200).
    assert!(
        refusal(&replies[2]).contains("cannot hand off"),
        "{}",
        refusal(&replies[2])
    );

    // The workspace's own agent sees the same comment handled, with the member's summary.
    let ws = workspace_session(
        &fixture.ws,
        &[
            initialize(),
            call(2, "review_comments_list", &json!({ "author": "any" })),
        ],
    );
    let stored = result(&ws[1])["comments"].as_array().unwrap().clone();
    let shown = with_body(&stored, "say why three matters");
    assert_eq!(shown["status"], "handled");
    assert_eq!(shown["workspace"], Value::Null);
    assert_eq!(
        shown["replies"].as_array().unwrap().last().unwrap()["body"],
        "the third line is the last one"
    );
}
