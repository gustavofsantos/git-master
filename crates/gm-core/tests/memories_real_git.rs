//! Memories (D190, D192) against real repositories in temporary directories: the proposal rules,
//! the review, redaction, the commit a source names, and files changed since acceptance, read from
//! `HEAD` with real `git`. The engine runs with the global and system Git configuration kept out.
//! Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::threads::{
    FileChange, FileChangeState, Memory, MemoryBasis, MemoryKind, MemorySourceInput,
    MemorySourceKind, MemoryStatus,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Repo {
    _base: tempfile::TempDir,
    root: PathBuf,
}

/// A repository on `main` with one commit, whose subject is `fix the parser`.
fn repo() -> Repo {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("parser.rs"), "fn parse() {}\n").unwrap();
    git(&root, &["add", "--", "parser.rs"]);
    git(&root, &["commit", "-q", "-m", "fix the parser"]);
    Repo { _base: base, root }
}

/// Commits every change of the working tree under `message`.
fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

fn engine_on(root: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = send(
        &mut engine,
        &Actor::Human,
        CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "open: {events:?}");
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

fn agent() -> Actor {
    Actor::Agent {
        name: "claude-code".into(),
    }
}

fn state_of(events: &[Event]) -> OperationState {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { state, .. } => Some(*state),
            _ => None,
        })
        .expect("an operation state")
}

fn failure(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        Event::OperationChanged { detail, .. } => detail.clone(),
        _ => None,
    })
}

/// The memory a write answered, and whether it created a row.
fn changed(events: &[Event]) -> (Option<Memory>, bool) {
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged {
                memory, created, ..
            } => Some((memory.clone(), *created)),
            _ => None,
        })
        .expect("a memory change")
}

/// The memory a write answered; the write must have kept one.
fn memory_of(events: &[Event]) -> Memory {
    changed(events).0.expect("a memory")
}

fn read(engine: &mut Engine, uid: &str) -> Memory {
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
            Event::MemoryRead { memory, .. } => Some(memory.clone()),
            _ => None,
        })
        .expect("a memory read")
}

fn list(engine: &mut Engine, actor: &Actor, status: Option<MemoryStatus>) -> Vec<Memory> {
    let events = send(
        engine,
        actor,
        CommandKind::ListMemories {
            query: String::new(),
            kind: None,
            status,
            path: None,
            author: None,
            scope: None,
            limit: 50,
            cursor: None,
        },
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed { memories, .. } => Some(memories.clone()),
            _ => None,
        })
        .expect("memories listed")
}

fn file(path: &str) -> MemorySourceInput {
    MemorySourceInput {
        kind: MemorySourceKind::File,
        reference: path.to_owned(),
        entry_from: None,
        entry_to: None,
    }
}

fn propose(
    engine: &mut Engine,
    actor: &Actor,
    kind: MemoryKind,
    text: &str,
    sources: Vec<MemorySourceInput>,
) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::ProposeMemory {
            kind,
            text: text.to_owned(),
            sources,
            context: None,
            basis: MemoryBasis::Observed,
        },
    )
}

fn accept(
    engine: &mut Engine,
    uid: &str,
    text: Option<&str>,
    supersedes: Vec<String>,
) -> Vec<Event> {
    send(
        engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: uid.to_owned(),
            text: text.map(str::to_owned),
            supersedes,
        },
    )
}

/// A memory an agent proposed about `parser.rs`, accepted by the human.
fn accepted_about_parser(engine: &mut Engine, text: &str) -> Memory {
    let proposed = memory_of(&propose(
        engine,
        &agent(),
        MemoryKind::Rule,
        text,
        vec![file("parser.rs")],
    ));
    memory_of(&accept(engine, &proposed.uid, None, Vec::new()))
}

#[test]
fn an_agent_proposal_needs_a_source_and_stores_nothing() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);

    let refused = propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Run the checks",
        Vec::new(),
    );
    assert_eq!(state_of(&refused), OperationState::Failed);
    let message = failure(&refused).expect("a reason");
    assert!(message.contains("source"), "{message}");
    assert!(list(&mut engine, &Actor::Human, None).is_empty());
}

