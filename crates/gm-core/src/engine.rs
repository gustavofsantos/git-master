use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use gm_schema::threads::ProvenanceResult;
use gm_schema::{
    Actor, Approval, Blocker, BranchFailure, Command, CommandKind, CommitFile, ConflictFileRead,
    ConflictSite, DirListing, Event, FileBody, FileView, FindingInput, GateOp, GateOutcome,
    GatesStatus, GrepOptions, History, MERGED_LIST_CHANGED, MemberRequest, MergedBranch,
    OperationId, OperationState, REFLOG_PAGE, RestackBlock, RestackPlan, RestackResolution,
    ReviewAuthorFilter, ReviewChange, ReviewComment, ReviewStatus, Reviewer, ReviewerFileError,
    ReviewerSignal, ReviewerSource, ReviewerSubject, STASH_LIST_CHANGED, SUBMODULE_COMMIT_LIMIT,
    StackSnapshot, WorkflowDiagnostic, WorktreeEntry, WorktreeInspection,
};

use crate::GitPort;
use crate::blame::BlameLine;
use crate::branch;
use crate::change::{self, Change};
use crate::combination;
use crate::commit_detail::{DetailJob, DetailReport};
use crate::commit_diff::{StackJob, StackReport};
use crate::conflict::{self, Hunk, Segment};
use crate::contexts_session;
use crate::files::{
    Building, FilesSession, HighlightJob, HighlightReport, IndexJob, IndexReport, ReadJob,
    ReadReport, SearchJob, SearchReport, TextJob, TextReport,
};
use crate::gates::{self, FindingFact, GateFacts};
use crate::grep::GrepRequest;
use crate::history::{Canceller, HistoryJob, HistoryOutcome, HistoryReport, HistoryStream};
use crate::move_exec;
use crate::move_preview;
use crate::port::{HistoryRevs, ReviewSide};
use crate::reflog;
use crate::remote::{RemoteJob, RemoteReport};
use crate::restack_resolutions::{self, Resolution};
use crate::restack_session;
use crate::review;
use crate::review_session::{self, ReviewSession};
use crate::review_store::{AgentThread, NewBypass, NewFinding, NewVerdict, Reply, ReviewStore};
use crate::reviewers;
use crate::script::builtins;
use crate::script::dry;
use crate::script::hook_convert;
use crate::script::inspect::inspect;
use crate::script::journal::{Journal, RETAINED_RUNS, RunId as ScriptRunId, RunStatus};
use crate::script::loader;
use crate::script::luau::LuauEngine;
use crate::script::meta::Concurrency;
use crate::script::phases::{self, Facts, Gate, Timing};
use crate::script::policy::StartedBy;
use crate::script::run::{EffectResult, ScriptEvent};
use crate::script::run::{RunSpec, Runtime};
use crate::script::switches::{self, Switches};
use crate::script::transforms;
use crate::script::transforms::Fingerprint;
use crate::script::triggers::{self, Decision, Lanes, TriggerEvent};
use crate::script::trust::{self, TrustStore};
use crate::script::{api_docs, input as script_input, templates as script_templates};
use crate::script_session::{
    self, Carried, EffectRequest, ScriptJob, ScriptLink, ScriptReport, ScriptSession, Waiting,
};
use crate::stack_session;
use crate::submodule;
use crate::syntax;
use crate::threads::members;
use crate::threads::redact::Redactor;
use crate::threads_session::{self, ProvenanceJob, ProvenanceReport, ThreadsSession, origin_of};
use crate::verdicts;
use crate::workflow_session::{self, Effected};
use crate::workspace::{CreateRequest, Workspaces};
use crate::worktree;

/// One state change of one operation. The log is audit, and the base for undo
/// later; it is not replayed to rebuild state (design/architecture.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub id: OperationId,
    pub actor: Actor,
    pub kind: CommandKind,
    pub state: OperationState,
}

/// Synchronous and deterministic: the same commands over the same repository
/// give the same events and the same log. Threads live in `runner`.
/// A Git command a script run waits for, while it runs (`WF4a`): the call to answer, and how to
/// stop the command.
struct ScriptWaiting {
    origin: OperationId,
    root: PathBuf,
    kind: CommandKind,
    reply: std::sync::mpsc::Sender<EffectResult>,
    stop: Box<dyn Fn() + Send + Sync>,
}

pub struct Engine {
    git: Box<dyn GitPort>,
    root: Option<PathBuf>,
    log: Vec<LogEntry>,
    /// The walk the history list reads, while a repository is open.
    history: Option<HistorySession>,
    /// History pages accepted since the last call; see `take_history_jobs`.
    history_jobs: Vec<HistoryJob>,
    /// The commit detail read in flight, if any. A new request supersedes it.
    detail: Option<ActiveDetail>,
    /// Detail reads accepted since the last call; see `take_detail_jobs`.
    detail_jobs: Vec<DetailJob>,
    /// The commit file diff read in flight, if any. A new request supersedes it.
    stack: Option<ActiveDetail>,
    /// Commit stack reads accepted since the last call; see `take_stack_jobs`.
    stack_jobs: Vec<StackJob>,
    /// The file list of the open repository (N1, N2).
    files: Option<FilesSession>,
    /// Reads of the file list accepted since the last call; see `take_index_jobs`.
    index_jobs: Vec<IndexJob>,
    /// The file search in flight, if any. A new request supersedes it.
    files_search: Option<ActiveDetail>,
    /// The query of `files_search` while it waits for the first file list.
    files_waiting: Option<String>,
    /// File searches accepted since the last call; see `take_search_jobs`.
    search_jobs: Vec<SearchJob>,
    /// The file read in flight, if any. A new request supersedes it.
    files_read: Option<ActiveDetail>,
    /// File reads accepted since the last call; see `take_read_jobs`.
    read_jobs: Vec<ReadJob>,
    /// The coloring in flight, by the id of the read it follows (N4). A newer read stops it.
    highlight: Option<(OperationId, Arc<AtomicBool>)>,
    /// Colorings accepted since the last call; see `take_highlight_jobs`.
    highlight_jobs: Vec<HighlightJob>,
    /// The text search in flight, if any. A new request supersedes it.
    text_search: Option<ActiveDetail>,
    /// Text searches accepted since the last call; see `take_text_jobs`.
    text_jobs: Vec<TextJob>,
    /// The blame of the provenance read in flight (D144), if any.
    provenance: Option<ActiveDetail>,
    /// Provenance reads accepted since the last call; see `take_provenance_jobs`.
    provenance_jobs: Vec<ProvenanceJob>,
    /// The remote operation in flight, if any. Only one runs at a time.
    remote: Option<ActiveRemote>,
    /// Remote operations accepted since the last call; see `take_remote_jobs`.
    remote_jobs: Vec<RemoteJob>,
    /// Git commands that script runs wait for, by run (`WF4a`): the call to answer, and the way to
    /// stop its command.
    script_waiting: BTreeMap<ScriptRunId, ScriptWaiting>,
    /// The Git commands script runs started, for the runner to run (`WF4a`).
    script_jobs: Vec<ScriptJob>,
    /// The user's workflow directory, read before `gitmaster.workflowPath`.
    workflow_user_dir: Option<PathBuf>,
    /// The user's reviewer directory (D212 (1)): `reviewers/` of the configuration directory.
    reviewer_user_dir: Option<PathBuf>,
    /// The user's gate file directory (D212 (2)): the configuration directory itself, where
    /// `gates.json` sits.
    gates_user_dir: Option<PathBuf>,
    /// The folder a deleted workflow file is moved to; see `delete_workflow_file`.
    workflow_trash_dir: Option<PathBuf>,
    /// The contexts file (`contexts.db`); see `set_contexts_db`.
    contexts_db: Option<PathBuf>,
    /// The home folder a workspace root may start with `~` (D174 (4)), from `HOME` at start.
    workspace_home: Option<PathBuf>,
    /// The global Git configuration the workspace commands' Git calls read; `None` reads the
    /// process's own. See `set_workspace_global_config`.
    workspace_config: Option<PathBuf>,
    /// How script runs reach the engine thread; see `script_session`. Set by the runner.
    script_link: Option<ScriptLink>,
    /// The script runs of the open repository; see `script_session`.
    script: Option<ScriptSession>,
    /// Which versions of a script the person reviewed (D101). Opened on first use.
    trust: Option<TrustStore>,
    /// The `.api/` reference was written for this session already.
    api_written: bool,
    /// The review comments of the open repository; see `review_session`.
    review: Option<ReviewSession>,
    /// The threads of the open repository; see `threads_session`.
    threads: Option<ThreadsSession>,
    /// The restack conflict being resolved in its worktree (D154), if any.
    restack_resolution: Option<ResolutionSession>,
    /// Whether engine events start workflows (`WF3a`, D155(4)). Only the app turns it on.
    triggers: bool,
    /// The event runs in progress, one lane per workflow (D100), in memory.
    lanes: Lanes<ScriptRunId, (OperationId, TriggerEvent)>,
    /// The local branch tips of the open repository at the last check, for `refs.changed`.
    tips: Option<(PathBuf, BTreeMap<String, String>)>,
    /// The stacks of the open repository at the last check, for `stack.*` (`WF3f`, D162(2)).
    stack_seen: Option<(PathBuf, StackSnapshot)>,
    /// The operation waiting for its `before.<op>` gates (`WF3c`, D157), if any. One at a time.
    gate: Option<GatedOp>,
    /// The `after.<op>` observer runs in progress, by run: the workflow's name and the operation.
    observers: BTreeMap<ScriptRunId, (String, &'static str)>,
    /// The transform operation whose `transform.<field>` runs are in progress (`WF3d`), if any.
    /// One at a time: a new one supersedes it.
    transform: Option<TransformRun>,
    /// The `transform.conflict_hunk` runs of the file last read (`WF3d`, D168), if any. One at a
    /// time: a new read of a file supersedes it.
    conflict_hunks: Option<HunkRun>,
    /// The event runs that wait for the review of their version, one per workflow: the latest
    /// trigger wins (`WF3b`, D155(3)). Dropped when the file changes, the person dismisses it or
    /// the repository is left.
    wanted: BTreeMap<String, Wanted>,
}

/// An event run that waits for the review of its version (`WF3b`, D155(3)): what it starts once
/// the person reviews `hash`.
struct Wanted {
    hash: String,
    origin: OperationId,
    root: PathBuf,
    concurrency: Concurrency,
    trigger: TriggerEvent,
}

/// The phases of one operation, as the engine read them when the operation started (`WF3c`).
struct Phases {
    /// The operation's name as a phase names it, such as `commit`.
    op: &'static str,
    root: PathBuf,
    /// What the phases read before the operation ran.
    facts: Facts,
    /// The `before.<op>` event each gate reads as `input.trigger`.
    before: TriggerEvent,
    /// The `before.<op>` scripts, in file-name order: name and hash.
    gates: Vec<(String, String)>,
    /// The `after.<op>` scripts: name and hash.
    observers: Vec<(String, String)>,
}

/// A gated operation waiting for its gates (`WF3c`, D157). Its command runs once every gate passed.
struct GatedOp {
    id: OperationId,
    command: Command,
    phases: Phases,
    /// The gates not passed yet, in order. The first one is the gate `run` belongs to.
    waiting: VecDeque<(String, String)>,
    /// The run of the gate in progress.
    run: Option<ScriptRunId>,
}

/// A transform operation whose `transform.<field>` runs are in progress (`WF3d`, D155). The runs
/// go one after another in file-name order; each reads the value so far.
struct TransformRun {
    id: OperationId,
    command: Command,
    root: PathBuf,
    field: String,
    /// The branch HEAD was on when the operation started: what `match` reads.
    branch: Option<String>,
    /// The transforms not started yet: name and hash, in file-name order.
    waiting: VecDeque<(String, String)>,
    seq: transforms::Sequence,
    /// HEAD and the staged diff when the operation started, for `commit_message`.
    fingerprint: Option<Fingerprint>,
    /// The run of the transform in progress, and its workflow's name.
    run: Option<(ScriptRunId, String)>,
}

/// The `transform.conflict_hunk` runs of one read of a conflicted file (`WF3d`, D168). The hunks go
/// one after another in file order, and the workflows of each hunk go one after another in
/// file-name order, each reading the suggestion so far.
struct HunkRun {
    /// The read the suggestions answer: `HunkSuggested { read }`.
    read: OperationId,
    site: ConflictSite,
    root: PathBuf,
    file: ConflictFileRead,
    /// The branch `match` reads: HEAD's, or the conflicted branch of a restack.
    branch: Option<String>,
    /// The hunks not finished, in file order. The first one is the hunk in progress.
    hunks: VecDeque<HunkJob>,
    /// The workflows of the hunk in progress not started yet: name and hash.
    waiting: VecDeque<(String, String)>,
    /// The suggestion of the hunk in progress, folded from its workflows.
    seq: transforms::Sequence,
    /// The workflows already asked for review for this read: one request each (`WF3b`).
    asked: BTreeSet<String>,
    /// The workflow run in progress, and its name.
    run: Option<(ScriptRunId, String)>,
}

/// One conflict hunk of a read, with the workflows that apply to it.
struct HunkJob {
    index: usize,
    hunk: Hunk,
    workflows: Vec<(String, String)>,
}

/// The restack conflict being resolved (D154): the plan it came from, and the conflict
/// whose commit is replayed in `worktree`. One at a time, in memory; a worktree left by a
/// crash is removed when the repository opens (`ResumeRestack`).
struct ResolutionSession {
    from: String,
    skip: Vec<String>,
    /// The branch whose commit conflicts: the `branch` of `restack.resumed` (D162(1)).
    conflicted: String,
    parent: String,
    commit: String,
    onto: String,
    worktree: PathBuf,
}

/// The walk behind the history list: the stream while no job reads it, how to
/// stop its git process, and the page request in flight.
struct HistorySession {
    stream: Option<Box<dyn HistoryStream>>,
    stop: Canceller,
    /// The last commit the list holds. A page after it continues the walk.
    last: Option<String>,
    active: Option<ActiveHistory>,
}

/// A page request that has not answered yet.
struct ActiveHistory {
    id: OperationId,
    actor: Actor,
    kind: CommandKind,
}

/// A commit read (the detail, or one file of it) that has not answered yet.
/// `stop` is shared with its job: setting it ends the read.
struct ActiveDetail {
    op: ActiveHistory,
    stop: Arc<AtomicBool>,
}

/// A fetch, pull or push with lease that has not answered yet, and the
/// repository it runs in.
struct ActiveRemote {
    op: ActiveHistory,
    root: PathBuf,
    /// The phases of a push, whose `after.push` observers run when it succeeds (`WF3c`).
    phases: Option<Phases>,
}

struct Op<'a> {
    id: OperationId,
    command: &'a Command,
    state: OperationState,
}

