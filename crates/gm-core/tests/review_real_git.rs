//! Review comments (B4, D96, D97) against real repositories in temporary
//! directories: the comment commands through the engine, the snapshots through
//! the adapter, and what the snapshots leave untouched. Each test removes its
//! directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_core::{Engine, GitPort, ReviewSide};
use gm_schema::{
    Actor, Command, CommandKind, Event, OperationId, OperationState, ReviewAuthorFilter,
    ReviewChange, ReviewComment, ReviewKind, ReviewLocation, ReviewSeverity, ReviewStatus,
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
        label: None,
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

fn current_text(events: &[Event]) -> (String, Vec<String>) {
    events
        .iter()
        .find_map(|e| match e {
            Event::ReviewCurrentText { path, lines, .. } => Some((path.clone(), lines.clone())),
            _ => None,
        })
        .expect("a current text")
}

/// Lists the comments through the engine: the list is a read, not a write.
fn list(engine: &mut Engine) -> Vec<ReviewComment> {
    let events = engine.handle(
        OperationId(900),
        &human(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        }),
    );
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
fn an_agent_marks_a_comment_handled_and_only_a_human_resolves_it() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "rename")));
    let id = only(&mut engine).id;

    let events = engine.handle(
        OperationId(11),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: "  renamed b to bee  ".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let handled = listed(&events).remove(0);
    assert_eq!(handled.status, ReviewStatus::Handled);
    assert_eq!(handled.replies.len(), 1);
    assert_eq!(handled.replies[0].author, "agent:test-agent");
    assert_eq!(handled.replies[0].body, "renamed b to bee");

    let events = engine.handle(
        OperationId(12),
        &agent(CommandKind::ResolveReviewComment { id }),
    );
    assert_eq!(
        failure(&events),
        "only a human can resolve a review comment"
    );
    let events = engine.handle(
        OperationId(13),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: " ".to_owned(),
        }),
    );
    assert_eq!(failure(&events), "the summary is empty");

    let events = engine.handle(
        OperationId(14),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    let resolved = listed(&events).remove(0);
    assert_eq!(resolved.status, ReviewStatus::Resolved);
    assert_eq!(resolved.replies[0].body, "renamed b to bee");
    let events = engine.handle(
        OperationId(15),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: "again".to_owned(),
        }),
    );
    assert_eq!(failure(&events), "the comment is already resolved");

    let events = engine.handle(
        OperationId(16),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    let reopened = listed(&events).remove(0);
    assert_eq!(reopened.status, ReviewStatus::Open);
    assert_eq!(reopened.replies.len(), 1, "reopening keeps the replies");
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
    assert_eq!(failure(&events), "the comment is already open");

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
    port()
        .set_review_ref(&linked, 7, ReviewSide::Before, &oid)
        .unwrap();

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
    port()
        .delete_review_ref(&linked, 7, ReviewSide::Before)
        .unwrap();
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
fn the_current_text_follows_the_comment_and_is_empty_when_it_is_outdated() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 3, "about b and c")));
    let id = only(&mut engine).id;
    let read = |engine: &mut Engine, op: u64| {
        engine.handle(
            OperationId(op),
            &human(CommandKind::LoadReviewCurrentText { id }),
        )
    };
    let words = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| (*line).to_owned())
            .collect::<Vec<_>>()
    };

    let events = read(&mut engine, 11);
    assert!(done(&events), "{}", failure(&events));
    assert_eq!(
        current_text(&events),
        ("f.txt".to_owned(), words(&["b", "c"]))
    );

    write(dir.path(), "f.txt", "x\na\nb\nc\nd\ne\n");
    let moved = read(&mut engine, 12);
    assert_eq!(
        current_text(&moved).1,
        words(&["b", "c"]),
        "found where it moved"
    );

    write(dir.path(), "f.txt", "a\nB\nC\nd\ne\n");
    let edited = read(&mut engine, 13);
    assert!(done(&edited));
    assert!(current_text(&edited).1.is_empty(), "outdated is empty");

    write(dir.path(), "f.txt", "a\nb\nc\nd\ne\n");
    std::fs::remove_file(dir.path().join("f.txt")).unwrap();
    let gone = read(&mut engine, 14);
    assert!(done(&gone));
    assert!(current_text(&gone).1.is_empty(), "a missing file is empty");
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

    let events = engine.handle(
        OperationId(30),
        &agent(CommandKind::ListReviewComments {
            author: ReviewAuthorFilter::Any,
        }),
    );
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
    engine.handle(
        OperationId(10),
        &human(CommandKind::LoadReviewCurrentText { id: 1 }),
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

/// Marks comment `id` handled as the agent, which takes the "after" snapshot (D140).
fn handle(engine: &mut Engine, id: i64, summary: &str) -> ReviewComment {
    let events = engine.handle(
        OperationId(40),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: summary.to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    listed(&events)
        .into_iter()
        .find(|comment| comment.id == id)
        .expect("the handled comment")
}

/// The change of comment `id`, read by the agent: it may run the read.
fn change(engine: &mut Engine, id: i64) -> Option<ReviewChange> {
    let events = engine.handle(
        OperationId(41),
        &agent(CommandKind::LoadReviewChange { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    events
        .iter()
        .find_map(|e| match e {
            Event::ReviewChange {
                comment,
                path,
                change,
                ..
            } => {
                assert_eq!(*comment, id);
                assert_eq!(path, "f.txt");
                Some(change.clone())
            }
            _ => None,
        })
        .expect("a review change")
}

fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

fn after_ref(id: i64) -> String {
    format!("refs/gitmaster/review/{id}/after")
}

fn before_ref(id: i64) -> String {
    format!("refs/gitmaster/review/{id}/before")
}

#[test]
fn handling_a_comment_takes_an_after_snapshot_under_its_own_ref() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 3, "about b and c")));
    let id = only(&mut engine).id;
    assert_eq!(change(&mut engine, id), None, "no after snapshot yet");

    write(dir.path(), "f.txt", "a\nB\nc\nd\ne\n");
    let handled = handle(&mut engine, id, "capitalised b");
    let after = handled.after_snapshot.clone().expect("an after snapshot");
    assert_ne!(Some(&after), handled.snapshot.as_ref());
    assert_eq!(
        private_refs(dir.path()),
        vec![after_ref(id), before_ref(id)]
    );
    assert_eq!(
        git(dir.path(), &["rev-parse", &after_ref(id)]).trim(),
        after
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{after}:f.txt")]),
        "a\nB\nc\nd\ne\n"
    );
    assert_eq!(read(dir.path(), "f.txt"), "a\nB\nc\nd\ne\n");
    assert_eq!(only(&mut engine).after_snapshot, Some(after));
}

/// Git's racy-index case: a file rewritten with the same size in the same timestamp tick as the
/// index entry that recorded it still matches that entry's stat data. Git's racy rule (an entry
/// at or after the index file's own time is checked by content) is what catches it, and the
/// snapshot's copy of the index must keep that time. `core.checkStat=minimal` drops the
/// sub-second part of ctime, so a rewrite in the same second keeps the stat data Git compares.
#[test]
fn a_snapshot_sees_a_same_size_rewrite_in_the_same_timestamp_tick() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "t"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    git(dir.path(), &["config", "core.checkStat", "minimal"]);
    write(dir.path(), "f.txt", "a\nb\nc\nd\ne\n");
    git(dir.path(), &["add", "f.txt"]);
    let tick = std::fs::metadata(dir.path().join("f.txt"))
        .and_then(|meta| meta.modified())
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(dir.path().join(".git").join("index"))
        .unwrap()
        .set_modified(tick)
        .unwrap();
    write(dir.path(), "f.txt", "a\nB\nc\nd\ne\n");
    std::fs::File::options()
        .write(true)
        .open(dir.path().join("f.txt"))
        .unwrap()
        .set_modified(tick)
        .unwrap();

    let oid = port().snapshot_worktree(dir.path()).expect("snapshot");
    assert_eq!(
        git(dir.path(), &["show", &format!("{oid}:f.txt")]),
        "a\nB\nc\nd\ne\n"
    );
}

