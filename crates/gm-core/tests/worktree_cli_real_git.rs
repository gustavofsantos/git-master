//! `gitmaster worktree …` (docs/reference/cli.md) against real repositories in temporary
//! directories. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::worktree_cli::{Env, run};

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Process::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
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

/// A repository `<base>/repo` with one commit on `main` and a `.gitignore` for `.env`,
/// `node_modules/` and `secrets/`.
struct Fixture {
    base: tempfile::TempDir,
    main: PathBuf,
}

fn fixture() -> Fixture {
    let base = tempfile::tempdir().unwrap();
    let base_path = base.path().canonicalize().unwrap();
    let main = base_path.join("repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    std::fs::write(main.join(".gitignore"), ".env\nnode_modules/\nsecrets/\n").unwrap();
    git_in(&main, &["add", "."]);
    git_in(&main, &["commit", "-q", "-m", "init"]);
    Fixture { base, main }
}

impl Fixture {
    fn env(&self, cwd: &Path) -> Env {
        Env {
            cwd: cwd.to_path_buf(),
            home: Some(self.base.path().join("home")),
            worktrees_home: None,
        }
    }

    /// Runs `gitmaster worktree <words>` in `cwd`; returns the status, standard output and error.
    fn run(&self, env: &Env, words: &[&str]) -> (i32, String, String) {
        self.run_with(env, words, "")
    }

    /// `run` with `stdin` as the standard input.
    fn run_with(&self, env: &Env, words: &[&str], stdin: &str) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.base.path().join("no-global-config")),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&words, env, &git, &mut stdin.as_bytes(), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn worktrees_home(&self) -> PathBuf {
        self.base.path().join("wt")
    }

    fn env_with_home(&self) -> Env {
        Env {
            worktrees_home: Some(self.worktrees_home()),
            ..self.env(&self.main)
        }
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn add_creates_a_sibling_worktree_by_default() {
    let f = fixture();
    let (code, out, _) = f.run(&f.env(&f.main), &["add", "feature/login"]);
    assert_eq!(code, 0);
    let expected = f.main.parent().unwrap().join("repo-feature-login");
    assert_eq!(out, format!("{}\n", expected.display()));
    assert_eq!(
        git_in(&expected, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feature/login"
    );
}

#[test]
fn add_puts_worktrees_under_worktrees_home() {
    let f = fixture();
    let (code, out, _) = f.run(&f.env_with_home(), &["add", "feature/login"]);
    assert_eq!(code, 0);
    let expected = f.worktrees_home().join("repo").join("feature-login");
    assert_eq!(out.trim_end(), expected.to_str().unwrap());
    assert!(expected.join("a.txt").exists());
}

#[test]
fn the_git_config_root_beats_worktrees_home_and_the_flag_beats_both() {
    let f = fixture();
    let configured = f.base.path().join("configured");
    git_in(
        &f.main,
        &[
            "config",
            "gitmaster.worktree.root",
            configured.to_str().unwrap(),
        ],
    );
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "one"]);
    assert_eq!(
        out.trim_end(),
        configured.join("repo/one").to_str().unwrap()
    );
    let flagged = f.base.path().join("flagged");
    let (_, out, _) = f.run(
        &f.env_with_home(),
        &["add", "two", "--root", flagged.to_str().unwrap()],
    );
    assert_eq!(out.trim_end(), flagged.join("repo/two").to_str().unwrap());
}

#[test]
fn a_tilde_in_the_root_is_the_home_directory() {
    let f = fixture();
    let (_, out, _) = f.run(&f.env(&f.main), &["add", "x", "--root", "~/trees"]);
    let expected = f.base.path().join("home/trees/repo/x");
    assert_eq!(out.trim_end(), expected.to_str().unwrap());
}

#[test]
fn add_twice_prints_the_same_path_and_changes_nothing() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, first, _) = f.run(&env, &["add", "feature"]);
    let (code, second, _) = f.run(&env, &["add", "feature"]);
    assert_eq!(code, 0);
    assert_eq!(first, second);
}