#[test]
fn an_agent_proposal_is_proposed_and_a_repeat_is_the_same_memory() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);

    let events = propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Run the  release checks",
        vec![file("parser.rs")],
    );
    assert_eq!(state_of(&events), OperationState::Done);
    let (memory, created) = changed(&events);
    let memory = memory.expect("the memory");
    assert!(created);
    assert_eq!(memory.status, MemoryStatus::Proposed);
    assert_eq!(memory.origin, "agent:claude-code");
    assert_eq!(memory.proposed_by.as_deref(), Some("agent:claude-code"));
    assert!(!memory.reviewed);

    let again = propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "run the release checks",
        vec![file("parser.rs")],
    );
    let (repeat, created) = changed(&again);
    assert!(!created, "a repeat creates nothing");
    assert_eq!(repeat.expect("the same memory").id, memory.id);
}

#[test]
fn an_agent_cannot_accept_reject_or_delete() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Fact,
        "The parser is total",
        vec![file("parser.rs")],
    ));

    let requests = [
        (
            CommandKind::AcceptMemory {
                uid: proposed.uid.clone(),
                text: None,
                supersedes: Vec::new(),
            },
            "only a human can accept a memory",
        ),
        (
            CommandKind::RejectMemory {
                uid: proposed.uid.clone(),
            },
            "only a human can reject a memory",
        ),
        (
            CommandKind::DeleteMemory {
                uid: proposed.uid.clone(),
            },
            "only a human can delete a memory",
        ),
    ];
    for (request, refusal) in requests {
        let events = send(&mut engine, &agent(), request.clone());
        assert_eq!(state_of(&events), OperationState::Failed, "{request:?}");
        assert_eq!(failure(&events).as_deref(), Some(refusal), "{request:?}");
    }
    let kept = read(&mut engine, &proposed.uid);
    assert_eq!(kept.status, MemoryStatus::Proposed);
    assert_eq!(kept.text, "The parser is total");
}

#[test]
fn a_human_proposal_is_accepted_and_records_the_blob() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);

    let events = propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Rule,
        "Parse without panics",
        vec![file("parser.rs")],
    );
    let (memory, created) = changed(&events);
    let memory = memory.expect("the memory");
    assert!(created);
    assert_eq!(memory.status, MemoryStatus::Accepted);
    assert_eq!(memory.origin, "human");
    assert!(memory.reviewed);
    let blob = git(&repo.root, &["rev-parse", "HEAD:parser.rs"]);
    assert_eq!(memory.sources[0].blob_oid.as_deref(), Some(blob.as_str()));
    assert!(memory.sources[0].blob_read, "read at acceptance");
}

