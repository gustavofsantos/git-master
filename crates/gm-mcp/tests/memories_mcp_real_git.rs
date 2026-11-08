//! The memory and context tools (D190, D192, D193) end to end: JSON-RPC lines in, a real
//! repository and a real `contexts.db` behind them. The person's side (accepting, creating
//! contexts, joining repositories) runs through the engine as the human, on the same contexts
//! file the server reads.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_mcp::Session;
use gm_schema::threads::{MemoryBasis, MemoryKind, MemorySourceInput, MemorySourceKind};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};
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

/// A repository at `dir` with one commit that holds `f.txt`.
fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "a\nb\nc\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// A temporary directory with a repository `repo` and a contexts file `db` that is not there yet.
/// The directory is returned so it lives as long as the test.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    make_repo(&repo);
    let db = tmp.path().join("contexts.db");
    (tmp, repo, db)
}

/// Runs the server on `requests` for the repository at `repo`, with the contexts file at `db`.
fn session(repo: &Path, db: &Path, requests: &[Value]) -> Vec<Value> {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_contexts_db(Some(db.to_path_buf()));
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

fn result(reply: &Value) -> &Value {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    &reply["result"]["structuredContent"]
}

/// An engine on `repo` with the contexts file `db`, opened as the person.
fn engine_on(repo: &Path, db: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    engine.set_contexts_db(Some(db.to_path_buf()));
    let events = send(
        &mut engine,
        &Actor::Human,
        CommandKind::OpenRepo {
            path: repo.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "open: {events:?}");
    engine
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

fn agent() -> Actor {
    Actor::Agent {
        name: "test-client".into(),
    }
}

fn file_source(path: &str) -> MemorySourceInput {
    MemorySourceInput {
        kind: MemorySourceKind::File,
        reference: path.into(),
        entry_from: None,
        entry_to: None,
    }
}

/// Proposes a memory as an agent and returns its uid. `context` is a context name, if any.
fn propose_as_agent(engine: &mut Engine, text: &str, path: &str, context: Option<&str>) -> String {
    propose(engine, &agent(), text, path, context)
}

/// Writes a memory as the person; it is accepted at once. Returns its uid.
fn propose_as_human(engine: &mut Engine, text: &str, path: &str, context: Option<&str>) -> String {
    propose(engine, &Actor::Human, text, path, context)
}

fn propose(
    engine: &mut Engine,
    actor: &Actor,
    text: &str,
    path: &str,
    context: Option<&str>,
) -> String {
    let events = send(
        engine,
        actor,
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.into(),
            sources: vec![file_source(path)],
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
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

/// Accepts memory `uid` as the person, superseding `supersedes`.
fn accept(engine: &mut Engine, uid: &str, supersedes: Vec<String>) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: uid.to_owned(),
            text: None,
            supersedes,
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "accept: {events:?}");
}

/// Creates a context under the root as the person and returns its id.
fn create_context(engine: &mut Engine, name: &str) -> i64 {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::CreateContext {
            name: name.into(),
            parent: None,
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "create: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsChanged {
                context: Some(context),
                ..
            } => Some(context.id),
            _ => None,
        })
        .expect("the context")
}

/// Adds the repository at `path` to context `context` as the person.
fn join(engine: &mut Engine, context: i64, path: &Path) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::AddRepoToContext {
            context,
            path: path.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "join: {events:?}");
}

/// Commits a new content of `f.txt`, after the memories that name it were accepted.
fn change_f(repo: &Path) {
    std::fs::write(repo.join("f.txt"), "a\nb\nchanged\n").unwrap();
    git(repo, &["commit", "-q", "-am", "change f.txt"]);
}

#[test]
fn an_agent_proposes_a_memory_and_a_repeat_of_its_text_returns_the_same_one() {
    let (_tmp, repo, db) = fixture();
    let proposal = json!({
        "kind": "rule",
        "text": "Run the release checks before tagging.",
        "sources": [{ "kind": "file", "ref": "f.txt" }]
    });
    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memory_propose", &proposal),
            call(3, "memory_propose", &proposal),
        ],
    );
    let first = result(&replies[1]);
    assert_eq!(first["created"], true);
    assert_eq!(first["origin"], "agent:test-client");
    assert_eq!(first["memory"]["status"], "proposed");
    assert_eq!(first["memory"]["reviewed"], false);
    assert_eq!(first["memory"]["untrusted"], true);
    assert_eq!(
        first["memory"]["text"],
        "Run the release checks before tagging."
    );
    let again = result(&replies[2]);
    assert_eq!(again["created"], false);
    assert_eq!(again["memory"]["id"], first["memory"]["id"]);
}

