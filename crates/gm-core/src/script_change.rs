//! `ctx.change({ branch })` (D219, D214 (1)): the change of a branch against its base, read the
//! way `ctx.scorecards` reads: a read that writes nothing and logs nothing. The base is the stack
//! parent, else the upstream, else the default branch of the remote (`change::base_of`). The
//! description is the one Git keeps for the branch. A changed path is data: a script names it and
//! never runs it.

use std::path::Path;

use serde_json::{Value, json};

use crate::GitPort;
use crate::change;
use crate::script::run::EffectResult;
use crate::script_scorecards::call_branch;

/// `ctx.change(args)`: `base`, `files` and `description` of the branch `args.branch` names, else
/// the run's branch, else HEAD's. A branch with no base has `base` `nil` and no files. Above 10,000
/// paths `change::change_of` gives no list, so `files` is empty and `truncated` is true.
pub(crate) fn read(
    git: &dyn GitPort,
    root: &Path,
    args: &Value,
    run_branch: Option<String>,
) -> EffectResult {
    match change_for_call(git, root, args, run_branch) {
        Ok(value) => EffectResult::done(value),
        Err(message) => EffectResult::failed(message),
    }
}

fn change_for_call(
    git: &dyn GitPort,
    root: &Path,
    args: &Value,
    run_branch: Option<String>,
) -> Result<Value, String> {
    let branch = call_branch(git, root, args, run_branch)?;
    let description = git.branch_description(root, &branch).map_err(|e| e.0)?;
    let (base, files, truncated) = match change::change_of(git, root, &branch).map_err(|e| e.0)? {
        None => (None, Vec::new(), false),
        Some(change) => match change.paths {
            Some(paths) => (Some(change.base_ref), paths, false),
            None => (Some(change.base_ref), Vec::new(), true),
        },
    };
    Ok(json!({
        "base": base,
        "files": files,
        "description": description,
        "truncated": truncated,
    }))
}
