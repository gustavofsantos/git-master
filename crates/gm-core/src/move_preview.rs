//! Reads what moving the Working Copy change of one file to a branch or a
//! worktree would refuse (D109). It writes nothing that a user can see: the
//! snapshot and the merge trees are unreferenced objects, and the three-way
//! merges are computations. Not recorded in the command log.

use std::path::Path;

use gm_schema::{BranchEntry, MoveBlocker, MovePreview, MoveTarget, StatusEntry};

use crate::branch;
use crate::engine::same_folder;
use crate::move_changes::{FilePlan, git_blocker, order_blockers, plan_file_move, safe_relative};
use crate::{FileMerge, GitPort, TreeMerge};

/// The blockers of moving the change of `path` to `target`, in the order the
/// human reads them. `root` is the window's own worktree. An `Err` is a Git
/// failure, not a refusal.
pub fn preview(
    git: &dyn GitPort,
    root: &Path,
    path: &str,
    target: &MoveTarget,
) -> Result<MovePreview, String> {
    let version = git.git_version().map_err(|e| e.0)?;
    // Without a usable Git nothing else can be computed.
    if let Some(blocker) = git_blocker(&version) {
        return Ok(answer(vec![blocker]));
    }
    let status = git.status(root).map_err(|e| e.0)?;
    if let Some(blocker) = source_refusal(&status.entries, path) {
        return Ok(answer(vec![blocker]));
    }
    let blockers = match target {
        MoveTarget::Branch { name, create } => branch_target(git, root, path, name, *create)?,
        MoveTarget::Worktree { path: folder } => worktree_target(git, root, path, folder)?,
    };
    Ok(answer(blockers))
}

fn answer(blockers: Vec<MoveBlocker>) -> MovePreview {
    MovePreview {
        blockers: order_blockers(blockers),
    }
}

/// The blockers of a branch target: the branch rules, then the merge of the
/// change onto the branch tip (or onto HEAD for a new branch).
fn branch_target(
    git: &dyn GitPort,
    root: &Path,
    path: &str,
    name: &str,
    create: bool,
) -> Result<Vec<MoveBlocker>, String> {
    let branches = git.branches(root).map_err(|e| e.0)?;
    let existing = branches
        .iter()
        .find(|branch| !branch.is_remote && branch.name == name);
    let worktrees = git.worktrees(root).map_err(|e| e.0)?;
    let elsewhere = worktrees
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(name) && !same_folder(root, &entry.path))
        .map(|entry| entry.path.clone());
    if let Some(blocker) = branch_refusal(create, name, existing, elsewhere) {
        return Ok(vec![blocker]);
    }
    let snapshot = git
        .snapshot_paths(root, &[path.to_owned()])
        .map_err(|e| e.0)?;
    let ours = if create {
        "HEAD".to_owned()
    } else {
        format!("refs/heads/{name}")
    };
    let merge = git
        .merge_trees(root, "HEAD", &ours, &snapshot)
        .map_err(|e| e.0)?;
    Ok(tree_blocker(merge).into_iter().collect())
}

/// The blockers of a worktree target: the worktree rules and the path rules,
/// then the change of the file merged into the file on disk there.
fn worktree_target(
    git: &dyn GitPort,
    root: &Path,
    path: &str,
    folder: &str,
) -> Result<Vec<MoveBlocker>, String> {
    let worktrees = git.worktrees(root).map_err(|e| e.0)?;
    let Some(entry) = worktrees
        .iter()
        .find(|entry| same_folder(Path::new(&entry.path), folder))
    else {
        return Ok(vec![MoveBlocker::WorktreeNotFound]);
    };
    let target = Path::new(&entry.path);
    let mut blockers = Vec::new();
    if same_folder(root, &entry.path) {
        blockers.push(MoveBlocker::SameWorktree);
    }
    if entry.prunable.is_some() || !target.is_dir() {
        blockers.push(MoveBlocker::WorktreeMissing);
    }
    if !safe_relative(path) {
        blockers.push(MoveBlocker::UnsafePath);
    }
    // Nothing below depends on a folder that is not the target, or on a path
    // that leaves it.
    if !blockers.is_empty() {
        return Ok(blockers);
    }
    if crosses_symlink(target, path)? {
        return Ok(vec![MoveBlocker::UnsafePath]);
    }
    file_target(git, root, target, path)
}

