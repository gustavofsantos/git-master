//! The file list, search, folder listing and file read through the engine, with
//! a fake Git (N1, N2). The flow is the one the runner drives: a command is
//! handled, the jobs it accepted are run, and each report goes back in.

use std::sync::atomic::Ordering;

use gm_core::fake::FakeGit;
use gm_core::{Engine, GitError};
use gm_schema::{
    Actor, Command, CommandKind, Event, FileBody, FileSearch, OperationId, OperationState,
    RepoStatus,
};

fn command(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn engine_over(root: &str, files: &[&str]) -> (Engine, FakeGit) {
    let git = FakeGit::new(
        root,
        RepoStatus {
            root: root.into(),
            ..RepoStatus::default()
        },
    );
    git.set_files(Ok(files.iter().map(|f| (*f).to_owned()).collect()));
    let mut engine = Engine::new(Box::new(git.clone()));
    engine.handle(
        OperationId(1),
        &command(CommandKind::OpenRepo { path: root.into() }),
    );
    (engine, git)
}

fn search(query: &str) -> Command {
    command(CommandKind::SearchFiles {
        query: query.into(),
    })
}

/// Runs every job the engine accepted, feeding the reports back, until none is left.
fn drain(engine: &mut Engine) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let index = engine.take_index_jobs();
        let searches = engine.take_search_jobs();
        let reads = engine.take_read_jobs();
        if index.is_empty() && searches.is_empty() && reads.is_empty() {
            return events;
        }
        for job in index {
            events.extend(engine.index_report(job.run()));
        }
        for job in searches {
            events.extend(engine.search_report(job.run()));
        }
        for job in reads {
            events.extend(engine.read_report(job.run()));
        }
    }
}

fn found(events: &[Event]) -> Vec<(OperationId, FileSearch)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::FilesFound { id, search } => Some((*id, search.clone())),
            _ => None,
        })
        .collect()
}

fn state(id: u64, state: OperationState, detail: Option<&str>) -> Event {
    Event::OperationChanged {
        id: OperationId(id),
        actor: Actor::Human,
        state,
        detail: detail.map(str::to_owned),
    }
}

#[test]
fn a_search_before_any_list_reads_the_list_then_answers() {
    let (mut engine, git) = engine_over("/r", &["src/main.rs", "src/lib.rs", "README.md"]);

    let started = engine.handle(OperationId(2), &search("main"));

    // Executing, not done: the answer comes after the list is read.
    assert!(
        !started
            .iter()
            .any(|e| matches!(e, Event::FilesFound { .. }))
    );
    let events = drain(&mut engine);
    assert!(events.contains(&Event::FilesIndexed {
        id: OperationId(2),
        files: 3
    }));
    let answers = found(&events);
    assert_eq!(answers.len(), 1);
    let (id, answer) = &answers[0];
    assert_eq!(*id, OperationId(2));
    assert_eq!(answer.hits[0].path, "src/main.rs");
    assert_eq!(answer.files, 3);
    assert!(!answer.indexing);
    assert_eq!(git.file_reads(), 1);
    assert!(events.contains(&state(2, OperationState::Done, None)));
}

#[test]
fn a_search_with_the_list_in_memory_reads_nothing_again() {
    let (mut engine, git) = engine_over("/r", &["a.rs", "b.rs"]);
    engine.handle(OperationId(2), &search("a"));
    drain(&mut engine);

    engine.handle(OperationId(3), &search("b"));
    let events = drain(&mut engine);

    assert_eq!(git.file_reads(), 1);
    let answers = found(&events);
    assert_eq!(answers[0].0, OperationId(3));
    assert_eq!(answers[0].1.hits[0].path, "b.rs");
}