#[test]
fn add_works_from_inside_another_worktree() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, first, _) = f.run(&env, &["add", "one"]);
    let inside = Env {
        cwd: PathBuf::from(first.trim_end()),
        ..env
    };
    let (code, second, _) = f.run(&inside, &["add", "two"]);
    assert_eq!(code, 0);
    // The project is the main checkout's name, not the worktree's.
    assert_eq!(
        second.trim_end(),
        f.worktrees_home().join("repo/two").to_str().unwrap()
    );
}

#[test]
fn add_checks_out_an_existing_local_branch() {
    let f = fixture();
    git_in(&f.main, &["branch", "topic"]);
    let (code, out, _) = f.run(&f.env_with_home(), &["add", "topic"]);
    assert_eq!(code, 0);
    assert_eq!(
        git_in(
            Path::new(out.trim_end()),
            &["rev-parse", "--abbrev-ref", "HEAD"]
        ),
        "topic"
    );
}

#[test]
fn add_creates_a_new_branch_from_the_base() {
    let f = fixture();
    git_in(&f.main, &["checkout", "-q", "-b", "other"]);
    std::fs::write(f.main.join("b.txt"), "two\n").unwrap();
    git_in(&f.main, &["add", "."]);
    git_in(&f.main, &["commit", "-q", "-m", "second"]);
    git_in(&f.main, &["checkout", "-q", "main"]);
    let (code, out, _) = f.run(&f.env_with_home(), &["add", "child", "other"]);
    assert_eq!(code, 0);
    assert!(Path::new(out.trim_end()).join("b.txt").exists());
}

#[test]
fn add_tracks_a_remote_branch_when_no_local_one_exists() {
    let f = fixture();
    let remote = f.base.path().join("remote.git");
    git_in(
        f.base.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git_in(
        &f.main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(&f.main, &["push", "-q", "origin", "main:feature/remote"]);
    git_in(&f.main, &["fetch", "-q", "origin"]);
    let (code, out, _) = f.run(&f.env_with_home(), &["add", "feature/remote"]);
    assert_eq!(code, 0);
    let tree = Path::new(out.trim_end());
    assert_eq!(
        git_in(
            tree,
            &["rev-parse", "--abbrev-ref", "feature/remote@{upstream}"]
        ),
        "origin/feature/remote"
    );
}

#[test]
fn add_refuses_a_bad_branch_name_and_an_existing_folder() {
    let f = fixture();
    let env = f.env_with_home();
    let (code, _, err) = f.run(&env, &["add", "bad..name"]);
    assert_eq!(code, 1);
    assert!(err.contains("not a valid branch name"), "{err}");
    std::fs::create_dir_all(f.worktrees_home().join("repo/taken")).unwrap();
    let (code, _, err) = f.run(&env, &["add", "taken"]);
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn list_prints_paths_branches_and_pairs() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, one, _) = f.run(&env, &["add", "one"]);
    let main = f.main.to_str().unwrap();

    let (_, paths, _) = f.run(&env, &["list"]);
    assert_eq!(lines(&paths), [main, one.trim_end()]);

    let (_, linked, _) = f.run(&env, &["list", "--linked"]);
    assert_eq!(lines(&linked), [one.trim_end()]);

    let (_, branches, _) = f.run(&env, &["list", "--format", "branches"]);
    assert_eq!(lines(&branches), ["main", "one"]);

    let (_, tsv, _) = f.run(&env, &["list", "--format=tsv"]);
    assert_eq!(
        lines(&tsv),
        [format!("main\t{main}"), format!("one\t{}", one.trim_end())]
    );

    let (_, json, _) = f.run(&env, &["list", "--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed[1]["branch"], "one");
    assert_eq!(parsed[0]["is_main"], true);
}

#[test]
fn list_leaves_out_a_worktree_whose_folder_is_gone() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, one, _) = f.run(&env, &["add", "one"]);
    std::fs::remove_dir_all(one.trim_end()).unwrap();
    let (_, paths, _) = f.run(&env, &["list"]);
    assert_eq!(lines(&paths), [f.main.to_str().unwrap()]);
}

#[test]
fn path_finds_the_worktree_of_a_branch() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, one, _) = f.run(&env, &["add", "one"]);
    let (code, out, _) = f.run(&env, &["path", "one"]);
    assert_eq!((code, out), (0, one));
    let (code, out, err) = f.run(&env, &["path", "missing"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("no worktree has branch 'missing'"), "{err}");
}