#[test]
fn an_edit_inside_the_region_shows_as_a_change() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 3, "about b and c")));
    let id = only(&mut engine).id;

    write(dir.path(), "f.txt", "a\nB\nc\nd\ne\n");
    handle(&mut engine, id, "capitalised b");
    // A later edit of the working tree does not change what was observed.
    write(dir.path(), "f.txt", "zzz\n");
    assert_eq!(
        change(&mut engine, id),
        Some(ReviewChange {
            before: strings(&["b", "c"]),
            after: strings(&["B", "c"]),
            changed: true,
            before_start_line: 2,
            after_start_line: 2,
        })
    );
}

#[test]
fn a_comment_handled_without_an_edit_is_unchanged() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 3, "about b and c")));
    let id = only(&mut engine).id;
    // An edit away from the region is not a change of it.
    write(dir.path(), "f.txt", "a\nb\nc\nd\nE\n");
    handle(&mut engine, id, "nothing to do");
    assert_eq!(
        change(&mut engine, id),
        Some(ReviewChange {
            before: strings(&["b", "c"]),
            after: strings(&["b", "c"]),
            changed: false,
            before_start_line: 2,
            after_start_line: 2,
        })
    );
}

#[test]
fn an_edit_above_the_region_moves_it_and_keeps_its_text() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 4, 5, "about d and e")));
    let id = only(&mut engine).id;
    write(dir.path(), "f.txt", "x\ny\nz\na\nb\nc\nd\nE2\n");
    handle(&mut engine, id, "renamed e");
    assert_eq!(
        change(&mut engine, id),
        Some(ReviewChange {
            before: strings(&["d", "e"]),
            after: strings(&["d", "E2"]),
            changed: true,
            before_start_line: 4,
            after_start_line: 7,
        })
    );

    // Lines inserted above, and none touching it: the same lines, moved.
    engine.handle(OperationId(11), &human(add("f.txt", 7, 7, "about d")));
    let second = list(&mut engine)
        .into_iter()
        .find(|comment| comment.anchor.text == "d" && comment.id != id)
        .expect("the second comment")
        .id;
    write(dir.path(), "f.txt", "top\nx\ny\nz\na\nb\nc\nd\nE2\n");
    let moved = handle(&mut engine, second, "nothing");
    assert!(moved.after_snapshot.is_some());
    assert_eq!(
        change(&mut engine, second),
        Some(ReviewChange {
            before: strings(&["d"]),
            after: strings(&["d"]),
            changed: false,
            before_start_line: 7,
            after_start_line: 8,
        })
    );
}

