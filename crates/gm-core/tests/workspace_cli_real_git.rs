//! `gitmaster workspace …` (docs/reference/cli.md, "Workspaces") against real repositories in
//! temporary directories. Each test removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::worktree_cli::Env;

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

/// A scratch folder with a global Git configuration that gives commits an identity, so the code
/// under test can commit in a workspace without touching the process environment.
struct Scratch {
    base: tempfile::TempDir,
    identity: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let base = tempfile::tempdir().unwrap();
        let identity = base.path().join("identity.gitconfig");
        std::fs::write(
            &identity,
            "[user]\n\tname = t\n\temail = t@t\n[commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        Self { base, identity }
    }

    fn path(&self) -> PathBuf {
        self.base.path().canonicalize().unwrap()
    }

    fn root(&self) -> PathBuf {
        self.path().join("ws")
    }

    /// A repository `<base>/<name>` on `main` with one commit and the given extra branches.
    fn repo(&self, name: &str, branches: &[&str]) -> PathBuf {
        let repo = self.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        for branch in branches {
            git_in(&repo, &["branch", branch]);
        }
        repo
    }

    fn env(&self, cwd: &Path) -> Env {
        Env {
            cwd: cwd.to_path_buf(),
            home: Some(self.path().join("home")),
            worktrees_home: None,
        }
    }

    /// Runs `gitmaster workspace <words>` in `cwd` with `stdin`; returns status, stdout, stderr.
    fn run_in(&self, cwd: &Path, words: &[&str], stdin: &str) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &self.env(cwd),
            &git,
            Some(&self.identity),
            &mut stdin.as_bytes(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// Runs the command in the scratch folder itself.
    fn run(&self, words: &[&str]) -> (i32, String, String) {
        self.run_in(&self.path(), words, "")
    }

    /// `create <name>` under the test root, asserting success.
    fn create(&self, name: &str) -> PathBuf {
        let root = self.root();
        let (code, out, err) = self.run(&["create", name, "--root", root.to_str().unwrap()]);
        assert_eq!(code, 0, "create failed: {err}");
        let dir = root.join(name);
        assert_eq!(out, format!("{}\n", dir.display()));
        dir
    }

    /// `add <repo> <branch>` into the workspace `ws`, asserting success; returns the member path.
    fn add(&self, ws: &Path, repo: &Path, branch: &str) -> PathBuf {
        let (code, out, err) = self.run_in(
            &self.path(),
            &[
                "add",
                repo.to_str().unwrap(),
                branch,
                "--workspace",
                ws.to_str().unwrap(),
            ],
            "",
        );
        assert_eq!(code, 0, "add failed: {err}");
        PathBuf::from(out.trim_end())
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn worktrees_of(repo: &Path) -> String {
    git_in(repo, &["worktree", "list", "--porcelain"])
}

#[test]
fn create_makes_a_folder_that_is_a_workspace_with_its_kind() {
    let s = Scratch::new();
    let dir = s.create("checkout");
    assert!(dir.join(".git").is_dir());
    assert_eq!(
        git_in(&dir, &["config", "--local", "gitmaster.workspace.kind"]),
        "durable"
    );
    assert_eq!(git_in(&dir, &["symbolic-ref", "--short", "HEAD"]), "main");
}

#[test]
fn create_ephemeral_marks_the_kind() {
    let s = Scratch::new();
    let root = s.root();
    let (code, _, err) = s.run(&[
        "create",
        "scratch",
        "--root",
        root.to_str().unwrap(),
        "--ephemeral",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        git_in(
            &root.join("scratch"),
            &["config", "--local", "gitmaster.workspace.kind"]
        ),
        "ephemeral"
    );
}

#[test]
fn create_refuses_an_existing_folder_and_bad_names() {
    let s = Scratch::new();
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    s.create("taken");
    let (code, _, err) = s.run(&["create", "taken", "--root", root_arg]);
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");
    for bad in ["a/b", ".x", "-x"] {
        let (code, out, _) = s.run(&["create", bad, "--root", root_arg]);
        assert_eq!(code, 1, "name {bad} accepted");
        assert_eq!(out, "", "name {bad} printed a path");
        assert!(!root.join(bad).exists(), "name {bad} created a folder");
    }
}

#[test]
fn add_makes_a_member_that_is_a_worktree_on_its_branch() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");

    assert_eq!(member, ws.join("service-a"));
    assert!(worktrees_of(&repo).contains(&format!("worktree {}", member.display())));
    let gitmodules = std::fs::read_to_string(ws.join(".gitmodules")).unwrap();
    assert!(gitmodules.contains("path = service-a"), "{gitmodules}");
    assert!(
        gitmodules.contains(&format!("url = {}", repo.display())),
        "{gitmodules}"
    );
    assert!(gitmodules.contains("branch = feat/x"), "{gitmodules}");
    assert_eq!(git_in(&ws, &["status", "--porcelain"]), "");
    assert_eq!(
        git_in(&member, &["symbolic-ref", "--short", "HEAD"]),
        "feat/x"
    );
    assert_eq!(
        git_in(&ws, &["log", "-1", "--format=%s"]),
        "Add service-a on feat/x"
    );
}

#[test]
fn a_commit_in_the_member_is_on_the_branch_of_the_original_repository() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    std::fs::write(member.join("b.txt"), "two\n").unwrap();
    git_in(&member, &["add", "b.txt"]);
    git_in(&member, &["commit", "-q", "-m", "member change"]);
    assert_eq!(
        git_in(&repo, &["log", "-1", "--format=%s", "feat/x"]),
        "member change"
    );
}

#[test]
fn add_refuses_a_branch_checked_out_elsewhere_and_says_where() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let elsewhere = s.path().join("elsewhere");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            elsewhere.to_str().unwrap(),
            "feat/x",
        ],
    );
    let ws = s.create("w");
    let (code, _, err) = s.run_in(
        &s.path(),
        &[
            "add",
            repo.to_str().unwrap(),
            "feat/x",
            "--workspace",
            ws.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(code, 1);
    assert!(err.contains(elsewhere.to_str().unwrap()), "{err}");
    assert!(!ws.join("service-a").exists());
}

#[test]
fn add_refuses_a_duplicate_member_and_a_repository_already_a_member() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x", "feat/y"]);
    let ws = s.create("w");
    s.add(&ws, &repo, "feat/x");
    let ws_arg = ws.to_str().unwrap();
    let (code, _, err) = s.run_in(
        &s.path(),
        &[
            "add",
            repo.to_str().unwrap(),
            "feat/y",
            "--workspace",
            ws_arg,
        ],
        "",
    );
    assert_eq!(code, 1);
    assert!(err.contains("already a member"), "{err}");
    let (code, _, err) = s.run_in(
        &s.path(),
        &[
            "add",
            repo.to_str().unwrap(),
            "feat/y",
            "--as",
            "other",
            "--workspace",
            ws_arg,
        ],
        "",
    );
    assert_eq!(code, 1);
    assert!(err.contains("already a member"), "{err}");
}