#[test]
fn a_changed_file_shows_after_a_commit_and_not_an_uncommitted_edit() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let memory = accepted_about_parser(&mut engine, "Keep the parser total");
    assert!(
        read(&mut engine, &memory.uid)
            .files_changed_since
            .is_empty()
    );

    std::fs::write(repo.root.join("parser.rs"), "fn parse() { 1 }\n").unwrap();
    assert!(
        read(&mut engine, &memory.uid)
            .files_changed_since
            .is_empty(),
        "the working tree is not HEAD"
    );

    commit_all(&repo.root, "change the parser");
    assert_eq!(
        read(&mut engine, &memory.uid).files_changed_since,
        vec![FileChange {
            path: "parser.rs".into(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn a_renamed_file_is_missing_at_its_old_path() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let memory = accepted_about_parser(&mut engine, "Keep the parser total");

    git(&repo.root, &["mv", "parser.rs", "lexer.rs"]);
    commit_all(&repo.root, "rename the parser");
    assert_eq!(
        read(&mut engine, &memory.uid).files_changed_since,
        vec![FileChange {
            path: "parser.rs".into(),
            state: FileChangeState::Missing,
        }],
        "no guess at the new name"
    );
}

#[test]
fn a_deleted_file_is_missing_and_a_file_added_later_is_changed() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let gone = accepted_about_parser(&mut engine, "Keep the parser total");
    let note = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Fact,
        "Notes live in notes.md",
        vec![file("notes.md")],
    ));
    assert_eq!(note.sources[0].blob_oid, None, "absent when accepted");
    assert!(note.sources[0].blob_read, "read, and absent");

    git(&repo.root, &["rm", "-q", "--", "parser.rs"]);
    std::fs::write(repo.root.join("notes.md"), "notes\n").unwrap();
    commit_all(&repo.root, "remove the parser and add notes");

    assert_eq!(
        read(&mut engine, &gone.uid).files_changed_since,
        vec![FileChange {
            path: "parser.rs".into(),
            state: FileChangeState::Missing,
        }]
    );
    assert_eq!(
        read(&mut engine, &note.uid).files_changed_since,
        vec![FileChange {
            path: "notes.md".into(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn accepting_again_is_idempotent_and_keeps_the_change() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let memory = accepted_about_parser(&mut engine, "Keep the parser total");

    std::fs::write(repo.root.join("parser.rs"), "fn parse() { 1 }\n").unwrap();
    commit_all(&repo.root, "change the parser");

    let reaccepted = memory_of(&accept(&mut engine, &memory.uid, None, Vec::new()));
    assert_eq!(reaccepted.status, MemoryStatus::Accepted);
    assert_eq!(reaccepted.decided_at, memory.decided_at);
    assert_eq!(reaccepted.decided_by, memory.decided_by);
    let read_back = read(&mut engine, &memory.uid);
    assert_eq!(read_back.decided_at, memory.decided_at);
    assert_eq!(
        read_back.files_changed_since,
        vec![FileChange {
            path: "parser.rs".into(),
            state: FileChangeState::Changed,
        }]
    );
}

#[test]
fn the_stored_text_is_redacted() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let key = ["AKIA", "ABCDEFGHIJKLMNOP"].concat();

    let events = propose(
        &mut engine,
        &agent(),
        MemoryKind::Lesson,
        &format!("Set {key} in the CI config"),
        vec![file("parser.rs")],
    );
    let memory = memory_of(&events);
    assert!(!memory.text.contains(&key), "{}", memory.text);
    assert_eq!(
        memory.text,
        "Set [REDACTED:aws-access-key-id] in the CI config"
    );
    assert!(!read(&mut engine, &memory.uid).text.contains(&key));
}

#[test]
fn a_commit_source_is_stored_as_its_full_id_and_an_unknown_one_is_refused() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let full = git(&repo.root, &["rev-parse", "HEAD"]);

    let events = propose(
        &mut engine,
        &agent(),
        MemoryKind::Decision,
        "The parser was fixed first",
        vec![MemorySourceInput {
            kind: MemorySourceKind::Commit,
            reference: full[..7].to_owned(),
            entry_from: None,
            entry_to: None,
        }],
    );
    let memory = memory_of(&events);
    assert_eq!(memory.sources.len(), 1);
    assert_eq!(memory.sources[0].kind, MemorySourceKind::Commit);
    assert_eq!(memory.sources[0].reference, full);

    for unknown in ["deadbeef", "0123456789abcdef0123456789abcdef01234567"] {
        let refused = propose(
            &mut engine,
            &agent(),
            MemoryKind::Decision,
            &format!("A claim about {unknown}"),
            vec![MemorySourceInput {
                kind: MemorySourceKind::Commit,
                reference: unknown.to_owned(),
                entry_from: None,
                entry_to: None,
            }],
        );
        assert_eq!(state_of(&refused), OperationState::Failed, "{unknown}");
        assert!(
            failure(&refused).is_some_and(|message| message.contains("not in this repository")),
            "{unknown}"
        );
    }
}

#[test]
fn a_human_edit_on_accept_keeps_who_proposed_it() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "old text",
        vec![file("parser.rs")],
    ));

    let accepted = memory_of(&accept(
        &mut engine,
        &proposed.uid,
        Some("  new text  "),
        Vec::new(),
    ));
    assert_eq!(accepted.status, MemoryStatus::Accepted);
    assert_eq!(accepted.text, "new text");
    assert_eq!(accepted.origin, "human");
    assert_eq!(accepted.proposed_by.as_deref(), Some("agent:claude-code"));
}