#[test]
fn a_file_gone_from_the_after_snapshot_has_an_empty_after_side() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "about b")));
    let id = only(&mut engine).id;
    std::fs::remove_file(dir.path().join("f.txt")).unwrap();
    handle(&mut engine, id, "removed the file");
    assert_eq!(
        change(&mut engine, id),
        Some(ReviewChange {
            before: strings(&["b"]),
            after: Vec::new(),
            changed: true,
            before_start_line: 2,
            after_start_line: 1,
        })
    );
}

#[test]
fn handling_again_replaces_the_after_snapshot() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "about c")));
    let id = only(&mut engine).id;
    write(dir.path(), "f.txt", "a\nb\nC\nd\ne\n");
    let first = handle(&mut engine, id, "first").after_snapshot.unwrap();
    write(dir.path(), "f.txt", "a\nb\nCC\nd\ne\n");
    let second = handle(&mut engine, id, "second").after_snapshot.unwrap();
    assert_ne!(first, second);
    assert_eq!(
        git(dir.path(), &["rev-parse", &after_ref(id)]).trim(),
        second
    );
    assert_eq!(change(&mut engine, id).unwrap().after, strings(&["CC"]));
}

#[test]
fn reopening_drops_the_after_snapshot_and_its_ref_and_deleting_drops_both_refs() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "about c")));
    let id = only(&mut engine).id;
    write(dir.path(), "f.txt", "a\nb\nC\nd\ne\n");
    handle(&mut engine, id, "done");

    // Resolving keeps it: the human resolves what the agent did.
    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    assert!(listed(&events)[0].after_snapshot.is_some());
    let events = engine.handle(
        OperationId(12),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    let reopened = listed(&events).remove(0);
    assert_eq!(reopened.after_snapshot, None);
    assert_eq!(reopened.replies.len(), 1, "reopening keeps the replies");
    assert!(reopened.snapshot.is_some());
    assert_eq!(private_refs(dir.path()), vec![before_ref(id)]);
    assert_eq!(change(&mut engine, id), None);

    handle(&mut engine, id, "again");
    assert_eq!(
        private_refs(dir.path()),
        vec![after_ref(id), before_ref(id)]
    );
    let events = engine.handle(
        OperationId(13),
        &human(CommandKind::DeleteReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    assert!(private_refs(dir.path()).is_empty());
}

#[test]
fn a_comment_is_handled_without_an_after_when_the_snapshot_fails() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "about c")));
    let id = only(&mut engine).id;
    handle(&mut engine, id, "first");
    assert!(private_refs(dir.path()).contains(&after_ref(id)));

    // The snapshot folder cannot be made when a file takes its place.
    let gitmaster = dir.path().join(".git").join("gitmaster");
    std::fs::remove_dir_all(gitmaster.join("tmp")).unwrap();
    write(&gitmaster, "tmp", "not a folder");
    let handled = handle(&mut engine, id, "second");
    assert_eq!(handled.status, ReviewStatus::Handled);
    assert_eq!(
        handled.replies.last().map(|r| r.body.as_str()),
        Some("second")
    );
    assert_eq!(handled.after_snapshot, None);
    assert_eq!(private_refs(dir.path()), vec![before_ref(id)]);
    assert_eq!(change(&mut engine, id), None);
}

#[test]
fn reading_the_change_is_not_recorded() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "about c")));
    let id = only(&mut engine).id;
    handle(&mut engine, id, "done");
    let before = engine.log().len();
    let records = log.entries().len();
    change(&mut engine, id);
    assert_eq!(engine.log().len(), before, "a read leaves no entry");
    assert_eq!(log.entries().len(), records, "and runs no logged git");
}

fn add_by_text(path: &str, text: &str) -> CommandKind {
    CommandKind::AddReviewCommentByText {
        label: None,
        path: path.to_owned(),
        text: text.to_owned(),
        body: "by text".to_owned(),
    }
}

#[test]
fn a_comment_by_text_is_anchored_where_the_text_is_once_and_refused_otherwise() {
    let dir = repo();
    write(dir.path(), "g.txt", "x\ny\nx\nz\n");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let events = engine.handle(OperationId(10), &human(add_by_text("f.txt", "c\nd")));
    assert!(done(&events), "{}", failure(&events));
    let comment = only(&mut engine);
    assert_eq!(comment.anchor.text, "c\nd");
    assert_eq!(comment.location, Some(location(3, 4)));

    // Twice in the file: refused, and nothing is written.
    let events = engine.handle(OperationId(11), &human(add_by_text("g.txt", "x")));
    assert!(!done(&events));
    assert!(
        failure(&events).contains("more than once"),
        "{}",
        failure(&events)
    );
    // Not in the file now: refused.
    let events = engine.handle(OperationId(12), &human(add_by_text("f.txt", "gone")));
    assert!(
        failure(&events).contains("not in the file"),
        "{}",
        failure(&events)
    );
    assert_eq!(list(&mut engine).len(), 1);

    // An agent may not write comments.
    let events = engine.handle(OperationId(13), &agent(add_by_text("f.txt", "a")));
    assert!(!done(&events));
    assert_eq!(list(&mut engine).len(), 1);
}