#[test]
fn add_gives_a_remote_only_branch_a_tracking_branch() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &[]);
    let origin = s.path().join("origin.git");
    git_in(
        &s.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&repo, &["push", "-q", "origin", "main"]);
    git_in(
        &repo,
        &["push", "-q", "origin", "main:refs/heads/remote-feat"],
    );
    git_in(&repo, &["fetch", "-q", "origin"]);
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "remote-feat");
    assert_eq!(
        git_in(
            &member,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]
        ),
        "origin/remote-feat"
    );
}

#[test]
fn create_with_branch_and_repos_from_adds_the_repos_that_have_it_and_skips_the_others() {
    let s = Scratch::new();
    let a = s.repo("service-a", &["feat/x"]);
    let b = s.repo("service-b", &[]);
    let list = s.path().join("repos.txt");
    std::fs::write(&list, format!("{}\n{}\n", a.display(), b.display())).unwrap();
    let root = s.root();
    let (code, out, err) = s.run(&[
        "create",
        "w",
        "--root",
        root.to_str().unwrap(),
        "--branch",
        "feat/x",
        "--repos-from",
        list.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, format!("{}\n", root.join("w").display()));
    assert!(err.contains("skipped service-b"), "{err}");
    assert!(root.join("w").join("service-a").join("a.txt").exists());
    assert!(!root.join("w").join("service-b").exists());
    assert!(worktrees_of(&a).contains("feat/x"));
}

#[test]
fn create_with_repos_from_from_stdin_reads_the_list() {
    let s = Scratch::new();
    let a = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let (code, _, err) = s.run_in(
        &s.path(),
        &[
            "create",
            "w",
            "--root",
            root.to_str().unwrap(),
            "--branch",
            "feat/x",
            "--repos-from",
            "-",
        ],
        &format!("{}\n", a.display()),
    );
    assert_eq!(code, 0, "{err}");
    assert!(root.join("w").join("service-a").is_dir());
}

#[test]
fn list_prints_the_workspaces_in_every_format() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    let w = s.create("alpha");
    s.run(&["create", "beta", "--root", root_arg, "--ephemeral"]);
    s.add(&w, &repo, "feat/x");
    let beta = root.join("beta");
    let alpha_path = w.display().to_string();
    let beta_path = beta.display().to_string();

    let (code, out, _) = s.run(&["list", "--root", root_arg]);
    assert_eq!(code, 0);
    assert_eq!(lines(&out), [alpha_path.as_str(), beta_path.as_str()]);

    let (_, out, _) = s.run(&["list", "--root", root_arg, "--format", "names"]);
    assert_eq!(lines(&out), ["alpha", "beta"]);

    let (_, out, _) = s.run(&["list", "--root", root_arg, "--format", "tsv"]);
    assert_eq!(
        lines(&out),
        [
            format!("alpha\tdurable\t{alpha_path}"),
            format!("beta\tephemeral\t{beta_path}"),
        ]
    );

    let (_, out, _) = s.run(&["list", "--root", root_arg, "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json[0]["name"], "alpha");
    assert_eq!(json[0]["kind"], "durable");
    assert_eq!(json[0]["path"], alpha_path.as_str());
    assert_eq!(json[0]["members"], 1);
    assert_eq!(json[1]["members"], 0);
}

#[test]
fn a_folder_without_the_kind_key_is_not_listed() {
    let s = Scratch::new();
    let root = s.root();
    std::fs::create_dir_all(root.join("plain")).unwrap();
    git_in(&root.join("plain"), &["init", "-q"]);
    s.create("real");
    let (_, out, _) = s.run(&[
        "list",
        "--root",
        root.to_str().unwrap(),
        "--format",
        "names",
    ]);
    assert_eq!(lines(&out), ["real"]);
}

#[test]
fn path_prints_the_workspace_or_exits_one() {
    let s = Scratch::new();
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    s.create("alpha");
    let (code, out, _) = s.run(&["path", "alpha", "--root", root_arg]);
    assert_eq!(code, 0);
    assert_eq!(out, format!("{}\n", root.join("alpha").display()));
    let (code, out, _) = s.run(&["path", "nope", "--root", root_arg]);
    assert_eq!(code, 1);
    assert_eq!(out, "");
}

#[test]
fn status_tsv_and_json_follow_the_member() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    let repo_path = repo.display().to_string();

    let (code, out, _) = s.run_in(
        &s.path(),
        &["status", "--workspace", ws.to_str().unwrap()],
        "",
    );
    assert_eq!(code, 0);
    assert_eq!(out, format!("service-a\tfeat/x\tclean\t0\t{repo_path}\n"));

    std::fs::write(member.join("new.txt"), "x\n").unwrap();
    let (_, out, _) = s.run_in(&ws, &["status"], "");
    assert_eq!(out, format!("service-a\tfeat/x\tdirty\t0\t{repo_path}\n"));
    std::fs::remove_file(member.join("new.txt")).unwrap();

    std::fs::write(member.join("b.txt"), "two\n").unwrap();
    git_in(&member, &["add", "b.txt"]);
    git_in(&member, &["commit", "-q", "-m", "member change"]);
    let (_, out, _) = s.run_in(&ws, &["status", "--format", "json"], "");
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json[0]["member"], "service-a");
    assert_eq!(json[0]["branch"], "feat/x");
    assert_eq!(json[0]["detached"], false);
    assert_eq!(json[0]["dirty"], false);
    assert_eq!(json[0]["new_commits"], 1);
    assert_eq!(json[0]["repo"], repo_path.as_str());
    assert!(json[0]["recorded"].is_string());

    git_in(&member, &["checkout", "-q", "--detach"]);
    let (_, out, _) = s.run_in(&ws, &["status"], "");
    assert!(
        out.starts_with("service-a\t(detached)\tclean\t1\t"),
        "{out}"
    );
}

