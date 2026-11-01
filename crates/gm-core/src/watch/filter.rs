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
/// worktree, which holds `packed-refs`, `refs/` and `worktrees/`) can change
/// the status or the worktree list. `rel` is relative to either directory.
/// Lock files, objects, logs and messages are not status inputs.
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
            | "worktrees"
    )
}

/// Whether `rel`, relative to the common dir, is the thread database or its
/// write-ahead log (design/threads.md). Those two are the only files of
/// `gitmaster/` that the watcher reacts to; the others there are not threads.
pub fn threads_database_changed(rel: &Path) -> bool {
    let mut parts = rel.components();
    let (Some(Component::Normal(dir)), Some(Component::Normal(file)), None) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    dir.to_str() == Some("gitmaster")
        && matches!(file.to_str(), Some("threads.db" | "threads.db-wal"))
}

/// Whether `rel`, relative to the common dir, is the `gitmaster` folder itself,
/// which is created by the first thread capture.
pub fn threads_dir_appeared(rel: &Path) -> bool {
    rel == Path::new("gitmaster")
}

/// Whether `rel`, relative to the git dir or the common dir, may move a branch or another ref:
/// a loose ref under `refs/`, or `packed-refs`. Lock files do not count (`WF3a`, `refs.changed`).
pub fn refs_changed(rel: &Path) -> bool {
    let is_ref = rel.starts_with("refs") || rel == Path::new("packed-refs");
    let is_lock = rel
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_lock);
    is_ref && !is_lock
}

/// Whether a git metadata change means the set of tracked paths, and so the
/// set of directories to watch, may have changed.
pub fn changes_tracked_set(rel: &Path) -> bool {
    matches!(rel.to_str(), Some("index" | "HEAD"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_thread_database_and_its_wal_are_threads_changes() {
        assert!(threads_database_changed(Path::new("gitmaster/threads.db")));
        assert!(threads_database_changed(Path::new(
            "gitmaster/threads.db-wal"
        )));
        assert!(!threads_database_changed(Path::new(
            "gitmaster/threads.db-shm"
        )));
        assert!(!threads_database_changed(Path::new("gitmaster/review.db")));
        assert!(!threads_database_changed(Path::new(
            "gitmaster/state.db-wal"
        )));
        assert!(!threads_database_changed(Path::new(
            "gitmaster/threads.db/x"
        )));
        assert!(!threads_database_changed(Path::new("threads.db")));
        assert!(!threads_database_changed(Path::new("gitmaster")));
        assert!(threads_dir_appeared(Path::new("gitmaster")));
        assert!(!threads_dir_appeared(Path::new("gitmaster/threads.db")));
    }

    #[test]
    fn loose_refs_and_packed_refs_are_ref_changes() {
        assert!(refs_changed(Path::new("refs/heads/feat/x")));
        assert!(refs_changed(Path::new("refs/remotes/origin/main")));
        assert!(refs_changed(Path::new("packed-refs")));
        assert!(!refs_changed(Path::new("refs/heads/main.lock")));
        assert!(!refs_changed(Path::new("HEAD")));
        assert!(!refs_changed(Path::new("logs/refs/heads/main")));
    }

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
    fn the_worktree_registry_matters_but_not_its_files() {
        // The watch on `worktrees/` is non-recursive, so only its direct entries arrive.
        assert!(matters("worktrees"));
        assert!(matters("worktrees/feature"));
    }

    #[test]
    fn only_index_and_head_rebuild_the_watch_set() {
        assert!(changes_tracked_set(Path::new("index")));
        assert!(changes_tracked_set(Path::new("HEAD")));
        assert!(!changes_tracked_set(Path::new("refs/heads/main")));
    }
}
