//! Reading and resolving conflicted files (D114) against real repositories in
//! temporary directories. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicU64, Ordering};

use gm_core::Engine;
use gm_core::command_log::CommandLog;
use gm_core::conflict::parse;
use gm_core::git::SubprocessGit;
use gm_schema::{
    Actor, Command, CommandKind, ConflictFileRead, ConflictOperation, ConflictSide, Event,
    HunkOrigin, OperationId, OperationState, RecordedResolution,
};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Runs git without asserting its exit code: a merge or rebase is meant to stop.
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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write(dir: &Path, name: &str, content: &[u8]) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// A repository on `main` with one commit that holds `a.txt` ("base").
struct Repo {
    _base: tempfile::TempDir,
    root: PathBuf,
}

fn repo() -> Repo {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git_in(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.txt", b"base\n");
    git_in(&root, &["add", "."]);
    git_in(&root, &["commit", "-q", "-m", "base"]);
    Repo { _base: base, root }
}

/// Commits `content` to `file` on the current branch with `message`.
fn commit(dir: &Path, file: &str, content: &[u8], message: &str) {
    write(dir, file, content);
    git_in(dir, &["add", "--", file]);
    git_in(dir, &["commit", "-q", "-m", message]);
}

fn engine_on(root: &Path) -> Engine {
    let git = SubprocessGit {
        log: Some(CommandLog::default()),
        // The engine must not see the developer's own `rerere.autoupdate` and the like.
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = Engine::new(Box::new(git));
    let events = engine.handle(
        OperationId(1),
        &human(CommandKind::OpenRepo { path: text(root) }),
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
    events.iter().any(|e| {
        matches!(
            e,
            Event::OperationChanged {
                state: OperationState::Done,
                ..
            }
        )
    })
}

fn failure(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::OperationChanged {
            state: OperationState::Failed,
            detail,
            ..
        } => detail.clone(),
        _ => None,
    })
}

static NEXT_ID: AtomicU64 = AtomicU64::new(100);

fn next_id() -> OperationId {
    OperationId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// The answer to `ReadConflict` for `path`, or Git's refusal.
fn read(engine: &mut Engine, path: &str) -> Result<ConflictFileRead, String> {
    let events = engine.handle(
        next_id(),
        &human(CommandKind::ReadConflict {
            path: path.to_owned(),
        }),
    );
    if let Some(detail) = failure(&events) {
        return Err(detail);
    }
    events
        .iter()
        .find_map(|e| match e {
            Event::ConflictRead { read, .. } => Some(read.clone()),
            _ => None,
        })
        .ok_or_else(|| "no conflict was read".to_owned())
}

fn resolve(engine: &mut Engine, path: &str, text: &str) -> Result<(), String> {
    let events = engine.handle(
        next_id(),
        &human(CommandKind::ResolveConflict {
            path: path.to_owned(),
            text: text.to_owned(),
            origins: Vec::new(),
        }),
    );
    if let Some(detail) = failure(&events) {
        return Err(detail);
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ConflictResolved { path: p, .. } if p == path)),
        "no ConflictResolved: {events:?}"
    );
    Ok(())
}

fn side(branch: &str, role: &str, subject: &str) -> ConflictSide {
    ConflictSide {
        branch: branch.to_owned(),
        role: role.to_owned(),
        subject: subject.to_owned(),
    }
}

/// `main` and `feature` both change `a.txt`, and `git merge feature` stops.
fn merge_stopped(repo: &Repo) {
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    commit(&repo.root, "a.txt", b"feature\n", "feature change");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"main\n", "main change");
    let output = git_output(&repo.root, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
}

const MERGE_TEXT: &str =
    "<<<<<<< main\nmain\n||||||| base\nbase\n=======\nfeature\n>>>>>>> feature\n";

#[test]
fn a_merge_conflict_is_read_with_the_sides_named_by_role() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.path, "a.txt");
    assert_eq!(read.operation, ConflictOperation::Merge);
    assert_eq!(read.left, side("main", "current branch", "main change"));
    assert_eq!(
        read.right,
        side("feature", "merged branch", "feature change")
    );
    assert!(read.has_base);
    assert_eq!(read.text, MERGE_TEXT);
    assert_eq!(parse(&read.text).unwrap().conflict_count(), 1);
}

