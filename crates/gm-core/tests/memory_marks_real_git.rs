//! Memory marks (D202) against real repositories in temporary directories: an agent and the person
//! mark a memory useful or wrong, a mark can change and keeps its history, a repeated mark writes
//! nothing, only an accepted memory takes a mark, marks move with a memory into a context, a mark on
//! a memory of a context is kept in `contexts.db`, marks are in the command log, they order and
//! filter nothing, and the reason is redacted. Each engine keeps the global and system Git
//! configuration out. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::threads::{
    Context, Memory, MemoryBasis, MemoryKind, MemoryMark, MemoryMarkSummary, MemorySourceInput,
    MemorySourceKind, MemoryStatus,
};
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A temporary directory with a repository `root` and a contexts file `db` that is not there yet.
struct World {
    _base: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

fn world() -> World {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("parser.rs"), "fn parse() {}\n").unwrap();
    git(&root, &["add", "--", "parser.rs"]);
    git(&root, &["commit", "-q", "-m", "fix the parser"]);
    let db = base.path().join("contexts.db");
    World {
        _base: base,
        root,
        db,
    }
}

fn engine_on(world: &World) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    engine.set_contexts_db(Some(world.db.clone()));
    let events = send(
        &mut engine,
        &Actor::Human,
        CommandKind::OpenRepo {
            path: world.root.to_string_lossy().into_owned(),
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

/// The detail a refused operation gives.
fn failure(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::OperationChanged { detail, .. } => detail.clone(),
            _ => None,
        })
        .expect("a refusal says why")
}

fn memory_of(events: &[Event]) -> Memory {
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged {
                memory: Some(memory),
                ..
            } => Some(memory.clone()),
            _ => None,
        })
        .expect("a memory")
}

/// Whether the write recorded a new row (a new memory, or a new mark).
fn created_of(events: &[Event]) -> bool {
    events
        .iter()
        .find_map(|event| match event {
            Event::MemoryChanged { created, .. } => Some(*created),
            _ => None,
        })
        .expect("a memory change")
}

fn file(path: &str) -> MemorySourceInput {
    MemorySourceInput {
        kind: MemorySourceKind::File,
        reference: path.to_owned(),
        entry_from: None,
        entry_to: None,
    }
}

fn propose(engine: &mut Engine, actor: &Actor, text: &str, context: Option<&str>) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::ProposeMemory {
            kind: MemoryKind::Rule,
            text: text.to_owned(),
            sources: vec![file("parser.rs")],
            context: context.map(str::to_owned),
            basis: MemoryBasis::Observed,
        },
    )
}

/// A memory the person wrote, which is accepted at once.
fn accepted(engine: &mut Engine, text: &str) -> Memory {
    let events = propose(engine, &Actor::Human, text, None);
    assert_eq!(state_of(&events), OperationState::Done, "{events:?}");
    memory_of(&events)
}

fn mark(
    engine: &mut Engine,
    actor: &Actor,
    uid: &str,
    mark: MemoryMark,
    reason: Option<&str>,
) -> Vec<Event> {
    send(
        engine,
        actor,
        CommandKind::MarkMemory {
            uid: uid.to_owned(),
            mark,
            reason: reason.map(str::to_owned),
        },
    )
}

/// The memory as the read answers it, with its mark history, oldest first.
fn read(engine: &mut Engine, uid: &str) -> (Memory, Vec<MemoryMarkSummary>) {
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
            Event::MemoryRead {
                memory,
                mark_history,
                ..
            } => Some((memory.clone(), mark_history.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the memory is read: {events:?}"))
}

/// The uids of the memories a listing returns, in its order.
fn listed_uids(
    engine: &mut Engine,
    query: &str,
    kind: Option<MemoryKind>,
    status: Option<MemoryStatus>,
) -> Vec<String> {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::ListMemories {
            query: query.to_owned(),
            kind,
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
            Event::MemoriesListed { memories, .. } => {
                Some(memories.iter().map(|memory| memory.uid.clone()).collect())
            }
            _ => None,
        })
        .expect("memories listed")
}

fn mark_of<'a>(marks: &'a [MemoryMarkSummary], actor: &str) -> Option<&'a MemoryMarkSummary> {
    marks.iter().find(|summary| summary.actor == actor)
}

/// Creates a context as the person and returns it.
fn create_context(engine: &mut Engine, name: &str) -> Context {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::CreateContext {
            name: name.to_owned(),
            parent: None,
        },
    );
    assert_eq!(
        state_of(&events),
        OperationState::Done,
        "create: {events:?}"
    );
    events
        .iter()
        .find_map(|event| match event {
            Event::ContextsChanged {
                context: Some(context),
                ..
            } => Some(context.clone()),
            _ => None,
        })
        .expect("the context")
}