impl Engine {
    pub fn new(git: Box<dyn GitPort>) -> Self {
        Self {
            git,
            root: None,
            log: Vec::new(),
            history: None,
            history_jobs: Vec::new(),
            detail: None,
            detail_jobs: Vec::new(),
            stack: None,
            stack_jobs: Vec::new(),
            files: None,
            index_jobs: Vec::new(),
            files_search: None,
            files_waiting: None,
            search_jobs: Vec::new(),
            files_read: None,
            read_jobs: Vec::new(),
            highlight: None,
            highlight_jobs: Vec::new(),
            text_search: None,
            text_jobs: Vec::new(),
            provenance: None,
            provenance_jobs: Vec::new(),
            remote: None,
            remote_jobs: Vec::new(),
            script_waiting: BTreeMap::new(),
            script_jobs: Vec::new(),
            workflow_user_dir: loader::default_user_dir(),
            reviewer_user_dir: crate::paths::config_dir().map(|dir| reviewers::user_dir(&dir)),
            gates_user_dir: crate::paths::config_dir(),
            workspace_home: std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from),
            workspace_config: None,
            workflow_trash_dir: crate::paths::data_dir().map(|data| data.join("workflow-trash")),
            contexts_db: crate::paths::data_dir().map(|data| data.join("contexts.db")),
            script_link: None,
            script: None,
            trust: None,
            api_written: false,
            review: None,
            threads: None,
            restack_resolution: None,
            triggers: false,
            lanes: Lanes::default(),
            tips: None,
            stack_seen: None,
            gate: None,
            observers: BTreeMap::new(),
            transform: None,
            conflict_hunks: None,
            wanted: BTreeMap::new(),
        }
    }

    /// Sets the directory workflows are read from before `gitmaster.workflowPath`.
    /// `None` reads none. The default is the user's config directory.
    pub fn set_workflow_user_dir(&mut self, dir: Option<PathBuf>) {
        self.workflow_user_dir = dir;
    }

    /// Sets the directory of the user's reviewers (D212 (1)). `None` reads none. The default is
    /// `reviewers/` of the user's config directory; tests point it at a temporary folder.
    pub fn set_reviewer_user_dir(&mut self, dir: Option<PathBuf>) {
        self.reviewer_user_dir = dir;
    }

    /// Sets the directory of the user's `gates.json` (D212 (2)). `None` reads none. The default
    /// is the user's configuration directory; tests point it at a temporary folder.
    pub fn set_gates_user_dir(&mut self, dir: Option<PathBuf>) {
        self.gates_user_dir = dir;
    }

    /// Sets the folder deleted workflow files are moved to. `None` refuses
    /// deletes. The default is `workflow-trash` in the app's data directory.
    pub fn set_workflow_trash_dir(&mut self, dir: Option<PathBuf>) {
        self.workflow_trash_dir = dir;
    }

    /// Sets the contexts file. `None` leaves the contexts unavailable: they read as empty and no
    /// write is possible. The default is `contexts.db` in the app's data directory; tests set a
    /// temporary one.
    pub fn set_contexts_db(&mut self, path: Option<PathBuf>) {
        self.contexts_db = path;
    }

    /// Sets the global Git configuration the workspace commands' Git calls read (D177 (5)). `None`
    /// reads the process's own. Tests give the commits of a workspace an identity this way.
    pub fn set_workspace_global_config(&mut self, path: Option<PathBuf>) {
        self.workspace_config = path;
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// The history pages accepted since the last call. The caller runs each on
    /// a thread and passes its report to `history_report`.
    pub fn take_history_jobs(&mut self) -> Vec<HistoryJob> {
        std::mem::take(&mut self.history_jobs)
    }

    /// The reads of the file list accepted since the last call. The caller runs
    /// each on a thread and passes its report to `index_report`.
    pub fn take_index_jobs(&mut self) -> Vec<IndexJob> {
        std::mem::take(&mut self.index_jobs)
    }

    /// The file searches accepted since the last call. The caller runs each on
    /// a thread and passes its report to `search_report`.
    pub fn take_search_jobs(&mut self) -> Vec<SearchJob> {
        std::mem::take(&mut self.search_jobs)
    }

    /// The text searches accepted since the last call. The caller runs each on a
    /// thread and passes its report to `text_report`.
    pub fn take_text_jobs(&mut self) -> Vec<TextJob> {
        std::mem::take(&mut self.text_jobs)
    }

    /// The provenance reads accepted since the last call (D144). The caller runs each job on
    /// a thread of its own and passes its report back to `provenance_report`.
    pub fn take_provenance_jobs(&mut self) -> Vec<ProvenanceJob> {
        std::mem::take(&mut self.provenance_jobs)
    }

    /// The file reads accepted since the last call. The caller runs each on a
    /// thread and passes its report to `read_report`.
    pub fn take_read_jobs(&mut self) -> Vec<ReadJob> {
        std::mem::take(&mut self.read_jobs)
    }

    /// The colorings accepted since the last call. The caller runs each on a
    /// thread and passes its report to `highlight_report`.
    pub fn take_highlight_jobs(&mut self) -> Vec<HighlightJob> {
        std::mem::take(&mut self.highlight_jobs)
    }

    /// The commit detail reads accepted since the last call. The caller runs
    /// each on a thread and passes its report to `detail_report`.
    pub fn take_detail_jobs(&mut self) -> Vec<DetailJob> {
        std::mem::take(&mut self.detail_jobs)
    }

    /// The file diff reads accepted since the last call. The caller runs each on a
    /// thread and passes its report to `stack_report`.
    pub fn take_stack_jobs(&mut self) -> Vec<StackJob> {
        std::mem::take(&mut self.stack_jobs)
    }

    /// The remote operations accepted since the last call. The caller runs each
    /// on a thread and passes its report to `remote_report`.
    pub fn take_remote_jobs(&mut self) -> Vec<RemoteJob> {
        std::mem::take(&mut self.remote_jobs)
    }

    /// Sets how script runs post their effects and events to the engine thread. The runner
    /// calls it when it starts the engine; tests give their own link.
    pub fn set_script_link(&mut self, link: ScriptLink) {
        self.script_link = Some(link);
    }

    /// Starts a script run in the open repository and returns its id at once; the run goes on
    /// its own thread. Its Git writes are logged under `id`. The first call opens the journal in
    /// `state.db` and marks the runs a previous session left unfinished as interrupted (D100).
    pub fn start_script(&mut self, id: OperationId, spec: RunSpec) -> Result<ScriptRunId, String> {
        let root = self.open_root()?;
        let link = self
            .script_link
            .clone()
            .ok_or_else(|| "script runs are not connected to the engine".to_owned())?;
        if self
            .script
            .as_ref()
            .is_none_or(|session| session.root != root)
        {
            let common = self.git.common_dir(&root).map_err(|e| e.0)?;
            let db = common.join("gitmaster").join("state.db");
            let clock: crate::script::run::Clock = Arc::new(now_ms);
            self.script = Some(ScriptSession::open(
                root,
                &db,
                &link,
                self.git.as_ref(),
                clock,
                id,
            )?);
        }
        let session = self
            .script
            .as_mut()
            .ok_or_else(|| "the script session is not open".to_owned())?;
        let workflow = spec.workflow.clone();
        let run = session.runtime.start(spec)?;
        session.note_origin(run, id);
        session.note_workflow(run, workflow);
        Ok(run)
    }

    /// The runtime of the open repository's script runs: how a person approves or cancels one,
    /// and how an event wakes a wait. `None` before the first run.
    pub fn script_runtime(&self) -> Option<Runtime> {
        self.script.as_ref().map(|session| session.runtime.clone())
    }

    /// Carries out an effect a script run posted, and answers the run. An effect that waits for Git
    /// (`ctx.fetch`, `ctx.push.lease`) is answered when its command ends, or when the run is
    /// cancelled, and meanwhile the engine serves other commands (`WF4a`). Returns the events the
    /// Git write caused (the new status).
    pub fn script_effect(&mut self, request: EffectRequest) -> Vec<Event> {
        let Some(session) = self.script.as_mut() else {
            return Vec::new();
        };
        let root = session.root.clone();
        let run = request.call.run;
        let origin = session.origin(run);
        match session.execute(self.git.as_ref(), &mut self.log, request) {
            Carried::Answered(effected) => {
                let mut events = self.script_answered(origin, &root, &effected);
                if let Some(plan) = effected.conflict {
                    events.extend(self.restack_conflict(run, plan));
                }
                events
            }
            Carried::Waiting(waiting) => {
                let Waiting {
                    run,
                    kind,
                    remote,
                    reply,
                } = *waiting;
                self.script_waiting.insert(
                    run,
                    ScriptWaiting {
                        origin,
                        root,
                        kind,
                        reply,
                        stop: remote.stop,
                    },
                );
                self.script_jobs.push(ScriptJob {
                    run,
                    command: remote.run,
                });
                Vec::new()
            }
        }
    }

    /// A run's restack stopped on a conflict: the event names the run, its workflow and the plan
    /// after the apply, so the window can open Resolve conflict (`WF4c`).
    fn restack_conflict(&self, run: ScriptRunId, plan: RestackPlan) -> Vec<Event> {
        let workflow = self
            .script
            .as_ref()
            .and_then(|session| session.workflow(run))
            .unwrap_or_default()
            .to_owned();
        vec![Event::WorkflowRestackConflict {
            run,
            workflow,
            plan,
        }]
    }

    /// Takes the Git commands script runs wait for, to start them on threads of their own.
    pub fn take_script_jobs(&mut self) -> Vec<ScriptJob> {
        std::mem::take(&mut self.script_jobs)
    }

    /// The end of a Git command a script run waits for: the run gets its answer, and the command is
    /// logged under the run's operation (D81). A report for a run that was cancelled meanwhile is
    /// dropped.
    pub fn script_remote_report(&mut self, report: ScriptReport) -> Vec<Event> {
        let Some(waiting) = self.script_waiting.remove(&report.run) else {
            return Vec::new();
        };
        let (state, result, refs) = match report.outcome {
            Ok(()) => (OperationState::Done, EffectResult::done(Value::Null), true),
            Err(message) => (OperationState::Failed, EffectResult::failed(message), false),
        };
        self.log.push(LogEntry {
            id: waiting.origin,
            actor: Actor::Human,
            kind: waiting.kind,
            state,
        });
        let _ = waiting.reply.send(result);
        let effected = Effected {
            wrote: true,
            refs,
            ..Effected::default()
        };
        self.script_answered(waiting.origin, &waiting.root, &effected)
    }

    /// Ends the wait of a script run whose command is still running: the command is stopped, and
    /// the run gets `cancelled` (`WF4a`). The command is logged as failed.
    fn stop_script_wait(&mut self, run: ScriptRunId) {
        let Some(waiting) = self.script_waiting.remove(&run) else {
            return;
        };
        (waiting.stop)();
        self.log.push(LogEntry {
            id: waiting.origin,
            actor: Actor::Human,
            kind: waiting.kind,
            state: OperationState::Failed,
        });
        let _ = waiting.reply.send(EffectResult::cancelled());
    }

    /// What an answered script call changed, as events: the replays of moved commits resolve the
    /// thread links (D145), the triggers see the baseline of this write (`WF3a`), and the window reads
    /// the status again, and the branches and the stacks if refs moved (`WF4a`).
    fn script_answered(
        &mut self,
        origin: OperationId,
        root: &Path,
        effected: &Effected,
    ) -> Vec<Event> {
        threads_session::remap(&mut self.threads, self.git.as_ref(), root, &effected.remaps);
        // A run's own writes become the baseline of `refs.changed`, so they fire nothing (no
        // cascades, WF3a).
        if effected.wrote && self.triggers {
            self.snapshot_tips(root);
            // The same for the stacks: a run's commit on a parent fires no `stack.*` (D162(2)).
            self.refresh_stacks(origin, root, false);
        }
        let mut events = self.workflow_events(origin, root, Vec::new(), effected.wrote);
        // Moved refs change the branch list and the stacks the window shows (WF4a).
        if effected.refs {
            events.extend(self.branches_changed(origin, root).unwrap_or_default());
            if let Ok(stacks) = stack_session::snapshot(self.git.as_ref(), root) {
                events.push(Event::StacksLoaded { id: origin, stacks });
            }
        }
        // A restack a run made is undoable like one the window made, without a reopen (WF4c).
        if effected.restack {
            let record = restack_session::last(self.git.as_ref(), root)
                .ok()
                .flatten();
            events.push(Event::RestackFinished { id: origin, record });
        }
        events
    }

    /// Turns what a script run reported into the events the front ends get.
    pub fn script_event(&mut self, event: ScriptEvent) -> Vec<Event> {
        if let (ScriptEvent::Finished { run, .. }, Some(session)) = (&event, self.script.as_mut()) {
            session.drop_diff(*run);
        }
        let mut started = Vec::new();
        match &event {
            ScriptEvent::Applying { run } => self.lanes.applied(*run),
            ScriptEvent::Finished {
                run,
                status,
                reason,
                value,
                ..
            } => {
                // The run's script may have a pending trigger, which starts now (WF3a).
                if let Some((name, (origin, trigger))) = self.lanes.finished(*run) {
                    let root = self.script.as_ref().map(|session| session.root.clone());
                    started = if let Some(root) = root {
                        self.begin_event_run(origin, &root, &name, &trigger)
                    } else {
                        self.lanes.start_failed(&name);
                        Vec::new()
                    };
                }
                // A gate or an observer of a phase moves its operation on (`WF3c`).
                started.extend(self.phase_finished(*run, *status, reason.as_deref()));
                // A transform run gives its answer to the sequence of its field (`WF3d`).
                started.extend(self.transform_finished(
                    *run,
                    *status,
                    reason.as_deref(),
                    value.as_ref(),
                ));
                // A conflict hunk run gives its suggestion for the hunk in progress (D168).
                started.extend(self.hunk_finished(
                    *run,
                    *status,
                    reason.as_deref(),
                    value.as_ref(),
                ));
            }
            _ => {}
        }
        let mut events = script_session::to_events(event);
        events.extend(started);
        events
    }

    /// Turns the event triggers on or off (`WF3a`, D155(4)). They are off by default, so the MCP
    /// server and the tests run no workflow on an event. The app turns them on (`spawn_app`).
    pub fn set_triggers(&mut self, on: bool) {
        self.triggers = on;
    }

    /// The events a command that succeeded starts (`WF3a`): `repo.opened`, `commit.created`,
    /// `checkout.done` and `restack.done`. `fetch.done` fires when the fetch ends.
    fn fire_after(
        &mut self,
        id: OperationId,
        kind: &CommandKind,
        produced: &[Event],
    ) -> Vec<Event> {
        if !self.triggers {
            return Vec::new();
        }
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        if matches!(kind, CommandKind::OpenRepo { .. }) {
            self.snapshot_tips(&root);
            // The stacks the repository opens with are the baseline and fire nothing (D162(2)).
            self.stack_seen = None;
            self.refresh_stacks(id, &root, false);
        }
        match self.trigger_of(&root, kind, produced) {
            Some(event) => self.fire_triggers(id, &root, &event),
            None => Vec::new(),
        }
    }

    /// The event that a command which succeeded makes, if it makes one.
    fn trigger_of(
        &self,
        root: &Path,
        kind: &CommandKind,
        produced: &[Event],
    ) -> Option<TriggerEvent> {
        match kind {
            CommandKind::OpenRepo { .. } => {
                let branch = self.current(root).ok().and_then(|(branch, _)| branch);
                Some(triggers::repo_opened(&root.display().to_string(), branch))
            }
            CommandKind::Commit { .. } => {
                let oid = produced.iter().find_map(|event| match event {
                    Event::Committed { oid, .. } => Some(oid.as_str()),
                    _ => None,
                })?;
                let (branch, _) = self.current(root).ok()?;
                Some(triggers::commit_created(oid, branch))
            }
            CommandKind::Checkout { .. } | CommandKind::CheckoutCommit { .. } => {
                let (branch, head) = self.current(root).ok()?;
                Some(triggers::checkout_done(branch, head.as_deref()))
            }
            CommandKind::ApplyRestack { .. } => {
                let record = produced.iter().find_map(|event| match event {
                    Event::RestackFinished {
                        record: Some(record),
                        ..
                    } => Some(record),
                    _ => None,
                })?;
                let moved: Vec<String> = record
                    .moves
                    .iter()
                    .map(|moved| moved.name.clone())
                    .collect();
                Some(triggers::restack_done(&record.from, &moved))
            }
            _ => None,
        }
    }

    /// Starts, queues or drops the runs that `event` triggers (`WF3a`). A matched script whose version
    /// the person has not reviewed does not start: it waits for the review, behind a request the
    /// window shows (`WF3b`, D155(3)).
    fn fire_triggers(&mut self, id: OperationId, root: &Path, event: &TriggerEvent) -> Vec<Event> {
        if !self.triggers {
            return Vec::new();
        }
        let mut events = self.drop_stale_requests(root);
        let loaded = match self.load_active_scripts(root) {
            Ok(loaded) => loaded,
            Err(reason) => {
                events.push(workflow_notice(format!(
                    "workflows did not run on {}: {reason}",
                    event.on
                )));
                return events;
            }
        };
        let matched: Vec<&loader::ScriptEntry> = loaded
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .meta
                    .as_ref()
                    .is_ok_and(|meta| triggers::matches(meta, event))
            })
            .collect();
        if matched.is_empty() {
            return events;
        }
        let trust = match self.trust_store() {
            Ok(trust) => trust,
            Err(reason) => {
                events.push(workflow_notice(format!(
                    "workflows did not run on {}: {reason}",
                    event.on
                )));
                return events;
            }
        };
        for entry in matched {
            let Ok(meta) = &entry.meta else {
                continue;
            };
            match reviewed_by(&trust, &entry.hash) {
                Ok(true) => {}
                Ok(false) => {
                    let request = self.want_review(
                        id,
                        root,
                        &entry.name,
                        &entry.hash,
                        meta.concurrency,
                        event,
                    );
                    events.push(request);
                    continue;
                }
                Err(reason) => {
                    events.push(workflow_notice(format!(
                        "{} did not run on {}: {}",
                        entry.name, event.on, reason
                    )));
                    continue;
                }
            }
            events.extend(self.start_on_trigger(
                id,
                root,
                &entry.name,
                meta.concurrency,
                event.clone(),
            ));
        }
        events
    }

    /// Starts the run of the script `name` for `trigger` when its lane lets it through; a run
    /// that cannot start frees its lane (`WF3a`).
    fn start_on_trigger(
        &mut self,
        id: OperationId,
        root: &Path,
        name: &str,
        mode: Concurrency,
        trigger: TriggerEvent,
    ) -> Vec<Event> {
        match self.lanes.on_trigger(name, mode, (id, trigger)) {
            Decision::Start((origin, trigger)) => {
                self.begin_event_run(origin, root, name, &trigger)
            }
            Decision::Cancel { run } => {
                if let Some(runtime) = self.script_runtime() {
                    runtime.cancel(run);
                }
                Vec::new()
            }
            Decision::Queued | Decision::Dropped => Vec::new(),
        }
    }

    /// An event for a version the person has not reviewed: the run waits for the review (`WF3b`,
    /// D155(3)). It replaces the pending request of the workflow, so the latest trigger wins.
    fn want_review(
        &mut self,
        origin: OperationId,
        root: &Path,
        name: &str,
        hash: &str,
        concurrency: Concurrency,
        trigger: &TriggerEvent,
    ) -> Event {
        self.wanted.insert(
            name.to_owned(),
            Wanted {
                hash: hash.to_owned(),
                origin,
                root: root.to_path_buf(),
                concurrency,
                trigger: trigger.clone(),
            },
        );
        Event::WorkflowWantsReview {
            workflow: name.to_owned(),
            hash: hash.to_owned(),
            trigger: trigger.on.clone(),
        }
    }

    /// Drops the pending requests whose file changed, or was removed, or that belong to another
    /// repository (`WF3b`). Returns one `WorkflowRequestDropped` for each.
    fn drop_stale_requests(&mut self, root: &Path) -> Vec<Event> {
        if self.wanted.is_empty() {
            return Vec::new();
        }
        let Ok(loaded) = self.load_active_scripts(root) else {
            return Vec::new();
        };
        let stale: Vec<String> = self
            .wanted
            .iter()
            .filter(|(name, wanted)| {
                wanted.root != root
                    || !loaded
                        .entries
                        .iter()
                        .any(|entry| entry.name == **name && entry.hash == wanted.hash)
            })
            .map(|(name, _)| name.clone())
            .collect();
        stale
            .into_iter()
            .map(|workflow| {
                self.wanted.remove(&workflow);
                Event::WorkflowRequestDropped { workflow }
            })
            .collect()
    }

    /// Starts the pending run of `name` when the person reviewed the version it waits for (`WF3b`,
    /// D155(3)). Its request is dropped whether or not the run starts.
    fn start_wanted(&mut self, root: &Path, name: &str, hash: &str) -> Vec<Event> {
        let waits = self
            .wanted
            .get(name)
            .is_some_and(|wanted| wanted.hash == hash && wanted.root == root);
        if !waits {
            return Vec::new();
        }
        let Some(wanted) = self.wanted.remove(name) else {
            return Vec::new();
        };
        let mut events = vec![Event::WorkflowRequestDropped {
            workflow: name.to_owned(),
        }];
        events.extend(self.start_on_trigger(
            wanted.origin,
            root,
            name,
            wanted.concurrency,
            wanted.trigger,
        ));
        events
    }

    /// Drops the pending request of `workflow`: the run does not start (`WF3b`).
    fn dismiss_request(&mut self, actor: &Actor, workflow: &str) -> Result<Vec<Event>, String> {
        only_a_human(actor, "dismiss a workflow request")?;
        self.wanted.remove(workflow);
        Ok(vec![Event::WorkflowRequestDropped {
            workflow: workflow.to_owned(),
        }])
    }

    /// The review of `hash` is taken back, with its grant (`WF3b`, D101). Answers the listing, so
    /// the window shows the version as unreviewed.
    fn revoke_review(
        &mut self,
        id: OperationId,
        actor: &Actor,
        hash: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "revoke a review")?;
        let root = self.open_root()?;
        self.trust_store()?.revoke(hash).map_err(|e| e.0)?;
        Ok(vec![self.scripts_listed(id, &root)?])
    }

    /// Whether the version with `hash` was reviewed, or why that cannot be read.
    fn reviewed(&mut self, hash: &str) -> Result<bool, String> {
        let trust = self.trust_store()?;
        reviewed_by(&trust, hash)
    }

    /// Starts the run of the script `name` for `trigger`, which the lanes let through. A run that
    /// does not start frees its lane, and the reason is a notice.
    fn begin_event_run(
        &mut self,
        origin: OperationId,
        root: &Path,
        name: &str,
        trigger: &TriggerEvent,
    ) -> Vec<Event> {
        match self.start_event_run(origin, root, name, trigger) {
            Ok(run) => {
                self.lanes.started(name, run);
                Vec::new()
            }
            Err(reason) => {
                self.lanes.start_failed(name);
                vec![workflow_notice(reason)]
            }
        }
    }

    /// Starts the run of the script `name` for `trigger`: the reviewed version, with its automatic
    /// approval (D101), no inputs, `input.trigger` and `input.repo`. It is started by an event, so
    /// its journal row shows `event:<name>` (D64).
    fn start_event_run(
        &mut self,
        id: OperationId,
        root: &Path,
        name: &str,
        trigger: &TriggerEvent,
    ) -> Result<ScriptRunId, String> {
        let entry = self.find_script(root, name)?;
        let trust = self.trust_store()?;
        if !reviewed_by(&trust, &entry.hash)? {
            return Err(format!(
                "{name} did not run on {}: review it first.",
                trigger.on
            ));
        }
        let auto_approve = trust.auto_approve(&entry.hash).map_err(|e| e.0)?;
        let repo = script_input::repo_input(self.git.as_ref(), root);
        let value = trigger.trigger_value();
        let input = json!({ "inputs": {}, "trigger": value.clone(), "repo": repo.value });
        let seed = u64::try_from(now_ms()).unwrap_or(0) ^ id.0.rotate_left(32);
        let run = self.start_script(
            id,
            RunSpec {
                workflow: entry.name,
                chunk: entry.file_name,
                source: entry.source,
                definition_hash: entry.hash,
                started_by: StartedBy::Event(trigger.on.clone()),
                trigger: value,
                input,
                reviewed: true,
                auto_approve,
                seed,
            },
        )?;
        if let (Some(file), Some(session)) = (repo.diff_file, self.script.as_mut()) {
            session.keep_diff(run, file);
        }
        Ok(run)
    }

    /// Reads the phases of a command that has any (`WF3c`): its facts before it runs, and the
    /// `before.<op>` and `after.<op>` scripts that apply. `None` when the command has no phases,
    /// the switch is off, or no script applies. A phase run is not subject to `concurrency`
    /// (D155): each gate is part of its operation, and the observers of one operation run beside
    /// each other.
    fn open_phases(&self, command: &Command) -> Result<Option<Phases>, String> {
        let Some(op) = phases::operation(&command.kind) else {
            return Ok(None);
        };
        if self.gate.is_some() {
            return Err(
                "another operation is waiting for its workflow gates: cancel it first".to_owned(),
            );
        }
        if !self.triggers {
            return Ok(None);
        }
        // An empty message fails at once, as the commit does, without a gate run first.
        if let CommandKind::Commit { message } = &command.kind
            && message.trim().is_empty()
        {
            return Err("the commit message is empty".to_owned());
        }
        let Ok(root) = self.open_root() else {
            return Ok(None);
        };
        let facts = self.phase_facts(&root, &command.kind)?;
        let loaded = self.load_active_scripts(&root)?;
        let (Some(before), Some(after)) = (
            phases::trigger(&command.kind, &facts, Timing::Before),
            phases::trigger(&command.kind, &facts, Timing::After),
        ) else {
            return Ok(None);
        };
        let gates = phases::select(&loaded.entries, &before);
        let observers = phases::select(&loaded.entries, &after);
        if gates.is_empty() && observers.is_empty() {
            return Ok(None);
        }
        Ok(Some(Phases {
            op,
            root,
            facts,
            before,
            gates,
            observers,
        }))
    }

    /// What the phases of `kind` read before it runs (`WF3c`): the branch HEAD is on, and for a
    /// restack the branches its plan moves.
    fn phase_facts(&self, root: &Path, kind: &CommandKind) -> Result<Facts, String> {
        let head = self.current(root).ok().and_then(|(branch, _)| branch);
        let mut facts = Facts {
            head,
            ..Facts::default()
        };
        if let CommandKind::ApplyRestack { branch, skip, .. } = kind {
            let plan = restack_session::plan(self.git.as_ref(), root, branch, skip, &|path| {
                same_folder(root, path)
            })?;
            facts.planned = phases::moved_by_plan(&plan);
        }
        Ok(facts)
    }

    /// Starts the first gate still to run (`WF3c`, D157), or runs the command once every gate
    /// passed. A gate that is not reviewed, or cannot start, vetoes the operation.
    fn step_gates(&mut self, mut gate: GatedOp, events: &mut Vec<Event>) {
        let Some((name, hash)) = gate.waiting.front().cloned() else {
            self.run_gated(gate, events);
            return;
        };
        let reviewed = self.reviewed(&hash);
        let unreviewed = matches!(reviewed, Ok(false));
        let refused = match reviewed {
            Ok(true) => None,
            Ok(false) => Some(phases::unreviewed(&name)),
            Err(reason) => Some(format!("{name} failed: {reason}")),
        };
        if let Some(text) = refused {
            // The window offers Review for this version beside the veto (`WF3b`).
            if unreviewed {
                events.push(Event::WorkflowGateUnreviewed {
                    id: gate.id,
                    workflow: name,
                    hash,
                });
            }
            self.veto(gate, text, events);
            return;
        }
        match self.start_event_run(gate.id, &gate.phases.root, &name, &gate.phases.before) {
            Ok(run) => {
                gate.run = Some(run);
                events.push(Event::GateStarted {
                    id: gate.id,
                    workflow: name,
                });
                self.gate = Some(gate);
            }
            Err(reason) => self.veto(gate, format!("{name} failed: {reason}"), events),
        }
    }

    /// The gate in progress ended with `status` (`WF3c`, D157): a clean finish moves to the next
    /// gate, and any other end vetoes the operation.
    fn gate_finished(
        &mut self,
        mut gate: GatedOp,
        status: RunStatus,
        reason: Option<&str>,
        events: &mut Vec<Event>,
    ) {
        gate.run = None;
        let Some((name, _)) = gate.waiting.front().cloned() else {
            return;
        };
        match phases::gate_answer(&name, gate.phases.op, status, reason) {
            Gate::Passed => {
                gate.waiting.pop_front();
                self.step_gates(gate, events);
            }
            Gate::Vetoed(text) => self.veto(gate, text, events),
        }
    }

    /// Runs the command of a gated operation, now that its gates passed (`WF3c`).
    fn run_gated(&mut self, gate: GatedOp, events: &mut Vec<Event>) {
        let id = gate.id;
        let command = gate.command;
        let phases = gate.phases;
        let mut op = Op {
            id,
            command: &command,
            state: OperationState::Gating,
        };
        if matches!(command.kind, CommandKind::PushWithLease) {
            self.start_remote_op(&mut op, Some(phases), events);
        } else {
            self.run_op(&mut op, Some(&phases), events);
        }
    }

    /// Ends a gated operation with a veto and its reason (`WF3c`, D157).
    fn veto(&mut self, gate: GatedOp, text: String, events: &mut Vec<Event>) {
        let id = gate.id;
        let command = gate.command;
        let mut op = Op {
            id,
            command: &command,
            state: OperationState::Gating,
        };
        self.advance_with(&mut op, OperationState::Vetoed, Some(text), events);
    }

    /// Runs the command of an operation, after its gates if it had any: executes it and, when it
    /// succeeds, starts its observers. It ends `Done` or `Failed`.
    fn run_op(&mut self, op: &mut Op, phases: Option<&Phases>, events: &mut Vec<Event>) {
        self.advance(op, OperationState::Executing, events);
        match self.execute(op.id, &op.command.actor, &op.command.kind) {
            Ok(produced) => {
                let fired = self.fire_after(op.id, &op.command.kind, &produced);
                let observed = match phases {
                    Some(found) => self.start_observers(op.id, found, &op.command.kind, &produced),
                    None => Vec::new(),
                };
                events.extend(produced);
                events.extend(fired);
                events.extend(observed);
                self.advance(op, OperationState::Done, events);
            }
            Err(message) => {
                self.advance_with(op, OperationState::Failed, Some(message), events);
            }
        }
    }

    /// Starts the observers of an operation that succeeded (`WF3c`, D157(4)): every `after.<op>`
    /// script, beside each other, with the result in `input.trigger`. A failure is a warning
    /// when the observer ends, and the operation is not affected.
    fn start_observers(
        &mut self,
        id: OperationId,
        phases: &Phases,
        kind: &CommandKind,
        produced: &[Event],
    ) -> Vec<Event> {
        let mut facts = phases.facts.clone();
        match kind {
            CommandKind::Commit { .. } => {
                facts.oid = produced.iter().find_map(|event| match event {
                    Event::Committed { oid, .. } => Some(oid.clone()),
                    _ => None,
                });
            }
            CommandKind::CreateBranch { name, .. }
            | CommandKind::CreateChildBranch { name, .. } => {
                facts.oid = self.git.branch_tip(&phases.root, name).ok().flatten();
            }
            CommandKind::PushWithLease => {
                facts.oid = self.current(&phases.root).ok().and_then(|(_, oid)| oid);
            }
            _ => {}
        }
        facts.moved = produced
            .iter()
            .find_map(|event| match event {
                Event::RestackFinished {
                    record: Some(record),
                    ..
                } => Some(phases::moved_by_record(record)),
                _ => None,
            })
            .unwrap_or_default();
        let Some(after) = phases::trigger(kind, &facts, Timing::After) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for (name, hash) in &phases.observers {
            match self.reviewed(hash) {
                Ok(true) => {}
                Ok(false) => {
                    let concurrency = self
                        .find_script(&phases.root, name)
                        .ok()
                        .and_then(|entry| entry.meta.ok().map(|meta| meta.concurrency))
                        .unwrap_or_default();
                    let request =
                        self.want_review(id, &phases.root, name, hash, concurrency, &after);
                    events.push(request);
                    continue;
                }
                Err(reason) => {
                    events.push(workflow_notice(format!(
                        "{name} did not run on {}: {reason}",
                        after.on
                    )));
                    continue;
                }
            }
            match self.start_event_run(id, &phases.root, name, &after) {
                Ok(run) => {
                    self.observers.insert(run, (name.clone(), phases.op));
                }
                Err(reason) => events.push(workflow_notice(reason)),
            }
        }
        events
    }

    /// A phase run ended (`WF3c`): an observer that failed warns, and a gate moves its operation on
    /// or vetoes it. Runs that belong to no phase return nothing.
    fn phase_finished(
        &mut self,
        run: ScriptRunId,
        status: RunStatus,
        reason: Option<&str>,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some((name, op)) = self.observers.remove(&run)
            && let Some(text) = phases::observer_failure(&name, op, status, reason)
        {
            events.push(workflow_notice(text));
        }
        if self.gate.as_ref().is_some_and(|gate| gate.run == Some(run))
            && let Some(gate) = self.gate.take()
        {
            self.gate_finished(gate, status, reason, &mut events);
        }
        events
    }

    /// Cancels an operation that waits for its gates (`WF3c`): the gate's run is stopped, and the
    /// operation ends `Cancelled` with nothing written. Refused for any other operation.
    fn cancel_operation(&mut self, actor: &Actor, op: OperationId) -> Result<Vec<Event>, String> {
        only_a_human(actor, "cancel an operation")?;
        // The read of a conflicted file is done when it answers; its hunk runs are what is cancelled (D168).
        if self
            .conflict_hunks
            .as_ref()
            .is_some_and(|hunks| hunks.read == op)
        {
            self.stop_conflict_hunks();
            return Ok(Vec::new());
        }
        if self
            .transform
            .as_ref()
            .is_some_and(|transform| transform.id == op)
        {
            let mut events = Vec::new();
            if let Some(transform) = self.transform.take() {
                self.stop_transform(&transform, &mut events);
            }
            return Ok(events);
        }
        let gate = match self.gate.take() {
            Some(gate) if gate.id == op => gate,
            other => {
                self.gate = other;
                return Err(format!(
                    "operation {} is not waiting for its workflow gates",
                    op.0
                ));
            }
        };
        if let (Some(run), Some(runtime)) = (gate.run, self.script_runtime()) {
            runtime.cancel(run);
        }
        let id = gate.id;
        let command = gate.command;
        let mut state = Op {
            id,
            command: &command,
            state: OperationState::Gating,
        };
        let mut events = Vec::new();
        self.advance(&mut state, OperationState::Cancelled, &mut events);
        Ok(events)
    }

    /// Starts a transform operation (`WF3d`, D155). `composer` is set for `ComposerOpened`, which
    /// fires `composer.opened` before the transforms run. An operation in progress is superseded:
    /// its run is cancelled and it ends with no value. With the triggers off nothing runs, and the
    /// answer has no value.
    fn start_transform(
        &mut self,
        id: OperationId,
        command: &Command,
        field: &str,
        current: &str,
        composer: bool,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        only_a_human(&command.actor, "run a transform")?;
        if !transforms::is_field(field) {
            return Err(format!("no transform runs for the field `{field}`"));
        }
        if let Some(earlier) = self.transform.take() {
            self.stop_transform(&earlier, events);
        }
        let mut run = TransformRun {
            id,
            command: command.clone(),
            root: PathBuf::new(),
            field: field.to_owned(),
            branch: None,
            waiting: VecDeque::new(),
            seq: transforms::Sequence::new(current),
            fingerprint: None,
            run: None,
        };
        if self.triggers {
            let root = self.open_root()?;
            let (branch, head) = self.current(&root)?;
            // Taken before anything runs, so a change while the transforms run shows (D155(1)).
            let fingerprint = if field == "commit_message" {
                let staged = self.git.staged_diff(&root).map_err(|e| e.0)?;
                Some(transforms::fingerprint(head.as_deref(), &staged))
            } else {
                None
            };
            let loaded = self.load_active_scripts(&root)?;
            if composer {
                let opened = TriggerEvent {
                    on: "composer.opened".to_owned(),
                    branch: branch.clone(),
                    payload: json!({}),
                };
                events.extend(self.fire_triggers(id, &root, &opened));
            }
            let event = transforms::trigger(field, branch.clone(), current);
            run.waiting = phases::select(&loaded.entries, &event)
                .into_iter()
                .collect();
            run.root = root;
            run.branch = branch;
            run.fingerprint = fingerprint;
        }
        self.next_transform(run, events);
        Ok(())
    }

    /// Starts the next transform of `run` that is reviewed, or ends the operation when none is left
    /// (`WF3d`). A transform that is not reviewed, or cannot start, is skipped with a notice, and
    /// the sequence goes on.
    fn next_transform(&mut self, mut run: TransformRun, events: &mut Vec<Event>) {
        while let Some((name, hash)) = run.waiting.pop_front() {
            let on = format!("transform.{}", run.field);
            match self.reviewed(&hash) {
                Ok(true) => {}
                // A transform is not run retroactively: the person asks again after the review
                // (`WF3b`), so nothing waits here.
                Ok(false) => {
                    events.push(Event::WorkflowWantsReview {
                        workflow: name,
                        hash,
                        trigger: on,
                    });
                    continue;
                }
                Err(reason) => {
                    let notice = format!("{name} did not run on {on}: {reason}");
                    run.seq = std::mem::take(&mut run.seq).skipped(notice);
                    flush_notices(&mut run.seq, events);
                    continue;
                }
            }
            let trigger = transforms::trigger(&run.field, run.branch.clone(), &run.seq.current);
            match self.start_event_run(run.id, &run.root, &name, &trigger) {
                Ok(script_run) => {
                    run.run = Some((script_run, name));
                    self.transform = Some(run);
                    return;
                }
                Err(reason) => {
                    run.seq =
                        std::mem::take(&mut run.seq).skipped(format!("{name} failed: {reason}"));
                    flush_notices(&mut run.seq, events);
                }
            }
        }
        self.end_transform(&run, events);
    }

    /// Ends the transform operation `run` (`WF3d`): `TransformFinished` with its value and whether
    /// the repository moved while it ran, then the operation is `Done`.
    fn end_transform(&mut self, run: &TransformRun, events: &mut Vec<Event>) {
        let changed = run
            .fingerprint
            .as_ref()
            .is_some_and(|before| self.moved_since(&run.root, before));
        events.push(Event::TransformFinished {
            id: run.id,
            field: run.field.clone(),
            value: run.seq.value(),
            changed,
        });
        let mut op = Op {
            id: run.id,
            command: &run.command,
            state: OperationState::Executing,
        };
        self.advance(&mut op, OperationState::Done, events);
    }

    /// Stops a transform operation (`WF3d`): its run, if one is in progress, is cancelled, and the
    /// operation ends `Cancelled` with no value.
    fn stop_transform(&mut self, transform: &TransformRun, events: &mut Vec<Event>) {
        if let (Some((run, _)), Some(runtime)) = (transform.run.as_ref(), self.script_runtime()) {
            runtime.cancel(*run);
        }
        events.push(Event::TransformFinished {
            id: transform.id,
            field: transform.field.clone(),
            value: None,
            changed: false,
        });
        let mut op = Op {
            id: transform.id,
            command: &transform.command,
            state: OperationState::Executing,
        };
        self.advance(&mut op, OperationState::Cancelled, events);
    }

    /// A script run ended (`WF3d`). When it was a transform of the operation in progress, its answer
    /// goes into the sequence and the next transform starts. A run that was cancelled on its own
    /// ends the operation with no value. Runs that belong to no transform return nothing.
    fn transform_finished(
        &mut self,
        run: ScriptRunId,
        status: RunStatus,
        reason: Option<&str>,
        value: Option<&serde_json::Value>,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        let owned = self
            .transform
            .as_ref()
            .and_then(|transform| transform.run.as_ref())
            .is_some_and(|(script_run, _)| *script_run == run);
        if !owned {
            return events;
        }
        let Some(mut transform) = self.transform.take() else {
            return events;
        };
        let Some((_, name)) = transform.run.take() else {
            return events;
        };
        if status == RunStatus::Cancelled {
            self.stop_transform(&transform, &mut events);
            return events;
        }
        transform.seq = std::mem::take(&mut transform.seq).advance(&name, status, reason, value);
        flush_notices(&mut transform.seq, &mut events);
        self.next_transform(transform, &mut events);
        events
    }

    /// After a read of a conflicted file answers (`WF3d`, D168): the `transform.conflict_hunk`
    /// workflows run for each conflict of the file, in the background, hunk by hunk. Nothing runs
    /// without a reviewed, enabled workflow for the hunk; an unreviewed one is asked for once.
    fn start_conflict_hunks(
        &mut self,
        id: OperationId,
        command: &Command,
        events: &mut Vec<Event>,
    ) {
        let CommandKind::ReadConflict { site, .. } = &command.kind else {
            return;
        };
        if !self.triggers {
            return;
        }
        let Some(file) = events.iter().find_map(|event| match event {
            Event::ConflictRead { id: answered, read } if *answered == id => Some(read.clone()),
            _ => None,
        }) else {
            return;
        };
        let (root, branch) = match site {
            ConflictSite::WorkingCopy => {
                let Ok(root) = self.open_root() else {
                    return;
                };
                let branch = self.current(&root).ok().and_then(|(branch, _)| branch);
                (root, branch)
            }
            ConflictSite::Restack => {
                let Ok(session) = self.open_resolution() else {
                    return;
                };
                (session.worktree.clone(), Some(session.conflicted.clone()))
            }
        };
        let hunks = conflict_hunks_of(&file.text);
        if hunks.is_empty() {
            return;
        }
        let loaded = match self.load_active_scripts(&root) {
            Ok(loaded) => loaded,
            Err(reason) => {
                events.push(workflow_notice(format!(
                    "workflows did not run on transform.conflict_hunk: {reason}"
                )));
                return;
            }
        };
        let jobs: VecDeque<HunkJob> = hunks
            .into_iter()
            .enumerate()
            .map(|(index, hunk)| {
                let event = transforms::conflict_hunk(&file, index, &hunk, branch.clone(), "");
                let workflows = phases::select(&loaded.entries, &event);
                HunkJob {
                    index,
                    hunk,
                    workflows,
                }
            })
            .collect();
        if jobs.iter().all(|job| job.workflows.is_empty()) {
            return;
        }
        self.stop_conflict_hunks();
        let run = HunkRun {
            read: id,
            site: *site,
            root,
            file,
            branch,
            waiting: jobs
                .front()
                .map(|job| job.workflows.iter().cloned().collect())
                .unwrap_or_default(),
            hunks: jobs,
            seq: transforms::Sequence::new(""),
            asked: BTreeSet::new(),
            run: None,
        };
        self.advance_conflict_hunks(run, events);
    }

    /// Starts the next workflow of the conflict hunks that is reviewed, or moves on: when every
    /// workflow of a hunk has answered, its suggestion is sent when one was kept, and the next
    /// hunk starts. The runs end with the last hunk (`WF3d`, D168).
    fn advance_conflict_hunks(&mut self, mut run: HunkRun, events: &mut Vec<Event>) {
        loop {
            if let Some((name, hash)) = run.waiting.pop_front() {
                match self.reviewed(&hash) {
                    Ok(true) => {}
                    // Asked once for the file, not once per hunk; the file is read again after the review.
                    Ok(false) => {
                        if run.asked.insert(name.clone()) {
                            events.push(Event::WorkflowWantsReview {
                                workflow: name,
                                hash,
                                trigger: "transform.conflict_hunk".to_owned(),
                            });
                        }
                        continue;
                    }
                    Err(reason) => {
                        let notice =
                            format!("{name} did not run on transform.conflict_hunk: {reason}");
                        run.seq = std::mem::take(&mut run.seq).skipped(notice);
                        flush_notices(&mut run.seq, events);
                        continue;
                    }
                }
                let Some(job) = run.hunks.front() else {
                    return;
                };
                let trigger = transforms::conflict_hunk(
                    &run.file,
                    job.index,
                    &job.hunk,
                    run.branch.clone(),
                    &run.seq.current,
                );
                match self.start_event_run(run.read, &run.root, &name, &trigger) {
                    Ok(script_run) => {
                        run.run = Some((script_run, name));
                        self.conflict_hunks = Some(run);
                        return;
                    }
                    Err(reason) => {
                        run.seq = std::mem::take(&mut run.seq)
                            .skipped(format!("{name} failed: {reason}"));
                        flush_notices(&mut run.seq, events);
                    }
                }
                continue;
            }
            // Every workflow of the hunk in progress has answered.
            let Some(job) = run.hunks.pop_front() else {
                return;
            };
            if let Some(text) = run.seq.value() {
                events.push(Event::HunkSuggested {
                    read: run.read,
                    path: run.file.path.clone(),
                    index: job.index,
                    text,
                    workflow: run.seq.kept_by().unwrap_or_default().to_owned(),
                });
            }
            flush_notices(&mut run.seq, events);
            run.seq = transforms::Sequence::new("");
            run.waiting = run
                .hunks
                .front()
                .map(|job| job.workflows.iter().cloned().collect())
                .unwrap_or_default();
            if run.hunks.is_empty() {
                return;
            }
        }
    }

    /// A script run ended (`WF3d`, D168). When it was a workflow of the conflict hunks in progress,
    /// its answer goes into the hunk's suggestion and the sequence goes on. A run cancelled on its
    /// own ends the suggestions of the file with no event. Other runs return nothing.
    fn hunk_finished(
        &mut self,
        run: ScriptRunId,
        status: RunStatus,
        reason: Option<&str>,
        value: Option<&serde_json::Value>,
    ) -> Vec<Event> {
        let owned = self
            .conflict_hunks
            .as_ref()
            .and_then(|hunks| hunks.run.as_ref())
            .is_some_and(|(script_run, _)| *script_run == run);
        if !owned {
            return Vec::new();
        }
        let Some(mut hunks) = self.conflict_hunks.take() else {
            return Vec::new();
        };
        let Some((_, name)) = hunks.run.take() else {
            return Vec::new();
        };
        if status == RunStatus::Cancelled {
            return Vec::new();
        }
        let mut events = Vec::new();
        hunks.seq = std::mem::take(&mut hunks.seq).advance(&name, status, reason, value);
        flush_notices(&mut hunks.seq, &mut events);
        self.advance_conflict_hunks(hunks, &mut events);
        events
    }

    /// Stops the conflict hunk runs in progress: the workflow running now is cancelled, and no
    /// late answer of it is sent (D168(4)).
    fn stop_conflict_hunks(&mut self) {
        let Some(hunks) = self.conflict_hunks.take() else {
            return;
        };
        if let (Some((run, _)), Some(runtime)) = (hunks.run.as_ref(), self.script_runtime()) {
            runtime.cancel(*run);
        }
    }

    /// A command that makes the conflict hunk runs in progress out of date stops them (D168(4)): a
    /// read of any file (the newest read is the one that counts), a write or forget of the file
    /// they read, opening another repository, and starting, finishing or cancelling a restack
    /// resolution.
    fn supersede_conflict_hunks(&mut self, kind: &CommandKind) {
        let Some(hunks) = self.conflict_hunks.as_ref() else {
            return;
        };
        let supersedes = match kind {
            CommandKind::ReadConflict { .. }
            | CommandKind::OpenRepo { .. }
            | CommandKind::StartRestackResolution { .. }
            | CommandKind::FinishRestackResolution
            | CommandKind::CancelRestackResolution => true,
            CommandKind::ResolveConflict { path, site, .. }
            | CommandKind::ForgetRecordedResolution { path, site } => {
                *path == hunks.file.path && *site == hunks.site
            }
            _ => false,
        };
        if supersedes {
            self.stop_conflict_hunks();
        }
    }

    /// The watcher saw refs move, after its quiet period (`WF3a`). The local branch tips are compared
    /// with the last check, and `refs.changed` fires once for each branch that moved. The first
    /// check only takes the baseline.
    pub fn refs_changed(&mut self, id: OperationId) -> Vec<Event> {
        if !self.triggers {
            return Vec::new();
        }
        let Ok(root) = self.open_root() else {
            return Vec::new();
        };
        let Ok(tips) = self.git.local_tips(&root) else {
            return Vec::new();
        };
        let now: BTreeMap<String, String> = tips.into_iter().collect();
        let previous = self.tips.replace((root.clone(), now.clone()));
        let mut events = Vec::new();
        if let Some((_, before)) = previous.filter(|(previous_root, _)| *previous_root == root) {
            for change in triggers::ref_changes(&before, &now) {
                events.extend(self.fire_triggers(id, &root, &triggers::refs_changed(&change)));
            }
        }
        // The stacks are read after the refs moved, so a parent that moved fires `stack.*` (WF3f).
        events.extend(self.refresh_stacks(id, &root, true));
        events
    }

    /// Takes `stacks` as the newest snapshot the engine saw (`WF3f`, D162(2)). A snapshot of another
    /// repository, or the first one, is the baseline and fires nothing. Otherwise, when `fire` is
    /// set, each transition fires its `stack.*` event.
    fn stacks_seen(
        &mut self,
        id: OperationId,
        root: &Path,
        stacks: &StackSnapshot,
        fire: bool,
    ) -> Vec<Event> {
        if !self.triggers {
            return Vec::new();
        }
        let previous = self
            .stack_seen
            .replace((root.to_path_buf(), stacks.clone()));
        let Some((_, before)) = previous.filter(|(previous_root, _)| previous_root == root) else {
            return Vec::new();
        };
        if !fire {
            return Vec::new();
        }
        let mut events = Vec::new();
        for mut event in triggers::stack_transitions(&before, stacks) {
            if event.on == "stack.parent_merged" {
                self.add_grandparent(root, &mut event);
            }
            events.extend(self.fire_triggers(id, root, &event));
        }
        events
    }

    /// Adds `grandparent` to a `stack.parent_merged` event: the branch `ReparentToGrandparent` would
    /// give its branch (D138(5)), read now. The built-in reparent workflow moves onto it (`WF4b`).
    /// A read that fails leaves the payload without it, and the script then fails with a reason.
    fn add_grandparent(&self, root: &Path, event: &mut TriggerEvent) {
        let Some(branch) = event.branch.clone() else {
            return;
        };
        if let Ok(Some(grandparent)) =
            stack_session::grandparent_of(self.git.as_ref(), root, &branch)
            && let Some(fields) = event.payload.as_object_mut()
        {
            fields.insert("grandparent".to_owned(), json!(grandparent));
        }
    }

    /// Reads the stacks of `root` and takes them as the newest snapshot (`WF3f`). They are read only
    /// while a loaded workflow listens to a `stack.*` event; otherwise the last snapshot is dropped,
    /// so the next one that is read is a baseline.
    fn refresh_stacks(&mut self, id: OperationId, root: &Path, fire: bool) -> Vec<Event> {
        if !self.triggers {
            return Vec::new();
        }
        let listened = self.load_active_scripts(root).is_ok_and(|loaded| {
            loaded.entries.iter().any(|entry| {
                entry
                    .meta
                    .as_ref()
                    .is_ok_and(|meta| meta.trigger.on.starts_with("stack."))
            })
        });
        if !listened {
            self.stack_seen = None;
            return Vec::new();
        }
        match stack_session::snapshot(self.git.as_ref(), root) {
            Ok(stacks) => self.stacks_seen(id, root, &stacks, fire),
            Err(_) => Vec::new(),
        }
    }

    /// Reads the branch tips of `root` as the baseline of `refs.changed`. A failed read keeps the
    /// baseline it had.
    fn snapshot_tips(&mut self, root: &Path) {
        if let Ok(tips) = self.git.local_tips(root) {
            self.tips = Some((root.to_path_buf(), tips.into_iter().collect()));
        }
    }

    /// Chooses where reviews are kept, in place of the app's data directory (tests).
    pub fn set_trust_store(&mut self, store: TrustStore) {
        self.trust = Some(store);
    }

    /// Turns the answer of a remote operation into events: the operation ends
    /// (with Git's text on failure, or the success message), then the status and
    /// the branches are read again, since ahead and behind may have moved. A report
    /// for an operation that is not in flight is dropped.
    pub fn remote_report(&mut self, report: RemoteReport) -> Vec<Event> {
        let active = match self.remote.take() {
            Some(active) if active.op.id == report.id => active,
            other => {
                self.remote = other;
                return Vec::new();
            }
        };
        let (state, detail) = match report.outcome {
            Ok(()) => (
                OperationState::Done,
                Some(success_message(&active.op.kind).to_owned()),
            ),
            Err(message) => (OperationState::Failed, Some(message)),
        };
        let mut events = Vec::new();
        // The status is read for the repository the operation ran in, and only while
        // it is still the open one. A read that fails leaves the last shown state.
        if self.root.as_deref() == Some(active.root.as_path()) {
            if let Ok(status) = self.git.status(&active.root) {
                events.push(Event::StatusLoaded {
                    id: report.id,
                    status,
                });
            }
            if let Ok(branches) = self.git.branches(&active.root) {
                let user_email = self.git.user_email(&active.root).unwrap_or_default();
                events.push(Event::BranchesLoaded {
                    id: report.id,
                    branches,
                    user_email,
                });
            }
            if state == OperationState::Done && matches!(active.op.kind, CommandKind::Fetch) {
                events.extend(self.fire_triggers(
                    report.id,
                    &active.root,
                    &triggers::fetch_done("all"),
                ));
                events.extend(self.refresh_stacks(report.id, &active.root, true));
            }
            // The observers of a push run when it succeeded, in the repository it ran in (D157(4)).
            if state == OperationState::Done
                && let Some(phases) = &active.phases
            {
                events.extend(self.start_observers(report.id, phases, &active.op.kind, &[]));
            }
        }
        // The end comes last: a status clears the error line, and a failure must stay on it.
        events.push(self.finish_op(&active.op, state, detail));
        events
    }

    /// Turns the answer of a file diff read into events. A report for a read that
    /// was superseded, or for a repository that was left, is dropped.
    pub fn stack_report(&mut self, report: StackReport) -> Vec<Event> {
        match self.stack.take() {
            Some(active) if active.op.id == report.id => {
                let (state, detail) = match &report.outcome {
                    Ok(_) => (OperationState::Done, None),
                    Err(message) => (OperationState::Failed, Some(message.clone())),
                };
                let mut events = vec![self.finish_op(&active.op, state, detail)];
                if let Ok(diffs) = report.outcome {
                    events.push(Event::CommitStackLoaded {
                        id: report.id,
                        diffs,
                    });
                }
                events
            }
            other => {
                self.stack = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a read of the file list into events: `FilesIndexed`,
    /// and the search that was waiting for the list starts. A report for a read
    /// that was replaced, or for a repository that was left, is dropped.
    pub fn index_report(&mut self, report: IndexReport) -> Vec<Event> {
        let Some(session) = self.files.as_mut() else {
            return Vec::new();
        };
        let building = match session.building.take() {
            Some(building) if building.token == report.token => building,
            other => {
                session.building = other;
                return Vec::new();
            }
        };
        match report.outcome {
            Ok(index) => {
                let index = Arc::new(index);
                session.index = Some(Arc::clone(&index));
                let files = index.len();
                if let (Some(active), Some(query)) = (&self.files_search, self.files_waiting.take())
                {
                    self.search_jobs.push(SearchJob {
                        id: active.op.id,
                        index,
                        query,
                        indexing: false,
                        stop: Arc::clone(&active.stop),
                    });
                }
                vec![Event::FilesIndexed {
                    id: building.id,
                    files,
                }]
            }
            // A search that waited for the list fails with Git's message. With
            // none waiting the failure is silent: the next request reads again.
            Err(message) => {
                if self.files_waiting.take().is_none() {
                    return Vec::new();
                }
                match self.files_search.take() {
                    Some(active) => {
                        vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                    }
                    None => Vec::new(),
                }
            }
        }
    }

    /// Turns the answer of a file search into events. A report for a search
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn search_report(&mut self, report: SearchReport) -> Vec<Event> {
        match self.files_search.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Some(search) => vec![
                    self.finish_op(&active.op, OperationState::Done, None),
                    Event::FilesFound {
                        id: report.id,
                        search,
                    },
                ],
                None => vec![self.finish_op(&active.op, OperationState::Cancelled, None)],
            },
            other => {
                self.files_search = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a text search into events. A report for a search
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn text_report(&mut self, report: TextReport) -> Vec<Event> {
        match self.text_search.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Ok(result) => vec![
                    self.finish_op(&active.op, OperationState::Done, None),
                    Event::TextFound {
                        id: report.id,
                        result,
                    },
                ],
                Err(message) => {
                    vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                }
            },
            other => {
                self.text_search = other;
                Vec::new()
            }
        }
    }

    /// Turns the blame of a provenance read into its answer (D144). A report for a read that was
    /// superseded, or for a repository that was left, is dropped.
    pub fn provenance_report(&mut self, report: ProvenanceReport) -> Vec<Event> {
        match self.provenance.take() {
            Some(active) if active.op.id == report.id => {
                let CommandKind::ReadProvenance {
                    path,
                    from_line,
                    to_line,
                } = &active.op.kind
                else {
                    return vec![Self::operation_changed(
                        &active.op,
                        OperationState::Failed,
                        None,
                    )];
                };
                let answer = report
                    .lines
                    .and_then(|lines| self.provenance_answer(path, *from_line, *to_line, &lines));
                match answer {
                    Ok(provenance) => vec![
                        Self::operation_changed(&active.op, OperationState::Done, None),
                        Event::ThreadProvenance {
                            id: report.id,
                            provenance,
                        },
                    ],
                    Err(message) => {
                        vec![Self::operation_changed(
                            &active.op,
                            OperationState::Failed,
                            Some(message),
                        )]
                    }
                }
            }
            other => {
                self.provenance = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a file read into events. A report for a read that
    /// was superseded, or for a repository that was left, is dropped.
    pub fn read_report(&mut self, report: ReadReport) -> Vec<Event> {
        match self.files_read.take() {
            Some(active) if active.op.id == report.id => match report.outcome {
                Ok(file) => {
                    self.start_highlight(report.id, &file);
                    vec![
                        self.finish_op(&active.op, OperationState::Done, None),
                        Event::FileRead {
                            id: report.id,
                            file,
                        },
                    ]
                }
                Err(message) => {
                    vec![self.finish_op(&active.op, OperationState::Failed, Some(message))]
                }
            },
            other => {
                self.files_read = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a coloring into an event. A report for a read that was superseded, or
    /// for a coloring that gave up, is dropped.
    pub fn highlight_report(&mut self, report: HighlightReport) -> Vec<Event> {
        match self.highlight.take() {
            Some((id, _)) if id == report.id => report
                .spans
                .map(|spans| Event::FileHighlighted {
                    id: report.id,
                    spans,
                })
                .into_iter()
                .collect(),
            other => {
                self.highlight = other;
                Vec::new()
            }
        }
    }

    /// Queues the coloring of the text of `file`, when its language has a grammar. The text is
    /// copied so the viewer shows it first and the colors follow.
    fn start_highlight(&mut self, id: OperationId, file: &FileView) {
        self.stop_highlight();
        let FileBody::Text { text } = &file.body else {
            return;
        };
        let Some(language) = syntax::language_for_path(&file.path) else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.highlight = Some((id, Arc::clone(&stop)));
        self.highlight_jobs.push(HighlightJob {
            id,
            language,
            text: text.clone(),
            stop,
        });
    }

    fn stop_highlight(&mut self) {
        if let Some((_, stop)) = self.highlight.take() {
            stop.store(true, Ordering::SeqCst);
        }
    }

    /// Turns the answer of a commit detail read into events. A report for a
    /// read that was superseded, or for a repository that was left, is dropped.
    pub fn detail_report(&mut self, report: DetailReport) -> Vec<Event> {
        match self.detail.take() {
            Some(active) if active.op.id == report.id => {
                let state = if report.outcome.is_ok() {
                    OperationState::Done
                } else {
                    OperationState::Failed
                };
                let mut events = Vec::new();
                match report.outcome {
                    Ok(detail) => {
                        events.push(self.finish_op(&active.op, state, None));
                        events.push(Event::CommitDetailLoaded {
                            id: report.id,
                            detail,
                        });
                    }
                    Err(message) => {
                        events.push(self.finish_op(&active.op, state, Some(message)));
                    }
                }
                events
            }
            other => {
                self.detail = other;
                Vec::new()
            }
        }
    }

    /// Turns the answer of a history page into events. A report for a request
    /// that was superseded, or for a repository that was left, is dropped.
    pub fn history_report(&mut self, report: HistoryReport) -> Vec<Event> {
        let Some(mut session) = self.history.take() else {
            return Vec::new();
        };
        let active = match session.active.take() {
            Some(active) if active.id == report.id => active,
            other => {
                session.active = other;
                self.history = Some(session);
                return Vec::new();
            }
        };
        let mut events = Vec::new();
        match report.outcome {
            HistoryOutcome::Page { stream, page } => {
                if let Some(last) = page.commits.last() {
                    session.last = Some(last.oid.clone());
                }
                session.stream = stream;
                let history = History {
                    commits: page.commits,
                    unborn: page.unborn,
                    more: !page.done,
                };
                events.push(self.finish_op(&active, OperationState::Done, None));
                events.push(Event::HistoryLoaded {
                    id: report.id,
                    history,
                });
            }
            HistoryOutcome::Failed(message) => {
                // The walk is gone; the cursor stays, so a later page resumes after it.
                session.stream = None;
                events.push(self.finish_op(&active, OperationState::Failed, Some(message)));
            }
        }
        self.history = Some(session);
        events
    }

    /// Starts the read a command asks for when it is one of the reads that run
    /// on a thread and answer later: the history, a commit, a stack of file
    /// diffs, a file search or a file. `None` for any other command. A read in
    /// flight of the same kind is superseded; the result carries the events of
    /// that cancellation.
    fn start_read(
        &mut self,
        id: OperationId,
        command: &Command,
    ) -> Option<Result<Vec<Event>, String>> {
        let (actor, kind) = (&command.actor, &command.kind);
        Some(match kind {
            CommandKind::LoadHistory { .. } => self.start_history(id, actor, kind, None),
            CommandKind::LoadHistoryPage { after } => {
                self.start_history(id, actor, kind, Some(after.clone()))
            }
            CommandKind::LoadCommitStack { oid, files } => {
                self.start_stack(id, actor, kind, oid, files)
            }
            CommandKind::LoadCommitDetail { oid } => self.start_detail(id, actor, kind, oid),
            CommandKind::SearchFiles { query } => self.start_files_search(id, actor, kind, query),
            CommandKind::ReadFile { path } => self.start_file_read(id, actor, kind, path),
            CommandKind::SearchText {
                query,
                options,
                paths,
            } => self.start_text_search(id, actor, kind, query, *options, paths),
            CommandKind::ReadProvenance {
                path,
                from_line,
                to_line,
            } => self.start_provenance(id, actor, kind, path, *from_line, *to_line),
            _ => return None,
        })
    }

    /// Runs one command to a terminal state and returns the events, in order.
    pub fn handle(&mut self, id: OperationId, command: &Command) -> Vec<Event> {
        self.supersede_conflict_hunks(&command.kind);
        let mut events = Vec::new();
        let mut op = Op {
            id,
            command,
            state: OperationState::Requested,
        };
        self.record(&op, None, &mut events);
        if let CommandKind::BypassGates {
            gates,
            reason,
            then,
        } = &command.kind
        {
            self.bypass_gates(&mut op, gates, reason, then, &mut events);
        } else {
            // Gating has nothing to check for most commands; the states are walked so that every
            // operation has the same shape in the log.
            self.advance(&mut op, OperationState::Gating, &mut events);
            self.run_command(&mut op, GateCheck::Native, &mut events);
        }
        events
    }

    /// Runs the command of `op`, which is `Gating`, to its end. The native gate check runs first
    /// unless `gates` is `Bypassed` (D212 (3), (4)); the workflow phases run either way. An
    /// operation with phases (`WF3c`) stays in Gating while its gates run.
    fn run_command(&mut self, op: &mut Op, gates: GateCheck, events: &mut Vec<Event>) {
        let id = op.id;
        let command = op.command;
        if gates == GateCheck::Native && self.gate_stops(op, events) {
            return;
        }
        if let Some(started) = self.start_read(id, command) {
            match started {
                Ok(superseded) => {
                    events.extend(superseded);
                    self.advance(op, OperationState::Executing, events);
                }
                Err(message) => {
                    self.advance_with(op, OperationState::Failed, Some(message), events);
                }
            }
            return;
        }
        if matches!(command.kind, CommandKind::Fetch | CommandKind::Pull) {
            self.start_remote_op(op, None, events);
            return;
        }
        if matches!(command.kind, CommandKind::PushWithLease) {
            self.run_push(op, events);
            return;
        }
        // A transform operation runs its workflows and answers with `TransformFinished` (`WF3d`).
        let transform = match &command.kind {
            CommandKind::RunTransform { field, current } => {
                Some((field.clone(), current.clone(), false))
            }
            CommandKind::ComposerOpened => Some(("commit_message".to_owned(), String::new(), true)),
            _ => None,
        };
        if let Some((field, current, composer)) = transform {
            self.advance(op, OperationState::Executing, events);
            if let Err(message) =
                self.start_transform(id, command, &field, &current, composer, events)
            {
                self.advance_with(op, OperationState::Failed, Some(message), events);
            }
            return;
        }
        match self.open_phases(command) {
            Err(message) => {
                self.advance_with(op, OperationState::Failed, Some(message), events);
            }
            Ok(Some(found)) if !found.gates.is_empty() => {
                let gate = gated_op(op, found);
                self.step_gates(gate, events);
            }
            Ok(found) => {
                self.run_op(op, found.as_ref(), events);
                self.start_conflict_hunks(id, command, events);
            }
        }
    }

    /// A push (`PushWithLease`) after the native check: its `before.push` gates run first, and then
    /// the push starts as a remote operation, whose `after.push` observers run when it succeeds
    /// (`WF3c`, D157). A push that cannot start now is refused before any gate runs.
    fn run_push(&mut self, op: &mut Op, events: &mut Vec<Event>) {
        if let Err(message) = self.remote_free(&op.command.actor, &op.command.kind) {
            self.advance_with(op, OperationState::Failed, Some(message), events);
            return;
        }
        match self.open_phases(op.command) {
            Err(message) => self.advance_with(op, OperationState::Failed, Some(message), events),
            Ok(Some(found)) if !found.gates.is_empty() => {
                let gate = gated_op(op, found);
                self.step_gates(gate, events);
            }
            Ok(found) => self.start_remote_op(op, found, events),
        }
    }

    /// Starts the remote operation of `op`, which is `Gating`: it is `Executing` until its report,
    /// or `Failed` when it cannot start.
    fn start_remote_op(&mut self, op: &mut Op, phases: Option<Phases>, events: &mut Vec<Event>) {
        match self.start_remote(op.id, &op.command.actor, &op.command.kind, phases) {
            Ok(()) => self.advance(op, OperationState::Executing, events),
            Err(message) => self.advance_with(op, OperationState::Failed, Some(message), events),
        }
    }

    /// The native gate check of a push or a restack (D212 (3)). Returns true when it ended the
    /// operation: a block vetoes it with the reasons and sends `GatesBlocked`, and a check that
    /// cannot run fails closed. Nothing is started and no gate state is kept.
    fn gate_stops(&mut self, op: &mut Op, events: &mut Vec<Event>) -> bool {
        let command = op.command;
        match self.blocking_gates(&command.actor, &command.kind) {
            Ok(None) => false,
            Ok(Some(blocked)) => {
                let text = block_text(&blocked.block);
                self.advance_with(op, OperationState::Vetoed, Some(text), events);
                events.push(gates_blocked_event(op.id, blocked));
                true
            }
            Err(text) => {
                self.advance_with(op, OperationState::Vetoed, Some(text), events);
                true
            }
        }
    }

    /// `BypassGates` (D212 (4)): a human runs `then`, a push or a restack, past the native gate
    /// check, having seen the gates in `gates` block it. A block that now holds a gate the person
    /// did not name is refused, and the current block is sent for the window. The command is
    /// logged whole, so the log keeps the gates and the reason (D28). The command then runs under
    /// the same operation, with the native check off and its phases as usual.
    fn bypass_gates(
        &mut self,
        op: &mut Op,
        gates: &[String],
        reason: &str,
        then: &CommandKind,
        events: &mut Vec<Event>,
    ) {
        match self.prepare_bypass(&op.command.actor, gates, reason, then) {
            Err(message) => {
                self.advance(op, OperationState::Gating, events);
                self.advance_with(op, OperationState::Failed, Some(message), events);
            }
            Ok(Bypass::Stale { message, blocked }) => {
                self.advance(op, OperationState::Gating, events);
                self.advance_with(op, OperationState::Failed, Some(message), events);
                events.push(gates_blocked_event(op.id, blocked));
            }
            Ok(Bypass::Run { detail, audit }) => {
                // The audit row is written first: a bypass that cannot be audited does not run
                // (D212 (4), R2h).
                if let Err(reason) = self.write_bypass_audit(audit) {
                    self.advance(op, OperationState::Gating, events);
                    let message =
                        format!("the bypass is refused, its audit row was not written: {reason}");
                    self.advance_with(op, OperationState::Failed, Some(message), events);
                    return;
                }
                self.advance_with(op, OperationState::Gating, Some(detail), events);
                let inner = Command {
                    actor: op.command.actor.clone(),
                    kind: then.clone(),
                };
                self.supersede_conflict_hunks(&inner.kind);
                let mut inner_op = Op {
                    id: op.id,
                    command: &inner,
                    state: OperationState::Gating,
                };
                self.run_command(&mut inner_op, GateCheck::Bypassed, events);
            }
        }
    }

    /// Checks a bypass: the person, the reason, the command, the gates named, and whether the
    /// block still holds only gates the person saw. Returns what to do, or the refusal.
    fn prepare_bypass(
        &mut self,
        actor: &Actor,
        gates: &[String],
        reason: &str,
        then: &CommandKind,
    ) -> Result<Bypass, String> {
        only_a_human(actor, "bypass gates")?;
        let reason = reason.trim();
        let length = reason.chars().count();
        if !(1..=BYPASS_REASON_LIMIT).contains(&length) {
            return Err(format!(
                "a bypass needs a reason of 1 to {BYPASS_REASON_LIMIT} characters"
            ));
        }
        if !matches!(
            then,
            CommandKind::PushWithLease | CommandKind::ApplyRestack { .. }
        ) {
            return Err("only a push or a restack can be bypassed".to_owned());
        }
        check_bypass_gates(gates)?;
        let (operation, branch, head) = self.bypass_subject(then)?;
        let audit = BypassAudit {
            branch,
            head,
            operation,
            gates: gates.to_vec(),
            reason: reason.to_owned(),
        };
        let Some(blocked) = self.blocking_gates(actor, then)? else {
            return Ok(Bypass::Run {
                detail: format!("no gate was blocking; reason: {reason}"),
                audit,
            });
        };
        let unseen: Vec<String> = block_ids(&blocked.block)
            .into_iter()
            .filter(|id| !gates.contains(id))
            .collect();
        if !unseen.is_empty() {
            return Ok(Bypass::Stale {
                message: format!(
                    "the gates changed since you looked: {} now block; look again",
                    unseen.join(", ")
                ),
                blocked,
            });
        }
        Ok(Bypass::Run {
            detail: format!(
                "bypassed gates: {} (errors: {}); reason: {reason}",
                gates.join(", "),
                blocked.block.errors.len()
            ),
            audit,
        })
    }

    /// The operation, branch and tip a bypass of `then` is about: a restack of its root branch, or
    /// a push of the branch HEAD is on. A detached HEAD has no branch to audit, so it is refused.
    fn bypass_subject(&self, then: &CommandKind) -> Result<(GateOp, String, String), String> {
        let root = self.open_root()?;
        let (operation, branch) = match then {
            CommandKind::ApplyRestack { branch, .. } => (GateOp::Restack, branch.clone()),
            _ => match self.current(&root)?.0 {
                Some(branch) => (GateOp::Push, branch),
                None => {
                    return Err(
                        "HEAD is detached; check out the branch whose push to bypass".to_owned(),
                    );
                }
            },
        };
        let head = self
            .git
            .branch_tip(&root, &branch)
            .map_err(|e| e.0)?
            .ok_or_else(|| format!("there is no branch named {branch}"))?;
        Ok((operation, branch, head))
    }

    /// Writes the audit row of a bypass (D212 (4), R2h). A failure refuses the bypass.
    fn write_bypass_audit(&mut self, audit: BypassAudit) -> Result<(), String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        store
            .record_bypass(&NewBypass {
                branch: audit.branch,
                head: audit.head,
                operation: audit.operation,
                gates: audit.gates,
                reason: audit.reason,
                actor: "human".to_owned(),
                at: now_ms(),
            })
            .map(|_| ())
            .map_err(|e| e.0)
    }

    /// Lists the bypasses, newest first (D212 (4)). A read: never recorded (D28).
    fn list_bypasses(
        &mut self,
        id: OperationId,
        branch: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let limit = match limit {
            0 => BYPASS_LIST_DEFAULT,
            n => n.min(BYPASS_LIST_MAX),
        };
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let bypasses = store
            .bypasses(branch, usize::try_from(limit).unwrap_or(usize::MAX))
            .map_err(|e| e.0)?;
        Ok(vec![Event::BypassesListed { id, bypasses }])
    }

    /// The gates of a push or a restack that block it, for a human (D212 (3)). `None` for any
    /// other command or actor, when no gate file exists, when HEAD is detached and when no
    /// repository is open: the command then reports its own refusal. A check that cannot run is
    /// an error, which the caller turns into a veto.
    fn blocking_gates(
        &mut self,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Option<BlockedGates>, String> {
        let operation = match (actor, kind) {
            (Actor::Human, CommandKind::PushWithLease) => GateOp::Push,
            (Actor::Human, CommandKind::ApplyRestack { .. }) => GateOp::Restack,
            _ => return Ok(None),
        };
        let Ok(root) = self.open_root() else {
            return Ok(None);
        };
        // Without a gate file the branch is not read, so a user without gates pays nothing.
        let loaded = self
            .gate_files(&root)
            .map_err(|reason| unchecked(&reason))?;
        if loaded.gates.is_empty() && loaded.errors.is_empty() {
            return Ok(None);
        }
        let branch = match kind {
            CommandKind::ApplyRestack { branch, .. } => branch.clone(),
            _ => match self.current(&root).map_err(|reason| unchecked(&reason))?.0 {
                Some(branch) => branch,
                None => return Ok(None),
            },
        };
        let block = self
            .gate_check(operation, &branch)
            .map_err(|reason| unchecked(&reason))?;
        Ok(block.map(|block| BlockedGates {
            operation,
            branch,
            block,
        }))
    }

    fn execute(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::OpenRepo { path } => self.open_repo(id, path),
            CommandKind::RefreshStatus => self.refresh_status(id),
            CommandKind::ListWorktrees => self.list_worktrees(id),
            CommandKind::LoadDiff { path, side } => {
                let root = self.open_root()?;
                let diff = self.git.diff(&root, path, *side).map_err(|e| e.0)?;
                Ok(vec![Event::DiffLoaded {
                    id,
                    side: *side,
                    diff,
                }])
            }
            CommandKind::Stage { target } => {
                let root = self.open_root()?;
                self.git.stage(&root, target).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::Unstage { target } => {
                let root = self.open_root()?;
                self.git.unstage(&root, target).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::DiscardHunk { path, hunk } => {
                let root = self.open_root()?;
                self.git.discard_hunk(&root, path, hunk).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::DiscardFile { .. } => self.discard_file(id, actor, kind),
            CommandKind::Commit { message } => {
                if message.trim().is_empty() {
                    return Err("the commit message is empty".to_owned());
                }
                let root = self.open_root()?;
                let oid = self.git.commit(&root, message).map_err(|e| e.0)?;
                self.committed(id, &root, oid)
            }
            CommandKind::RecordCombination => self.record_combination(id, actor),
            CommandKind::ListBranches => self.list_branches(id),
            CommandKind::ListRepoBranches { repo } => self.list_repo_branches(id, repo),
            CommandKind::IndexFiles => {
                self.start_index(id)?;
                Ok(Vec::new())
            }
            CommandKind::ListDirectory { dir } => self.list_directory(id, dir),
            CommandKind::SearchFiles { .. }
            | CommandKind::ReadFile { .. }
            | CommandKind::SearchText { .. } => Err("file reads are started by handle".to_owned()),
            CommandKind::LoadHistory { .. } | CommandKind::LoadHistoryPage { .. } => {
                Err("history is started by handle".to_owned())
            }
            CommandKind::LoadCommitDetail { .. } | CommandKind::LoadCommitStack { .. } => {
                Err("commit reads are started by handle".to_owned())
            }
            CommandKind::Fetch | CommandKind::Pull | CommandKind::PushWithLease => {
                Err("remote operations are started by handle".to_owned())
            }
            CommandKind::CreateWorktree { .. } => self.add_worktree(id, actor, kind),
            CommandKind::Checkout { branch } => {
                let root = self.open_root()?;
                self.git.checkout(&root, branch).map_err(|e| e.0)?;
                self.reload_status(id, &root)
            }
            CommandKind::ListHookSections => {
                let root = self.open_root()?;
                let sections = hook_convert::section_entries(self.git.as_ref(), &root)?;
                Ok(vec![Event::HookSectionsListed { id, sections }])
            }
            CommandKind::ConvertHooksToWorkflows => self.convert_hooks(id, actor),
            CommandKind::ApplyReparent { .. } => {
                Err("a reparent is applied by a workflow run".to_owned())
            }
            _ if is_handled_run(kind) => Err("handle starts this command, not execute".to_owned()),
            CommandKind::StageAll => self.stage_all(id),
            CommandKind::CancelOperation { op } => self.cancel_operation(actor, *op),
            _ if is_conflict_command(kind) => self.conflict_command(id, actor, kind),
            CommandKind::ListWorkflows
            | CommandKind::RunWorkflow { .. }
            | CommandKind::ReviewWorkflow { .. }
            | CommandKind::RevokeWorkflowReview { .. }
            | CommandKind::DismissWorkflowRequest { .. }
            | CommandKind::ApproveWorkflowStep { .. }
            | CommandKind::CancelWorkflowRun { .. }
            | CommandKind::CreateWorkflowFromTemplate { .. }
            | CommandKind::SetWorkflowEnabled { .. }
            | CommandKind::CopyBuiltinWorkflow { .. }
            | CommandKind::ListWorkflowRuns { .. }
            | CommandKind::LoadWorkflowRun { .. }
            | CommandKind::DryRunWorkflow { .. }
            | CommandKind::CheckWorkflowScript { .. }
            | CommandKind::SaveWorkflowFile { .. }
            | CommandKind::DeleteWorkflowFile { .. } => self.workflow_command(id, actor, kind),
            _ if threads_session::is_threads_command(kind) => self.threads_command(id, actor, kind),
            _ if contexts_session::is_contexts_command(kind) => {
                self.contexts_command(id, actor, kind)
            }
            _ if is_review_command(kind) => self.review_command(id, actor, kind),
            _ if is_workspace_command(kind) => self.workspace_command(id, actor, kind),
            // The worktree and branch commands that `execute` does not list above.
            _ => self.manage_worktree(id, actor, kind),
        }
    }

    /// Branch name and HEAD commit, both `None` when they do not apply.
    fn current(&self, root: &std::path::Path) -> Result<(Option<String>, Option<String>), String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        Ok((status.branch.head, status.branch.oid))
    }

    /// Whether HEAD or the staged diff of `root` moved since `fingerprint` was taken. When the
    /// repository cannot be read now, it counts as changed.
    fn moved_since(&self, root: &Path, fingerprint: &Fingerprint) -> bool {
        let Ok((_, head)) = self.current(root) else {
            return true;
        };
        let Ok(staged) = self.git.staged_diff(root) else {
            return true;
        };
        transforms::fingerprint(head.as_deref(), &staged) != *fingerprint
    }

    /// Starts a page request. It continues the walk when the walk stands right
    /// after `after` (the list's last commit); otherwise it starts a walk from
    /// HEAD and reads past `after`, which is keyset resume and not `--skip`.
    /// A request in flight is superseded: its walk is stopped and it is
    /// cancelled. Returns the events of that cancellation.
    fn start_history(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        after: Option<String>,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let continues = match (&self.history, &after) {
            (Some(session), Some(after)) => {
                session.active.is_none()
                    && session.stream.is_some()
                    && session.last.as_deref() == Some(after.as_str())
            }
            _ => false,
        };
        let active = ActiveHistory {
            id,
            actor: actor.clone(),
            kind: kind.clone(),
        };
        if continues {
            let session = self.history.as_mut().expect("a session continues");
            let stream = session.stream.take();
            session.active = Some(active);
            self.history_jobs.push(HistoryJob {
                id,
                stream,
                after: None,
            });
            return Ok(Vec::new());
        }
        // A page that does not continue the walk starts a new one of HEAD; a
        // `LoadHistory` starts the walk of its scope (D139).
        let revs = match kind {
            CommandKind::LoadHistory { scope } => {
                let (branch, _) = self.current(&root)?;
                stack_session::history_revs(self.git.as_ref(), &root, scope, branch.as_deref())?
            }
            _ => HistoryRevs::Head,
        };
        // The new walk starts before the old one is stopped, so a failure keeps the old one.
        let stream = self.git.history_stream(&root, &revs).map_err(|e| e.0)?;
        let stop: Canceller = match &stream {
            Some(stream) => stream.canceller(),
            None => std::sync::Arc::new(|| {}),
        };
        let superseded = self.drop_history();
        self.history = Some(HistorySession {
            stream: None,
            stop,
            last: None,
            active: Some(active),
        });
        self.history_jobs.push(HistoryJob { id, stream, after });
        Ok(superseded)
    }

    /// Refuses a remote operation that cannot start now: only a person starts one, and one runs at
    /// a time.
    fn remote_free(&self, actor: &Actor, kind: &CommandKind) -> Result<(), String> {
        let what = match kind {
            CommandKind::Fetch => "fetch",
            CommandKind::Pull => "pull",
            _ => "push with lease",
        };
        only_a_human(actor, what)?;
        if self.remote.is_some() {
            return Err("a remote operation is already running".to_owned());
        }
        Ok(())
    }

    /// Checks a fetch, pull or push with lease and queues its run. The run starts
    /// on a thread after this returns, so the operation is `Executing` until its
    /// report. Refused for an agent, and while another remote operation runs.
    fn start_remote(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        phases: Option<Phases>,
    ) -> Result<(), String> {
        self.remote_free(actor, kind)?;
        let root = self.open_root()?;
        let run = match kind {
            CommandKind::Fetch => self.git.fetch(&root),
            CommandKind::Pull => self.git.pull(&root),
            _ => self.git.push_with_lease(&root),
        }
        .map_err(|e| e.0)?;
        self.remote = Some(ActiveRemote {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            root,
            phases,
        });
        self.remote_jobs.push(RemoteJob { id, run });
        Ok(())
    }

    /// Starts the read of commit `oid`. A read in flight is superseded: it is
    /// stopped and cancelled. Returns the events of that cancellation.
    fn start_detail(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        oid: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let read = self.git.commit_detail(&root, oid).map_err(|e| e.0)?;
        // The read is built before the old one is stopped, so a refusal keeps the old one.
        // A file diff of the commit list before this one is no longer wanted either.
        let mut superseded = self.drop_detail();
        superseded.extend(self.drop_stack());
        let stop = Arc::new(AtomicBool::new(false));
        self.detail = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.detail_jobs.push(DetailJob { id, read, stop });
        Ok(superseded)
    }

    /// Starts the diffs of the files of commit `oid` (H4c). A read in flight is
    /// superseded. Returns the events of that cancellation.
    fn start_stack(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        oid: &str,
        files: &[CommitFile],
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let read = self.git.commit_stack(&root, oid, files).map_err(|e| e.0)?;
        let superseded = self.drop_stack();
        let stop = Arc::new(AtomicBool::new(false));
        self.stack = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.stack_jobs.push(StackJob { id, read, stop });
        Ok(superseded)
    }

    /// Ends the stack read: it is stopped, and a request still in flight is cancelled.
    fn drop_stack(&mut self) -> Vec<Event> {
        let Some(active) = self.stack.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// The file list of the open repository, started afresh when `root` is not
    /// the repository it was made for.
    fn files_session(&mut self, root: &Path) -> &mut FilesSession {
        if !matches!(&self.files, Some(session) if session.root == root) {
            self.drop_files();
        }
        self.files
            .get_or_insert_with(|| FilesSession::new(root.to_path_buf()))
    }

    /// Reads the file list, unless a read is already in flight. `id` is the
    /// command that asked; `FilesIndexed` carries it.
    fn start_index(&mut self, id: OperationId) -> Result<(), String> {
        let root = self.open_root()?;
        if self.files_session(&root).building.is_some() {
            return Ok(());
        }
        let read = self.git.file_list(&root).map_err(|e| e.0)?;
        let stop = Arc::new(AtomicBool::new(false));
        let session = self.files_session(&root);
        let token = session.next_token();
        session.building = Some(Building {
            token,
            id,
            stop: Arc::clone(&stop),
        });
        self.index_jobs.push(IndexJob { token, read, stop });
        Ok(())
    }

    /// The first read of the list, when none was made and none is in flight.
    fn ensure_index(&mut self, id: OperationId) -> Result<(), String> {
        let root = self.open_root()?;
        let session = self.files_session(&root);
        if session.index.is_none() && session.building.is_none() {
            self.start_index(id)?;
        }
        Ok(())
    }

    /// One folder of the file tree, from the list in memory. With no list yet
    /// the listing is empty and says it is being read.
    fn list_directory(&mut self, id: OperationId, dir: &str) -> Result<Vec<Event>, String> {
        self.ensure_index(id)?;
        let root = self.open_root()?;
        let session = self.files_session(&root);
        let indexing = session.indexing();
        let listing = match &session.index {
            Some(index) => index.list_dir(dir, indexing),
            None => DirListing {
                dir: dir.trim_matches('/').to_owned(),
                indexing,
                ..DirListing::default()
            },
        };
        Ok(vec![Event::DirectoryListed { id, listing }])
    }

    /// Starts a search of the file list. A search in flight is superseded:
    /// stopped and cancelled. With no list yet the search waits for the first
    /// read. Returns the events of the cancellation.
    fn start_files_search(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        query: &str,
    ) -> Result<Vec<Event>, String> {
        self.ensure_index(id)?;
        let root = self.open_root()?;
        let session = self.files_session(&root);
        let index = session.index.clone();
        let indexing = session.indexing();
        let superseded = self.drop_files_search();
        let stop = Arc::new(AtomicBool::new(false));
        self.files_search = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        match index {
            Some(index) => self.search_jobs.push(SearchJob {
                id,
                index,
                query: query.to_owned(),
                indexing,
                stop,
            }),
            None => self.files_waiting = Some(query.to_owned()),
        }
        Ok(superseded)
    }

    /// Starts the read of file `path` for the viewer. A read in flight is
    /// superseded. Returns the events of the cancellation.
    fn start_file_read(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        path: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let superseded = self.drop_files_read();
        let stop = Arc::new(AtomicBool::new(false));
        self.files_read = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.read_jobs.push(ReadJob {
            id,
            root,
            path: path.to_owned(),
            stop,
        });
        Ok(superseded)
    }

    /// Starts a text search of the worktree. A search in flight is superseded:
    /// stopped and cancelled. Returns the events of the cancellation. A text
    /// Git cannot take is refused here, before anything is cancelled.
    fn start_text_search(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        query: &str,
        options: GrepOptions,
        paths: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let request = GrepRequest::new(query, options, paths)?;
        let read = self.git.grep_text(&root, &request).map_err(|e| e.0)?;
        let superseded = self.drop_text_search();
        let stop = Arc::new(AtomicBool::new(false));
        self.text_search = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.text_jobs.push(TextJob { id, read, stop });
        Ok(superseded)
    }

    /// Starts the blame of a provenance read (D144). A read in flight is superseded: it is
    /// stopped and cancelled. A request the rules refuse is refused before anything is
    /// cancelled. Returns the events of the cancellation.
    fn start_provenance(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
        path: &str,
        from: u32,
        to: u32,
    ) -> Result<Vec<Event>, String> {
        threads_session::check_provenance(path, from, to)?;
        let root = self.open_root()?;
        // In a member, blame runs in the member's checkout with the path inside it (D181).
        let read = match member_named(&root, path) {
            Some(member) => {
                let inside = members::in_member(path, &member).unwrap_or(path);
                self.git.blame_lines(&root.join(&member), inside, from, to)
            }
            None => self.git.blame_lines(&root, path, from, to),
        }
        .map_err(|e| e.0)?;
        let superseded = self.drop_provenance();
        let stop = Arc::new(AtomicBool::new(false));
        self.provenance = Some(ActiveDetail {
            op: ActiveHistory {
                id,
                actor: actor.clone(),
                kind: kind.clone(),
            },
            stop: Arc::clone(&stop),
        });
        self.provenance_jobs.push(ProvenanceJob { id, read, stop });
        Ok(superseded)
    }

    /// Ends the provenance read: it is stopped, and a request in flight is cancelled.
    fn drop_provenance(&mut self) -> Vec<Event> {
        let Some(active) = self.provenance.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![Self::operation_changed(
            &active.op,
            OperationState::Cancelled,
            None,
        )]
    }

    /// Answers a provenance read from the lines blame gave (D144): the threads, read from the
    /// store of the open repository.
    fn provenance_answer(
        &mut self,
        path: &str,
        from: u32,
        to: u32,
        lines: &[BlameLine],
    ) -> Result<ProvenanceResult, String> {
        let root = self.open_root()?;
        let member = member_named(&root, path);
        // Blame reports member-relative paths; the threads and the answer use the workspace's.
        let mut lines = lines.to_vec();
        if let Some(member) = &member {
            for line in &mut lines {
                line.path = format!("{member}/{}", line.path);
            }
        }
        let store = threads_session::store(&mut self.threads, self.git.as_ref(), &root)?;
        let ctx = threads_session::Context {
            git: self.git.as_ref(),
            root: &root,
            global_config: None,
            contexts_db: None,
        };
        threads_session::provenance_from_lines(
            store,
            &ctx,
            path,
            from,
            to,
            &lines,
            member.as_deref(),
        )
    }

    /// Ends the text search: it is stopped, and a request in flight is cancelled.
    fn drop_text_search(&mut self) -> Vec<Event> {
        let Some(active) = self.text_search.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the file search: it is stopped, and a request in flight is cancelled.
    fn drop_files_search(&mut self) -> Vec<Event> {
        self.files_waiting = None;
        let Some(active) = self.files_search.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the file read: it is stopped, and a request in flight is cancelled.
    fn drop_files_read(&mut self) -> Vec<Event> {
        self.stop_highlight();
        let Some(active) = self.files_read.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Forgets the file list and ends everything that reads it or the files.
    fn drop_files(&mut self) -> Vec<Event> {
        if let Some(building) = self.files.take().and_then(|session| session.building) {
            building.stop.store(true, Ordering::SeqCst);
        }
        let mut events = self.drop_files_search();
        events.extend(self.drop_files_read());
        events.extend(self.drop_text_search());
        events
    }

    /// Ends the commit detail read: it is stopped, and a request still in flight
    /// is cancelled.
    fn drop_detail(&mut self) -> Vec<Event> {
        let Some(active) = self.detail.take() else {
            return Vec::new();
        };
        active.stop.store(true, Ordering::SeqCst);
        vec![self.finish_op(&active.op, OperationState::Cancelled, None)]
    }

    /// Ends the history walk: its git process is stopped, and a page request
    /// still in flight is cancelled.
    fn drop_history(&mut self) -> Vec<Event> {
        let Some(session) = self.history.take() else {
            return Vec::new();
        };
        (session.stop)();
        match session.active {
            Some(active) => vec![self.finish_op(&active, OperationState::Cancelled, None)],
            None => Vec::new(),
        }
    }

    /// Records the end of a page request, as the log records every operation.
    fn finish_op(
        &mut self,
        active: &ActiveHistory,
        state: OperationState,
        detail: Option<String>,
    ) -> Event {
        self.log.push(LogEntry {
            id: active.id,
            actor: active.actor.clone(),
            kind: active.kind.clone(),
            state,
        });
        Self::operation_changed(active, state, detail)
    }

    /// The event that ends an operation, without a log entry. A read ends this way: reads are
    /// not in the command log (D28, D144).
    fn operation_changed(
        active: &ActiveHistory,
        state: OperationState,
        detail: Option<String>,
    ) -> Event {
        Event::OperationChanged {
            id: active.id,
            actor: active.actor.clone(),
            state,
            detail,
        }
    }

    /// The branch, HEAD and stash commands: create and delete a branch, check out a
    /// commit, read the reflog (B3) and the stashes (B2). Human only where a command
    /// writes.
    fn branch_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::CreateBranch {
                name,
                start_point,
                checkout,
            } => self.create_branch(id, actor, name, start_point.as_deref(), *checkout),
            CommandKind::DeleteBranch { name, force } => {
                self.delete_branch(id, actor, name, *force)
            }
            CommandKind::ListMergedBranches => self.list_merged(id),
            CommandKind::PlanRestack { branch, skip } => self.plan_restack(id, branch, skip),
            CommandKind::ApplyRestack { .. }
            | CommandKind::UndoRestack
            | CommandKind::ResumeRestack => self.restack_command(id, actor, kind),
            CommandKind::StartRestackResolution { .. }
            | CommandKind::FinishRestackResolution
            | CommandKind::CancelRestackResolution => {
                self.restack_resolution_command(id, actor, kind)
            }
            CommandKind::LoadStacks
            | CommandKind::CreateChildBranch { .. }
            | CommandKind::SetStackParent { .. }
            | CommandKind::ClearStackParent { .. }
            | CommandKind::ReparentToGrandparent { .. } => self.stack_command(id, actor, kind),
            CommandKind::DeleteMergedBranches { branches } => {
                self.delete_merged(id, actor, branches)
            }
            CommandKind::CheckoutCommit { oid } => self.checkout_commit(id, actor, oid),
            CommandKind::LoadReflog { skip } => self.load_reflog(id, *skip),
            CommandKind::LoadSubmodule { path } => self.load_submodule(id, path),
            CommandKind::ListMembers => self.list_members(id),
            CommandKind::ListStashes => self.list_stashes(id),
            CommandKind::CreateStash { .. }
            | CommandKind::ApplyStash { .. }
            | CommandKind::PopStash { .. }
            | CommandKind::DropStash { .. } => self.stash_command(id, actor, kind),
            _ => Err("not a worktree or branch command".to_owned()),
        }
    }

    /// A new local branch, checked out or not. The status, the worktrees and the
    /// branch list follow, as after a checkout. Human only.
    fn create_branch(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        start_point: Option<&str>,
        checkout: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "create a branch")?;
        let root = self.open_root()?;
        if let Some(problem) = branch::name_problem(name) {
            return Err(problem.to_owned());
        }
        if let Some(problem) = start_point.and_then(branch::start_point_problem) {
            return Err(problem.to_owned());
        }
        self.git
            .create_branch(&root, name, start_point, checkout)
            .map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    /// The stack commands (D68, D138): the read of every stack, and the writes of
    /// the stack metadata, which are human only. A write answers with a fresh
    /// `StacksLoaded`; a new child also brings the status and the branch list.
    fn stack_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let git = self.git.as_ref();
        let mut events = Vec::new();
        match kind {
            CommandKind::LoadStacks => {}
            CommandKind::CreateChildBranch {
                name,
                parent,
                checkout,
            } => {
                only_a_human(actor, "create a branch")?;
                if let Some(problem) = branch::name_problem(name) {
                    return Err(problem.to_owned());
                }
                let tip = stack_session::child_start(git, &root, parent)?;
                git.create_branch(&root, name, Some(&tip), *checkout)
                    .map_err(|e| e.0)?;
                git.write_stack_link(&root, name, Some((parent, &tip)))
                    .map_err(|e| e.0)?;
                events = self.branches_changed(id, &root)?;
            }
            CommandKind::SetStackParent { branch, parent } => {
                only_a_human(actor, "set a stack parent")?;
                stack_session::set_parent(git, &root, branch, parent)?;
            }
            CommandKind::ClearStackParent { branch } => {
                only_a_human(actor, "remove a branch from its stack")?;
                stack_session::clear_parent(git, &root, branch)?;
            }
            CommandKind::ReparentToGrandparent { branch } => {
                only_a_human(actor, "reparent a branch")?;
                stack_session::reparent_to_grandparent(git, &root, branch)?;
            }
            _ => return Err("not a stack command".to_owned()),
        }
        let stacks = stack_session::snapshot(git, &root)?;
        events.extend(self.stacks_seen(id, &root, &stacks, true));
        events.push(Event::StacksLoaded { id, stacks });
        Ok(events)
    }

    /// Plans a restack of `branch` and the branches under it (D142): reads the
    /// stacks and replays each commit without moving a ref. A read.
    fn plan_restack(
        &self,
        id: OperationId,
        branch: &str,
        skip: &[String],
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let plan = restack_session::plan(self.git.as_ref(), &root, branch, skip, &|path| {
            same_folder(&root, path)
        })?;
        Ok(vec![Event::RestackPlanned { id, plan }])
    }

    /// Applies a plan, or undoes the last restack (D142). Human only. The status,
    /// the branches and the stacks follow.
    fn restack_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "restack")?;
        let root = self.open_root()?;
        let git = self.git.as_ref();
        // An undo is a baseline for the stack events: the branches it puts back fire nothing, so an
        // automatic restack does not undo the undo (D162(2)).
        let fire = !matches!(kind, CommandKind::UndoRestack);
        let record = match kind {
            CommandKind::ApplyRestack {
                branch,
                plan_id,
                skip,
            } => {
                let (record, remaps) =
                    restack_session::apply(git, &root, branch, plan_id, skip, &|path| {
                        same_folder(&root, path)
                    })?;
                // Commit links to the replaced commits resolve from now on (D145).
                threads_session::remap(&mut self.threads, git, &root, &remaps);
                Some(record)
            }
            CommandKind::UndoRestack => {
                restack_session::undo(git, &root)?;
                None
            }
            CommandKind::ResumeRestack => {
                // A resolution worktree left by a crash, or a session of an earlier
                // repository, is dropped (D154).
                self.git.restack_resolution_close(&root).map_err(|e| e.0)?;
                self.restack_resolution = None;
                restack_session::resume(git, &root)?
            }
            _ => return Err("not a restack command".to_owned()),
        };
        let mut events = self.branches_changed(id, &root)?;
        events.push(Event::RestackFinished { id, record });
        let stacks = stack_session::snapshot(git, &root)?;
        events.extend(self.stacks_seen(id, &root, &stacks, fire));
        events.push(Event::StacksLoaded { id, stacks });
        Ok(events)
    }

    /// Starts, finishes or cancels the resolution of a restack conflict (D154). Human only.
    fn restack_resolution_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "resolve a restack conflict")?;
        match kind {
            CommandKind::StartRestackResolution {
                branch,
                plan_id,
                skip,
                conflicted,
            } => self.start_restack_resolution(id, branch, plan_id, skip, conflicted),
            CommandKind::FinishRestackResolution => self.finish_restack_resolution(id),
            CommandKind::CancelRestackResolution => self.cancel_restack_resolution(id),
            _ => Err("not a restack resolution command".to_owned()),
        }
    }

    /// Opens the resolution worktree on the conflict of `conflicted` in the plan `branch`
    /// makes, if the plan is still `plan_id` (D154). Starting another closes the one open.
    fn start_restack_resolution(
        &mut self,
        id: OperationId,
        branch: &str,
        plan_id: &str,
        skip: &[String],
        conflicted: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let git = self.git.as_ref();
        let plan =
            restack_session::plan(git, &root, branch, skip, &|path| same_folder(&root, path))?;
        if plan.plan_id != plan_id {
            return Err("The stack changed since the plan; plan again.".to_owned());
        }
        let not_conflicted = || format!("{conflicted} has no conflict in the plan.");
        let planned = plan
            .branches
            .iter()
            .find(|entry| entry.name == conflicted)
            .ok_or_else(not_conflicted)?;
        let Some(RestackBlock::Conflict { commit, onto }) = &planned.block else {
            return Err(not_conflicted());
        };
        let subject = planned
            .commits
            .iter()
            .find(|entry| &entry.oid == commit)
            .map(|entry| entry.subject.clone())
            .unwrap_or_default();
        let parent = planned.parent.clone();
        self.restack_resolution = None;
        let worktree = self
            .git
            .restack_resolution_open(&root, onto, commit)
            .map_err(|e| e.0)?;
        let paths = self
            .git
            .restack_resolution_unmerged(&worktree)
            .map_err(|e| e.0)?;
        let resolution = RestackResolution {
            from: branch.to_owned(),
            conflicted: conflicted.to_owned(),
            parent: parent.clone(),
            commit: commit.clone(),
            subject,
            onto: onto.clone(),
            paths,
        };
        self.restack_resolution = Some(ResolutionSession {
            from: branch.to_owned(),
            skip: skip.to_vec(),
            conflicted: conflicted.to_owned(),
            parent,
            commit: commit.clone(),
            onto: onto.clone(),
            worktree,
        });
        Ok(vec![Event::RestackResolutionStarted { id, resolution }])
    }

    /// Records the resolved commit of the open conflict, closes its worktree and plans the
    /// restack again (D154). Refused while a path is unmerged; the session stays then.
    fn finish_restack_resolution(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let session = self.open_resolution()?;
        let resolved = self
            .git
            .restack_resolution_commit(&root, &session.worktree, &session.onto, &session.commit)
            .map_err(|e| e.0)?;
        let resolution = Resolution {
            onto: session.onto.clone(),
            commit: session.commit.clone(),
            resolved: resolved.clone(),
        };
        let from = session.from.clone();
        let skip = session.skip.clone();
        let wake = Self::resumed_payload(session);
        let git = self.git.as_ref();
        restack_resolutions::add(git, &root, resolution)?;
        self.git.restack_resolution_close(&root).map_err(|e| e.0)?;
        self.restack_resolution = None;
        // The resolution is recorded, so the runs waiting on `restack.resumed` go on (WF3f,
        // D162(1)), whether or not the plan that follows can be made.
        if let Some(runtime) = self.script_runtime() {
            runtime.deliver_with("restack.resumed", "resumed", &wake);
        }
        let plan =
            restack_session::plan(git, &root, &from, &skip, &|path| same_folder(&root, path))?;
        Ok(vec![
            Event::RestackResolutionFinished { id, resolved },
            Event::RestackPlanned { id, plan },
        ])
    }

    /// Closes the resolution worktree and forgets the open conflict. Succeeds when none is
    /// open; a resolution already finished stays in the store (D154).
    fn cancel_restack_resolution(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        self.git.restack_resolution_close(&root).map_err(|e| e.0)?;
        let wake = self
            .restack_resolution
            .take()
            .map_or_else(|| json!({}), |session| Self::resumed_payload(&session));
        // The runs waiting on `restack.resumed` end with `aborted` (WF3f, D162(1)).
        if let Some(runtime) = self.script_runtime() {
            runtime.deliver_with("restack.resumed", "aborted", &wake);
        }
        Ok(vec![Event::RestackResolutionCancelled { id }])
    }

    /// The fields `restack.resumed` carries: the branch whose commit conflicted and the commit the
    /// resolution replays (D162(1)).
    fn resumed_payload(session: &ResolutionSession) -> serde_json::Value {
        json!({ "branch": session.conflicted, "commit": session.commit })
    }

    /// The open restack conflict, or the refusal when none is being resolved.
    fn open_resolution(&self) -> Result<&ResolutionSession, String> {
        self.restack_resolution
            .as_ref()
            .ok_or_else(|| "No restack conflict is being resolved.".to_owned())
    }

    /// Deletes a local branch, safely (`-d`) unless `force` (`-D`). A branch that a
    /// worktree has checked out is refused here, naming the worktree. Human only.
    fn delete_branch(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        force: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete a branch")?;
        let root = self.open_root()?;
        if let Some(problem) = branch::name_problem(name) {
            return Err(problem.to_owned());
        }
        let status = self.git.status(&root).map_err(|e| e.0)?;
        let checked_out_at = if status.branch.head.as_deref() == Some(name) {
            Some((root.to_string_lossy().into_owned(), true))
        } else {
            let worktrees = self.git.worktrees(&root).map_err(|e| e.0)?;
            worktrees
                .iter()
                .find(|entry| entry.branch.as_deref() == Some(name))
                .map(|entry| (entry.path.clone(), same_folder(&root, &entry.path)))
        };
        if let Some(refusal) = branch::delete_refusal(
            name,
            checked_out_at
                .as_ref()
                .map(|(path, open)| (path.as_str(), *open)),
        ) {
            return Err(refusal);
        }
        self.git
            .delete_branch(&root, name, force)
            .map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    /// The merged-branch list: the base the check used and the branches Git
    /// would delete. Reads only.
    fn list_merged(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let plan = self.merged_plan(&root)?;
        Ok(vec![Event::MergedBranchesListed {
            id,
            base: plan.base,
            branches: plan.branches,
        }])
    }

    /// Deletes the branches the human confirmed, with `git branch -d`, if the
    /// merged list still holds exactly those. Each branch is tried on its own: a
    /// refusal is reported and the others still go. Human only.
    fn delete_merged(
        &self,
        id: OperationId,
        actor: &Actor,
        names: &[String],
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete merged branches")?;
        let root = self.open_root()?;
        let mut now: Vec<String> = self
            .merged_plan(&root)?
            .branches
            .into_iter()
            .map(|branch| branch.name)
            .collect();
        let mut asked = names.to_vec();
        now.sort();
        asked.sort();
        if now != asked {
            return Err(MERGED_LIST_CHANGED.to_owned());
        }
        let mut deleted = Vec::new();
        let mut failures = Vec::new();
        for name in names {
            match self.git.delete_branch(&root, name, false) {
                Ok(()) => deleted.push(name.clone()),
                Err(e) => failures.push(BranchFailure {
                    name: name.clone(),
                    message: e.0,
                }),
            }
        }
        let mut events = self.branches_changed(id, &root)?;
        events.push(Event::MergedBranchesDeleted {
            id,
            deleted,
            failures,
        });
        Ok(events)
    }

    /// The base and the branches merged into it that may be deleted now. Git
    /// lists local branches only; the rules are `branch::merge_base` and
    /// `branch::is_merged_candidate`.
    fn merged_plan(&self, root: &Path) -> Result<MergedPlan, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        let current = status.branch.head.clone();
        let locals: Vec<String> = self
            .git
            .branches(root)
            .map_err(|e| e.0)?
            .into_iter()
            .filter(|entry| !entry.is_remote)
            .map(|entry| entry.name)
            .collect();
        let local_refs: Vec<&str> = locals.iter().map(String::as_str).collect();
        let origin = self
            .git
            .origin_head(root)
            .map_err(|e| e.0)?
            .as_deref()
            .and_then(branch::origin_head_branch)
            .map(str::to_owned);
        let Some(base) = branch::merge_base(origin.as_deref(), &local_refs, current.as_deref())
        else {
            return Ok(MergedPlan {
                base: None,
                branches: Vec::new(),
            });
        };
        let elsewhere: Vec<String> = self
            .git
            .worktrees(root)
            .map_err(|e| e.0)?
            .into_iter()
            .filter_map(|entry| entry.branch)
            .collect();
        let elsewhere_refs: Vec<&str> = elsewhere.iter().map(String::as_str).collect();
        let branches = self
            .git
            .merged_branches(root, &base)
            .map_err(|e| e.0)?
            .into_iter()
            .filter(|merged| local_refs.contains(&merged.name.as_str()))
            .filter(|merged| {
                branch::is_merged_candidate(
                    &merged.name,
                    &base,
                    current.as_deref(),
                    &elsewhere_refs,
                )
            })
            .collect();
        Ok(MergedPlan {
            base: Some(base),
            branches,
        })
    }

    /// The status, the worktrees and the branch list, after a branch changed.
    fn branches_changed(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        let mut events = self.loaded(id, root, status)?;
        let branches = self.git.branches(root).map_err(|e| e.0)?;
        let user_email = self.git.user_email(root).map_err(|e| e.0)?;
        events.push(Event::BranchesLoaded {
            id,
            branches,
            user_email,
        });
        Ok(events)
    }

    fn list_worktrees(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let worktrees = self.git.worktrees(&root).map_err(|e| e.0)?;
        Ok(vec![Event::WorktreesLoaded { id, worktrees }])
    }

    /// The branch list, with the configured user email so the UI can tell
    /// which branches are the user's own.
    /// One page of the reflog of HEAD (B3, D95). One entry more than a page is
    /// read, so `has_more` is known without a second run. Reads only.
    fn load_reflog(&self, id: OperationId, skip: u32) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let mut entries = self
            .git
            .reflog(&root, skip, REFLOG_PAGE + 1)
            .map_err(|e| e.0)?;
        let has_more = entries.len() > REFLOG_PAGE;
        entries.truncate(REFLOG_PAGE);
        Ok(vec![Event::ReflogLoaded {
            id,
            skip,
            entries,
            has_more,
        }])
    }

    /// The state of one submodule (D175). The path is checked before Git runs; a
    /// read, so nothing is recorded.
    fn load_submodule(&self, id: OperationId, path: &str) -> Result<Vec<Event>, String> {
        submodule::check_path(path)?;
        let root = self.open_root()?;
        let submodule = self
            .git
            .submodule(&root, path, SUBMODULE_COMMIT_LIMIT)
            .map_err(|e| e.0)?;
        Ok(vec![Event::SubmoduleLoaded { id, submodule }])
    }

    /// The checked-out submodules of the repository (D181), the workspace's members. The same test
    /// the threads capture uses. A read, so nothing is recorded.
    fn list_members(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let members = members::checked_out(&root)
            .into_iter()
            .map(|member| member.name)
            .collect();
        Ok(vec![Event::MembersListed { id, members }])
    }

    /// Detaches HEAD at a commit, as a reflog entry is checked out (B3, D95). The
    /// id is checked before Git runs. The status and the branch list follow, as
    /// after a branch change. Human only.
    fn checkout_commit(
        &self,
        id: OperationId,
        actor: &Actor,
        oid: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "check out a commit")?;
        if let Some(problem) = reflog::oid_problem(oid) {
            return Err(problem.to_owned());
        }
        let root = self.open_root()?;
        self.git.checkout_commit(&root, oid).map_err(|e| e.0)?;
        self.branches_changed(id, &root)
    }

    fn list_branches(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let branches = self.git.branches(&root).map_err(|e| e.0)?;
        let user_email = self.git.user_email(&root).map_err(|e| e.0)?;
        Ok(vec![Event::BranchesLoaded {
            id,
            branches,
            user_email,
        }])
    }

    /// The branches of the repository at `repo`, read without opening it as the window's repository
    /// (D195). A read, so nothing is recorded.
    fn list_repo_branches(&self, id: OperationId, repo: &str) -> Result<Vec<Event>, String> {
        if repo.trim().is_empty() {
            return Err("no repository was given".to_owned());
        }
        let branches = self.git.branches(Path::new(repo.trim())).map_err(|e| e.0)?;
        Ok(vec![Event::RepoBranchesLoaded {
            id,
            repo: repo.to_owned(),
            branches,
        }])
    }

    /// A new linked worktree; the list and the status follow, as after a checkout.
    fn add_worktree(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        let CommandKind::CreateWorktree { path, mode } = kind else {
            return Err("not a worktree command".to_owned());
        };
        // Worktrees have no agent path yet (D31, Q12).
        if !matches!(actor, Actor::Human) {
            return Err("only a human can create a worktree".to_owned());
        }
        let root = self.open_root()?;
        self.git
            .create_worktree(&root, path, mode)
            .map_err(|e| e.0)?;
        let status = self.git.status(&root).map_err(|e| e.0)?;
        self.loaded(id, &root, status)
    }

    /// The worktree commands after the listing: inspect, remove, lock, unlock,
    /// and the prune pair. Any other kind is passed on to `branch_command`.
    fn manage_worktree(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::InspectWorktree { path } => {
                let root = self.open_root()?;
                let inspection = self.inspect(&root, path)?;
                Ok(vec![Event::WorktreeInspected { id, inspection }])
            }
            CommandKind::PreviewMoveChanges { path, target } => {
                let root = self.open_root()?;
                let preview = move_preview::preview(self.git.as_ref(), &root, path, target)?;
                Ok(vec![Event::MovePreviewed { id, preview }])
            }
            CommandKind::MoveChanges {
                path,
                target,
                message,
            } => {
                only_a_human(actor, "move changes")?;
                let root = self.open_root()?;
                let moved =
                    move_exec::run(self.git.as_ref(), &root, path, target, message.as_deref())?;
                let mut events = self.reloaded(id, &root)?;
                events.push(Event::ChangesMoved { id, moved });
                Ok(events)
            }
            CommandKind::RemoveWorktree { path, force } => {
                self.remove_worktree(id, actor, path, *force)
            }
            CommandKind::LockWorktree { path, reason } => {
                only_a_human(actor, "lock a worktree")?;
                let root = self.open_root()?;
                self.listed(&root, path)?;
                self.git
                    .lock_worktree(&root, path, reason)
                    .map_err(|e| e.0)?;
                self.reloaded(id, &root)
            }
            CommandKind::UnlockWorktree { path } => {
                only_a_human(actor, "unlock a worktree")?;
                let root = self.open_root()?;
                self.listed(&root, path)?;
                self.git.unlock_worktree(&root, path).map_err(|e| e.0)?;
                self.reloaded(id, &root)
            }
            CommandKind::ListPrunable => {
                let root = self.open_root()?;
                let candidates = self.git.prunable(&root).map_err(|e| e.0)?;
                Ok(vec![Event::PrunableListed { id, candidates }])
            }
            CommandKind::PruneWorktrees { names } => self.prune(id, actor, names),
            _ => self.branch_command(id, actor, kind),
        }
    }

    /// The listed worktree at `path`; any other path is refused before Git runs.
    fn listed(&self, root: &Path, path: &str) -> Result<WorktreeEntry, String> {
        self.git
            .worktrees(root)
            .map_err(|e| e.0)?
            .into_iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| format!("{path} is not a worktree of this repository"))
    }

    /// What removing the worktree at `path` would lose or refuse. Reads only;
    /// the same state gives the same inspection, with no timestamp in it.
    fn inspect(&self, root: &Path, path: &str) -> Result<WorktreeInspection, String> {
        let entry = self.listed(root, path)?;
        let folder = Path::new(path);
        let missing = !folder.is_dir();
        let (mut staged, mut unstaged, mut untracked, mut unpushed) = (0, 0, 0, 0);
        if !missing {
            let status = self.git.status(folder).map_err(|e| e.0)?;
            for change in &status.entries {
                staged += u32::from(change.staged.is_some());
                unstaged += u32::from(change.unstaged.is_some());
                untracked += u32::from(change.untracked);
            }
            unpushed = self
                .git
                .unpushed_commits(folder, entry.branch.as_deref())
                .map_err(|e| e.0)?;
        }
        let blockers = worktree::blockers([
            (entry.is_main, Blocker::Main),
            (
                self.root
                    .as_deref()
                    .is_some_and(|open| same_folder(open, path)),
                Blocker::Current,
            ),
            (missing, Blocker::Missing),
            (entry.locked.is_some(), Blocker::Locked),
            (staged + unstaged + untracked > 0, Blocker::Dirty),
            (unpushed > 0, Blocker::Unpushed),
        ]);
        Ok(WorktreeInspection {
            path: path.to_owned(),
            branch: entry.branch.clone(),
            name: worktree::name_of(path, entry.branch.as_deref()),
            locked: entry.locked.clone(),
            missing,
            staged,
            unstaged,
            untracked,
            unpushed,
            blockers,
        })
    }

    /// Removes a worktree after the rule of D31 checks it again, now: the
    /// dialog's earlier inspection is not trusted.
    fn remove_worktree(
        &self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        force: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "remove a worktree")?;
        let root = self.open_root()?;
        let inspection = self.inspect(&root, path)?;
        inspection.check_removal(force)?;
        // A locked worktree needs Git's second `--force`.
        let level = if inspection.blockers.is_empty() {
            0
        } else if inspection.locked.is_some() {
            2
        } else {
            1
        };
        self.git
            .remove_worktree(&root, path, level)
            .map_err(|e| e.0)?;
        self.reloaded(id, &root)
    }

    /// Removes the admin records named in `names`, if the dry run still lists
    /// exactly those.
    fn prune(
        &self,
        id: OperationId,
        actor: &Actor,
        names: &[String],
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "prune worktrees")?;
        let root = self.open_root()?;
        let mut now: Vec<String> = self
            .git
            .prunable(&root)
            .map_err(|e| e.0)?
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        let mut expected = names.to_vec();
        now.sort();
        expected.sort();
        if now != expected {
            return Err(
                "the worktrees to prune changed since they were listed; list them again".to_owned(),
            );
        }
        self.git.prune_worktrees(&root).map_err(|e| e.0)?;
        self.reloaded(id, &root)
    }

    /// The status and the worktree list, after a change to the worktrees.
    fn reloaded(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        self.loaded(id, root, status)
    }

    /// Opens the repository at `path`. The history walk of the old one ends.
    fn open_repo(&mut self, id: OperationId, path: &str) -> Result<Vec<Event>, String> {
        let root = self.git.resolve_root(path.as_ref()).map_err(|e| e.0)?;
        let status = self.git.status(&root).map_err(|e| e.0)?;
        let mut events = self.loaded(id, &root, status)?;
        events.extend(self.drop_history());
        events.extend(self.drop_detail());
        events.extend(self.drop_stack());
        events.extend(self.drop_files());
        events.extend(self.drop_provenance());
        // A resolution belongs to the repository it was started in. Its worktree is left for
        // `ResumeRestack` to remove when that repository opens again (D154).
        self.restack_resolution = None;
        self.root = Some(root);
        Ok(events)
    }

    fn open_root(&self) -> Result<PathBuf, String> {
        self.root
            .clone()
            .ok_or_else(|| "no repository is open".to_owned())
    }

    /// The stash list, newest first. Reads only.
    fn list_stashes(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let stashes = self.git.stashes(&root).map_err(|e| e.0)?;
        Ok(vec![Event::StashesListed { id, stashes }])
    }

    /// The conflict commands (D114). A read answers with the file's stages merged
    /// into markers. The resolution is human only and changes the index.
    fn conflict_command(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::ReadConflict { path, site } => {
                let read = match site {
                    ConflictSite::WorkingCopy => {
                        let root = self.open_root()?;
                        self.git.read_conflict(&root, path)
                    }
                    ConflictSite::Restack => {
                        let session = self.open_resolution()?;
                        self.git.read_restack_conflict(
                            &session.worktree,
                            path,
                            &session.parent,
                            &session.onto,
                            &session.commit,
                        )
                    }
                }
                .map_err(|e| e.0)?;
                Ok(vec![Event::ConflictRead { id, read }])
            }
            CommandKind::ResolveConflict {
                path, text, site, ..
            } => {
                only_a_human(actor, "resolve a conflict")?;
                match site {
                    ConflictSite::WorkingCopy => {
                        let root = self.open_root()?;
                        self.git
                            .resolve_conflict(&root, path, text)
                            .map_err(|e| e.0)?;
                        let mut events = self.reload_status(id, &root)?;
                        events.push(Event::ConflictResolved {
                            id,
                            path: path.clone(),
                        });
                        Ok(events)
                    }
                    // The resolution worktree is not the checkout: its status is not shown.
                    ConflictSite::Restack => {
                        let worktree = self.open_resolution()?.worktree.clone();
                        self.git
                            .resolve_conflict(&worktree, path, text)
                            .map_err(|e| e.0)?;
                        Ok(vec![Event::ConflictResolved {
                            id,
                            path: path.clone(),
                        }])
                    }
                }
            }
            CommandKind::ForgetRecordedResolution { path, site } => {
                only_a_human(actor, "forget a recorded resolution")?;
                let root = match site {
                    ConflictSite::WorkingCopy => self.open_root()?,
                    ConflictSite::Restack => self.open_resolution()?.worktree.clone(),
                };
                self.git
                    .forget_recorded_resolution(&root, path)
                    .map_err(|e| e.0)?;
                Ok(vec![Event::RecordedResolutionForgotten {
                    id,
                    path: path.clone(),
                }])
            }
            _ => Err("not a conflict command".to_owned()),
        }
    }

    /// The stash writes (B2, D94). Human only. An apply, a pop or a drop first
    /// checks that `stash@{index}` still is the stash the human listed, so a
    /// stale index never acts on another stash.
    fn stash_command(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::CreateStash {
                message,
                include_untracked,
            } => {
                only_a_human(actor, "create a stash")?;
                let root = self.open_root()?;
                let message = message
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty());
                self.git
                    .create_stash(&root, message, *include_untracked)
                    .map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::ApplyStash { index, oid } => {
                only_a_human(actor, "apply a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.apply_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::PopStash { index, oid } => {
                only_a_human(actor, "pop a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.pop_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            CommandKind::DropStash { index, oid } => {
                only_a_human(actor, "drop a stash")?;
                let root = self.open_root()?;
                self.ensure_stash_listed(&root, *index, oid)?;
                self.git.drop_stash(&root, *index).map_err(|e| e.0)?;
                self.stashes_changed(id, &root)
            }
            _ => Err("not a stash command".to_owned()),
        }
    }

    /// Refuses unless `stash@{index}` still is the stash `oid` the human listed.
    fn ensure_stash_listed(&self, root: &Path, index: u32, oid: &str) -> Result<(), String> {
        match self.git.resolve_stash(root, index).map_err(|e| e.0)? {
            Some(now) if now == oid => Ok(()),
            _ => Err(STASH_LIST_CHANGED.to_owned()),
        }
    }

    /// The status and the stash list after a stash write, so no front-end asks.
    fn stashes_changed(&self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        let mut events = self.reload_status(id, root)?;
        let stashes = self.git.stashes(root).map_err(|e| e.0)?;
        events.push(Event::StashesListed { id, stashes });
        Ok(events)
    }

    /// Discards every change of one file (B1, D93). Human only: the loss is
    /// permanent. The status follows, as after a stage.
    fn discard_file(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        let CommandKind::DiscardFile {
            path,
            include_staged,
        } = kind
        else {
            return Err("not a discard command".to_owned());
        };
        only_a_human(actor, "discard changes")?;
        let root = self.open_root()?;
        self.git
            .discard_file(&root, path, *include_staged)
            .map_err(|e| e.0)?;
        self.reload_status(id, &root)
    }

    /// A mutation ends by showing the new state, so no front-end has to ask.
    /// The status and the worktree list of one read, so both reach the UI together.
    fn loaded(
        &self,
        id: OperationId,
        root: &std::path::Path,
        status: gm_schema::RepoStatus,
    ) -> Result<Vec<Event>, String> {
        let worktrees = self.git.worktrees(root).map_err(|e| e.0)?;
        Ok(vec![
            Event::StatusLoaded { id, status },
            Event::WorktreesLoaded { id, worktrees },
        ])
    }

    fn reload_status(&self, id: OperationId, root: &std::path::Path) -> Result<Vec<Event>, String> {
        let status = self.git.status(root).map_err(|e| e.0)?;
        Ok(vec![Event::StatusLoaded { id, status }])
    }

    /// Converts the user's `prepare-commit` sections into a workflow in the user's workflows
    /// directory (`WF3e`, D160). The answer lists the file, the skipped sections and the sections
    /// the person can remove. Human only.
    fn convert_hooks(&self, id: OperationId, actor: &Actor) -> Result<Vec<Event>, String> {
        only_a_human(actor, "convert hooks to workflows")?;
        let root = self.open_root()?;
        let dir = self
            .workflow_user_dir
            .as_deref()
            .ok_or_else(|| "there is no user workflows directory (HOME is not set)".to_owned())?;
        let converted = hook_convert::convert(self.git.as_ref(), &root, dir)?;
        Ok(vec![Event::HooksConverted {
            id,
            written: converted.written,
            skipped: converted.skipped,
            sections: converted.sections,
        }])
    }

    /// The workspace commands (D183). The reads answer with the listing, the plan and the removal
    /// inspection, and write nothing. The writes are human only and refresh the open repository's
    /// status, which may be one of the members. The window's own workspace is the one that cannot
    /// be removed from it: the open repository stands for the directory the command runs in.
    fn workspace_command(
        &self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        // Relative roots and the configured root are read from the open repository, if any.
        let here = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
        let ws = Workspaces::new(self.git.as_ref(), self.workspace_config.clone());
        match kind {
            CommandKind::ListWorkspaces { root } => {
                let root = ws
                    .root(root.as_deref(), &here, self.workspace_home.as_deref())
                    .map_err(|e| e.to_string())?;
                let workspaces = ws.list(&root).map_err(|e| e.to_string())?;
                Ok(vec![Event::WorkspacesListed {
                    id,
                    workspaces,
                    root: root.display().to_string(),
                }])
            }
            CommandKind::PlanWorkspace {
                name,
                members,
                base,
                root,
            } => {
                let request = self.workspace_request(
                    &ws,
                    &here,
                    (name, members, base.as_deref(), root.as_deref()),
                )?;
                let plan = ws.plan(&request).map_err(|e| e.to_string())?;
                Ok(vec![Event::WorkspacePlanned {
                    id,
                    path: plan.path.display().to_string(),
                    members: plan.members,
                    problems: plan.problems,
                }])
            }
            CommandKind::CreateWorkspace {
                name,
                members,
                base,
                root,
            } => {
                only_a_human(actor, "create a workspace")?;
                let request = self.workspace_request(
                    &ws,
                    &here,
                    (name, members, base.as_deref(), root.as_deref()),
                )?;
                self.create_workspace(id, &ws, &request)
            }
            CommandKind::InspectWorkspaceRemoval { path, closing } => {
                let root = self.open_root()?;
                let inspection = ws
                    .inspect_removal(Path::new(path), &root, *closing)
                    .map_err(|e| e.to_string())?;
                Ok(vec![Event::WorkspaceRemovalInspected {
                    id,
                    path: path.clone(),
                    blockers: inspection.blockers,
                }])
            }
            CommandKind::RemoveWorkspace {
                path,
                force,
                closing,
            } => {
                only_a_human(actor, "remove a workspace")?;
                let root = self.open_root()?;
                ws.remove(Path::new(path), &root, *force, *closing)
                    .map_err(|e| e.to_string())?;
                // A closing window's repository is the folder just removed: there is no status
                // to read (D185).
                let mut events = if *closing {
                    Vec::new()
                } else {
                    self.refreshed(id)?
                };
                events.push(Event::WorkspaceRemoved {
                    id,
                    path: path.clone(),
                });
                Ok(events)
            }
            CommandKind::CheckOutMemberBranch { path } => {
                only_a_human(actor, "check out a member's branch")?;
                let root = self.open_root()?;
                ws.check_out_member_branch(&root, path)
                    .map_err(|e| e.to_string())?;
                self.reload_status(id, &root)
            }
            _ => Err("not a workspace command".to_owned()),
        }
    }

    /// Creates the workspace `request` plans (D183). It refuses a plan with problems, or with no
    /// member, before anything is written. A member that failed is reported with the workspace,
    /// which exists either way.
    fn create_workspace(
        &self,
        id: OperationId,
        ws: &Workspaces,
        request: &CreateRequest,
    ) -> Result<Vec<Event>, String> {
        let plan = ws.plan(request).map_err(|e| e.to_string())?;
        if !plan.problems.is_empty() {
            let problems: Vec<String> = plan
                .problems
                .iter()
                .map(|problem| problem.message.clone())
                .collect();
            return Err(format!("no workspace was created: {}", problems.join("; ")));
        }
        if plan.members.is_empty() {
            return Err("no repository would be added; no workspace was created".to_owned());
        }
        let created = ws.create(plan).map_err(|e| e.to_string())?;
        let path = created.path.display().to_string();
        let failed: Vec<String> = created
            .added
            .iter()
            .filter_map(|added| {
                added
                    .failure
                    .as_ref()
                    .map(|failure| format!("{}: {failure}", added.member))
            })
            .collect();
        if !failed.is_empty() {
            return Err(format!(
                "the workspace {path} was created, but {}",
                failed.join("; ")
            ));
        }
        let mut events = self.refreshed(id)?;
        events.push(Event::WorkspaceCreated { id, path });
        Ok(events)
    }

    /// The request a planned or created workspace is checked with: the root (the flag, else the
    /// configured one, else `~/workspaces`) and the members as they were asked.
    fn workspace_request(
        &self,
        ws: &Workspaces,
        here: &Path,
        asked: (&str, &[MemberRequest], Option<&str>, Option<&str>),
    ) -> Result<CreateRequest, String> {
        let (name, members, base, root) = asked;
        let root = ws
            .root(root, here, self.workspace_home.as_deref())
            .map_err(|e| e.to_string())?;
        Ok(CreateRequest {
            name: name.to_owned(),
            root,
            members: members.to_vec(),
            base: base.map(str::to_owned),
            existing: false,
        })
    }

    /// The status and the worktree list of the open repository, when one is open: a workspace
    /// command may have changed a worktree of it.
    fn refreshed(&self, id: OperationId) -> Result<Vec<Event>, String> {
        match self.root.clone() {
            Some(root) => self.reloaded(id, &root),
            None => Ok(Vec::new()),
        }
    }

    /// Record Combination (D180(3)): the gitlinks of the members whose commit is new are
    /// committed alone, with the message the members list. Refusals leave nothing written.
    /// Human only; the answer is the same as a commit's: the new id and the status.
    fn record_combination(&self, id: OperationId, actor: &Actor) -> Result<Vec<Event>, String> {
        only_a_human(actor, "record a combination")?;
        let root = self.open_root()?;
        let members = self.git.combination_members(&root).map_err(|e| e.0)?;
        let plan = combination::plan(&members)?;
        let oid = self
            .git
            .commit_paths(&root, &plan.paths, &plan.message)
            .map_err(|e| e.0)?;
        self.committed(id, &root, oid)
    }

    /// The answer of a commit: its id, then the status it leaves.
    fn committed(
        &self,
        id: OperationId,
        root: &std::path::Path,
        oid: String,
    ) -> Result<Vec<Event>, String> {
        let mut events = vec![Event::Committed { id, oid }];
        events.extend(self.reload_status(id, root)?);
        Ok(events)
    }

    /// Stages every change of the working tree, then reads the status again.
    fn stage_all(&self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        self.git.stage_all(&root).map_err(|e| e.0)?;
        self.reload_status(id, &root)
    }

    /// The thread commands (design/threads.md, "Engine surface"). The reads are open
    /// to any actor; the writes and the export are human only. A write answers
    /// `ThreadsChanged` in its own events.
    fn threads_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        if let Some(what) = threads_session::human_only(kind) {
            only_a_human(actor, what)?;
        }
        let root = self.open_root()?;
        let store = threads_session::store(&mut self.threads, self.git.as_ref(), &root)?;
        let ctx = threads_session::Context {
            git: self.git.as_ref(),
            root: &root,
            global_config: None,
            contexts_db: self.contexts_db.as_deref(),
        };
        threads_session::run(id, store, &ctx, actor, kind)
    }

    /// Reads the status of the open repository again and answers with it.
    fn refresh_status(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let status = self.git.status(&root).map_err(|e| e.0)?;
        self.loaded(id, &root, status)
    }

    /// The context commands (design/threads.md, "Contexts"). The list is open to any actor; the
    /// writes are human only. Only `ListContexts { repo: true }` and `RenewRepoId` read the open
    /// repository.
    fn contexts_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        if let Some(what) = contexts_session::human_only(kind) {
            only_a_human(actor, what)?;
        }
        contexts_session::run(
            id,
            self.contexts_db.as_deref(),
            self.git.as_ref(),
            self.root.as_deref(),
            kind,
        )
    }

    /// The review comment commands (B4, D96). The writes are human only; the list
    /// and a snapshot's text are reads.
    fn review_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::ListReviewComments { author } => {
                let root = self.open_root()?;
                self.review_listed_by(id, &root, *author)
            }
            CommandKind::CreateAgentReviewComment {
                path,
                anchor_text,
                kind,
                body,
                severity,
            } => {
                let author = agent_author(actor)?;
                let body = review_body(body)?;
                review::check_kind_and_severity(*kind, *severity)?;
                let thread = AgentThread {
                    author: &author,
                    kind: *kind,
                    severity: *severity,
                    body,
                };
                self.create_agent_review_comment(id, path, anchor_text, &thread)
            }
            CommandKind::LoadReviewSnapshotText { id: comment } => {
                self.load_review_snapshot(id, *comment)
            }
            CommandKind::LoadReviewCurrentText { id: comment } => {
                self.load_review_current(id, *comment)
            }
            CommandKind::LoadReviewChange { id: comment } => self.load_review_change(id, *comment),
            CommandKind::AddReviewComment {
                path,
                start_line,
                end_line,
                body,
            } => {
                only_a_human(actor, "comment on the code")?;
                self.add_review_comment(id, path, *start_line, *end_line, body)
            }
            CommandKind::AddReviewCommentByText { path, text, body } => {
                only_a_human(actor, "comment on the code")?;
                self.add_review_comment_by_text(id, path, text, body)
            }
            CommandKind::EditReviewComment { id: comment, body } => {
                only_a_human(actor, "edit a review comment")?;
                self.edit_review_comment(id, *comment, body)
            }
            CommandKind::ResolveReviewComment { id: comment } => {
                only_a_human(actor, "resolve a review comment")?;
                self.set_review_status(id, *comment, ReviewStatus::Resolved)
            }
            CommandKind::HandleReviewComment {
                id: comment,
                summary,
            } => self.handle_review_comment(id, actor, *comment, summary),
            CommandKind::ReplyToReviewComment { id: comment, body } => {
                self.reply_to_review_comment(id, actor, *comment, body)
            }
            CommandKind::ReopenReviewComment { id: comment } => {
                only_a_human(actor, "reopen a review comment")?;
                self.set_review_status(id, *comment, ReviewStatus::Open)
            }
            CommandKind::DeleteReviewComment { id: comment } => {
                only_a_human(actor, "delete a review comment")?;
                self.delete_review_comment(id, *comment)
            }
            CommandKind::RecordVerdict {
                reviewer,
                branch,
                head,
                value,
                rationale,
                findings,
            } => {
                let request = VerdictRequest {
                    reviewer,
                    branch,
                    head,
                    value,
                    rationale,
                    findings,
                };
                self.record_verdict(id, actor, &request)
            }
            CommandKind::ListVerdicts {
                branch,
                reviewer,
                limit,
            } => self.list_verdicts(id, branch.as_deref(), reviewer.as_deref(), *limit),
            CommandKind::ListReviewers { name } => self.list_reviewers(id, name.as_deref()),
            CommandKind::GatesStatus { branch } => self.gates_status(id, branch.as_deref()),
            CommandKind::ListBypasses { branch, limit } => {
                self.list_bypasses(id, branch.as_deref(), *limit)
            }
            _ if is_approval_command(kind) => self.approval_command(id, actor, kind),
            _ => Err("not a review command".to_owned()),
        }
    }

    /// The approval and intent commands (D215). The writes are human only, so an agent can never
    /// approve a branch or change its intent; the reads are not logged (D28).
    fn approval_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::ApproveBranch { branch, head, note } => {
                only_a_human(actor, "approve a branch")?;
                self.approve_branch(id, actor, branch, head, note.as_deref())
            }
            CommandKind::RevokeApproval { branch } => {
                only_a_human(actor, "revoke an approval")?;
                self.revoke_approval(id, branch)
            }
            CommandKind::ReadApproval { branch } => self.read_approval(id, branch),
            CommandKind::SetBranchDescription { branch, text } => {
                only_a_human(actor, "edit a branch description")?;
                self.set_branch_description(id, branch, text)
            }
            CommandKind::ReadBranchDescription { branch } => {
                self.read_branch_description(id, branch)
            }
            _ => Err("not an approval command".to_owned()),
        }
    }

    /// Approves the tip of `branch` (D215 (2)). `head` must be the tip now, so a person approves
    /// exactly what they saw; a note of more than 500 characters is refused, and a blank one is
    /// no note.
    fn approve_branch(
        &mut self,
        id: OperationId,
        actor: &Actor,
        branch: &str,
        head: &str,
        note: Option<&str>,
    ) -> Result<Vec<Event>, String> {
        let note = note.filter(|note| !note.trim().is_empty());
        if let Some(note) = note {
            let count = note.chars().count();
            if count > MAX_APPROVAL_NOTE_CHARS {
                return Err(format!(
                    "an approval note is at most {MAX_APPROVAL_NOTE_CHARS} characters, not {count}"
                ));
            }
        }
        let root = self.open_root()?;
        let tip = self
            .git
            .branch_tip(&root, branch)
            .map_err(|e| e.0)?
            .ok_or_else(|| format!("there is no branch named {branch}"))?;
        let head = self
            .git
            .resolve_commit(&root, head)
            .map_err(|_| format!("{head:?} is not a commit"))?;
        if head != tip {
            return Err(format!(
                "the branch moved since you looked: {} is not the tip {}; look again",
                short_id(&head),
                short_id(&tip)
            ));
        }
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        store
            .approve(branch, &tip, note, &origin_of(actor), now_ms())
            .map_err(|e| e.0)?;
        let approval = approval_now(store, branch, Some(&tip))?;
        Ok(vec![Event::ApprovalRead {
            id,
            branch: branch.to_owned(),
            approval,
        }])
    }

    /// Withdraws the approval of `branch`; the answer has no approval. Revoking a branch with no
    /// approval changes nothing.
    fn revoke_approval(&mut self, id: OperationId, branch: &str) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        store.revoke(branch).map_err(|e| e.0)?;
        Ok(vec![Event::ApprovalRead {
            id,
            branch: branch.to_owned(),
            approval: None,
        }])
    }

    /// The approval of `branch` with whether it is stale against the tip now (D215 (2)).
    fn read_approval(&mut self, id: OperationId, branch: &str) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let tip = self.git.branch_tip(&root, branch).map_err(|e| e.0)?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let approval = approval_now(store, branch, tip.as_deref())?;
        Ok(vec![Event::ApprovalRead {
            id,
            branch: branch.to_owned(),
            approval,
        }])
    }

    /// Sets the intent of `branch` (D215 (1)): its branch description. Blank text unsets it. The
    /// answer is the description as Git stores it now.
    fn set_branch_description(
        &mut self,
        id: OperationId,
        branch: &str,
        text: &str,
    ) -> Result<Vec<Event>, String> {
        if text.contains('\0') {
            return Err("a branch description cannot contain a NUL character".to_owned());
        }
        let count = text.chars().count();
        if count > MAX_DESCRIPTION_CHARS {
            return Err(format!(
                "a branch description is at most {MAX_DESCRIPTION_CHARS} characters, not {count}"
            ));
        }
        let root = self.open_root()?;
        let stored = (!text.trim().is_empty()).then_some(text);
        self.git
            .set_branch_description(&root, branch, stored)
            .map_err(|e| e.0)?;
        self.read_branch_description(id, branch)
    }

    /// The intent of `branch`, its branch description, `None` when it has none.
    fn read_branch_description(
        &mut self,
        id: OperationId,
        branch: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let text = self
            .git
            .branch_description(&root, branch)
            .map_err(|e| e.0)?;
        Ok(vec![Event::BranchDescriptionRead {
            id,
            branch: branch.to_owned(),
            text,
        }])
    }

    /// What the gates of `branch` need from the repository (D215, D214 (1)): the tip, the change,
    /// whether a person approved the tip, and the intent. It gathers and never runs a gate.
    pub fn gate_facts_inputs(&mut self, root: &Path, branch: &str) -> Result<GateInputs, String> {
        let head = self
            .git
            .branch_tip(root, branch)
            .map_err(|e| e.0)?
            .ok_or_else(|| format!("there is no branch named {branch}"))?;
        let change = change::change_of(self.git.as_ref(), root, branch).map_err(|e| e.0)?;
        let intent = self.git.branch_description(root, branch).map_err(|e| e.0)?;
        let store = review_store(&mut self.review, self.git.as_ref(), root)?;
        let approved = store
            .approval(branch)
            .map_err(|e| e.0)?
            .is_some_and(|approval| !verdicts::is_stale(&approval.head, Some(&head)));
        Ok(GateInputs {
            head,
            change,
            approved,
            intent,
        })
    }

    /// Answers `GatesStatus`: the gate check of `branch`, or of the branch HEAD is on. A
    /// detached HEAD needs the branch named. Reads only (D28).
    fn gates_status(
        &mut self,
        id: OperationId,
        branch: Option<&str>,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let branch = match branch {
            Some(branch) => branch.to_owned(),
            None => self
                .git
                .status(&root)
                .map_err(|e| e.0)?
                .branch
                .head
                .ok_or_else(|| {
                    "HEAD is detached; name the branch whose gates to check".to_owned()
                })?,
        };
        let report = self.gates_report(&root, &branch, None)?;
        Ok(vec![Event::GatesStatusRead {
            id,
            status: report.status,
            intent: report.intent,
            approval: report.approval,
        }])
    }

    /// The gate check of `branch` for `op` (every gate when `op` is `None`) (D212 (5), D214,
    /// D215). It reads the definitions of the repository and the user, the tip, the change, the
    /// verdicts and findings of the branch, the approval and the stack parent's distance, then
    /// calls the pure `gates::evaluate`. A file that cannot be read is an error of the status,
    /// and a status with any error is never ready (fail closed). It blocks nothing by itself.
    pub fn gates_report(
        &mut self,
        root: &Path,
        branch: &str,
        op: Option<GateOp>,
    ) -> Result<GatesReport, String> {
        let common = self.git.common_dir(root).map_err(|e| e.0)?;
        let inputs = self.gate_facts_inputs(root, branch)?;
        let behind_parent = self.behind_parent(root, branch);
        let defined = reviewers::load(
            Some(&reviewers::repo_dir(&common)),
            self.reviewer_user_dir.as_deref(),
        );
        let loaded = gates::load(
            Some(&common.join("gitmaster")),
            self.gates_user_dir.as_deref(),
        );
        let store = review_store(&mut self.review, self.git.as_ref(), root)?;
        // Every verdict the store keeps for the branch: `evaluate` picks the newest one for each
        // reviewer whose head is the tip, so an older verdict of an earlier commit still counts
        // after a reset back to it. Retention keeps five per reviewer, so the limit is a bound.
        let verdicts = store
            .list_verdicts(Some(branch), None, GATE_VERDICT_LIMIT)
            .map_err(|e| e.0)?;
        let findings = counted_findings(self.git.as_ref(), store, root, branch, &inputs.head)?;
        let approval = approval_now(store, branch, Some(&inputs.head))?;
        let preset_names: Vec<String> = reviewers::presets()
            .into_iter()
            .map(|preset| preset.name)
            .collect();
        let facts = GateFacts {
            branch,
            head: &inputs.head,
            changed_paths: inputs
                .change
                .as_ref()
                .and_then(|change| change.paths.as_deref()),
            reviewers: &defined.reviewers,
            preset_names: &preset_names,
            verdicts: &verdicts,
            findings: &findings,
            behind_parent,
            approved: inputs.approved,
        };
        let mut status = gates::evaluate(&loaded.gates, op, &facts);
        let mut errors: Vec<String> = defined
            .errors
            .into_iter()
            .map(|(file, reason)| format!("{file}: {reason}"))
            .collect();
        errors.extend(loaded.errors);
        status.ready = status.ready && errors.is_empty();
        status.errors = errors;
        Ok(GatesReport {
            status,
            intent: inputs.intent,
            approval,
        })
    }

    /// The gate files of the repository and the user, as read (D212 (2)). Nothing else is read.
    fn gate_files(&self, root: &Path) -> Result<gates::LoadedGates, String> {
        let common = self.git.common_dir(root).map_err(|e| e.0)?;
        Ok(gates::load(
            Some(&common.join("gitmaster")),
            self.gates_user_dir.as_deref(),
        ))
    }

    /// The native gate check of `branch` for `operation` (D212 (3)): `None` when nothing blocks.
    /// With no gate and no gate file problem it returns at once. A reviewer file problem alone
    /// does not block; with a gate defined, the report counts it, and it fails closed.
    fn gate_check(&mut self, operation: GateOp, branch: &str) -> Result<Option<GateBlock>, String> {
        let root = self.open_root()?;
        let loaded = self.gate_files(&root)?;
        if loaded.gates.is_empty() && loaded.errors.is_empty() {
            return Ok(None);
        }
        let report = self.gates_report(&root, branch, Some(operation))?;
        if report.status.ready {
            return Ok(None);
        }
        let gates = report
            .status
            .gates
            .into_iter()
            .filter(|gate| gate.applicable && !gate.passed)
            .collect();
        Ok(Some(GateBlock {
            gates,
            errors: report.status.errors,
        }))
    }

    /// How many commits `branch` is behind its parent in the stack model (D214 (1)). `None` when
    /// it has no parent, the parent is not a local branch any more, or the stack cannot be read.
    fn behind_parent(&self, root: &Path, branch: &str) -> Option<u32> {
        let link = self
            .git
            .stack_links(root)
            .ok()?
            .into_iter()
            .find(|link| link.branch == branch && link.parent != branch)?;
        self.git
            .stack_edge(root, branch, &link.parent, link.base.as_deref())
            .ok()
            .map(|edge| edge.behind)
    }

    /// Anchors a comment on lines of `path` and keeps a snapshot of the working
    /// tree for it. The row is stored first, so the private ref is made for a
    /// comment that exists; a snapshot that fails leaves the comment without one.
    fn add_review_comment(
        &mut self,
        id: OperationId,
        path: &str,
        start_line: u32,
        end_line: u32,
        body: &str,
    ) -> Result<Vec<Event>, String> {
        let body = review_body(body)?;
        let root = self.open_root()?;
        let text = review_session::read_text(&root, path)?;
        let anchor = review::anchor_for(&text, start_line, end_line)?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let comment = store.add(path, body, &anchor, now_ms()).map_err(|e| e.0)?;
        snapshot_before(self.git.as_ref(), store, &root, comment, path)?;
        self.review_listed(id, &root)
    }

    /// Starts an agent's thread on the text `anchor_text` of `path` (D200). The text is found
    /// as a person's comment finds it (D141): exactly once in the file now, or the thread is
    /// refused and nothing is written. The store checks the limits in the same write, and the
    /// "before" snapshot is taken as for a person's comment (D140).
    fn create_agent_review_comment(
        &mut self,
        id: OperationId,
        path: &str,
        anchor_text: &str,
        thread: &AgentThread,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let file = review_session::read_text(&root, path)?;
        let place = review::lines_of_text(&file, anchor_text)?;
        let anchor = review::anchor_for(&file, place.start_line, place.end_line)?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let comment = store
            .add_agent(path, thread, &anchor, now_ms())
            .map_err(|e| e.0)?;
        snapshot_before(self.git.as_ref(), store, &root, comment, path)?;
        self.review_listed(id, &root)
    }

    /// Records a verdict and its findings (D212 (6)). Every check runs before the first write:
    /// the store keeps the verdict and its findings in one transaction, and each finding is
    /// anchored on its lines as an agent's comment is (D97), with the "before" snapshot taken as
    /// for a person's comment (D140).
    fn record_verdict(
        &mut self,
        id: OperationId,
        actor: &Actor,
        request: &VerdictRequest,
    ) -> Result<Vec<Event>, String> {
        if !matches!(actor, Actor::Agent { .. }) {
            return Err("only an agent records a verdict".to_owned());
        }
        let origin = origin_of(actor);
        let root = self.open_root()?;
        // Redaction first, and it fails closed: a configuration that does not compile refuses the
        // whole record (D121, D212 (6)).
        let redactor = threads_session::redactor_at(&root, None).map_err(|reason| {
            format!("redaction is misconfigured, so nothing was recorded: {reason}")
        })?;
        let reviewer = self.reviewer_named(&root, request.reviewer)?;
        if reviewer.subject != ReviewerSubject::Branch {
            return Err(format!(
                "reviewer {} has subject {}; only branch reviewers can record verdicts for now",
                reviewer.name,
                subject_name(reviewer.subject)
            ));
        }
        match self.git.status(&root).map_err(|e| e.0)?.branch.head {
            Some(current) if current == request.branch => {}
            Some(current) => {
                return Err(format!(
                    "the branch {} is not checked out here, HEAD is on {current}; a verdict is for the branch checked out in this worktree",
                    request.branch
                ));
            }
            None => {
                return Err(format!(
                    "HEAD is detached; check out {} to record its verdict",
                    request.branch
                ));
            }
        }
        let head = self
            .git
            .resolve_commit(&root, request.head)
            .map_err(|_| format!("{:?} is not a commit", request.head))?;
        let tip = self
            .git
            .branch_tip(&root, request.branch)
            .map_err(|e| e.0)?;
        if tip.as_deref() != Some(head.as_str()) {
            return Err(format!(
                "head {} is not the tip of {} ({}); the branch moved since you judged it, run the reviewer again",
                short_id(&head),
                request.branch,
                tip.as_deref().map_or("none", short_id)
            ));
        }
        let value = verdicts::check_value(reviewer.signal, request.value)?;
        let rationale = redactor.redact(request.rationale).text;
        let rationale = verdict_text(&rationale, verdicts::MAX_RATIONALE_CHARS, "the rationale")?;
        if request.findings.len() > verdicts::MAX_FINDINGS {
            return Err(format!(
                "a verdict has at most {} findings, not {}",
                verdicts::MAX_FINDINGS,
                request.findings.len()
            ));
        }
        let findings = anchored_findings(&root, request.findings, &redactor)?;
        let new = NewVerdict {
            reviewer: reviewer.name.clone(),
            branch: request.branch.to_owned(),
            head,
            signal: signal_name(reviewer.signal).to_owned(),
            value,
            rationale: rationale.to_owned(),
            origin,
            model: None,
            run_thread: None,
            at: now_ms(),
        };
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let recorded = store.record_verdict(&new, &findings).map_err(|e| e.0)?;
        for (comment, finding) in recorded.comments.iter().zip(request.findings) {
            snapshot_before(self.git.as_ref(), store, &root, *comment, &finding.path)?;
        }
        let verdict = store
            .list_verdicts(Some(request.branch), Some(&reviewer.name), 1)
            .map_err(|e| e.0)?
            .into_iter()
            .next()
            .ok_or_else(|| "the engine did not list the new verdict".to_owned())?;
        let mut events = vec![Event::VerdictRecorded {
            id,
            verdict,
            comment_ids: recorded.comments,
        }];
        events.extend(self.review_listed(id, &root)?);
        Ok(events)
    }

    /// The verdicts that `branch` and `reviewer` pick, newest first, each with whether it is
    /// stale against the tip of its branch now (D189 (2)). Each branch's tip is read once.
    fn list_verdicts(
        &mut self,
        id: OperationId,
        branch: Option<&str>,
        reviewer: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let limit = match limit {
            0 => verdicts::DEFAULT_LIST_LIMIT,
            n => n.min(verdicts::MAX_LIST_LIMIT),
        };
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let mut found = store
            .list_verdicts(
                branch,
                reviewer,
                usize::try_from(limit).unwrap_or(usize::MAX),
            )
            .map_err(|e| e.0)?;
        let mut tips: BTreeMap<String, Option<String>> = BTreeMap::new();
        for verdict in &mut found {
            if !tips.contains_key(&verdict.branch) {
                let tip = self
                    .git
                    .branch_tip(&root, &verdict.branch)
                    .map_err(|e| e.0)?;
                tips.insert(verdict.branch.clone(), tip);
            }
            let tip = tips.get(&verdict.branch).and_then(|tip| tip.as_deref());
            verdict.stale = verdicts::is_stale(&verdict.head, tip);
        }
        Ok(vec![Event::VerdictsListed {
            id,
            verdicts: found,
        }])
    }

    /// The reviewers of the repository and the user with the presets they do not override, by
    /// name, or only the one called `name` (D212 (1)). The unreadable files come back too.
    fn list_reviewers(
        &mut self,
        id: OperationId,
        name: Option<&str>,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let (mut reviewers, errors) = self.reviewers(&root)?;
        if let Some(name) = name {
            reviewers.retain(|reviewer| reviewer.name == name);
        }
        Ok(vec![Event::ReviewersListed {
            id,
            reviewers,
            errors,
        }])
    }

    /// The reviewers a run can use in `root`: the repository's, the user's and the presets.
    fn reviewers(&self, root: &Path) -> Result<(Vec<Reviewer>, Vec<ReviewerFileError>), String> {
        let common = self.git.common_dir(root).map_err(|e| e.0)?;
        Ok(verdicts::merged_reviewers(
            &reviewers::repo_dir(&common),
            self.reviewer_user_dir.as_deref(),
        ))
    }

    /// The reviewer called `name`; refused, naming the ones there are, when none is. A preset is
    /// a template and not a reviewer (R2h, D212 (1)): a name that only a preset has is refused
    /// until a reviewer file defines it.
    fn reviewer_named(&self, root: &Path, name: &str) -> Result<Reviewer, String> {
        let (found, _) = self.reviewers(root)?;
        let defined: Vec<&str> = found
            .iter()
            .map(|reviewer| reviewer.name.as_str())
            .collect();
        let reviewer = found
            .iter()
            .find(|reviewer| reviewer.name == name)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "there is no reviewer named {name:?}; the defined ones are {}",
                    defined.join(", ")
                )
            })?;
        if reviewer.source == ReviewerSource::Preset {
            return Err(format!(
                "{name} is a preset, not a reviewer yet: add it as a reviewer first (Reviewers Settings, or write .git/gitmaster/reviewers/{name}.md)"
            ));
        }
        Ok(reviewer)
    }

    /// Comments on `text` where it is in `path` now (D141): found exactly once, or refused.
    fn add_review_comment_by_text(
        &mut self,
        id: OperationId,
        path: &str,
        text: &str,
        body: &str,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let file = review_session::read_text(&root, path)?;
        let place = review::lines_of_text(&file, text)?;
        self.add_review_comment(id, path, place.start_line, place.end_line, body)
    }

    fn edit_review_comment(
        &mut self,
        id: OperationId,
        comment: i64,
        body: &str,
    ) -> Result<Vec<Event>, String> {
        let body = review_body(body)?;
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        store.set_body(comment, body).map_err(|e| e.0)?;
        self.review_listed(id, &root)
    }

    /// Resolves or reopens comment `comment`; refused when it already has `status`.
    fn set_review_status(
        &mut self,
        id: OperationId,
        comment: i64,
        status: ReviewStatus,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        if current.status == status {
            return Err(match status {
                ReviewStatus::Resolved => "the comment is already resolved".to_owned(),
                ReviewStatus::Open | ReviewStatus::Handled => {
                    "the comment is already open".to_owned()
                }
            });
        }
        store
            .set_status(comment, status, now_ms())
            .map_err(|e| e.0)?;
        // Reopening drops the "after" snapshot (D140); the row no longer names it.
        if status == ReviewStatus::Open && current.after_snapshot.is_some() {
            self.git
                .delete_review_ref(&root, comment, ReviewSide::After)
                .map_err(|e| e.0)?;
        }
        self.review_listed(id, &root)
    }

    /// An agent's note that it worked on comment `comment` (D135): the status becomes
    /// `Handled`, and the summary is added as a reply by the actor. A resolved
    /// comment is a human's decision and is not touched. Then the "after" snapshot of
    /// the working tree is taken and kept under its private ref (D140), in place of an
    /// earlier one; a snapshot that fails leaves the comment handled without one, and
    /// its old ref is dropped.
    fn handle_review_comment(
        &mut self,
        id: OperationId,
        actor: &Actor,
        comment: i64,
        summary: &str,
    ) -> Result<Vec<Event>, String> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("the summary is empty".to_owned());
        }
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        if matches!(actor, Actor::Agent { .. }) && review::is_agent_started(&current) {
            return Err(
                "an agent cannot hand off a thread it started: only a person closes it".to_owned(),
            );
        }
        let (earlier, path) = (current.after_snapshot, current.path);
        if !store
            .handle(comment, &origin_of(actor), summary, now_ms())
            .map_err(|e| e.0)?
        {
            return Err("the comment is already resolved".to_owned());
        }
        let snapshot = self
            .git
            .snapshot_worktree_for(&root, &path)
            .and_then(|oid| {
                self.git
                    .set_review_ref(&root, comment, ReviewSide::After, &oid)?;
                Ok(oid)
            });
        match snapshot {
            Ok(oid) => store.set_after_snapshot(comment, &oid).map_err(|e| e.0)?,
            Err(_) if earlier.is_some() => {
                // The comment stays handled without an "after"; a ref left behind
                // would keep an old snapshot alive for nothing.
                let _ = self
                    .git
                    .delete_review_ref(&root, comment, ReviewSide::After);
            }
            Err(_) => {}
        }
        self.review_listed(id, &root)
    }

    /// A reply to comment `comment`, which may be a root or a reply: it attaches to the
    /// root. An agent's reply keeps the status. A person's reply reopens a handled
    /// comment, as reopening does: the "after" snapshot is dropped with its ref (D140).
    /// A resolved comment takes no reply.
    fn reply_to_review_comment(
        &mut self,
        id: OperationId,
        actor: &Actor,
        comment: i64,
        body: &str,
    ) -> Result<Vec<Event>, String> {
        let body = body.trim();
        if body.is_empty() {
            return Err("the reply is empty".to_owned());
        }
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let parent = store
            .root_of(comment)
            .map_err(|e| e.0)?
            .ok_or_else(|| format!("no review comment {comment}"))?;
        let current = find_review(store, parent)?;
        let reopen = matches!(actor, Actor::Human);
        let outcome = store
            .reply(parent, &origin_of(actor), body, now_ms(), reopen)
            .map_err(|e| e.0)?;
        match outcome {
            Reply::Resolved => return Err("the comment is already resolved".to_owned()),
            Reply::Added { reopened } => {
                if reopened && current.after_snapshot.is_some() {
                    self.git
                        .delete_review_ref(&root, parent, ReviewSide::After)
                        .map_err(|e| e.0)?;
                }
            }
        }
        self.review_listed(id, &root)
    }

    /// Deletes comment `comment` and the private refs that kept its snapshots.
    fn delete_review_comment(
        &mut self,
        id: OperationId,
        comment: i64,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        store.delete(comment).map_err(|e| e.0)?;
        if current.snapshot.is_some() {
            self.git
                .delete_review_ref(&root, comment, ReviewSide::Before)
                .map_err(|e| e.0)?;
        }
        if current.after_snapshot.is_some() {
            self.git
                .delete_review_ref(&root, comment, ReviewSide::After)
                .map_err(|e| e.0)?;
        }
        self.review_listed(id, &root)
    }

    /// The lines comment `comment` covered in its snapshot, for the Before side.
    /// Empty when there is no snapshot, the file was not in it, or it is not UTF-8.
    fn load_review_snapshot(
        &mut self,
        id: OperationId,
        comment: i64,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        let lines = match &current.snapshot {
            Some(oid) => match self
                .git
                .file_at_commit(&root, oid, &current.path)
                .map_err(|e| e.0)?
            {
                Some(bytes) => String::from_utf8(bytes).map_or_else(
                    |_| Vec::new(),
                    |text| review::snapshot_region(&text, &current.anchor),
                ),
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        Ok(vec![Event::ReviewSnapshotText {
            id,
            comment,
            path: current.path,
            lines,
        }])
    }

    /// The lines comment `comment` covers in the file now, for the After side.
    /// Empty when the comment is outdated, or its file cannot be read.
    fn load_review_current(&mut self, id: OperationId, comment: i64) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        let lines = review_session::read_text(&root, &current.path)
            .map(|text| review::current_region(&text, &current.anchor))
            .unwrap_or_default();
        Ok(vec![Event::ReviewCurrentText {
            id,
            comment,
            path: current.path,
            lines,
        }])
    }

    /// The anchored lines of comment `comment` in its "before" and "after"
    /// snapshots (D140). No change when either snapshot is missing, the file
    /// was not in the "before" one, either side is not UTF-8, or the anchor is
    /// not in the "before" text. A file gone from the "after" snapshot has an
    /// empty after side.
    fn load_review_change(&mut self, id: OperationId, comment: i64) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let store = review_store(&mut self.review, self.git.as_ref(), &root)?;
        let current = find_review(store, comment)?;
        let change = match (&current.snapshot, &current.after_snapshot) {
            (Some(before), Some(after)) => self.review_change(&root, &current, before, after)?,
            _ => None,
        };
        Ok(vec![Event::ReviewChange {
            id,
            comment,
            path: current.path,
            change,
        }])
    }

    /// Reads both snapshots of `current` and maps its region from one to the other.
    fn review_change(
        &self,
        root: &Path,
        current: &ReviewComment,
        before: &str,
        after: &str,
    ) -> Result<Option<ReviewChange>, String> {
        let text_at = |oid: &str| -> Result<Option<Result<String, ()>>, String> {
            Ok(self
                .git
                .file_at_commit(root, oid, &current.path)
                .map_err(|e| e.0)?
                .map(|bytes| String::from_utf8(bytes).map_err(drop)))
        };
        let Some(Ok(before_text)) = text_at(before)? else {
            return Ok(None);
        };
        let after_text = match text_at(after)? {
            Some(Ok(text)) => Some(text),
            Some(Err(())) => return Ok(None),
            None => None,
        };
        let hunks: Vec<review::LineHunk> = if after_text.is_some() {
            self.git
                .diff_commits(root, before, after, &current.path)
                .map_err(|e| e.0)?
                .hunks
                .iter()
                .map(|hunk| review::LineHunk {
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(review::review_change(
            &before_text,
            after_text.as_deref(),
            &current.anchor,
            &hunks,
        ))
    }

    /// Every comment with its location in the file now, as the front-ends list them after a
    /// write: all threads, whoever started them, so that no write hides one.
    fn review_listed(&mut self, id: OperationId, root: &Path) -> Result<Vec<Event>, String> {
        self.review_listed_by(id, root, ReviewAuthorFilter::Any)
    }

    /// The comments that `author` asks for (D200), with their locations in the file now.
    fn review_listed_by(
        &mut self,
        id: OperationId,
        root: &Path,
        author: ReviewAuthorFilter,
    ) -> Result<Vec<Event>, String> {
        let store = review_store(&mut self.review, self.git.as_ref(), root)?;
        let comments = review_session::listed(store, root)?
            .into_iter()
            .filter(|comment| review::listed_for(comment, author))
            .collect();
        Ok(vec![Event::ReviewCommentsListed { id, comments }])
    }

    /// The workflow commands: the listing, and a run's start, approval and cancel.
    fn workflow_command(
        &mut self,
        id: OperationId,
        actor: &Actor,
        kind: &CommandKind,
    ) -> Result<Vec<Event>, String> {
        match kind {
            CommandKind::RunWorkflow { name, inputs } => self.run_script(id, actor, name, inputs),
            CommandKind::ReviewWorkflow {
                name,
                hash,
                auto_approve,
            } => self.review_script(id, actor, name, hash, *auto_approve),
            CommandKind::RevokeWorkflowReview { hash } => self.revoke_review(id, actor, hash),
            CommandKind::DismissWorkflowRequest { workflow } => {
                self.dismiss_request(actor, workflow)
            }
            CommandKind::ApproveWorkflowStep { run, approved } => {
                self.approve_script_step(actor, *run, *approved)
            }
            CommandKind::CancelWorkflowRun { run } => self.cancel_script_run(actor, *run),
            CommandKind::ListWorkflows => self.list_scripts(id),
            CommandKind::CreateWorkflowFromTemplate { template } => {
                self.create_script_from_template(id, actor, template)
            }
            CommandKind::SetWorkflowEnabled { name, enabled } => {
                self.set_workflow_enabled(id, actor, name, *enabled)
            }
            CommandKind::CopyBuiltinWorkflow { name } => self.copy_builtin(id, actor, name),
            CommandKind::ListWorkflowRuns { workflow, limit } => {
                self.list_workflow_runs(id, workflow, *limit)
            }
            CommandKind::LoadWorkflowRun { run } => self.load_workflow_run(id, *run),
            CommandKind::DryRunWorkflow { source, inputs } => {
                self.dry_run_workflow(id, source, inputs)
            }
            CommandKind::CheckWorkflowScript { source } => Ok(check_workflow_script(id, source)),
            CommandKind::SaveWorkflowFile {
                path,
                source,
                expected_hash,
            } => self.save_workflow_file(id, actor, path, source, expected_hash.as_deref()),
            CommandKind::DeleteWorkflowFile {
                path,
                expected_hash,
            } => self.delete_workflow_file(id, actor, path, expected_hash),
            _ => Err("not a workflow command".to_owned()),
        }
    }

    /// The run journal of the open repository, for a read. A session that is open has
    /// recovered the runs a previous session left (D100) when it opened; otherwise this is the
    /// first look since the app started, and the runs still marked unfinished are over now.
    fn journal(&self, root: &Path) -> Result<Journal, String> {
        let common = self.git.common_dir(root).map_err(|e| e.0)?;
        let journal = Journal::open(&common.join("gitmaster").join("state.db")).map_err(|e| e.0)?;
        if self
            .script
            .as_ref()
            .is_none_or(|session| session.root != root)
        {
            journal.recover_interrupted(now_ms()).map_err(|e| e.0)?;
        }
        Ok(journal)
    }

    /// The newest `limit` runs of `workflow` in the open repository. Reads only.
    fn list_workflow_runs(
        &mut self,
        id: OperationId,
        workflow: &str,
        limit: u32,
    ) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let journal = self.journal(&root)?;
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let rows = journal
            .runs(workflow, limit.min(RETAINED_RUNS))
            .map_err(|e| e.0)?;
        Ok(vec![Event::WorkflowRunsListed {
            id,
            workflow: workflow.to_owned(),
            runs: rows.iter().map(script_session::run_summary).collect(),
        }])
    }

    /// One run with its script, input and every `ctx` call. Reads only.
    fn load_workflow_run(&mut self, id: OperationId, run: i64) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let journal = self.journal(&root)?;
        let row = journal
            .run(run)
            .map_err(|e| e.0)?
            .ok_or_else(|| format!("no run {run}"))?;
        let calls = journal.calls(run).map_err(|e| e.0)?;
        Ok(vec![Event::WorkflowRunLoaded {
            id,
            detail: Box::new(script_session::run_detail(&row, &calls)),
        }])
    }

    /// Dry run of the script text `source` (the editor's, not a saved file): it runs with every
    /// `ctx` call answered by the dry host, so nothing is performed. Reads only; no run is
    /// recorded. A declared input that `inputs` does not give is `""`.
    fn dry_run_workflow(
        &self,
        id: OperationId,
        source: &str,
        inputs: &serde_json::Value,
    ) -> Result<Vec<Event>, String> {
        let engine = LuauEngine::new();
        let dry_run = match inspect(&engine, TEXT_CHUNK, source) {
            Ok(meta) => {
                let root = self.open_root()?;
                let mut given = Map::new();
                let mut missing = Vec::new();
                for spec in &meta.inputs {
                    let value = if let Some(value) =
                        inputs.get(&spec.name).and_then(serde_json::Value::as_str)
                    {
                        value.to_owned()
                    } else {
                        missing.push(spec.name.clone());
                        String::new()
                    };
                    given.insert(spec.name.clone(), json!(value));
                }
                let repo = script_input::repo_input(self.git.as_ref(), &root);
                let input = json!({
                    "inputs": given,
                    "trigger": { "on": "manual" },
                    "repo": repo.value,
                });
                let run = dry::walk(&engine, TEXT_CHUNK, source, &input);
                script_session::dry_run_result(missing, run)
            }
            Err(problems) => script_session::dry_run_error(&problems),
        };
        Ok(vec![Event::WorkflowDryRun {
            id,
            dry_run: Box::new(dry_run),
        }])
    }

    /// Writes the workflow file at `path`, which must be in a workflows directory
    /// (never a repository). With `expected_hash` the file on disk must still hash
    /// to it; without, the file must not exist. Human only. It writes no Git
    /// state, so it is not in the command log.
    fn save_workflow_file(
        &mut self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        source: &str,
        expected_hash: Option<&str>,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "save a workflow file")?;
        let root = self.open_root()?;
        let dirs = workflow_session::workflow_dirs(
            self.git.as_ref(),
            &root,
            self.workflow_user_dir.as_deref(),
        )?;
        if let Some(user) = self.workflow_user_dir.as_deref()
            && Path::new(path).parent() == Some(user)
            && !user.exists()
        {
            std::fs::create_dir_all(user)
                .map_err(|e| format!("could not create {}: {e}", user.display()))?;
        }
        let file = workflow_session::workflow_file(&root, &dirs, path)?;
        if u64::try_from(source.len()).unwrap_or(u64::MAX) > loader::MAX_FILE {
            return Err("the workflow is larger than 256 KiB".to_owned());
        }
        match expected_hash {
            Some(hash) => check_hash(&file, hash)?,
            None if std::fs::symlink_metadata(&file).is_ok() => {
                return Err(format!("a file already exists at {}", file.display()));
            }
            None => {}
        }
        workflow_session::write_atomically(&file, source)?;
        let mut events = vec![
            Event::WorkflowFileSaved {
                id,
                path: path.to_owned(),
                hash: loader::sha256_hex(source.as_bytes()),
            },
            self.scripts_listed(id, &root)?,
        ];
        events.extend(self.drop_stale_requests(&root));
        Ok(events)
    }

    /// Moves the workflow file at `path` to the trash folder, when it still hashes
    /// to `expected_hash`. Human only. Like a save, it is not in the command log.
    fn delete_workflow_file(
        &mut self,
        id: OperationId,
        actor: &Actor,
        path: &str,
        expected_hash: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "delete a workflow file")?;
        let root = self.open_root()?;
        let dirs = workflow_session::workflow_dirs(
            self.git.as_ref(),
            &root,
            self.workflow_user_dir.as_deref(),
        )?;
        let file = workflow_session::workflow_file(&root, &dirs, path)?;
        check_hash(&file, expected_hash)?;
        let trash = self
            .workflow_trash_dir
            .as_deref()
            .ok_or_else(|| "there is no trash folder for workflow files".to_owned())?;
        let trashed_to = workflow_session::trash_file(&file, trash, now_ms())?;
        let mut events = vec![
            Event::WorkflowFileDeleted {
                id,
                path: path.to_owned(),
                trashed_to: trashed_to.display().to_string(),
            },
            self.scripts_listed(id, &root)?,
        ];
        events.extend(self.drop_stale_requests(&root));
        Ok(events)
    }

    /// The review store, opened on first use.
    fn trust_store(&mut self) -> Result<TrustStore, String> {
        if self.trust.is_none() {
            let file = trust::default_file()
                .ok_or_else(|| "there is no data directory (HOME is not set)".to_owned())?;
            self.trust = Some(TrustStore::open(&file).map_err(|e| e.0)?);
        }
        self.trust
            .clone()
            .ok_or_else(|| "the review store is not open".to_owned())
    }

    /// The scripts of the user's directories, read in the sandbox without `ctx`, then the built-in
    /// ones that no user script takes the name of (`WF4b`).
    fn load_scripts(&self, root: &Path) -> Result<loader::Loaded, String> {
        let dirs = workflow_session::workflow_dirs(
            self.git.as_ref(),
            root,
            self.workflow_user_dir.as_deref(),
        )?;
        let engine = LuauEngine::new();
        let mut loaded = loader::load_dirs(&engine, &dirs, Some(root));
        builtins::add_to(&engine, &mut loaded);
        Ok(loaded)
    }

    /// The scripts that run in the open repository: `load_scripts` without the ones switched off
    /// here (D164(1)). Triggers, phases and transforms read them this way; a manual run does not.
    fn load_active_scripts(&self, root: &Path) -> Result<loader::Loaded, String> {
        let mut loaded = self.load_scripts(root)?;
        switches::retain_enabled(&mut loaded.entries, &self.switches_of(root)?);
        Ok(loaded)
    }

    /// The workflow switches of the repository at `root` (`WF4b`).
    fn switches_of(&self, root: &Path) -> Result<Switches, String> {
        switches::load(self.git.as_ref(), root)
    }

    /// The script `name`, if it loads.
    fn find_script(&self, root: &Path, name: &str) -> Result<loader::ScriptEntry, String> {
        let loaded = self.load_scripts(root)?;
        let entry = loaded
            .entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("no workflow named `{name}`"))?;
        if let Err(problems) = &entry.meta {
            let first = problems
                .first()
                .map_or_else(String::new, ToString::to_string);
            return Err(format!("the workflow `{name}` does not load: {first}"));
        }
        Ok(entry)
    }

    /// The scripts as the front ends list them. Reads only, apart from the `.api/` reference
    /// that is written once per session when its version changed (D102).
    fn list_scripts(&mut self, id: OperationId) -> Result<Vec<Event>, String> {
        let root = self.open_root()?;
        let mut events = self.drop_stale_requests(&root);
        let mut listed = self.scripts_listed(id, &root)?;
        if !self.api_written {
            self.api_written = true;
            if let Some(dir) = self.workflow_user_dir.as_deref()
                && let Err(e) = api_docs::write_if_stale(dir, env!("CARGO_PKG_VERSION"))
                && let Event::WorkflowsListed { ignored, .. } = &mut listed
            {
                ignored.push(format!(
                    "{}: could not write the .api reference: {e}",
                    dir.display()
                ));
            }
        }
        events.push(listed);
        Ok(events)
    }

    /// The `WorkflowsListed` event of the open repository's scripts.
    fn scripts_listed(&mut self, id: OperationId, root: &Path) -> Result<Event, String> {
        let loaded = self.load_scripts(root)?;
        // Each version's review and grant, read once for the listing (`WF3b`). A store that cannot
        // be read lists every version as unreviewed. The switches of an unreadable file are the
        // defaults here: the listing still shows the scripts, and the triggers say why they do not run.
        let trust = self.trust_store().ok();
        let switches = self.switches_of(root).unwrap_or_default();
        Ok(Event::WorkflowsListed {
            id,
            workflows: loaded
                .entries
                .iter()
                .map(|entry| {
                    let mut info = script_session::workflow_info(entry);
                    let review = trust
                        .as_ref()
                        .and_then(|trust| trust.get(&entry.hash).ok().flatten());
                    info.reviewed = review.is_some() || builtins::is_shipped(&entry.hash);
                    info.auto_approve = review.is_some_and(|review| review.auto_approve);
                    info.enabled = switches.is_enabled(&entry.name, entry.builtin);
                    info
                })
                .collect(),
            ignored: script_session::ignored_lines(&loaded),
            user_dir: self
                .workflow_user_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
            templates: script_templates::all()
                .iter()
                .map(|template| template.name.to_owned())
                .collect(),
        })
    }

    /// Turns the workflow `name` on or off in the open repository (`WF4b`, D164(1)). A built-in is
    /// switched on by its name, a user workflow off by its name. Switching a stack workflow changes
    /// which stacks it watches, so the stacks are read again as the baseline: the change that was
    /// already there fires nothing (D162(2)).
    fn set_workflow_enabled(
        &mut self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        enabled: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "switch a workflow")?;
        let root = self.open_root()?;
        let loaded = self.load_scripts(&root)?;
        // The user's file wins over a built-in of the same name, as the listing shows it.
        let builtin = loaded
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("no workflow named `{name}`"))?
            .builtin;
        let mut switched = self.switches_of(&root)?;
        switched.set(name, builtin, enabled);
        switches::save(self.git.as_ref(), &root, &switched)?;
        let mut events = self.drop_stale_requests(&root);
        events.extend(self.refresh_stacks(id, &root, false));
        events.push(self.scripts_listed(id, &root)?);
        Ok(events)
    }

    /// Copies the built-in `name` into the user's workflows directory as `<name>-custom.lua` (then
    /// `-2`, ...), for editing (`WF4b`, D164(4)). The copy is unreviewed, so it runs only after the
    /// person reviews it (D101). Never into a repository.
    fn copy_builtin(
        &self,
        id: OperationId,
        actor: &Actor,
        name: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "copy a built-in workflow")?;
        let builtin = builtins::get(name)
            .ok_or_else(|| format!("there is no built-in workflow named `{name}`"))?;
        let dir = self
            .workflow_user_dir
            .as_deref()
            .ok_or_else(|| "there is no user workflows directory (HOME is not set)".to_owned())?;
        let copied = script_templates::copy(
            dir,
            &builtins::copy_base(builtin.name),
            builtin.source,
            builtin.name,
        )?;
        Ok(vec![Event::WorkflowCopied {
            id,
            path: copied.path.display().to_string(),
            name: copied.name,
        }])
    }

    /// Copies a built-in template into the user's workflows directory; never into a repository.
    fn create_script_from_template(
        &self,
        id: OperationId,
        actor: &Actor,
        template_name: &str,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "create a workflow from a template")?;
        let template = script_templates::get(template_name)
            .ok_or_else(|| format!("no template named `{template_name}`"))?;
        let dir = self
            .workflow_user_dir
            .as_deref()
            .ok_or_else(|| "there is no user workflows directory (HOME is not set)".to_owned())?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let created = script_templates::create(dir, template)?;
        Ok(vec![Event::WorkflowCreated {
            id,
            path: created.path.display().to_string(),
            name: created.name,
        }])
    }

    /// Starts the manual script `name`. A version the person has not reviewed does not start:
    /// the answer is `WorkflowReviewRequested` (D101). The run goes on with its own events.
    fn run_script(
        &mut self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        inputs: &BTreeMap<String, String>,
    ) -> Result<Vec<Event>, String> {
        if !matches!(actor, Actor::Human) {
            return Err("agents run workflows through MCP in a later slice".to_owned());
        }
        let root = self.open_root()?;
        let entry = self.find_script(&root, name)?;
        let meta = entry
            .meta
            .clone()
            .map_err(|_| "the workflow does not load".to_owned())?;
        if meta.trigger.on != "manual" {
            return Err(format!(
                "`{name}` runs on `{}`, not by hand",
                meta.trigger.on
            ));
        }
        for spec in &meta.inputs {
            if !inputs.contains_key(&spec.name) {
                return Err(format!("the input `{}` is required", spec.name));
            }
        }
        if let Some(extra) = inputs
            .keys()
            .find(|k| !meta.inputs.iter().any(|s| &s.name == *k))
        {
            return Err(format!("`{name}` has no input named `{extra}`"));
        }
        let trust = self.trust_store()?;
        if !reviewed_by(&trust, &entry.hash)? {
            return Ok(vec![Event::WorkflowReviewRequested {
                id,
                name: entry.name,
                hash: entry.hash,
                source: entry.source,
                inputs: inputs.clone(),
            }]);
        }
        let auto_approve = trust.auto_approve(&entry.hash).map_err(|e| e.0)?;
        let repo = script_input::repo_input(self.git.as_ref(), &root);
        let input = json!({ "inputs": inputs, "trigger": { "on": "manual" }, "repo": repo.value });
        let seed = u64::try_from(now_ms()).unwrap_or(0) ^ id.0.rotate_left(32);
        let run = self.start_script(
            id,
            RunSpec {
                workflow: entry.name,
                chunk: entry.file_name,
                source: entry.source,
                definition_hash: entry.hash,
                started_by: StartedBy::Human,
                trigger: json!({ "on": "manual" }),
                input,
                reviewed: true,
                auto_approve,
                seed,
            },
        )?;
        if let (Some(file), Some(session)) = (repo.diff_file, self.script.as_mut()) {
            session.keep_diff(run, file);
        }
        Ok(Vec::new())
    }

    /// Records that the person reviewed this version (D101). Refused when the file changed
    /// since the hash was shown, so a review is always of the text that was read.
    fn review_script(
        &mut self,
        id: OperationId,
        actor: &Actor,
        name: &str,
        hash: &str,
        auto_approve: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "review a workflow")?;
        let root = self.open_root()?;
        let entry = self.find_script(&root, name)?;
        if entry.hash != hash {
            return Err(
                "the script changed after it was shown to you; open it again to review the new version"
                    .to_owned(),
            );
        }
        self.trust_store()?
            .review(hash, &entry.name, auto_approve, now_ms())
            .map_err(|e| e.0)?;
        // An event run that waited for this version starts now, with the grant just chosen (`WF3b`).
        let mut events = self.start_wanted(&root, name, hash);
        events.push(self.scripts_listed(id, &root)?);
        Ok(events)
    }

    /// The person's answer to an approval a run waits for.
    fn approve_script_step(
        &self,
        actor: &Actor,
        run: i64,
        approved: bool,
    ) -> Result<Vec<Event>, String> {
        only_a_human(actor, "approve a workflow step")?;
        match self.script_runtime() {
            Some(runtime) if runtime.approve(run, approved) => Ok(Vec::new()),
            _ => Err(format!("run {run} is not awaiting approval")),
        }
    }

    /// Stops a script run: a blocked call wakes and its program is killed.
    fn cancel_script_run(&mut self, actor: &Actor, run: i64) -> Result<Vec<Event>, String> {
        only_a_human(actor, "cancel a workflow run")?;
        match self.script_runtime() {
            Some(runtime) if runtime.cancel(run) => {
                self.stop_script_wait(run);
                Ok(Vec::new())
            }
            _ => Err(format!("run {run} is not active")),
        }
    }

    /// The events of a workflow call. Once Git was written, the status is read
    /// again, so the window shows the change.
    fn workflow_events(
        &self,
        id: OperationId,
        root: &Path,
        mut events: Vec<Event>,
        wrote: bool,
    ) -> Vec<Event> {
        if wrote && let Ok(status) = self.git.status(root) {
            events.push(Event::StatusLoaded { id, status });
        }
        events
    }

    fn advance(&mut self, op: &mut Op, to: OperationState, events: &mut Vec<Event>) {
        self.advance_with(op, to, None, events);
    }

    fn advance_with(
        &mut self,
        op: &mut Op,
        to: OperationState,
        detail: Option<String>,
        events: &mut Vec<Event>,
    ) {
        op.state = op
            .state
            .move_to(to)
            .expect("the engine only takes legal transitions");
        self.record(op, detail, events);
    }

    fn record(&mut self, op: &Op, detail: Option<String>, events: &mut Vec<Event>) {
        // A workflow listing or run read is a read (D28), as is the stash list, and a
        // template copy or a workflow file save or delete writes no Git state: none
        // is logged.
        if !matches!(
            op.command.kind,
            CommandKind::ListStashes
                | CommandKind::LoadStacks
                | CommandKind::PlanRestack { .. }
                | CommandKind::ListWorkflows
                | CommandKind::ListHookSections
                | CommandKind::LoadReflog { .. }
                | CommandKind::LoadSubmodule { .. }
                | CommandKind::ListMembers
                | CommandKind::ListRepoBranches { .. }
                | CommandKind::ListWorkspaces { .. }
                | CommandKind::PlanWorkspace { .. }
                | CommandKind::InspectWorkspaceRemoval { .. }
                | CommandKind::ListWorkflowRuns { .. }
                | CommandKind::LoadWorkflowRun { .. }
                | CommandKind::DryRunWorkflow { .. }
                | CommandKind::CheckWorkflowScript { .. }
                | CommandKind::CreateWorkflowFromTemplate { .. }
                | CommandKind::SetWorkflowEnabled { .. }
                | CommandKind::CopyBuiltinWorkflow { .. }
                | CommandKind::SaveWorkflowFile { .. }
                | CommandKind::DeleteWorkflowFile { .. }
                | CommandKind::DismissWorkflowRequest { .. }
                | CommandKind::ListReviewComments { .. }
                | CommandKind::ListVerdicts { .. }
                | CommandKind::ListReviewers { .. }
                | CommandKind::ListBypasses { .. }
                | CommandKind::GatesStatus { .. }
                | CommandKind::ReadApproval { .. }
                | CommandKind::ReadBranchDescription { .. }
                | CommandKind::LoadReviewSnapshotText { .. }
                | CommandKind::LoadReviewCurrentText { .. }
                | CommandKind::LoadReviewChange { .. }
                | CommandKind::PreviewMoveChanges { .. }
                | CommandKind::ReadConflict { .. }
                | CommandKind::ListThreads { .. }
                | CommandKind::SearchThreads { .. }
                | CommandKind::ReadThread { .. }
                | CommandKind::ReadProvenance { .. }
                | CommandKind::ReadThreadEdit { .. }
                | CommandKind::ReadThreadMarkdown { .. }
                | CommandKind::ExportThread { .. }
                | CommandKind::ListLabels { .. }
                | CommandKind::ReadLabel { .. }
                | CommandKind::ListMemories { .. }
                | CommandKind::ReadMemory { .. }
                | CommandKind::RecordMemoryRetrieval { .. }
                | CommandKind::ListContexts { .. }
                | CommandKind::PreviewDeleteContext { .. }
        ) {
            self.log.push(LogEntry {
                id: op.id,
                actor: op.command.actor.clone(),
                kind: op.command.kind.clone(),
                state: op.state,
            });
        }
        events.push(Event::OperationChanged {
            id: op.id,
            actor: op.command.actor.clone(),
            state: op.state,
            detail,
        });
    }
}