/// A workspace on `main` whose member `billing` is a linked worktree of the repository
/// `member` on `feat` (D180, D182): the workspace's index holds the gitlink, and the folder
/// has a `.git` file. Returns the two repositories, which stay alive with the test.
fn workspace() -> (tempfile::TempDir, tempfile::TempDir) {
    let member = tempfile::tempdir().unwrap();
    git(member.path(), &["init", "-q", "-b", "main"]);
    git(member.path(), &["config", "user.name", "t"]);
    git(member.path(), &["config", "user.email", "t@t"]);
    std::fs::create_dir_all(member.path().join("src")).unwrap();
    write(member.path(), "src/x.rs", "one\ntwo\nthree\nfour\n");
    write(member.path(), "run.sh", "#!/bin/sh\n");
    git(member.path(), &["add", "."]);
    git(member.path(), &["commit", "-q", "-m", "init"]);

    let ws = tempfile::tempdir().unwrap();
    git(ws.path(), &["init", "-q", "-b", "main"]);
    write(ws.path(), "README.md", "workspace\n");
    git(ws.path(), &["add", "."]);
    git(ws.path(), &["commit", "-q", "-m", "init"]);

    let billing = ws.path().join("billing");
    git(
        member.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            billing.to_str().unwrap(),
        ],
    );
    write(
        ws.path(),
        ".gitmodules",
        &format!(
            "[submodule \"billing\"]\n\tpath = billing\n\turl = {}\n",
            member.path().display()
        ),
    );
    let head = git(&billing, &["rev-parse", "HEAD"]).trim().to_owned();
    git(ws.path(), &["add", ".gitmodules"]);
    git(
        ws.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},billing"),
        ],
    );
    git(ws.path(), &["commit", "-q", "-m", "add billing"]);
    (ws, member)
}

/// The index of the member's linked worktree `billing` (in the member's `.git`) and its time.
fn member_index(member: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let path = member
        .join(".git")
        .join("worktrees")
        .join("billing")
        .join("index");
    let bytes = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    (bytes, modified)
}

/// The change of comment `id` on the member file `billing/src/x.rs`, read by the agent.
fn member_change(engine: &mut Engine, id: i64) -> Option<ReviewChange> {
    let events = engine.handle(
        OperationId(41),
        &agent(CommandKind::LoadReviewChange { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    events
        .iter()
        .find_map(|e| match e {
            Event::ReviewChange { path, change, .. } => {
                assert_eq!(path, "billing/src/x.rs");
                Some(change.clone())
            }
            _ => None,
        })
        .expect("a review change")
}

#[test]
fn a_comment_on_a_members_file_keeps_its_text_before_and_after_the_edit() {
    let (ws, member) = workspace();
    let log = CommandLog::default();
    let mut engine = engine_on(ws.path(), &log);
    let ws_index = index_bytes(ws.path());
    let member_refs = public_refs(member.path());
    let member_state = member_index(member.path());

    let events = engine.handle(
        OperationId(10),
        &human(add("billing/src/x.rs", 2, 3, "about two and three")),
    );
    assert!(done(&events), "{}", failure(&events));
    let id = only(&mut engine).id;
    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::LoadReviewSnapshotText { id }),
    );
    let (path, lines) = snapshot_text(&events);
    assert_eq!(path, "billing/src/x.rs");
    assert_eq!(
        lines,
        vec!["two", "three"],
        "the before text of the member file"
    );

    // The before snapshot holds the member's file under its workspace path, not a gitlink.
    assert!(private_refs(ws.path()).contains(&before_ref(id)));
    let oid = git(ws.path(), &["rev-parse", &before_ref(id)]);
    let oid = oid.trim();
    let folder = git(ws.path(), &["ls-tree", oid, "billing"]);
    assert!(folder.starts_with("040000 tree"), "{folder}");
    assert!(git(ws.path(), &["ls-tree", "-r", oid, "billing/src/x.rs"]).starts_with("100644 blob"));

    write(
        &ws.path().join("billing").join("src"),
        "x.rs",
        "one\nTWO\nthree\nfour\n",
    );
    let events = engine.handle(
        OperationId(12),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: "renamed two".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));

    let change = member_change(&mut engine, id).expect("a change of the member file");
    assert!(change.changed, "{change:?}");
    assert_eq!(change.before, vec!["two", "three"]);
    assert_eq!(change.after, vec!["TWO", "three"]);

    // The member's refs and index, and the workspace's real index, are as they were.
    assert_eq!(public_refs(member.path()), member_refs);
    assert_eq!(member_index(member.path()), member_state);
    assert_eq!(index_bytes(ws.path()), ws_index);
    assert_eq!(private_refs(ws.path()).len(), 2, "both snapshots are kept");
}

