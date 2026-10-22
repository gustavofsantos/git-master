//! The reflog of HEAD (B3, D95) against real repositories in temporary
//! directories: the entries and their order after each kind of move, the time of
//! an entry, the pages, an unborn HEAD, the detached checkout, a branch from an
//! entry, the detail of an entry no branch names, and the command log. Each test
//! removes its directory when it ends.

use std::path::Path;
use std::process::Command as Process;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort};
use gm_schema::{
    Actor, Command, CommandKind, CommitDetail, Event, OperationId, OperationState, REFLOG_PAGE,
    ReflogEntry,
};

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

/// A repository on `main` with `f.txt` committed as `init`.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "t"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    write(dir.path(), "f.txt", "one\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

/// Commits a new file `name` with `message`, and returns the new id. Each commit
/// adds its own file, so rebases never conflict.
fn commit(dir: &Path, name: &str, message: &str) -> String {
    write(dir, name, &format!("{message}\n"));
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn port() -> SubprocessGit {
    SubprocessGit::default()
}

fn root_of(dir: &Path) -> std::path::PathBuf {
    port().resolve_root(dir).unwrap()
}

fn oid_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev]).trim().to_owned()
}

/// Runs `git` with extra environment, for the dates of a commit.
fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
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

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

/// The reason of the failed operation, if it failed.
fn refusal(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

/// The page the operation answered with: its entries and `has_more`.
fn page(events: &[Event]) -> Option<(Vec<ReflogEntry>, bool)> {
    events.iter().find_map(|e| match e {
        Event::ReflogLoaded {
            entries, has_more, ..
        } => Some((entries.clone(), *has_more)),
        _ => None,
    })
}

/// Whether the status the operation answered with has HEAD detached, if it
/// answered one.
fn detached_in(events: &[Event]) -> Option<bool> {
    events.iter().find_map(|e| match e {
        Event::StatusLoaded { status, .. } => Some(status.branch.head.is_none()),
        _ => None,
    })
}

fn load(engine: &mut Engine, id: u64, skip: u32) -> (Vec<ReflogEntry>, bool) {
    let events = engine.handle(OperationId(id), &human(CommandKind::LoadReflog { skip }));
    assert!(done(&events), "load failed: {events:?}");
    page(&events).expect("a page")
}

fn checkout_commit(engine: &mut Engine, id: u64, oid: &str) -> Vec<Event> {
    engine.handle(
        OperationId(id),
        &human(CommandKind::CheckoutCommit {
            oid: oid.to_owned(),
        }),
    )
}

/// The reflog file of HEAD, newest entry first: the unix time of each line.
fn reflog_file_times(dir: &Path) -> Vec<i64> {
    let text = std::fs::read_to_string(dir.join(".git/logs/HEAD")).unwrap();
    text.lines()
        .rev()
        .map(|line| {
            let meta = line.split('\t').next().unwrap();
            let mut fields = meta.rsplit(' ');
            fields.next(); // the offset
            fields.next().unwrap().parse().unwrap()
        })
        .collect()
}

/// The actions of `entries`, newest first, as (action, detail).
fn actions(entries: &[ReflogEntry]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|e| (e.action.as_str(), e.detail.as_str()))
        .collect()
}

#[test]
fn entries_follow_each_kind_of_move_newest_first() {
    let dir = repo();
    commit(dir.path(), "g.txt", "second");
    git(dir.path(), &["checkout", "-q", "-b", "topic"]);
    commit(dir.path(), "t1.txt", "topic one");
    git(
        dir.path(),
        &["commit", "-q", "--amend", "-m", "topic one amended"],
    );
    commit(dir.path(), "t2.txt", "topic two");
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), "m3.txt", "main three");
    git(dir.path(), &["checkout", "-q", "topic"]);
    git(dir.path(), &["rebase", "-q", "main"]);
    git(dir.path(), &["reset", "-q", "--hard", "HEAD~1"]);

    let entries = port().reflog(&root_of(dir.path()), 0, 50).unwrap();

    assert_eq!(
        actions(&entries),
        [
            ("reset", "moving to HEAD~1"),
            ("rebase (finish)", "returning to refs/heads/topic"),
            ("rebase (pick)", "topic two"),
            ("rebase (pick)", "topic one amended"),
            ("rebase (start)", "checkout main"),
            ("checkout", "moving from main to topic"),
            ("commit", "main three"),
            ("checkout", "moving from topic to main"),
            ("commit", "topic two"),
            ("commit (amend)", "topic one amended"),
            ("commit", "topic one"),
            ("checkout", "moving from main to topic"),
            ("commit", "second"),
            ("commit (initial)", "init"),
        ]
    );
    for (n, entry) in entries.iter().enumerate() {
        assert_eq!(entry.index as usize, n);
        assert_eq!(entry.oid, oid_of(dir.path(), &format!("HEAD@{{{n}}}")));
    }
    assert_eq!(entries[0].subject.as_deref(), Some("topic one amended"));
    assert_eq!(entries[9].subject.as_deref(), Some("topic one amended"));
}