/// The base the merged check used, and the branches it would delete.
struct MergedPlan {
    base: Option<String>,
    branches: Vec<MergedBranch>,
}

/// The gated form of `op`, whose gates are the ones `phases` found, in order.
fn gated_op(op: &Op, phases: Phases) -> GatedOp {
    let waiting = phases.gates.iter().cloned().collect();
    GatedOp {
        id: op.id,
        command: op.command.clone(),
        phases,
        waiting,
        run: None,
    }
}

/// What a finished remote operation says in its `Done` event.
fn success_message(kind: &CommandKind) -> &'static str {
    match kind {
        CommandKind::Fetch => "Fetched",
        CommandKind::Pull => "Pulled",
        _ => "Pushed",
    }
}

/// The member of the workspace at `root` that `path` is in (D181), by name. `None` for a file of
/// the workspace itself, and for every path of a repository without checked-out members.
fn member_named(root: &Path, path: &str) -> Option<String> {
    let members = members::checked_out(root);
    members::member_of(path, &members).map(str::to_owned)
}

/// Milliseconds since the Unix epoch: the clock of workflow runs.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// What a save or delete answers when the file moved on since it was read.
const CHANGED_ON_DISK: &str = "the file changed on disk; reload it";

/// Refuses unless the file at `file` still hashes to `expected`.
fn check_hash(file: &Path, expected: &str) -> Result<(), String> {
    match workflow_session::file_hash(file) {
        Some(actual) if actual == expected => Ok(()),
        _ => Err(CHANGED_ON_DISK.to_owned()),
    }
}

