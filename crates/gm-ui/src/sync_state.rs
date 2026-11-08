//! The top bar's state group (UI redesign, slice 1): one sync button whose label is the
//! state of the checked-out branch against its upstream, and the Restack button that shows
//! only while the stack of HEAD is behind. Pure: the title bar draws what this says, and the
//! button runs registry actions, so the rules are the menus' and the palette's.

use std::time::Duration;

use gm_schema::{StackSnapshot, StackState};

use crate::action_registry::Action;

/// How often the window fetches by itself while a repository with an upstream is open.
pub(crate) const BACKGROUND_FETCH: Duration = Duration::from_mins(10);

/// What the sync button says and runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sync {
    pub(crate) label: String,
    /// What a click on the button runs; `None` while there is nothing to do (Up to date runs
    /// a Fetch, so only a running operation leaves it `None`).
    pub(crate) primary: Option<Action>,
    /// The branch has commits to send or to take: the button is drawn as the accent.
    pub(crate) pending: bool,
}

/// The branch's standing as the status reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Standing<'a> {
    pub(crate) branch: Option<&'a str>,
    pub(crate) upstream: Option<&'a str>,
    pub(crate) ahead: u32,
    pub(crate) behind: u32,
    /// The remote operation in flight, if any.
    pub(crate) running: Option<Action>,
}

/// The sync button for `standing`. Behind comes first when the branch has diverged: a push
/// with lease would be refused until the remote commits are in.
#[must_use]
pub(crate) fn sync(standing: Standing<'_>) -> Sync {
    if let Some(running) = standing.running {
        let label = match running {
            Action::Fetch => "Fetching…",
            Action::Pull => "Pulling…",
            _ => "Pushing…",
        };
        return Sync {
            label: label.to_owned(),
            primary: None,
            pending: false,
        };
    }
    if standing.branch.is_none() {
        return Sync {
            label: "Detached".to_owned(),
            primary: Some(Action::Fetch),
            pending: false,
        };
    }
    if standing.upstream.is_none() {
        return Sync {
            label: "Publish".to_owned(),
            primary: Some(Action::PushWithLease),
            pending: true,
        };
    }
    match (standing.ahead, standing.behind) {
        (0, 0) => Sync {
            label: "Up to date".to_owned(),
            primary: Some(Action::Fetch),
            pending: false,
        },
        (ahead, 0) => Sync {
            label: format!("Push ↑{ahead}"),
            primary: Some(Action::PushWithLease),
            pending: true,
        },
        (0, behind) => Sync {
            label: format!("Pull ↓{behind}"),
            primary: Some(Action::Pull),
            pending: true,
        },
        (ahead, behind) => Sync {
            label: format!("Pull ↓{behind} ↑{ahead}"),
            primary: Some(Action::Pull),
            pending: true,
        },
    }
}

/// The actions of the sync button's menu, in order.
pub(crate) const MENU: [Action; 3] = [Action::Fetch, Action::Pull, Action::PushWithLease];

/// How many branches of the stack that holds `head` are behind their parent: the count the
/// "Restack N behind" button shows. Zero when HEAD is in no stack or the stack is current.
#[must_use]
pub(crate) fn stack_behind(snapshot: Option<&StackSnapshot>, head: Option<&str>) -> u32 {
    let (Some(snapshot), Some(head)) = (snapshot, head) else {
        return 0;
    };
    snapshot
        .stacks
        .iter()
        .find(|stack| stack.branches.iter().any(|branch| branch.name == head))
        .map_or(0, |stack| {
            let stale = stack
                .branches
                .iter()
                .filter(|branch| branch.state == StackState::Stale)
                .count();
            u32::try_from(stale).unwrap_or(u32::MAX)
        })
}

/// What the Restack button says, when it shows at all.
#[must_use]
pub(crate) fn restack_label(behind: u32) -> Option<String> {
    (behind > 0).then(|| format!("Restack {behind} behind"))
}

/// Whether the background fetch may run now: a branch with an upstream, a repository open
/// and nothing remote in flight.
#[must_use]
pub(crate) fn background_fetch_due(standing: Standing<'_>, repo_open: bool) -> bool {
    repo_open && standing.upstream.is_some() && standing.running.is_none()
}

#[cfg(test)]
mod tests {
    use gm_schema::{Stack, StackBranch};

    use super::*;

    fn standing(ahead: u32, behind: u32) -> Standing<'static> {
        Standing {
            branch: Some("main"),
            upstream: Some("origin/main"),
            ahead,
            behind,
            running: None,
        }
    }

    #[test]
    fn the_label_is_the_state_and_the_click_runs_what_the_state_needs() {
        assert_eq!(sync(standing(0, 0)).label, "Up to date");
        assert_eq!(sync(standing(0, 0)).primary, Some(Action::Fetch));
        assert!(!sync(standing(0, 0)).pending);
        assert_eq!(sync(standing(2, 0)).label, "Push ↑2");
        assert_eq!(sync(standing(2, 0)).primary, Some(Action::PushWithLease));
        assert_eq!(sync(standing(0, 3)).label, "Pull ↓3");
        assert_eq!(sync(standing(0, 3)).primary, Some(Action::Pull));
        assert_eq!(sync(standing(2, 3)).label, "Pull ↓3 ↑2");
        assert_eq!(sync(standing(2, 3)).primary, Some(Action::Pull));
    }

    #[test]
    fn a_branch_with_no_upstream_publishes_and_a_detached_head_fetches() {
        let unpublished = Standing {
            upstream: None,
            ..standing(0, 0)
        };
        assert_eq!(sync(unpublished).label, "Publish");
        assert_eq!(sync(unpublished).primary, Some(Action::PushWithLease));
        let detached = Standing {
            branch: None,
            ..standing(0, 0)
        };
        assert_eq!(sync(detached).primary, Some(Action::Fetch));
    }

    #[test]
    fn a_running_operation_names_itself_and_offers_nothing() {
        let running = Standing {
            running: Some(Action::Pull),
            ..standing(1, 1)
        };
        assert_eq!(sync(running).label, "Pulling…");
        assert_eq!(sync(running).primary, None);
        assert!(!background_fetch_due(running, true));
        assert!(background_fetch_due(standing(0, 0), true));
        assert!(!background_fetch_due(standing(0, 0), false));
    }

    fn branch(name: &str, state: StackState) -> StackBranch {
        StackBranch {
            name: name.to_owned(),
            parent: None,
            depth: 0,
            state,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        }
    }

    #[test]
    fn restack_shows_only_while_the_stack_of_head_is_behind() {
        let snapshot = StackSnapshot {
            stacks: vec![
                Stack {
                    branches: vec![
                        branch("main", StackState::Root),
                        branch("a", StackState::Stale),
                        branch("b", StackState::Stale),
                    ],
                },
                Stack {
                    branches: vec![
                        branch("dev", StackState::Root),
                        branch("c", StackState::Current),
                    ],
                },
            ],
            looped: Vec::new(),
        };
        assert_eq!(stack_behind(Some(&snapshot), Some("a")), 2);
        assert_eq!(stack_behind(Some(&snapshot), Some("c")), 0);
        assert_eq!(stack_behind(Some(&snapshot), Some("other")), 0);
        assert_eq!(stack_behind(None, Some("a")), 0);
        assert_eq!(restack_label(2), Some("Restack 2 behind".to_owned()));
        assert_eq!(restack_label(0), None);
    }
}
