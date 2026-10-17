//! Creating a linked worktree with each mode against real repositories in
//! temporary directories. Each test removes its directory when it ends.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::WorktreeMode;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository with one commit on `main`, in a fresh directory under `base`.
fn repo(base: &Path) -> std::path::PathBuf {
    let main = base.join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    main
}

fn canonical(path: &Path) -> String {
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

fn git_for(log: &CommandLog) -> SubprocessGit {
    SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    }
}

#[test]
fn an_existing_branch_gets_a_worktree_on_that_branch() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    git_in(&main, &["branch", "feature"]);
    let target = base.path().join("repo-feature");
    let log = CommandLog::default();

    git_for(&log)
        .create_worktree(
            &main,
            target.to_str().unwrap(),
            &WorktreeMode::Existing {
                branch: "feature".into(),
            },
        )
        .unwrap();

    let head = git_in(&target, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(head, "feature");
    let listed = git_in(&main, &["worktree", "list", "--porcelain"]);
    assert!(
        listed.contains(&format!("worktree {}", canonical(&target))),
        "{listed}"
    );
    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].succeeded());
    assert!(
        entries[0].line().contains("worktree add"),
        "{}",
        entries[0].line()
    );
}

#[test]
fn a_new_branch_is_created_at_the_base_and_checked_out() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let first = git_in(&main, &["rev-parse", "HEAD"]);
    git_in(&main, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let target = base.path().join("repo-new");

    git_for(&CommandLog::default())
        .create_worktree(
            &main,
            target.to_str().unwrap(),
            &WorktreeMode::New {
                branch: "topic/new".into(),
                base: first.clone(),
            },
        )
        .unwrap();

    assert_eq!(
        git_in(&target, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "topic/new"
    );
    assert_eq!(git_in(&main, &["rev-parse", "topic/new"]), first);
}

#[test]
fn a_detached_worktree_has_no_branch() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let target = base.path().join("repo-detached");

    git_for(&CommandLog::default())
        .create_worktree(
            &main,
            target.to_str().unwrap(),
            &WorktreeMode::Detached {
                commit: "main".into(),
            },
        )
        .unwrap();

    assert_eq!(
        git_in(&target, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD"
    );
    assert_eq!(
        git_in(&target, &["rev-parse", "HEAD"]),
        git_in(&main, &["rev-parse", "main"])
    );
}

#[test]
fn a_branch_checked_out_elsewhere_fails_with_gits_own_message() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let target = base.path().join("repo-main");
    let direct = Command::new("git")
        .args(["worktree", "add", target.to_str().unwrap(), "main"])
        .current_dir(&main)
        .output()
        .unwrap();
    assert!(
        !direct.status.success(),
        "git itself should refuse a second checkout of main"
    );
    let git_says = String::from_utf8_lossy(&direct.stderr).trim().to_owned();
    let log = CommandLog::default();

    let error = git_for(&log)
        .create_worktree(
            &main,
            target.to_str().unwrap(),
            &WorktreeMode::Existing {
                branch: "main".into(),
            },
        )
        .unwrap_err();

    assert_eq!(error.0, git_says);
    assert!(!target.exists());
    assert_eq!(log.entries().len(), 1, "a refused command is still logged");
    assert!(!log.entries()[0].succeeded());
}

#[test]
fn a_path_that_exists_is_refused_even_when_empty() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let full = base.path().join("full");
    std::fs::create_dir(&full).unwrap();
    std::fs::write(full.join("keep.txt"), "mine\n").unwrap();
    let empty = base.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let git = git_for(&CommandLog::default());

    for path in [&full, &empty] {
        let error = git
            .create_worktree(
                &main,
                path.to_str().unwrap(),
                &WorktreeMode::Detached {
                    commit: "main".into(),
                },
            )
            .unwrap_err();
        assert_eq!(error.0, format!("'{}' already exists", path.display()));
    }
    assert_eq!(
        std::fs::read_to_string(full.join("keep.txt")).unwrap(),
        "mine\n"
    );
}

#[test]
fn an_unknown_branch_is_refused_by_git() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());
    let target = base.path().join("repo-ghost");

    let error = git_for(&CommandLog::default())
        .create_worktree(
            &main,
            target.to_str().unwrap(),
            &WorktreeMode::Existing {
                branch: "ghost".into(),
            },
        )
        .unwrap_err();

    assert!(error.0.contains("ghost"), "{}", error.0);
    assert!(!target.exists());
}

#[test]
fn a_leading_dash_is_never_read_as_an_option() {
    let base = tempfile::tempdir().unwrap();
    let main = repo(base.path());

    let error = git_for(&CommandLog::default())
        .create_worktree(
            &main,
            base.path().join("x").to_str().unwrap(),
            &WorktreeMode::Existing {
                branch: "--force".into(),
            },
        )
        .unwrap_err();

    assert_eq!(error.0, "invalid worktree path or revision");
}
