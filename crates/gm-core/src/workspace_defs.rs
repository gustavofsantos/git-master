//! Which workspaces a member's clone belongs to (D214 (5), D174, D180 (2)). A member's clone lists
//! the workspaces it belongs to in the pointer file of its common directory (`gitmaster/workspaces`);
//! from that list this module finds the members that are this clone, with the folder name of each
//! workspace. Nothing here is written. The workspace comments read it (`workspace_comments`).

use std::path::{Path, PathBuf};

use crate::workspace::Workspaces;
use crate::{GitPort, workspace_pointer};

/// The workspaces a clone belongs to, from the pointer file of its common directory, in file
/// order. A missing file names none, and a path that is no longer a directory is skipped.
pub fn workspaces_of_clone(common_dir: &Path) -> Vec<PathBuf> {
    workspace_pointer::read(common_dir)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// The members that are the clone whose main checkout is `clone_root` (D214 (5)): for each of
/// `workspaces` that is still a workspace, every member whose main checkout (the `url` its
/// `.gitmodules` records) is `clone_root` gives one `(workspace folder name, member folder name)`.
/// A workspace that is gone, or whose members or Git directory cannot be read, gives nothing.
pub fn defs_for_member(
    git: &dyn GitPort,
    workspaces: &[PathBuf],
    clone_root: &Path,
) -> Vec<(String, String)> {
    let reader = Workspaces::new(git, None);
    let clone = canonical(clone_root);
    let mut found = Vec::new();
    for ws in workspaces {
        if !reader.is_workspace(ws) {
            continue;
        }
        let Ok(members) = reader.members(ws) else {
            continue;
        };
        let matching: Vec<String> = members
            .into_iter()
            .filter(|member| canonical(Path::new(&member.url)) == clone)
            .map(|member| member.path)
            .collect();
        if matching.is_empty() {
            continue;
        }
        let name = folder_name(ws);
        found.extend(matching.into_iter().map(|member| (name.clone(), member)));
    }
    found
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pointer_file_names_no_workspace() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(workspaces_of_clone(&root.path().join("common")).is_empty());
    }

    #[test]
    fn the_pointer_file_lists_existing_folders_and_skips_the_others() {
        let root = tempfile::tempdir().expect("tempdir");
        let common = root.path().join("common");
        let kept = root.path().join("kept");
        let gone = root.path().join("gone");
        std::fs::create_dir_all(&kept).expect("dir");
        workspace_pointer::add(&common, &gone).expect("add gone");
        workspace_pointer::add(&common, &kept).expect("add kept");
        assert_eq!(workspaces_of_clone(&common), [kept]);
    }
}
