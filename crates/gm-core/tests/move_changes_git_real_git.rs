//! The Git primitives of moving a file's change to another branch or worktree
//! (D109), against real repositories in temporary directories. Each test
//! removes its directory when it ends. They need Git 2.38 or later, for
//! `merge-tree --write-tree`.

use std::path::Path;
use std::process::Command;

use gm_core::git::SubprocessGit;
use gm_core::{DiscardOutcome, FileMerge, GitPort, TreeMerge};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
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

/// The major and minor version of the `git` in use.
fn version_pair(text: &str) -> (u32, u32) {
    let number = text
        .split_whitespace()
        .nth(2)
        .unwrap_or_else(|| panic!("cannot read the git version from {text:?}"));
    let mut parts = number
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Fails with a clear message when the `git` in use cannot merge trees.
fn require_git_2_38() {
    let version = SubprocessGit::default().git_version().unwrap();
    assert!(
        version_pair(&version) >= (2, 38),
        "these tests need Git 2.38 or later (merge-tree --write-tree), found: {version}"
    );
}

/// A repository on `main` with `f.txt`, `g.txt` and `h.txt` committed, and the
/// identity Ana in its own config (what `commit_tree` must use).
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Ana"]);
    git(dir.path(), &["config", "user.email", "ana@example.com"]);
    write(dir.path(), "f.txt", "one\n");
    write(dir.path(), "g.txt", "two\n");
    write(dir.path(), "h.txt", "gone\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

/// Stages everything and commits it, returning the new commit id.
fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    head(dir)
}

/// The public state a move snapshot must leave alone: status, refs, HEAD and the
/// index, plus the content of every file in the repository.
fn public_state(dir: &Path) -> String {
    let mut state = String::new();
    state.push_str(&git(dir, &["status", "--porcelain"]));
    state.push_str(&git(dir, &["for-each-ref"]));
    state.push_str(&git(dir, &["rev-parse", "HEAD"]));
    state.push_str(&git(dir, &["ls-files", "-s"]));
    for name in ["f.txt", "g.txt", "h.txt"] {
        if let Ok(content) = std::fs::read_to_string(dir.join(name)) {
            state.push_str(&content);
        }
    }
    state
}

/// The paths that differ between two commits.
fn changed_between(dir: &Path, from: &str, to: &str) -> Vec<String> {
    git(dir, &["diff", "--name-only", from, to])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn git_version_is_the_version_line_and_is_new_enough() {
    let version = SubprocessGit::default().git_version().unwrap();
    assert!(version.starts_with("git version "), "{version:?}");
    assert!(!version.ends_with('\n'), "{version:?}");
    require_git_2_38();
}

#[test]
fn snapshot_paths_holds_only_the_chosen_modified_file() {
    require_git_2_38();
    let dir = repo();
    write(dir.path(), "f.txt", "one\nchanged\n");
    write(dir.path(), "g.txt", "two\nalso changed\n");
    let before = public_state(dir.path());

    let snapshot = SubprocessGit::default()
        .snapshot_paths(dir.path(), &["f.txt".to_owned()])
        .unwrap();

    assert_eq!(
        changed_between(dir.path(), &head(dir.path()), &snapshot),
        ["f.txt"]
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{snapshot}:f.txt")]),
        "one\nchanged\n"
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{snapshot}:g.txt")]),
        "two\n"
    );
    assert_eq!(public_state(dir.path()), before);
}

#[test]
fn snapshot_paths_holds_a_new_untracked_file_and_a_deleted_one() {
    require_git_2_38();
    let dir = repo();
    write(dir.path(), "n.txt", "new\n");
    std::fs::remove_file(dir.path().join("h.txt")).unwrap();
    let before = public_state(dir.path());
    let git_port = SubprocessGit::default();

    let added = git_port
        .snapshot_paths(dir.path(), &["n.txt".to_owned()])
        .unwrap();
    assert_eq!(
        changed_between(dir.path(), &head(dir.path()), &added),
        ["n.txt"]
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{added}:n.txt")]),
        "new\n"
    );

    let deleted = git_port
        .snapshot_paths(dir.path(), &["h.txt".to_owned()])
        .unwrap();
    assert_eq!(
        changed_between(dir.path(), &head(dir.path()), &deleted),
        ["h.txt"]
    );
    assert!(
        !git_output(
            dir.path(),
            &["rev-parse", "--verify", "-q", &format!("{deleted}:h.txt")]
        )
        .status
        .success()
    );

    assert_eq!(public_state(dir.path()), before);
}

#[test]
fn snapshot_paths_starts_from_head_not_from_the_staged_index() {
    require_git_2_38();
    let dir = repo();
    write(dir.path(), "f.txt", "one\nchosen\n");
    write(dir.path(), "g.txt", "staged elsewhere\n");
    git(dir.path(), &["add", "g.txt"]);
    let before = public_state(dir.path());

    let snapshot = SubprocessGit::default()
        .snapshot_paths(dir.path(), &["f.txt".to_owned()])
        .unwrap();

    assert_eq!(
        changed_between(dir.path(), &head(dir.path()), &snapshot),
        ["f.txt"]
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{snapshot}:g.txt")]),
        "two\n"
    );
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"]),
        "g.txt\n"
    );
    assert_eq!(public_state(dir.path()), before);
}

#[test]
fn merge_trees_merges_cleanly_and_names_the_conflicting_file() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    write(dir.path(), "f.txt", "a\nb\nc\n");
    let base = commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-q", "-b", "ours"]);
    write(dir.path(), "f.txt", "A\nb\nc\n");
    let ours = commit_all(dir.path(), "ours");
    git(dir.path(), &["checkout", "-q", "--detach", &base]);
    write(dir.path(), "f.txt", "Z\nb\nc\n");
    let theirs = commit_all(dir.path(), "theirs");
    git(dir.path(), &["checkout", "-q", "--detach", &base]);
    write(dir.path(), "g.txt", "elsewhere\n");
    let theirs_elsewhere = commit_all(dir.path(), "theirs elsewhere");

    let conflict = git_port
        .merge_trees(dir.path(), &base, &ours, &theirs)
        .unwrap();
    assert_eq!(
        conflict,
        TreeMerge::Conflict {
            paths: vec!["f.txt".to_owned()]
        }
    );

    let clean = git_port
        .merge_trees(dir.path(), &base, &ours, &theirs_elsewhere)
        .unwrap();
    let TreeMerge::Clean { tree } = clean else {
        panic!("expected a clean merge, got {clean:?}");
    };
    assert_eq!(
        git(dir.path(), &["show", &format!("{tree}:f.txt")]),
        "A\nb\nc\n"
    );
    assert_eq!(
        git(dir.path(), &["show", &format!("{tree}:g.txt")]),
        "elsewhere\n"
    );
}

