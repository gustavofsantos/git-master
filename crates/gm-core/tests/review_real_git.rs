//! Review comments (B4, D96, D97) against real repositories in temporary
//! directories: the comment commands through the engine, the snapshots through
//! the adapter, and what the snapshots leave untouched. Each test removes its
//! directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, ReviewComment, ReviewLocation,
    ReviewStatus,
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

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap()
}

/// A repository on `main` with `f.txt` ("a" to "e", one per line) committed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "t"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    write(dir.path(), "f.txt", "a\nb\nc\nd\ne\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn port() -> SubprocessGit {
    SubprocessGit::default()
}

fn human(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Human,
        kind,
    }
}

fn agent(kind: CommandKind) -> Command {
    Command {
        actor: Actor::Agent {
            name: "test-agent".to_owned(),
        },
        kind,
    }
}

fn add(path: &str, start_line: u32, end_line: u32, body: &str) -> CommandKind {
    CommandKind::AddReviewComment {
        path: path.to_owned(),
        start_line,
        end_line,
        body: body.to_owned(),
    }
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

fn state_of(events: &[Event]) -> Option<OperationState> {
    events.iter().rev().find_map(|e| match e {
        Event::OperationChanged { state, .. } => Some(*state),
        _ => None,
    })
}

fn done(events: &[Event]) -> bool {
    state_of(events) == Some(OperationState::Done)
}

/// The reason the operation failed, or an empty string.
fn failure(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
        .unwrap_or_default()
}

/// The comments of the last `ReviewCommentsListed` in `events`.
fn listed(events: &[Event]) -> Vec<ReviewComment> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::ReviewCommentsListed { comments, .. } => Some(comments.clone()),
            _ => None,
        })
        .expect("a comment listing")
}

fn snapshot_text(events: &[Event]) -> (String, Vec<String>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::ReviewSnapshotText { path, lines, .. } => Some((path.clone(), lines.clone())),
            _ => None,
        })
        .expect("a snapshot text")
}

/// Lists the comments through the engine: the list is a read, not a write.
fn list(engine: &mut Engine) -> Vec<ReviewComment> {
    let events = engine.handle(OperationId(900), &human(CommandKind::ListReviewComments));
    assert!(done(&events));
    listed(&events)
}

fn only(engine: &mut Engine) -> ReviewComment {
    let mut comments = list(engine);
    assert_eq!(comments.len(), 1, "one comment: {comments:?}");
    comments.remove(0)
}

fn location(start_line: u32, end_line: u32) -> ReviewLocation {
    ReviewLocation {
        start_line,
        end_line,
    }
}

/// The refs of the repository, without the private review refs.
fn public_refs(dir: &Path) -> String {
    git(dir, &["for-each-ref"])
        .lines()
        .filter(|line| !line.contains("refs/gitmaster/"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn index_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(".git").join("index")).unwrap_or_default()
}

fn private_refs(dir: &Path) -> Vec<String> {
    git(
        dir,
        &["for-each-ref", "--format=%(refname)", "refs/gitmaster/"],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

#[test]
fn a_comment_is_added_listed_resolved_reopened_edited_and_deleted() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(
        OperationId(10),
        &human(add("f.txt", 2, 3, "  rename these  ")),
    );
    assert!(done(&events), "{}", failure(&events));
    let comment = only(&mut engine);
    assert_eq!(comment.path, "f.txt");
    assert_eq!(comment.body, "rename these");
    assert_eq!(comment.status, ReviewStatus::Open);
    assert_eq!(comment.anchor.text, "b\nc");
    assert_eq!(comment.anchor.start_line, 2);
    assert_eq!(comment.location, Some(location(2, 3)));
    assert!(!comment.outdated);
    assert!(comment.snapshot.is_some());
    assert_eq!(comment.resolved_at, None);
    let id = comment.id;

    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    let resolved = listed(&events).remove(0);
    assert_eq!(resolved.status, ReviewStatus::Resolved);
    assert!(resolved.resolved_at.is_some());

    let events = engine.handle(
        OperationId(12),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    assert!(!done(&events));
    assert_eq!(failure(&events), "the comment is already resolved");

    let events = engine.handle(
        OperationId(13),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    let reopened = listed(&events).remove(0);
    assert_eq!(reopened.status, ReviewStatus::Open);
    assert_eq!(reopened.resolved_at, None);

    let events = engine.handle(
        OperationId(14),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    assert_eq!(failure(&events), "the comment is not resolved");

    let events = engine.handle(
        OperationId(15),
        &human(CommandKind::EditReviewComment {
            id,
            body: "   ".to_owned(),
        }),
    );
    assert_eq!(failure(&events), "the comment is empty");
    let events = engine.handle(
        OperationId(16),
        &human(CommandKind::EditReviewComment {
            id,
            body: "  name it properly  ".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    assert_eq!(listed(&events)[0].body, "name it properly");

    let events = engine.handle(
        OperationId(17),
        &human(CommandKind::DeleteReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    assert!(listed(&events).is_empty());
    assert!(list(&mut engine).is_empty());

    let missing = engine.handle(
        OperationId(18),
        &human(CommandKind::DeleteReviewComment { id }),
    );
    assert_eq!(failure(&missing), format!("no review comment {id}"));
}

#[test]
fn the_private_ref_is_created_for_a_comment_and_deleted_with_it() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 1, "first")));
    let comment = only(&mut engine);
    let snapshot = comment.snapshot.clone().expect("a snapshot");
    assert_eq!(
        private_refs(dir.path()),
        vec![format!("refs/gitmaster/review/{}/before", comment.id)]
    );
    assert_eq!(
        git(
            dir.path(),
            &[
                "rev-parse",
                &format!("refs/gitmaster/review/{}/before", comment.id)
            ]
        )
        .trim(),
        snapshot
    );

    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::DeleteReviewComment { id: comment.id }),
    );
    assert!(done(&events));
    assert!(private_refs(dir.path()).is_empty());
}

#[test]
fn a_location_moves_when_lines_are_inserted_above_it() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "this c")));
    assert_eq!(only(&mut engine).location, Some(location(3, 3)));

    write(dir.path(), "f.txt", "x\ny\na\nb\nc\nd\ne\n");
    let comment = only(&mut engine);
    assert_eq!(comment.location, Some(location(5, 5)));
    assert!(!comment.outdated);
    assert_eq!(
        comment.anchor.start_line, 3,
        "the anchor keeps the line it was made on"
    );
}

