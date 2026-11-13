//! `LoadChangeStats` (D236 (8)) against real repositories: the lines each file adds and
//! removes, unstaged and staged, from `git diff --numstat`.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, ChangeStats, Command, CommandKind, Event, LineCounts, OperationId};

fn git(dir: &Path, args: &[&str]) {
    let status = Process::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn counts(path: &str, added: u32, deleted: u32) -> LineCounts {
    LineCounts {
        path: path.to_owned(),
        added,
        deleted,
    }
}

fn stats(root: &Path) -> ChangeStats {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let open = Command {
        actor: Actor::Human,
        kind: CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    };
    engine.handle(OperationId(1), &open);
    let load = Command {
        actor: Actor::Human,
        kind: CommandKind::LoadChangeStats,
    };
    engine
        .handle(OperationId(2), &load)
        .into_iter()
        .find_map(|event| match event {
            Event::ChangeStatsLoaded {
                id: OperationId(2),
                stats,
            } => Some(stats),
            _ => None,
        })
        .expect("no ChangeStatsLoaded")
}

#[test]
fn counts_each_side_and_names_a_staged_rename_by_its_new_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "1\n2\n3\n").unwrap();
    std::fs::write(root.join("old name.txt"), "keep\nthese\nlines\nhere\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "first"]);

    // Staged: a rename with one line changed, and one line added to a.txt.
    git(root, &["mv", "old name.txt", "new name.txt"]);
    std::fs::write(root.join("new name.txt"), "keep\nthese\nlines\nthere\n").unwrap();
    std::fs::write(root.join("a.txt"), "1\n2\n3\n4\n").unwrap();
    git(root, &["add", "."]);
    // Unstaged: a.txt loses two lines, the binary changes, and an untracked file appears.
    std::fs::write(root.join("a.txt"), "1\n4\n").unwrap();
    std::fs::write(root.join("blob.bin"), [9u8, 0, 9]).unwrap();
    std::fs::write(root.join("untracked.txt"), "new\n").unwrap();

    let stats = stats(root);

    assert_eq!(stats.unstaged, [counts("a.txt", 0, 2)]);
    let mut staged = stats.staged;
    staged.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        staged,
        [counts("a.txt", 1, 0), counts("new name.txt", 1, 1)]
    );
}