#[test]
fn a_proposal_without_a_source_is_refused_and_the_server_keeps_answering() {
    let (_tmp, repo, db) = fixture();
    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_propose",
                &json!({ "kind": "fact", "text": "No source." }),
            ),
            call(
                3,
                "memory_propose",
                &json!({ "kind": "fact", "text": "Empty list.", "sources": [] }),
            ),
            call(4, "contexts_list", &json!({})),
        ],
    );
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[1]["result"]["isError"], true, "{}", replies[1]);
    assert_eq!(replies[2]["result"]["isError"], true, "{}", replies[2]);
    assert!(
        replies[2]["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("at least one source")),
        "{}",
        replies[2]
    );
    assert_eq!(replies[3]["result"]["isError"], false, "{}", replies[3]);
}

#[test]
fn a_search_returns_only_accepted_memories_unless_a_status_is_asked_for() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let accepted = propose_as_agent(&mut human, "Keep the ledger append-only.", "f.txt", None);
    accept(&mut human, &accepted, Vec::new());
    let proposed = propose_as_agent(&mut human, "Prefer rebase over merge.", "f.txt", None);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memories_search", &json!({})),
            call(3, "memories_search", &json!({ "status": "proposed" })),
        ],
    );

    let default = result(&replies[1]);
    let memories = default["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "{default}");
    assert_eq!(memories[0]["id"], accepted);
    assert_eq!(memories[0]["status"], "accepted");
    assert_eq!(memories[0]["reviewed"], true);

    let pending = result(&replies[2]);
    let memories = pending["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "{pending}");
    assert_eq!(memories[0]["id"], proposed);
    assert_eq!(memories[0]["status"], "proposed");
    assert_eq!(memories[0]["reviewed"], false);
    assert_eq!(memories[0]["origin"], "agent:test-client");

    for memory in default["memories"]
        .as_array()
        .unwrap()
        .iter()
        .chain(memories)
    {
        assert_eq!(memory["untrusted"], true, "{memory}");
    }
}

#[test]
fn a_memory_read_shows_its_sources_its_supersession_and_a_file_changed_since_acceptance() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let old = propose_as_agent(&mut human, "Return errors, never panic.", "f.txt", None);
    accept(&mut human, &old, Vec::new());
    let new = propose_as_agent(&mut human, "Return errors as Result.", "f.txt", None);
    accept(&mut human, &new, vec![old.clone()]);
    change_f(&repo);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memory_get", &json!({ "id": new })),
            call(3, "memory_get", &json!({ "id": old })),
            call(4, "memory_get", &json!({ "id": "0123456789abcdef" })),
        ],
    );

    let newer = &result(&replies[1])["memory"];
    assert_eq!(newer["untrusted"], true);
    assert_eq!(newer["status"], "accepted");
    assert_eq!(newer["supersedes"], json!([old]));
    assert_eq!(newer["proposed_by"], "agent:test-client");
    assert!(newer["created_at"].is_i64(), "{newer}");
    assert!(newer["decided_at"].is_i64(), "{newer}");
    assert_eq!(newer["sources"][0]["kind"], "file");
    assert_eq!(newer["sources"][0]["ref"], "f.txt");
    assert_eq!(newer["sources"][0]["repo_id"], Value::Null);
    assert_eq!(
        newer["files_changed_since"],
        json!([{ "path": "f.txt", "state": "changed" }])
    );

    let older = &result(&replies[2])["memory"];
    assert_eq!(older["status"], "superseded");
    assert_eq!(older["superseded_by"], json!([new]));
    assert_eq!(
        older["files_changed_since"],
        json!([{ "path": "f.txt", "state": "changed" }])
    );

    assert_eq!(replies[3]["result"]["isError"], true, "{}", replies[3]);
}

