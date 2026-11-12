//! The engine's side of workflow scripts (design/workflow-scripts.md): the Git actions a run takes
//! through the engine's `GitPort`, the directories scripts are read from, and the files of the
//! Workflows page (open in the editor, delete to the trash). The engine's methods that drive them
//! live in `engine`.

use std::path::{Path, PathBuf};

use gm_schema::{Actor, CommandKind, OperationId, RestackPlan};

use crate::branch;
use crate::script::loader::sha256_hex;
use crate::{GitPort, LogEntry};

/// The `gitmaster.workflowPath` key: extra directories of workflow files (D61).
const WORKFLOW_PATH_KEY: &str = "gitmaster.workflowPath";

/// The Git side effects of the script's `ctx` calls: the engine implements it over its
/// `GitPort`, and each call is one side effect that goes to the command log.
pub(crate) trait GitActions {
    /// `stage.all`: stages every change of the working tree.
    fn stage_all(&mut self) -> Result<(), String>;
    /// `commit.create`: commits the index; returns the new commit id.
    fn commit(&mut self, message: &str) -> Result<String, String>;
    /// `branch.create`.
    fn create_branch(&mut self, name: &str, checkout: bool) -> Result<(), String>;
    /// `branch.checkout`.
    fn checkout(&mut self, branch: &str) -> Result<(), String>;
}

/// The Git actions of a script run: each one is a Git write of the engine's
/// `GitPort`, and goes into the command log under the operation that caused it.
pub(crate) struct Actions<'a> {
    git: &'a dyn GitPort,
    root: &'a Path,
    log: &'a mut Vec<LogEntry>,
    origin: OperationId,
    wrote: bool,
    refs: bool,
    restack: bool,
    remaps: Vec<(String, String)>,
    conflict: Option<RestackPlan>,
}

/// What a call of a run changed, for the engine: whether Git was written, whether branch refs
/// moved (so the branches and the stacks are read again), whether a restack moved them (so the
/// window can undo it, `WF4c`), the replays of moved commits (D145), and the restack a call
/// stopped on (`WF4c`).
#[derive(Default)]
pub(crate) struct Effected {
    pub(crate) wrote: bool,
    pub(crate) refs: bool,
    pub(crate) restack: bool,
    pub(crate) remaps: Vec<(String, String)>,
    pub(crate) conflict: Option<RestackPlan>,
}

impl<'a> Actions<'a> {
    pub(crate) fn new(
        git: &'a dyn GitPort,
        root: &'a Path,
        log: &'a mut Vec<LogEntry>,
        origin: OperationId,
    ) -> Self {
        Self {
            git,
            root,
            log,
            origin,
            wrote: false,
            refs: false,
            restack: false,
            remaps: Vec::new(),
            conflict: None,
        }
    }

    /// The end of the call: what it changed.
    pub(crate) fn finish(self) -> Effected {
        Effected {
            wrote: self.wrote,
            refs: self.refs,
            restack: self.restack,
            remaps: self.remaps,
            conflict: self.conflict,
        }
    }

    pub(crate) fn git(&self) -> &'a dyn GitPort {
        self.git
    }

    pub(crate) fn root(&self) -> &'a Path {
        self.root
    }

    /// Notes that a restack or a reparent (or its undo) moved the branch refs: the window gets the
    /// record it can undo (`WF4c`).
    pub(crate) fn moved_by_restack(&mut self) {
        self.refs = true;
        self.restack = true;
    }

    /// Notes the restack this call stopped on: the plan after it, with the blocked branch (`WF4c`).
    pub(crate) fn stopped_on_conflict(&mut self, plan: RestackPlan) {
        self.conflict = Some(plan);
    }

    /// Keeps the replays of moved commits, so the thread links resolve to them (D145).
    pub(crate) fn remap(&mut self, pairs: Vec<(String, String)>) {
        self.remaps.extend(pairs);
    }

    /// Logs one Git write under the run's operation, and passes its result on as text on failure.
    pub(crate) fn record<T>(
        &mut self,
        kind: CommandKind,
        result: Result<T, String>,
    ) -> Result<T, String> {
        let state = if result.is_ok() {
            gm_schema::OperationState::Done
        } else {
            gm_schema::OperationState::Failed
        };
        self.log.push(LogEntry {
            id: self.origin,
            actor: Actor::Human,
            kind,
            state,
        });
        self.wrote = true;
        result
    }
}

