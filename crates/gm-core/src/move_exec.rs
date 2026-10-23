//! Carries out a move of one file's Working Copy change to a branch or a
//! worktree (D109). The order is the safety: the target receives the change
//! before the source loses it, and every refusal is decided before anything is
//! written. A failure at any step returns an error and stops; the data then
//! lives in the source, in the target, or in both.

use std::io::Write;
use std::path::{Path, PathBuf};

use gm_schema::{MoveBlocker, MoveOutcome, MoveTarget, OperationId};

use crate::DiscardOutcome;
use crate::engine::same_folder;
use crate::move_changes::{
    FilePlan, commit_message, describe, move_ref_name, plan_file_move, safe_relative,
};
use crate::move_preview::{crosses_symlink, preview, read_target};
use crate::{FileMerge, GitPort, TreeMerge};

/// The prefix of the temporary file a worktree write goes through. The file is
/// hidden, and it is removed on every way out.
const TEMP_PREFIX: &str = ".gitmaster-move-";

/// Moves the change of `path` from the Working Copy of `root` to `target`.
/// `message` is the commit message of a branch target and is ignored for a
/// worktree. The returned outcome names the new commit and the reason the
/// source file was kept, when it was.
pub fn run(
    git: &dyn GitPort,
    root: &Path,
    id: OperationId,
    path: &str,
    target: &MoveTarget,
    message: Option<&str>,
) -> Result<MoveOutcome, String> {
    // Step 1: the message and every refusal, decided again on the state now.
    let message = match target {
        MoveTarget::Branch { .. } => commit_message(message)?,
        MoveTarget::Worktree { .. } => String::new(),
    };
    if let Some(blocker) = preview(git, root, path, target)?.blockers.first() {
        return Err(describe(blocker));
    }
    // Step 2: the snapshot of the file, kept by a ref that is not removed (D109, Q24).
    let snapshot = git
        .snapshot_paths(root, &[path.to_owned()])
        .map_err(|e| e.0)?;
    git.update_ref(root, &move_ref_name(id), &snapshot, None)
        .map_err(|e| e.0)?;
    // Step 3: the target gets the change.
    let mut moved = match target {
        MoveTarget::Branch { name, create } => MoveOutcome {
            commit: Some(commit_on_branch(
                git, root, &snapshot, name, *create, &message,
            )?),
            kept: None,
        },
        MoveTarget::Worktree { path: folder } => {
            write_worktree(git, root, &snapshot, path, folder)?;
            MoveOutcome::default()
        }
    };
    // Step 4: the source loses the file, only if it still matches the snapshot.
    let discarded = git.discard_moved(root, path, &snapshot).map_err(|e| {
        format!(
            "the change is in the target, but the source was not cleaned up: {}",
            e.0
        )
    })?;
    if let DiscardOutcome::Kept { reason } = discarded {
        moved.kept = Some(reason);
    }
    Ok(moved)
}

/// The revision a branch move merges onto: HEAD for a new branch, else the tip
/// of the branch.
fn ours_rev(name: &str, create: bool) -> String {
    if create {
        "HEAD".to_owned()
    } else {
        format!("refs/heads/{name}")
    }
}

/// Commits the change on the branch `name` without a checkout (D109, branch
/// target). The tree is a three-way merge of the snapshot onto the tip, so the
/// tip's own changes stay; the branch moves only from the tip it was read at.
fn commit_on_branch(
    git: &dyn GitPort,
    root: &Path,
    snapshot: &str,
    name: &str,
    create: bool,
    message: &str,
) -> Result<String, String> {
    let tip = git
        .resolve_commit(root, &ours_rev(name, create))
        .map_err(|e| e.0)?;
    let tree = match git
        .merge_trees(root, "HEAD", &tip, snapshot)
        .map_err(|e| e.0)?
    {
        TreeMerge::Clean { tree } => tree,
        TreeMerge::Conflict { paths } => {
            return Err(describe(&MoveBlocker::Conflict { paths }));
        }
    };
    let commit = git
        .commit_tree(root, &tree, Some(&tip), message)
        .map_err(|e| e.0)?;
    let old = (!create).then_some(tip.as_str());
    git.update_ref(root, &format!("refs/heads/{name}"), &commit, old)
        .map_err(|e| e.0)?;
    Ok(commit)
}