#[test]
fn contexts_connect_two_repositories_and_a_memory_of_one_is_found_from_the_other() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_a = tmp.path().join("a");
    let repo_b = tmp.path().join("b");
    make_repo(&repo_a);
    make_repo(&repo_b);
    let db = tmp.path().join("contexts.db");

    let mut a = engine_on(&repo_a, &db);
    let platform = create_context(&mut a, "Platform");
    join(&mut a, platform, &repo_a);
    join(&mut a, platform, &repo_b);
    let shared = propose_as_human(
        &mut a,
        "Billing and the API share one ledger schema.",
        "f.txt",
        Some("Platform"),
    );

    let mut b = engine_on(&repo_b, &db);
    let local = propose_as_agent(
        &mut b,
        "Repo B deploys from the release branch.",
        "f.txt",
        None,
    );
    accept(&mut b, &local, Vec::new());

    let replies = session(
        &repo_b,
        &db,
        &[
            initialize(),
            call(2, "contexts_list", &json!({})),
            call(3, "memories_search", &json!({})),
            call(4, "memories_search", &json!({ "scope": "repository" })),
            call(5, "memories_search", &json!({ "scope": "Platform" })),
            call(
                6,
                "memory_propose",
                &json!({
                    "kind": "lesson",
                    "text": "Ledger changes need a migration note.",
                    "sources": [{ "kind": "file", "ref": "f.txt" }],
                    "context": "Platform"
                }),
            ),
            call(
                7,
                "memory_propose",
                &json!({
                    "kind": "lesson",
                    "text": "Ledger changes need a migration note.",
                    "sources": [{ "kind": "file", "ref": "f.txt" }],
                    "context": "Nope"
                }),
            ),
        ],
    );

    let listed = result(&replies[1]);
    assert!(listed.get("clash").is_none(), "{listed}");
    let contexts = listed["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1, "{listed}");
    assert_eq!(contexts[0]["name"], "Platform");
    assert_eq!(contexts[0]["path"], "Platform");
    assert_eq!(contexts[0]["accepted_memories"], 1);

    let everything = result(&replies[2])["memories"].as_array().unwrap().clone();
    let shared_hit = everything
        .iter()
        .find(|memory| memory["id"] == shared.as_str())
        .expect("the shared memory is found");
    assert_eq!(shared_hit["scope"], "Platform");
    assert_eq!(shared_hit["untrusted"], true);

    let own = result(&replies[3])["memories"].as_array().unwrap().clone();
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0]["id"], local.as_str());
    assert_eq!(own[0]["scope"], "repository");

    let by_context = result(&replies[4])["memories"].as_array().unwrap().clone();
    assert_eq!(by_context.len(), 1, "{by_context:?}");
    assert_eq!(by_context[0]["id"], shared.as_str());

    let proposed = result(&replies[5]);
    assert_eq!(proposed["created"], true);
    assert_eq!(proposed["memory"]["status"], "proposed");

    assert_eq!(replies[6]["result"]["isError"], true, "{}", replies[6]);
}

#[test]
fn pages_of_memories_follow_the_cursor_to_the_end_without_repeats() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let mut uids = Vec::new();
    for text in ["The first rule.", "The second rule.", "The third rule."] {
        let uid = propose_as_agent(&mut human, text, "f.txt", None);
        accept(&mut human, &uid, Vec::new());
        uids.push(uid);
    }

    // Each page is a call of its own, as an agent makes it: the cursor carries the position.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..=uids.len() {
        let mut arguments = json!({ "limit": 1 });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let replies = session(
            &repo,
            &db,
            &[initialize(), call(2, "memories_search", &arguments)],
        );
        let page = result(&replies[1]);
        for memory in page["memories"].as_array().unwrap() {
            seen.push(memory["id"].as_str().unwrap().to_owned());
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    assert!(cursor.is_none(), "the last page has no next cursor");
    assert_eq!(seen.len(), uids.len(), "{seen:?}");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a memory repeated: {seen:?}");
    for uid in &uids {
        assert!(seen.contains(uid), "{uid} is missing from {seen:?}");
    }
}

/// The note an unconfirmed memory carries in every result (D201 (1)).
const UNCONFIRMED_NOTE: &str = "Unconfirmed: a belief, not established fact";

