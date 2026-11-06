//! Record Combination (D180(3)) against real repositories in temporary directories.
//! A workspace is a superproject whose submodules are linked worktrees of two
//! clones, as its members are. The tests cover: the commit of the changed member
//! alone, with the message the members list; the files of the workspace left as
//! they were, staged or not; a commit that is refused, with nothing written, when
//! no member changed, when a member is detached, and when the repository has no
//! member; and the command log. Each test removes its directories when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, Command, CommandKind, Event, OperationId, OperationState};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Process::new("git")
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
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

fn oid_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev]).trim().to_owned()
}

/// Commits a new file `name` with `message` in `dir`.
fn commit(dir: &Path, name: &str, message: &str) {
    write(dir, name, &format!("{message}\n"));
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// Two clones `a` and `b`, each with `f.txt` on `main` and a branch `feat`, and a
/// workspace `ws` whose members `billing` (of `a`) and `checkout` (of `b`) are
/// linked worktrees on `feat`, registered as submodules the way a workspace adds them.
fn workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    commit(&ws, "README", "workspace");
    for name in ["billing", "checkout"] {
        let clone = tmp.path().join(format!("{name}-clone"));
        std::fs::create_dir(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);
        commit(&clone, "f.txt", "init");
        git(&clone, &["branch", "feat"]);
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                ws.join(name).to_str().unwrap(),
                "feat",
            ],
        );
        git(
            &ws,
            &[
                "submodule",
                "add",
                "-q",
                "-b",
                "feat",
                clone.to_str().unwrap(),
                name,
            ],
        );
    }
    git(&ws, &["commit", "-q", "-m", "add members"]);
    (tmp, ws)
}

fn engine_on(dir: &Path, log: &CommandLog) -> Engine {
    let git = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo {
            path: dir.to_string_lossy().into_owned(),
        }),
    );
    assert!(done(&events), "open failed: {events:?}");
    engine
}

fn human(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn done(events: &[Event]) -> bool {
    events.iter().rev().any(|e| {
        matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => Some(detail.clone().unwrap_or_default()),
        _ => None,
    })
}

fn committed(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::Committed { oid, .. } => Some(oid.clone()),
        _ => None,
    })
}

fn record(engine: &mut Engine, id: u64) -> Vec<Event> {
    engine.handle(OperationId(id), &human(CommandKind::RecordCombination))
}

#[test]
fn the_changed_member_is_recorded_alone_with_the_members_listed() {
    let (_tmp, ws) = workspace();
    let billing = ws.join("billing");
    commit(&billing, "a.txt", "second");
    let billing_head = oid_of(&billing, "HEAD");
    let checkout_head = oid_of(&ws.join("checkout"), "HEAD");
    write(&ws, "notes.txt", "my notes\n");
    write(&ws, "README", "edited\n");
    let before = oid_of(&ws, "HEAD");
    let log = CommandLog::default();
    let mut engine = engine_on(&ws, &log);

    let events = record(&mut engine, 2);

    assert!(done(&events), "record failed: {events:?}");
    let oid = committed(&events).expect("a commit");
    assert_ne!(oid, before);
    assert_eq!(oid, oid_of(&ws, "HEAD"));
    assert_eq!(
        git(&ws, &["show", "--name-only", "--format=", "HEAD"]).trim(),
        "billing"
    );
    assert!(git(&ws, &["ls-tree", "HEAD", "billing"]).contains(&billing_head));
    assert!(git(&ws, &["ls-tree", "HEAD", "checkout"]).contains(&checkout_head));
    let message = git(&ws, &["log", "-1", "--format=%B"]);
    let short_billing = &billing_head[..7];
    let short_checkout = &checkout_head[..7];
    assert_eq!(
        message.trim_end(),
        format!(
            "Record combination\n\nbilling   feat  {short_billing}\ncheckout  feat  {short_checkout}"
        )
    );
    // The workspace's own files are untouched and unstaged.
    assert_eq!(git(&ws, &["diff", "--cached", "--name-only"]), "");
    assert_eq!(git(&ws, &["diff", "--name-only"]).trim(), "README");
    assert!(git(&ws, &["status", "--porcelain"]).contains("?? notes.txt"));
    assert_eq!(
        std::fs::read_to_string(ws.join("README")).unwrap(),
        "edited\n"
    );
    // The commit goes through Git's commit command, which the log keeps.
    let entries = log.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry.args.first().map(String::as_str) == Some("commit")),
        "{entries:?}"
    );
}

#[test]
fn a_file_staged_before_the_record_stays_staged_and_out_of_it() {
    let (_tmp, ws) = workspace();
    commit(&ws.join("billing"), "a.txt", "second");
    write(&ws, "extra.txt", "staged\n");
    git(&ws, &["add", "extra.txt"]);
    let mut engine = engine_on(&ws, &CommandLog::default());

    let events = record(&mut engine, 2);

    assert!(done(&events), "record failed: {events:?}");
    assert_eq!(
        git(&ws, &["show", "--name-only", "--format=", "HEAD"]).trim(),
        "billing"
    );
    assert_eq!(
        git(&ws, &["diff", "--cached", "--name-only"]).trim(),
        "extra.txt"
    );
}

#[test]
fn no_member_with_a_new_commit_is_refused_and_nothing_is_written() {
    let (_tmp, ws) = workspace();
    let before = oid_of(&ws, "HEAD");
    write(&ws, "notes.txt", "my notes\n");
    let mut engine = engine_on(&ws, &CommandLog::default());

    let events = record(&mut engine, 2);

    assert_eq!(
        refusal(&events).as_deref(),
        Some("No member has a new commit.")
    );
    assert_eq!(committed(&events), None);
    assert_eq!(oid_of(&ws, "HEAD"), before);
    assert!(git(&ws, &["status", "--porcelain"]).contains("?? notes.txt"));
}

#[test]
fn a_detached_member_is_refused_and_named_and_nothing_is_written() {
    let (_tmp, ws) = workspace();
    commit(&ws.join("billing"), "a.txt", "second");
    git(&ws.join("checkout"), &["checkout", "-q", "--detach"]);
    let before = oid_of(&ws, "HEAD");
    let mut engine = engine_on(&ws, &CommandLog::default());

    let events = record(&mut engine, 2);

    assert_eq!(
        refusal(&events).as_deref(),
        Some("checkout is on a detached HEAD: check out its branch first.")
    );
    assert_eq!(committed(&events), None);
    assert_eq!(oid_of(&ws, "HEAD"), before);
}

#[test]
fn a_repository_without_members_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    git(&plain, &["init", "-q", "-b", "main"]);
    commit(&plain, "f.txt", "init");
    let before = oid_of(&plain, "HEAD");
    let mut engine = engine_on(&plain, &CommandLog::default());

    let events = record(&mut engine, 2);

    assert_eq!(
        refusal(&events).as_deref(),
        Some("This repository has no submodule to record.")
    );
    assert_eq!(oid_of(&plain, "HEAD"), before);
}

#[test]
fn only_a_human_may_record_a_combination() {
    let (_tmp, ws) = workspace();
    commit(&ws.join("billing"), "a.txt", "second");
    let before = oid_of(&ws, "HEAD");
    let mut engine = engine_on(&ws, &CommandLog::default());

    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Agent {
                name: "test".into(),
            },
            kind: CommandKind::RecordCombination,
        },
    );

    assert!(refusal(&events).is_some(), "{events:?}");
    assert_eq!(oid_of(&ws, "HEAD"), before);
}
