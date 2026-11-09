//! Where a workspace's definitions live, and which of them a member's clone sees (D214 (5), D174,
//! D180 (2)). A workspace repository keeps its gates and reviewers in `.git/gitmaster/`, as any
//! repository does. A member's clone lists the workspaces it belongs to in the pointer file of its
//! common directory (`gitmaster/workspaces`); from that list this module finds the definitions
//! that apply to the clone, and the member folder each one is. Nothing here is written, and no
//! engine path calls it yet.

use std::path::{Path, PathBuf};

use crate::workspace::{self, Workspaces};
use crate::{GitPort, workspace_pointer};

/// The most member names one definition names (D214 (5)).
pub const MEMBER_COUNT_LIMIT: usize = 20;

/// The definitions of one workspace repository, as the gate and reviewer loaders read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDefs {
    /// The workspace repository's `<git dir>/gitmaster`, which holds `gates.json` and `reviewers/`.
    pub dir: PathBuf,
    /// The workspace's folder name, shown in errors.
    pub name: String,
}

/// Checks a member folder name as a definition names it (D214 (5)). It is one folder name the
/// workspace accepts: no `/`, not starting with `.` or `-`. A comma is refused, since a reviewer
/// file lists its members separated by commas, and so are control characters and a space at
/// either end, which the lists would not keep.
pub fn validate_member(name: &str) -> Result<(), String> {
    if !workspace::valid_name(name) {
        return Err("a member is one folder name: no '/', not starting with '.' or '-'".to_owned());
    }
    if name.contains(',') {
        return Err("a member name cannot contain a comma".to_owned());
    }
    if name.chars().any(char::is_control) {
        return Err("a member name cannot contain a control character".to_owned());
    }
    if name.trim() != name {
        return Err("a member name cannot start or end with a space".to_owned());
    }
    Ok(())
}

/// The workspaces a clone belongs to, from the pointer file of its common directory, in file
/// order. A missing file names none, and a path that is no longer a directory is skipped.
pub fn workspaces_of_clone(common_dir: &Path) -> Vec<PathBuf> {
    workspace_pointer::read(common_dir)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// The definitions that apply to the clone whose main checkout is `clone_root` (D214 (5)). For
/// each of `workspaces` that is still a workspace, every member whose main checkout (the `url`
/// its `.gitmodules` records) is `clone_root` gives one entry: the workspace's definitions and
/// the member's folder name. A workspace that is gone, or whose members or Git directory cannot
/// be read, gives nothing.
pub fn defs_for_member(
    git: &dyn GitPort,
    workspaces: &[PathBuf],
    clone_root: &Path,
) -> Vec<(WorkspaceDefs, String)> {
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
        let Ok(common) = git.common_dir(ws) else {
            continue;
        };
        let defs = WorkspaceDefs {
            dir: common.join("gitmaster"),
            name: folder_name(ws),
        };
        found.extend(matching.into_iter().map(|member| (defs.clone(), member)));
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
    fn a_member_is_one_folder_name_without_a_comma_or_edge_space() {
        for name in ["api", "web-app", "my service", "v2.1", "Docs"] {
            assert!(validate_member(name).is_ok(), "{name}");
        }
        for name in [
            "", "../x", "a/b", ".hidden", "-dash", "a,b", " api", "api ", "a\tb",
        ] {
            assert!(validate_member(name).is_err(), "{name:?}");
        }
    }

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