/// Proposes a memory of `basis` as an agent and returns its uid.
fn propose_with_basis(engine: &mut Engine, text: &str, basis: MemoryBasis) -> String {
    let events = send(
        engine,
        &agent(),
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.into(),
            sources: vec![file_source("f.txt")],
            context: None,
            basis,
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

#[test]
fn a_proposal_records_its_basis_and_a_bad_one_is_refused_and_the_server_answers() {
    let (_tmp, repo, db) = fixture();
    let source = json!([{ "kind": "file", "ref": "f.txt" }]);
    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_propose",
                &json!({ "kind": "rule", "text": "Observed by default.", "sources": source }),
            ),
            call(
                3,
                "memory_propose",
                &json!({ "kind": "rule", "text": "Checked by a run.", "basis": "tested", "sources": source }),
            ),
            call(
                4,
                "memory_propose",
                &json!({ "kind": "rule", "text": "A belief.", "basis": "unconfirmed", "sources": source }),
            ),
            call(
                5,
                "memory_propose",
                &json!({ "kind": "rule", "text": "Guessed.", "basis": "guessed", "sources": source }),
            ),
            call(6, "contexts_list", &json!({})),
        ],
    );
    assert_eq!(replies.len(), 6, "{replies:?}");

    assert_eq!(result(&replies[1])["memory"]["basis"], "observed");
    assert!(result(&replies[1])["memory"].get("note").is_none());
    assert_eq!(result(&replies[2])["memory"]["basis"], "tested");
    assert!(result(&replies[2])["memory"].get("note").is_none());
    let belief = result(&replies[3]);
    assert_eq!(belief["memory"]["basis"], "unconfirmed");
    assert_eq!(belief["memory"]["note"], UNCONFIRMED_NOTE);

    assert_eq!(replies[4]["result"]["isError"], true, "{}", replies[4]);
    assert!(
        replies[4]["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("basis must be observed, tested or unconfirmed")),
        "{}",
        replies[4]
    );
    assert_eq!(replies[5]["result"]["isError"], false, "{}", replies[5]);
}

#[test]
fn an_unconfirmed_memory_is_marked_in_search_and_get_and_every_basis_is_shown() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let tested = propose_with_basis(&mut human, "Checked by a run.", MemoryBasis::Tested);
    let belief = propose_with_basis(&mut human, "A belief.", MemoryBasis::Unconfirmed);
    accept(&mut human, &tested, Vec::new());
    accept(&mut human, &belief, Vec::new());

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memories_search", &json!({})),
            call(3, "memory_get", &json!({ "id": belief })),
        ],
    );

    let listed = result(&replies[1])["memories"].as_array().unwrap().clone();
    let find = |id: &str| {
        listed
            .iter()
            .find(|memory| memory["id"] == id)
            .unwrap_or_else(|| panic!("{id} is listed: {listed:?}"))
    };
    assert_eq!(find(&tested)["basis"], "tested");
    assert!(find(&tested).get("note").is_none(), "{}", find(&tested));
    assert_eq!(find(&belief)["basis"], "unconfirmed");
    assert_eq!(find(&belief)["note"], UNCONFIRMED_NOTE);

    let read = &result(&replies[2])["memory"];
    assert_eq!(read["id"], belief);
    assert_eq!(read["basis"], "unconfirmed");
    assert_eq!(read["note"], UNCONFIRMED_NOTE);
    assert_eq!(read["untrusted"], true);
}

/// The retrieval count the app shows for memory `uid`, read as the person (D201 (4)).
fn retrieval_count(engine: &mut Engine, uid: &str) -> u32 {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ReadMemory {
            uid: uid.to_owned(),
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryRead { memory, .. } => Some(memory.retrieval_count),
            _ => None,
        })
        .expect("a memory read")
}

#[test]
fn a_search_counts_only_the_memories_it_returns() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let first = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);
    let second = propose_as_human(&mut human, "Prefer rebase over merge.", "f.txt", None);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memories_search", &json!({ "limit": 1 })),
        ],
    );
    let page = result(&replies[1]);
    let returned = page["memories"][0]["id"].as_str().unwrap().to_owned();
    let other = if returned == first { &second } else { &first };
    assert_eq!(page["memories"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(retrieval_count(&mut human, &returned), 1);
    assert_eq!(retrieval_count(&mut human, other), 0);
}

#[test]
fn a_get_counts_the_memory_it_reads_each_time() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let uid = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memory_get", &json!({ "id": uid })),
            call(3, "memory_get", &json!({ "id": uid })),
        ],
    );
    assert_eq!(result(&replies[1])["memory"]["id"], uid);
    assert_eq!(result(&replies[2])["memory"]["id"], uid);
    assert_eq!(retrieval_count(&mut human, &uid), 2);
}