#[test]
fn a_new_search_cancels_the_one_in_flight_and_its_answer_is_dropped() {
    let (mut engine, _) = engine_over("/r", &["a.rs", "b.rs"]);
    engine.handle(OperationId(2), &search("a"));
    drain(&mut engine);
    engine.handle(OperationId(3), &search("a"));
    let superseding = engine.handle(OperationId(4), &search("b"));
    assert!(superseding.contains(&state(3, OperationState::Cancelled, None)));

    let mut jobs = engine.take_search_jobs().into_iter();
    let stale = jobs.next().unwrap();
    assert!(
        stale.stop.load(Ordering::SeqCst),
        "the stale search must be stopped"
    );
    assert!(engine.search_report(stale.run()).is_empty());
    let current = jobs.next().unwrap();
    assert!(!current.stop.load(Ordering::SeqCst));
    let events = engine.search_report(current.run());
    assert_eq!(found(&events)[0].0, OperationId(4));
}

#[test]
fn a_search_that_waited_for_the_first_list_is_superseded_without_an_answer() {
    let (mut engine, _) = engine_over("/r", &["a.rs"]);
    engine.handle(OperationId(2), &search("a"));
    let superseding = engine.handle(OperationId(3), &search("a"));
    assert!(superseding.contains(&state(2, OperationState::Cancelled, None)));

    let events = drain(&mut engine);

    let answers = found(&events);
    assert_eq!(answers.len(), 1, "{events:?}");
    assert_eq!(answers[0].0, OperationId(3));
}

#[test]
fn index_files_reads_the_list_again_but_not_twice_at_once() {
    let (mut engine, git) = engine_over("/r", &["a.rs"]);
    engine.handle(OperationId(2), &command(CommandKind::IndexFiles));
    engine.handle(OperationId(3), &command(CommandKind::IndexFiles));
    assert_eq!(git.file_reads(), 1, "one read in flight is enough");
    let events = drain(&mut engine);
    assert!(events.contains(&Event::FilesIndexed {
        id: OperationId(2),
        files: 1
    }));

    git.set_files(Ok(vec!["a.rs".into(), "new.rs".into()]));
    engine.handle(OperationId(4), &command(CommandKind::IndexFiles));
    let events = drain(&mut engine);

    assert_eq!(git.file_reads(), 2);
    assert!(events.contains(&Event::FilesIndexed {
        id: OperationId(4),
        files: 2
    }));
}

#[test]
fn the_old_list_keeps_answering_while_a_new_one_is_read() {
    let (mut engine, git) = engine_over("/r", &["a.rs"]);
    engine.handle(OperationId(2), &command(CommandKind::IndexFiles));
    drain(&mut engine);
    git.set_files(Ok(vec!["a.rs".into(), "b.rs".into()]));
    engine.handle(OperationId(3), &command(CommandKind::IndexFiles));

    engine.handle(OperationId(4), &search("a"));
    let search_jobs = engine.take_search_jobs();
    let answer = engine.search_report(search_jobs.into_iter().next().unwrap().run());

    let (_, found) = &found(&answer)[0];
    assert_eq!(found.files, 1, "the old list answered");
    assert!(found.indexing, "and says the list is being read");
}

#[test]
fn a_folder_listing_comes_from_the_list_in_memory() {
    let (mut engine, _) = engine_over("/r", &["src/a.rs", "src/b.rs", "Cargo.toml"]);
    engine.handle(OperationId(2), &command(CommandKind::IndexFiles));
    drain(&mut engine);

    let events = engine.handle(
        OperationId(3),
        &command(CommandKind::ListDirectory { dir: String::new() }),
    );

    let listing = events
        .iter()
        .find_map(|e| match e {
            Event::DirectoryListed { listing, .. } => Some(listing),
            _ => None,
        })
        .expect("a listing");
    let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["src", "Cargo.toml"]);
    assert!(!listing.indexing);
    assert!(events.contains(&state(3, OperationState::Done, None)));
}

#[test]
fn a_folder_listing_before_any_list_is_empty_and_starts_the_read() {
    let (mut engine, git) = engine_over("/r", &["a.rs"]);

    let events = engine.handle(
        OperationId(2),
        &command(CommandKind::ListDirectory { dir: String::new() }),
    );

    let Some(Event::DirectoryListed { listing, .. }) = events
        .iter()
        .find(|e| matches!(e, Event::DirectoryListed { .. }))
    else {
        panic!("no listing in {events:?}");
    };
    assert!(listing.entries.is_empty());
    assert!(listing.indexing);
    assert_eq!(git.file_reads(), 1);
}