#[test]
fn the_text_is_zdiff3_whatever_the_user_configured() {
    let repo = repo();
    git_in(&repo.root, &["config", "merge.conflictStyle", "merge"]);
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.text, MERGE_TEXT);
}

#[test]
fn the_text_is_built_from_the_stages_not_from_the_working_tree() {
    let repo = repo();
    merge_stopped(&repo);
    write(&repo.root, "a.txt", b"edited by hand\n");
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.text, MERGE_TEXT);
}

#[test]
fn a_rebase_conflict_names_the_new_parent_and_the_replayed_commit() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    commit(&repo.root, "a.txt", b"feature\n", "feature change");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"main\n", "main change");
    git_in(&repo.root, &["checkout", "-q", "feature"]);
    let output = git_output(&repo.root, &["rebase", "main"]);
    assert!(!output.status.success(), "the rebase should conflict");
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.operation, ConflictOperation::Rebase);
    assert_eq!(read.left, side("main", "new parent", "main change"));
    assert_eq!(
        read.right,
        side("feature", "commit being replayed", "feature change")
    );
    assert_eq!(read.text, MERGE_TEXT);
}

#[test]
fn a_cherry_pick_conflict_names_the_picked_commit() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "dev"]);
    commit(&repo.root, "a.txt", b"dev\n", "dev change");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"main\n", "main change");
    let output = git_output(&repo.root, &["cherry-pick", "dev"]);
    assert!(!output.status.success(), "the cherry-pick should conflict");
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.operation, ConflictOperation::CherryPick);
    assert_eq!(read.left, side("main", "current branch", "main change"));
    assert_eq!(read.right, side("dev", "picked commit", "dev change"));
    assert!(read.has_base);
}

#[test]
fn a_conflict_in_a_linked_worktree_is_read_from_that_worktree() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    commit(&repo.root, "a.txt", b"feature\n", "feature change");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"main\n", "main change");
    let worktree = repo.root.parent().unwrap().join("linked");
    git_in(
        &repo.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            &text(&worktree),
            "main",
        ],
    );
    let output = git_output(&worktree, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let mut engine = engine_on(&worktree);

    let read = read(&mut engine, "a.txt").unwrap();

    assert_eq!(read.operation, ConflictOperation::Merge);
    assert_eq!(read.left.branch, "linked");
    assert_eq!(read.left.role, "current branch");
    assert_eq!(
        read.right,
        side("feature", "merged branch", "feature change")
    );
}

#[test]
fn an_add_add_conflict_has_no_base() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    commit(&repo.root, "b.txt", b"feature\n", "feature adds b");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "b.txt", b"main\n", "main adds b");
    let output = git_output(&repo.root, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let mut engine = engine_on(&repo.root);

    let read = read(&mut engine, "b.txt").unwrap();

    assert!(!read.has_base);
    assert_eq!(read.operation, ConflictOperation::Merge);
    assert_eq!(parse(&read.text).unwrap().conflict_count(), 1);
    assert!(read.text.starts_with("<<<<<<< main\nmain\n"));
    assert!(read.text.ends_with("=======\nfeature\n>>>>>>> feature\n"));
}

#[test]
fn a_path_that_is_not_conflicted_is_refused() {
    let repo = repo();
    let mut engine = engine_on(&repo.root);

    let error = read(&mut engine, "a.txt").unwrap_err();

    assert!(error.contains("not conflicted"), "{error}");
}

#[test]
fn a_conflict_with_a_deleted_side_is_refused_as_unsupported() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    git_in(&repo.root, &["rm", "-q", "a.txt"]);
    git_in(&repo.root, &["commit", "-q", "-m", "feature deletes"]);
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"main\n", "main change");
    let output = git_output(&repo.root, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let mut engine = engine_on(&repo.root);

    let error = read(&mut engine, "a.txt").unwrap_err();

    assert!(error.contains("deleted side"), "{error}");
}

