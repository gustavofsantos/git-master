//! The whole-file blame of the terminal companion (D247) against real repositories in
//! temporary directories. Each test removes its directory when it ends.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, BlameEntry, Command, CommandKind, Event, OperationId, OperationState};

fn git(dir: &Path, args: &[&str], date: Option<&str>) {
    let mut command = Process::new("git");
    command
        .args([
            "-c",
            "user.name=Ada",
            "-c",
            "user.email=a@a",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn handle(engine: &mut Engine, id: u64, kind: CommandKind) -> Vec<Event> {
    engine.handle(
        OperationId(id),
        &Command {
            actor: Actor::Human,
            kind,
        },
    )
}

fn blame(engine: &mut Engine, id: u64, path: &str) -> Vec<Event> {
    let events = handle(
        engine,
        id,
        CommandKind::LoadBlame {
            path: path.to_owned(),
        },
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Failed,
                ..
            }
        )),
        "{events:?}"
    );
    let job = engine.take_provenance_jobs().pop().expect("a blame job");
    engine.provenance_report(job.run())
}

fn lines_of(events: &[Event]) -> Vec<BlameEntry> {
    events
        .iter()
        .find_map(|event| match event {
            Event::BlameLoaded { lines, .. } => Some(lines.clone()),
            _ => None,
        })
        .expect("a blame answer")
}

#[test]
fn every_line_has_its_commit_and_uncommitted_lines_have_none() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"], None);
    std::fs::write(root.join("a.rs"), "one\ntwo\nthree\n").unwrap();
    git(root, &["add", "a.rs"], None);
    git(
        root,
        &["commit", "-q", "-m", "Start"],
        Some("2020-01-01T00:00:00Z"),
    );
    std::fs::write(root.join("a.rs"), "one\nTWO\nthree\n").unwrap();
    git(
        root,
        &["commit", "-q", "-am", "Shout two"],
        Some("2020-02-01T00:00:00Z"),
    );
    std::fs::write(root.join("a.rs"), "one\nTWO\nthree\nfour\n").unwrap();

    let mut engine = Engine::new(Box::new(SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    }));
    handle(
        &mut engine,
        1,
        CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    );
    let events = blame(&mut engine, 2, "a.rs");
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )),
        "{events:?}"
    );
    let lines = lines_of(&events);
    let subjects: Vec<(u32, &str, bool)> = lines
        .iter()
        .map(|l| (l.line, l.subject.as_str(), l.commit.is_some()))
        .collect();
    assert_eq!(
        subjects,
        [
            (1, "Start", true),
            (2, "Shout two", true),
            (3, "Start", true),
            (4, "", false)
        ]
    );
    assert_eq!(lines[0].author, "Ada");
    assert_eq!(lines[0].author_at, 1_577_836_800_000);
}

#[test]
fn a_new_blame_supersedes_the_one_in_flight_and_a_bad_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"], None);
    std::fs::write(root.join("a.rs"), "one\n").unwrap();
    git(root, &["add", "a.rs"], None);
    git(root, &["commit", "-q", "-m", "Start"], None);
    let mut engine = Engine::new(Box::new(SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    }));
    handle(
        &mut engine,
        1,
        CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    );
    handle(
        &mut engine,
        2,
        CommandKind::LoadBlame {
            path: "a.rs".to_owned(),
        },
    );
    let second = handle(
        &mut engine,
        3,
        CommandKind::LoadBlame {
            path: "a.rs".to_owned(),
        },
    );
    assert!(second.iter().any(|e| matches!(
        e,
        Event::OperationChanged {
            id: OperationId(2),
            state: OperationState::Cancelled,
            ..
        }
    )));
    let mut jobs = engine.take_provenance_jobs().into_iter();
    let stale = jobs.next().unwrap();
    assert!(engine.provenance_report(stale.run()).is_empty());
    let current = jobs.next().unwrap();
    assert_eq!(lines_of(&engine.provenance_report(current.run())).len(), 1);
    let refused = handle(
        &mut engine,
        4,
        CommandKind::LoadBlame {
            path: "--all".to_owned(),
        },
    );
    assert!(refused.iter().any(|e| matches!(
        e,
        Event::OperationChanged {
            state: OperationState::Failed,
            ..
        }
    )));
}