#[test]
fn merge_trees_uses_the_base_it_is_given() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    write(dir.path(), "f.txt", "a\nb\nc\n");
    let base = commit_all(dir.path(), "base");
    git(dir.path(), &["checkout", "-q", "-b", "ours"]);
    write(dir.path(), "f.txt", "A\nb\nc\n");
    let ours = commit_all(dir.path(), "ours");
    git(dir.path(), &["checkout", "-q", "--detach", &base]);
    write(dir.path(), "f.txt", "Z\nb\nc\n");
    let theirs = commit_all(dir.path(), "theirs");
    // The same content as `ours`, so the merge base that has `A` makes `theirs`
    // the only change.
    let other_base = git(
        dir.path(),
        &[
            "commit-tree",
            &format!("{ours}^{{tree}}"),
            "-p",
            &base,
            "-m",
            "other base",
        ],
    )
    .trim()
    .to_owned();

    assert!(matches!(
        git_port
            .merge_trees(dir.path(), &base, &ours, &theirs)
            .unwrap(),
        TreeMerge::Conflict { .. }
    ));
    let merged = git_port
        .merge_trees(dir.path(), &other_base, &ours, &theirs)
        .unwrap();
    let TreeMerge::Clean { tree } = merged else {
        panic!("expected a clean merge, got {merged:?}");
    };
    assert_eq!(
        git(dir.path(), &["show", &format!("{tree}:f.txt")]),
        "Z\nb\nc\n"
    );
}

#[test]
fn commit_tree_commits_the_tree_on_the_parent_under_the_users_identity() {
    require_git_2_38();
    let dir = repo();
    let parent = head(dir.path());
    let tree = git(dir.path(), &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_owned();

    let commit = SubprocessGit::default()
        .commit_tree(dir.path(), &tree, Some(&parent), "moved change\n")
        .unwrap();

    assert_eq!(
        git(dir.path(), &["rev-parse", &format!("{commit}^{{tree}}")]).trim(),
        tree
    );
    assert_eq!(
        git(dir.path(), &["rev-parse", &format!("{commit}^")]).trim(),
        parent
    );
    assert_eq!(
        git(
            dir.path(),
            &["log", "-1", "--format=%an <%ae>%n%B", &commit]
        ),
        "Ana <ana@example.com>\nmoved change\n\n"
    );
}

#[test]
fn update_ref_checks_the_old_value_and_refuses_names_outside_refs() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    let first = head(dir.path());
    write(dir.path(), "f.txt", "two\n");
    let second = commit_all(dir.path(), "second");
    let name = "refs/gitmaster/move/test";

    git_port.update_ref(dir.path(), name, &first, None).unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", name]).trim(), first);

    assert!(
        git_port
            .update_ref(dir.path(), name, &second, None)
            .is_err()
    );
    assert!(
        git_port
            .update_ref(dir.path(), name, &second, Some(&second))
            .is_err()
    );
    assert_eq!(git(dir.path(), &["rev-parse", name]).trim(), first);

    git_port
        .update_ref(dir.path(), name, &second, Some(&first))
        .unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", name]).trim(), second);

    assert!(
        git_port
            .update_ref(dir.path(), "HEAD", &first, None)
            .is_err()
    );
    assert!(
        git_port
            .update_ref(dir.path(), "heads/main", &first, None)
            .is_err()
    );
    assert_eq!(head(dir.path()), second);
}