/// Adds the repository to `context` as the person.
fn join(engine: &mut Engine, context: i64, root: &Path) {
    let events = send(
        engine,
        &Actor::Human,
        CommandKind::AddRepoToContext {
            context,
            path: root.to_string_lossy().into_owned(),
        },
    );
    assert_eq!(state_of(&events), OperationState::Done, "join: {events:?}");
}

#[test]
fn an_agent_and_the_person_each_mark_a_memory_and_the_latest_mark_of_each_counts() {
    let world = world();
    let mut engine = engine_on(&world);
    let memory = accepted(&mut engine, "Run the release checks");

    let by_agent = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Useful,
        Some("it saved a rerun"),
    );
    assert_eq!(state_of(&by_agent), OperationState::Done, "{by_agent:?}");
    assert!(created_of(&by_agent), "the first mark writes a row");
    let by_person = mark(
        &mut engine,
        &Actor::Human,
        &memory.uid,
        MemoryMark::Wrong,
        None,
    );
    assert_eq!(state_of(&by_person), OperationState::Done, "{by_person:?}");

    let (read_memory, history) = read(&mut engine, &memory.uid);
    assert_eq!(read_memory.marks.len(), 2, "one summary per actor");
    let agent_mark = mark_of(&read_memory.marks, "agent:claude-code").expect("the agent's mark");
    assert_eq!(agent_mark.mark, MemoryMark::Useful);
    assert_eq!(agent_mark.reason.as_deref(), Some("it saved a rerun"));
    let person_mark = mark_of(&read_memory.marks, "human").expect("the person's mark");
    assert_eq!(person_mark.mark, MemoryMark::Wrong);
    assert_eq!(person_mark.reason, None);
    assert_eq!(history.len(), 2);
}

#[test]
fn a_change_of_mind_keeps_the_history_and_the_latest_mark_of_each_actor_counts() {
    let world = world();
    let mut engine = engine_on(&world);
    let memory = accepted(&mut engine, "Run the release checks");
    mark(
        &mut engine,
        &Actor::Human,
        &memory.uid,
        MemoryMark::Useful,
        None,
    );
    mark(&mut engine, &agent(), &memory.uid, MemoryMark::Useful, None);

    // The agent called it useful, then found that it led to a wrong action.
    let changed = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Wrong,
        Some("it sent the release to staging"),
    );
    assert_eq!(state_of(&changed), OperationState::Done, "{changed:?}");
    assert!(created_of(&changed), "a changed mark writes a row");

    let (read_memory, history) = read(&mut engine, &memory.uid);
    let latest = mark_of(&read_memory.marks, "agent:claude-code").expect("the agent's latest");
    assert_eq!(latest.mark, MemoryMark::Wrong, "the latest mark counts");
    assert_eq!(
        mark_of(&read_memory.marks, "human")
            .expect("the person's")
            .mark,
        MemoryMark::Useful,
        "the person's mark is not changed by the agent's"
    );
    let order: Vec<(&str, MemoryMark)> = history
        .iter()
        .map(|summary| (summary.actor.as_str(), summary.mark))
        .collect();
    assert_eq!(
        order,
        [
            ("human", MemoryMark::Useful),
            ("agent:claude-code", MemoryMark::Useful),
            ("agent:claude-code", MemoryMark::Wrong),
        ],
        "the earlier mark stays, oldest first"
    );
    assert_eq!(
        history[2].reason.as_deref(),
        Some("it sent the release to staging")
    );
}

#[test]
fn a_repeated_mark_writes_no_row_and_the_memory_answers_as_it_is() {
    let world = world();
    let mut engine = engine_on(&world);
    let memory = accepted(&mut engine, "Run the release checks");
    let first = mark(&mut engine, &agent(), &memory.uid, MemoryMark::Useful, None);
    assert!(created_of(&first));

    let again = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Useful,
        Some("same again"),
    );
    assert_eq!(state_of(&again), OperationState::Done, "{again:?}");
    assert!(
        !created_of(&again),
        "a repeat of the latest mark is not recorded"
    );
    let (_, history) = read(&mut engine, &memory.uid);
    assert_eq!(history.len(), 1, "no duplicate row: {history:?}");
    assert_eq!(
        history[0].reason, None,
        "the repeat does not change the reason"
    );
}