#[test]
fn the_time_is_the_time_the_entry_was_written_not_the_commit_time() {
    let dir = repo();
    write(dir.path(), "old.txt", "old\n");
    git(dir.path(), &["add", "old.txt"]);
    let dated = "1000000000 +0000";
    git_env(
        dir.path(),
        &["commit", "-q", "-m", "backdated"],
        &[("GIT_AUTHOR_DATE", dated), ("GIT_COMMITTER_DATE", dated)],
    );
    commit(dir.path(), "recent.txt", "recent");
    git(dir.path(), &["reset", "-q", "--hard", "HEAD~1"]);

    let entries = port().reflog(&root_of(dir.path()), 0, 10).unwrap();

    // The reset is the newest entry: it was written now, though the commit it
    // moved HEAD to was made in 2001.
    assert_eq!(entries[0].action, "reset");
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%ct", &entries[0].oid]).trim(),
        "1000000000"
    );
    assert!(entries[0].time > 1_700_000_000, "{}", entries[0].time);
    // Git writes a commit's entry with the commit's own date, as the file holds it.
    assert_eq!(entries[2].subject.as_deref(), Some("backdated"));
    assert_eq!(entries[2].time, 1_000_000_000);
    let times: Vec<i64> = entries.iter().map(|e| e.time).collect();
    assert_eq!(times, reflog_file_times(dir.path()));
}

#[test]
fn the_entries_are_read_in_pages_and_has_more_is_known() {
    let dir = repo();
    let page_size = u32::try_from(REFLOG_PAGE).unwrap();
    // The init commit and these make REFLOG_PAGE + 6 entries.
    for n in 0..REFLOG_PAGE + 5 {
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", &format!("n{n}")],
        );
    }
    let root = root_of(dir.path());
    let total = page_size + 6;

    let first = port().reflog(&root, 0, REFLOG_PAGE + 1).unwrap();
    assert_eq!(first.len(), REFLOG_PAGE + 1);
    let rest = port().reflog(&root, page_size, REFLOG_PAGE + 1).unwrap();
    assert_eq!(rest.len(), 6);
    assert_eq!(rest[0].index, page_size);
    assert_eq!(
        rest[0].oid,
        oid_of(dir.path(), &format!("HEAD@{{{REFLOG_PAGE}}}"))
    );
    assert!(
        port()
            .reflog(&root, total, REFLOG_PAGE + 1)
            .unwrap()
            .is_empty()
    );

    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let (page, has_more) = load(&mut engine, 2, 0);
    assert_eq!(page.len(), REFLOG_PAGE);
    assert!(has_more);
    assert_eq!(page[REFLOG_PAGE - 1].index as usize, REFLOG_PAGE - 1);

    let (page, has_more) = load(&mut engine, 3, page_size);
    assert_eq!(page.len(), 6);
    assert!(!has_more);
    assert_eq!(page[0].index as usize, REFLOG_PAGE);
    assert_eq!(page[5].index, total - 1);

    // Exactly one page left: nothing more follows.
    let (page, has_more) = load(&mut engine, 4, 6);
    assert_eq!(page.len(), REFLOG_PAGE);
    assert!(!has_more);
}

#[test]
fn an_unborn_head_is_an_empty_reflog_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);

    assert!(
        port()
            .reflog(&root_of(dir.path()), 0, REFLOG_PAGE + 1)
            .unwrap()
            .is_empty()
    );

    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let (page, has_more) = load(&mut engine, 2, 0);
    assert!(page.is_empty());
    assert!(!has_more);
}

#[test]
fn a_repository_without_a_reflog_is_an_empty_list() {
    let dir = repo();
    std::fs::remove_dir_all(dir.path().join(".git/logs")).unwrap();

    assert!(
        port()
            .reflog(&root_of(dir.path()), 0, REFLOG_PAGE + 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_reads_are_not_logged_and_do_not_wait_for_index_lock() {
    let dir = repo();
    commit(dir.path(), "g.txt", "second");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    write(dir.path(), ".git/index.lock", "");

    let (entries, _) = load(&mut engine, 2, 0);
    assert_eq!(entries.len(), 2);
    let agent = engine.handle(
        OperationId(3),
        &Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::LoadReflog { skip: 0 },
        },
    );
    assert!(page(&agent).is_some());

    assert!(log.entries().is_empty());
    assert!(
        engine
            .log()
            .iter()
            .all(|entry| !matches!(entry.kind, CommandKind::LoadReflog { .. }))
    );
    assert!(port().reflog(&root_of(dir.path()), 0, 5).is_ok());
    std::fs::remove_file(dir.path().join(".git/index.lock")).unwrap();
}

#[test]
fn a_checkout_detaches_head_at_the_entry_and_is_logged() {
    let dir = repo();
    let init = oid_of(dir.path(), "HEAD");
    let second = commit(dir.path(), "g.txt", "second");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = checkout_commit(&mut engine, 2, &init);

    assert!(done(&events), "{events:?}");
    assert_eq!(detached_in(&events), Some(true));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BranchesLoaded { .. }))
    );
    assert_eq!(oid_of(dir.path(), "HEAD"), init);
    assert!(
        !git_output(dir.path(), &["symbolic-ref", "-q", "HEAD"])
            .status
            .success()
    );
    let logged = log.entries();
    assert_eq!(logged.len(), 1);
    assert_eq!(
        logged[0].args.join(" "),
        format!("checkout --detach {init}")
    );
    assert!(engine.log().iter().any(|entry| {
        entry.id == OperationId(2) && matches!(entry.kind, CommandKind::CheckoutCommit { .. })
    }));
    assert_ne!(init, second);
}