/// Worktree changes are human decisions (D31, Q12): an agent may not make one.
/// The name errors of a script's text cite: the text has no file.
const TEXT_CHUNK: &str = "workflow.lua";

/// Answers `CheckWorkflowScript`: the problems `inspect` finds in `source`, none when it loads.
fn check_workflow_script(id: OperationId, source: &str) -> Vec<Event> {
    let diagnostics = match inspect(&LuauEngine::new(), TEXT_CHUNK, source) {
        Ok(_) => Vec::new(),
        Err(problems) => problems
            .into_iter()
            .map(|problem| WorkflowDiagnostic {
                line: u32::try_from(problem.line).unwrap_or(u32::MAX),
                message: problem.message,
            })
            .collect(),
    };
    vec![Event::WorkflowChecked { id, diagnostics }]
}

/// The runs `handle` starts, which `execute` never runs: the transforms (`WF3d`).
fn is_handled_run(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::RunTransform { .. }
            | CommandKind::ComposerOpened
            | CommandKind::BypassGates { .. }
    )
}

/// The conflict commands (D114, D115): reads and writes of a conflicted file.
fn is_conflict_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ReadConflict { .. }
            | CommandKind::ResolveConflict { .. }
            | CommandKind::ForgetRecordedResolution { .. }
    )
}

