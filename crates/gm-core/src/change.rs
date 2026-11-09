//! The change of a branch (D214 (1)): the files that differ between the merge base of the branch
//! with its base and the tip of the branch. Reviewers and gates with `when.paths` are checked
//! against it, and so is the fail-closed rule: a change whose files are unknown counts as
//! touching everything.

use std::path::Path;

use crate::{GitError, GitPort};

/// The default branches a branch is measured against when nothing nearer is known, in order.
const DEFAULT_BRANCHES: [&str; 4] = ["main", "master", "develop", "trunk"];

/// The change of one branch against its base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The ref the branch is measured against: a local branch, or a remote-tracking ref such as
    /// `origin/main`.
    pub base_ref: String,
    /// The full id of the merge base of the base and the branch.
    pub merge_base: String,
    /// The paths that differ between the merge base and the tip, sorted; `None` when there are
    /// more than 10,000 of them (the caller then treats the change as touching everything).
    pub paths: Option<Vec<String>>,
}

/// The ref `branch` is measured against, the first of these that exists (D214 (1)):
///
/// 1. its parent in the stack model (`branch.<name>.stackParent`), when that local branch exists;
/// 2. its upstream, unless the upstream is the branch's own remote counterpart (`origin/<name>`
///    for the branch `<name>`), which would measure only the commits not pushed yet;
/// 3. the default branch of the remote, `origin/HEAD`, under the same counterpart rule;
/// 4. the first local branch of `main`, `master`, `develop` and `trunk` that exists and is not
///    the branch itself.
///
/// `None` when none applies, which includes the default branch itself. An adapter that cannot
/// read the stack links is treated as having none.
pub fn base_of(git: &dyn GitPort, root: &Path, branch: &str) -> Result<Option<String>, GitError> {
    let parent = git
        .stack_links(root)
        .unwrap_or_default()
        .into_iter()
        .find(|link| link.branch == branch)
        .map(|link| link.parent)
        .filter(|parent| parent != branch)
        .filter(|parent| git.branch_tip(root, parent).ok().flatten().is_some());
    if let Some(parent) = parent {
        return Ok(Some(parent));
    }
    let upstream = git
        .upstream_of(root, branch)?
        .filter(|upstream| usable_remote(git, root, upstream, branch));
    if let Some(upstream) = upstream {
        return Ok(Some(upstream));
    }
    let head = git
        .origin_head(root)?
        .filter(|head| usable_remote(git, root, head, branch));
    if let Some(head) = head {
        return Ok(Some(head));
    }
    for name in DEFAULT_BRANCHES {
        if name != branch && git.branch_tip(root, name)?.is_some() {
            return Ok(Some(name.to_owned()));
        }
    }
    Ok(None)
}

/// Whether the remote-tracking ref `reference` can be a base for `branch`: it resolves, and it is
/// not the branch's own remote counterpart.
fn usable_remote(git: &dyn GitPort, root: &Path, reference: &str, branch: &str) -> bool {
    let own_counterpart = reference
        .split_once('/')
        .is_some_and(|(_, name)| name == branch);
    !own_counterpart && git.resolve_commit(root, reference).is_ok()
}

