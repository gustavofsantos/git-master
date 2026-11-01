//! The engine's side of restack (D14, D142): gathers what the pure plan reads,
//! applies a plan as one ref transaction and records it so undo can put it back.
//! The commands are in `engine`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gm_schema::{ReplayStep, RestackMove, RestackPhase, RestackPlan, RestackRecord};

use crate::GitPort;
use crate::port::RefMove;
use crate::restack::{self, CheckedOut};
use crate::stack_session;

/// Plans a restack of `branch` and the branches under it. A read.
pub(crate) fn plan(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
    skip: &[String],
    same_folder: &dyn Fn(&str) -> bool,
) -> Result<RestackPlan, String> {
    let snapshot = stack_session::snapshot(git, root)?;
    let links = git.stack_links(root).map_err(|e| e.0)?;
    let mut tips = HashMap::new();
    for entry in snapshot.stacks.iter().flat_map(|stack| &stack.branches) {
        if let Some(tip) = git.branch_tip(root, &entry.name).map_err(|e| e.0)? {
            tips.insert(entry.name.clone(), tip);
        }
    }
    let status = git.status(root).map_err(|e| e.0)?;
    let mut checked_out = HashMap::new();
    for entry in git.worktrees(root).map_err(|e| e.0)? {
        let Some(name) = entry.branch else { continue };
        let placed = if same_folder(&entry.path) {
            CheckedOut::Here {
                dirty: !status.entries.is_empty(),
            }
        } else {
            CheckedOut::Elsewhere { path: entry.path }
        };
        checked_out.insert(name, placed);
    }
    let inputs = restack::Inputs {
        snapshot: &snapshot,
        links: &links,
        tips: &tips,
        checked_out: &checked_out,
        skip: &skip.iter().cloned().collect::<HashSet<_>>(),
    };
    restack::plan(branch, &inputs, &mut |onto, commit| {
        git.restack_replay(root, onto, commit).map_err(|e| e.0)
    })
}

fn local(name: &str) -> String {
    format!("refs/heads/{name}")
}

fn store_path(git: &dyn GitPort, root: &Path) -> Result<PathBuf, String> {
    Ok(git
        .common_dir(root)
        .map_err(|e| e.0)?
        .join("gitmaster")
        .join("restack.json"))
}

fn save(path: &Path, record: &RestackRecord) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot save the restack record: {e}"))?;
    }
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot save the restack record: {e}"))
}

/// The record of the last restack that was not undone, if any.
pub(crate) fn last(git: &dyn GitPort, root: &Path) -> Result<Option<RestackRecord>, String> {
    let path = store_path(git, root)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("the restack record is unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read the restack record: {e}")),
    }
}

/// Moves the branches of the plan `plan_id` makes for `branch` (D142): the plan
/// is made again and must still have that id, the refs move in one transaction
/// that fails if any is not where the plan saw it, the checked-out branch gets
/// its files, and the bases are written last.
pub(crate) fn apply(
    git: &dyn GitPort,
    root: &Path,
    branch: &str,
    plan_id: &str,
    skip: &[String],
    same_folder: &dyn Fn(&str) -> bool,
) -> Result<(RestackRecord, Vec<(String, String)>), String> {
    let plan = plan(git, root, branch, skip, same_folder)?;
    if plan.plan_id != plan_id {
        return Err("The stacks changed since the plan was made. Plan again.".to_owned());
    }
    if let Some(reason) = plan.stopped.first() {
        return Err(reason.clone());
    }
    let moves: Vec<RestackMove> = plan
        .branches
        .iter()
        .filter(|entry| entry.block.is_none())
        .filter_map(|entry| {
            let new_tip = entry.new_tip.clone()?;
            Some(RestackMove {
                name: entry.name.clone(),
                parent: entry.parent.clone(),
                old_tip: entry.old_tip.clone(),
                new_tip,
                old_base: entry.old_base.clone(),
                new_base: entry.onto.clone(),
            })
        })
        .collect();
    if moves.is_empty() {
        return Err("Nothing in the plan can run.".to_owned());
    }
    // The replays of the branches that move: each old commit now has its new one (D145). A
    // branch that is blocked moves nothing, so its replays are not recorded.
    let remaps: Vec<(String, String)> = plan
        .branches
        .iter()
        .filter(|entry| entry.block.is_none() && entry.new_tip.is_some())
        .flat_map(|entry| entry.commits.iter())
        .filter_map(|commit| match &commit.step {
            ReplayStep::Clean { new } => Some((commit.oid.clone(), new.clone())),
            _ => None,
        })
        .collect();
    let record = RestackRecord {
        plan_id: plan.plan_id,
        from: branch.to_owned(),
        moves,
        phase: RestackPhase::Applying,
    };
    let path = store_path(git, root)?;
    save(&path, &record)?;
    transact(git, root, &record, false)?;
    Ok((finish(git, root, record, &path)?, remaps))
}

