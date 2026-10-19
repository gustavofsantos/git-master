//! The engine's side of manual workflows (docs/10): the session that holds the
//! driver and the store of one open repository, the Git actions a run takes
//! through the engine's `GitPort`, and the mapping of run events to the events
//! the front-ends get. The engine's methods that drive it live in `engine`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gm_schema::{
    Actor, CommandKind, Event, OperationId, RepoStatus, WorkflowEnd, WorkflowInfo, WorkflowInput,
    WorkflowRunDetail, WorkflowRunStatus, WorkflowRunSummary, WorkflowStepRecord,
};
use serde_json::{Value, json};

use crate::branch;
use crate::workflow::Severity;
use crate::workflow::driver::{Driver, ExecJob, GitActions, Outcome, RunEvent};
use crate::workflow::loader::{Entry, Ignored, Loaded, sha256_hex};
use crate::workflow::model::{Definition, End, StateKind};
use crate::workflow::parse;
use crate::workflow::store::{OutputRow, RunId, RunRow, RunStatus, Store, TransitionRow, prefix};
use crate::{GitError, GitPort, LogEntry};

/// The `gitmaster.workflowPath` key: extra directories of workflow files (D61).
const WORKFLOW_PATH_KEY: &str = "gitmaster.workflowPath";

/// A run's `repo.staged_diff` is kept to this many bytes in the run view.
const STAGED_DIFF_HEAD: usize = 4 * 1024;

/// The workflow session of one open repository: created on the first workflow
/// command, and replaced when another repository opens.
pub(crate) struct Session {
    pub(crate) root: PathBuf,
    pub(crate) driver: Driver,
    /// `exec` jobs started and not yet taken by the caller.
    pub(crate) jobs: Vec<ExecJob>,
    /// The operation that started each run. Its Git writes are logged under it.
    origins: BTreeMap<RunId, OperationId>,
    /// The operation that opened the session: the origin of a run it resumed.
    default_origin: OperationId,
}

impl Session {
    pub(crate) fn new(root: PathBuf, store: Store, opened_by: OperationId) -> Self {
        Self {
            driver: Driver::new(store, root.clone()),
            root,
            jobs: Vec::new(),
            origins: BTreeMap::new(),
            default_origin: opened_by,
        }
    }

    pub(crate) fn origin(&self, run: RunId) -> OperationId {
        self.origins
            .get(&run)
            .copied()
            .unwrap_or(self.default_origin)
    }

    pub(crate) fn started(&mut self, run: RunId, by: OperationId) {
        self.origins.insert(run, by);
    }

    /// Keeps the jobs of a driver call and turns its events into the ones the
    /// front-ends get.
    pub(crate) fn absorb(&mut self, outcome: Outcome) -> Vec<Event> {
        self.jobs.extend(outcome.jobs);
        outcome.events.into_iter().map(run_event).collect()
    }
}

/// The Git actions of a workflow step: each one is a Git write of the engine's
/// `GitPort`, and goes into the command log under the operation that caused it.
pub(crate) struct Actions<'a> {
    git: &'a dyn GitPort,
    root: &'a Path,
    log: &'a mut Vec<LogEntry>,
    origin: OperationId,
    wrote: bool,
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
        }
    }

    /// Whether a Git write was attempted, so the status is read again.
    pub(crate) fn wrote(&self) -> bool {
        self.wrote
    }

    fn record<T>(&mut self, kind: CommandKind, result: Result<T, GitError>) -> Result<T, String> {
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
        result.map_err(|e| e.0)
    }
}

impl GitActions for Actions<'_> {
    fn stage_all(&mut self) -> Result<(), String> {
        let result = self.git.stage_all(self.root);
        self.record(CommandKind::StageAll, result)
    }

    fn commit(&mut self, message: &str) -> Result<String, String> {
        let result = self.git.commit(self.root, message);
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
        let result = self.git.create_branch(self.root, name, None, checkout);
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
        let result = self.git.checkout(self.root, branch);
        self.record(
            CommandKind::Checkout {
                branch: branch.to_owned(),
            },
            result,
        )
    }
}