#[test]
#[cfg(unix)]
fn a_member_snapshot_keeps_the_mode_of_a_file_and_a_symlink_and_drops_a_deleted_one() {
    use std::os::unix::fs::PermissionsExt;

    let (ws, _member) = workspace();
    let billing = ws.path().join("billing");
    let meta = std::fs::metadata(billing.join("run.sh")).unwrap();
    std::fs::set_permissions(
        billing.join("run.sh"),
        std::fs::Permissions::from_mode(meta.permissions().mode() | 0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("src/x.rs", billing.join("link")).unwrap();
    let ws_index = index_bytes(ws.path());

    let oid = port()
        .snapshot_worktree_for(ws.path(), "billing/run.sh")
        .unwrap();
    assert!(
        git(ws.path(), &["ls-tree", &oid, "billing/run.sh"]).starts_with("100755 blob"),
        "an executable file keeps its mode"
    );
    let oid = port()
        .snapshot_worktree_for(ws.path(), "billing/link")
        .unwrap();
    assert!(git(ws.path(), &["ls-tree", &oid, "billing/link"]).starts_with("120000 blob"));
    assert_eq!(
        git(
            ws.path(),
            &["cat-file", "blob", &format!("{oid}:billing/link")]
        ),
        "src/x.rs"
    );

    std::fs::remove_file(billing.join("src").join("x.rs")).unwrap();
    let oid = port()
        .snapshot_worktree_for(ws.path(), "billing/src/x.rs")
        .unwrap();
    assert!(
        git(ws.path(), &["ls-tree", "-r", &oid, "billing/src/x.rs"]).is_empty(),
        "a deleted file has no entry"
    );
    assert_eq!(
        index_bytes(ws.path()),
        ws_index,
        "the real index is untouched"
    );
}

#[test]
fn a_snapshot_of_a_plain_file_is_the_same_tree_as_the_whole_worktree() {
    let dir = repo();
    write(dir.path(), "f.txt", "changed\n");
    let whole = port().snapshot_worktree(dir.path()).unwrap();
    let for_path = port().snapshot_worktree_for(dir.path(), "f.txt").unwrap();
    assert_eq!(
        git(dir.path(), &["rev-parse", &format!("{whole}^{{tree}}")]),
        git(dir.path(), &["rev-parse", &format!("{for_path}^{{tree}}")]),
        "a repository without members is snapshotted as before"
    );
}

/// The root comment `id` in a listing answer.
fn root_listed(events: &[Event], id: i64) -> ReviewComment {
    listed(events)
        .into_iter()
        .find(|comment| comment.id == id)
        .expect("the comment in the listing")
}

/// A reply by the person to comment `id`.
fn reply_as_human(engine: &mut Engine, id: i64, body: &str) -> Vec<Event> {
    engine.handle(
        OperationId(50),
        &human(CommandKind::ReplyToReviewComment {
            id,
            body: body.to_owned(),
        }),
    )
}

#[test]
fn a_handled_comment_gets_one_reply_per_handle_and_the_latest_after_snapshot() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "rename b")));
    let id = only(&mut engine).id;

    write(dir.path(), "f.txt", "a\nB\nc\nd\ne\n");
    handle(&mut engine, id, "first");
    let first_after = only(&mut engine).after_snapshot.expect("an after snapshot");

    write(dir.path(), "f.txt", "a\nBee\nc\nd\ne\n");
    let events = engine.handle(
        OperationId(20),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: "second".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let handled = root_listed(&events, id);
    assert_eq!(handled.status, ReviewStatus::Handled);
    let bodies: Vec<&str> = handled.replies.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["first", "second"]);
    assert!(
        handled
            .replies
            .iter()
            .all(|reply| reply.author == "agent:test-agent")
    );
    let second_after = handled
        .after_snapshot
        .clone()
        .expect("a new after snapshot");
    assert_ne!(second_after, first_after);
    assert_eq!(
        private_refs(dir.path()),
        vec![after_ref(id), before_ref(id)]
    );
    assert_eq!(
        git(dir.path(), &["rev-parse", &after_ref(id)]).trim(),
        second_after
    );
}