/// Whether the version with `hash` was reviewed (D101): the app's own built-ins are (D164(2)), any
/// other version when the person reviewed it. Reads the review store the caller holds.
fn reviewed_by(trust: &TrustStore, hash: &str) -> Result<bool, String> {
    if builtins::is_shipped(hash) {
        return Ok(true);
    }
    trust.is_reviewed(hash).map_err(|e| e.0)
}

/// The findings that count for `branch` at its tip `tip` (R2h, D212 (6), D214 (1)): a finding
/// counts when its branch is `branch`, or when the commit it was made on is an ancestor of the tip,
/// so a rename of the branch does not let it escape. Each distinct commit is tested once. A test
/// that fails, and a commit past `FINDING_HEAD_LIMIT`, count the finding: the check fails closed.
fn counted_findings(
    git: &dyn GitPort,
    store: &ReviewStore,
    root: &Path,
    branch: &str,
    tip: &str,
) -> Result<Vec<FindingFact>, String> {
    let rows = store.branch_findings_with_heads(branch).map_err(|e| e.0)?;
    let mut ancestors: BTreeMap<String, bool> = BTreeMap::new();
    let mut facts = Vec::new();
    for (_, severity, status, finding_branch, finding_head) in rows {
        let counts = if finding_branch.as_deref() == Some(branch) {
            true
        } else {
            match finding_head {
                Some(head) => match ancestors.get(&head) {
                    Some(known) => *known,
                    None if ancestors.len() >= FINDING_HEAD_LIMIT => true,
                    None => {
                        let answer = git.is_ancestor(root, &head, tip).unwrap_or(true);
                        ancestors.insert(head, answer);
                        answer
                    }
                },
                None => false,
            }
        };
        if counts {
            facts.push(FindingFact { severity, status });
        }
    }
    Ok(facts)
}

