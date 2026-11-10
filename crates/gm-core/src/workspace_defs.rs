//! Where a workspace's definitions live, and which of them a member's clone sees (D214 (5), D174,
//! D180 (2)). A workspace repository keeps its reviewers in `.git/gitmaster/`, as any
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
    /// The workspace repository's `<git dir>/gitmaster`, which holds `reviewers/`.
    pub dir: PathBuf,
    /// The workspace's folder name, shown in errors.
    pub name: String,
    /// The member folder the definitions are read for (D214 (5)). `None` reads every definition
    /// as it is written; `Some` applies the member filter of `apply_member` to each one read.
    pub member: Option<String>,
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
            member: None,
        };
        found.extend(matching.into_iter().map(|member| (defs.clone(), member)));
    }
    found
}

/// The workspace layer of the clone whose Git common directory is `common_dir` (D214 (5)): one
/// entry per member of a workspace the clone belongs to, each with its member folder. The main
/// checkout of the clone is the parent of its common directory, the folder that holds `.git`. A
/// clone without a pointer file gives nothing and runs no Git call.
pub fn workspace_layer(git: &dyn GitPort, common_dir: &Path) -> Vec<WorkspaceDefs> {
    let workspaces = workspaces_of_clone(common_dir);
    if workspaces.is_empty() {
        return Vec::new();
    }
    let Some(clone_root) = common_dir.parent() else {
        return Vec::new();
    };
    defs_for_member(git, &workspaces, clone_root)
        .into_iter()
        .map(|(defs, member)| WorkspaceDefs {
            member: Some(member),
            ..defs
        })
        .collect()
}

/// The workspace layers a check of a checkout reads (D214 (5)): the explicit one when a workspace
/// names the member it judges, else the layers of the clone the checkout belongs to.
pub fn layers_for(
    git: &dyn GitPort,
    common_dir: &Path,
    explicit: Option<&WorkspaceDefs>,
) -> Vec<WorkspaceDefs> {
    match explicit {
        Some(defs) => vec![defs.clone()],
        None => workspace_layer(git, common_dir),
    }
}

/// The definitions of workspace `ws` for its member folder `member`, when `member` is one of its
/// members. The workspace's own Git common directory holds the definitions (`gitmaster/`).
pub fn member_layer(git: &dyn GitPort, ws: &Path, member: &str) -> Result<WorkspaceDefs, String> {
    let common = git.common_dir(ws).map_err(|e| e.0)?;
    Ok(WorkspaceDefs {
        dir: common.join("gitmaster"),
        name: folder_name(ws),
        member: Some(member.to_owned()),
    })
}

/// Applies the member of a workspace layer to the members one of its definitions names (D214
/// (5)). A definition for every member, or for this member, is for the member here, so its names
/// are cleared and it applies with no member, as the check of one member judges it. A definition
/// for other members stays as it is: it is not applicable here, and says which members it is for.
/// A layer without a member changes nothing.
pub fn apply_member(layer: Option<&str>, members: &mut Vec<String>) {
    let Some(member) = layer else {
        return;
    };
    if members.is_empty() || members.iter().any(|name| name == member) {
        members.clear();
    }
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

    #[test]
    fn a_member_layer_applies_to_its_own_definitions_and_keeps_the_others_for_their_members() {
        let mut for_all = Vec::new();
        apply_member(Some("api"), &mut for_all);
        assert!(for_all.is_empty());

        let mut for_api = vec!["api".to_owned(), "web".to_owned()];
        apply_member(Some("api"), &mut for_api);
        assert!(for_api.is_empty());

        let mut for_web = vec!["web".to_owned()];
        apply_member(Some("api"), &mut for_web);
        assert_eq!(for_web, ["web"]);

        let mut untouched = vec!["web".to_owned()];
        apply_member(None, &mut untouched);
        assert_eq!(untouched, ["web"]);
    }

    #[test]
    fn a_clone_without_a_pointer_file_has_no_workspace_layer() {
        let root = tempfile::tempdir().expect("tempdir");
        let common = root.path().join("clone").join(".git");
        assert!(workspace_layer(&crate::git::SubprocessGit::default(), &common).is_empty());
        assert_eq!(
            layers_for(&crate::git::SubprocessGit::default(), &common, None),
            Vec::new()
        );
    }
}
