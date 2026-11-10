//! The workspace definitions a member's clone sees (D214 (5), D174, D180 (2)) against real
//! repositories in temporary directories: a workspace with two members, each a worktree of its own
//! clone, and the reviewer files its repository keeps in `.git/gitmaster/`. Each test
//! removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::git::SubprocessGit;
use gm_core::workspace_cli::run_with;
use gm_core::workspace_defs::{defs_for_member, workspaces_of_clone};
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

/// The Git common directory of `repo`, absolute.
fn common_of(repo: &Path) -> PathBuf {
    PathBuf::from(git_in(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
}

/// A scratch folder with a global Git configuration that gives commits an identity.
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

    /// Runs `gitmaster workspace <words>` in `cwd`; returns status, stdout and stderr.
    fn run_in(&self, cwd: &Path, words: &[&str]) -> (i32, String, String) {
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
            &mut "".as_bytes(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// `create <name>` under the test root, asserting success.
    fn create(&self, name: &str) -> PathBuf {
        let root = self.root();
        let (code, out, err) = self.run_in(
            &self.path(),
            &["create", name, "--root", root.to_str().unwrap()],
        );
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
        );
        assert_eq!(code, 0, "add failed: {err}");
        PathBuf::from(out.trim_end())
    }
}

#[test]
fn a_listed_folder_that_is_not_a_workspace_and_a_removed_workspace_are_skipped() {
    let s = Scratch::new();
    let a = s.repo("service-a", &["feat/a"]);
    let ws = s.create("pay");
    s.add(&ws, &a, "feat/a");
    let plain = s.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    // A pointer to a folder that is no workspace: listed, but it gives no definitions.
    let listing = common_of(&a).join("gitmaster").join("workspaces");
    let text = std::fs::read_to_string(&listing).unwrap();
    std::fs::write(&listing, format!("{text}{}\n", plain.display())).unwrap();

    let git = SubprocessGit::default();
    let pointed = workspaces_of_clone(&common_of(&a));
    assert_eq!(pointed.len(), 2, "{pointed:?}");
    let seen = defs_for_member(&git, &pointed, &a);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].1, "service-a");

    // A removed workspace is no longer listed, and gives nothing even when named directly.
    std::fs::remove_dir_all(&ws).unwrap();
    let after = workspaces_of_clone(&common_of(&a));
    assert_eq!(after, [plain]);
    assert!(defs_for_member(&git, std::slice::from_ref(&ws), &a).is_empty());
}
