//! The workspace parts of the MCP tools (D214 (5)) and the agent's own label (D200) over JSON-RPC,
//! against a real workspace with two members: worktrees of two clones, each on `feat`. Each test
//! removes its directory when it ends, and the person's configuration is a temporary folder, so no
//! real configuration is read.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_core::{Engine, reviewers};
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
    /// The person's configuration folder: its reviewers and gate file.
    config: PathBuf,
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
        let config = base.join("config");
        std::fs::create_dir(&config).unwrap();
        let api = clone(&base, "api");
        let web = clone(&base, "web");
        let mut fixture = Self {
            _dir: dir,
            base,
            identity,
            config,
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

/// The workspace's own definitions: `.git/gitmaster`, where a workspace keeps them.
fn ws_gitmaster(ws: &Path) -> PathBuf {
    ws.join(".git").join("gitmaster")
}

/// The `security` reviewer of the workspace, for member `api` only.
fn write_security(ws: &Path) {
    let dir = ws_gitmaster(ws).join("reviewers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("security.md"),
        "---\ndescription: Checks unchecked input.\nsignal: pass\nsubject: branch\nmembers: api\n---\nRead the change and flag unchecked input.\n",
    )
    .unwrap();
}

/// The workspace gate `security-review`: a fresh verdict of `security`, for member `api` only.
fn write_security_gate(ws: &Path) {
    let dir = ws_gitmaster(ws);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("gates.json"),
        r#"{"gates": [{"id": "security-review", "kind": "fresh_verdict", "on": ["push"], "reviewers": ["security"], "members": ["api"]}]}"#,
    )
    .unwrap();
}

/// Runs the server on `requests` for the repository at `repo`, with the person's configuration in
/// `config`, and returns its replies.
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

/// The row of member `name` in an aggregate gate answer.
fn member_row<'a>(answer: &'a Value, name: &str) -> &'a Value {
    answer["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["member"] == name)
        .unwrap_or_else(|| panic!("no member {name} in {answer}"))
}

/// The tip of `branch` in `dir`, a full commit id.
fn tip(dir: &Path, branch: &str) -> String {
    git(dir, &["rev-parse", branch])
}