#[test]
fn a_supersede_marks_the_old_memory_and_links_both_ways() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let old = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Decision,
        "Use the old scheme",
        vec![file("parser.rs")],
    ));
    let new = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Decision,
        "Use the new scheme",
        vec![file("parser.rs")],
    ));

    let accepted = memory_of(&accept(&mut engine, &new.uid, None, vec![old.uid.clone()]));
    assert_eq!(accepted.supersedes, vec![old.uid.clone()]);
    let old_uid = old.uid.clone();
    let old = read(&mut engine, &old_uid);
    assert_eq!(old.status, MemoryStatus::Superseded);
    assert_eq!(old.superseded_by, vec![new.uid.clone()]);
    assert_eq!(
        read(&mut engine, &new.uid).supersedes,
        vec![old.uid.clone()]
    );
}

#[test]
fn an_agent_lists_accepted_memories_and_asks_for_proposals_by_status() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let accepted = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Decision,
        "We keep the store in SQLite",
        Vec::new(),
    ));
    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Run the release checks",
        vec![file("parser.rs")],
    ));

    let by_default = list(&mut engine, &agent(), None);
    assert_eq!(
        by_default.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![accepted.id]
    );
    assert!(by_default[0].reviewed);

    let asked = list(&mut engine, &agent(), Some(MemoryStatus::Proposed));
    assert_eq!(
        asked.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![proposed.id]
    );
    assert!(!asked[0].reviewed);

    let everything = list(&mut engine, &Actor::Human, None);
    assert_eq!(everything.len(), 2);
}

#[test]
fn memory_reads_are_not_logged_and_writes_are_with_their_actor() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Run the release checks",
        vec![file("parser.rs")],
    ));
    list(&mut engine, &agent(), None);
    read(&mut engine, &proposed.uid);
    send(
        &mut engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: None,
            supersedes: Vec::new(),
        },
    );

    let log = engine.log();
    assert!(log.iter().any(|entry| {
        entry.actor == agent() && matches!(entry.kind, CommandKind::ProposeMemory { .. })
    }));
    assert!(log.iter().any(|entry| {
        entry.actor == Actor::Human && matches!(entry.kind, CommandKind::AcceptMemory { .. })
    }));
    assert!(
        log.iter().all(|entry| !matches!(
            entry.kind,
            CommandKind::ListMemories { .. } | CommandKind::ReadMemory { .. }
        )),
        "reads are not in the command log"
    );
}

/// Writes `text` to `path` under the repository root, making its folder.
fn write_file_in(root: &Path, path: &str, text: &str) {
    if let Some(parent) = root.join(path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(root.join(path), text).unwrap();
}

/// A `ListMemories` with the given filters, unpaged unless a cursor is given.
fn listing_request(
    query: &str,
    kind: Option<MemoryKind>,
    status: Option<MemoryStatus>,
    path: Option<&str>,
    author: Option<&str>,
    limit: u32,
    cursor: Option<&str>,
) -> CommandKind {
    CommandKind::ListMemories {
        query: query.to_owned(),
        kind,
        status,
        path: path.map(str::to_owned),
        author: author.map(str::to_owned),
        scope: None,
        limit,
        cursor: cursor.map(str::to_owned),
    }
}

/// The memories a list answered, its next cursor and the exact count of proposed memories.
fn listing(
    engine: &mut Engine,
    actor: &Actor,
    request: CommandKind,
) -> (Vec<Memory>, Option<String>, u32) {
    let events = send(engine, actor, request);
    assert_eq!(state_of(&events), OperationState::Done, "list: {events:?}");
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed {
                memories,
                next_cursor,
                proposed_total,
                ..
            } => Some((memories.clone(), next_cursor.clone(), *proposed_total)),
            _ => None,
        })
        .expect("memories listed")
}

/// The number of proposed memories among `memories`.
fn proposed_in(memories: &[Memory]) -> u32 {
    u32::try_from(
        memories
            .iter()
            .filter(|memory| memory.status == MemoryStatus::Proposed)
            .count(),
    )
    .unwrap()
}

#[test]
fn the_proposed_total_is_zero_when_nothing_is_proposed() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Decision,
        "We keep the store in SQLite",
        Vec::new(),
    );
    let (memories, _, total) = listing(
        &mut engine,
        &Actor::Human,
        listing_request("", None, None, None, None, 50, None),
    );
    assert_eq!(memories.len(), 1);
    assert_eq!(total, 0);
}

