//! The engine's side of branch stacks (D68, D138): reads the links and the graph
//! facts through the port and hands them to the pure rules of `stack`, and makes
//! the checked metadata writes. The commands are in `engine`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gm_schema::{HistoryScope, StackSnapshot};

use crate::GitPort;
use crate::branch;
use crate::port::HistoryRevs;
use crate::stack::{self, Forest, StackLink};

/// The local branch names, as the branch list gives them.
pub(crate) fn locals(git: &dyn GitPort, root: &Path) -> Result<Vec<String>, String> {
    Ok(git
        .branches(root)
        .map_err(|e| e.0)?
        .into_iter()
        .filter(|entry| !entry.is_remote)
        .map(|entry| entry.name)
        .collect())
}

/// Every stack of the repository with its derived states: one graph read per
/// link and one merge check per parent that has a parent of its own.
pub(crate) fn snapshot(git: &dyn GitPort, root: &Path) -> Result<StackSnapshot, String> {
    let links = git.stack_links(root).map_err(|e| e.0)?;
    if links.is_empty() {
        return Ok(StackSnapshot::default());
    }
    let locals = locals(git, root)?;
    let forest = Forest::new(&links, &locals);
    let mut facts = HashMap::new();
    for link in forest.edges() {
        let edge = git
            .stack_edge(root, &link.branch, &link.parent, link.base.as_deref())
            .map_err(|e| e.0)?;
        facts.insert(link.branch.clone(), edge);
    }
    let mut merged = HashSet::new();
    for check in forest.merge_checks() {
        if git
            .branch_landed(
                root,
                &check.parent,
                check.parent_base.as_deref(),
                &check.root,
            )
            .map_err(|e| e.0)?
        {
            merged.insert(check.parent);
        }
    }
    Ok(forest.assemble(&facts, &merged))
}

/// Records `parent` as the parent of `branch`, with their merge base (D138).
pub(crate) fn set_parent(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
    parent: &str,
) -> Result<(), String> {
    let links = git.stack_links(root).map_err(|e| e.0)?;
    let locals = locals(git, root)?;
    if let Some(problem) = stack::set_parent_problem(branch, parent, &links, &locals) {
        return Err(problem);
    }
    let base = git
        .branch_merge_base(root, branch, parent)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("{branch} and {parent} share no history."))?;
    git.write_stack_link(root, branch, Some((parent, &base)))
        .map_err(|e| e.0)
}

/// Removes the parent of `branch`. Refused when it has none.
pub(crate) fn clear_parent(git: &dyn GitPort, root: &Path, branch: &str) -> Result<(), String> {
    let links = git.stack_links(root).map_err(|e| e.0)?;
    if !links.iter().any(|link| link.branch == branch) {
        return Err(format!("{branch} has no stack parent."));
    }
    git.write_stack_link(root, branch, None).map_err(|e| e.0)
}

/// Gives `branch` the parent of its old parent, keeping its base, when the old
/// parent is gone or merged (D138). Refused in any other state.
pub(crate) fn reparent_to_grandparent(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
) -> Result<String, String> {
    let state = snapshot(git, root)?
        .branch(branch)
        .map(|entry| entry.state)
        .ok_or_else(|| format!("{branch} has no stack parent."))?;
    if !state.parent_left() {
        return Err(format!(
            "The parent of {branch} is neither gone nor merged, so it keeps its parent."
        ));
    }
    let target = grandparent_of(git, root, branch)?
        .ok_or_else(|| format!("There is no branch to reparent {branch} to."))?;
    let links = git.stack_links(root).map_err(|e| e.0)?;
    let base = link_of(&links, branch)
        .and_then(|link| link.base.clone())
        .ok_or_else(|| format!("{branch} has no recorded base."))?;
    git.write_stack_link(root, branch, Some((&target, &base)))
        .map_err(|e| e.0)?;
    Ok(target)
}

/// The parent `ReparentToGrandparent` gives `branch` (D138(5)), read now: the recorded parent of
/// its old parent, else the default branch. `None` when there is none. The `stack.parent_merged`
/// event carries it as `grandparent`, read when the event fires.
pub(crate) fn grandparent_of(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
) -> Result<Option<String>, String> {
    let links = git.stack_links(root).map_err(|e| e.0)?;
    let locals = locals(git, root)?;
    let fallback = default_branch(git, root, &locals)?;
    Ok(stack::grandparent(
        branch,
        &links,
        &locals,
        fallback.as_deref(),
    ))
}

/// Checks a new child of `parent` and returns the parent's tip, where the child
/// starts and which it records as its base.
pub(crate) fn child_start(git: &dyn GitPort, root: &Path, parent: &str) -> Result<String, String> {
    git.branch_tip(root, parent)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("The parent must be a local branch; {parent} is not one."))
}

fn link_of<'a>(links: &'a [StackLink], branch: &str) -> Option<&'a StackLink> {
    links.iter().find(|link| link.branch == branch)
}

/// What a history walk of `scope` starts from (D139). Current stack is every
/// branch of the stack of HEAD's branch, the default branch and HEAD; HEAD
/// alone when its branch is in no stack. A local branch is named by its full
/// ref, so a tag or a path of the same name cannot be read in its place.
pub(crate) fn history_revs(
    git: &dyn GitPort,
    root: &Path,
    scope: &HistoryScope,
    head_branch: Option<&str>,
) -> Result<HistoryRevs, String> {
    let local = |name: &str| format!("refs/heads/{name}");
    Ok(match scope {
        HistoryScope::CurrentBranch => HistoryRevs::Head,
        HistoryScope::AllRefs => HistoryRevs::All,
        HistoryScope::Ref { name } => {
            if let Some(problem) = branch::start_point_problem(name) {
                return Err(problem.to_owned());
            }
            let locals = locals(git, root)?;
            let name = if locals.iter().any(|known| known == name) {
                local(name)
            } else {
                name.clone()
            };
            HistoryRevs::Refs {
                refs: vec![name],
                with_head: false,
            }
        }
        HistoryScope::CurrentStack => {
            let Some(head) = head_branch else {
                return Ok(HistoryRevs::Head);
            };
            let snapshot = snapshot(git, root)?;
            let Some(stack) = snapshot
                .stacks
                .iter()
                .find(|stack| stack.branches.iter().any(|entry| entry.name == head))
            else {
                return Ok(HistoryRevs::Head);
            };
            let mut names: Vec<&str> = stack
                .branches
                .iter()
                .map(|entry| entry.name.as_str())
                .collect();
            let locals = locals(git, root)?;
            let default = default_branch(git, root, &locals)?;
            if let Some(default) = default.as_deref()
                && !names.contains(&default)
            {
                names.push(default);
            }
            // A root whose parent is gone names no branch of its own in the stack.
            names.retain(|name| locals.iter().any(|known| known == name));
            HistoryRevs::Refs {
                refs: names.into_iter().map(local).collect(),
                with_head: true,
            }
        }
    })
}

/// The default branch, by the rule of `ListMergedBranches` without the current
/// branch as the last resort.
fn default_branch(
    git: &dyn GitPort,
    root: &Path,
    locals: &[String],
) -> Result<Option<String>, String> {
    let origin = git.origin_head(root).map_err(|e| e.0)?;
    let refs: Vec<&str> = locals.iter().map(String::as_str).collect();
    Ok(branch::merge_base(
        origin.as_deref().and_then(branch::origin_head_branch),
        &refs,
        None,
    ))
}