/// A verdict of `security` on `head` for member `api`, as the agent records it.
fn security_verdict(id: u64, member: Option<&str>, head: &str) -> Value {
    let mut arguments = json!({
        "reviewer": "security",
        "branch": "feat",
        "head": head,
        "value": "pass",
        "rationale": "Read the change; the input is checked."
    });
    if let Some(member) = member {
        arguments["member"] = json!(member);
    }
    call(id, "verdict_record", &arguments)
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
fn gates_status_without_a_member_answers_for_both_members_until_each_is_ready() {
    let fx = Fixture::new();
    write_security(&fx.ws);
    write_security_gate(&fx.ws);
    let head = tip(&fx.member_api, "feat");
    let replies = session(
        &fx.ws,
        &fx.config,
        &[
            initialize(),
            call(2, "gates_status", &json!({})),
            call(3, "gates_status", &json!({ "member": "api" })),
            call(4, "gates_status", &json!({ "member": "web" })),
            security_verdict(5, Some("api"), &head),
            call(6, "gates_status", &json!({})),
            call(7, "gates_status", &json!({ "member": "api" })),
        ],
    );

    let before = result(&replies[1]);
    assert_eq!(before["workspace"], true, "{before}");
    assert_eq!(
        before["ready"], false,
        "api still needs its verdict: {before}"
    );
    assert_eq!(before["untrusted"], true);
    assert_eq!(before["members"].as_array().unwrap().len(), 2, "{before}");
    let api = member_row(before, "api");
    assert_eq!(api["ready"], false, "{api}");
    assert_eq!(api["branch"], "feat");
    assert_eq!(api["error"], Value::Null);
    assert_eq!(api["missing"][0]["reviewer"], "security", "{api}");
    let gate = api["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gate| gate["id"] == "security-review")
        .expect("the workspace gate of api");
    assert_eq!(gate["applicable"], true, "{gate}");
    let web = member_row(before, "web");
    assert_eq!(web["ready"], true, "web has no gate that applies: {web}");
    assert_eq!(web["missing"], json!([]));

    let one = result(&replies[2]);
    assert_eq!(one["member"], "api", "{one}");
    assert_eq!(one["ready"], false);
    assert!(
        one.get("members").is_none(),
        "one member, not the aggregate: {one}"
    );
    assert_eq!(one["missing"][0]["reviewer"], "security");
    assert_eq!(result(&replies[3])["ready"], true);
    assert_eq!(result(&replies[3])["member"], "web");

    // The verdict for the tip makes every member ready.
    assert_eq!(result(&replies[4])["verdict"]["reviewer"], "security");
    let after = result(&replies[5]);
    assert_eq!(after["ready"], true, "{after}");
    assert_eq!(member_row(after, "api")["ready"], true);
    assert_eq!(result(&replies[6])["ready"], true);
}

#[test]
fn a_verdict_with_a_member_lands_in_its_clone_and_a_second_session_lists_it() {
    let fx = Fixture::new();
    write_security(&fx.ws);
    let head = tip(&fx.member_api, "feat");
    let recorded = session(
        &fx.ws,
        &fx.config,
        &[initialize(), security_verdict(2, Some("api"), &head)],
    );
    assert_eq!(result(&recorded[1])["verdict"]["head"], head);

    // The clone's own session, on its main checkout, sees the verdict of its branch.
    let listed = session(
        &fx.api,
        &fx.config,
        &[
            initialize(),
            call(2, "verdicts_list", &json!({ "branch": "feat" })),
        ],
    );
    let verdicts = result(&listed[1])["verdicts"].as_array().unwrap().clone();
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    assert_eq!(verdicts[0]["reviewer"], "security");
    assert_eq!(verdicts[0]["head"], head);
    assert_eq!(verdicts[0]["stale"], false);

    // The workspace's member listing names the member, and takes the member's branch by default.
    let by_member = session(
        &fx.ws,
        &fx.config,
        &[
            initialize(),
            call(2, "verdicts_list", &json!({ "member": "api" })),
        ],
    );
    assert_eq!(
        result(&by_member[1])["verdicts"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn a_workspace_refuses_a_write_or_a_read_without_a_member_and_an_unknown_member() {
    let fx = Fixture::new();
    write_security(&fx.ws);
    let head = tip(&fx.member_api, "feat");
    let replies = session(
        &fx.ws,
        &fx.config,
        &[
            initialize(),
            security_verdict(2, None, &head),
            call(3, "verdicts_list", &json!({ "branch": "feat" })),
            call(4, "gates_status", &json!({ "member": "nope" })),
            call(5, "gates_status", &json!({ "member": "" })),
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
    assert!(
        refusal(&replies[3]).contains("there is no member nope"),
        "{}",
        refusal(&replies[3])
    );
    assert!(refusal(&replies[4]).contains("member must not be empty"));
}

#[test]
fn a_member_in_a_plain_repository_is_a_tool_error() {
    let fx = Fixture::new();
    let head = tip(&fx.api, "feat");
    let replies = session(
        &fx.api,
        &fx.config,
        &[
            initialize(),
            call(
                2,
                "gates_status",
                &json!({ "branch": "feat", "member": "api" }),
            ),
            call(
                3,
                "verdict_record",
                &json!({
                    "reviewer": "security", "branch": "feat", "head": head,
                    "value": "pass", "rationale": "Read.", "member": "api"
                }),
            ),
            call(
                4,
                "verdicts_list",
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
fn reviewers_list_shows_a_workspace_reviewer_and_the_members_it_names() {
    let fx = Fixture::new();
    write_security(&fx.ws);
    let in_workspace = session(
        &fx.ws,
        &fx.config,
        &[
            initialize(),
            call(2, "reviewers_list", &json!({ "name": "security" })),
        ],
    );
    let reviewer = &result(&in_workspace[1])["reviewers"][0];
    assert_eq!(reviewer["name"], "security");
    assert_eq!(reviewer["members"], json!(["api"]), "{reviewer}");

    // A member's checkout reads the workspace's reviewer as its own: the source is the workspace.
    let in_member = session(
        &fx.member_api,
        &fx.config,
        &[
            initialize(),
            call(2, "reviewers_list", &json!({ "name": "security" })),
        ],
    );
    let seen = &result(&in_member[1])["reviewers"][0];
    assert_eq!(seen["source"], "workspace", "{seen}");
    assert!(seen["members"].is_array(), "{seen}");
}

#[test]
fn reviewer_propose_and_gate_propose_carry_members_into_a_workspace_proposal() {
    let fx = Fixture::new();
    let replies = session(
        &fx.ws,
        &fx.config,
        &[
            initialize(),
            call(
                2,
                "reviewer_propose",
                &json!({
                    "name": "api-check",
                    "description": "Checks the API.",
                    "instructions": "Read the handlers.",
                    "signal": "pass",
                    "direction": "higher_is_better",
                    "subject": "branch",
                    "members": ["api"],
                    "reason": "The api member needs a check."
                }),
            ),
            call(
                3,
                "gate_propose",
                &json!({
                    "id": "api-current",
                    "kind": "up_to_date",
                    "on": ["push"],
                    "members": ["api"],
                    "reason": "The api branch must be current."
                }),
            ),
        ],
    );
    let reviewer = &result(&replies[1])["proposal"];
    assert_eq!(reviewer["status"], "proposed", "{reviewer}");
    assert_eq!(
        reviewer["reviewer"]["members"],
        json!(["api"]),
        "{reviewer}"
    );
    assert_eq!(reviewer["untrusted"], true);
    let gate = &result(&replies[2])["proposal"];
    assert_eq!(gate["status"], "proposed", "{gate}");
    assert_eq!(gate["gate"]["members"], json!(["api"]), "{gate}");
}

#[test]
fn the_agent_label_is_part_of_the_author_and_a_bad_label_is_a_tool_error() {
    let fx = Fixture::new();
    let replies = session(
        &fx.ws,
        &fx.config,
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
        &fx.config,
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
        &fx.config,
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
        &fx.config,
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
        &fx.config,
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
        &fx.config,
        &[initialize(), call(2, "review_comments_list", &json!({}))],
    );
    assert_eq!(
        result(&member[1])["agent_threads_not_shown"],
        2,
        "{member:?}"
    );
}
