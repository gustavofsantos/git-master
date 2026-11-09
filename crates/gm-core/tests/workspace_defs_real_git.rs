//! The workspace definitions a member's clone sees (D214 (5), D174, D180 (2)) against real
//! repositories in temporary directories: a workspace with two members, each a worktree of its own
//! clone, and the gate and reviewer files its repository keeps in `.git/gitmaster/`. Each test
//! removes its directory when it ends.

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use gm_core::gates::{self, GateFacts};
use gm_core::git::SubprocessGit;
use gm_core::reviewers;
use gm_core::workspace_cli::run_with;
use gm_core::workspace_defs::{WorkspaceDefs, defs_for_member, workspaces_of_clone};
use gm_core::worktree_cli::Env;
use gm_schema::{GateLayer, GateOp, GateOutcome, ReviewerSource};

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

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
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

/// The folder name of a member path.
fn name_of(member: &Path) -> String {
    member.file_name().unwrap().to_string_lossy().into_owned()
}

/// Judges one gate of `gates` for `member` on a branch and returns its outcome.
fn judged(layered: &[gm_schema::LayeredGate], id: &str, member: &str) -> GateOutcome {
    let status = gates::evaluate(
        layered,
        Some(GateOp::Push),
        &GateFacts {
            branch: "feat/x",
            head: "1234567890abcdef",
            changed_paths: None,
            reviewers: &[],
            preset_names: &[],
            verdicts: &[],
            findings: &[],
            behind_parent: Some(0),
            approved: true,
            member: Some(member),
        },
    );
    status
        .gates
        .into_iter()
        .find(|gate| gate.id == id)
        .expect("listed")
}

#[test]
fn each_member_clone_sees_the_definitions_of_its_workspace_and_its_own_member() {
    let s = Scratch::new();
    let a = s.repo("service-a", &["feat/a"]);
    let b = s.repo("service-b", &["feat/b"]);
    let unrelated = s.repo("unrelated", &[]);
    let ws = s.create("pay");
    let member_a = name_of(&s.add(&ws, &a, "feat/a"));
    let member_b = name_of(&s.add(&ws, &b, "feat/b"));
    assert_eq!(
        (member_a.as_str(), member_b.as_str()),
        ("service-a", "service-b")
    );

    // The workspace repository keeps its definitions in .git/gitmaster, as any repository does.
    let defs_dir = ws.join(".git").join("gitmaster");
    std::fs::create_dir_all(defs_dir.join("reviewers")).unwrap();
    std::fs::write(
        defs_dir.join("gates.json"),
        r#"{"gates": [{"id": "service-a-up", "kind": "up_to_date", "on": ["push"], "members": ["service-a"]}]}"#,
    )
    .unwrap();
    std::fs::write(
        defs_dir.join("reviewers").join("api-check.md"),
        "---\ndescription: Checks the api\nsignal: pass\nmembers: service-a\n---\nCheck it.\n",
    )
    .unwrap();

    let git = SubprocessGit::default();
    let pointed = workspaces_of_clone(&common_of(&a));
    assert_eq!(
        pointed.iter().map(|p| canonical(p)).collect::<Vec<_>>(),
        [canonical(&ws)]
    );

    let seen_by_a = defs_for_member(&git, &pointed, &a);
    assert_eq!(seen_by_a.len(), 1, "{seen_by_a:?}");
    let (defs, member) = &seen_by_a[0];
    assert_eq!(member, "service-a");
    assert_eq!(defs.name, "pay");
    assert_eq!(canonical(&defs.dir), canonical(&defs_dir));

    // The definitions load with the workspace layer, and the member filter applies to them.
    let loaded = gates::load_layers(None, None, std::slice::from_ref(defs));
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.gates.len(), 1);
    assert_eq!(loaded.gates[0].layer, GateLayer::Workspace);
    let for_a = judged(&loaded.gates, "service-a-up", &member_a);
    assert!(for_a.applicable, "{for_a:?}");
    let reviewers = reviewers::load_layers(None, None, std::slice::from_ref(defs));
    assert_eq!(reviewers.reviewers.len(), 1);
    assert_eq!(reviewers.reviewers[0].source, ReviewerSource::Workspace);

    // The member b of the same workspace sees the same definitions for its own folder.
    let seen_by_b = defs_for_member(&git, &pointed, &b);
    assert_eq!(seen_by_b.len(), 1, "{seen_by_b:?}");
    assert_eq!(seen_by_b[0].1, "service-b");
    assert_eq!(canonical(&seen_by_b[0].0.dir), canonical(&defs_dir));
    let for_b = judged(&loaded.gates, "service-a-up", &member_b);
    assert!(!for_b.applicable, "{for_b:?}");
    assert_eq!(
        for_b.not_applicable.as_deref(),
        Some("the member service-b is not one of service-a")
    );

    // A clone that is no member of the workspace sees nothing, and has no pointer at all.
    assert!(defs_for_member(&git, &pointed, &unrelated).is_empty());
    assert!(workspaces_of_clone(&common_of(&unrelated)).is_empty());
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
    let defs = WorkspaceDefs {
        dir: ws.join(".git").join("gitmaster"),
        name: "pay".to_owned(),
    };
    assert!(gates::load_layers(None, None, &[defs]).gates.is_empty());
}
