//! The restack conflict resolution (D154) against real repositories in temporary directories:
//! the resolution worktree and its stages, the commit it builds, the removal of the worktree,
//! the internal folder hidden from `worktrees()`, and the resolutions store with its refs.
//! Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_core::restack_resolutions::{self, Resolution};

fn git_env(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> String {
    let mut command = Process::new("git");
    command
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    git_env(dir, &[], args)
}

fn ref_exists(dir: &Path, name: &str) -> bool {
    Process::new("git")
        .args(["rev-parse", "--verify", "--quiet", name])
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success()
}

fn write(path: PathBuf, text: &str) {
    std::fs::write(path, text).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    git: SubprocessGit,
    /// The tip of `main`, the `onto` of the replay.
    onto: String,
    /// The commit of `feature` that conflicts with `onto` in `a.txt` and `d.txt`.
    commit: String,
}

/// `main` and `feature` diverge from one commit: `feature` changes `a.txt` and `d.txt`, `main`
/// changes the same two lines, so cherry-picking `feature` onto `main` conflicts in both. The
/// main checkout is left dirty: an unstaged change to `b.txt` and a staged new file `s.txt`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git_in(&root, &["init", "-q", "-b", "main"]);
    write(root.join("a.txt"), "one\n");
    write(root.join("b.txt"), "b\n");
    write(root.join("d.txt"), "one\n");
    git_in(&root, &["add", "."]);
    git_in(&root, &["commit", "-q", "-m", "init"]);

    git_in(&root, &["checkout", "-q", "-b", "feature"]);
    write(root.join("a.txt"), "two\n");
    write(root.join("d.txt"), "two\n");
    git_in(&root, &["add", "."]);
    git_env(
        &root,
        &[
            ("GIT_AUTHOR_NAME", "Ann Author"),
            ("GIT_AUTHOR_EMAIL", "ann@example.com"),
            ("GIT_AUTHOR_DATE", "1577934245 +0000"),
            ("GIT_COMMITTER_DATE", "1623053350 +0000"),
        ],
        &["commit", "-q", "-m", "Change a and d", "-m", "With a body."],
    );
    let commit = git_in(&root, &["rev-parse", "HEAD"]);

    git_in(&root, &["checkout", "-q", "main"]);
    write(root.join("a.txt"), "three\n");
    write(root.join("d.txt"), "three\n");
    git_in(&root, &["commit", "-q", "-am", "main moves"]);
    let onto = git_in(&root, &["rev-parse", "HEAD"]);

    write(root.join("b.txt"), "b changed\n");
    write(root.join("s.txt"), "staged\n");
    git_in(&root, &["add", "s.txt"]);

    Fixture {
        _dir: dir,
        root,
        git: SubprocessGit::default(),
        onto,
        commit,
    }
}

/// The porcelain status, untrimmed: its first line starts with a space when only the worktree changed.
fn status(root: &Path) -> String {
    let output = Process::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn opening_on_a_conflict_lists_the_conflicting_paths_and_leaves_the_checkout_alone() {
    let fixture = fixture();
    let before = status(&fixture.root);
    assert_eq!(before, " M b.txt\nA  s.txt\n");

    let path = fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();

    assert!(path.exists());
    assert!(path.ends_with(Path::new(".git/gitmaster/restack-resolve")));
    assert_eq!(
        fixture.git.restack_resolution_unmerged(&path).unwrap(),
        vec!["a.txt".to_owned(), "d.txt".to_owned()]
    );
    assert_eq!(status(&fixture.root), before);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("b.txt")).unwrap(),
        "b changed\n"
    );
    assert!(!ref_exists(&fixture.root, "CHERRY_PICK_HEAD"));
}

#[test]
fn committing_refuses_while_unmerged_and_then_keeps_the_metadata() {
    let fixture = fixture();
    let path = fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();

    let refused = fixture
        .git
        .restack_resolution_commit(&fixture.root, &path, &fixture.onto, &fixture.commit)
        .unwrap_err();
    assert!(refused.0.contains("a.txt"), "{}", refused.0);

    write(path.join("a.txt"), "resolved\n");
    write(path.join("d.txt"), "resolved\n");
    git_in(&path, &["add", "a.txt", "d.txt"]);
    let resolved = fixture
        .git
        .restack_resolution_commit(&fixture.root, &path, &fixture.onto, &fixture.commit)
        .unwrap();

    assert_eq!(
        git_in(&fixture.root, &["rev-parse", &format!("{resolved}^")]),
        fixture.onto
    );
    assert_eq!(
        git_in(&fixture.root, &["show", &format!("{resolved}:a.txt")]),
        "resolved"
    );
    assert_eq!(
        git_in(&fixture.root, &["show", &format!("{resolved}:b.txt")]),
        "b"
    );
    assert!(path.exists(), "the worktree stays until it is closed");

    let fields = "--format=%an%x00%ae%x00%ad%x00%cd%x00%B";
    let original = git_in(
        &fixture.root,
        &["log", "-1", "--date=raw", fields, &fixture.commit],
    );
    let built = git_in(
        &fixture.root,
        &["log", "-1", "--date=raw", fields, &resolved],
    );
    assert_eq!(built, original);
    let author_date = built.split('\0').nth(2).unwrap();
    assert_eq!(author_date, "1577934245 +0000");
    assert!(built.ends_with("Change a and d\n\nWith a body."));

    fixture.git.restack_resolution_close(&fixture.root).unwrap();
}

