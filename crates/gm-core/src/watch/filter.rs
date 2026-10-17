//! Which changed paths can change what the status shows. Pure: the caller
//! supplies the paths relative to the git dir or to the worktree root.

use std::path::{Component, Path};

/// Whether `path` is git's own state inside a worktree (`.git`, or anything
/// under a `.git` directory). Those paths are read through the git dir.
pub fn is_inside_dot_git(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == ".git")
}

fn is_lock(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| ext == "lock")
}

/// Whether a change under the git dir (or the common dir of a linked
/// worktree, which holds `packed-refs` and `refs/`) can change the status.
/// `rel` is relative to either directory. Lock files, objects, logs and
/// messages are not status inputs.
pub fn git_metadata_matters(rel: &Path) -> bool {
    let mut parts = rel.components();
    let Some(Component::Normal(first)) = parts.next() else {
        return false;
    };
    let Some(first) = first.to_str() else {
        return false;
    };
    if rel
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_lock)
    {
        return false;
    }
    matches!(
        first,
        "HEAD"
            | "index"
            | "packed-refs"
            | "MERGE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "REBASE_HEAD"
            | "refs"
            | "rebase-merge"
            | "rebase-apply"
    )
}

/// Whether a git metadata change means the set of tracked paths, and so the
/// set of directories to watch, may have changed.
pub fn changes_tracked_set(rel: &Path) -> bool {
    matches!(rel.to_str(), Some("index" | "HEAD"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matters(path: &str) -> bool {
        git_metadata_matters(Path::new(path))
    }

    #[test]
    fn head_index_and_refs_matter() {
        for path in [
            "HEAD",
            "index",
            "packed-refs",
            "refs/heads/main",
            "refs/heads/feature/x",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REBASE_HEAD",
            "rebase-merge/git-rebase-todo",
            "rebase-apply/next",
        ] {
            assert!(matters(path), "{path} should matter");
        }
    }

    #[test]
    fn lock_files_objects_logs_and_messages_do_not() {
        for path in [
            "index.lock",
            "refs/heads/main.lock",
            "objects/ab/cdef",
            "logs/HEAD",
            "COMMIT_EDITMSG",
            "FETCH_HEAD",
            "ORIG_HEAD",
            "config",
            "",
        ] {
            assert!(!matters(path), "{path} should not matter");
        }
    }

    #[test]
    fn dot_git_components_are_git_state() {
        assert!(is_inside_dot_git(Path::new(".git")));
        assert!(is_inside_dot_git(Path::new("sub/.git/index")));
        assert!(!is_inside_dot_git(Path::new("sub/.gitignore")));
        assert!(!is_inside_dot_git(Path::new(".github/ci.yml")));
    }

    #[test]
    fn only_index_and_head_rebuild_the_watch_set() {
        assert!(changes_tracked_set(Path::new("index")));
        assert!(changes_tracked_set(Path::new("HEAD")));
        assert!(!changes_tracked_set(Path::new("refs/heads/main")));
    }
}
