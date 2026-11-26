//! `ReadImage` (D268) against a real repository: the bytes of an image in the working tree,
//! the index and a commit, a missing file, and a path that leaves the worktree.

use std::path::Path;
use std::process::Command as Process;

use gm_core::Engine;
use gm_core::git::SubprocessGit;
use gm_schema::{Actor, Command, CommandKind, Event, ImageAt, ImageBytes, OperationId};

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

fn engine(root: &Path) -> Engine {
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let open = Command {
        actor: Actor::Human,
        kind: CommandKind::OpenRepo {
            path: root.to_string_lossy().into_owned(),
        },
    };
    engine.handle(OperationId(1), &open);
    engine
}

/// The answer to `ReadImage`, or the reason it failed.
fn read(engine: &mut Engine, path: &str, at: &ImageAt) -> Result<ImageBytes, String> {
    let command = Command {
        actor: Actor::Human,
        kind: CommandKind::ReadImage {
            path: path.to_owned(),
            at: at.clone(),
        },
    };
    let events = engine.handle(OperationId(2), &command);
    for event in &events {
        if let Event::ImageRead {
            id: OperationId(2),
            path: read,
            at: read_at,
            image,
        } = event
        {
            assert_eq!((read.as_str(), read_at), (path, at));
            return Ok(image.clone());
        }
    }
    Err(format!("no ImageRead in {events:?}"))
}

fn bytes(data: &[u8]) -> ImageBytes {
    ImageBytes::Bytes {
        data: data.to_vec(),
    }
}

#[test]
fn reads_an_image_from_the_worktree_the_index_and_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::create_dir(root.join("img")).unwrap();
    std::fs::write(root.join("img/logo.png"), b"\x89PNG one").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("img/logo.png"), b"\x89PNG two").unwrap();
    git(root, &["add", "."]);
    std::fs::write(root.join("img/logo.png"), b"\x89PNG three").unwrap();

    let mut engine = engine(root);
    let path = "img/logo.png";
    assert_eq!(
        read(&mut engine, path, &ImageAt::Worktree),
        Ok(bytes(b"\x89PNG three"))
    );
    assert_eq!(
        read(&mut engine, path, &ImageAt::Index),
        Ok(bytes(b"\x89PNG two"))
    );
    let head = ImageAt::Commit {
        rev: "HEAD".to_owned(),
    };
    assert_eq!(read(&mut engine, path, &head), Ok(bytes(b"\x89PNG one")));
    // Missing: not in the commit, not on disk, a folder.
    assert_eq!(
        read(&mut engine, "gone.png", &head),
        Ok(ImageBytes::Missing)
    );
    assert_eq!(
        read(&mut engine, "gone.png", &ImageAt::Worktree),
        Ok(ImageBytes::Missing)
    );
    assert_eq!(
        read(&mut engine, "img", &ImageAt::Worktree),
        Ok(ImageBytes::Missing)
    );
    // A path that leaves the worktree is refused, not read.
    assert!(read(&mut engine, "../outside.png", &ImageAt::Worktree).is_err());
}