#[test]
fn the_counts_are_in_no_mcp_result() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let uid = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);
    session(
        &repo,
        &db,
        &[initialize(), call(2, "memory_get", &json!({ "id": uid }))],
    );
    assert_eq!(retrieval_count(&mut human, &uid), 1);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memories_search", &json!({})),
            call(3, "memory_get", &json!({ "id": uid })),
            call(
                4,
                "memory_propose",
                &json!({
                    "kind": "rule",
                    "text": "Count nothing in results.",
                    "sources": [{ "kind": "file", "ref": "f.txt" }]
                }),
            ),
            call(5, "contexts_list", &json!({})),
        ],
    );
    for reply in &replies[1..] {
        let text = reply.to_string();
        assert!(!text.contains("retrieval_count"), "{text}");
        assert!(!text.contains("last_retrieved"), "{text}");
    }
}

#[test]
fn a_memory_of_a_context_is_counted_in_contexts_db_and_a_search_counts_it_too() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let context = create_context(&mut human, "Platform");
    join(&mut human, context, &repo);
    let uid = propose_as_human(
        &mut human,
        "The billing ledger is append-only.",
        "f.txt",
        Some("Platform"),
    );

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memory_get", &json!({ "id": uid })),
            call(3, "memories_search", &json!({ "scope": "Platform" })),
        ],
    );
    assert_eq!(result(&replies[1])["memory"]["scope"], "Platform");
    assert_eq!(result(&replies[2])["memories"][0]["id"], uid);
    assert_eq!(retrieval_count(&mut human, &uid), 2);
}

#[test]
fn a_moved_memory_keeps_its_count() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let uid = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);
    session(
        &repo,
        &db,
        &[initialize(), call(2, "memory_get", &json!({ "id": uid }))],
    );
    let context = create_context(&mut human, "Platform");
    join(&mut human, context, &repo);
    let events = send(
        &mut human,
        &Actor::Human,
        CommandKind::MoveMemoryToContext {
            uid: uid.clone(),
            context,
        },
    );
    assert_eq!(ended(&events), OperationState::Done, "move: {events:?}");

    assert_eq!(retrieval_count(&mut human, &uid), 1);
}

#[test]
fn a_failing_count_does_not_fail_the_read() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let uid = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);
    // Every write that counts a retrieval is refused; the reads must still answer.
    let refused = rusqlite::Connection::open(repo.join(".git/gitmaster/threads.db")).unwrap();
    refused
        .execute_batch(
            "CREATE TRIGGER refuse_count BEFORE UPDATE OF retrieval_count ON memory
             BEGIN SELECT RAISE(ABORT, 'the counter is refused in this test'); END;",
        )
        .unwrap();

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(2, "memory_get", &json!({ "id": uid })),
            call(3, "memories_search", &json!({})),
        ],
    );
    assert_eq!(result(&replies[1])["memory"]["id"], uid);
    assert_eq!(result(&replies[2])["memories"][0]["id"], uid);
    assert_eq!(retrieval_count(&mut human, &uid), 0);
}

/// The error text of a tool call that was refused, with the server still answering.
fn tool_error(reply: &Value) -> &str {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"][0]["text"]
        .as_str()
        .expect("the error text")
}