#[test]
fn an_agent_reply_keeps_the_status_and_a_person_reply_reopens_a_handled_comment() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 3, 3, "about c")));
    let id = only(&mut engine).id;
    write(dir.path(), "f.txt", "a\nb\nC\nd\ne\n");
    handle(&mut engine, id, "done");

    let events = engine.handle(
        OperationId(21),
        &agent(CommandKind::ReplyToReviewComment {
            id,
            body: "  one more thing  ".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let kept = root_listed(&events, id);
    assert_eq!(
        kept.status,
        ReviewStatus::Handled,
        "an agent reply keeps the status"
    );
    assert!(kept.after_snapshot.is_some());
    assert_eq!(
        kept.replies.last().map(|r| r.body.as_str()),
        Some("one more thing")
    );

    let events = engine.handle(
        OperationId(22),
        &human(CommandKind::ReplyToReviewComment {
            id,
            body: "no, again".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let reopened = root_listed(&events, id);
    assert_eq!(reopened.status, ReviewStatus::Open);
    assert_eq!(reopened.after_snapshot, None);
    assert_eq!(private_refs(dir.path()), vec![before_ref(id)]);
    let bodies: Vec<&str> = reopened.replies.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["done", "one more thing", "no, again"]);
    let authors: Vec<&str> = reopened.replies.iter().map(|r| r.author.as_str()).collect();
    assert_eq!(authors, ["agent:test-agent", "agent:test-agent", "human"]);

    let listing = list(&mut engine);
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].status, ReviewStatus::Open);
    assert_eq!(listing[0].replies, reopened.replies);
}

#[test]
fn a_reply_to_a_reply_attaches_to_its_root_and_the_root_keeps_its_status() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 1, "about a")));
    let id = only(&mut engine).id;
    let events = reply_as_human(&mut engine, id, "first");
    assert!(done(&events), "{}", failure(&events));
    let first = root_listed(&events, id).replies[0].id;
    assert_ne!(first, id, "a reply has an id of its own");

    let events = engine.handle(
        OperationId(23),
        &agent(CommandKind::ReplyToReviewComment {
            id: first,
            body: "answer to the reply".to_owned(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let listing = listed(&events);
    assert_eq!(listing.len(), 1, "a reply is not a comment of its own");
    assert_eq!(listing[0].id, id);
    assert_eq!(listing[0].status, ReviewStatus::Open);
    let bodies: Vec<&str> = listing[0].replies.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["first", "answer to the reply"]);
}

#[test]
fn a_reply_is_refused_on_a_resolved_comment_an_empty_body_and_an_unknown_id() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "rename b")));
    let id = only(&mut engine).id;

    let events = engine.handle(
        OperationId(11),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    let events = reply_as_human(&mut engine, id, "late");
    assert_eq!(failure(&events), "the comment is already resolved");
    let events = engine.handle(
        OperationId(12),
        &agent(CommandKind::ReplyToReviewComment {
            id,
            body: "late".to_owned(),
        }),
    );
    assert_eq!(failure(&events), "the comment is already resolved");
    assert!(only(&mut engine).replies.is_empty());

    engine.handle(
        OperationId(13),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    let events = reply_as_human(&mut engine, id, "  \n ");
    assert_eq!(failure(&events), "the reply is empty");
    let events = reply_as_human(&mut engine, 999, "x");
    assert_eq!(failure(&events), "no review comment 999");
    assert!(only(&mut engine).replies.is_empty());
}

#[test]
fn a_reply_id_is_not_a_comment_for_the_other_comment_commands() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "rename b")));
    let id = only(&mut engine).id;
    let events = reply_as_human(&mut engine, id, "reply");
    assert!(done(&events), "{}", failure(&events));
    let reply = root_listed(&events, id).replies[0].id;
    let missing = format!("no review comment {reply}");

    let resolve = engine.handle(
        OperationId(11),
        &human(CommandKind::ResolveReviewComment { id: reply }),
    );
    assert_eq!(failure(&resolve), missing);
    let edit = engine.handle(
        OperationId(12),
        &human(CommandKind::EditReviewComment {
            id: reply,
            body: "changed".to_owned(),
        }),
    );
    assert_eq!(failure(&edit), missing);
    let handle = engine.handle(
        OperationId(13),
        &agent(CommandKind::HandleReviewComment {
            id: reply,
            summary: "x".to_owned(),
        }),
    );
    assert_eq!(failure(&handle), missing);
    let delete = engine.handle(
        OperationId(14),
        &human(CommandKind::DeleteReviewComment { id: reply }),
    );
    assert_eq!(failure(&delete), missing);

    let kept = only(&mut engine);
    assert_eq!(kept.status, ReviewStatus::Open);
    assert_eq!(kept.replies.len(), 1);
    assert_eq!(kept.replies[0].body, "reply");

    let events = engine.handle(
        OperationId(15),
        &human(CommandKind::DeleteReviewComment { id }),
    );
    assert!(done(&events), "{}", failure(&events));
    assert!(
        list(&mut engine).is_empty(),
        "the replies go with their comment"
    );
}

#[test]
fn a_reply_is_a_logged_write_and_a_listing_is_not() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 2, 2, "rename b")));
    let id = only(&mut engine).id;
    let before = engine.log().len();
    list(&mut engine);
    assert_eq!(engine.log().len(), before, "a listing leaves no entry");
    reply_as_human(&mut engine, id, "logged");
    let states: Vec<OperationState> = engine
        .log()
        .iter()
        .filter(|entry| matches!(entry.kind, CommandKind::ReplyToReviewComment { .. }))
        .filter(|entry| entry.id == OperationId(50))
        .map(|entry| entry.state)
        .collect();
    assert!(
        states.contains(&OperationState::Done),
        "the reply is in the command log: {states:?}"
    );
}

fn agent_comment(path: &str, anchor_text: &str, kind: ReviewKind) -> CommandKind {
    CommandKind::CreateAgentReviewComment {
        path: path.to_owned(),
        anchor_text: anchor_text.to_owned(),
        kind: Some(kind),
        body: format!("{kind:?} on {anchor_text}"),
        severity: None,
        label: None,
        decorations: Vec::new(),
    }
}

/// The comments the list asks for, through the engine: a read, as `list` is.
fn list_as(engine: &mut Engine, author: ReviewAuthorFilter) -> Vec<ReviewComment> {
    let events = engine.handle(
        OperationId(905),
        &human(CommandKind::ListReviewComments { author }),
    );
    assert!(done(&events), "{}", failure(&events));
    listed(&events)
}

fn reply_as(engine: &mut Engine, actor: Actor, id: i64, body: &str) -> Vec<Event> {
    engine.handle(
        OperationId(906),
        &Command {
            actor,
            kind: CommandKind::ReplyToReviewComment {
                id,
                body: body.to_owned(),
            },
        },
    )
}

