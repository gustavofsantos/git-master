//! An agent on a workspace reads and proposes the memories of its members over JSON-RPC (D216 (3),
//! (5), D222 (5), D218 (1)), against a real workspace with two members, each a worktree of its own clone. The
//! person's side (accepting, writing as a person) runs through the engine as the human, on the same
//! contexts file the server reads. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;
use gm_mcp::Session;
use gm_schema::threads::{MemoryBasis, MemoryKind, MemoryStatus};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A clone `base/<name>` on `main` with `f.txt`, and a `feat` branch with one change to it, left for
/// a member to check out.
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
    /// The contexts file, not there yet.
    db: PathBuf,
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
            db: base.join("data").join("contexts.db"),
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

    /// Runs `gitmaster workspace <words>`; returns status, stdout and stderr.
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

    /// An engine on `root` as the person, on the fixture's contexts file and workspace configuration.
    fn engine_on(&self, root: &Path) -> Engine {
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let mut engine = Engine::new(Box::new(git));
        engine.set_contexts_db(Some(self.db.clone()));
        engine.set_workspace_global_config(Some(self.identity.clone()));
        let events = send(
            &mut engine,
            &Actor::Human,
            CommandKind::OpenRepo {
                path: root.to_string_lossy().into_owned(),
            },
        );
        assert_eq!(ended(&events), OperationState::Done, "open: {events:?}");
        engine
    }
}

fn send(engine: &mut Engine, actor: &Actor, kind: CommandKind) -> Vec<Event> {
    engine.handle(
        OperationId(1),
        &Command {
            actor: actor.clone(),
            kind,
        },
    )
}

/// The state the operation ended in.
fn ended(events: &[Event]) -> OperationState {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, .. } if state.is_terminal() => Some(*state),
            _ => None,
        })
        .expect("the operation ends")
}

/// The memory the person writes in `engine` as `text`, accepted at once.
fn human_memory(engine: &mut Engine, text: &str) -> String {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            sources: Vec::new(),
            context: None,
            basis: MemoryBasis::Observed,
            member: None,
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "propose: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged {
                memory: Some(memory),
                ..
            } => Some(memory.uid.clone()),
            _ => None,
        })
        .expect("a memory")
}

