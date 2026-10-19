//! The engine's side of manual workflows (docs/10): the session that holds the
//! driver and the store of one open repository, the Git actions a run takes
//! through the engine's `GitPort`, and the mapping of run events to the events
//! the front-ends get. The engine's methods that drive it live in `engine`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gm_schema::{
    Actor, CommandKind, Event, OperationId, RepoStatus, WorkflowEnd, WorkflowInfo, WorkflowInput,
    WorkflowRunStatus,
};
use serde_json::{Value, json};

use crate::branch;
use crate::workflow::Severity;
use crate::workflow::driver::{Driver, ExecJob, GitActions, Outcome, RunEvent};
use crate::workflow::loader::{Entry, Ignored, Loaded};
use crate::workflow::model::End;
use crate::workflow::store::{RunId, RunStatus, Store};
use crate::{GitError, GitPort, LogEntry};

/// The `gitmaster.workflowPath` key: extra directories of workflow files (D61).
const WORKFLOW_PATH_KEY: &str = "gitmaster.workflowPath";

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