/// The change of `branch`: `None` when it has no base, no merge base with it, or it is its own
/// base. Otherwise the base, the merge base and the changed paths (D214 (1)). Never records
/// anything.
pub fn change_of(git: &dyn GitPort, root: &Path, branch: &str) -> Result<Option<Change>, GitError> {
    let Some(base_ref) = base_of(git, root, branch)? else {
        return Ok(None);
    };
    let Some(tip) = git.branch_tip(root, branch)? else {
        return Ok(None);
    };
    let base = git.resolve_commit(root, &base_ref)?;
    let Some(merge_base) = git.merge_base_of_commits(root, &base, &tip)? else {
        return Ok(None);
    };
    let paths = git.changed_paths(root, &merge_base, &tip)?;
    Ok(Some(Change {
        base_ref,
        merge_base,
        paths,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use gm_schema::RepoStatus;

    use super::*;
    use crate::stack::StackLink;

    /// Answers the ref facts of one repository from maps, and nothing else.
    #[derive(Default)]
    struct Refs {
        links: Vec<StackLink>,
        /// Local branch tips, by name; a remote-tracking ref resolves through `remote`.
        tips: HashMap<String, String>,
        upstreams: HashMap<String, String>,
        origin_head: Option<String>,
        /// Remote-tracking refs that resolve, by name, to a commit.
        remote: HashMap<String, String>,
        /// The merge base every pair of commits answers with.
        merge_base: Option<String>,
        /// What `changed_paths` answers.
        paths: Option<Vec<String>>,
    }

    impl GitPort for Refs {
        fn resolve_root(&self, path: &Path) -> Result<PathBuf, GitError> {
            Ok(path.to_path_buf())
        }

        fn status(&self, _root: &Path) -> Result<RepoStatus, GitError> {
            Ok(RepoStatus::default())
        }

        fn stack_links(&self, _root: &Path) -> Result<Vec<StackLink>, GitError> {
            Ok(self.links.clone())
        }

        fn branch_tip(&self, _root: &Path, name: &str) -> Result<Option<String>, GitError> {
            Ok(self.tips.get(name).cloned())
        }

        fn upstream_of(&self, _root: &Path, branch: &str) -> Result<Option<String>, GitError> {
            Ok(self.upstreams.get(branch).cloned())
        }

        fn origin_head(&self, _root: &Path) -> Result<Option<String>, GitError> {
            Ok(self.origin_head.clone())
        }

        fn resolve_commit(&self, _root: &Path, rev: &str) -> Result<String, GitError> {
            self.tips
                .get(rev)
                .or_else(|| self.remote.get(rev))
                .cloned()
                .ok_or_else(|| GitError(format!("{rev} is not a commit")))
        }

        fn merge_base_of_commits(
            &self,
            _root: &Path,
            _one: &str,
            _two: &str,
        ) -> Result<Option<String>, GitError> {
            Ok(self.merge_base.clone())
        }

        fn changed_paths(
            &self,
            _root: &Path,
            _from: &str,
            _to: &str,
        ) -> Result<Option<Vec<String>>, GitError> {
            Ok(self.paths.clone())
        }
    }

    fn link(branch: &str, parent: &str) -> StackLink {
        StackLink {
            branch: branch.to_owned(),
            parent: parent.to_owned(),
            base: None,
        }
    }

    fn tips(names: &[&str]) -> HashMap<String, String> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), format!("{name}-tip")))
            .collect()
    }

    fn base(refs: &Refs, branch: &str) -> Option<String> {
        base_of(refs, Path::new("/repo"), branch).unwrap()
    }

    #[test]
    fn the_stack_parent_wins_over_everything_else() {
        let refs = Refs {
            links: vec![link("feat", "parent")],
            tips: tips(&["feat", "parent", "main"]),
            upstreams: HashMap::from([("feat".to_owned(), "origin/feat2".to_owned())]),
            origin_head: Some("origin/main".to_owned()),
            remote: HashMap::from([("origin/feat2".to_owned(), "x".to_owned())]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "feat").as_deref(), Some("parent"));
    }

    #[test]
    fn a_stack_parent_that_is_gone_falls_through_to_the_upstream() {
        let refs = Refs {
            links: vec![link("feat", "gone")],
            tips: tips(&["feat", "main"]),
            upstreams: HashMap::from([("feat".to_owned(), "origin/upstream".to_owned())]),
            remote: HashMap::from([("origin/upstream".to_owned(), "u".to_owned())]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "feat").as_deref(), Some("origin/upstream"));
    }

    #[test]
    fn an_upstream_that_is_the_own_remote_counterpart_is_ignored() {
        let refs = Refs {
            tips: tips(&["feat", "main"]),
            upstreams: HashMap::from([("feat/x".to_owned(), "origin/feat/x".to_owned())]),
            remote: HashMap::from([("origin/feat/x".to_owned(), "c".to_owned())]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "feat/x").as_deref(), Some("main"));
    }

    #[test]
    fn origin_head_is_used_when_there_is_no_nearer_base() {
        let refs = Refs {
            tips: tips(&["feat", "main"]),
            origin_head: Some("origin/trunk-two".to_owned()),
            remote: HashMap::from([("origin/trunk-two".to_owned(), "t".to_owned())]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "feat").as_deref(), Some("origin/trunk-two"));
    }

    #[test]
    fn origin_head_is_skipped_for_the_branch_itself() {
        let refs = Refs {
            tips: tips(&["main", "develop"]),
            origin_head: Some("origin/main".to_owned()),
            remote: HashMap::from([("origin/main".to_owned(), "m".to_owned())]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "main").as_deref(), Some("develop"));
    }

    #[test]
    fn the_default_branches_are_tried_in_order_and_not_the_branch_itself() {
        let refs = Refs {
            tips: tips(&["feat", "trunk", "develop"]),
            ..Refs::default()
        };
        assert_eq!(base(&refs, "feat").as_deref(), Some("develop"));
        assert_eq!(base(&refs, "develop").as_deref(), Some("trunk"));
        let alone = Refs {
            tips: tips(&["feat"]),
            ..Refs::default()
        };
        assert_eq!(base(&alone, "feat"), None);
    }

    #[test]
    fn a_branch_with_no_base_has_no_change() {
        let refs = Refs {
            tips: tips(&["feat"]),
            ..Refs::default()
        };
        assert!(
            change_of(&refs, Path::new("/repo"), "feat")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_change_carries_the_base_the_merge_base_and_the_paths() {
        let refs = Refs {
            tips: tips(&["feat", "main"]),
            merge_base: Some("mb".to_owned()),
            paths: Some(vec!["a.rs".to_owned()]),
            ..Refs::default()
        };
        let change = change_of(&refs, Path::new("/repo"), "feat")
            .unwrap()
            .expect("a change");
        assert_eq!(change.base_ref, "main");
        assert_eq!(change.merge_base, "mb");
        assert_eq!(change.paths, Some(vec!["a.rs".to_owned()]));
    }

    #[test]
    fn no_merge_base_means_no_change() {
        let refs = Refs {
            tips: tips(&["feat", "main"]),
            ..Refs::default()
        };
        assert!(
            change_of(&refs, Path::new("/repo"), "feat")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_paths_are_carried_as_none_for_the_caller_to_fail_closed() {
        let refs = Refs {
            tips: tips(&["feat", "main"]),
            merge_base: Some("mb".to_owned()),
            paths: None,
            ..Refs::default()
        };
        let change = change_of(&refs, Path::new("/repo"), "feat")
            .unwrap()
            .expect("a change");
        assert_eq!(change.paths, None);
    }
}