#[test]
fn the_proposed_total_follows_propose_accept_and_reject() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let first = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Run the release checks",
        vec![file("parser.rs")],
    ));
    let second = memory_of(&propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Keep parse() pure",
        vec![file("parser.rs")],
    ));
    let total = |engine: &mut Engine| {
        listing(
            engine,
            &Actor::Human,
            listing_request("", None, None, None, None, 50, None),
        )
        .2
    };
    assert_eq!(total(&mut engine), 2);

    accept(&mut engine, &first.uid, None, Vec::new());
    assert_eq!(total(&mut engine), 1);

    send(
        &mut engine,
        &Actor::Human,
        CommandKind::RejectMemory {
            uid: second.uid.clone(),
        },
    );
    assert_eq!(total(&mut engine), 0);

    propose(
        &mut engine,
        &agent(),
        MemoryKind::Lesson,
        "A lost parse is a bug",
        vec![file("parser.rs")],
    );
    assert_eq!(total(&mut engine), 1);
}

#[test]
fn the_proposed_total_ignores_status_query_and_paging() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    for text in ["alpha widget", "beta gadget", "gamma widget"] {
        propose(
            &mut engine,
            &agent(),
            MemoryKind::Rule,
            text,
            vec![file("parser.rs")],
        );
    }
    propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Decision,
        "An accepted note",
        Vec::new(),
    );

    // Accepted only: the list holds one row, the total still counts the three proposals.
    let (accepted, _, total) = listing(
        &mut engine,
        &Actor::Human,
        listing_request("", None, Some(MemoryStatus::Accepted), None, None, 50, None),
    );
    assert_eq!(accepted.len(), 1);
    assert_eq!(total, 3);

    // A query that matches nothing: no rows, the same total.
    let (none, _, total) = listing(
        &mut engine,
        &Actor::Human,
        listing_request("zebra", None, None, None, None, 50, None),
    );
    assert!(none.is_empty());
    assert_eq!(total, 3);

    // A query that matches two proposals: the total is not narrowed to the query.
    let (matched, _, total) = listing(
        &mut engine,
        &Actor::Human,
        listing_request(
            "widget",
            None,
            Some(MemoryStatus::Proposed),
            None,
            None,
            50,
            None,
        ),
    );
    assert_eq!(matched.len(), 2);
    assert_eq!(total, 3);

    // A page of one: the first page and the last both report the full number.
    let (first, next, total) = listing(
        &mut engine,
        &agent(),
        listing_request("", None, Some(MemoryStatus::Proposed), None, None, 1, None),
    );
    assert_eq!(first.len(), 1);
    assert_eq!(total, 3);
    let mut cursor = next;
    let mut pages = 1;
    while let Some(next) = cursor {
        let (page, after, total) = listing(
            &mut engine,
            &agent(),
            listing_request(
                "",
                None,
                Some(MemoryStatus::Proposed),
                None,
                None,
                1,
                Some(&next),
            ),
        );
        assert_eq!(page.len(), 1);
        assert_eq!(total, 3);
        cursor = after;
        pages += 1;
    }
    assert_eq!(pages, 3);
}

#[test]
fn the_proposed_total_ignores_the_kind_author_and_path_of_the_request() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    write_file_in(&repo.root, "src/lexer.rs", "fn lex() {}\n");
    commit_all(&repo.root, "add the lexer");
    propose(
        &mut engine,
        &agent(),
        MemoryKind::Rule,
        "Claude rule about the parser",
        vec![file("parser.rs")],
    );
    propose(
        &mut engine,
        &agent(),
        MemoryKind::Decision,
        "A decision about the lexer",
        vec![file("src/lexer.rs")],
    );
    let codex = Actor::Agent {
        name: "codex".into(),
    };
    propose(
        &mut engine,
        &codex,
        MemoryKind::Rule,
        "Codex rule about the parser",
        vec![file("parser.rs")],
    );
    let listing_with = |engine: &mut Engine, kind, path: Option<&str>, author: Option<&str>| {
        listing(
            engine,
            &Actor::Human,
            listing_request("", kind, None, path, author, 50, None),
        )
    };

    // Every request sees the same total: three proposals in the repository.
    let (all, _, total) = listing_with(&mut engine, None, None, None);
    assert_eq!(total, 3);
    assert_eq!(proposed_in(&all), 3);
    let (rules, _, total) = listing_with(&mut engine, Some(MemoryKind::Rule), None, None);
    assert_eq!(proposed_in(&rules), 2, "the list is filtered");
    assert_eq!(total, 3, "the total is not");
    let (_, _, total) = listing_with(&mut engine, None, None, Some("agent:codex"));
    assert_eq!(total, 3);
    let (in_src, _, total) = listing_with(&mut engine, None, Some("src"), None);
    assert_eq!(proposed_in(&in_src), 1);
    assert_eq!(total, 3);
    let (_, _, total) = listing_with(
        &mut engine,
        Some(MemoryKind::Rule),
        Some("parser.rs"),
        Some("agent:claude-code"),
    );
    assert_eq!(total, 3);
}