#[test]
fn an_agent_starts_a_thread_on_a_unique_anchor_with_a_before_snapshot() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let events = engine.handle(
        OperationId(10),
        &agent(CommandKind::CreateAgentReviewComment {
            path: "f.txt".to_owned(),
            anchor_text: "c".to_owned(),
            kind: Some(ReviewKind::Flag),
            body: "a guess".to_owned(),
            severity: Some(ReviewSeverity::Medium),
            label: None,
            decorations: Vec::new(),
        }),
    );
    assert!(done(&events), "{}", failure(&events));
    let comment = only_any(&mut engine);
    assert_eq!(comment.author.as_deref(), Some("agent:test-agent"));
    assert_eq!(comment.kind, Some(ReviewKind::Flag));
    assert_eq!(comment.severity, Some(ReviewSeverity::Medium));
    assert_eq!(comment.status, ReviewStatus::Open);
    assert_eq!(comment.location, Some(location(3, 3)));
    assert!(
        comment.snapshot.is_some(),
        "a before snapshot, as for a person"
    );
    assert_eq!(private_refs(dir.path()).len(), 1);
    assert!(!comment.needs_answer);

    let person = engine.handle(OperationId(11), &human(add("f.txt", 1, 1, "mine")));
    assert!(done(&person), "{}", failure(&person));
    let refused = engine.handle(
        OperationId(12),
        &human(agent_comment("f.txt", "d", ReviewKind::Note)),
    );
    assert_eq!(
        failure(&refused),
        "only an agent can start a note, question or flag"
    );
}

/// The only comment of the Any listing.
fn only_any(engine: &mut Engine) -> ReviewComment {
    let mut comments = list_as(engine, ReviewAuthorFilter::Any);
    assert_eq!(comments.len(), 1, "one comment: {comments:?}");
    comments.remove(0)
}

#[test]
fn a_missing_or_ambiguous_anchor_is_refused_and_writes_nothing() {
    let dir = repo();
    write(dir.path(), "dup.txt", "x\ny\nx\n");
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let missing = engine.handle(
        OperationId(10),
        &agent(agent_comment("f.txt", "not in the file", ReviewKind::Note)),
    );
    assert_eq!(
        failure(&missing),
        "that text is not in the file as it is now"
    );
    let ambiguous = engine.handle(
        OperationId(11),
        &agent(agent_comment("dup.txt", "x", ReviewKind::Note)),
    );
    assert_eq!(
        failure(&ambiguous),
        "that text appears more than once in the file as it is now"
    );
    let no_file = engine.handle(
        OperationId(12),
        &agent(agent_comment("nope.txt", "x", ReviewKind::Note)),
    );
    assert!(
        failure(&no_file).contains("nope.txt"),
        "{}",
        failure(&no_file)
    );

    assert!(list_as(&mut engine, ReviewAuthorFilter::Any).is_empty());
    assert!(private_refs(dir.path()).is_empty());
    let written = engine
        .log()
        .iter()
        .filter(|entry| entry.state == OperationState::Done)
        .filter(|entry| matches!(entry.kind, CommandKind::CreateAgentReviewComment { .. }))
        .count();
    assert_eq!(written, 0, "no refused thread is logged as done");
}

#[test]
fn an_agent_thread_is_limited_to_five_per_file_and_twenty_per_repository() {
    let dir = repo();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        let text = (1..=6).fold(String::new(), |text, n| {
            text + &format!("{name} line {n}\n")
        });
        write(dir.path(), name, &text);
    }
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);

    let severity_missing = engine.handle(
        OperationId(10),
        &agent(agent_comment("a.txt", "a.txt line 1", ReviewKind::Flag)),
    );
    assert!(failure(&severity_missing).contains("a flag needs a severity"));

    for n in 1..=5 {
        let events = engine.handle(
            OperationId(20 + n),
            &agent(agent_comment(
                "a.txt",
                &format!("a.txt line {n}"),
                ReviewKind::Note,
            )),
        );
        assert!(done(&events), "{}", failure(&events));
    }
    let file_full = engine.handle(
        OperationId(30),
        &agent(agent_comment("a.txt", "a.txt line 6", ReviewKind::Note)),
    );
    assert!(
        failure(&file_full).contains("this file already has 5 open agent threads"),
        "{}",
        failure(&file_full)
    );

    for name in ["b.txt", "c.txt", "d.txt"] {
        for n in 1..=5 {
            let events = engine.handle(
                OperationId(40),
                &agent(agent_comment(
                    name,
                    &format!("{name} line {n}"),
                    ReviewKind::Note,
                )),
            );
            assert!(done(&events), "{}", failure(&events));
        }
    }
    let repo_full = engine.handle(
        OperationId(50),
        &agent(agent_comment("e.txt", "e.txt line 1", ReviewKind::Note)),
    );
    assert!(
        failure(&repo_full).contains("the repository already has 20 open agent threads"),
        "{}",
        failure(&repo_full)
    );
    assert_eq!(list_as(&mut engine, ReviewAuthorFilter::Agent).len(), 20);
}