#[test]
fn merge_file_merges_cleanly_conflicts_or_refuses_binary_content() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    let base = b"a\nb\nc\n";

    assert_eq!(
        git_port
            .merge_file(dir.path(), base, b"A\nb\nc\n", b"a\nb\nC\n")
            .unwrap(),
        FileMerge::Clean(b"A\nb\nC\n".to_vec())
    );
    assert_eq!(
        git_port
            .merge_file(dir.path(), base, b"X\nb\nc\n", b"Y\nb\nc\n")
            .unwrap(),
        FileMerge::Conflict
    );
    assert_eq!(
        git_port
            .merge_file(dir.path(), base, b"a\0\nb\n", b"a\nb\nc\n")
            .unwrap(),
        FileMerge::Binary
    );
    // Nothing is left in the common directory after the merges.
    let scratch = dir.path().join(".git").join("gitmaster").join("tmp");
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn discard_moved_discards_the_matching_modified_new_and_deleted_files() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    write(dir.path(), "f.txt", "one\nchanged\n");
    write(dir.path(), "n.txt", "new\n");
    std::fs::remove_file(dir.path().join("h.txt")).unwrap();
    write(dir.path(), "g.txt", "two\nleft alone\n");
    let snapshot = git_port
        .snapshot_paths(
            dir.path(),
            &["f.txt".to_owned(), "n.txt".to_owned(), "h.txt".to_owned()],
        )
        .unwrap();

    for path in ["f.txt", "n.txt", "h.txt"] {
        assert_eq!(
            git_port.discard_moved(dir.path(), path, &snapshot).unwrap(),
            DiscardOutcome::Discarded,
            "{path}"
        );
    }

    assert_eq!(read(dir.path(), "f.txt"), "one\n");
    assert!(!dir.path().join("n.txt").exists());
    // The deletion is undone in the source: the file is back as HEAD has it.
    assert_eq!(read(dir.path(), "h.txt"), "gone\n");
    assert_eq!(read(dir.path(), "g.txt"), "two\nleft alone\n");
}

#[test]
fn discard_moved_keeps_a_file_edited_after_the_snapshot() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    write(dir.path(), "f.txt", "one\nchanged\n");
    let snapshot = git_port
        .snapshot_paths(dir.path(), &["f.txt".to_owned()])
        .unwrap();
    write(dir.path(), "f.txt", "one\nchanged again\n");

    assert_eq!(
        git_port
            .discard_moved(dir.path(), "f.txt", &snapshot)
            .unwrap(),
        DiscardOutcome::Kept {
            reason: "the file changed after the move started".into()
        }
    );
    assert_eq!(read(dir.path(), "f.txt"), "one\nchanged again\n");
}

#[test]
fn discard_moved_keeps_a_file_deleted_after_the_snapshot() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    write(dir.path(), "f.txt", "one\nchanged\n");
    let snapshot = git_port
        .snapshot_paths(dir.path(), &["f.txt".to_owned()])
        .unwrap();
    std::fs::remove_file(dir.path().join("f.txt")).unwrap();

    assert!(matches!(
        git_port
            .discard_moved(dir.path(), "f.txt", &snapshot)
            .unwrap(),
        DiscardOutcome::Kept { .. }
    ));
    assert!(!dir.path().join("f.txt").exists());
}

#[test]
fn discard_moved_refuses_a_symbolic_link_and_a_path_outside_the_tree() {
    require_git_2_38();
    let dir = repo();
    let git_port = SubprocessGit::default();
    #[cfg(unix)]
    std::os::unix::fs::symlink("f.txt", dir.path().join("link")).unwrap();
    let snapshot = head(dir.path());

    #[cfg(unix)]
    assert!(
        git_port
            .discard_moved(dir.path(), "link", &snapshot)
            .is_err()
    );
    assert!(
        git_port
            .discard_moved(dir.path(), "../escape.txt", &snapshot)
            .is_err()
    );
    assert_eq!(read(dir.path(), "f.txt"), "one\n");
}