#[test]
fn branches_lists_local_and_remote_only_names_once() {
    let f = fixture();
    let remote = f.base.path().join("remote.git");
    git_in(
        f.base.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git_in(
        &f.main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(
        &f.main,
        &["push", "-q", "origin", "main", "main:feature/remote"],
    );
    git_in(&f.main, &["fetch", "-q", "origin"]);
    git_in(&f.main, &["branch", "local"]);
    let env = f.env_with_home();
    let (code, out, _) = f.run(&env, &["branches"]);
    assert_eq!(code, 0);
    assert_eq!(lines(&out), ["local", "main", "feature/remote"]);

    f.run(&env, &["add", "local"]);
    let (_, missing, _) = f.run(&env, &["branches", "--missing"]);
    assert_eq!(lines(&missing), ["feature/remote"]);
}

#[test]
fn wrong_command_lines_exit_two() {
    let f = fixture();
    let env = f.env(&f.main);
    for words in [
        &[][..],
        &["nope"],
        &["add"],
        &["add", "a", "b", "c"],
        &["list", "--format", "xml"],
        &["list", "--bogus"],
        &["path"],
        &["add", "x", "--include", "copy"],
    ] {
        let (code, _, _) = f.run(&env, words);
        assert_eq!(code, 2, "{words:?}");
    }
}

#[test]
fn outside_a_repository_the_command_fails() {
    let f = fixture();
    let outside = f.base.path().join("home");
    std::fs::create_dir_all(&outside).unwrap();
    let (code, _, err) = f.run(&f.env(&outside), &["list"]);
    assert_eq!(code, 1);
    assert!(err.contains("not inside a git repository"), "{err}");
}

// .worktreeinclude

/// Writes `.worktreeinclude` and the ignored files it should pick up.
fn with_include(f: &Fixture) {
    std::fs::write(
        f.main.join(".worktreeinclude"),
        "# shared\n.env\nnode_modules/\n",
    )
    .unwrap();
    std::fs::write(f.main.join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(f.main.join("node_modules/pkg")).unwrap();
    std::fs::write(f.main.join("node_modules/pkg/index.js"), "x").unwrap();
    std::fs::create_dir_all(f.main.join("secrets")).unwrap();
    std::fs::write(f.main.join("secrets/.env"), "NESTED=1\n").unwrap();
    std::fs::write(f.main.join("secrets/other"), "no\n").unwrap();
    git_in(&f.main, &["add", ".worktreeinclude"]);
    git_in(&f.main, &["commit", "-q", "-m", "share files"]);
}

#[test]
fn add_links_the_included_ignored_files_instead_of_copying() {
    let f = fixture();
    with_include(&f);
    let (code, out, err) = f.run(&f.env_with_home(), &["add", "feature"]);
    assert_eq!(code, 0, "{err}");
    let tree = PathBuf::from(out.trim_end());

    let env_file = tree.join(".env");
    assert!(env_file.symlink_metadata().unwrap().is_symlink());
    assert_eq!(std::fs::read_link(&env_file).unwrap(), f.main.join(".env"));
    assert_eq!(std::fs::read_to_string(&env_file).unwrap(), "SECRET=1\n");

    // A folder pattern is one link to the folder.
    let modules = tree.join("node_modules");
    assert!(modules.symlink_metadata().unwrap().is_symlink());
    assert!(modules.join("pkg/index.js").exists());

    // `.env` is found inside the ignored `secrets/`, which is then a real folder.
    let nested = tree.join("secrets/.env");
    assert!(nested.symlink_metadata().unwrap().is_symlink());
    assert!(
        !tree
            .join("secrets")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert!(!tree.join("secrets/other").exists());

    assert!(err.contains("linked .env"), "{err}");
    assert!(err.contains("linked node_modules/"), "{err}");
}

#[test]
fn linked_folders_stay_out_of_git_status_in_every_worktree() {
    let f = fixture();
    with_include(&f);
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    // `node_modules/` ignores a folder, not a link to one: info/exclude covers the link.
    assert_eq!(git_in(&tree, &["status", "--porcelain"]), "");
    assert_eq!(git_in(&f.main, &["status", "--porcelain"]), "");
}

#[test]
fn a_tracked_file_is_never_shared() {
    let f = fixture();
    std::fs::write(f.main.join(".worktreeinclude"), "a.txt\n.env\n").unwrap();
    std::fs::write(f.main.join(".env"), "k\n").unwrap();
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    assert!(!tree.join("a.txt").symlink_metadata().unwrap().is_symlink());
    assert!(tree.join(".env").symlink_metadata().unwrap().is_symlink());
}

#[test]
fn nothing_is_shared_without_the_file_or_with_include_none() {
    let f = fixture();
    std::fs::write(f.main.join(".env"), "k\n").unwrap();
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "plain"]);
    assert!(!Path::new(out.trim_end()).join(".env").exists());

    std::fs::write(f.main.join(".worktreeinclude"), ".env\n").unwrap();
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "off", "--include", "none"]);
    assert!(!Path::new(out.trim_end()).join(".env").exists());

    git_in(&f.main, &["config", "gitmaster.worktree.include", "none"]);
    let (_, out, _) = f.run(&f.env_with_home(), &["add", "off2"]);
    assert!(!Path::new(out.trim_end()).join(".env").exists());
}