/// The per-file plan (D109) for a file in a worktree target.
fn file_target(
    git: &dyn GitPort,
    root: &Path,
    target: &Path,
    path: &str,
) -> Result<Vec<MoveBlocker>, String> {
    let snapshot = git
        .snapshot_paths(root, &[path.to_owned()])
        .map_err(|e| e.0)?;
    let base = git.file_at_commit(root, "HEAD", path).map_err(|e| e.0)?;
    let theirs = git.file_at_commit(root, &snapshot, path).map_err(|e| e.0)?;
    let ours = read_target(&target.join(path))?;
    Ok(
        match plan_file_move(base.as_deref(), theirs.as_deref(), ours.as_deref()) {
            FilePlan::Blocked(blocker) => vec![named_conflict(blocker, path)],
            FilePlan::Merge => {
                // `plan_file_move` asks for a merge only when all three sides exist.
                let merge = git
                    .merge_file(
                        root,
                        base.as_deref().unwrap_or_default(),
                        ours.as_deref().unwrap_or_default(),
                        theirs.as_deref().unwrap_or_default(),
                    )
                    .map_err(|e| e.0)?;
                merge_blocker(&merge, path).into_iter().collect()
            }
            FilePlan::NoOp | FilePlan::Create(_) | FilePlan::Replace(_) | FilePlan::Delete => {
                Vec::new()
            }
        },
    )
}

/// The refusal for the source path: nothing changed there, or it is conflicted.
fn source_refusal(entries: &[StatusEntry], path: &str) -> Option<MoveBlocker> {
    match entries.iter().find(|entry| entry.path == path) {
        None => Some(MoveBlocker::NothingToMove),
        Some(entry) if entry.conflicted => Some(MoveBlocker::Conflicted),
        Some(_) => None,
    }
}

/// The refusal for a branch target, or `None` when the branch rules pass. The
/// rules depend on each other, so the first that applies is the one given.
fn branch_refusal(
    create: bool,
    name: &str,
    existing: Option<&BranchEntry>,
    elsewhere: Option<String>,
) -> Option<MoveBlocker> {
    if create {
        if let Some(reason) = branch::name_problem(name) {
            return Some(MoveBlocker::InvalidName {
                reason: reason.to_owned(),
            });
        }
        return existing.is_some().then_some(MoveBlocker::BranchExists);
    }
    let Some(branch) = existing else {
        return Some(MoveBlocker::BranchNotFound);
    };
    if branch.is_current {
        return Some(MoveBlocker::SameBranch);
    }
    elsewhere.map(|path| MoveBlocker::BranchCheckedOutElsewhere { path })
}

/// A conflict from the per-file plan names the file, as the tree merge does.
fn named_conflict(blocker: MoveBlocker, path: &str) -> MoveBlocker {
    match blocker {
        MoveBlocker::Conflict { paths } if paths.is_empty() => MoveBlocker::Conflict {
            paths: vec![path.to_owned()],
        },
        other => other,
    }
}

/// The refusal for the result of a three-way merge of one file.
fn merge_blocker(merge: &FileMerge, path: &str) -> Option<MoveBlocker> {
    match merge {
        FileMerge::Clean(_) => None,
        FileMerge::Conflict => Some(MoveBlocker::Conflict {
            paths: vec![path.to_owned()],
        }),
        FileMerge::Binary => Some(MoveBlocker::BinaryDiffers),
    }
}

/// The refusal for the result of the tree merge of a branch target.
fn tree_blocker(merge: TreeMerge) -> Option<MoveBlocker> {
    match merge {
        TreeMerge::Clean { .. } => None,
        TreeMerge::Conflict { paths } => Some(MoveBlocker::Conflict { paths }),
    }
}

/// True when the file or a folder between the worktree and the file is a
/// symbolic link. The walk ends at the first part that is not there.
pub(crate) fn crosses_symlink(worktree: &Path, path: &str) -> Result<bool, String> {
    let mut current = worktree.to_path_buf();
    for part in path.split('/') {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("cannot read {}: {e}", current.display())),
        }
    }
    Ok(false)
}