/// The ids of a block as a bypass names them: each blocking gate, then `gates-file` once when a
/// gate file has a problem (D212 (4)).
fn block_ids(block: &GateBlock) -> Vec<String> {
    let mut ids: Vec<String> = block.gates.iter().map(|gate| gate.id.clone()).collect();
    if !block.errors.is_empty() {
        ids.push(GATES_FILE_ID.to_owned());
    }
    ids
}

/// Checks the gate ids a bypass names: 1 to 50 of them, each a gate id or `gates-file` (D212 (4)).
fn check_bypass_gates(gates: &[String]) -> Result<(), String> {
    if gates.is_empty() || gates.len() > BYPASS_GATES_LIMIT {
        return Err(format!(
            "a bypass names the gates it bypasses: 1 to {BYPASS_GATES_LIMIT} gate ids"
        ));
    }
    for id in gates {
        if id != GATES_FILE_ID && reviewers::validate_name(id).is_err() {
            return Err(
                "a gate id is 1 to 40 characters of a-z, 0-9 and '-', or gates-file".to_owned(),
            );
        }
    }
    Ok(())
}

/// The `GatesBlocked` event for a blocked push or restack (D212 (3)).
fn gates_blocked_event(id: OperationId, blocked: BlockedGates) -> Event {
    Event::GatesBlocked {
        id,
        operation: blocked.operation,
        branch: blocked.branch,
        gates: blocked.block.gates,
        errors: blocked.block.errors,
    }
}

