//! The remote operations (fetch, pull, push with lease) as Git argument lists,
//! and the choices made before Git runs. Pure: nothing here starts a process
//! or reads a repository. The adapter reads the branch, its upstream and the
//! remotes, then asks these functions what to run; the runs against real
//! remotes are in `tests/remote_real_git.rs`.

use gm_schema::OperationId;

use crate::port::{GitError, RemoteRun};

/// A remote operation the adapter has checked, waiting for a thread of its own.
pub struct RemoteJob {
    pub id: OperationId,
    pub run: RemoteRun,
}

impl RemoteJob {
    /// Runs the operation to its end. The caller passes the report to the engine.
    pub fn run(self) -> RemoteReport {
        RemoteReport {
            id: self.id,
            outcome: (self.run)().map_err(|GitError(message)| message),
        }
    }
}

/// The answer of a remote operation: Git's own error text when it failed.
pub struct RemoteReport {
    pub id: OperationId,
    pub outcome: Result<(), String>,
}

/// `git fetch --all --prune`.
#[must_use]
pub fn fetch_args() -> Vec<String> {
    argv(&["fetch", "--all", "--prune"])
}

/// `git pull --ff-only`, once the branch is known to have an upstream. The
/// flag makes Git stop unless the branch can fast-forward, so a merge commit or
/// a rebase is never made, whatever `pull.rebase` says.
///
/// # Errors
///
/// When HEAD is detached, or the branch has no upstream to pull from.
pub fn pull_args(branch: Option<&str>, has_upstream: bool) -> Result<Vec<String>, String> {
    match branch {
        None => Err("HEAD is detached, so there is no branch to pull into".to_owned()),
        Some(branch) if !has_upstream => Err(format!(
            "{branch} has no upstream, so there is nothing to pull from"
        )),
        Some(_) => Ok(argv(&["pull", "--ff-only"])),
    }
}

/// `git push --force-with-lease` of the current branch alone. With an upstream
/// it goes to the branch that upstream names; with none and exactly one remote,
/// it goes there and becomes the upstream. The refspec is always the branch
/// itself, so no other branch and no tag moves, and `--no-follow-tags` keeps
/// `push.followTags` from adding tags.
///
/// `upstream` is the remote and the full name of the remote branch (its
/// `branch.<name>.remote` and `branch.<name>.merge`) when the branch has one.
///
/// # Errors
///
/// When HEAD is detached, or the branch has no upstream and there is no remote
/// or more than one, so the target would be a guess.
pub fn push_args(
    branch: Option<&str>,
    upstream: Option<(&str, &str)>,
    remotes: &[String],
) -> Result<Vec<String>, String> {
    let Some(branch) = branch else {
        return Err("HEAD is detached, so there is no branch to push".to_owned());
    };
    let mut args = argv(&["push", "--no-follow-tags", "--force-with-lease"]);
    if let Some((remote, merge)) = upstream {
        args.push(remote.to_owned());
        args.push(format!("refs/heads/{branch}:{merge}"));
        return Ok(args);
    }
    match remotes {
        [remote] => {
            args.push("--set-upstream".to_owned());
            args.push(remote.clone());
            args.push(format!("refs/heads/{branch}:refs/heads/{branch}"));
            Ok(args)
        }
        [] => Err(format!(
            "{branch} has no upstream, and the repository has no remote to push it to"
        )),
        _ => Err(format!(
            "{branch} has no upstream, and the repository has {} remotes, so nothing is pushed; push it once with git push -u <remote> {branch}",
            remotes.len()
        )),
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn fetch_fetches_every_remote_and_prunes() {
        assert_eq!(fetch_args(), ["fetch", "--all", "--prune"]);
    }

    #[test]
    fn pull_is_fast_forward_only() {
        assert_eq!(
            pull_args(Some("main"), true),
            Ok(vec!["pull".to_owned(), "--ff-only".to_owned()])
        );
    }

    #[test]
    fn pull_refuses_a_detached_head_and_a_branch_without_upstream() {
        assert!(pull_args(None, false).unwrap_err().contains("detached"));
        let message = pull_args(Some("feature"), false).unwrap_err();
        assert_eq!(
            message,
            "feature has no upstream, so there is nothing to pull from"
        );
    }

    #[test]
    fn push_with_upstream_names_the_remote_branch_in_the_refspec() {
        let args = push_args(
            Some("feature"),
            Some(("origin", "refs/heads/feat")),
            &names(&["origin", "fork"]),
        );
        assert_eq!(
            args,
            Ok(names(&[
                "push",
                "--no-follow-tags",
                "--force-with-lease",
                "origin",
                "refs/heads/feature:refs/heads/feat",
            ]))
        );
    }

    #[test]
    fn push_without_upstream_and_one_remote_sets_the_upstream() {
        let args = push_args(Some("feature"), None, &names(&["origin"]));
        assert_eq!(
            args,
            Ok(names(&[
                "push",
                "--no-follow-tags",
                "--force-with-lease",
                "--set-upstream",
                "origin",
                "refs/heads/feature:refs/heads/feature",
            ]))
        );
    }

    #[test]
    fn push_without_upstream_and_no_or_several_remotes_is_refused() {
        let none = push_args(Some("feature"), None, &[]).unwrap_err();
        assert!(none.contains("no remote"), "{none}");
        let several = push_args(Some("feature"), None, &names(&["origin", "fork"])).unwrap_err();
        assert!(several.contains("2 remotes"), "{several}");
        assert!(
            several.contains("git push -u <remote> feature"),
            "{several}"
        );
    }

    #[test]
    fn push_from_a_detached_head_is_refused() {
        let message = push_args(
            None,
            Some(("origin", "refs/heads/main")),
            &names(&["origin"]),
        )
        .unwrap_err();
        assert!(message.contains("detached"), "{message}");
    }
}