#[test]
fn a_comment_is_outdated_when_its_text_is_edited_or_duplicated() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "this c")));

    write(dir.path(), "f.txt", "a\nb\nC\nd\ne\n");
    let edited = only(&mut engine);
    assert_eq!(edited.location, None);
    assert!(edited.outdated);

    write(dir.path(), "f.txt", "a\nb\nc\nd\nc\ne\n");
    let duplicated = only(&mut engine);
    assert_eq!(
        duplicated.location, None,
        "two copies of the text is not a match"
    );
    assert!(duplicated.outdated);

    write(dir.path(), "f.txt", "a\nb\nc\nd\ne\n");
    let back = only(&mut engine);
    assert_eq!(back.location, Some(location(3, 3)));
    assert!(!back.outdated);
}

#[test]
fn a_comment_on_a_deleted_file_is_outdated() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 2, "gone soon")));
    std::fs::remove_file(dir.path().join("f.txt")).unwrap();
    let comment = only(&mut engine);
    assert!(comment.outdated);
    assert_eq!(comment.location, None);
}

#[test]
fn comments_survive_opening_the_engine_again() {
    let dir = repo();
    let log = CommandLog::default();
    {
        let mut engine = engine_on(dir.path(), &log);
        engine.handle(OperationId(10), &human(add("f.txt", 4, 4, "keep me")));
    }
    let mut engine = engine_on(dir.path(), &log);
    let comment = only(&mut engine);
    assert_eq!(comment.body, "keep me");
    assert_eq!(comment.anchor.text, "d");
    assert!(comment.snapshot.is_some());
}

#[test]
fn a_comment_on_another_repository_is_not_listed_here() {
    let first = repo();
    let second = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(first.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 1, "first only")));
    let mut other = engine_on(second.path(), &log);
    assert!(list(&mut other).is_empty());
    assert_eq!(only(&mut engine).body, "first only");
}

#[test]
fn a_snapshot_changes_nothing_the_user_can_see() {
    let dir = repo();
    write(dir.path(), "f.txt", "a\nB\nc\nd\ne\n");
    write(dir.path(), "untracked.txt", "new file\n");
    write(dir.path(), ".gitignore", "*.log\n");
    write(dir.path(), "build.log", "ignored\n");
    git(dir.path(), &["add", ".gitignore"]);
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    // Git itself may refresh the index on its first read; the baseline is taken after it.
    git(dir.path(), &["status", "--porcelain"]);
    let status = git(dir.path(), &["status", "--porcelain"]);
    let index = index_bytes(dir.path());
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let refs = public_refs(dir.path());
    let tmp = dir.path().join(".git").join("gitmaster").join("tmp");

    let events = engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "change this")));
    assert!(done(&events), "{}", failure(&events));

    assert_eq!(
        index_bytes(dir.path()),
        index,
        "the real index is not written"
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), status);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(public_refs(dir.path()), refs);
    assert_eq!(read(dir.path(), "f.txt"), "a\nB\nc\nd\ne\n");
    assert_eq!(read(dir.path(), "untracked.txt"), "new file\n");
    assert_eq!(read(dir.path(), "build.log"), "ignored\n");
    let leftovers = std::fs::read_dir(&tmp).unwrap().count();
    assert_eq!(leftovers, 0, "the snapshot folder is removed");
}