/// Applies the change of `path` to the file on disk in the worktree at
/// `folder` (D109, worktree target). The index of that worktree is not touched.
fn write_worktree(
    git: &dyn GitPort,
    root: &Path,
    snapshot: &str,
    path: &str,
    folder: &str,
) -> Result<(), String> {
    let worktrees = git.worktrees(root).map_err(|e| e.0)?;
    let entry = worktrees
        .iter()
        .find(|entry| same_folder(Path::new(&entry.path), folder))
        .ok_or_else(|| describe(&MoveBlocker::WorktreeNotFound))?;
    let target = PathBuf::from(&entry.path);
    if same_folder(root, &entry.path) {
        return Err(describe(&MoveBlocker::SameWorktree));
    }
    if entry.prunable.is_some() || !target.is_dir() {
        return Err(describe(&MoveBlocker::WorktreeMissing));
    }
    if !safe_relative(path) || crosses_symlink(&target, path)? {
        return Err(describe(&MoveBlocker::UnsafePath));
    }
    let base = git.file_at_commit(root, "HEAD", path).map_err(|e| e.0)?;
    let theirs = git.file_at_commit(root, snapshot, path).map_err(|e| e.0)?;
    let ours = read_target(&target.join(path))?;
    let source = root.join(path);
    let guard = Guard {
        folder: &target,
        path,
        expected: ours.as_deref(),
        mode_from: &source,
    };
    match plan_file_move(base.as_deref(), theirs.as_deref(), ours.as_deref()) {
        FilePlan::NoOp => Ok(()),
        FilePlan::Create(bytes) => guard.write(None, &bytes),
        FilePlan::Replace(bytes) => guard.write(ours.as_deref(), &bytes),
        FilePlan::Delete => guard.remove(),
        FilePlan::Merge => {
            let merged = git
                .merge_file(
                    root,
                    base.as_deref().unwrap_or_default(),
                    ours.as_deref().unwrap_or_default(),
                    theirs.as_deref().unwrap_or_default(),
                )
                .map_err(|e| e.0)?;
            match merged {
                FileMerge::Clean(bytes) => guard.write(ours.as_deref(), &bytes),
                FileMerge::Conflict => Err(describe(&MoveBlocker::Conflict {
                    paths: vec![path.to_owned()],
                })),
                FileMerge::Binary => Err(describe(&MoveBlocker::BinaryDiffers)),
            }
        }
        FilePlan::Blocked(MoveBlocker::Conflict { paths }) if paths.is_empty() => {
            Err(describe(&MoveBlocker::Conflict {
                paths: vec![path.to_owned()],
            }))
        }
        FilePlan::Blocked(blocker) => Err(describe(&blocker)),
    }
}

/// One file of a worktree target and the state it was read in. Its writes
/// refuse to go on when the file on disk is not `expected` any more.
struct Guard<'a> {
    folder: &'a Path,
    path: &'a str,
    /// The file as it was merged against; `None` when it was absent.
    expected: Option<&'a [u8]>,
    /// The file the change comes from, for the permissions of a new file.
    mode_from: &'a Path,
}

impl Guard<'_> {
    /// Writes `bytes` as the file, while it still holds `expected`.
    fn write(&self, expected: Option<&[u8]>, bytes: &[u8]) -> Result<(), String> {
        write_guarded(self.folder, self.path, expected, bytes, self.mode_from)
    }

    /// Removes the file, while it still holds the `expected` content.
    fn remove(&self) -> Result<(), String> {
        remove_guarded(self.folder, self.path, self.expected)
    }
}

/// The refusal for a target file that changed while the move ran.
fn changed_in(folder: &Path) -> String {
    format!(
        "the file changed in {} while the move ran; drop it again",
        folder.display()
    )
}