/// The directories workflows are read from: the user's directory first, then
/// each `gitmaster.workflowPath` value in config order.
pub(crate) fn workflow_dirs(
    git: &dyn GitPort,
    root: &Path,
    user_dir: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = user_dir.map(Path::to_path_buf).into_iter().collect();
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

/// The repository snapshot a run's expressions read as `repo.*`. A piece that
/// cannot be read is `null`.
pub(crate) fn repo_snapshot(git: &dyn GitPort, root: &Path) -> Value {
    let (branch, head) = match git.status(root) {
        Ok(RepoStatus { branch, .. }) => (branch.head, branch.oid),
        Err(_) => (None, None),
    };
    json!({
        "root": root.to_string_lossy(),
        "branch": branch,
        "head": head,
        "staged_diff": git.staged_diff(root).ok(),
    })
}

/// What `ListWorkflows` shows for one file.
pub(crate) fn workflow_info(entry: &Entry) -> WorkflowInfo {
    let (trigger, key, inputs, error, warnings) = match &entry.result {
        Ok((definition, diagnostics)) => (
            definition.trigger.on.clone(),
            definition.trigger.key.clone(),
            definition
                .inputs
                .iter()
                .map(|input| WorkflowInput {
                    name: input.name.clone(),
                    prompt: input.prompt.clone(),
                })
                .collect(),
            None,
            diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .map(|d| format!("line {}: {}", d.line, d.message))
                .collect(),
        ),
        Err(diagnostics) => (
            String::new(),
            None,
            Vec::new(),
            diagnostics
                .iter()
                .find(|d| d.severity == Severity::Error)
                .or(diagnostics.first())
                .map(|d| format!("line {}: {}", d.line, d.message)),
            Vec::new(),
        ),
    };
    WorkflowInfo {
        name: entry.name.clone(),
        path: entry.path.display().to_string(),
        trigger,
        key,
        inputs,
        hash: entry.hash.clone(),
        error,
        warnings,
        source: entry.source.clone(),
    }
}

/// One run as the history lists it.
pub(crate) fn run_summary(row: &RunRow) -> WorkflowRunSummary {
    WorkflowRunSummary {
        run: row.id,
        workflow: row.workflow.clone(),
        status: run_status(row.status),
        started_by: row.started_by.clone(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        end_reason: row.end_reason.clone(),
        current_state: row.current_state.clone(),
    }
}

/// Everything about one run, for the run view. `inputs`, `trigger` and `repo`
/// are the context the run stored when it started. A definition that no longer
/// parses leaves every step without its action; the history still shows.
pub(crate) fn run_detail(store: &Store, run: RunId) -> Result<WorkflowRunDetail, String> {
    let row = store
        .run(run)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("no run {run}"))?;
    let definition = parse::parse(&row.definition_text).ok();
    let outputs = store.outputs(run).map_err(|e| e.0)?;
    let transitions = store.transitions(run).map_err(|e| e.0)?;
    let field = |name: &str| row.trigger.get(name).cloned().unwrap_or(Value::Null);
    let mut repo = field("repo");
    cut_staged_diff(&mut repo);
    Ok(WorkflowRunDetail {
        summary: run_summary(&row),
        definition: row.definition_text.clone(),
        inputs: field("inputs"),
        trigger: field("trigger"),
        repo,
        steps: steps(&outputs, &transitions, definition.as_ref()),
    })
}

/// One record per output row, in the order the rows hold. The k-th visit of a
/// state is paired with the k-th transition out of it.
fn steps(
    outputs: &[OutputRow],
    transitions: &[TransitionRow],
    definition: Option<&Definition>,
) -> Vec<WorkflowStepRecord> {
    let mut out_of: BTreeMap<&str, Vec<&TransitionRow>> = BTreeMap::new();
    for transition in transitions {
        out_of
            .entry(transition.from_state.as_str())
            .or_default()
            .push(transition);
    }
    outputs
        .iter()
        .map(|row| {
            let taken = out_of.get(row.state.as_str()).and_then(|list| {
                list.get(usize::try_from(row.visit.saturating_sub(1)).ok()?)
                    .copied()
            });
            WorkflowStepRecord {
                state: row.state.clone(),
                visit: row.visit,
                action: definition.and_then(|definition| action_of(definition, &row.state)),
                input: row.input.clone(),
                output: row.output.clone(),
                truncated: row.truncated,
                outcome: taken.map(|t| t.outcome.clone()),
                next: taken.map(|t| t.to_state.clone()),
                started_at: row.started_at,
                ended_at: row.ended_at,
            }
        })
        .collect()
}

/// The action of the state at `path` (`each/plan` inside a `for_each`); `None`
/// for a wait, a `for_each`, or a state the definition does not have.
fn action_of(definition: &Definition, path: &str) -> Option<String> {
    let (top, inner) = match path.split_once('/') {
        Some((outer, inner)) => (outer, Some(inner)),
        None => (path, None),
    };
    let state = definition.state(top)?;
    let state = match (&state.kind, inner) {
        (StateKind::ForEach { states, .. }, Some(inner)) => {
            states.iter().find(|nested| nested.id == inner)?
        }
        (_, None) => state,
        _ => return None,
    };
    match &state.kind {
        StateKind::Action { action, .. } => Some(action.clone()),
        _ => None,
    }
}

/// Cuts a string `repo.staged_diff` to its head, saying how large it was.
fn cut_staged_diff(repo: &mut Value) {
    let Some(Value::String(diff)) = repo.get_mut("staged_diff") else {
        return;
    };
    if diff.len() <= STAGED_DIFF_HEAD {
        return;
    }
    let size = diff.len();
    let head = prefix(diff, STAGED_DIFF_HEAD);
    *diff = format!("{head}\n… ({size} bytes)");
}

/// The workflow file `path` names, when its folder is one of `dirs` and is not
/// inside the repository (D73). The result holds the folder canonicalized.
pub(crate) fn workflow_file(root: &Path, dirs: &[PathBuf], path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    // The loader reads `*.toml` by extension, so the same test applies here.
    let name = path
        .file_name()
        .filter(|_| path.extension().is_some_and(|ext| ext == "toml"))
        .ok_or_else(|| {
            format!(
                "{} is not a workflow file: its name must end in .toml",
                path.display()
            )
        })?;
    let parent = path.parent().unwrap_or(path);
    let folder = std::fs::canonicalize(parent)
        .map_err(|_| format!("the folder {} does not exist", parent.display()))?;
    if let Ok(repo) = std::fs::canonicalize(root)
        && folder.starts_with(&repo)
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

/// The listing's `ignored` lines: `<path>: <reason>`.
pub(crate) fn ignored_lines(ignored: &[Ignored]) -> Vec<String> {
    ignored
        .iter()
        .map(|item| format!("{}: {}", item.path.display(), item.reason))
        .collect()
}

/// Loads every workflow file of the listed directories.
pub(crate) fn load(
    git: &dyn GitPort,
    root: &Path,
    user_dir: Option<&Path>,
) -> Result<Loaded, String> {
    let dirs = workflow_dirs(git, root, user_dir)?;
    Ok(crate::workflow::loader::load_dirs(&dirs, Some(root)))
}

fn run_event(event: RunEvent) -> Event {
    match event {
        RunEvent::Changed {
            run,
            workflow,
            status,
            state,
        } => Event::WorkflowRunChanged {
            run,
            workflow,
            status: run_status(status),
            state,
        },
        RunEvent::SuggestMessage { run, message } => {
            Event::WorkflowMessageSuggested { run, message }
        }
        RunEvent::Notify { run, text } => Event::WorkflowNotice { run, text },
        RunEvent::AwaitingApproval {
            run,
            state,
            summary,
        } => Event::WorkflowApprovalRequested {
            run,
            state,
            summary,
        },
        RunEvent::Finished {
            run, end, reason, ..
        } => Event::WorkflowRunFinished {
            run,
            end: run_end(end),
            reason,
        },
    }
}

fn run_status(status: RunStatus) -> WorkflowRunStatus {
    match status {
        RunStatus::Running => WorkflowRunStatus::Running,
        RunStatus::Waiting => WorkflowRunStatus::Waiting,
        RunStatus::AwaitingApproval => WorkflowRunStatus::AwaitingApproval,
        RunStatus::Done => WorkflowRunStatus::Done,
        RunStatus::Failed => WorkflowRunStatus::Failed,
        RunStatus::Blocked => WorkflowRunStatus::Blocked,
        RunStatus::Cancelled => WorkflowRunStatus::Cancelled,
    }
}

fn run_end(end: End) -> WorkflowEnd {
    match end {
        End::Done => WorkflowEnd::Done,
        End::Failed => WorkflowEnd::Failed,
        End::Blocked => WorkflowEnd::Blocked,
    }
}
