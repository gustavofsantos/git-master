//! The restack plan (D14, D142). Pure: the engine reads the stacks, the tips and
//! where each branch is checked out, and supplies the replay of one commit; this
//! module walks the subtree parents first and decides what each branch does.
//! Nothing here runs Git, so the rules are tested on their own.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use gm_schema::{
    OWN_COMMITS_LIMIT, PlannedBranch, PlannedCommit, ReplayStep, RestackBlock, RestackPlan,
    StackBranch, StackSnapshot, StackState,
};
use sha2::{Digest, Sha256};

use crate::stack::StackLink;

/// What replaying one commit gave, as the adapter reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    pub subject: String,
    pub kind: ReplayKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayKind {
    Clean {
        new: String,
    },
    Dropped,
    Conflict {
        paths: Vec<String>,
    },
    /// The commit conflicted onto this base and the person resolved it (D154): it replays
    /// as `new`, and the walk goes on as after a clean replay.
    Resolved {
        new: String,
    },
    /// The commit has no single parent, so it has no change to replay.
    NotLinear,
}

/// Where a local branch is checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckedOut {
    /// In the working copy this window has open.
    Here { dirty: bool },
    /// In a linked worktree at `path`.
    Elsewhere { path: String },
}

/// Everything the plan reads, gathered by the engine.
pub struct Inputs<'a> {
    pub snapshot: &'a StackSnapshot,
    pub links: &'a [StackLink],
    /// The tip of each local branch.
    pub tips: &'a HashMap<String, String>,
    pub checked_out: &'a HashMap<String, CheckedOut>,
    /// Branches the person left out.
    pub skip: &'a HashSet<String>,
}

/// Plans a restack of `from` and the branches under it. `replay(onto, commit)`
/// replays one commit onto `onto` without referencing the result.
///
/// # Errors
/// When `from` is in no stack, or the replay fails.
pub fn plan(
    from: &str,
    inputs: &Inputs<'_>,
    replay: &mut dyn FnMut(&str, &str) -> Result<Replayed, String>,
) -> Result<RestackPlan, String> {
    let stack = inputs
        .snapshot
        .stacks
        .iter()
        .find(|stack| stack.branches.iter().any(|entry| entry.name == from))
        .ok_or_else(|| format!("{from} is in no stack."))?;
    let mut in_subtree: HashSet<&str> = HashSet::from([from]);
    let subtree: Vec<&StackBranch> = stack
        .branches
        .iter()
        .filter(|entry| {
            let wanted = entry.name == from
                || entry
                    .parent
                    .as_deref()
                    .is_some_and(|parent| in_subtree.contains(parent));
            if wanted {
                in_subtree.insert(entry.name.as_str());
            }
            wanted
        })
        .collect();

    let mut out = RestackPlan {
        from: from.to_owned(),
        ..RestackPlan::default()
    };
    let mut moved: HashMap<&str, String> = HashMap::new();
    let mut held_back: HashSet<&str> = HashSet::new();
    for entry in subtree {
        let Some(parent) = entry.parent.as_deref() else {
            continue;
        };
        let parent_moved = moved.get(parent).cloned();
        let parent_held_back = held_back.contains(parent);
        let own_trouble = matches!(
            entry.state,
            StackState::DivergedBase | StackState::ParentGone | StackState::ParentMerged
        );
        if !(parent_moved.is_some()
            || parent_held_back
            || entry.state == StackState::Stale
            || own_trouble)
        {
            continue;
        }
        let name = entry.name.as_str();
        let Some(old_tip) = inputs.tips.get(name).cloned() else {
            continue;
        };
        let Some(parent_tip) = inputs.tips.get(parent).cloned() else {
            // The parent is gone: the state above is `ParentGone`.
            out.branches.push(refused(
                entry,
                parent,
                &old_tip,
                inputs.links,
                &old_tip,
                format!("The parent {parent} is not a local branch; reparent {name} first."),
            ));
            held_back.insert(name);
            continue;
        };
        let onto = parent_moved.unwrap_or(parent_tip);
        let mut branch = PlannedBranch {
            name: name.to_owned(),
            parent: parent.to_owned(),
            old_tip: old_tip.clone(),
            old_base: link_base(inputs.links, name),
            onto: onto.clone(),
            commits: Vec::new(),
            new_tip: None,
            block: None,
        };
        if inputs.skip.contains(name) {
            branch.block = Some(RestackBlock::Skipped);
        } else if parent_held_back {
            branch.block = Some(RestackBlock::ParentBlocked {
                parent: parent.to_owned(),
            });
        } else if let Some(reason) = refusal(entry, inputs.checked_out.get(name), &mut out.stopped)
        {
            branch.block = Some(RestackBlock::Refused { reason });
        } else {
            replay_branch(entry, &mut branch, replay)?;
        }
        match (&branch.block, &branch.new_tip) {
            (None, Some(tip)) => {
                moved.insert(name, tip.clone());
            }
            _ => {
                held_back.insert(name);
            }
        }
        out.branches.push(branch);
    }
    out.plan_id = plan_id(&out);
    Ok(out)
}