#[test]
fn closing_removes_the_worktree_and_closing_twice_is_fine() {
    let fixture = fixture();
    let path = fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();

    fixture.git.restack_resolution_close(&fixture.root).unwrap();

    assert!(!path.exists());
    let list = git_in(&fixture.root, &["worktree", "list", "--porcelain"]);
    assert!(!list.contains("gitmaster"), "{list}");
    assert!(!fixture.root.join(".git/gitmaster/restack-resolve").exists());
    fixture.git.restack_resolution_close(&fixture.root).unwrap();
    assert_eq!(status(&fixture.root), " M b.txt\nA  s.txt\n");
}

#[test]
fn opening_twice_replaces_the_first_worktree() {
    let fixture = fixture();
    fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();
    write(
        fixture.root.join(".git/gitmaster/restack-resolve/a.txt"),
        "left over\n",
    );

    let path = fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();

    assert_eq!(
        fixture.git.restack_resolution_unmerged(&path).unwrap(),
        vec!["a.txt".to_owned(), "d.txt".to_owned()]
    );
    // The second open starts from a fresh checkout, not from the first worktree's files.
    let a = std::fs::read_to_string(path.join("a.txt")).unwrap();
    assert!(!a.contains("left over"), "{a}");
    fixture.git.restack_resolution_close(&fixture.root).unwrap();
}

#[test]
fn worktrees_leaves_out_the_resolution_worktree_and_keeps_a_normal_one() {
    let fixture = fixture();
    let normal = fixture.root.parent().unwrap().join("linked");
    git_in(
        &fixture.root,
        &[
            "worktree",
            "add",
            "--detach",
            normal.to_str().unwrap(),
            &fixture.onto,
        ],
    );
    fixture
        .git
        .restack_resolution_open(&fixture.root, &fixture.onto, &fixture.commit)
        .unwrap();

    let listed: Vec<PathBuf> = fixture
        .git
        .worktrees(&fixture.root)
        .unwrap()
        .into_iter()
        .map(|entry| PathBuf::from(entry.path))
        .collect();

    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap();
    assert!(
        listed.iter().any(|p| canonical(p) == canonical(&normal)),
        "{listed:?}"
    );
    assert!(
        !listed.iter().any(|p| p.ends_with("restack-resolve")),
        "{listed:?}"
    );
    fixture.git.restack_resolution_close(&fixture.root).unwrap();
}

#[test]
fn the_store_round_trips_and_keeps_each_resolved_commit_by_a_ref() {
    let fixture = fixture();
    let root = &fixture.root;
    let git = &fixture.git;
    assert!(restack_resolutions::load(git, root).unwrap().is_empty());

    let path = git
        .restack_resolution_open(root, &fixture.onto, &fixture.commit)
        .unwrap();
    write(path.join("a.txt"), "resolved\n");
    write(path.join("d.txt"), "resolved\n");
    git_in(&path, &["add", "a.txt", "d.txt"]);
    let resolved = git
        .restack_resolution_commit(root, &path, &fixture.onto, &fixture.commit)
        .unwrap();
    git.restack_resolution_close(root).unwrap();

    let first = Resolution {
        onto: fixture.onto.clone(),
        commit: fixture.commit.clone(),
        resolved: resolved.clone(),
    };
    let ref_of = |oid: &str| format!("refs/gitmaster/restack/resolved/{oid}");
    restack_resolutions::add(git, root, first.clone()).unwrap();
    assert_eq!(
        restack_resolutions::load(git, root).unwrap(),
        vec![first.clone()]
    );
    assert!(ref_exists(root, &ref_of(&resolved)));
    let list = restack_resolutions::load(git, root).unwrap();
    assert_eq!(
        restack_resolutions::lookup(&list, &fixture.onto, &fixture.commit),
        Some(resolved.as_str())
    );
    assert_eq!(
        restack_resolutions::lookup(&list, &fixture.commit, &fixture.commit),
        None
    );

    // The same pair again replaces the entry, and the replaced commit's ref goes.
    let tree = git_in(root, &["rev-parse", &format!("{resolved}^{{tree}}")]);
    let other = git_in(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            &fixture.onto,
            "-m",
            "other resolution",
        ],
    );
    let second = Resolution {
        resolved: other.clone(),
        ..first.clone()
    };
    restack_resolutions::add(git, root, second.clone()).unwrap();
    assert_eq!(
        restack_resolutions::load(git, root).unwrap(),
        vec![second.clone()]
    );
    assert!(ref_exists(root, &ref_of(&other)));
    assert!(!ref_exists(root, &ref_of(&resolved)));

    restack_resolutions::remove(git, root, &[second]).unwrap();
    assert!(restack_resolutions::load(git, root).unwrap().is_empty());
    assert!(!ref_exists(root, &ref_of(&other)));
}