/// The file on disk in the target, `None` when it is not there.
pub(crate) fn read_target(file: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_entry(path: &str, conflicted: bool) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged: None,
            unstaged: None,
            untracked: false,
            conflicted,
        }
    }

    fn branch_entry(name: &str, is_current: bool) -> BranchEntry {
        BranchEntry {
            name: name.to_owned(),
            is_remote: false,
            is_current,
            upstream: None,
            author_email: None,
        }
    }

    #[test]
    fn a_path_with_no_change_is_nothing_to_move() {
        let entries = [status_entry("a.txt", false)];
        assert_eq!(
            source_refusal(&entries, "b.txt"),
            Some(MoveBlocker::NothingToMove)
        );
        assert_eq!(source_refusal(&entries, "a.txt"), None);
    }

    #[test]
    fn a_conflicted_path_is_refused() {
        let entries = [status_entry("a.txt", true)];
        assert_eq!(
            source_refusal(&entries, "a.txt"),
            Some(MoveBlocker::Conflicted)
        );
    }

    #[test]
    fn a_new_branch_with_a_valid_free_name_passes() {
        assert_eq!(branch_refusal(true, "feature/x", None, None), None);
    }

    #[test]
    fn a_new_branch_with_an_invalid_name_gives_the_reason() {
        assert!(matches!(
            branch_refusal(true, "", None, None),
            Some(MoveBlocker::InvalidName { reason }) if !reason.is_empty()
        ));
    }

    #[test]
    fn a_new_branch_over_an_existing_one_is_refused() {
        let existing = branch_entry("dev", false);
        assert_eq!(
            branch_refusal(true, "dev", Some(&existing), None),
            Some(MoveBlocker::BranchExists)
        );
    }

    #[test]
    fn an_existing_branch_must_exist() {
        assert_eq!(
            branch_refusal(false, "dev", None, None),
            Some(MoveBlocker::BranchNotFound)
        );
        let existing = branch_entry("dev", false);
        assert_eq!(branch_refusal(false, "dev", Some(&existing), None), None);
    }

    #[test]
    fn the_branch_of_the_window_is_the_same_branch() {
        let current = branch_entry("main", true);
        assert_eq!(
            branch_refusal(false, "main", Some(&current), None),
            Some(MoveBlocker::SameBranch)
        );
    }

    #[test]
    fn a_branch_checked_out_elsewhere_names_its_folder() {
        let existing = branch_entry("dev", false);
        assert_eq!(
            branch_refusal(false, "dev", Some(&existing), Some("/w/dev".into())),
            Some(MoveBlocker::BranchCheckedOutElsewhere {
                path: "/w/dev".into()
            })
        );
    }

    #[test]
    fn a_named_conflict_names_the_file() {
        assert_eq!(
            named_conflict(MoveBlocker::Conflict { paths: Vec::new() }, "a.txt"),
            MoveBlocker::Conflict {
                paths: vec!["a.txt".into()]
            }
        );
        assert_eq!(
            named_conflict(MoveBlocker::AddOverExisting, "a.txt"),
            MoveBlocker::AddOverExisting
        );
    }

    #[test]
    fn a_merge_maps_to_its_refusal() {
        assert_eq!(merge_blocker(&FileMerge::Clean(b"x".to_vec()), "a"), None);
        assert_eq!(
            merge_blocker(&FileMerge::Conflict, "a"),
            Some(MoveBlocker::Conflict {
                paths: vec!["a".into()]
            })
        );
        assert_eq!(
            merge_blocker(&FileMerge::Binary, "a"),
            Some(MoveBlocker::BinaryDiffers)
        );
    }

    #[test]
    fn a_tree_merge_maps_to_its_refusal() {
        assert_eq!(tree_blocker(TreeMerge::Clean { tree: "abc".into() }), None);
        assert_eq!(
            tree_blocker(TreeMerge::Conflict {
                paths: vec!["a".into()]
            }),
            Some(MoveBlocker::Conflict {
                paths: vec!["a".into()]
            })
        );
    }
}