/// The veto text of a block: one line per blocking gate, then one per gate file problem (D212 (3)).
fn block_text(block: &GateBlock) -> String {
    block
        .gates
        .iter()
        .map(|gate| {
            format!(
                "Blocked by gate {}: {}",
                gate.id,
                gate.reason.as_deref().unwrap_or("the gate does not pass")
            )
        })
        .chain(
            block
                .errors
                .iter()
                .map(|error| format!("Gate file problem: {error}")),
        )
        .collect::<Vec<_>>()
        .join("; ")
}

/// The text of a native gate check that could not run: it fails closed (D212 (3)).
fn unchecked(reason: &str) -> String {
    format!("gates could not be checked: {reason}")
}

fn only_a_human(actor: &Actor, what: &str) -> Result<(), String> {
    if matches!(actor, Actor::Human) {
        Ok(())
    } else {
        Err(format!("only a human can {what}"))
    }
}

/// The store of the open repository: opened on first use, and reopened when
/// another repository is open.
fn review_store<'a>(
    slot: &'a mut Option<ReviewSession>,
    git: &dyn GitPort,
    root: &Path,
) -> Result<&'a mut ReviewStore, String> {
    if slot.as_ref().is_none_or(|session| session.root != root) {
        *slot = Some(ReviewSession::open(git, root)?);
    }
    match slot {
        Some(session) => Ok(&mut session.store),
        None => Err("the review store is not open".to_owned()),
    }
}

/// The most verdicts one gate check reads for a branch (D212 (5)). A reviewer keeps five, so
/// this holds two hundred reviewers; a branch past it reads as missing verdicts, which blocks.
const GATE_VERDICT_LIMIT: usize = 1000;

/// Whether the native gate check runs for an operation (D212 (3), (4)). `Bypassed` is set only by
/// `BypassGates` for the command it runs, and nothing else sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateCheck {
    Native,
    Bypassed,
}

/// What the native gate check found for a push or a restack (D212 (3)): the gates that block
/// (applicable and not passed) and the gate file problems.
struct GateBlock {
    gates: Vec<GateOutcome>,
    errors: Vec<String>,
}

/// A blocked push or restack: its operation and branch, and what blocks it.
struct BlockedGates {
    operation: GateOp,
    branch: String,
    block: GateBlock,
}

/// The longest reason a bypass takes, once trimmed (D212 (4)).
const BYPASS_REASON_LIMIT: usize = 500;

/// The most gate ids one bypass names (D212 (4)).
const BYPASS_GATES_LIMIT: usize = 50;

/// The id a gate file problem has in a bypass (D212 (4)).
const GATES_FILE_ID: &str = "gates-file";

/// What a checked bypass does: run `then` with this operation detail, or refuse it because the
/// block now holds a gate the person did not name, which is sent with the refusal.
enum Bypass {
    Run {
        detail: String,
        audit: BypassAudit,
    },
    Stale {
        message: String,
        blocked: BlockedGates,
    },
}

/// What a bypass writes to its audit row before it runs (D212 (4), R2h).
struct BypassAudit {
    branch: String,
    head: String,
    operation: GateOp,
    gates: Vec<String>,
    reason: String,
}

/// The bypasses `ListBypasses` returns when it names no limit, and the most it returns (R2h).
const BYPASS_LIST_DEFAULT: u32 = 20;
const BYPASS_LIST_MAX: u32 = 200;

/// The most distinct commits one gate check tests for ancestry (R2h). A finding whose commit is
/// past the limit counts, which blocks: the check fails closed.
const FINDING_HEAD_LIMIT: usize = 200;

/// What `Engine::gates_report` says about a branch (D212 (5), D215): the gate check, the intent
/// (text from the repository, so untrusted) and the approval, with whether it is stale.
pub struct GatesReport {
    pub status: GatesStatus,
    pub intent: Option<String>,
    pub approval: Option<Approval>,
}

/// What the gates of a branch need, gathered by `Engine::gate_facts_inputs` (D215, D214 (1)).
pub struct GateInputs {
    /// The tip of the branch: the commit the gates judge.
    pub head: String,
    /// The change of the branch, `None` when it has no base (D214 (1)).
    pub change: Option<Change>,
    /// True when a person approved `head` and the approval is not stale (D215 (2)).
    pub approved: bool,
    /// The intent of the branch, its description; text from the repository, so untrusted.
    pub intent: Option<String>,
}

/// What an agent records as a verdict (`CommandKind::RecordVerdict`), borrowed from the command.
struct VerdictRequest<'a> {
    reviewer: &'a str,
    branch: &'a str,
    head: &'a str,
    value: &'a str,
    rationale: &'a str,
    findings: &'a [FindingInput],
}

/// A verdict's text without its surrounding blank space; refused when it is empty or longer
/// than `limit` characters. `what` names the text in the reason.
fn verdict_text<'a>(text: &'a str, limit: usize, what: &str) -> Result<&'a str, String> {
    let text = text.trim();
    let length = text.chars().count();
    if text.is_empty() {
        Err(format!("{what} is empty"))
    } else if length > limit {
        Err(format!(
            "{what} is {length} characters, at most {limit} are allowed"
        ))
    } else {
        Ok(text)
    }
}

/// The findings of a verdict as the store takes them: each body redacted and checked, each
/// file read once, and each finding anchored on its lines as the file is now (D97).
fn anchored_findings(
    root: &Path,
    inputs: &[FindingInput],
    redactor: &Redactor,
) -> Result<Vec<NewFinding>, String> {
    let mut files: BTreeMap<&str, String> = BTreeMap::new();
    for finding in inputs {
        if !files.contains_key(finding.path.as_str()) {
            let text = review_session::read_text(root, &finding.path)?;
            files.insert(&finding.path, text);
        }
    }
    let mut findings = Vec::with_capacity(inputs.len());
    for finding in inputs {
        let body = redactor.redact(&finding.body).text;
        let body = verdict_text(&body, verdicts::MAX_FINDING_BODY_CHARS, "a finding's body")?;
        let file = files
            .get(finding.path.as_str())
            .ok_or_else(|| format!("there is no file {} in the worktree", finding.path))?;
        let anchor = review::anchor_for(file, finding.start_line, finding.end_line)
            .map_err(|reason| format!("{}: {reason}", finding.path))?;
        findings.push(NewFinding {
            severity: finding.severity,
            path: finding.path.clone(),
            body: body.to_owned(),
            anchor,
            snapshot: None,
        });
    }
    Ok(findings)
}

/// A commit id as people write it in a message.
/// The longest note an approval may carry, in characters (D215 (2)).
const MAX_APPROVAL_NOTE_CHARS: usize = 500;
/// The longest intent a branch description may carry, in characters (D215 (1)).
const MAX_DESCRIPTION_CHARS: usize = 2_000;

fn short_id(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

/// The name of a reviewer's subject, as the reviewer file writes it.
fn subject_name(subject: ReviewerSubject) -> &'static str {
    match subject {
        ReviewerSubject::Branch => "branch",
        ReviewerSubject::Staged => "staged",
        ReviewerSubject::Commit => "commit",
    }
}

/// The name a verdict's signal has in the store.
fn signal_name(signal: ReviewerSignal) -> &'static str {
    match signal {
        ReviewerSignal::Score => "score",
        ReviewerSignal::Level => "level",
        ReviewerSignal::Pass => "pass",
    }
}

/// Records the "before" snapshot of `path` for comment `comment`, as a person's comment has it
/// (D140): the private ref and the row that names it. A snapshot that fails leaves the comment
/// without one.
fn snapshot_before(
    git: &dyn GitPort,
    store: &ReviewStore,
    root: &Path,
    comment: i64,
    path: &str,
) -> Result<(), String> {
    let snapshot = git.snapshot_worktree_for(root, path).and_then(|oid| {
        git.set_review_ref(root, comment, ReviewSide::Before, &oid)?;
        Ok(oid)
    });
    if let Ok(oid) = snapshot {
        store.set_snapshot(comment, &oid).map_err(|e| e.0)?;
    }
    Ok(())
}

/// The author of a thread an agent starts (D200): `agent:<client>`. Refused for a person.
fn agent_author(actor: &Actor) -> Result<String, String> {
    if matches!(actor, Actor::Agent { .. }) {
        Ok(origin_of(actor))
    } else {
        Err("only an agent can start a note, question or flag".to_owned())
    }
}

/// The comment `comment` as the store holds it; refused when there is none.
fn find_review(store: &ReviewStore, comment: i64) -> Result<ReviewComment, String> {
    store
        .get(comment)
        .map_err(|e| e.0)?
        .ok_or_else(|| format!("no review comment {comment}"))
}

/// A comment's body without its surrounding blank space; refused when empty.
fn review_body(body: &str) -> Result<&str, String> {
    let body = body.trim();
    if body.is_empty() {
        Err("the comment is empty".to_owned())
    } else {
        Ok(body)
    }
}

/// Whether `kind` is a workspace command (D183).
fn is_workspace_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ListWorkspaces { .. }
            | CommandKind::PlanWorkspace { .. }
            | CommandKind::CreateWorkspace { .. }
            | CommandKind::InspectWorkspaceRemoval { .. }
            | CommandKind::RemoveWorkspace { .. }
            | CommandKind::CheckOutMemberBranch { .. }
    )
}

