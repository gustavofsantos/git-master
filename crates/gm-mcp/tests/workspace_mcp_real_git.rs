//! The workspace parts of the MCP tools (D214 (5)) and the agent's own label (D200) over JSON-RPC,
//! against a real workspace with two members: worktrees of two clones, each on `feat`. Each test
//! removes its directory when it ends, and the person's configuration is a temporary folder, so no
//! real configuration is read.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_mcp::Session;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId};
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A clone `base/<name>` on `main` with `f.txt`, and a `feat` branch with one change to it, left
/// for a member to check out.
fn clone(base: &Path, name: &str) -> PathBuf {
    let repo = base.join(name);
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(repo.join("f.txt"), "one\nthree\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feat change"]);
    git(&repo, &["checkout", "-q", "main"]);
    repo
}

/// The workspace `pay` with the members `api` and `web`, and the paths of one fixture.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    identity: PathBuf,
    /// The main checkout of the `api` clone, on `main`.
    api: PathBuf,
    ws: PathBuf,
    member_api: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let identity = base.join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let api = clone(&base, "api");
        let web = clone(&base, "web");
        let mut fixture = Self {
            _dir: dir,
            base,
            identity,
            api,
            ws: PathBuf::new(),
            member_api: PathBuf::new(),
        };
        fixture.ws = fixture.create("pay");
        fixture.member_api = fixture.add(&fixture.ws.clone(), &fixture.api.clone(), "feat");
        fixture.add(&fixture.ws.clone(), &web, "feat");
        fixture
    }

    /// Runs `gitmaster workspace <words>` in the fixture's folder; returns status, stdout and stderr.
    fn run_in(&self, words: &[&str]) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let env = Env {
            cwd: self.base.clone(),
            home: Some(self.base.join("home")),
            worktrees_home: None,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &env,
            &git,
            Some(&self.identity),
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn create(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspaces");
        let (code, out, err) = self.run_in(&["create", name, "--root", root.to_str().unwrap()]);
        assert_eq!(code, 0, "create failed: {err}");
        let dir = root.join(name);
        assert_eq!(out, format!("{}\n", dir.display()));
        dir
    }

    /// `add <repo> feat` into `ws`; returns the member folder.
    fn add(&self, ws: &Path, repo: &Path, branch: &str) -> PathBuf {
        let (code, out, err) = self.run_in(&[
            "add",
            repo.to_str().unwrap(),
            branch,
            "--workspace",
            ws.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "add failed: {err}");
        PathBuf::from(out.trim_end())
    }
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

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "test-client", "version": "1" } } })
}

fn call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } })
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

/// The tip of `branch` in `dir`, a full commit id.
fn tip(dir: &Path, branch: &str) -> String {
    git(dir, &["rev-parse", branch])
}

/// A scorecard of `security` on `head`, for `member` when one is named, as the agent publishes it.
fn security_scorecard(id: u64, member: Option<&str>, head: &str) -> Value {
    let mut arguments = json!({
        "name": "security",
        "branch": "feat",
        "head": head,
        "status": "pass",
        "summary": {
            "label": "praise",
            "subject": "Input checked",
            "body": "Read the change; the input is checked."
        }
    });
    if let Some(member) = member {
        arguments["member"] = json!(member);
    }
    call(id, "scorecard_publish", &arguments)
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
            path: path.into(),
            text: text.into(),
            body: body.into(),
        },
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::OperationChanged {
                state: gm_schema::OperationState::Done,
                ..
            }
        )),
        "{events:?}"
    );
}