#[test]
fn status_marks_a_missing_member_and_exits_one() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    std::fs::remove_dir_all(&member).unwrap();
    let (code, out, _) = s.run_in(&ws, &["status"], "");
    assert_eq!(code, 1);
    assert!(out.starts_with("service-a\t(missing)\t"), "{out}");
}

#[test]
fn remove_a_clean_workspace_removes_its_worktrees_and_keeps_the_branches() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    let (code, out, err) = s.run(&["remove", "w", "--root", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, format!("{}\n", ws.display()));
    assert!(!ws.exists());
    assert!(!member.exists());
    assert!(!worktrees_of(&repo).contains(member.to_str().unwrap()));
    git_in(&repo, &["rev-parse", "--verify", "feat/x"]);
}

#[test]
fn remove_refuses_a_dirty_member_and_force_removes_it() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    std::fs::write(member.join("scratch.txt"), "x\n").unwrap();
    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg]);
    assert_eq!(code, 1);
    assert!(
        err.contains("service-a") && err.contains("uncommitted"),
        "{err}"
    );
    assert!(ws.exists() && member.exists());

    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg, "--force"]);
    assert_eq!(code, 0, "{err}");
    assert!(!ws.exists());
}

#[test]
fn remove_refuses_a_change_of_the_workspace_itself_and_force_removes_it() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    let ws = s.create("w");
    s.add(&ws, &repo, "feat/x");
    std::fs::write(ws.join("notes.txt"), "x\n").unwrap();
    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg]);
    assert_eq!(code, 1);
    assert!(err.contains("notes.txt"), "{err}");
    assert!(ws.exists());

    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg, "--force"]);
    assert_eq!(code, 0, "{err}");
    assert!(!ws.exists());
}