#[test]
fn a_bad_oid_is_refused_before_git_runs() {
    let dir = repo();
    let second = commit(dir.path(), "g.txt", "second");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let too_long = "a".repeat(65);
    let bad = [
        "",
        "--detach",
        "main",
        "HEAD",
        "HEAD~1",
        "abc12g4",
        "abc 1234",
        too_long.as_str(),
    ];

    for (n, oid) in bad.iter().enumerate() {
        let events = checkout_commit(&mut engine, 2 + n as u64, oid);
        let reason = refusal(&events).unwrap_or_default();
        assert!(
            reason.contains("7 to 64 hex characters"),
            "{oid:?}: {reason}"
        );
    }

    assert!(log.entries().is_empty());
    assert_eq!(oid_of(dir.path(), "HEAD"), second);
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    assert!(port.checkout_commit(&root_of(dir.path()), "main").is_err());
    assert!(log.entries().is_empty());
}

#[test]
fn an_agent_cannot_check_out_a_commit() {
    let dir = repo();
    let init = oid_of(dir.path(), "HEAD");
    let second = commit(dir.path(), "g.txt", "second");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(
        OperationId(2),
        &Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::CheckoutCommit { oid: init },
        },
    );

    assert_eq!(
        refusal(&events).as_deref(),
        Some("only a human can check out a commit")
    );
    assert_eq!(oid_of(dir.path(), "HEAD"), second);
    assert!(log.entries().is_empty());
}

#[test]
fn local_changes_that_would_be_overwritten_give_gits_text() {
    let dir = repo();
    let init = oid_of(dir.path(), "HEAD");
    write(dir.path(), "f.txt", "two\n");
    git(dir.path(), &["commit", "-q", "-am", "two"]);
    let second = oid_of(dir.path(), "HEAD");
    write(dir.path(), "f.txt", "dirty\n");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = checkout_commit(&mut engine, 2, &init);

    let reason = refusal(&events).unwrap_or_default();
    assert!(
        reason.contains("would be overwritten by checkout"),
        "{reason}"
    );
    assert!(reason.contains("f.txt"), "{reason}");
    assert_eq!(read(dir.path(), "f.txt"), "dirty\n");
    assert_eq!(oid_of(dir.path(), "HEAD"), second);
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap()
}

#[test]
fn a_branch_can_start_from_an_entry_no_branch_names() {
    let dir = repo();
    let init = oid_of(dir.path(), "HEAD");
    let second = commit(dir.path(), "g.txt", "second");
    git(dir.path(), &["reset", "-q", "--hard", &init]);
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(
        OperationId(2),
        &human(CommandKind::CreateBranch {
            name: "rescued".into(),
            start_point: Some(second.clone()),
            checkout: true,
        }),
    );

    assert!(done(&events), "{events:?}");
    assert_eq!(oid_of(dir.path(), "rescued"), second);
    assert_eq!(oid_of(dir.path(), "HEAD"), second);
    assert_eq!(
        git(dir.path(), &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/rescued"
    );
}

#[test]
fn the_detail_of_an_entry_no_branch_names_loads() {
    let dir = repo();
    let init = oid_of(dir.path(), "HEAD");
    let second = commit(dir.path(), "g.txt", "second");
    git(dir.path(), &["reset", "-q", "--hard", &init]);
    assert!(
        git(dir.path(), &["branch", "--contains", &second])
            .trim()
            .is_empty()
    );
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    engine.handle(
        OperationId(2),
        &human(CommandKind::LoadCommitDetail {
            oid: second.clone(),
        }),
    );
    let details: Vec<CommitDetail> = engine
        .take_detail_jobs()
        .into_iter()
        .flat_map(|job| engine.detail_report(job.run()))
        .filter_map(|event| match event {
            Event::CommitDetailLoaded { detail, .. } => Some(detail),
            _ => None,
        })
        .collect();

    assert_eq!(details.len(), 1);
    assert_eq!(details[0].oid, second);
    assert_eq!(details[0].subject, "second");
    let paths: Vec<&str> = details[0].files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["g.txt"]);
}
