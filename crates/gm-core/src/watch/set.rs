//! Which worktree directories to watch.
//!
//! The status lists an untracked directory as `dir/`, whatever is inside it,
//! so a change deeper in an untracked tree cannot change the status. What the
//! status shows is decided by the root and by the directories that hold
//! tracked files: a new file or a deleted file there changes the listing.
//! Those directories and their ancestors are the watch set. Ignored trees
//! (`target/`, `node_modules/`) hold no tracked files, so they are never
//! watched, and no walk of the disk is needed. A new directory appears as a
//! change in its parent, which is watched, and the set is rebuilt when the
//! index or HEAD changes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The most directories watched in a worktree. Past it the watcher degrades
/// to refresh-on-focus; each watch costs an inotify watch.
pub const MAX_WATCHED_DIRS: usize = 5_000;

#[derive(Debug, PartialEq, Eq)]
pub struct TooManyDirs {
    pub limit: usize,
}

/// The directories, relative to the root (the root is the empty path), that
/// hold tracked files or are above them. `ls_files` is `git ls-files -z`.
pub fn watch_dirs(ls_files: &[u8], max: usize) -> Result<BTreeSet<PathBuf>, TooManyDirs> {
    let mut dirs = BTreeSet::new();
    dirs.insert(PathBuf::new());
    for entry in ls_files.split(|&byte| byte == 0).filter(|e| !e.is_empty()) {
        let path = String::from_utf8_lossy(entry);
        let mut dir = Path::new(path.as_ref()).parent();
        while let Some(current) = dir {
            if current.as_os_str().is_empty() || dirs.contains(current) {
                break;
            }
            dirs.insert(current.to_path_buf());
            if dirs.len() > max {
                return Err(TooManyDirs { limit: max });
            }
            dir = current.parent();
        }
    }
    if dirs.len() > max {
        return Err(TooManyDirs { limit: max });
    }
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(dirs: &BTreeSet<PathBuf>) -> Vec<&str> {
        dirs.iter().map(|d| d.to_str().unwrap()).collect()
    }

    #[test]
    fn root_and_every_ancestor_of_a_tracked_file() {
        let listing = b"README.md\0src/main.rs\0src/ui/view.rs\0docs/a/b.md\0";
        let dirs = watch_dirs(listing, MAX_WATCHED_DIRS).unwrap();
        assert_eq!(rel(&dirs), ["", "docs", "docs/a", "src", "src/ui"]);
    }

    #[test]
    fn a_repository_without_subdirectories_watches_only_the_root() {
        let dirs = watch_dirs(b"a.txt\0b.txt\0", MAX_WATCHED_DIRS).unwrap();
        assert_eq!(rel(&dirs), [""]);
    }

    #[test]
    fn an_empty_index_still_watches_the_root() {
        let dirs = watch_dirs(b"", MAX_WATCHED_DIRS).unwrap();
        assert_eq!(rel(&dirs), [""]);
    }

    #[test]
    fn the_cap_is_a_hard_limit_on_directories() {
        // Root plus "a" and "b": three directories.
        let listing = b"a/x\0b/y\0";
        assert_eq!(watch_dirs(listing, 3).unwrap().len(), 3);
        assert_eq!(watch_dirs(listing, 2), Err(TooManyDirs { limit: 2 }));
    }

    #[test]
    fn a_tiny_cap_refuses_a_nested_tree() {
        assert_eq!(watch_dirs(b"sub/a.txt\0", 1), Err(TooManyDirs { limit: 1 }));
    }
}