#[test]
fn only_an_accepted_memory_takes_a_mark_and_the_refusal_says_why() {
    let world = world();
    let mut engine = engine_on(&world);

    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        "A proposal nobody reviewed",
        None,
    ));
    let refused = mark(
        &mut engine,
        &agent(),
        &proposed.uid,
        MemoryMark::Useful,
        None,
    );
    assert_eq!(state_of(&refused), OperationState::Failed);
    assert!(
        failure(&refused)
            .contains("only an accepted memory can be marked, and this one is proposed"),
        "{}",
        failure(&refused)
    );
    let refused_by_person = mark(
        &mut engine,
        &Actor::Human,
        &proposed.uid,
        MemoryMark::Wrong,
        None,
    );
    assert_eq!(state_of(&refused_by_person), OperationState::Failed);

    let rejected = memory_of(&propose(&mut engine, &agent(), "A rejected idea", None));
    send(
        &mut engine,
        &Actor::Human,
        CommandKind::RejectMemory {
            uid: rejected.uid.clone(),
        },
    );
    let refused = mark(
        &mut engine,
        &agent(),
        &rejected.uid,
        MemoryMark::Wrong,
        None,
    );
    assert!(
        failure(&refused).contains("this one is rejected"),
        "{}",
        failure(&refused)
    );

    let old = accepted(&mut engine, "The old retry rule");
    let replacement = memory_of(&propose(&mut engine, &agent(), "The new retry rule", None));
    let accepted_new = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: replacement.uid.clone(),
            text: None,
            supersedes: vec![old.uid.clone()],
        },
    );
    assert_eq!(
        state_of(&accepted_new),
        OperationState::Done,
        "{accepted_new:?}"
    );
    let superseded = mark(&mut engine, &agent(), &old.uid, MemoryMark::Useful, None);
    assert!(
        failure(&superseded).contains("this one is superseded"),
        "{}",
        failure(&superseded)
    );

    for uid in [&proposed.uid, &rejected.uid, &old.uid] {
        let (memory, history) = read(&mut engine, uid);
        assert!(
            memory.marks.is_empty() && history.is_empty(),
            "a refused mark writes nothing"
        );
    }
}

#[test]
fn a_mark_on_a_memory_that_is_not_there_is_not_found() {
    let world = world();
    let mut engine = engine_on(&world);
    let events = mark(
        &mut engine,
        &agent(),
        "0000000000000000",
        MemoryMark::Useful,
        None,
    );
    assert_eq!(state_of(&events), OperationState::Failed);
    assert_eq!(failure(&events), "Memory not found.");
}

#[test]
fn marks_move_with_a_memory_into_a_context_and_the_repository_copy_is_gone() {
    let world = world();
    let mut engine = engine_on(&world);
    let platform = create_context(&mut engine, "Platform");
    join(&mut engine, platform.id, &world.root);
    let memory = accepted(&mut engine, "Retry the upload twice");
    mark(&mut engine, &agent(), &memory.uid, MemoryMark::Useful, None);
    mark(
        &mut engine,
        &Actor::Human,
        &memory.uid,
        MemoryMark::Wrong,
        Some("the second retry is not needed"),
    );

    let moved = send(
        &mut engine,
        &Actor::Human,
        CommandKind::MoveMemoryToContext {
            uid: memory.uid.clone(),
            context: platform.id,
        },
    );
    assert_eq!(state_of(&moved), OperationState::Done, "{moved:?}");
    let moved = memory_of(&moved);
    assert_eq!(moved.uid, memory.uid);
    assert_eq!(moved.marks.len(), 2, "the marks came with the memory");
    assert_eq!(moved.scope, "Platform");

    let (read_memory, history) = read(&mut engine, &memory.uid);
    assert_eq!(read_memory.marks.len(), 2);
    assert_eq!(history.len(), 2, "the whole history moved: {history:?}");
    assert_eq!(
        history[1].reason.as_deref(),
        Some("the second retry is not needed")
    );
    assert!(
        listed_uids(&mut engine, "", None, None)
            .iter()
            .filter(|uid| **uid == memory.uid)
            .count()
            == 1,
        "the memory is listed once, from the context"
    );
}

#[test]
fn a_memory_of_a_context_takes_marks_in_the_contexts_file_and_reads_its_history_there() {
    let world = world();
    let mut engine = engine_on(&world);
    let platform = create_context(&mut engine, "Platform");
    join(&mut engine, platform.id, &world.root);
    let proposed = memory_of(&propose(
        &mut engine,
        &agent(),
        "Keep the parser small",
        Some("Platform"),
    ));
    let accepted_in_context = send(
        &mut engine,
        &Actor::Human,
        CommandKind::AcceptMemory {
            uid: proposed.uid.clone(),
            text: None,
            supersedes: Vec::new(),
        },
    );
    assert_eq!(state_of(&accepted_in_context), OperationState::Done);

    let marked = mark(
        &mut engine,
        &agent(),
        &proposed.uid,
        MemoryMark::Useful,
        Some("the parser stayed small"),
    );
    assert_eq!(state_of(&marked), OperationState::Done, "{marked:?}");
    let marked = memory_of(&marked);
    assert_eq!(
        marked.scope, "Platform",
        "the answer keeps the context scope"
    );
    assert_eq!(marked.marks.len(), 1);

    let (_, history) = read(&mut engine, &proposed.uid);
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].reason.as_deref(),
        Some("the parser stayed small")
    );
}