/// The uid of the memory with `text` in the person's list of `engine`, whatever its status.
fn uid_of(engine: &mut Engine, text: &str) -> String {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListMemories {
            query: String::new(),
            kind: None,
            status: None,
            path: None,
            author: None,
            scope: None,
            limit: 100,
            cursor: None,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed { memories, .. } => memories
                .iter()
                .find(|memory| memory.text == text)
                .map(|memory| memory.uid.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{text} is listed"))
}

/// Runs the server on `requests` for the workspace at `repo`, with the fixture's contexts file, and
/// returns its replies.
fn session(fx: &Fixture, requests: &[Value]) -> Vec<Value> {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_contexts_db(Some(fx.db.clone()));
    engine.set_workspace_global_config(Some(fx.identity.clone()));
    let mut server = Session::open(engine, &fx.ws.to_string_lossy()).expect("the workspace opens");
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

fn file_source() -> Value {
    json!([{ "kind": "file", "ref": "f.txt" }])
}

#[test]
fn an_agent_on_a_workspace_proposes_into_a_member_marks_its_memory_and_searches_it() {
    let fx = Fixture::new();
    let mut api = fx.engine_on(&fx.api);
    // The person's memory in the clone goes to the clone's own context, `api`, which is then the one
    // context the workspace and its members share (D222 (4), (5)).
    let rule = human_memory(&mut api, "api rule over mcp");
    let replies = session(
        &fx,
        &[
            initialize(),
            call(
                2,
                "memory_propose",
                &json!({ "kind": "rule", "text": "api proposal over mcp", "sources": file_source() }),
            ),
            call(
                3,
                "memory_propose",
                &json!({ "kind": "fact", "text": "workspace proposal over mcp", "sources": file_source() }),
            ),
            call(
                4,
                "memory_mark",
                &json!({ "memory_id": rule, "mark": "useful", "reason": "it helped" }),
            ),
            call(
                5,
                "memories_search",
                &json!({ "scope": "api", "status": "proposed" }),
            ),
            call(6, "memory_get", &json!({ "id": rule })),
        ],
    );

    let proposed = &result(&replies[1])["memory"];
    assert_eq!(proposed["scope"], "api");
    assert_eq!(proposed["status"], "proposed");
    assert_eq!(proposed["origin"], "agent:test-client");
    assert_eq!(result(&replies[1])["created"], true);
    // Without a context, the one context bound to the workspace and its members takes it (D222 (5)).
    let own = &result(&replies[2])["memory"];
    assert_eq!(own["scope"], "api");
    assert_eq!(own["status"], "proposed");

    let marked = result(&replies[3]);
    assert_eq!(marked["recorded"], true);
    // The clone's own memory is in its context, labelled by it (D222 (4)).
    assert_eq!(marked["memory"]["scope"], "api");
    assert!(
        marked["memory"]["marks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mark| mark["actor"] == "agent:test-client" && mark["mark"] == "useful"),
        "{marked}"
    );

    let listed = result(&replies[4])["memories"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|memory| memory["text"] == "api proposal over mcp" && memory["scope"] == "api"),
        "{listed:?}"
    );
    assert!(
        listed
            .iter()
            .any(|memory| memory["text"] == "workspace proposal over mcp"
                && memory["scope"] == "api"),
        "{listed:?}"
    );

    let read = &result(&replies[5])["memory"];
    assert_eq!(read["scope"], "api");
    assert!(
        read["marks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mark| mark["actor"] == "agent:test-client" && mark["reason"] == "it helped"),
        "{read}"
    );

    // The mark is in the member's own store: the person reads it there.
    let read_rule = send(
        &mut api,
        &Actor::Human,
        CommandKind::ReadMemory { uid: rule },
    );
    assert!(
        read_rule.iter().any(|event| match event {
            Event::MemoryRead { memory, .. } => memory
                .marks
                .iter()
                .any(|mark| mark.actor == "agent:test-client"),
            _ => false,
        }),
        "{read_rule:?}"
    );
}

#[test]
fn an_agent_on_a_workspace_cannot_accept_reject_or_delete_a_member_memory() {
    let fx = Fixture::new();
    let replies = session(
        &fx,
        &[
            initialize(),
            call(
                2,
                "memory_propose",
                &json!({ "kind": "lesson", "text": "stays proposed", "sources": file_source() }),
            ),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
            call(
                4,
                "memory_accept",
                &json!({ "memory_id": "0123456789abcdef" }),
            ),
            call(
                5,
                "memory_reject",
                &json!({ "memory_id": "0123456789abcdef" }),
            ),
            call(
                6,
                "memory_delete",
                &json!({ "memory_id": "0123456789abcdef" }),
            ),
        ],
    );
    let names: Vec<&str> = replies[2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"memory_propose"), "{names:?}");
    assert!(names.contains(&"memory_mark"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.contains("accept")
            || name.contains("reject")
            || name.contains("delete")),
        "{names:?}"
    );
    for reply in &replies[3..] {
        assert_eq!(reply["result"]["isError"], true, "{reply}");
    }

    // Without a context the proposal goes to the workspace's own context, which the workspace reads.
    let mut ws = fx.engine_on(&fx.ws);
    let uid = uid_of(&mut ws, "stays proposed");
    let events = send(&mut ws, &Actor::Human, CommandKind::ReadMemory { uid });
    let status = events.iter().find_map(|event| match event {
        Event::MemoryRead { memory, .. } => Some(memory.status),
        _ => None,
    });
    assert_eq!(status, Some(MemoryStatus::Proposed), "{events:?}");
}

#[test]
fn a_member_argument_is_refused_and_the_server_keeps_answering() {
    let fx = Fixture::new();
    let replies = session(
        &fx,
        &[
            initialize(),
            call(
                2,
                "memory_propose",
                &json!({ "kind": "fact", "text": "nowhere", "sources": file_source(), "member": "api" }),
            ),
            call(
                3,
                "memory_propose",
                &json!({ "kind": "fact", "text": "still here", "sources": file_source() }),
            ),
        ],
    );
    assert_eq!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    let text = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
    // `member` no longer exists (K2): the memory is never filed in a member by accident.
    assert!(text.contains("member no longer exists"), "{text}");
    // Nothing is bound yet, so the workspace's own context takes the proposal (D222 (5)).
    assert_eq!(result(&replies[2])["memory"]["scope"], "pay");
}