#[test]
fn remove_refuses_a_detached_member_with_an_orphan_commit_and_force_removes_it() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let root_arg = root.to_str().unwrap();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    git_in(&member, &["checkout", "-q", "--detach"]);
    std::fs::write(member.join("orphan.txt"), "x\n").unwrap();
    git_in(&member, &["add", "orphan.txt"]);
    git_in(&member, &["commit", "-q", "-m", "orphan"]);
    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg]);
    assert_eq!(code, 1);
    assert!(err.contains("detached HEAD"), "{err}");
    assert!(member.exists());

    let (code, _, err) = s.run(&["remove", "w", "--root", root_arg, "--force"]);
    assert_eq!(code, 0, "{err}");
    assert!(!ws.exists());
}

#[test]
fn remove_refuses_from_inside_the_workspace_even_with_force() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    let (code, _, err) = s.run_in(
        &member,
        &["remove", "w", "--root", root.to_str().unwrap(), "--force"],
        "",
    );
    assert_eq!(code, 1);
    assert!(err.contains("inside that workspace"), "{err}");
    assert!(ws.exists());
}

#[test]
fn a_member_commit_alone_does_not_block_removal() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    std::fs::write(member.join("b.txt"), "two\n").unwrap();
    git_in(&member, &["add", "b.txt"]);
    git_in(&member, &["commit", "-q", "-m", "member change"]);
    let (code, _, err) = s.run(&["remove", "w", "--root", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(!ws.exists());
    assert_eq!(
        git_in(&repo, &["log", "-1", "--format=%s", "feat/x"]),
        "member change"
    );
}

#[test]
fn remove_prunes_the_record_of_a_member_whose_folder_is_gone() {
    let s = Scratch::new();
    let repo = s.repo("service-a", &["feat/x"]);
    let root = s.root();
    let ws = s.create("w");
    let member = s.add(&ws, &repo, "feat/x");
    std::fs::remove_dir_all(&member).unwrap();
    let (code, _, err) = s.run(&["remove", "w", "--root", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(!ws.exists());
    assert!(!worktrees_of(&repo).contains(member.to_str().unwrap()));
}