#[test]
fn a_scorecard_with_a_member_lands_in_its_clone_and_a_second_session_lists_it() {
    let fx = Fixture::new();
    let head = tip(&fx.member_api, "feat");
    let recorded = session(
        &fx.ws,
        &[initialize(), security_scorecard(2, Some("api"), &head)],
    );
    assert_eq!(result(&recorded[1])["scorecard"]["head"], head);

    // The clone's own session, on its main checkout, sees the scorecard of its branch.
    let listed = session(
        &fx.api,
        &[
            initialize(),
            call(2, "scorecards_list", &json!({ "branch": "feat" })),
        ],
    );
    let scorecards = result(&listed[1])["scorecards"].as_array().unwrap().clone();
    assert_eq!(scorecards.len(), 1, "{scorecards:?}");
    assert_eq!(scorecards[0]["name"], "security");
    assert_eq!(scorecards[0]["head"], head);
    assert_eq!(scorecards[0]["stale"], false);

    // The workspace's listing names the member, and takes the member's branch by default.
    let by_member = session(
        &fx.ws,
        &[
            initialize(),
            call(2, "scorecards_list", &json!({ "member": "api" })),
        ],
    );
    assert_eq!(
        result(&by_member[1])["scorecards"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_workspace_refuses_a_write_or_a_read_without_a_member_and_an_unknown_member() {
    let fx = Fixture::new();
    let head = tip(&fx.member_api, "feat");
    let replies = session(
        &fx.ws,
        &[
            initialize(),
            security_scorecard(2, None, &head),
            call(3, "scorecards_list", &json!({ "branch": "feat" })),
        ],
    );
    assert!(
        refusal(&replies[1]).contains("name the member"),
        "{}",
        refusal(&replies[1])
    );
    assert!(
        refusal(&replies[2]).contains("name the member"),
        "{}",
        refusal(&replies[2])
    );
}

#[test]
fn a_member_in_a_plain_repository_is_a_tool_error() {
    let fx = Fixture::new();
    let head = tip(&fx.api, "feat");
    let replies = session(
        &fx.api,
        &[
            initialize(),
            call(
                3,
                "scorecard_publish",
                &json!({
                    "name": "security", "branch": "feat", "head": head, "status": "pass",
                    "summary": { "label": "praise", "subject": "Read.", "body": "Read." },
                    "member": "api"
                }),
            ),
            call(
                4,
                "scorecards_list",
                &json!({ "branch": "feat", "member": "api" }),
            ),
        ],
    );
    for reply in &replies[1..] {
        assert!(
            refusal(reply).contains("this repository is not a workspace"),
            "{}",
            refusal(reply)
        );
    }
}

#[test]
fn the_agent_label_is_part_of_the_author_and_a_bad_label_is_a_tool_error() {
    let fx = Fixture::new();
    let replies = session(
        &fx.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "three", "kind": "note",
                         "body": "three is read once", "agent": "billing" }),
            ),
            call(
                3,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "no label here" }),
            ),
            call(
                4,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "bad", "agent": "bad label!" }),
            ),
            call(
                5,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "bad", "agent": "" }),
            ),
            call(
                6,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "bad", "agent": "a".repeat(41) }),
            ),
            call(
                7,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "long enough", "agent": "a".repeat(40) }),
            ),
        ],
    );
    assert_eq!(result(&replies[1])["author"], "agent:test-client/billing");
    assert_eq!(result(&replies[2])["author"], "agent:test-client");
    for reply in &replies[3..6] {
        assert_eq!(
            refusal(reply),
            "agent must be 1 to 40 letters, digits, '.', '_' or '-'"
        );
    }
    assert_eq!(
        result(&replies[6])["author"],
        format!("agent:test-client/{}", "a".repeat(40))
    );
}

#[test]
fn another_label_of_the_same_client_cannot_hand_off_an_agent_thread() {
    let fx = Fixture::new();
    let created = session(
        &fx.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note",
                         "body": "one is read once", "agent": "billing" }),
            ),
        ],
    );
    let id = result(&created[1])["id"].as_i64().unwrap();
    let replies = session(
        &fx.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_resolve",
                &json!({ "id": id, "summary": "done", "agent": "ops" }),
            ),
            call(
                3,
                "review_comment_resolve",
                &json!({ "id": id, "summary": "done" }),
            ),
            call(
                4,
                "review_comment_resolve",
                &json!({ "id": id, "summary": "done", "agent": "billing" }),
            ),
        ],
    );
    for reply in &replies[1..] {
        assert!(
            refusal(reply).contains("an agent cannot hand off a thread it started"),
            "{}",
            refusal(reply)
        );
    }
}

#[test]
fn agent_threads_not_shown_counts_the_agent_threads_the_default_filter_leaves_out() {
    let fx = Fixture::new();
    person_comment(&fx.ws, "api/f.txt", "three", "say why three matters");
    session(
        &fx.ws,
        &[
            initialize(),
            call(
                2,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "one", "kind": "note", "body": "one is read once" }),
            ),
            call(
                3,
                "review_comment_create",
                &json!({ "path": "api/f.txt", "anchor_text": "three", "kind": "question",
                         "body": "is three the last line?", "agent": "review-2" }),
            ),
        ],
    );

    let replies = session(
        &fx.ws,
        &[
            initialize(),
            call(2, "review_comments_list", &json!({})),
            call(3, "review_comments_list", &json!({ "author": "any" })),
            call(4, "review_comments_list", &json!({ "author": "agent" })),
        ],
    );
    let default = result(&replies[1]);
    assert_eq!(
        default["comments"].as_array().unwrap().len(),
        1,
        "{default}"
    );
    assert_eq!(default["agent_threads_not_shown"], 2, "{default}");
    let everyone = result(&replies[2]);
    assert_eq!(
        everyone["comments"].as_array().unwrap().len(),
        3,
        "{everyone}"
    );
    assert!(
        everyone.get("agent_threads_not_shown").is_none(),
        "{everyone}"
    );
    assert!(result(&replies[3]).get("agent_threads_not_shown").is_none());

    // The member's merged listing counts the same threads.
    let member = session(
        &fx.member_api,
        &[initialize(), call(2, "review_comments_list", &json!({}))],
    );
    assert_eq!(
        result(&member[1])["agent_threads_not_shown"],
        2,
        "{member:?}"
    );
}