#[test]
fn an_agent_cannot_hand_off_resolve_or_reopen_its_own_thread_but_a_person_can() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(
        OperationId(10),
        &agent(agent_comment("f.txt", "b", ReviewKind::Note)),
    );
    let id = only_any(&mut engine).id;

    let handed = engine.handle(
        OperationId(11),
        &agent(CommandKind::HandleReviewComment {
            id,
            summary: "done".to_owned(),
        }),
    );
    assert_eq!(
        failure(&handed),
        "an agent cannot hand off a thread it started: only a person closes it"
    );
    let resolved = engine.handle(
        OperationId(12),
        &agent(CommandKind::ResolveReviewComment { id }),
    );
    assert_eq!(
        failure(&resolved),
        "only a human can resolve a review comment"
    );
    let reopened = engine.handle(
        OperationId(13),
        &agent(CommandKind::ReopenReviewComment { id }),
    );
    assert_eq!(
        failure(&reopened),
        "only a human can reopen a review comment"
    );
    assert_eq!(only_any(&mut engine).status, ReviewStatus::Open);

    let closed = engine.handle(
        OperationId(14),
        &human(CommandKind::ResolveReviewComment { id }),
    );
    assert!(done(&closed), "{}", failure(&closed));
    assert_eq!(only_any(&mut engine).status, ReviewStatus::Resolved);
    let reopened = engine.handle(
        OperationId(15),
        &human(CommandKind::ReopenReviewComment { id }),
    );
    assert!(done(&reopened), "{}", failure(&reopened));
    assert_eq!(only_any(&mut engine).status, ReviewStatus::Open);
}

#[test]
fn a_question_needs_an_answer_until_a_person_replies() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(
        OperationId(10),
        &agent(agent_comment("f.txt", "c", ReviewKind::Question)),
    );
    let id = only_any(&mut engine).id;
    assert!(only_any(&mut engine).needs_answer);

    let events = reply_as(
        &mut engine,
        Actor::Agent {
            name: "test-agent".to_owned(),
        },
        id,
        "ping",
    );
    assert!(done(&events), "{}", failure(&events));
    assert!(
        only_any(&mut engine).needs_answer,
        "an agent's reply is not an answer"
    );

    let events = reply_as(&mut engine, Actor::Human, id, "use b");
    assert!(done(&events), "{}", failure(&events));
    assert!(!only_any(&mut engine).needs_answer);
}

#[test]
fn the_default_list_hides_an_agent_thread_until_a_person_replies() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    engine.handle(OperationId(10), &human(add("f.txt", 1, 1, "mine")));
    engine.handle(
        OperationId(11),
        &agent(agent_comment("f.txt", "c", ReviewKind::Note)),
    );
    let person = list_as(&mut engine, ReviewAuthorFilter::Human);
    assert_eq!(person.len(), 1, "the agent thread is hidden: {person:?}");
    assert_eq!(person[0].author, None);

    let agent_id = list_as(&mut engine, ReviewAuthorFilter::Agent)[0].id;
    assert_eq!(list_as(&mut engine, ReviewAuthorFilter::Agent).len(), 1);
    assert_eq!(list_as(&mut engine, ReviewAuthorFilter::Any).len(), 2);

    let events = reply_as(&mut engine, Actor::Human, agent_id, "thanks");
    assert!(done(&events), "{}", failure(&events));
    let shown = list_as(&mut engine, ReviewAuthorFilter::Human);
    assert_eq!(shown.len(), 2, "answered, it is shown: {shown:?}");
    assert!(shown.iter().any(|comment| comment.id == agent_id));
}

#[test]
fn an_agent_thread_is_a_logged_write() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let events = engine.handle(
        OperationId(60),
        &agent(agent_comment("f.txt", "e", ReviewKind::Note)),
    );
    assert!(done(&events), "{}", failure(&events));
    let logged: Vec<OperationState> = engine
        .log()
        .iter()
        .filter(|entry| matches!(entry.kind, CommandKind::CreateAgentReviewComment { .. }))
        .filter(|entry| entry.id == OperationId(60))
        .map(|entry| entry.state)
        .collect();
    assert!(logged.contains(&OperationState::Done), "{logged:?}");
    let before = engine.log().len();
    list_as(&mut engine, ReviewAuthorFilter::Any);
    assert_eq!(engine.log().len(), before, "a listing leaves no entry");
}

/// A person's comment carries the label they chose (D236 (7)); it stays a person's comment,
/// with no kind, and the label is listed with it, as an agent reads it over MCP.
#[test]
fn a_persons_comment_keeps_its_label_and_has_no_kind() {
    let dir = repo();
    let log = CommandLog::default();
    let mut engine = engine_on(dir.path(), &log);
    let labelled = CommandKind::AddReviewComment {
        label: Some(gm_schema::CommentLabel::Nitpick),
        path: "f.txt".to_owned(),
        start_line: 1,
        end_line: 1,
        body: "Rename this.".to_owned(),
    };
    let events = engine.handle(OperationId(2), &human(labelled));
    assert!(done(&events), "{}", failure(&events));
    let comment = only(&mut engine);
    assert_eq!(comment.label, Some(gm_schema::CommentLabel::Nitpick));
    assert_eq!(comment.kind, None);
    assert_eq!(comment.author, None);
}