#[test]
fn a_bad_pattern_warns_and_the_worktree_is_still_made() {
    let f = fixture();
    std::fs::write(f.main.join(".worktreeinclude"), "[z-a]\n").unwrap();
    let (code, out, err) = f.run(&f.env_with_home(), &["add", "feature"]);
    assert_eq!(code, 0);
    assert!(Path::new(out.trim_end()).exists());
    assert!(err.contains(".worktreeinclude:1:"), "{err}");
}

#[test]
fn reflink_clones_or_falls_back_to_a_link() {
    let f = fixture();
    with_include(&f);
    let (code, out, err) = f.run(
        &f.env_with_home(),
        &["add", "feature", "--include", "reflink"],
    );
    assert_eq!(code, 0, "{err}");
    let tree = PathBuf::from(out.trim_end());
    // Whichever the file system allows, the content is there and no plain copy was made silently.
    assert_eq!(
        std::fs::read_to_string(tree.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    let cloned = !tree.join(".env").symlink_metadata().unwrap().is_symlink();
    assert_eq!(cloned, err.contains("cloned .env"), "{err}");
    if !cloned {
        assert!(err.contains("cannot clone"), "{err}");
    }
}

#[test]
fn include_applies_to_an_existing_worktree_and_skips_what_is_there() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, out, _) = f.run(&env, &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    with_include(&f);

    let (code, _, err) = f.run(&env, &["include", "feature", "--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("would link .env"), "{err}");
    assert!(!tree.join(".env").exists());

    let (code, _, err) = f.run(&env, &["include", "feature"]);
    assert_eq!(code, 0, "{err}");
    assert!(tree.join(".env").symlink_metadata().unwrap().is_symlink());

    let (_, _, err) = f.run(&env, &["include", "feature"]);
    assert!(err.contains("skipped .env: already exists"), "{err}");
}

#[test]
fn include_refuses_the_main_checkout() {
    let f = fixture();
    let (code, _, err) = f.run(&f.env(&f.main), &["include"]);
    assert_eq!(code, 1);
    assert!(err.contains("main checkout"), "{err}");
}

// remove

#[test]
fn remove_deletes_the_worktree_keeps_the_branch_and_tidies_the_project_folder() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, out, _) = f.run(&env, &["add", "feature/x"]);
    let tree = PathBuf::from(out.trim_end());
    let (code, out, _) = f.run(&env, &["remove", "feature/x"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim_end(), tree.to_str().unwrap());
    assert!(!tree.exists());
    assert!(!f.worktrees_home().join("repo").exists());
    assert_eq!(
        git_in(&f.main, &["branch", "--list", "feature/x"]),
        "feature/x"
    );
    let (_, paths, _) = f.run(&env, &["list"]);
    assert_eq!(lines(&paths), [f.main.to_str().unwrap()]);
}

#[test]
fn remove_leaves_the_files_it_linked_in_the_main_checkout() {
    let f = fixture();
    with_include(&f);
    let env = f.env_with_home();
    let (_, out, _) = f.run(&env, &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    let (code, _, err) = f.run(&env, &["remove", "feature"]);
    assert_eq!(code, 0, "{err}");
    assert!(!tree.exists());
    assert_eq!(
        std::fs::read_to_string(f.main.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(f.main.join("node_modules/pkg/index.js").exists());
}

#[test]
fn remove_refuses_changes_until_forced() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, out, _) = f.run(&env, &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    std::fs::write(tree.join("new.txt"), "x").unwrap();
    let (code, _, err) = f.run(&env, &["remove", "feature"]);
    assert_eq!(code, 1);
    assert!(err.contains("uncommitted or untracked"), "{err}");
    assert!(tree.exists());
    let (code, _, _) = f.run(&env, &["remove", "feature", "--force"]);
    assert_eq!(code, 0);
    assert!(!tree.exists());
}

#[test]
fn remove_refuses_unpushed_commits_and_a_lock_until_forced() {
    let f = fixture();
    let env = f.env_with_home();
    let (_, out, _) = f.run(&env, &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    std::fs::write(tree.join("c.txt"), "x").unwrap();
    git_in(&tree, &["add", "."]);
    git_in(&tree, &["commit", "-q", "-m", "only here"]);
    let (code, _, err) = f.run(&env, &["remove", "feature"]);
    assert_eq!(code, 1);
    assert!(err.contains("no other branch or remote"), "{err}");

    git_in(&f.main, &["worktree", "lock", tree.to_str().unwrap()]);
    let (_, _, err) = f.run(&env, &["remove", "feature"]);
    assert!(err.contains("it is locked"), "{err}");
    let (code, _, err) = f.run(&env, &["remove", "feature", "--force"]);
    assert_eq!(code, 0, "{err}");
    assert!(!tree.exists());
}

#[test]
fn remove_never_takes_the_main_checkout_or_the_directory_it_runs_in() {
    let f = fixture();
    let env = f.env_with_home();
    let (code, _, err) = f.run(&env, &["remove", "main", "--force"]);
    assert_eq!(code, 1);
    assert!(err.contains("main checkout"), "{err}");

    let (_, out, _) = f.run(&env, &["add", "feature"]);
    let tree = PathBuf::from(out.trim_end());
    let inside = Env {
        cwd: tree.clone(),
        ..env.clone()
    };
    let (code, _, err) = f.run(&inside, &["remove", "feature", "--force"]);
    assert_eq!(code, 1);
    assert!(err.contains("inside that worktree"), "{err}");
    assert!(tree.exists());

    let (code, _, err) = f.run(&env, &["remove", "unknown"]);
    assert_eq!(code, 1);
    assert!(err.contains("no worktree has branch"), "{err}");
    let (code, _, _) = f.run(&env, &["remove"]);
    assert_eq!(code, 2);
}

// several repositories

/// A second repository `<base>/billing` with a branch `topic`, a folder that is no repository, and
/// the list a caller would pass: both repositories, the plain folder, and a linked worktree of the
/// first repository.
fn with_list(f: &Fixture) -> (PathBuf, String) {
    let billing = f.main.parent().unwrap().join("billing");
    std::fs::create_dir(&billing).unwrap();
    git_in(&billing, &["init", "-q", "-b", "main"]);
    git_in(&billing, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git_in(&billing, &["branch", "topic"]);
    let plain = f.base.path().canonicalize().unwrap().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let (_, linked, _) = f.run(&f.env_with_home(), &["add", "feature"]);
    let list = format!(
        "{}\n\n{}\n{}\n{}\n",
        f.main.display(),
        billing.display(),
        plain.display(),
        linked.trim_end(),
    );
    (billing, list)
}

fn elsewhere(f: &Fixture) -> Env {
    let cwd = f.base.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    f.env(&cwd)
}

#[test]
fn a_list_on_stdin_gives_every_worktree_once_with_the_project_first() {
    let f = fixture();
    let (billing, list) = with_list(&f);
    let env = elsewhere(&f);
    let (code, out, err) = f.run_with(
        &env,
        &["list", "--repos-from", "-", "--format", "tsv"],
        &list,
    );
    assert_eq!(code, 0, "{err}");
    let feature = f.worktrees_home().join("repo/feature");
    assert_eq!(
        lines(&out),
        [
            format!("repo\tmain\t{}", f.main.display()),
            format!("repo\tfeature\t{}", feature.display()),
            format!("billing\tmain\t{}", billing.display()),
        ]
    );
    // The linked worktree in the list did not add its repository a second time, and the plain
    // folder was skipped without a word.
    assert_eq!(err, "");
    let (_, paths, _) = f.run_with(&env, &["list", "--repos-from", "-"], &list);
    assert_eq!(paths.lines().count(), 3, "{paths}");
}

#[test]
fn a_list_in_a_file_works_and_a_relative_path_is_taken_from_the_current_directory() {
    let f = fixture();
    let (billing, _) = with_list(&f);
    let env = elsewhere(&f);
    std::fs::write(
        env.cwd.join("repos.txt"),
        format!("{}\n~/../billing\n", billing.display()),
    )
    .unwrap();
    let (code, out, err) = f.run(
        &env,
        &["list", "--repos-from", "repos.txt", "--format", "branches"],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(lines(&out), ["billing\tmain"]);
    let (code, _, err) = f.run(&env, &["list", "--repos-from", "missing.txt"]);
    assert_eq!(code, 1);
    assert!(err.contains("cannot read 'missing.txt'"), "{err}");
}

#[test]
fn repo_works_on_a_folder_without_changing_the_format() {
    let f = fixture();
    let (billing, _) = with_list(&f);
    let env = elsewhere(&f);
    let (code, out, err) = f.run(
        &env,
        &[
            "list",
            "--repo",
            billing.to_str().unwrap(),
            "--format",
            "tsv",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, format!("main\t{}\n", billing.display()));
    // More than one `--repo` is a list too.
    let (_, out, _) = f.run(
        &env,
        &[
            "list",
            "--repo",
            billing.to_str().unwrap(),
            "--repo",
            f.main.to_str().unwrap(),
            "--format",
            "branches",
        ],
    );
    assert_eq!(
        lines(&out),
        ["billing\tmain", "repo\tmain\nrepo\tfeature"]
            .iter()
            .flat_map(|l| l.split('\n'))
            .collect::<Vec<_>>()
    );
}

#[test]
fn branches_across_repositories_carry_the_project_and_the_checkout() {
    let f = fixture();
    let (billing, list) = with_list(&f);
    let (code, out, err) = f.run_with(
        &elsewhere(&f),
        &["branches", "--repos-from", "-", "--missing"],
        &list,
    );
    assert_eq!(code, 0, "{err}");
    // Every `main` has the main checkout, and `feature` has its worktree: only `topic` is missing.
    assert_eq!(
        lines(&out),
        [format!("billing\ttopic\t{}", billing.display())]
    );
}

#[test]
fn json_across_repositories_gains_the_project_and_the_repository() {
    let f = fixture();
    let (_, list) = with_list(&f);
    let (_, out, _) = f.run_with(
        &elsewhere(&f),
        &["list", "--repos-from", "-", "--format", "json"],
        &list,
    );
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 3);
    assert_eq!(parsed[0]["project"], "repo");
    assert_eq!(parsed[2]["repo"], parsed[2]["path"]);
}

#[test]
fn add_path_and_remove_work_on_another_repository() {
    let f = fixture();
    let (billing, _) = with_list(&f);
    let env = Env {
        worktrees_home: Some(f.worktrees_home()),
        ..elsewhere(&f)
    };
    let repo = billing.to_str().unwrap();
    let (code, out, err) = f.run(&env, &["add", "topic", "--repo", repo]);
    assert_eq!(code, 0, "{err}");
    let tree = f.worktrees_home().join("billing/topic");
    assert_eq!(out.trim_end(), tree.to_str().unwrap());
    let (_, found, _) = f.run(&env, &["path", "topic", "--repo", repo]);
    assert_eq!(found, out);
    let (code, _, err) = f.run(&env, &["remove", "topic", "--repo", repo]);
    assert_eq!(code, 0, "{err}");
    assert!(!tree.exists());
}

#[test]
fn the_repository_decides_the_worktree_settings_of_a_remote_add() {
    let f = fixture();
    let (billing, _) = with_list(&f);
    let root = f.base.path().join("billing-root");
    git_in(
        &billing,
        &["config", "gitmaster.worktree.root", root.to_str().unwrap()],
    );
    let (_, out, _) = f.run(
        &elsewhere(&f),
        &["add", "x", "--repo", billing.to_str().unwrap()],
    );
    assert_eq!(out.trim_end(), root.join("billing/x").to_str().unwrap());
}

#[test]
fn commands_that_change_things_take_one_repository() {
    let f = fixture();
    let (billing, list) = with_list(&f);
    let repo = billing.to_str().unwrap();
    for words in [
        &["add", "x", "--repos-from", "-"][..],
        &["add", "x", "--repo", repo, "--repo", repo],
        &["remove", "x", "--repos-from", "-"],
        &["path", "x", "--repos-from", "-"],
    ] {
        let (code, _, err) = f.run_with(&elsewhere(&f), words, &list);
        assert_eq!(code, 2, "{words:?}: {err}");
    }
}

#[test]
fn a_bad_repo_is_an_error_but_a_bad_folder_in_a_list_is_skipped() {
    let f = fixture();
    let env = elsewhere(&f);
    let (code, _, err) = f.run(&env, &["list", "--repo", "nowhere"]);
    assert_eq!(code, 1);
    assert!(err.contains("'nowhere' is not a folder"), "{err}");
    let plain = f.base.path().canonicalize().unwrap().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let (code, _, err) = f.run(&env, &["list", "--repo", plain.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("not inside a git repository"), "{err}");
    let (code, out, _) = f.run_with(
        &env,
        &["list", "--repos-from", "-"],
        &format!("{}\n", plain.display()),
    );
    assert_eq!((code, out.as_str()), (0, ""));
    let (code, _, err) = f.run_with(&env, &["list", "--repos-from", "-"], "\n");
    assert_eq!(code, 0);
    assert!(err.contains("list of repositories is empty"), "{err}");
}

#[test]
fn nothing_reads_a_projects_file() {
    let f = fixture();
    let env = elsewhere(&f);
    std::fs::write(
        env.home.as_ref().unwrap().join(".projects"),
        format!("{}\n", f.main.display()),
    )
    .unwrap();
    // The list is the caller's: with none given the command works on the current directory.
    let (code, _, err) = f.run(&env, &["list"]);
    assert_eq!(code, 1);
    assert!(err.contains("not inside a git repository"), "{err}");
    let (code, _, _) = f.run(&env, &["list", "--all"]);
    assert_eq!(code, 2);
}