/// A proposal as a client sends it: the JSON of `propose_memory`, with `basis` only when given.
fn propose_json(engine: &mut Engine, actor: &Actor, text: &str, basis: Option<&str>) -> Vec<Event> {
    let mut command = serde_json::json!({
        "type": "propose_memory",
        "kind": "rule",
        "text": text,
        "sources": [{ "kind": "file", "ref": "parser.rs" }],
    });
    if let Some(basis) = basis {
        command["basis"] = serde_json::json!(basis);
    }
    let kind: CommandKind = serde_json::from_value(command).expect("a proposal parses");
    send(engine, actor, kind)
}

#[test]
fn a_memory_records_its_basis_and_a_proposal_without_one_is_observed() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);

    let observed = memory_of(&propose_json(
        &mut engine,
        &agent(),
        "Keep the lock order.",
        None,
    ));
    let tested = memory_of(&propose_json(
        &mut engine,
        &agent(),
        "The parser fuzzer finds nothing new.",
        Some("tested"),
    ));
    let unconfirmed = memory_of(&propose_json(
        &mut engine,
        &agent(),
        "The cache is warm at start.",
        Some("unconfirmed"),
    ));
    assert_eq!(observed.basis, MemoryBasis::Observed);
    assert_eq!(tested.basis, MemoryBasis::Tested);
    assert_eq!(unconfirmed.basis, MemoryBasis::Unconfirmed);

    for (uid, basis) in [
        (&observed.uid, MemoryBasis::Observed),
        (&tested.uid, MemoryBasis::Tested),
        (&unconfirmed.uid, MemoryBasis::Unconfirmed),
    ] {
        assert_eq!(read(&mut engine, uid).basis, basis);
    }

    // Accepting keeps the basis; a person's proposal with no basis is observed too.
    let events = accept(&mut engine, &unconfirmed.uid, None, Vec::new());
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "accept: {events:?}"
    );
    assert_eq!(memory_of(&events).basis, MemoryBasis::Unconfirmed);
    let person = memory_of(&propose_json(
        &mut engine,
        &Actor::Human,
        "Ship on Fridays never.",
        None,
    ));
    assert_eq!(person.basis, MemoryBasis::Observed);
    assert_eq!(person.status, MemoryStatus::Accepted);

    let accepted = list(&mut engine, &Actor::Human, Some(MemoryStatus::Accepted));
    let basis_of = |uid: &str| {
        accepted
            .iter()
            .find(|memory| memory.uid == uid)
            .map(|memory| memory.basis)
    };
    assert_eq!(basis_of(&unconfirmed.uid), Some(MemoryBasis::Unconfirmed));
    assert_eq!(basis_of(&person.uid), Some(MemoryBasis::Observed));
}

#[test]
fn a_basis_outside_the_three_words_does_not_parse_as_a_proposal() {
    let parsed = serde_json::from_value::<CommandKind>(serde_json::json!({
        "type": "propose_memory",
        "kind": "rule",
        "text": "Guessed.",
        "sources": [{ "kind": "file", "ref": "parser.rs" }],
        "basis": "guessed",
    }));
    assert!(
        parsed.is_err(),
        "a basis that is not a known word is refused"
    );
}