#[test]
fn a_snapshot_holds_the_working_tree_with_untracked_files_and_not_ignored_ones() {
    let dir = repo();
    write(dir.path(), "f.txt", "changed\n");
    write(dir.path(), "new.txt", "untracked\n");
    write(dir.path(), ".gitignore", "*.log\n");
    write(dir.path(), "build.log", "ignored\n");
    let oid = port().snapshot_worktree(dir.path()).unwrap();

    let files = git(dir.path(), &["ls-tree", "-r", "--name-only", &oid]);
    assert!(files.lines().any(|line| line == "f.txt"));
    assert!(files.lines().any(|line| line == "new.txt"), "{files}");
    assert!(files.lines().any(|line| line == ".gitignore"));
    assert!(!files.lines().any(|line| line == "build.log"), "{files}");
    assert_eq!(
        git(dir.path(), &["show", &format!("{oid}:f.txt")]),
        "changed\n"
    );
    let parent = git(dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(git(dir.path(), &["rev-parse", &format!("{oid}^")]), parent);
    assert!(
        git(dir.path(), &["log", "-1", "--format=%an <%ae>|%s", &oid])
            .contains("GitMaster <gitmaster@localhost>|gitmaster review snapshot")
    );
}

#[test]
fn a_snapshot_of_an_unborn_head_has_no_parent_and_creates_no_index() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    write(dir.path(), "first.txt", "hello\n");
    assert!(!dir.path().join(".git").join("index").exists());

    let oid = port().snapshot_worktree(dir.path()).unwrap();

    let commit = git(dir.path(), &["cat-file", "-p", &oid]);
    assert!(
        !commit.lines().any(|line| line.starts_with("parent ")),
        "{commit}"
    );
    assert_eq!(
        git(dir.path(), &["ls-tree", "--name-only", &oid]),
        "first.txt\n"
    );
    assert!(
        !dir.path().join(".git").join("index").exists(),
        "no index is written"
    );
    assert!(
        !git_output(dir.path(), &["rev-parse", "--verify", "-q", "HEAD"])
            .status
            .success()
    );
}

#[test]
fn a_linked_worktree_shares_the_private_ref_and_keeps_its_own_index() {
    let dir = repo();
    let parent = tempfile::tempdir().unwrap();
    let linked: PathBuf = parent.path().join("linked");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    write(&linked, "only-here.txt", "in the linked worktree\n");
    let main_index = index_bytes(dir.path());
    let linked_index = std::fs::read(
        dir.path()
            .join(".git")
            .join("worktrees")
            .join("linked")
            .join("index"),
    );

    let oid = port().snapshot_worktree(&linked).unwrap();
    port().set_review_ref(&linked, 7, &oid).unwrap();

    assert!(git(dir.path(), &["ls-tree", "-r", "--name-only", &oid]).contains("only-here.txt"));
    assert_eq!(
        git(
            dir.path(),
            &["for-each-ref", "--format=%(refname)", "refs/gitmaster/"]
        ),
        "refs/gitmaster/review/7/before\n",
        "the ref lives in the shared repository"
    );
    assert_eq!(index_bytes(dir.path()), main_index);
    assert_eq!(
        std::fs::read(
            dir.path()
                .join(".git")
                .join("worktrees")
                .join("linked")
                .join("index")
        )
        .ok(),
        linked_index.ok()
    );
    assert!(
        !dir.path()
            .join(".git")
            .join("gitmaster")
            .join("tmp")
            .read_dir()
            .unwrap()
            .any(|_| true)
    );
    port().delete_review_ref(&linked, 7).unwrap();
    assert!(private_refs(dir.path()).is_empty());
}

#[test]
fn the_snapshot_text_is_the_old_text_after_the_file_was_edited() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 3, "about b and c")));
    let id = only(&mut engine).id;

    write(dir.path(), "f.txt", "new top\na\nB\nC\nd\ne\n");
    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::LoadReviewSnapshotText { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    let (path, lines) = snapshot_text(&events);
    assert_eq!(path, "f.txt");
    assert_eq!(lines, vec!["b", "c"]);
    assert!(only(&mut engine).outdated);
}