#[test]
fn an_agent_marks_a_memory_and_search_and_get_show_the_marks_and_the_wrong_note() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let belief = propose_with_basis(
        &mut human,
        "A belief about the parser.",
        MemoryBasis::Unconfirmed,
    );
    accept(&mut human, &belief, Vec::new());
    let plain = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_mark",
                &json!({ "memory_id": belief, "mark": "useful" }),
            ),
            call(
                3,
                "memory_mark",
                &json!({ "memory_id": belief, "mark": "wrong", "reason": "it sent the job to the wrong queue" }),
            ),
            call(
                4,
                "memory_mark",
                &json!({ "memory_id": plain, "mark": "useful" }),
            ),
            call(5, "memories_search", &json!({})),
            call(6, "memory_get", &json!({ "id": belief })),
        ],
    );

    let useful = &result(&replies[1])["memory"];
    assert_eq!(result(&replies[1])["mark"], "useful");
    assert_eq!(result(&replies[1])["recorded"], true);
    assert_eq!(useful["marks"][0]["actor"], "agent:test-client");
    assert_eq!(useful["marks"][0]["mark"], "useful");
    assert!(useful["marks"][0]["at"].is_number(), "{useful}");

    let changed = &result(&replies[2])["memory"];
    assert_eq!(
        result(&replies[2])["recorded"],
        true,
        "a change of mind is recorded"
    );
    assert_eq!(
        changed["marks"][0]["mark"], "wrong",
        "the latest mark counts"
    );
    assert_eq!(
        changed["note"],
        "Unconfirmed: a belief, not established fact. Marked wrong by agent:test-client; check before relying on it",
        "both notes, in that order"
    );

    let listed = result(&replies[4])["memories"].as_array().unwrap().clone();
    let find = |id: &str| {
        listed
            .iter()
            .find(|memory| memory["id"] == id)
            .unwrap_or_else(|| panic!("{id} is listed: {listed:?}"))
    };
    assert_eq!(find(&belief)["marks"][0]["mark"], "wrong");
    assert!(
        find(&belief)["marks"][0].get("reason").is_none(),
        "search has no reason"
    );
    assert!(
        find(&belief)["note"]
            .as_str()
            .unwrap()
            .contains("Marked wrong by agent:test-client")
    );
    assert_eq!(find(&plain)["marks"][0]["mark"], "useful");
    assert!(
        find(&plain).get("note").is_none(),
        "a memory marked useful has no note"
    );

    let read = &result(&replies[5])["memory"];
    assert_eq!(
        read["marks"][0]["reason"],
        "it sent the job to the wrong queue"
    );
    assert_eq!(
        read["marks"][0]["untrusted"], true,
        "a reason is untrusted text"
    );
}

#[test]
fn a_mark_on_a_memory_that_is_not_accepted_is_a_tool_error_and_the_server_answers() {
    let (_tmp, repo, db) = fixture();
    let proposed = {
        let replies = session(
            &repo,
            &db,
            &[
                initialize(),
                call(
                    2,
                    "memory_propose",
                    &json!({ "kind": "rule", "text": "Not reviewed yet.", "sources": [{ "kind": "file", "ref": "f.txt" }] }),
                ),
            ],
        );
        result(&replies[1])["memory"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_mark",
                &json!({ "memory_id": proposed, "mark": "useful" }),
            ),
            call(3, "memories_search", &json!({ "status": "proposed" })),
        ],
    );
    let refusal = tool_error(&replies[1]);
    assert!(
        refusal.contains("only an accepted memory can be marked"),
        "{refusal}"
    );
    assert_eq!(
        result(&replies[2])["memories"][0]["id"],
        proposed,
        "the server still answers"
    );
}

#[test]
fn an_invalid_mark_or_a_missing_one_is_refused_and_the_server_keeps_answering() {
    let (_tmp, repo, db) = fixture();
    let mut human = engine_on(&repo, &db);
    let uid = propose_as_human(&mut human, "Keep the ledger append-only.", "f.txt", None);

    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_mark",
                &json!({ "memory_id": uid, "mark": "helpful" }),
            ),
            call(3, "memory_mark", &json!({ "memory_id": uid })),
            call(
                4,
                "memory_mark",
                &json!({ "memory_id": uid, "mark": "useful", "reason": "x" }),
            ),
            call(5, "memories_search", &json!({})),
        ],
    );
    assert!(
        tool_error(&replies[1]).contains("mark must be useful or wrong"),
        "{replies:?}"
    );
    assert!(
        tool_error(&replies[2]).contains("mark is required"),
        "{replies:?}"
    );
    assert_eq!(result(&replies[3])["recorded"], true);
    assert_eq!(result(&replies[4])["memories"][0]["id"], uid);
    assert_eq!(
        result(&replies[4])["memories"][0]["marks"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "only the valid mark was written"
    );
}

#[test]
fn a_mark_on_a_memory_the_repository_cannot_see_is_not_found_and_says_how_to_find_one() {
    let (_tmp, repo, db) = fixture();
    let replies = session(
        &repo,
        &db,
        &[
            initialize(),
            call(
                2,
                "memory_mark",
                &json!({ "memory_id": "0000000000000000", "mark": "wrong" }),
            ),
        ],
    );
    assert!(
        tool_error(&replies[1]).contains("memories_search lists the memory ids"),
        "{replies:?}"
    );
}