/// Writes the bases and marks the record applied.
fn finish(
    git: &dyn GitPort,
    root: &Path,
    mut record: RestackRecord,
    path: &Path,
) -> Result<RestackRecord, String> {
    for item in &record.moves {
        git.write_stack_link(root, &item.name, Some((&item.parent, &item.new_base)))
            .map_err(|e| e.0)?;
    }
    record.phase = RestackPhase::Applied;
    save(path, &record)?;
    Ok(record)
}

/// Finishes a restack that was interrupted (D142). Refs move in one transaction,
/// so they are all at the old tips (nothing happened: the record is dropped) or
/// all at the new ones (the files of the checked-out branch and the bases may be
/// owed, and are written). Any mix is reported and left alone. Returns the record
/// undo would use.
pub(crate) fn resume(git: &dyn GitPort, root: &Path) -> Result<Option<RestackRecord>, String> {
    let Some(record) = last(git, root)? else {
        return Ok(None);
    };
    if record.phase == RestackPhase::Applied {
        return Ok(Some(record));
    }
    let path = store_path(git, root)?;
    let mut moved = 0;
    let mut kept = 0;
    for item in &record.moves {
        let tip = git.branch_tip(root, &item.name).map_err(|e| e.0)?;
        if tip.as_deref() == Some(&item.new_tip) {
            moved += 1;
        } else if tip.as_deref() == Some(&item.old_tip) {
            kept += 1;
        }
    }
    if moved == 0 && kept == record.moves.len() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot remove the restack record: {e}"))?;
        return Ok(None);
    }
    if moved != record.moves.len() {
        return Err(
            "An interrupted restack left its branches in a mix of old and new tips. Nothing was changed; check them with the reflog."
                .to_owned(),
        );
    }
    // The checked-out branch had a clean working copy; changes now mean the files
    // never followed the ref.
    let status = git.status(root).map_err(|e| e.0)?;
    if let Some(item) = record
        .moves
        .iter()
        .find(|item| status.branch.head.as_deref() == Some(item.name.as_str()))
        && !status.entries.is_empty()
    {
        git.refresh_checkout(root, &item.old_tip, &item.new_tip)
            .map_err(|e| e.0)?;
    }
    finish(git, root, record, &path).map(Some)
}

/// Puts back the branches and bases of the last restack (D142). Refused when a
/// branch moved since, or when the working copy has changes and its branch is
/// one of them.
pub(crate) fn undo(git: &dyn GitPort, root: &Path) -> Result<(), String> {
    let record = resume(git, root)?.ok_or_else(|| "There is no restack to undo.".to_owned())?;
    for item in &record.moves {
        if git
            .branch_tip(root, &item.name)
            .map_err(|e| e.0)?
            .as_deref()
            != Some(&item.new_tip)
        {
            return Err(format!(
                "{} moved since the restack, so it cannot be undone.",
                item.name
            ));
        }
    }
    transact(git, root, &record, true)?;
    for item in &record.moves {
        if let Some(base) = &item.old_base {
            git.write_stack_link(root, &item.name, Some((&item.parent, base)))
                .map_err(|e| e.0)?;
        }
    }
    std::fs::remove_file(store_path(git, root)?)
        .map_err(|e| format!("cannot remove the restack record: {e}"))
}

/// The ref transaction of a record, forwards or back, and the files of the
/// checked-out branch. When the files cannot follow, the refs are put back.
fn transact(
    git: &dyn GitPort,
    root: &Path,
    record: &RestackRecord,
    back: bool,
) -> Result<(), String> {
    let ends = |item: &RestackMove| -> (String, String) {
        if back {
            (item.new_tip.clone(), item.old_tip.clone())
        } else {
            (item.old_tip.clone(), item.new_tip.clone())
        }
    };
    let status = git.status(root).map_err(|e| e.0)?;
    let here = record
        .moves
        .iter()
        .find(|item| status.branch.head.as_deref() == Some(item.name.as_str()));
    if here.is_some() && !status.entries.is_empty() {
        return Err(
            "The working copy has uncommitted changes and its branch is part of the restack. Commit or stash them first."
                .to_owned(),
        );
    }
    let refs: Vec<RefMove> = record
        .moves
        .iter()
        .map(|item| {
            let (old, new) = ends(item);
            RefMove {
                name: local(&item.name),
                new,
                old,
            }
        })
        .collect();
    git.update_refs(root, &refs).map_err(|e| e.0)?;
    if let Some(item) = here {
        let (old, new) = ends(item);
        if let Err(e) = git.refresh_checkout(root, &old, &new) {
            let reverse: Vec<RefMove> = refs
                .iter()
                .map(|item| RefMove {
                    name: item.name.clone(),
                    new: item.old.clone(),
                    old: item.new.clone(),
                })
                .collect();
            git.update_refs(root, &reverse).map_err(|e| e.0)?;
            return Err(format!("The working copy could not follow: {}", e.0));
        }
    }
    Ok(())
}