/// Whether `kind` is one of the review comment, verdict or approval commands (B4, D212, D215).
fn is_review_command(kind: &CommandKind) -> bool {
    is_approval_command(kind)
        || matches!(
            kind,
            CommandKind::AddReviewComment { .. }
                | CommandKind::AddReviewCommentByText { .. }
                | CommandKind::ListReviewComments { .. }
                | CommandKind::CreateAgentReviewComment { .. }
                | CommandKind::EditReviewComment { .. }
                | CommandKind::ResolveReviewComment { .. }
                | CommandKind::HandleReviewComment { .. }
                | CommandKind::ReplyToReviewComment { .. }
                | CommandKind::ReopenReviewComment { .. }
                | CommandKind::DeleteReviewComment { .. }
                | CommandKind::LoadReviewSnapshotText { .. }
                | CommandKind::LoadReviewCurrentText { .. }
                | CommandKind::LoadReviewChange { .. }
                | CommandKind::RecordVerdict { .. }
                | CommandKind::ListVerdicts { .. }
                | CommandKind::ListReviewers { .. }
                | CommandKind::ListBypasses { .. }
                | CommandKind::GatesStatus { .. }
        )
}

/// Whether `kind` is an approval or branch intent command (D215).
fn is_approval_command(kind: &CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::ApproveBranch { .. }
            | CommandKind::RevokeApproval { .. }
            | CommandKind::ReadApproval { .. }
            | CommandKind::SetBranchDescription { .. }
            | CommandKind::ReadBranchDescription { .. }
    )
}

/// The approval of `branch` as the store holds it, with whether it is stale against `tip` now
/// (D215 (2)); the store never sets `stale`.
fn approval_now(
    store: &ReviewStore,
    branch: &str,
    tip: Option<&str>,
) -> Result<Option<Approval>, String> {
    let approval = store.approval(branch).map_err(|e| e.0)?;
    Ok(approval.map(|mut approval| {
        approval.stale = verdicts::is_stale(&approval.head, tip);
        approval
    }))
}

/// Whether two paths name the same folder, through symbolic links when both exist.
pub(crate) fn same_folder(open: &Path, path: &str) -> bool {
    let path = Path::new(path);
    match (std::fs::canonicalize(open), std::fs::canonicalize(path)) {
        (Ok(a), Ok(b)) => a == b,
        _ => open == path,
    }
}

/// A notice of the Workflows page. It is not about a run, so it carries no run id.
fn workflow_notice(text: String) -> Event {
    Event::WorkflowNotice { run: 0, text }
}

/// The conflicts of a file's text, in file order (`WF3d`, D168). A text that does not parse has none.
fn conflict_hunks_of(text: &str) -> Vec<Hunk> {
    conflict::parse(text).map_or_else(
        |_| Vec::new(),
        |doc| {
            doc.segments
                .into_iter()
                .filter_map(|segment| match segment {
                    Segment::Conflict(hunk) => Some(hunk),
                    Segment::Text(_) => None,
                })
                .collect()
        },
    )
}

/// Moves the notices of a transform sequence into the events, in order (`WF3d`).
fn flush_notices(seq: &mut transforms::Sequence, events: &mut Vec<Event>) {
    events.extend(seq.take_notices().into_iter().map(workflow_notice));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitError;
    use crate::fake::FakeGit;
    use gm_schema::{
        ConflictFileRead, ConflictSite, HistoryScope, HunkOrigin, RepoStatus, WorktreeMode,
    };
    use proptest::prelude::*;

    fn engine() -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        Engine::new(Box::new(FakeGit::new("/r", status)))
    }

    fn command(kind: CommandKind) -> Command {
        Command {
            actor: Actor::Human,
            kind,
        }
    }

    fn open() -> Command {
        command(CommandKind::OpenRepo { path: "/r".into() })
    }

    fn states(events: &[Event]) -> Vec<OperationState> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::OperationChanged { state, .. } => Some(*state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn open_walks_to_done_and_loads_status() {
        let events = engine().handle(OperationId(1), &open());
        assert_eq!(
            states(&events),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing,
                OperationState::Done
            ]
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { .. }))
        );
    }

    #[test]
    fn refresh_without_a_repo_fails_with_a_reason() {
        let events = engine().handle(OperationId(1), &command(CommandKind::RefreshStatus));
        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(detail.as_deref(), Some("no repository is open"));
    }

    #[test]
    fn list_branches_emits_the_branches_it_read() {
        let branches = vec![gm_schema::BranchEntry {
            name: "main".into(),
            is_remote: false,
            is_current: true,
            upstream: None,
            author_email: None,
        }];
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_branches(Ok(branches.clone()));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::ListBranches));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(events.contains(&Event::BranchesLoaded {
            id: OperationId(2),
            branches,
            user_email: None,
        }));
    }

    fn commit_of(oid: &str) -> gm_schema::CommitSummary {
        gm_schema::CommitSummary {
            oid: oid.into(),
            parents: Vec::new(),
            short: oid.chars().take(7).collect(),
            subject: oid.into(),
            refs: Vec::new(),
            author_time: 0,
        }
    }

    /// An engine over a fake repository whose HEAD has `oids`, newest first.
    fn history_engine(oids: &[&str]) -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_history(Ok(gm_schema::History {
            commits: oids.iter().map(|oid| commit_of(oid)).collect(),
            unborn: false,
            more: false,
        }));
        Engine::new(Box::new(git))
    }

    /// Runs every history job the engine accepted, as the runner's threads do.
    fn run_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_history_jobs()
            .into_iter()
            .flat_map(|job| engine.history_report(job.run()))
            .collect()
    }

    #[test]
    fn a_history_load_answers_once_its_page_is_read() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        let started = engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistory {
                scope: HistoryScope::default(),
            }),
        );
        assert_eq!(
            states(&started),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing
            ]
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::HistoryLoaded { .. }))
        );

        let events = run_jobs(&mut engine);

        let Some(Event::HistoryLoaded { id, history }) = events
            .iter()
            .find(|e| matches!(e, Event::HistoryLoaded { .. }))
        else {
            panic!("the page is not answered, got {events:?}");
        };
        assert_eq!(*id, OperationId(2));
        assert_eq!(history.commits.len(), 3);
        assert!(!history.more);
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    #[test]
    fn a_superseded_history_load_is_dropped_and_cancelled() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistory {
                scope: HistoryScope::default(),
            }),
        );
        let superseding = engine.handle(
            OperationId(3),
            &command(CommandKind::LoadHistory {
                scope: HistoryScope::default(),
            }),
        );
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let jobs = engine.take_history_jobs();
        assert_eq!(jobs.len(), 2);
        let mut jobs = jobs.into_iter();
        let stale = engine.history_report(jobs.next().unwrap().run());
        assert!(
            stale.is_empty(),
            "the superseded page must not show: {stale:?}"
        );
        let current = engine.history_report(jobs.next().unwrap().run());
        assert!(matches!(
            current.first(),
            Some(Event::OperationChanged {
                id: OperationId(3),
                state: OperationState::Done,
                ..
            })
        ));
        assert!(current.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_page_after_the_last_commit_continues_the_walk() {
        let oids = ["c", "b", "a"];
        let mut engine = history_engine(&oids);
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistory {
                scope: HistoryScope::default(),
            }),
        );
        run_jobs(&mut engine);

        // The first page held every commit, so the next one is the walk's end.
        let next = engine.handle(
            OperationId(3),
            &command(CommandKind::LoadHistoryPage { after: "a".into() }),
        );
        assert!(next.contains(&Event::OperationChanged {
            id: OperationId(3),
            actor: Actor::Human,
            state: OperationState::Executing,
            detail: None,
        }));
        let events = run_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded { id: OperationId(3), history } if history.commits.is_empty()
        )));
    }

    #[test]
    fn a_page_for_an_unknown_cursor_restarts_the_walk_past_it() {
        let mut engine = history_engine(&["c", "b", "a"]);
        engine.handle(OperationId(1), &open());
        // No page was read, so the cursor "b" matches no walk: the walk restarts and reads past it.
        engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistoryPage { after: "b".into() }),
        );
        let events = run_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::HistoryLoaded { id: OperationId(2), history }
                if history.commits.len() == 1 && history.commits[0].oid == "a"
        )));
    }

    fn detail_engine() -> Engine {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_detail(Ok(gm_schema::CommitDetail {
            oid: "a".repeat(40),
            subject: "the subject".into(),
            ..gm_schema::CommitDetail::default()
        }));
        Engine::new(Box::new(git))
    }

    fn load_detail(oid: &str) -> CommandKind {
        CommandKind::LoadCommitDetail { oid: oid.into() }
    }

    /// Runs every detail job the engine accepted, as the runner's threads do.
    fn run_detail_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_detail_jobs()
            .into_iter()
            .flat_map(|job| engine.detail_report(job.run()))
            .collect()
    }

    #[test]
    fn a_commit_detail_answers_once_it_is_read() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        let started = engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        assert_eq!(
            states(&started),
            [
                OperationState::Requested,
                OperationState::Gating,
                OperationState::Executing
            ]
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::CommitDetailLoaded { .. }))
        );

        let events = run_detail_jobs(&mut engine);

        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitDetailLoaded { id: OperationId(2), detail } if detail.subject == "the subject"
        )));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    fn stack_engine() -> (Engine, FakeGit) {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let git = FakeGit::new("/r", status);
        git.set_stack(Ok(vec![gm_schema::FileDiff {
            path: "a.txt".into(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        }]));
        (Engine::new(Box::new(git.clone())), git)
    }

    /// A stack request for commit `oid` that lists one file, `path`.
    fn load_file(oid: &str, path: &str) -> CommandKind {
        CommandKind::LoadCommitStack {
            oid: oid.into(),
            files: vec![CommitFile {
                path: path.into(),
                old_path: None,
                change: gm_schema::Change::Modified,
                added: 1,
                deleted: 1,
                binary: false,
            }],
        }
    }

    /// Runs every stack job the engine accepted, as the runner's threads do.
    fn run_stack_jobs(engine: &mut Engine) -> Vec<Event> {
        engine
            .take_stack_jobs()
            .into_iter()
            .flat_map(|job| engine.stack_report(job.run()))
            .collect()
    }

    #[test]
    fn a_commit_stack_answers_once_it_is_read() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        let started = engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        assert!(
            !started
                .iter()
                .any(|e| matches!(e, Event::CommitStackLoaded { .. }))
        );

        let events = run_stack_jobs(&mut engine);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitStackLoaded { id: OperationId(2), diffs } if diffs[0].path == "a.txt"
        )));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }));
    }

    #[test]
    fn a_superseded_commit_stack_is_cancelled_stopped_and_dropped() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let superseding = engine.handle(
            OperationId(3),
            &command(load_file(&"a".repeat(40), "b.txt")),
        );
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let mut jobs = engine.take_stack_jobs().into_iter();
        let stale = jobs.next().unwrap();
        assert!(stale.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.stack_report(stale.run()).is_empty());

        let current = jobs.next().unwrap();
        assert!(!current.stop.load(std::sync::atomic::Ordering::SeqCst));
        let events = engine.stack_report(current.run());
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitStackLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_new_commit_detail_ends_the_stack_read_in_flight() {
        let (mut engine, _) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let job = engine.take_stack_jobs().pop().unwrap();

        let events = engine.handle(OperationId(3), &command(load_detail(&"b".repeat(40))));
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(job.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.stack_report(job.run()).is_empty());
    }

    #[test]
    fn a_commit_stack_writes_nothing_and_stages_nothing() {
        let (mut engine, git) = stack_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        run_stack_jobs(&mut engine);

        assert_eq!(git.writes(), Vec::<&str>::new());
        assert!(!engine.log().iter().any(|entry| matches!(
            entry.kind,
            CommandKind::Stage { .. } | CommandKind::Unstage { .. }
        )));
    }

    #[test]
    fn a_failed_commit_stack_fails_with_gits_message() {
        let (mut engine, git) = stack_engine();
        git.set_stack(Err(GitError("fatal: bad object".into())));
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(load_file(&"a".repeat(40), "a.txt")),
        );
        let events = run_stack_jobs(&mut engine);
        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: Some("fatal: bad object".into()),
        }));
    }
    #[test]
    fn a_superseded_commit_detail_is_cancelled_stopped_and_dropped() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        let superseding = engine.handle(OperationId(3), &command(load_detail(&"b".repeat(40))));
        assert!(superseding.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));

        let mut jobs = engine.take_detail_jobs().into_iter();
        let stale = jobs.next().unwrap();
        assert!(
            stale.stop.load(std::sync::atomic::Ordering::SeqCst),
            "the stale read must be stopped"
        );
        let stale_events = engine.detail_report(stale.run());
        assert!(
            stale_events.is_empty(),
            "the superseded answer must not show: {stale_events:?}"
        );

        let current = jobs.next().unwrap();
        assert!(!current.stop.load(std::sync::atomic::Ordering::SeqCst));
        let events = engine.detail_report(current.run());
        assert!(events.iter().any(|e| matches!(
            e,
            Event::CommitDetailLoaded {
                id: OperationId(3),
                ..
            }
        )));
    }

    #[test]
    fn a_failed_commit_detail_fails_with_gits_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_detail(Err(GitError("fatal: bad object".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));

        let events = run_detail_jobs(&mut engine);

        assert!(events.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: Some("fatal: bad object".into()),
        }));
    }

    #[test]
    fn leaving_the_repository_cancels_the_commit_detail() {
        let mut engine = detail_engine();
        engine.handle(OperationId(1), &open());
        engine.handle(OperationId(2), &command(load_detail(&"a".repeat(40))));
        let job = engine.take_detail_jobs().pop().unwrap();
        let reopened = engine.handle(OperationId(3), &open());
        assert!(reopened.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(job.stop.load(std::sync::atomic::Ordering::SeqCst));
        assert!(engine.detail_report(job.run()).is_empty());
    }

    #[test]
    fn leaving_the_repository_cancels_the_history_load() {
        let mut engine = history_engine(&["c"]);
        engine.handle(OperationId(1), &open());
        engine.handle(
            OperationId(2),
            &command(CommandKind::LoadHistory {
                scope: HistoryScope::default(),
            }),
        );
        let reopened = engine.handle(OperationId(3), &open());
        assert!(reopened.contains(&Event::OperationChanged {
            id: OperationId(2),
            actor: Actor::Human,
            state: OperationState::Cancelled,
            detail: None,
        }));
        assert!(run_jobs(&mut engine).is_empty());
    }

    fn worktree(path: &str, branch: &str) -> gm_schema::WorktreeEntry {
        gm_schema::WorktreeEntry {
            path: path.into(),
            head: Some("abc123".into()),
            branch: Some(branch.into()),
            detached: false,
            bare: false,
            is_main: path == "/r",
            locked: None,
            prunable: None,
        }
    }

    #[test]
    fn list_worktrees_emits_the_worktrees_it_read() {
        let worktrees = vec![worktree("/r", "main"), worktree("/r-feature", "feature")];
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Ok(worktrees.clone()));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::ListWorktrees));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(events.contains(&Event::WorktreesLoaded {
            id: OperationId(2),
            worktrees,
        }));
    }

    #[test]
    fn refresh_follows_the_status_with_the_worktrees() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Ok(vec![worktree("/r", "main")]));
        let mut engine = Engine::new(Box::new(git));

        let opened = engine.handle(OperationId(1), &open());
        assert!(
            opened
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(1)))
        );

        let refreshed = engine.handle(OperationId(2), &command(CommandKind::RefreshStatus));
        assert_eq!(states(&refreshed).last(), Some(&OperationState::Done));
        assert!(
            refreshed
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { id, .. } if *id == OperationId(2)))
        );
        assert!(
            refreshed
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_failed_worktree_list_fails_the_refresh() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_worktrees(Err(GitError("worktree list broke".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(OperationId(2), &command(CommandKind::RefreshStatus));

        assert_eq!(states(&events).last(), Some(&OperationState::Failed));
    }

    #[test]
    fn checkout_reloads_status_so_the_ui_repaints() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let kind = CommandKind::Checkout {
            branch: "feature".into(),
        };

        let events = engine.handle(OperationId(2), &command(kind));

        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_refused_checkout_fails_with_the_git_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_checkout(Err(GitError("local changes would be overwritten".into())));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let kind = CommandKind::Checkout {
            branch: "feature".into(),
        };
        let events = engine.handle(OperationId(2), &command(kind));

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("local changes would be overwritten")
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::StatusLoaded { .. }))
        );
    }

    #[test]
    fn opening_outside_the_repo_fails() {
        let bad = command(CommandKind::OpenRepo {
            path: "/elsewhere".into(),
        });
        let events = engine().handle(OperationId(1), &bad);
        assert_eq!(states(&events).last(), Some(&OperationState::Failed));
    }

    fn create(path: &str, mode: WorktreeMode) -> CommandKind {
        CommandKind::CreateWorktree {
            path: path.into(),
            mode,
        }
    }

    #[test]
    fn create_worktree_hands_the_path_and_mode_to_git_and_reloads_the_list() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        let mut engine = Engine::new(Box::new(git.clone()));
        engine.handle(OperationId(1), &open());
        let mode = WorktreeMode::New {
            branch: "feature".into(),
            base: "main".into(),
        };

        let events = engine.handle(OperationId(2), &command(create("/r-feature", mode.clone())));

        assert_eq!(git.created(), vec![("/r-feature".to_owned(), mode)]);
        assert_eq!(states(&events).last(), Some(&OperationState::Done));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::WorktreesLoaded { id, .. } if *id == OperationId(2)))
        );
    }

    #[test]
    fn a_refused_create_worktree_fails_with_the_git_message() {
        let git = FakeGit::new(
            "/r",
            RepoStatus {
                root: "/r".into(),
                ..RepoStatus::default()
            },
        );
        git.set_create(Err(GitError(
            "'main' is already checked out at '/r'".into(),
        )));
        let mut engine = Engine::new(Box::new(git));
        engine.handle(OperationId(1), &open());

        let events = engine.handle(
            OperationId(2),
            &command(create(
                "/r-main",
                WorktreeMode::Existing {
                    branch: "main".into(),
                },
            )),
        );

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("'main' is already checked out at '/r'")
        );
    }

    #[test]
    fn an_agent_cannot_create_a_worktree() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: create(
                "/r-agent",
                WorktreeMode::Detached {
                    commit: "HEAD".into(),
                },
            ),
        };

        let events = engine.handle(OperationId(2), &command);

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("only a human can create a worktree")
        );
    }

    #[test]
    fn an_agent_cannot_convert_hooks() {
        let mut engine = engine();
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::ConvertHooksToWorkflows,
        };

        let events = engine.handle(OperationId(1), &command);

        let Some(Event::OperationChanged { state, detail, .. }) = events.last() else {
            panic!("no events");
        };
        assert_eq!(*state, OperationState::Failed);
        assert_eq!(
            detail.as_deref(),
            Some("only a human can convert hooks to workflows")
        );
    }

    #[test]
    fn an_agent_cannot_run_a_workflow_yet() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let command = Command {
            actor: Actor::Agent {
                name: "agent".into(),
            },
            kind: CommandKind::RunWorkflow {
                name: "suggest".into(),
                inputs: BTreeMap::new(),
            },
        };

        let events = engine.handle(OperationId(2), &command);

        assert_eq!(
            failure(&events).as_deref(),
            Some("agents run workflows through MCP in a later slice")
        );
    }

    fn failure(events: &[Event]) -> Option<String> {
        events.iter().find_map(|e| match e {
            Event::OperationChanged {
                state: OperationState::Failed,
                detail,
                ..
            } => detail.clone(),
            _ => None,
        })
    }

    #[test]
    fn an_agent_cannot_create_or_delete_a_branch() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let create = Command {
            actor: agent.clone(),
            kind: CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: None,
                checkout: false,
            },
        };
        let events = engine.handle(OperationId(2), &create);
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can create a branch")
        );
        let delete = Command {
            actor: agent,
            kind: CommandKind::DeleteBranch {
                name: "topic".into(),
                force: true,
            },
        };
        let events = engine.handle(OperationId(3), &delete);
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can delete a branch")
        );
    }

    fn stash(index: u32, oid: &str, message: &str) -> gm_schema::StashEntry {
        gm_schema::StashEntry {
            index,
            oid: oid.into(),
            message: message.into(),
            branch: Some("main".into()),
            time: 0,
        }
    }

    /// An engine over a fake whose stash list holds one stash, `a` at index 0.
    fn stash_engine() -> (Engine, FakeGit) {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        fake.set_stashes(Ok(vec![stash(0, &"a".repeat(40), "On main: x")]));
        let mut engine = Engine::new(Box::new(fake.clone()));
        engine.handle(OperationId(1), &open());
        (engine, fake)
    }

    #[test]
    fn an_agent_cannot_change_a_stash_and_a_human_can() {
        let (mut engine, fake) = stash_engine();
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let oid = "a".repeat(40);
        let writes = [
            (
                CommandKind::CreateStash {
                    message: None,
                    include_untracked: true,
                },
                "only a human can create a stash",
            ),
            (
                CommandKind::ApplyStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can apply a stash",
            ),
            (
                CommandKind::PopStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can pop a stash",
            ),
            (
                CommandKind::DropStash {
                    index: 0,
                    oid: oid.clone(),
                },
                "only a human can drop a stash",
            ),
        ];
        for (n, (kind, refused)) in writes.iter().enumerate() {
            let events = engine.handle(
                OperationId(2 + n as u64),
                &Command {
                    actor: agent.clone(),
                    kind: kind.clone(),
                },
            );
            assert_eq!(failure(&events).as_deref(), Some(*refused));
        }
        assert!(fake.writes().is_empty());

        let events = engine.handle(
            OperationId(10),
            &command(CommandKind::DropStash { index: 0, oid }),
        );
        assert_eq!(failure(&events), None);
        assert_eq!(fake.writes(), ["drop_stash"]);
        let listed = events.iter().find_map(|e| match e {
            Event::StashesListed { stashes, .. } => Some(stashes.len()),
            _ => None,
        });
        assert_eq!(listed, Some(1));
    }

    #[test]
    fn a_conflict_read_is_answered_and_never_logged() {
        let (mut engine, fake) = stash_engine();
        let read = ConflictFileRead {
            path: "a.txt".into(),
            operation: gm_schema::ConflictOperation::Merge,
            left: gm_schema::ConflictSide {
                branch: "main".into(),
                role: "current branch".into(),
                subject: "x".into(),
            },
            right: gm_schema::ConflictSide {
                branch: "feature".into(),
                role: "merged branch".into(),
                subject: "y".into(),
            },
            text: "<<<<<<< main\n=======\n>>>>>>> feature\n".into(),
            has_base: false,
            recorded: None,
        };
        fake.set_conflict(Ok(read.clone()));
        let logged = engine.log().len();
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::ReadConflict {
                path: "a.txt".into(),
                site: ConflictSite::WorkingCopy,
            }),
        );
        assert_eq!(failure(&events), None);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ConflictRead { read: got, .. } if *got == read
        )));
        assert_eq!(engine.log().len(), logged);
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn a_conflict_resolution_is_logged_and_only_a_human_may_make_it() {
        let (mut engine, fake) = stash_engine();
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: agent,
                kind: CommandKind::ResolveConflict {
                    path: "a.txt".into(),
                    text: "ok\n".into(),
                    origins: Vec::new(),
                    site: ConflictSite::WorkingCopy,
                },
            },
        );
        assert!(failure(&events).is_some());
        assert!(fake.writes().is_empty());

        let events = engine.handle(
            OperationId(3),
            &command(CommandKind::ResolveConflict {
                path: "a.txt".into(),
                text: "ok\n".into(),
                origins: vec![HunkOrigin::Human, HunkOrigin::Rerere],
                site: ConflictSite::WorkingCopy,
            }),
        );
        assert_eq!(failure(&events), None);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ConflictResolved { path, .. } if path == "a.txt"
        )));
        assert_eq!(fake.writes(), ["resolve_conflict"]);
        // The operation log keeps the origins of each hunk with the command.
        assert!(engine.log().iter().any(|entry| matches!(
            &entry.kind,
            CommandKind::ResolveConflict { origins, .. }
                if *origins == [HunkOrigin::Human, HunkOrigin::Rerere]
        )));
    }

    #[test]
    fn a_recorded_resolution_is_forgotten_only_by_a_human_and_logged() {
        let (mut engine, fake) = stash_engine();
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: agent,
                kind: CommandKind::ForgetRecordedResolution {
                    path: "a.txt".into(),
                    site: ConflictSite::WorkingCopy,
                },
            },
        );
        assert!(failure(&events).is_some());
        assert!(fake.writes().is_empty());

        let events = engine.handle(
            OperationId(3),
            &command(CommandKind::ForgetRecordedResolution {
                path: "a.txt".into(),
                site: ConflictSite::WorkingCopy,
            }),
        );
        assert_eq!(failure(&events), None);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::RecordedResolutionForgotten { path, .. } if path == "a.txt"
        )));
        assert_eq!(fake.writes(), ["forget_recorded_resolution"]);
        assert!(
            engine
                .log()
                .iter()
                .any(|entry| matches!(entry.kind, CommandKind::ForgetRecordedResolution { .. }))
        );
    }

    #[test]
    fn an_agent_may_list_the_stashes() {
        let (mut engine, fake) = stash_engine();
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: Actor::Agent {
                    name: "agent".into(),
                },
                kind: CommandKind::ListStashes,
            },
        );
        assert_eq!(failure(&events), None);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::StashesListed { stashes, .. } if stashes.len() == 1
        )));
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn a_stale_stash_is_refused_before_git_writes() {
        let (mut engine, fake) = stash_engine();
        // The list changed: `stash@{0}` is now another stash.
        fake.set_stashes(Ok(vec![stash(0, &"b".repeat(40), "On main: y")]));
        let stale = "a".repeat(40);
        for (n, kind) in [
            CommandKind::ApplyStash {
                index: 0,
                oid: stale.clone(),
            },
            CommandKind::PopStash {
                index: 0,
                oid: stale.clone(),
            },
            CommandKind::DropStash {
                index: 0,
                oid: stale.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let events = engine.handle(OperationId(2 + n as u64), &command(kind));
            assert_eq!(failure(&events).as_deref(), Some(STASH_LIST_CHANGED));
        }
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn stash_writes_are_logged_and_the_list_is_not() {
        let (mut engine, _fake) = stash_engine();
        engine.handle(OperationId(2), &command(CommandKind::ListStashes));
        assert!(
            engine
                .log()
                .iter()
                .all(|entry| entry.kind != CommandKind::ListStashes)
        );
        engine.handle(
            OperationId(3),
            &command(CommandKind::CreateStash {
                message: Some("note".into()),
                include_untracked: false,
            }),
        );
        assert!(engine.log().iter().any(|entry| {
            entry.id == OperationId(3) && matches!(entry.kind, CommandKind::CreateStash { .. })
        }));
    }

    #[test]
    fn an_agent_cannot_discard_a_file_and_a_human_can() {
        let status = RepoStatus {
            root: "/r".into(),
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        let mut engine = Engine::new(Box::new(fake.clone()));
        engine.handle(OperationId(1), &open());
        let agent = Actor::Agent {
            name: "agent".into(),
        };
        let discard = CommandKind::DiscardFile {
            path: "f.txt".into(),
            include_staged: true,
        };
        let events = engine.handle(
            OperationId(2),
            &Command {
                actor: agent,
                kind: discard.clone(),
            },
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("only a human can discard changes")
        );
        assert!(fake.writes().is_empty());
        let events = engine.handle(OperationId(3), &command(discard));
        assert_eq!(failure(&events), None);
        assert_eq!(fake.writes(), ["discard_file"]);
    }

    #[test]
    fn an_invalid_branch_name_is_refused_before_git_runs() {
        let mut engine = engine();
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::CreateBranch {
                name: "a b".into(),
                start_point: None,
                checkout: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("A branch name cannot contain spaces.")
        );
        let events = engine.handle(
            OperationId(3),
            &command(CommandKind::CreateBranch {
                name: "topic".into(),
                start_point: Some("--all".into()),
                checkout: true,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Choose a branch, tag or commit to start from.")
        );
        let events = engine.handle(
            OperationId(4),
            &command(CommandKind::DeleteBranch {
                name: "x..y".into(),
                force: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("A branch name cannot contain '..'.")
        );
    }

    #[test]
    fn the_checked_out_branch_is_refused_before_git_runs() {
        let status = RepoStatus {
            root: "/r".into(),
            branch: gm_schema::BranchInfo {
                head: Some("main".into()),
                ..gm_schema::BranchInfo::default()
            },
            ..RepoStatus::default()
        };
        let mut engine = Engine::new(Box::new(FakeGit::new("/r", status)));
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::DeleteBranch {
                name: "main".into(),
                force: true,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Cannot delete the checked-out branch main.")
        );
    }

    #[test]
    fn a_branch_checked_out_in_another_worktree_is_refused_with_its_path() {
        let status = RepoStatus {
            root: "/r".into(),
            branch: gm_schema::BranchInfo {
                head: Some("main".into()),
                ..gm_schema::BranchInfo::default()
            },
            ..RepoStatus::default()
        };
        let fake = FakeGit::new("/r", status);
        fake.set_worktrees(Ok(vec![
            worktree("/r", "main"),
            worktree("/r-feature", "feature"),
        ]));
        let mut engine = Engine::new(Box::new(fake));
        engine.handle(OperationId(1), &open());
        let events = engine.handle(
            OperationId(2),
            &command(CommandKind::DeleteBranch {
                name: "feature".into(),
                force: false,
            }),
        );
        assert_eq!(
            failure(&events).as_deref(),
            Some("Cannot delete feature: it is checked out in the worktree at /r-feature.")
        );
    }

    fn any_kind() -> impl Strategy<Value = CommandKind> {
        prop_oneof![
            Just(CommandKind::RefreshStatus),
            prop_oneof![Just("/r"), Just("/r/sub"), Just("/x")]
                .prop_map(|p| CommandKind::OpenRepo { path: p.into() }),
        ]
    }

    fn run(kinds: &[CommandKind]) -> (Vec<LogEntry>, Vec<Event>) {
        let mut engine = engine();
        let mut all = Vec::new();
        for (n, kind) in kinds.iter().enumerate() {
            all.extend(engine.handle(OperationId(n as u64 + 1), &command(kind.clone())));
        }
        (engine.log().to_vec(), all)
    }

    proptest! {
        #[test]
        fn same_commands_give_the_same_log(kinds in proptest::collection::vec(any_kind(), 0..12)) {
            prop_assert_eq!(run(&kinds), run(&kinds));
        }

        #[test]
        fn every_operation_ends_in_one_terminal_state(kinds in proptest::collection::vec(any_kind(), 1..12)) {
            let (log, _) = run(&kinds);
            for n in 1..=kinds.len() as u64 {
                let own: Vec<_> = log.iter().filter(|e| e.id == OperationId(n)).collect();
                // A read is not logged (D28), so it has no entry to check.
                if own.is_empty() {
                    continue;
                }
                for pair in own.windows(2) {
                    prop_assert!(pair[0].state.can_move_to(pair[1].state));
                }
                prop_assert!(own.last().unwrap().state.is_terminal());
                prop_assert_eq!(own.iter().filter(|e| e.state.is_terminal()).count(), 1);
            }
        }
    }
}
