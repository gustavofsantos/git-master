//! Following members and Update Workspace (D269; docs/guide/workspaces.md, "Share a repository
//! between workspaces" and "Update every member"; docs/reference/cli.md, "Workspaces") against
//! real repositories in temporary directories.

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

/// A scratch folder with an identity for the commits the code under test makes, a repository
/// `shared` with a bare `origin`, and a second clone of that origin to push new commits from.
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

    /// A repository `<base>/<name>` on `main` with one commit, pushed to a bare `<name>.git` as
    /// `origin`.
    fn repo_with_origin(&self, name: &str) -> PathBuf {
        let repo = self.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        let origin = self.path().join(format!("{name}.git"));
        git_in(
            &self.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        );
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&repo, &["push", "-q", "-u", "origin", "main"]);
        repo
    }

    /// Pushes a new commit on `branch` of `<name>.git` from a clone of its own, as a colleague
    /// would. Returns the new commit.
    fn push_upstream(&self, name: &str, branch: &str, file: &str) -> String {
        let work = self.path().join(format!("{name}-upstream"));
        if !work.exists() {
            let origin = self.path().join(format!("{name}.git"));
            git_in(
                &self.path(),
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    work.to_str().unwrap(),
                ],
            );
        }
        git_in(&work, &["fetch", "-q", "origin"]);
        git_in(
            &work,
            &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
        );
        std::fs::write(work.join(file), "x\n").unwrap();
        git_in(&work, &["add", file]);
        git_in(&work, &["commit", "-q", "-m", file]);
        git_in(&work, &["push", "-q", "origin", branch]);
        git_in(&work, &["rev-parse", "HEAD"])
    }

    fn run(&self, words: &[&str]) -> (i32, String, String) {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let git = SubprocessGit {
            global_config: Some(self.identity.clone()),
            ..SubprocessGit::default()
        };
        let env = Env {
            cwd: self.path(),
            home: Some(self.path().join("home")),
            worktrees_home: None,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            &words,
            &env,
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

    fn ok(&self, words: &[&str]) -> String {
        let (code, out, err) = self.run(words);
        assert_eq!(code, 0, "{words:?} failed: {err}");
        out
    }

    fn create(&self, name: &str) -> PathBuf {
        let root = self.path().join("ws");
        let out = self.ok(&["create", name, "--root", root.to_str().unwrap()]);
        PathBuf::from(out.trim_end())
    }

    fn follow(&self, ws: &Path, repo: &Path, follow: &str) -> PathBuf {
        let out = self.ok(&[
            "add",
            repo.to_str().unwrap(),
            "--follow",
            follow,
            "--workspace",
            ws.to_str().unwrap(),
        ]);
        PathBuf::from(out.trim_end())
    }

    fn update(&self, ws: &Path) -> (i32, Vec<String>) {
        let (code, out, _) = self.run(&["update", "--workspace", ws.to_str().unwrap()]);
        (code, out.lines().map(str::to_owned).collect())
    }
}

fn short(oid: &str) -> &str {
    &oid[..7]
}

#[test]
fn two_workspaces_follow_one_remote_branch_while_the_clone_keeps_main() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let one = s.create("one");
    let two = s.create("two");

    let first = s.follow(&one, &shared, "origin/main");
    let second = s.follow(&two, &shared, "origin/main");

    let tip = git_in(&shared, &["rev-parse", "origin/main"]);
    for member in [&first, &second] {
        assert_eq!(git_in(member, &["rev-parse", "HEAD"]), tip);
        let head = Process::new("git")
            .args(["symbolic-ref", "-q", "HEAD"])
            .current_dir(member)
            .status()
            .unwrap();
        assert!(!head.success(), "a following member holds no branch");
    }
    assert_eq!(git_in(&shared, &["branch", "--show-current"]), "main");
    assert_eq!(
        git_in(
            &one,
            &["config", "--file", ".gitmodules", "submodule.shared.follow"]
        ),
        "origin/main"
    );
    let branch = Process::new("git")
        .args(["config", "--file", ".gitmodules", "submodule.shared.branch"])
        .current_dir(&one)
        .output()
        .unwrap();
    assert!(
        !branch.status.success(),
        "a following member records no branch"
    );
    assert_eq!(
        git_in(&one, &["status", "--porcelain"]),
        "",
        "the add is committed"
    );
    assert_eq!(
        git_in(&one, &["log", "-1", "--format=%s"]),
        "Add shared following origin/main"
    );

    let status = s.ok(&["status", "--workspace", one.to_str().unwrap()]);
    let fields: Vec<&str> = status.trim_end().split('\t').collect();
    assert_eq!(fields[1], "(follows origin/main)", "{status}");
    let json = s.ok(&[
        "status",
        "--workspace",
        one.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(json.contains("\"follow\":\"origin/main\""), "{json}");
}

#[test]
fn add_follow_refuses_what_is_not_a_remote_branch_and_takes_no_branch() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let ws = s.create("one");

    let (code, _, err) = s.run(&[
        "add",
        shared.to_str().unwrap(),
        "--follow",
        "origin/nope",
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(
        err.contains("'origin/nope' is not a remote branch"),
        "{err}"
    );
    assert!(!ws.join("shared").exists());

    let (code, _, err) = s.run(&[
        "add",
        shared.to_str().unwrap(),
        "main",
        "--follow",
        "origin/main",
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn update_moves_followers_and_branches_by_one_rule_and_a_second_run_changes_nothing() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let app = s.repo_with_origin("app");
    git_in(&app, &["push", "-q", "origin", "main:feat"]);
    git_in(&app, &["fetch", "-q", "origin"]);
    git_in(&app, &["branch", "-q", "--track", "feat", "origin/feat"]);
    git_in(&app, &["branch", "-q", "local-only"]);
    let one = s.create("one");
    let two = s.create("two");
    let first = s.follow(&one, &shared, "origin/main");
    let second = s.follow(&two, &shared, "origin/main");
    s.ok(&[
        "add",
        app.to_str().unwrap(),
        "feat",
        "--workspace",
        one.to_str().unwrap(),
    ]);

    let before = git_in(&first, &["rev-parse", "HEAD"]);
    let shared_tip = s.push_upstream("shared", "main", "b.txt");
    let feat_tip = s.push_upstream("app", "feat", "c.txt");

    let (code, rows) = s.update(&one);
    assert_eq!(code, 0, "{rows:?}");
    assert_eq!(
        rows,
        [
            format!(
                "shared\tupdated\t{}..{}",
                short(&before),
                short(&shared_tip)
            ),
            format!(
                "app\tupdated\t{}..{}",
                short(&before_app(&app)),
                short(&feat_tip)
            ),
        ]
    );
    assert_eq!(git_in(&first, &["rev-parse", "HEAD"]), shared_tip);
    assert_eq!(
        git_in(&one.join("app"), &["branch", "--show-current"]),
        "feat"
    );
    assert_eq!(git_in(&one.join("app"), &["rev-parse", "HEAD"]), feat_tip);

    let (code, again) = s.update(&one);
    assert_eq!(code, 0);
    assert_eq!(
        again,
        [
            format!("shared\tup-to-date\t{}", short(&shared_tip)),
            format!("app\tup-to-date\t{}", short(&feat_tip)),
        ]
    );

    // The other workspace fetches nothing new, since the clone is shared, and moves as well.
    let (code, rows) = s.update(&two);
    assert_eq!(code, 0);
    assert_eq!(
        rows,
        [format!(
            "shared\tupdated\t{}..{}",
            short(&before),
            short(&shared_tip)
        )]
    );
    assert_eq!(git_in(&second, &["rev-parse", "HEAD"]), shared_tip);
    assert_eq!(
        git_in(&shared, &["branch", "--show-current"]),
        "main",
        "the clone itself is not moved"
    );
}

/// The commit `feat` of `app` started at: `main`'s first commit.
fn before_app(app: &Path) -> String {
    git_in(app, &["rev-parse", "main"])
}

#[test]
fn update_skips_what_cannot_fast_forward_and_leaves_it_as_it_was() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let app = s.repo_with_origin("app");
    git_in(&app, &["branch", "-q", "solo"]);
    let other = s.repo_with_origin("other");
    let ws = s.create("one");
    let member = s.follow(&ws, &shared, "origin/main");
    s.ok(&[
        "add",
        app.to_str().unwrap(),
        "solo",
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    let diverged = s.follow(&ws, &other, "origin/main");
    s.push_upstream("shared", "main", "b.txt");
    s.push_upstream("other", "main", "b.txt");

    // A change in the follower holds it; a local commit in the other makes it diverge.
    std::fs::write(member.join("a.txt"), "edited\n").unwrap();
    std::fs::write(diverged.join("local.txt"), "x\n").unwrap();
    git_in(&diverged, &["add", "local.txt"]);
    git_in(&diverged, &["commit", "-q", "-m", "local"]);
    let held = git_in(&member, &["rev-parse", "HEAD"]);
    let kept = git_in(&diverged, &["rev-parse", "HEAD"]);

    let (code, rows) = s.update(&ws);
    assert_eq!(code, 0, "a skip is not a failure");
    assert_eq!(
        rows,
        [
            "shared\tskipped\tchanges",
            "app\tskipped\tno upstream",
            "other\tskipped\tdiverged"
        ]
    );
    assert_eq!(git_in(&member, &["rev-parse", "HEAD"]), held);
    assert_eq!(
        std::fs::read_to_string(member.join("a.txt")).unwrap(),
        "edited\n"
    );
    assert_eq!(git_in(&diverged, &["rev-parse", "HEAD"]), kept);
    let (_, again) = s.update(&ws);
    assert_eq!(again, rows, "the same answer every time");
}

#[test]
fn start_branch_carries_the_changes_and_leaves_the_other_followers_alone() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let one = s.create("one");
    let two = s.create("two");
    let first = s.follow(&one, &shared, "origin/main");
    let second = s.follow(&two, &shared, "origin/main");
    std::fs::write(first.join("a.txt"), "edited\n").unwrap();

    let (code, _, err) = s.run(&[
        "start-branch",
        "shared",
        "main",
        "--workspace",
        one.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("branch 'main' exists"), "{err}");

    let out = s.ok(&[
        "start-branch",
        "shared",
        "feat/shared-change",
        "--workspace",
        one.to_str().unwrap(),
    ]);
    assert_eq!(out.trim_end(), first.display().to_string());
    assert_eq!(
        git_in(&first, &["branch", "--show-current"]),
        "feat/shared-change"
    );
    assert_eq!(
        std::fs::read_to_string(first.join("a.txt")).unwrap(),
        "edited\n"
    );
    assert_eq!(
        git_in(
            &one,
            &["config", "--file", ".gitmodules", "submodule.shared.branch"]
        ),
        "feat/shared-change"
    );
    assert_eq!(
        git_in(&one, &["log", "-1", "--format=%s"]),
        "Start feat/shared-change in shared"
    );
    let status = s.ok(&["status", "--workspace", one.to_str().unwrap()]);
    assert!(
        status.starts_with("shared\tfeat/shared-change\tdirty"),
        "{status}"
    );

    let other = s.ok(&["status", "--workspace", two.to_str().unwrap()]);
    assert!(
        other.starts_with("shared\t(follows origin/main)"),
        "{other}"
    );
    let head = Process::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(&second)
        .status()
        .unwrap();
    assert!(!head.success());

    let (code, _, err) = s.run(&[
        "start-branch",
        "shared",
        "again",
        "--workspace",
        one.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("follows nothing"), "{err}");
}

#[test]
fn follow_again_refuses_changes_then_keeps_the_branch() {
    let s = Scratch::new();
    let shared = s.repo_with_origin("shared");
    let ws = s.create("one");
    let member = s.follow(&ws, &shared, "origin/main");
    s.ok(&[
        "start-branch",
        "shared",
        "feat/x",
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    std::fs::write(member.join("b.txt"), "x\n").unwrap();
    let words = [
        "follow",
        "shared",
        "origin/main",
        "--workspace",
        ws.to_str().unwrap(),
    ];

    let (code, _, err) = s.run(&words);
    assert_eq!(code, 1);
    assert!(err.contains("untracked files"), "{err}");

    git_in(&member, &["add", "b.txt"]);
    git_in(&member, &["commit", "-q", "-m", "b"]);
    let work = git_in(&member, &["rev-parse", "HEAD"]);
    s.ok(&words);
    assert_eq!(
        git_in(&member, &["rev-parse", "HEAD"]),
        git_in(&shared, &["rev-parse", "origin/main"])
    );
    assert_eq!(
        git_in(&shared, &["rev-parse", "feat/x"]),
        work,
        "the branch stays"
    );
    assert_eq!(
        git_in(
            &ws,
            &["config", "--file", ".gitmodules", "submodule.shared.follow"]
        ),
        "origin/main"
    );
    assert_eq!(
        git_in(&ws, &["log", "-1", "--format=%s"]),
        "Follow origin/main in shared"
    );
    assert_eq!(git_in(&ws, &["status", "--porcelain"]), "");
}