/// Why `entry` cannot be restacked at all, or `None`. A dirty working copy on
/// its branch stops the whole plan instead (Q2).
fn refusal(
    entry: &StackBranch,
    placed: Option<&CheckedOut>,
    stopped: &mut Vec<String>,
) -> Option<String> {
    let name = &entry.name;
    match entry.state {
        StackState::DivergedBase => {
            return Some(format!(
                "The recorded base of {name} no longer marks where its commits start; set its parent again."
            ));
        }
        StackState::ParentGone => {
            return Some(format!("The parent of {name} is gone; reparent it first."));
        }
        StackState::ParentMerged => {
            return Some(format!(
                "The parent of {name} is already merged; reparent it to its grandparent first."
            ));
        }
        _ => {}
    }
    if entry.commits.len() >= OWN_COMMITS_LIMIT {
        return Some(format!(
            "{name} has {OWN_COMMITS_LIMIT} or more commits of its own, more than a restack replays."
        ));
    }
    match placed {
        Some(CheckedOut::Elsewhere { path }) => {
            Some(format!("{name} is checked out in the worktree at {path}."))
        }
        Some(CheckedOut::Here { dirty: true }) => {
            stopped.push(format!(
                "The working copy has uncommitted changes and {name} is checked out. Commit or stash them first."
            ));
            None
        }
        _ => None,
    }
}

fn replay_branch(
    entry: &StackBranch,
    branch: &mut PlannedBranch,
    replay: &mut dyn FnMut(&str, &str) -> Result<Replayed, String>,
) -> Result<(), String> {
    let mut cur = branch.onto.clone();
    for oid in entry.commits.iter().rev() {
        let replayed = replay(&cur, oid)?;
        let step = match replayed.kind {
            ReplayKind::Clean { new } => {
                cur.clone_from(&new);
                ReplayStep::Clean { new }
            }
            ReplayKind::Resolved { new } => {
                cur.clone_from(&new);
                ReplayStep::Resolved { new }
            }
            ReplayKind::Dropped => ReplayStep::Dropped,
            ReplayKind::Conflict { paths } => {
                branch.commits.push(PlannedCommit {
                    oid: oid.clone(),
                    subject: replayed.subject,
                    step: ReplayStep::Conflict { paths },
                });
                branch.block = Some(RestackBlock::Conflict {
                    commit: oid.clone(),
                    onto: cur.clone(),
                });
                return Ok(());
            }
            ReplayKind::NotLinear => {
                branch.block = Some(RestackBlock::Refused {
                    reason: format!(
                        "{} has a merge commit among its own commits, which restack does not replay.",
                        branch.name
                    ),
                });
                return Ok(());
            }
        };
        branch.commits.push(PlannedCommit {
            oid: oid.clone(),
            subject: replayed.subject,
            step,
        });
    }
    branch.new_tip = Some(cur);
    Ok(())
}