#[test]
fn a_failed_list_read_fails_the_search_that_waited_with_gits_message() {
    let (mut engine, git) = engine_over("/r", &["a.rs"]);
    git.set_files(Err(GitError("fatal: not a git repository".into())));
    engine.handle(OperationId(2), &search("a"));

    let events = drain(&mut engine);

    assert!(events.contains(&state(
        2,
        OperationState::Failed,
        Some("fatal: not a git repository")
    )));
    assert!(found(&events).is_empty());
}

#[test]
fn leaving_the_repository_cancels_the_search_and_forgets_the_list() {
    let (mut engine, git) = engine_over("/r", &["a.rs"]);
    engine.handle(OperationId(2), &search("a"));
    let job = engine.take_index_jobs().pop().unwrap();

    let reopened = engine.handle(
        OperationId(3),
        &command(CommandKind::OpenRepo { path: "/r".into() }),
    );

    assert!(reopened.contains(&state(2, OperationState::Cancelled, None)));
    assert!(job.stop.load(Ordering::SeqCst));
    assert!(
        engine.index_report(job.run()).is_empty(),
        "a stale list is dropped"
    );
    // The list is read afresh for the repository now open.
    engine.handle(OperationId(4), &search("a"));
    drain(&mut engine);
    assert_eq!(git.file_reads(), 2);
}

#[test]
fn searching_with_no_repository_open_fails() {
    let git = FakeGit::new("/r", RepoStatus::default());
    let mut engine = Engine::new(Box::new(git));

    let events = engine.handle(OperationId(1), &search("a"));

    assert!(events.iter().any(|e| matches!(
        e,
        Event::OperationChanged {
            state: OperationState::Failed,
            ..
        }
    )));
}

#[test]
fn a_file_read_answers_with_the_text_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    let root = dir.path().to_str().unwrap();
    let (mut engine, _) = engine_over(root, &[]);

    engine.handle(
        OperationId(2),
        &command(CommandKind::ReadFile {
            path: "a.rs".into(),
        }),
    );
    let events = drain(&mut engine);

    let file = events
        .iter()
        .find_map(|e| match e {
            Event::FileRead { file, .. } => Some(file),
            _ => None,
        })
        .expect("a file");
    assert_eq!(
        file.body,
        FileBody::Text {
            text: "fn a() {}\n".into()
        }
    );
}

#[test]
fn a_new_file_read_cancels_the_one_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), "a").unwrap();
    std::fs::write(dir.path().join("b"), "b").unwrap();
    let (mut engine, _) = engine_over(dir.path().to_str().unwrap(), &[]);
    let read = |path: &str| command(CommandKind::ReadFile { path: path.into() });
    engine.handle(OperationId(2), &read("a"));

    let superseding = engine.handle(OperationId(3), &read("b"));

    assert!(superseding.contains(&state(2, OperationState::Cancelled, None)));
    let mut jobs = engine.take_read_jobs().into_iter();
    let stale = jobs.next().unwrap();
    assert!(stale.stop.load(Ordering::SeqCst));
    assert!(engine.read_report(stale.run()).is_empty());
    let events = engine.read_report(jobs.next().unwrap().run());
    assert!(matches!(
        events.last(),
        Some(Event::FileRead {
            id: OperationId(3),
            ..
        })
    ));
}

#[test]
fn a_path_outside_the_worktree_fails_the_read_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, _) = engine_over(dir.path().to_str().unwrap(), &[]);

    engine.handle(
        OperationId(2),
        &command(CommandKind::ReadFile {
            path: "../secret".into(),
        }),
    );
    let events = drain(&mut engine);

    assert!(events.iter().any(|e| matches!(
        e,
        Event::OperationChanged { state: OperationState::Failed, detail: Some(d), .. }
            if d.contains("not a path inside the worktree")
    )));
}
