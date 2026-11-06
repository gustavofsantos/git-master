//! The members of a workspace (D180): the submodules checked out in the workspace's worktree.
//! A thread that runs at the workspace root names the member each of its edits and commits
//! belongs to. A repository without a `.gitmodules` file has no members and costs no Git call.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::submodule_files::gitlinks;

/// A checked-out member of a workspace, with the commit its `HEAD` named when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// The member's folder, relative to the workspace root.
    pub name: String,
    /// The commit `HEAD` names in the member; `None` on an unborn branch.
    #[serde(default)]
    pub head: Option<String>,
}

/// The checked-out members of the repository at `root`, with their HEADs. A member is a gitlink
/// of the index whose folder holds its own `.git`. Empty without a `.gitmodules` file. Costs one
/// `git ls-files` and one `git rev-parse` per checked-out member.
#[must_use]
pub fn checked_out(root: &Path) -> Vec<Member> {
    if !root.join(".gitmodules").exists() {
        return Vec::new();
    }
    let Some(stage) = git_bytes(root, &["ls-files", "-z", "--stage"]) else {
        return Vec::new();
    };
    gitlinks(&stage)
        .into_iter()
        .filter(|name| root.join(name).join(".git").exists())
        .map(|name| {
            let head = git_bytes(&root.join(&name), &["rev-parse", "--verify", "-q", "HEAD"])
                .and_then(|out| String::from_utf8(out).ok())
                .map(|out| out.trim().to_owned())
                .filter(|head| super::pipeline::is_commit_id(head));
            Member { name, head }
        })
        .collect()
}

/// The member `path` (relative to the workspace root) is in: the member whose folder is its
/// longest prefix. `None` for the workspace's own files.
#[must_use]
pub fn member_of<'a>(path: &str, members: &'a [Member]) -> Option<&'a str> {
    members
        .iter()
        .filter(|member| {
            path.strip_prefix(member.name.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|member| member.name.len())
        .map(|member| member.name.as_str())
}

/// The standard output of `git` run in `cwd`, or `None` when it fails or is missing.
fn git_bytes(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str) -> Member {
        Member {
            name: name.to_owned(),
            head: None,
        }
    }

    #[test]
    fn a_path_is_in_the_member_whose_folder_is_its_longest_prefix() {
        let members = [member("billing"), member("billing/vendor")];

        assert_eq!(member_of("billing/src/x.rs", &members), Some("billing"));
        assert_eq!(
            member_of("billing/vendor/lib.rs", &members),
            Some("billing/vendor")
        );
    }

    #[test]
    fn the_workspace_own_files_and_a_folder_named_alike_have_no_member() {
        let members = [member("billing")];

        assert_eq!(member_of("README.md", &members), None);
        assert_eq!(member_of("billing", &members), None);
        assert_eq!(member_of("billing-old/a.rs", &members), None);
        assert_eq!(member_of("billing/a.rs", &[]), None);
    }
}