#[test]
fn a_mark_is_in_the_command_log_with_its_actor_and_a_read_is_not() {
    let world = world();
    let mut engine = engine_on(&world);
    let memory = accepted(&mut engine, "Run the release checks");
    mark(&mut engine, &agent(), &memory.uid, MemoryMark::Useful, None);
    mark(
        &mut engine,
        &Actor::Human,
        &memory.uid,
        MemoryMark::Wrong,
        None,
    );
    read(&mut engine, &memory.uid);

    let log = engine.log();
    // The log has one entry per state an operation passes through; the finished ones are the writes.
    let marks: Vec<&Actor> = log
        .iter()
        .filter(|entry| {
            matches!(entry.kind, CommandKind::MarkMemory { .. })
                && entry.state == OperationState::Done
        })
        .map(|entry| &entry.actor)
        .collect();
    assert_eq!(marks.len(), 2, "both marks are logged");
    assert_eq!(*marks[0], agent());
    assert_eq!(*marks[1], Actor::Human);
    assert!(
        log.iter()
            .all(|entry| !matches!(entry.kind, CommandKind::ReadMemory { .. })),
        "a read is not logged"
    );
}

#[test]
fn marks_order_and_filter_nothing_so_a_search_reads_the_same_before_and_after() {
    let world = world();
    let mut engine = engine_on(&world);
    let first = accepted(&mut engine, "Run the release checks");
    let second = accepted(&mut engine, "Keep the parser small");
    let third = accepted(&mut engine, "Retry the upload twice");
    let before_all = listed_uids(&mut engine, "", None, None);
    let before_query = listed_uids(&mut engine, "parser", None, None);
    let before_kind = listed_uids(
        &mut engine,
        "",
        Some(MemoryKind::Rule),
        Some(MemoryStatus::Accepted),
    );
    let before_fields = [&first, &second, &third].map(|memory| {
        let (read_memory, _) = read(&mut engine, &memory.uid);
        (
            read_memory.text,
            read_memory.status,
            read_memory.origin,
            read_memory.created_at,
            read_memory.decided_at,
        )
    });

    // The most used memory is marked wrong; the least used is marked useful.
    mark(
        &mut engine,
        &Actor::Human,
        &first.uid,
        MemoryMark::Wrong,
        None,
    );
    mark(&mut engine, &agent(), &third.uid, MemoryMark::Wrong, None);
    mark(&mut engine, &agent(), &second.uid, MemoryMark::Useful, None);

    assert_eq!(listed_uids(&mut engine, "", None, None), before_all);
    assert_eq!(listed_uids(&mut engine, "parser", None, None), before_query);
    assert_eq!(
        listed_uids(
            &mut engine,
            "",
            Some(MemoryKind::Rule),
            Some(MemoryStatus::Accepted)
        ),
        before_kind
    );
    let after_fields = [&first, &second, &third].map(|memory| {
        let (read_memory, _) = read(&mut engine, &memory.uid);
        (
            read_memory.text,
            read_memory.status,
            read_memory.origin,
            read_memory.created_at,
            read_memory.decided_at,
        )
    });
    assert_eq!(
        after_fields, before_fields,
        "a mark changes nothing else about the memory"
    );
}

#[test]
fn the_reason_is_redacted_and_a_reason_over_two_hundred_characters_is_refused() {
    let world = world();
    let mut engine = engine_on(&world);
    let memory = accepted(&mut engine, "Run the release checks");
    let key = ["AKIA", "ABCDEFGHIJKLMNOP"].concat();

    let marked = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Useful,
        Some(&format!("Confirmed with {key} rotated")),
    );
    assert_eq!(state_of(&marked), OperationState::Done, "{marked:?}");
    let (_, history) = read(&mut engine, &memory.uid);
    let reason = history[0].reason.clone().expect("a reason");
    assert!(!reason.contains(&key), "{reason}");
    assert_eq!(
        reason,
        "Confirmed with [REDACTED:aws-access-key-id] rotated"
    );

    let too_long = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Wrong,
        Some(&"a".repeat(201)),
    );
    assert_eq!(state_of(&too_long), OperationState::Failed);
    assert!(
        failure(&too_long).contains("at most 200 characters"),
        "{}",
        failure(&too_long)
    );
    let (_, history) = read(&mut engine, &memory.uid);
    assert_eq!(history.len(), 1, "a refused reason writes nothing");

    let at_limit = mark(
        &mut engine,
        &agent(),
        &memory.uid,
        MemoryMark::Wrong,
        Some(&"a".repeat(200)),
    );
    assert_eq!(state_of(&at_limit), OperationState::Done, "{at_limit:?}");
    let (_, history) = read(&mut engine, &memory.uid);
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].reason.as_deref().map(str::len), Some(200));
}