fn refused(
    entry: &StackBranch,
    parent: &str,
    old_tip: &str,
    links: &[StackLink],
    onto: &str,
    reason: String,
) -> PlannedBranch {
    PlannedBranch {
        name: entry.name.clone(),
        parent: parent.to_owned(),
        old_tip: old_tip.to_owned(),
        old_base: link_base(links, &entry.name),
        onto: onto.to_owned(),
        commits: Vec::new(),
        new_tip: None,
        block: Some(RestackBlock::Refused { reason }),
    }
}

fn link_base(links: &[StackLink], branch: &str) -> Option<String> {
    links
        .iter()
        .find(|link| link.branch == branch)
        .and_then(|link| link.base.clone())
}

/// A digest of everything apply relies on: each branch, its tip and base now,
/// its commits and what their replay gave, and the tip it lands on when its
/// parent is not restacked. A clean replay's new commit is left out: it carries
/// the time of the replay, so planning again gives other ids for the same plan
/// (D66). A resolved one is in: a resolution is an input of the plan, so a
/// different resolution is a different plan (D154).
fn plan_id(plan: &RestackPlan) -> String {
    let planned: HashSet<&str> = plan
        .branches
        .iter()
        .map(|branch| branch.name.as_str())
        .collect();
    let mut text = format!("restack:{}\n", plan.from);
    for branch in &plan.branches {
        let onto = if planned.contains(branch.parent.as_str()) {
            "-"
        } else {
            branch.onto.as_str()
        };
        let _ = writeln!(
            text,
            "{} {} {} {onto} {}",
            branch.name,
            branch.old_tip,
            branch.old_base.as_deref().unwrap_or("-"),
            branch.new_tip.is_some(),
        );
        for commit in &branch.commits {
            let step = match &commit.step {
                ReplayStep::Clean { .. } => "clean".to_owned(),
                ReplayStep::Dropped => "dropped".to_owned(),
                ReplayStep::Conflict { .. } => "conflict".to_owned(),
                ReplayStep::Resolved { new } => format!("resolved {new}"),
            };
            let _ = writeln!(text, "  {} {step}", commit.oid);
        }
    }
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(8).fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::Stack;

    fn branch(
        name: &str,
        parent: Option<&str>,
        state: StackState,
        commits: &[&str],
    ) -> StackBranch {
        StackBranch {
            name: name.into(),
            parent: parent.map(Into::into),
            depth: 0,
            state,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: commits.iter().map(|c| (*c).to_owned()).collect(),
            base: None,
            parent_tip: None,
        }
    }

    fn tips(names: &[&str]) -> HashMap<String, String> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), format!("{name}-tip")))
            .collect()
    }

    fn run(
        branches: Vec<StackBranch>,
        from: &str,
        placed: &HashMap<String, CheckedOut>,
        conflicts_on: &[&str],
    ) -> RestackPlan {
        run_with(branches, from, placed, &mut |onto, commit| {
            let kind = if conflicts_on.contains(&commit) {
                ReplayKind::Conflict {
                    paths: vec!["a.txt".into()],
                }
            } else if commit.starts_with("drop") {
                ReplayKind::Dropped
            } else if commit.starts_with("merge") {
                ReplayKind::NotLinear
            } else {
                ReplayKind::Clean {
                    new: format!("{commit}@{onto}"),
                }
            };
            Ok(Replayed {
                subject: format!("subject of {commit}"),
                kind,
            })
        })
    }

    fn run_with(
        branches: Vec<StackBranch>,
        from: &str,
        placed: &HashMap<String, CheckedOut>,
        replay: &mut dyn FnMut(&str, &str) -> Result<Replayed, String>,
    ) -> RestackPlan {
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        let tips = tips(&names);
        let snapshot = StackSnapshot {
            stacks: vec![Stack { branches }],
            looped: vec![],
        };
        let inputs = Inputs {
            snapshot: &snapshot,
            links: &[],
            tips: &tips,
            checked_out: placed,
            skip: &HashSet::new(),
        };
        plan(from, &inputs, replay).unwrap()
    }

    fn chain() -> Vec<StackBranch> {
        vec![
            branch("main", None, StackState::Root, &[]),
            branch("a", Some("main"), StackState::Stale, &["a2", "a1"]),
            branch("b", Some("a"), StackState::Current, &["b1"]),
        ]
    }

    #[test]
    fn a_stale_branch_is_replayed_oldest_first_and_its_children_follow() {
        let plan = run(chain(), "main", &HashMap::new(), &[]);
        let names: Vec<_> = plan.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let a = &plan.branches[0];
        assert_eq!(a.onto, "main-tip");
        let oids: Vec<_> = a.commits.iter().map(|c| c.oid.as_str()).collect();
        assert_eq!(oids, ["a1", "a2"]);
        assert_eq!(a.new_tip.as_deref(), Some("a2@a1@main-tip"));
        // The child lands on the parent's planned tip, not its current one.
        assert_eq!(plan.branches[1].onto, "a2@a1@main-tip");
        assert!(plan.can_apply());
    }

    #[test]
    fn a_current_branch_with_a_current_parent_is_left_out() {
        let mut branches = chain();
        branches[1].state = StackState::Current;
        let plan = run(branches, "main", &HashMap::new(), &[]);
        assert!(plan.branches.is_empty());
        assert!(!plan.can_apply());
    }

    #[test]
    fn a_conflict_blocks_the_branch_and_everything_under_it_but_not_siblings() {
        let mut branches = chain();
        branches.push(branch("c", Some("main"), StackState::Stale, &["c1"]));
        let plan = run(branches, "main", &HashMap::new(), &["a2"]);
        let a = &plan.branches[0];
        assert_eq!(
            a.block,
            Some(RestackBlock::Conflict {
                commit: "a2".into(),
                onto: "a1@main-tip".into(),
            })
        );
        assert!(a.new_tip.is_none());
        assert!(matches!(
            plan.branches[1].block,
            Some(RestackBlock::ParentBlocked { .. })
        ));
        let c = plan.branches.iter().find(|b| b.name == "c").unwrap();
        assert!(c.block.is_none() && c.new_tip.is_some());
        assert!(plan.can_apply());
    }

    #[test]
    fn a_dropped_commit_is_listed_and_not_replayed() {
        let mut branches = chain();
        branches[1].commits = vec!["a2".into(), "drop1".into()];
        let plan = run(branches, "main", &HashMap::new(), &[]);
        let a = &plan.branches[0];
        assert_eq!(a.commits[0].step, ReplayStep::Dropped);
        assert_eq!(a.replayed(), 1);
        assert_eq!(a.new_tip.as_deref(), Some("a2@main-tip"));
    }

    #[test]
    fn a_merge_commit_refuses_the_branch() {
        let mut branches = chain();
        branches[1].commits = vec!["merge1".into()];
        let plan = run(branches, "main", &HashMap::new(), &[]);
        assert!(matches!(
            plan.branches[0].block,
            Some(RestackBlock::Refused { .. })
        ));
        assert!(!plan.can_apply());
    }

    #[test]
    fn a_dirty_checked_out_branch_stops_the_plan_and_a_worktree_branch_is_refused() {
        let placed = HashMap::from([("a".to_owned(), CheckedOut::Here { dirty: true })]);
        let plan = run(chain(), "main", &placed, &[]);
        assert_eq!(plan.stopped.len(), 1);
        assert!(!plan.can_apply());

        let placed = HashMap::from([("a".to_owned(), CheckedOut::Elsewhere { path: "/w".into() })]);
        let plan = run(chain(), "main", &placed, &[]);
        assert!(matches!(
            plan.branches[0].block,
            Some(RestackBlock::Refused { .. })
        ));
    }

    #[test]
    fn unfit_states_are_refused_and_block_their_children() {
        for state in [
            StackState::DivergedBase,
            StackState::ParentGone,
            StackState::ParentMerged,
        ] {
            let mut branches = chain();
            branches[1].state = state;
            let plan = run(branches, "a", &HashMap::new(), &[]);
            assert!(matches!(
                plan.branches[0].block,
                Some(RestackBlock::Refused { .. })
            ));
            assert!(matches!(
                plan.branches[1].block,
                Some(RestackBlock::ParentBlocked { .. })
            ));
        }
    }

    #[test]
    fn planning_from_a_branch_leaves_its_parents_alone() {
        let plan = run(chain(), "b", &HashMap::new(), &[]);
        assert!(plan.branches.is_empty());
    }

    #[test]
    fn a_conflict_records_the_base_it_was_replayed_onto() {
        let plan = run(chain(), "main", &HashMap::new(), &["a2"]);
        assert_eq!(
            plan.branches[0].block,
            Some(RestackBlock::Conflict {
                commit: "a2".into(),
                onto: "a1@main-tip".into(),
            })
        );
    }

    #[test]
    fn a_resolved_commit_continues_the_walk_and_its_children_plan() {
        // a1 replays clean onto main; a2 is resolved onto a1's replay; b1 goes on from there.
        let mut replay = |onto: &str, commit: &str| {
            let kind = match (onto, commit) {
                ("main-tip", "a1") => ReplayKind::Clean { new: "a1n".into() },
                ("a1n", "a2") => ReplayKind::Resolved { new: "a2r".into() },
                _ => ReplayKind::Clean {
                    new: format!("{commit}@{onto}"),
                },
            };
            Ok(Replayed {
                subject: format!("subject of {commit}"),
                kind,
            })
        };
        let plan = run_with(chain(), "main", &HashMap::new(), &mut replay);
        let [a, b] = plan.branches.as_slice() else {
            panic!("{plan:?}")
        };
        assert_eq!(a.block, None);
        assert_eq!(
            a.commits.iter().map(|c| c.step.clone()).collect::<Vec<_>>(),
            [
                ReplayStep::Clean { new: "a1n".into() },
                ReplayStep::Resolved { new: "a2r".into() },
            ]
        );
        assert_eq!(a.replayed(), 2);
        assert_eq!(a.new_tip.as_deref(), Some("a2r"));
        assert_eq!(b.onto, "a2r");
        assert_eq!(b.new_tip.as_deref(), Some("b1@a2r"));
        assert!(plan.can_apply());
    }

    #[test]
    fn the_id_tells_a_conflict_from_a_resolution_and_one_resolution_from_another() {
        let resolved = |new: &'static str| {
            move |onto: &str, commit: &str| {
                let kind = if commit == "a2" {
                    ReplayKind::Resolved { new: new.into() }
                } else {
                    ReplayKind::Clean {
                        new: format!("{commit}@{onto}"),
                    }
                };
                Ok(Replayed {
                    subject: String::new(),
                    kind,
                })
            }
        };
        let conflict = run(chain(), "main", &HashMap::new(), &["a2"]);
        let one = run_with(chain(), "main", &HashMap::new(), &mut resolved("r1"));
        let same = run_with(chain(), "main", &HashMap::new(), &mut resolved("r1"));
        let other = run_with(chain(), "main", &HashMap::new(), &mut resolved("r2"));
        assert_ne!(conflict.plan_id, one.plan_id);
        assert_ne!(one.plan_id, other.plan_id);
        assert_eq!(one.plan_id, same.plan_id);
    }

    #[test]
    fn the_id_changes_with_the_inputs_and_not_without_them() {
        let one = run(chain(), "main", &HashMap::new(), &[]);
        let same = run(chain(), "main", &HashMap::new(), &[]);
        let other = run(chain(), "main", &HashMap::new(), &["a2"]);
        assert_eq!(one.plan_id, same.plan_id);
        assert_ne!(one.plan_id, other.plan_id);
    }
}