#[test]
fn a_binary_conflict_is_refused() {
    let repo = repo();
    git_in(&repo.root, &["checkout", "-q", "-b", "feature"]);
    commit(&repo.root, "a.txt", b"\0feature", "feature change");
    git_in(&repo.root, &["checkout", "-q", "main"]);
    commit(&repo.root, "a.txt", b"\0main", "main change");
    let output = git_output(&repo.root, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let mut engine = engine_on(&repo.root);

    let error = read(&mut engine, "a.txt").unwrap_err();

    assert!(error.contains("binary"), "{error}");
}

#[test]
fn resolving_writes_the_file_and_stages_it() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    let events = engine.handle(
        next_id(),
        &human(CommandKind::ResolveConflict {
            path: "a.txt".into(),
            text: "resolved\n".into(),
            origins: Vec::new(),
        }),
    );

    assert_eq!(failure(&events), None);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ConflictResolved { path, .. } if path == "a.txt"))
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "resolved\n"
    );
    assert_eq!(git_in(&repo.root, &["ls-files", "-u"]), "");
    assert_eq!(git_in(&repo.root, &["show", ":a.txt"]), "resolved");
    assert_eq!(
        git_in(&repo.root, &["diff", "--cached", "--name-only"]),
        "a.txt"
    );
    assert!(!git_in(&repo.root, &["status", "--porcelain"]).contains("UU"));
    assert!(
        engine
            .log()
            .iter()
            .any(|entry| matches!(entry.kind, CommandKind::ResolveConflict { .. }))
    );
}

#[test]
fn leftover_markers_are_refused_and_the_file_is_untouched() {
    let repo = repo();
    merge_stopped(&repo);
    let before = std::fs::read(repo.root.join("a.txt")).unwrap();
    let mut engine = engine_on(&repo.root);
    let markers = read(&mut engine, "a.txt").unwrap().text;

    let error = resolve(&mut engine, "a.txt", &markers).unwrap_err();

    assert!(error.contains("1 conflict(s) to resolve"), "{error}");
    assert_eq!(std::fs::read(repo.root.join("a.txt")).unwrap(), before);
    assert_eq!(git_in(&repo.root, &["ls-files", "-u"]).lines().count(), 3);
}

#[test]
fn a_malformed_marker_is_refused() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    let error = resolve(&mut engine, "a.txt", "<<<<<<< main\nleft\n").unwrap_err();

    assert!(error.contains("conflict marker"), "{error}");
    assert_eq!(git_in(&repo.root, &["ls-files", "-u"]).lines().count(), 3);
}

#[test]
fn resolving_an_already_resolved_file_is_refused() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    resolve(&mut engine, "a.txt", "resolved\n").unwrap();

    let error = resolve(&mut engine, "a.txt", "again\n").unwrap_err();

    assert!(error.contains("already resolved"), "{error}");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "resolved\n"
    );
}

#[cfg(unix)]
#[test]
fn resolving_keeps_the_file_mode() {
    use std::os::unix::fs::PermissionsExt;

    let repo = repo();
    merge_stopped(&repo);
    let file = repo.root.join("a.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut engine = engine_on(&repo.root);

    resolve(&mut engine, "a.txt", "resolved\n").unwrap();

    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}

/// The number of resolutions rerere has recorded in the repository of `root`.
fn recorded_postimages(root: &Path) -> usize {
    let cache = root.join(".git").join("rr-cache");
    std::fs::read_dir(cache).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| entry.path().join("postimage").is_file())
            .count()
    })
}

/// Forgets the recorded resolution of `path`, or returns Git's refusal.
fn forget(engine: &mut Engine, path: &str) -> Result<(), String> {
    let events = engine.handle(
        next_id(),
        &human(CommandKind::ForgetRecordedResolution {
            path: path.to_owned(),
        }),
    );
    if let Some(detail) = failure(&events) {
        return Err(detail);
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::RecordedResolutionForgotten { path: p, .. } if p == path)),
        "no RecordedResolutionForgotten: {events:?}"
    );
    Ok(())
}