/// Writes `bytes` to `path` under `folder` through a temporary file in the same
/// folder, renamed over the file (D109). Nothing is written when the file on
/// disk is not `expected` any more, or when a symbolic link is on the way.
/// `expected` of `None` means the file must be absent, and it is never replaced.
/// Permissions come from the file on disk, or from `mode_from` for a new file.
fn write_guarded(
    folder: &Path,
    path: &str,
    expected: Option<&[u8]>,
    bytes: &[u8],
    mode_from: &Path,
) -> Result<(), String> {
    let file = folder.join(path);
    if crosses_symlink(folder, path)? {
        return Err(describe(&MoveBlocker::UnsafePath));
    }
    let parent = file
        .parent()
        .ok_or_else(|| format!("cannot write {}", file.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let mut temp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|e| format!("cannot write in {}: {e}", parent.display()))?;
    temp.write_all(bytes)
        .map_err(|e| format!("cannot write in {}: {e}", parent.display()))?;
    temp.flush()
        .map_err(|e| format!("cannot write in {}: {e}", parent.display()))?;
    if let Ok(meta) = std::fs::metadata(&file).or_else(|_| std::fs::metadata(mode_from)) {
        std::fs::set_permissions(temp.path(), meta.permissions())
            .map_err(|e| format!("cannot set the permissions of {}: {e}", file.display()))?;
    }
    // The last look, right before the rename: the file must still be what the
    // merge read. The temporary file goes away when `temp` drops on the error path.
    if read_target(&file)?.as_deref() != expected {
        return Err(changed_in(folder));
    }
    let result = match expected {
        None => temp.persist_noclobber(&file),
        Some(_) => temp.persist(&file),
    };
    result
        .map(drop)
        .map_err(|e| format!("cannot write {}: {}", file.display(), e.error))
}

/// Removes `path` under `folder`, while the file on disk still holds `expected`
/// (D109). A symbolic link on the way is refused.
fn remove_guarded(folder: &Path, path: &str, expected: Option<&[u8]>) -> Result<(), String> {
    if crosses_symlink(folder, path)? {
        return Err(describe(&MoveBlocker::UnsafePath));
    }
    let file = folder.join(path);
    if read_target(&file)?.as_deref() != expected {
        return Err(changed_in(folder));
    }
    std::fs::remove_file(&file).map_err(|e| format!("cannot remove {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder with `file` holding `content`, and a source file beside it.
    fn folder_with(file: &str, content: Option<&[u8]>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(content) = content {
            std::fs::write(dir.path().join(file), content).unwrap();
        }
        dir
    }

    fn source(dir: &Path) -> PathBuf {
        let source = dir.join("source.txt");
        std::fs::write(&source, b"mode").unwrap();
        source
    }

    /// The entries of `dir`, so a test can see a temporary file left behind.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn ours_is_head_for_a_new_branch_and_the_branch_otherwise() {
        assert_eq!(ours_rev("feature/x", true), "HEAD");
        assert_eq!(ours_rev("dev", false), "refs/heads/dev");
    }

    #[test]
    fn a_new_file_is_written_when_the_path_is_absent() {
        let dir = folder_with("other.txt", None);
        let mode = source(dir.path());
        write_guarded(dir.path(), "sub/a.txt", None, b"new\n", &mode).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("sub/a.txt")).unwrap(),
            b"new\n"
        );
        assert_eq!(names(&dir.path().join("sub")), ["a.txt"]);
    }

    #[test]
    fn a_file_is_replaced_only_while_it_still_holds_what_was_merged_against() {
        let dir = folder_with("a.txt", Some(b"old\n"));
        let mode = source(dir.path());
        write_guarded(dir.path(), "a.txt", Some(b"old\n"), b"new\n", &mode).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"new\n");
        assert_eq!(names(dir.path()), ["a.txt", "source.txt"]);
    }

    #[test]
    fn a_file_that_changed_since_the_merge_is_kept_and_no_temporary_is_left() {
        let dir = folder_with("a.txt", Some(b"edited meanwhile\n"));
        let mode = source(dir.path());
        let error =
            write_guarded(dir.path(), "a.txt", Some(b"old\n"), b"new\n", &mode).unwrap_err();
        assert!(error.contains("changed"), "{error}");
        assert!(error.contains("drop it again"), "{error}");
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"edited meanwhile\n"
        );
        assert_eq!(names(dir.path()), ["a.txt", "source.txt"]);
    }

    #[test]
    fn a_new_file_is_never_written_over_a_file_that_appeared_meanwhile() {
        let dir = folder_with("a.txt", Some(b"appeared\n"));
        let mode = source(dir.path());
        assert!(write_guarded(dir.path(), "a.txt", None, b"new\n", &mode).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"appeared\n"
        );
        assert_eq!(names(dir.path()), ["a.txt", "source.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_on_the_way_is_refused_and_nothing_is_written() {
        let dir = folder_with("other.txt", None);
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let mode = source(dir.path());
        let error = write_guarded(dir.path(), "link/a.txt", None, b"new\n", &mode).unwrap_err();
        assert_eq!(error, describe(&MoveBlocker::UnsafePath));
        assert!(names(outside.path()).is_empty());
    }

    #[test]
    fn a_removed_file_must_still_hold_what_was_merged_against() {
        let dir = folder_with("a.txt", Some(b"same\n"));
        remove_guarded(dir.path(), "a.txt", Some(b"same\n")).unwrap();
        assert!(!dir.path().join("a.txt").exists());

        let dir = folder_with("a.txt", Some(b"edited\n"));
        assert!(remove_guarded(dir.path(), "a.txt", Some(b"same\n")).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"edited\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_the_permissions_it_had() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder_with("run.sh", Some(b"old\n"));
        let file = dir.path().join("run.sh");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mode = source(dir.path());
        write_guarded(dir.path(), "run.sh", Some(b"old\n"), b"new\n", &mode).unwrap();
        let mode_now = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_now, 0o755);
    }
}