impl GitActions for Actions<'_> {
    fn stage_all(&mut self) -> Result<(), String> {
        let result = self.git.stage_all(self.root).map_err(|e| e.0);
        self.record(CommandKind::StageAll, result)
    }

    fn commit(&mut self, message: &str) -> Result<String, String> {
        let result = self.git.commit(self.root, message).map_err(|e| e.0);
        self.record(
            CommandKind::Commit {
                message: message.to_owned(),
            },
            result,
        )
    }

    fn create_branch(&mut self, name: &str, checkout: bool) -> Result<(), String> {
        if let Some(problem) = branch::name_problem(name) {
            return Err(problem.to_owned());
        }
        let result = self
            .git
            .create_branch(self.root, name, None, checkout)
            .map_err(|e| e.0);
        self.record(
            CommandKind::CreateBranch {
                name: name.to_owned(),
                start_point: None,
                checkout,
            },
            result,
        )
    }

    fn checkout(&mut self, branch: &str) -> Result<(), String> {
        let result = self.git.checkout(self.root, branch).map_err(|e| e.0);
        self.record(
            CommandKind::Checkout {
                branch: branch.to_owned(),
            },
            result,
        )
    }
}

/// The repository's workflows folder, `.git/gitmaster/workflows` in the common Git directory, so
/// every worktree of the repository shares it (D219 (2)). `None` when Git does not say where its
/// directory is. The folder may not exist: a missing folder is no workflows.
pub(crate) fn repository_dir(git: &dyn GitPort, root: &Path) -> Option<PathBuf> {
    git.common_dir(root)
        .ok()
        .map(|dir| dir.join("gitmaster").join("workflows"))
}

/// The directories workflows are read from: the repository's folder first (when it wins a name
/// against the user's, D219 (2)), then the user's directory, then each `gitmaster.workflowPath`
/// value in config order.
pub(crate) fn workflow_dirs(
    git: &dyn GitPort,
    root: &Path,
    user_dir: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = repository_dir(git, root).into_iter().collect();
    dirs.extend(user_dir.map(Path::to_path_buf));
    for value in git
        .config_values(root, WORKFLOW_PATH_KEY)
        .map_err(|e| e.0)?
    {
        dirs.push(expand_home(&value));
    }
    Ok(dirs)
}

/// A leading `~/` is the home directory; any other path is taken as written.
fn expand_home(value: &str) -> PathBuf {
    match (value.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(value),
    }
}

/// The workflow file `path` names, when its folder is one of `dirs` and is not
/// inside the repository (D73), except the repository's own folder `repository` (D219 (2)). The
/// result holds the folder canonicalized.
pub(crate) fn workflow_file(
    root: &Path,
    dirs: &[PathBuf],
    repository: Option<&Path>,
    path: &str,
) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    // The loader reads `*.lua` by extension, so the same test applies here.
    let name = path
        .file_name()
        .filter(|_| path.extension().is_some_and(|ext| ext == "lua"))
        .ok_or_else(|| {
            format!(
                "{} is not a workflow file: its name must end in .lua",
                path.display()
            )
        })?;
    let parent = path.parent().unwrap_or(path);
    let folder = std::fs::canonicalize(parent)
        .map_err(|_| format!("the folder {} does not exist", parent.display()))?;
    let in_repository_folder = repository
        .and_then(|dir| std::fs::canonicalize(dir).ok())
        .is_some_and(|dir| dir == folder);
    if let Ok(repo) = std::fs::canonicalize(root)
        && folder.starts_with(&repo)
        && !in_repository_folder
    {
        return Err(format!(
            "{} is inside the repository; workflows are only read from your own config (D73)",
            folder.display()
        ));
    }
    let listed = dirs
        .iter()
        .any(|dir| std::fs::canonicalize(dir).is_ok_and(|real| real == folder));
    if !listed {
        return Err(format!(
            "{} is not a workflows directory; save only into your workflows directory or a gitmaster.workflowPath directory",
            folder.display()
        ));
    }
    Ok(folder.join(name))
}

/// The sha256 of the file as it is on disk; `None` when it cannot be read.
pub(crate) fn file_hash(file: &Path) -> Option<String> {
    std::fs::read(file).ok().map(|bytes| sha256_hex(&bytes))
}

/// Writes `source` to `file` through a hidden temporary file in the same
/// folder, then renames it over `file`: a reader sees the old file or the new.
pub(crate) fn write_atomically(file: &Path, source: &str) -> Result<(), String> {
    let name = file
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let temp = file.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, source)
        .map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, file).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("could not replace {}: {e}", file.display())
    })
}

/// Moves `file` into `dir` as `<stamp>-<file name>`, creating `dir`. A rename
/// that fails (another device, usually) is a copy and a removal. Returns the
/// new path.
pub(crate) fn trash_file(file: &Path, dir: &Path, stamp: i64) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let name = file
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let mut stamp = stamp;
    let target = loop {
        let candidate = dir.join(format!("{stamp}-{name}"));
        if std::fs::symlink_metadata(&candidate).is_err() {
            break candidate;
        }
        stamp += 1;
    };
    if std::fs::rename(file, &target).is_err() {
        std::fs::copy(file, &target)
            .map_err(|e| format!("could not move {} to the trash: {e}", file.display()))?;
        std::fs::remove_file(file).map_err(|e| {
            format!(
                "copied {} to {} but could not remove it: {e}",
                file.display(),
                target.display()
            )
        })?;
    }
    Ok(target)
}
