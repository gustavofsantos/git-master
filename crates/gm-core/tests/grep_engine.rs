//! The text search through the engine, with a fake Git (N3): the flow the
//! runner drives, and the cancelling of a search that a newer one superseded.

use std::sync::atomic::Ordering;

use gm_core::fake::FakeGit;
use gm_core::{Engine, GitError};
use gm_schema::{
    Actor, Command, CommandKind, Event, GrepFile, GrepLine, GrepOptions, GrepResult, GrepScope,
    OperationId, OperationState, RepoStatus,
};

fn command(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn search(query: &str) -> Command {
    command(CommandKind::SearchText {
        query: query.into(),
        options: GrepOptions::default(),
        paths: String::new(),
    })
}

fn engine() -> (Engine, FakeGit) {
    let git = FakeGit::new(
        "/r",
        RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        },
    );
    let mut engine = Engine::new(Box::new(git.clone()));
    engine.handle(
        OperationId(1),
        &command(CommandKind::OpenRepo { path: "/r".into() }),
    );
    (engine, git)
}

fn result(query: &str) -> GrepResult {
    GrepResult {
        query: query.into(),
        files: vec![GrepFile {
            path: "a.rs".into(),
            lines: vec![GrepLine {
                line: 3,
                text: query.into(),
                ranges: vec![(0, query.len())],
                cut_start: false,
                cut_end: false,
            }],
            more: 0,
        }],
        listed: 1,
        truncated: false,
    }
}

fn state(id: u64, state: OperationState, detail: Option<&str>) -> Event {
    Event::OperationChanged {
        id: OperationId(id),
        actor: Actor::Human,
        state,
        detail: detail.map(str::to_owned),
    }
}

fn is_failed(events: &[Event], id: u64) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            id: got,
            state: OperationState::Failed,
            detail,
            ..
        } if *got == OperationId(id) => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

#[test]
fn a_search_runs_on_a_job_and_answers_with_the_result() {
    let (mut engine, git) = engine();
    git.set_grep(Ok(result("needle")));

    let started = engine.handle(OperationId(2), &search("needle"));
    assert!(!started.iter().any(|e| matches!(e, Event::TextFound { .. })));
    let job = engine.take_text_jobs().pop().expect("a job");
    let events = engine.text_report(job.run());

    assert!(events.contains(&state(2, OperationState::Done, None)));
    assert!(events.contains(&Event::TextFound {
        id: OperationId(2),
        result: result("needle")
    }));
}

#[test]
fn the_request_carries_the_options_and_the_paths_to_git() {
    let (mut engine, git) = engine();

    engine.handle(
        OperationId(2),
        &command(CommandKind::SearchText {
            query: "-v".into(),
            options: GrepOptions {
                case_sensitive: true,
                whole_word: true,
                regex: false,
                scope: GrepScope::TrackedOnly,
            },
            paths: "crates !docs".into(),
        }),
    );

    let requests = git.greps();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query, "-v");
    assert!(requests[0].options.case_sensitive && requests[0].options.whole_word);
    assert_eq!(requests[0].options.scope, GrepScope::TrackedOnly);
    assert_eq!(requests[0].paths, ["crates", ":(exclude)docs"]);
}

#[test]
fn a_new_search_cancels_the_one_in_flight_and_its_answer_is_dropped() {
    let (mut engine, git) = engine();
    git.set_grep(Ok(result("x")));
    engine.handle(OperationId(2), &search("a"));

    let superseding = engine.handle(OperationId(3), &search("ab"));

    assert!(superseding.contains(&state(2, OperationState::Cancelled, None)));
    let mut jobs = engine.take_text_jobs().into_iter();
    let stale = jobs.next().unwrap();
    assert!(
        stale.stop.load(Ordering::SeqCst),
        "the stale search must be stopped"
    );
    assert!(engine.text_report(stale.run()).is_empty());
    let current = jobs.next().unwrap();
    assert!(!current.stop.load(Ordering::SeqCst));
    let events = engine.text_report(current.run());
    assert!(matches!(
        events.last(),
        Some(Event::TextFound {
            id: OperationId(3),
            ..
        })
    ));
}

#[test]
fn a_text_git_cannot_take_is_refused_and_leaves_the_search_in_flight_alone() {
    let (mut engine, _) = engine();
    engine.handle(OperationId(2), &search("a"));

    let refused = engine.handle(OperationId(3), &search(""));

    assert_eq!(
        is_failed(&refused, 3).as_deref(),
        Some("the search text is empty")
    );
    assert!(!refused.contains(&state(2, OperationState::Cancelled, None)));
    let job = engine.take_text_jobs().pop().unwrap();
    assert!(!job.stop.load(Ordering::SeqCst));
    let multi = engine.handle(OperationId(4), &search("a\nb"));
    assert!(is_failed(&multi, 4).is_some());
}

#[test]
fn a_failed_search_fails_with_gits_own_message() {
    let (mut engine, git) = engine();
    git.set_grep(Err(GitError(
        "fatal: -e option, '(': Unmatched ( or \\(".into(),
    )));
    engine.handle(OperationId(2), &search("("));

    let job = engine.take_text_jobs().pop().unwrap();
    let events = engine.text_report(job.run());

    assert_eq!(
        is_failed(&events, 2).as_deref(),
        Some("fatal: -e option, '(': Unmatched ( or \\(")
    );
    assert!(!events.iter().any(|e| matches!(e, Event::TextFound { .. })));
}

#[test]
fn leaving_the_repository_cancels_the_search() {
    let (mut engine, _) = engine();
    engine.handle(OperationId(2), &search("a"));
    let job = engine.take_text_jobs().pop().unwrap();

    let reopened = engine.handle(
        OperationId(3),
        &command(CommandKind::OpenRepo { path: "/r".into() }),
    );

    assert!(reopened.contains(&state(2, OperationState::Cancelled, None)));
    assert!(job.stop.load(Ordering::SeqCst));
    assert!(engine.text_report(job.run()).is_empty());
}

#[test]
fn searching_with_no_repository_open_fails() {
    let git = FakeGit::new("/r", RepoStatus::default());
    let mut engine = Engine::new(Box::new(git));

    let events = engine.handle(OperationId(1), &search("a"));

    assert!(is_failed(&events, 1).is_some());
}