/// Resolves the merge of `merge_stopped` to `resolved`, undoes it and merges the
/// same two branches again, so the same conflict is back.
fn conflict_resolved_then_back(repo: &Repo, engine: &mut Engine) {
    resolve(engine, "a.txt", "resolved\n").unwrap();
    git_in(&repo.root, &["reset", "-q", "--hard"]);
    let output = git_output(&repo.root, &["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict again");
}

#[test]
fn a_resolution_is_recorded_for_the_next_identical_conflict() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    assert_eq!(read(&mut engine, "a.txt").unwrap().recorded, None);

    resolve(&mut engine, "a.txt", "resolved\n").unwrap();

    assert_eq!(recorded_postimages(&repo.root), 1);
}

#[test]
fn a_resolution_is_recorded_even_when_the_conflict_was_not_read_first() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    resolve(&mut engine, "a.txt", "resolved\n").unwrap();

    assert_eq!(recorded_postimages(&repo.root), 1);
}

#[test]
fn an_identical_conflict_is_read_with_its_recorded_resolution() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    conflict_resolved_then_back(&repo, &mut engine);

    let first = read(&mut engine, "a.txt").unwrap();

    let expected = Some(RecordedResolution {
        text: "resolved\n".into(),
    });
    assert_eq!(first.recorded, expected);
    // Reading again, as when the resolver is reopened, still finds it.
    let again = read(&mut engine, "a.txt").unwrap();
    assert_eq!(again.recorded, expected);
    // The stage-based text stays the conflict, so the engineer can compare.
    assert_eq!(first.text, MERGE_TEXT);
    // Git applied the resolution to the working tree and left the file unmerged.
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "resolved\n"
    );
    assert_ne!(git_in(&repo.root, &["ls-files", "-u"]), "");
}

#[test]
fn forgetting_a_recorded_resolution_brings_the_markers_back() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    conflict_resolved_then_back(&repo, &mut engine);
    assert!(read(&mut engine, "a.txt").unwrap().recorded.is_some());

    forget(&mut engine, "a.txt").unwrap();

    let file = std::fs::read_to_string(repo.root.join("a.txt")).unwrap();
    assert!(file.contains("<<<<<<<"), "{file}");
    assert_eq!(parse(&file).unwrap().conflict_count(), 1);
    assert_ne!(git_in(&repo.root, &["ls-files", "-u"]), "");
    assert_eq!(recorded_postimages(&repo.root), 0);
    assert_eq!(read(&mut engine, "a.txt").unwrap().recorded, None);
}

#[test]
fn forgetting_with_nothing_recorded_or_on_a_resolved_file_is_refused() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    let before = std::fs::read(repo.root.join("a.txt")).unwrap();

    let error = forget(&mut engine, "a.txt").unwrap_err();
    assert!(error.contains("no recorded resolution"), "{error}");
    assert_eq!(std::fs::read(repo.root.join("a.txt")).unwrap(), before);

    resolve(&mut engine, "a.txt", "resolved\n").unwrap();
    let error = forget(&mut engine, "a.txt").unwrap_err();
    assert!(error.contains("not conflicted"), "{error}");
}

#[test]
fn rerere_never_writes_the_users_config() {
    let repo = repo();
    let config = repo.root.join(".git").join("config");
    let before = std::fs::read(&config).unwrap();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);
    conflict_resolved_then_back(&repo, &mut engine);
    read(&mut engine, "a.txt").unwrap();
    forget(&mut engine, "a.txt").unwrap();

    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert!(
        !git_output(&repo.root, &["config", "--get", "rerere.enabled"])
            .status
            .success()
    );
}

#[test]
fn the_origins_of_a_resolution_are_in_the_operation_log() {
    let repo = repo();
    merge_stopped(&repo);
    let mut engine = engine_on(&repo.root);

    let events = engine.handle(
        next_id(),
        &human(CommandKind::ResolveConflict {
            path: "a.txt".into(),
            text: "resolved\n".into(),
            origins: vec![HunkOrigin::Human],
        }),
    );

    assert_eq!(failure(&events), None);
    assert!(engine.log().iter().any(|entry| matches!(
        &entry.kind,
        CommandKind::ResolveConflict { origins, .. } if *origins == [HunkOrigin::Human]
    )));
}