#[test]
fn an_agent_is_refused_every_write_and_can_still_list() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 1, "mine")));
    let before = only(&mut engine);

    let refused = [
        (
            agent(add("f.txt", 2, 2, "agent comment")),
            "only a human can comment on the code",
        ),
        (
            agent(CommandKind::EditReviewComment {
                id: before.id,
                body: "agent edit".to_owned(),
            }),
            "only a human can edit a review comment",
        ),
        (
            agent(CommandKind::ResolveReviewComment { id: before.id }),
            "only a human can resolve a review comment",
        ),
        (
            agent(CommandKind::ReopenReviewComment { id: before.id }),
            "only a human can reopen a review comment",
        ),
        (
            agent(CommandKind::DeleteReviewComment { id: before.id }),
            "only a human can delete a review comment",
        ),
    ];
    for (command, message) in refused {
        let events = engine.handle(OperationId(20), &command);
        assert_eq!(failure(&events), message);
    }

    let events = engine.handle(OperationId(30), &agent(CommandKind::ListReviewComments));
    assert!(done(&events));
    assert_eq!(listed(&events), vec![before]);
}

#[test]
fn a_comment_is_saved_without_a_snapshot_when_the_snapshot_fails() {
    let dir = repo();
    // The snapshot folder cannot be made when a file takes its place.
    let gitmaster = dir.path().join(".git").join("gitmaster");
    std::fs::create_dir_all(&gitmaster).unwrap();
    write(&gitmaster, "tmp", "not a folder");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(OperationId(10), &human(add("f.txt", 5, 5, "no snapshot")));
    assert!(done(&events), "{}", failure(&events));
    let comment = only(&mut engine);
    assert_eq!(comment.snapshot, None);
    assert_eq!(comment.location, Some(location(5, 5)));
    assert!(private_refs(dir.path()).is_empty());

    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::LoadReviewSnapshotText { id: comment.id }),
    );
    assert!(snapshot_text(&events).1.is_empty());

    let events = engine.handle(
        OperationId(12),
        &human(CommandKind::DeleteReviewComment { id: comment.id }),
    );
    assert!(done(&events), "{}", failure(&events));
}

#[test]
fn a_comment_that_cannot_be_anchored_is_refused_and_not_saved() {
    let dir = repo();
    write(dir.path(), "blank.txt", "\n   \nx\n");
    std::fs::write(dir.path().join("latin1.txt"), [0x66, 0xff, 0x0a]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("f.txt"), dir.path().join("link.txt")).unwrap();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let refused = [
        (
            add("../outside.txt", 1, 1, "x"),
            "not a path inside the worktree",
        ),
        (
            add("/etc/hostname", 1, 1, "x"),
            "not a path inside the worktree",
        ),
        (
            add("missing.txt", 1, 1, "x"),
            "there is no file missing.txt in the worktree",
        ),
        (add("f.txt", 1, 1, "   "), "the comment is empty"),
        (
            add("f.txt", 0, 1, "x"),
            "lines 0-1 are not in the file, which has 5 lines",
        ),
        (
            add("f.txt", 4, 9, "x"),
            "lines 4-9 are not in the file, which has 5 lines",
        ),
        (
            add("blank.txt", 1, 2, "x"),
            "the selection has only blank lines",
        ),
        (add("latin1.txt", 1, 1, "x"), "latin1.txt is not UTF-8 text"),
    ];
    for (kind, message) in refused {
        let events = engine.handle(OperationId(10), &human(kind));
        assert!(
            failure(&events).starts_with(message),
            "{:?} vs {message}",
            failure(&events)
        );
    }
    #[cfg(unix)]
    {
        let events = engine.handle(OperationId(11), &human(add("link.txt", 1, 1, "x")));
        assert_eq!(failure(&events), "link.txt is not a regular file");
    }
    assert!(list(&mut engine).is_empty());
}

#[test]
fn reads_are_not_recorded_and_the_writes_reach_the_command_log() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let before = engine.log().len();

    list(&mut engine);
    engine.handle(
        OperationId(10),
        &human(CommandKind::LoadReviewSnapshotText { id: 1 }),
    );
    assert_eq!(engine.log().len(), before, "reads leave no entry");

    engine.handle(OperationId(11), &human(add("f.txt", 1, 1, "logged")));
    let kinds: Vec<&CommandKind> = engine
        .log()
        .iter()
        .map(|entry| &entry.kind)
        .filter(|kind| matches!(kind, CommandKind::AddReviewComment { .. }))
        .collect();
    assert!(!kinds.is_empty(), "the write is logged under its operation");

    let records = log.entries();
    assert!(records.iter().any(|record| {
        record.args.first().map(String::as_str) == Some("update-ref")
            && record
                .args
                .iter()
                .any(|arg| arg.starts_with("refs/gitmaster/review/"))
    }));
    assert!(
        records
            .iter()
            .any(|record| record.args.iter().any(|arg| arg == "commit-tree"))
    );
    assert!(!records.iter().any(|record| {
        record
            .args
            .iter()
            .any(|arg| arg == "ls-tree" || arg == "cat-file")
    }));
}