/// Counts one retrieval now of each of `uids`, as the MCP reads do after they answer (D201 (4)).
fn retrieved(engine: &mut Engine, actor: &Actor, uids: &[&str]) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::RecordMemoryRetrieval {
            uids: uids.iter().map(|uid| (*uid).to_owned()).collect(),
        },
    )
}

/// The size an accepted-memories listing reported: its `accepted_total` and `accepted_characters`.
fn size_of(events: &[Event]) -> (u32, u64) {
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoriesListed {
                accepted_total,
                accepted_characters,
                ..
            } => Some((*accepted_total, *accepted_characters)),
            _ => None,
        })
        .expect("memories listed")
}

/// A listing with the query and status given, the rest as an app's tab asks for it.
fn listing_with(
    engine: &mut Engine,
    actor: &Actor,
    query: &str,
    status: Option<MemoryStatus>,
) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::ListMemories {
            query: query.to_owned(),
            kind: None,
            status,
            path: None,
            author: None,
            scope: None,
            limit: 50,
            cursor: None,
        },
    )
}

#[test]
fn a_retrieval_counts_the_memories_named_and_only_those_the_repository_sees() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let first = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Rule,
        "Run the release checks",
        vec![file("parser.rs")],
    ));
    let second = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Fact,
        "The parser is small",
        vec![file("parser.rs")],
    ));
    // Listing and reading are not retrievals: the app reads the same way.
    list(&mut engine, &agent(), None);
    read(&mut engine, &first.uid);
    assert_eq!(read(&mut engine, &first.uid).retrieval_count, 0);

    let events = retrieved(&mut engine, &agent(), &[&first.uid, "0000000000000000"]);
    assert_eq!(state_of(&events), OperationState::Done, "{events:?}");
    let counted = read(&mut engine, &first.uid);
    assert_eq!(counted.retrieval_count, 1);
    assert!(counted.last_retrieved_at.is_some());
    assert_eq!(read(&mut engine, &second.uid).retrieval_count, 0);
    let listed = list(&mut engine, &Actor::Human, None);
    assert_eq!(
        listed
            .iter()
            .find(|memory| memory.uid == first.uid)
            .expect("listed")
            .retrieval_count,
        1,
        "the list the app shows carries the count"
    );

    retrieved(&mut engine, &agent(), &[&first.uid]);
    assert_eq!(read(&mut engine, &first.uid).retrieval_count, 2);
}

#[test]
fn a_retrieval_is_not_in_the_command_log() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let memory = memory_of(&propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Rule,
        "Run the release checks",
        vec![file("parser.rs")],
    ));
    retrieved(&mut engine, &agent(), &[&memory.uid]);

    let log = engine.log();
    assert!(
        log.iter()
            .all(|entry| !matches!(entry.kind, CommandKind::RecordMemoryRetrieval { .. })),
        "a retrieval is a read, so it is not logged (D28)"
    );
    assert!(
        log.iter()
            .any(|entry| matches!(entry.kind, CommandKind::ProposeMemory { .. })),
        "the write that made the memory is logged"
    );
}

#[test]
fn the_size_line_counts_accepted_memories_and_their_characters_whatever_the_request() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);
    let characters = |text: &str| u64::try_from(text.chars().count()).expect("a count");
    let accented = "Cálculo é rápido";
    let plain = "Run the release checks";
    propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Fact,
        accented,
        vec![file("parser.rs")],
    );
    propose(
        &mut engine,
        &Actor::Human,
        MemoryKind::Rule,
        plain,
        vec![file("parser.rs")],
    );
    propose(
        &mut engine,
        &agent(),
        MemoryKind::Lesson,
        "A proposal is not accepted",
        vec![file("parser.rs")],
    );
    let expected = (2, characters(accented) + characters(plain));

    assert_eq!(
        size_of(&listing_with(&mut engine, &Actor::Human, "", None)),
        expected
    );
    // The query and the status of the request play no part.
    assert_eq!(
        size_of(&listing_with(
            &mut engine,
            &Actor::Human,
            "release",
            Some(MemoryStatus::Proposed)
        )),
        expected
    );
    assert_eq!(
        size_of(&listing_with(&mut engine, &agent(), "", None)),
        expected,
        "an agent sees the same accepted memories"
    );
}
