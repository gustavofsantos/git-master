//! The Workflows page's model (docs/05, D76): which workflow is open, the
//! buffer typed into its file and what the file held when it was loaded or
//! last saved, the live diagnostics of the buffer, the save in flight, the Flow
//! rows of the parsed file, and the runs of the open workflow. Plain data and
//! pure functions; the screen is in `shell_workflows`, and the engine's answers
//! reach this model through `Model::apply`.

use std::collections::BTreeSet;

use gm_core::workflow::catalog::{self, DEFAULTED, Level};
use gm_core::workflow::edit::{self, Direction, EditError};
use gm_core::workflow::model::{Definition, End, State, StateKind, Target};
use gm_core::workflow::{Diagnostic, Severity, parse, validate};
use gm_schema::{
    CommandKind, OperationId, OperationState, WorkflowDryRun, WorkflowInfo, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunSummary, WorkflowStepRecord,
};
use serde_json::{Value, json};

/// How many runs the Runs tab asks the engine for.
pub const RUN_LIMIT: u32 = 50;

/// How many lines of a JSON block show before "Show all".
pub const SHOWN_LINES: usize = 6;

/// The three tabs of an open workflow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Flow,
    Toml,
    Runs,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Flow, Tab::Toml, Tab::Runs];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Tab::Flow => "Flow",
            Tab::Toml => "TOML",
            Tab::Runs => "Runs",
        }
    }
}

/// Where the last save stands. `Saving` lasts while the engine writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SaveState {
    #[default]
    Idle,
    Saving,
    Failed(String),
    /// The file changed on disk since it was loaded: Reload or Overwrite.
    Conflict,
}

/// The engine's words for a save that found the file changed.
const CONFLICT_TEXT: &str = "changed on disk";

/// A structural change the Flow's controls ask for. It is applied to the buffer
/// through `gm_core::workflow::edit`, after the selected state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowEdit {
    /// A new state after the selected one, or at the end when none is selected.
    Add,
    Remove,
    Move(Direction),
}

/// Where the Flow's selection stands, for the action rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowPosition {
    /// The buffer parses, so the Flow's controls can edit it.
    pub parses: bool,
    /// How many top-level states the file has.
    pub states: usize,
    /// The index of the selected top-level state.
    pub selected: Option<usize>,
}

/// The target picker open over one edge of the selected state.
#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub state: String,
    pub outcome: String,
    /// The transition of `on.<outcome>` the choice sets.
    pub index: usize,
    /// The pointer's position in window pixels, where the panel opens.
    pub at: (f32, f32),
}

/// What the picker sets on its edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A state or an end, as the file writes it.
    Target(String),
    /// The outcome goes back to its default: its rule is removed (D78).
    Default,
}

/// The last dry run answered: the text it walked and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DryRunAnswer {
    source: String,
    result: WorkflowDryRun,
}

/// The workflow open on the page: its file, the buffer and what the file held.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    path: String,
    name: String,
    /// The text on disk as the page last loaded or saved it. The buffer is
    /// dirty while it differs from this.
    baseline: String,
    /// The buffer, as typed.
    text: String,
    /// The hash of the file the buffer was loaded from; a save expects it.
    loaded_hash: String,
    /// The file as the latest listing read it, with its hash: what Reload and
    /// Overwrite take.
    disk_source: String,
    disk_hash: String,
}

/// The Workflows page's state. Reset when the repository changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowsPage {
    open: Option<Open>,
    tab: Tab,
    definition: Option<Definition>,
    diagnostics: Vec<Diagnostic>,
    save: SaveState,
    /// The save the engine is writing: its operation and the text it writes.
    saving: Option<(OperationId, String)>,
    /// The text of a save request not yet sent; `track_save` takes it.
    pending_save: Option<String>,
    /// The path of the workflow the user asked for while the buffer was dirty.
    asked_switch: Option<String>,
    /// Whether the delete confirmation is showing.
    asking_delete: bool,
    /// The runs of the open workflow, once read.
    runs: Option<Vec<WorkflowRunSummary>>,
    runs_failed: bool,
    runs_op: Option<OperationId>,
    /// The run whose detail is shown, and the read of it in flight.
    run: Option<i64>,
    detail: Option<WorkflowRunDetail>,
    run_op: Option<OperationId>,
    /// The JSON blocks the user expanded with "Show all", by `expand_key`.
    expanded: BTreeSet<String>,
    /// The top-level state the Flow's controls act on.
    selected: Option<String>,
    /// Why the last Flow edit was refused; the buffer is unchanged then.
    flow_error: Option<String>,
    /// The target picker open over an edge of the selected state.
    picker: Option<Picker>,
    /// The dry run asked for and not sent yet: the text it walks.
    pending_dry_run: Option<String>,
    /// The dry run the engine is computing: its operation and the text it walks.
    dry_running: Option<(OperationId, String)>,
    /// The last dry run answered.
    dry_run: Option<DryRunAnswer>,
}

/// What the window's action rules need to know of the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageState<'a> {
    /// Whether the Workflows page is the main screen.
    pub shown: bool,
    /// The name of the open workflow.
    pub name: Option<&'a str>,
    pub dirty: bool,
    pub saving: bool,
    /// The Flow's selection and the file's states.
    pub flow: FlowPosition,
}

impl WorkflowsPage {
    /// Forgets everything: the repository being left had its own workflows.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.path.as_str())
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.name.as_str())
    }

    /// The buffer; empty when no workflow is open.
    #[must_use]
    pub fn text(&self) -> &str {
        self.open.as_ref().map_or("", |open| open.text.as_str())
    }

    /// Whether the buffer holds text the file on disk does not.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.text != open.baseline)
    }

    #[must_use]
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The parsed file, when it parses. Errors in it still leave it here.
    #[must_use]
    pub fn definition(&self) -> Option<&Definition> {
        self.definition.as_ref()
    }

    /// Every diagnostic of the buffer, errors first, then by line.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn save_state(&self) -> &SaveState {
        &self.save
    }

    #[must_use]
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    #[must_use]
    pub fn asked_switch(&self) -> Option<&str> {
        self.asked_switch.as_deref()
    }

    #[must_use]
    pub fn asking_delete(&self) -> bool {
        self.asking_delete
    }

    /// The runs read for the open workflow; `None` until the first answer.
    #[must_use]
    pub fn runs(&self) -> Option<&[WorkflowRunSummary]> {
        self.runs.as_deref()
    }

    #[must_use]
    pub fn runs_failed(&self) -> bool {
        self.runs_failed
    }

    #[must_use]
    pub fn selected_run(&self) -> Option<i64> {
        self.run
    }

    #[must_use]
    pub fn detail(&self) -> Option<&WorkflowRunDetail> {
        self.detail.as_ref()
    }

    #[must_use]
    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }

    /// Shows the JSON block `key` in full, or folds it back.
    pub fn toggle_expanded(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
    }

    /// What the window's action rules read of the page; `shown` says whether
    /// the page is the main screen.
    #[must_use]
    pub fn state(&self, shown: bool) -> PageState<'_> {
        PageState {
            shown,
            name: self.name(),
            dirty: self.dirty(),
            saving: self.is_saving(),
            flow: self.flow_position(),
        }
    }

    /// The top-level state the Flow's controls act on.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Why the last Flow edit was refused, as the engine's parser or editor words it.
    #[must_use]
    pub fn flow_error(&self) -> Option<&str> {
        self.flow_error.as_deref()
    }

    #[must_use]
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// The top-level state ids of the parsed file, in file order.
    fn top_level_ids(&self) -> Vec<&str> {
        self.definition
            .as_ref()
            .map_or_else(Vec::new, |definition| {
                definition
                    .states
                    .iter()
                    .map(|state| state.id.as_str())
                    .collect()
            })
    }

    fn flow_position(&self) -> FlowPosition {
        let ids = self.top_level_ids();
        FlowPosition {
            parses: self.definition.is_some(),
            states: ids.len(),
            selected: self
                .selected
                .as_deref()
                .and_then(|id| ids.iter().position(|known| *known == id)),
        }
    }

    /// Selects the top-level state `id`. Another state than the selected one
    /// closes the picker; a click on the selected state leaves it open.
    pub fn select_state(&mut self, id: &str) {
        if self.selected() == Some(id) || !self.top_level_ids().contains(&id) {
            return;
        }
        self.selected = Some(id.to_owned());
        self.picker = None;
    }

    /// Applies a Flow edit to the buffer as one change, and selects what the
    /// edit leaves selected: the new state after an add, the neighbour after a
    /// remove. A refused edit keeps the buffer and sets `flow_error`. Returns
    /// whether the buffer changed.
    pub fn apply_flow_edit(&mut self, edit: FlowEdit) -> bool {
        let Some(text) = self.open.as_ref().map(|open| open.text.clone()) else {
            return false;
        };
        let selected = self.selected.clone();
        let result = match (edit, selected.as_deref()) {
            (FlowEdit::Add, _) => edit::fresh_id(&text, "step").and_then(|id| {
                let edited = edit::add_state(&text, selected.as_deref(), &id, "noop")?;
                Ok((edited, Some(id)))
            }),
            (FlowEdit::Remove, Some(id)) => {
                let ids: Vec<String> = self
                    .top_level_ids()
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                edit::remove_state(&text, id)
                    .map(|edited| (edited, neighbour_after_removal(&ids, id)))
            }
            (FlowEdit::Move(direction), Some(id)) => {
                edit::move_state(&text, id, direction).map(|edited| (edited, Some(id.to_owned())))
            }
            (FlowEdit::Remove | FlowEdit::Move(_), None) => Err(no_selection()),
        };
        self.commit(result)
    }

    /// Opens the target picker over transition `index` of `on.<outcome>` in the
    /// selected state `state`, under the pointer. Only the selected state has one.
    pub fn open_picker(&mut self, state: &str, outcome: &str, index: usize, at: (f32, f32)) {
        if self.selected() != Some(state) {
            return;
        }
        self.picker = Some(Picker {
            state: state.to_owned(),
            outcome: outcome.to_owned(),
            index,
            at,
        });
    }

    pub fn close_picker(&mut self) {
        self.picker = None;
    }

    /// The targets the picker of `state` offers; see `target_options`.
    #[must_use]
    pub fn target_options(&self, state: &str) -> Vec<String> {
        self.definition
            .as_ref()
            .map_or_else(Vec::new, |definition| target_options(definition, state))
    }

    /// Sets the picker's edge to `choice` as one change, and closes the picker.
    /// Returns whether the buffer changed.
    pub fn pick_target(&mut self, choice: &Choice) -> bool {
        let Some(picker) = self.picker.take() else {
            return false;
        };
        let Some(text) = self.open.as_ref().map(|open| open.text.clone()) else {
            return false;
        };
        let result = match choice {
            Choice::Target(target) => {
                edit::set_target(&text, &picker.state, &picker.outcome, picker.index, target)
            }
            Choice::Default => edit::clear_outcome(&text, &picker.state, &picker.outcome),
        };
        let selection = self.selected.clone();
        self.commit(result.map(|edited| (edited, selection)))
    }

    /// Takes an edit's text: the buffer becomes it and the selection it names.
    /// A refused edit only sets `flow_error`. Returns whether the buffer changed.
    fn commit(&mut self, result: Result<(String, Option<String>), EditError>) -> bool {
        match result {
            Ok((text, selection)) => {
                self.flow_error = None;
                self.selected = selection;
                self.picker = None;
                let changed = self.open.as_ref().is_some_and(|open| open.text != text);
                if let Some(open) = self.open.as_mut() {
                    open.text = text;
                }
                self.analyse();
                changed
            }
            Err(EditError(message)) => {
                self.flow_error = Some(message);
                false
            }
        }
    }

    /// Asks for a dry run of the buffer, which must parse. The request is
    /// tracked by `track_dry_run` once the window has sent it.
    pub fn dry_run_request(&mut self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        self.definition.as_ref()?;
        self.pending_dry_run = Some(open.text.clone());
        Some(CommandKind::DryRunWorkflow {
            source: open.text.clone(),
            inputs: json!({}),
        })
    }

    /// Records the operation of the dry run the last request sent.
    pub fn track_dry_run(&mut self, id: OperationId) {
        if let Some(source) = self.pending_dry_run.take() {
            self.dry_running = Some((id, source));
        }
    }

    /// The dry run answer, when it answers the one in flight. Another
    /// operation's answer is dropped.
    pub fn dry_run_answered(&mut self, id: OperationId, result: WorkflowDryRun) {
        match self.dry_running.take() {
            Some((op, source)) if op == id => {
                self.dry_run = Some(DryRunAnswer { source, result });
            }
            other => self.dry_running = other,
        }
    }

    /// The last dry run, when it walked the buffer as it is now.
    #[must_use]
    pub fn dry_run_result(&self) -> Option<&WorkflowDryRun> {
        let answer = self.dry_run.as_ref()?;
        (self.open.as_ref().map(|open| open.text.as_str()) == Some(answer.source.as_str()))
            .then_some(&answer.result)
    }

    /// Whether a dry run answered before the buffer changed.
    #[must_use]
    pub fn dry_run_stale(&self) -> bool {
        self.dry_run.is_some() && self.dry_run_result().is_none()
    }

    #[must_use]
    pub fn dry_run_pending(&self) -> bool {
        self.dry_running.is_some()
    }

    /// Closes the dry run panel.
    pub fn dismiss_dry_run(&mut self) {
        self.dry_run = None;
    }

    /// Follows the engine's listing. The open file takes the listed hash and
    /// source; while the buffer is clean, the buffer takes the listed source
    /// too, so an outside edit shows. A file the listing no longer has closes.
    pub fn listed(&mut self, workflows: &[WorkflowInfo]) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(info) = workflows.iter().find(|info| info.path == open.path) else {
            self.close();
            return;
        };
        open.name = info.name.clone();
        open.disk_hash = info.hash.clone();
        open.disk_source = info.source.clone();
        if open.text == open.baseline {
            open.baseline = info.source.clone();
            open.text = info.source.clone();
            open.loaded_hash = info.hash.clone();
            self.analyse();
        }
    }

    /// Opens the workflow at `path` from the listing. With unsaved changes to
    /// another file, it asks first and returns `false`.
    pub fn select(&mut self, path: &str, workflows: &[WorkflowInfo]) -> bool {
        if self.path() == Some(path) {
            return true;
        }
        let Some(info) = workflows.iter().find(|info| info.path == path) else {
            return false;
        };
        if self.dirty() {
            self.asked_switch = Some(path.to_owned());
            return false;
        }
        self.open_file(info);
        true
    }

    /// The answer to "Unsaved changes": drops the buffer and opens the file asked for.
    pub fn discard_and_switch(&mut self, workflows: &[WorkflowInfo]) {
        if let Some(path) = self.asked_switch.take()
            && let Some(info) = workflows.iter().find(|info| info.path == path)
        {
            self.open_file(info);
        }
    }

    /// The answer to "Unsaved changes": keeps the buffer and the open file.
    pub fn keep_editing(&mut self) {
        self.asked_switch = None;
    }

    /// Replaces the buffer with what the user typed.
    pub fn edit(&mut self, text: &str) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.text == text {
            return;
        }
        text.clone_into(&mut open.text);
        self.analyse();
    }

    /// Throws the unsaved changes away: the buffer goes back to the file on disk as loaded.
    pub fn revert(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.text = open.baseline.clone();
            self.save = SaveState::Idle;
        }
        self.analyse();
    }

    /// Takes the file as the listing last read it, dropping the buffer.
    pub fn reload(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.text = open.disk_source.clone();
            open.baseline = open.disk_source.clone();
            open.loaded_hash = open.disk_hash.clone();
            self.save = SaveState::Idle;
        }
        self.analyse();
    }

    /// Shows `tab`. The Runs tab asks for the open workflow's runs; the request
    /// comes back for the window to send and to track.
    pub fn show_tab(&mut self, tab: Tab) -> Option<CommandKind> {
        self.tab = tab;
        match tab {
            Tab::Runs => self.runs_request(),
            Tab::Flow | Tab::Toml => None,
        }
    }

    /// The request for the open workflow's newest runs.
    #[must_use]
    pub fn runs_request(&self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        Some(CommandKind::ListWorkflowRuns {
            workflow: open.name.clone(),
            limit: RUN_LIMIT,
        })
    }

    /// Records the operation that answers the runs request.
    pub fn track_runs(&mut self, id: OperationId) {
        self.runs_op = Some(id);
        self.runs_failed = false;
    }

    /// The runs answer, when it is the request in flight for the open workflow.
    pub fn runs_listed(&mut self, id: OperationId, workflow: &str, runs: Vec<WorkflowRunSummary>) {
        if self.runs_op != Some(id) || self.name() != Some(workflow) {
            return;
        }
        self.runs_op = None;
        self.runs = Some(runs);
    }

    /// Selects a run of the Runs list and returns the read of its detail.
    pub fn open_run(&mut self, run: i64) -> Option<CommandKind> {
        self.open.as_ref()?;
        self.run = Some(run);
        self.detail = None;
        Some(CommandKind::LoadWorkflowRun { run })
    }

    /// Records the operation that answers a run's detail read.
    pub fn track_run(&mut self, id: OperationId) {
        self.run_op = Some(id);
    }

    /// The detail answer, when it is the read in flight for the selected run.
    pub fn run_loaded(&mut self, id: OperationId, detail: WorkflowRunDetail) {
        if self.run_op != Some(id) || self.run != Some(detail.summary.run) {
            return;
        }
        self.run_op = None;
        self.detail = Some(detail);
    }

    /// The read to send again when the selected run ended, so its steps are complete.
    #[must_use]
    pub fn refresh_run(&self, run: i64) -> Option<CommandKind> {
        (self.run == Some(run)).then_some(CommandKind::LoadWorkflowRun { run })
    }

    /// The save the buffer asks for. Without `overwrite` it expects the hash the
    /// buffer was loaded from, and needs unsaved changes; with it, it expects
    /// the hash now on disk. The text is tracked by `track_save` once sent.
    pub fn save_request(&mut self, overwrite: bool) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        if self.saving.is_some() || (!overwrite && !self.dirty()) {
            return None;
        }
        let expected = if overwrite {
            open.disk_hash.clone()
        } else {
            open.loaded_hash.clone()
        };
        self.pending_save = Some(open.text.clone());
        Some(CommandKind::SaveWorkflowFile {
            path: open.path.clone(),
            source: open.text.clone(),
            expected_hash: (!expected.is_empty()).then_some(expected),
        })
    }

    /// Records the operation that writes the file the last save request sent.
    pub fn track_save(&mut self, id: OperationId) {
        if let Some(text) = self.pending_save.take() {
            self.saving = Some((id, text));
            self.save = SaveState::Saving;
        }
    }

    /// The file was written: it now holds the text that was saved, with `hash`.
    pub fn saved(&mut self, id: OperationId, hash: &str) {
        let Some((op, text)) = self.saving.take() else {
            return;
        };
        if op != id {
            self.saving = Some((op, text));
            return;
        }
        if let Some(open) = self.open.as_mut() {
            open.baseline.clone_from(&text);
            hash.clone_into(&mut open.loaded_hash);
            hash.clone_into(&mut open.disk_hash);
            open.disk_source = text;
        }
        self.save = SaveState::Idle;
    }

    /// The save operation ended without writing. A conflict is the engine's
    /// "changed on disk"; any other failure keeps its reason.
    pub fn operation_ended(
        &mut self,
        id: OperationId,
        state: OperationState,
        detail: Option<&str>,
    ) {
        if !state.is_terminal() {
            return;
        }
        if self.runs_op == Some(id) {
            self.runs_op = None;
            self.runs_failed = state == OperationState::Failed;
        }
        if self.run_op == Some(id) {
            self.run_op = None;
        }
        // A done dry run may still have its answer to come, so only a failure clears it.
        if state != OperationState::Done && self.dry_running.as_ref().map(|(op, _)| *op) == Some(id)
        {
            self.dry_running = None;
        }
        // A done save waits for its `WorkflowFileSaved`, which carries the new hash.
        if state == OperationState::Done || self.saving.as_ref().map(|(op, _)| *op) != Some(id) {
            return;
        }
        self.saving = None;
        self.save = match (state, detail) {
            (OperationState::Failed, Some(text)) if is_conflict(text) => SaveState::Conflict,
            (OperationState::Failed, Some(text)) => SaveState::Failed(text.to_owned()),
            (OperationState::Failed, None) => {
                SaveState::Failed("The file could not be saved.".to_owned())
            }
            _ => SaveState::Idle,
        };
    }

    /// The delete confirmation opens for the open workflow.
    pub fn ask_delete(&mut self) {
        if self.open.is_some() {
            self.asking_delete = true;
        }
    }

    /// Closes the delete confirmation without deleting.
    pub fn cancel_delete(&mut self) {
        self.asking_delete = false;
    }

    /// The request that moves the open file to the trash, expecting the hash it
    /// was loaded from. Closes the confirmation.
    pub fn delete_request(&mut self) -> Option<CommandKind> {
        let open = self.open.as_ref()?;
        let expected_hash = open.loaded_hash.clone();
        let path = open.path.clone();
        self.asking_delete = false;
        Some(CommandKind::DeleteWorkflowFile {
            path,
            expected_hash,
        })
    }

    /// The engine moved `path` to the trash: a page on it closes.
    pub fn deleted(&mut self, path: &str) {
        if self.path() == Some(path) {
            self.close();
        }
    }

    fn open_file(&mut self, info: &WorkflowInfo) {
        let tab = self.tab;
        *self = Self::default();
        self.tab = tab;
        self.open = Some(Open {
            path: info.path.clone(),
            name: info.name.clone(),
            baseline: info.source.clone(),
            text: info.source.clone(),
            loaded_hash: info.hash.clone(),
            disk_source: info.source.clone(),
            disk_hash: info.hash.clone(),
        });
        self.analyse();
    }

    /// Closes the open workflow and keeps the tab.
    fn close(&mut self) {
        let tab = self.tab;
        *self = Self::default();
        self.tab = tab;
    }

    /// Parses the buffer and keeps its diagnostics. A file that parses no longer
    /// has the selected state, or none, drops the selection and the picker; a
    /// file that does not parse keeps them, so a typo does not lose the place.
    fn analyse(&mut self) {
        let text = self.text().to_owned();
        let (definition, diagnostics) = analyse(&text);
        self.definition = definition;
        self.diagnostics = diagnostics;
        if self.definition.is_some() {
            let known = self
                .selected
                .as_deref()
                .is_some_and(|id| self.top_level_ids().contains(&id));
            if !known {
                self.selected = None;
                self.picker = None;
            }
        }
    }
}

/// The state the Flow selects after `removed` goes: the one after it, or the
/// one before it when it was last.
fn neighbour_after_removal(ids: &[String], removed: &str) -> Option<String> {
    let index = ids.iter().position(|id| id == removed)?;
    ids.get(index + 1)
        .or_else(|| index.checked_sub(1).and_then(|before| ids.get(before)))
        .cloned()
}

fn no_selection() -> EditError {
    EditError("Select a state in the Flow first.".to_owned())
}

/// The targets a state's picker offers: the other top-level states, then
/// `end.done` and `end.failed`, and `end.blocked` only in a `before.*` phase,
/// where a run can veto.
#[must_use]
pub fn target_options(definition: &Definition, state: &str) -> Vec<String> {
    let mut options: Vec<String> = definition
        .states
        .iter()
        .map(|known| known.id.clone())
        .filter(|id| id != state)
        .collect();
    options.push("end.done".to_owned());
    options.push("end.failed".to_owned());
    if definition.trigger.is_before() {
        options.push("end.blocked".to_owned());
    }
    options
}

/// Whether a save's failure text is the engine's conflict answer.
#[must_use]
pub fn is_conflict(text: &str) -> bool {
    text.to_ascii_lowercase().contains(CONFLICT_TEXT)
}

/// Parses and checks a workflow file as the editor reads it. The definition is
/// kept even when validation finds errors, so the Flow can still show the
/// states; a file that does not parse gives only its one diagnostic. Errors
/// come first, then the rest, each by line.
#[must_use]
pub fn analyse(source: &str) -> (Option<Definition>, Vec<Diagnostic>) {
    let (definition, mut diagnostics) = match parse::parse(source) {
        Err(diagnostic) => (None, vec![diagnostic]),
        Ok(definition) => {
            let diagnostics = validate::validate(&definition);
            (Some(definition), diagnostics)
        }
    };
    diagnostics.sort_by_key(|d| (d.severity != Severity::Error, d.line));
    (definition, diagnostics)
}

/// The text of one diagnostic in the editor: `line N: message`.
#[must_use]
pub fn diagnostic_text(diagnostic: &Diagnostic) -> String {
    if diagnostic.line == 0 {
        diagnostic.message.clone()
    } else {
        format!("line {}: {}", diagnostic.line, diagnostic.message)
    }
}

/// One state of the Flow, with its nesting depth in a `for_each`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowRow {
    pub depth: usize,
    /// The line of the state's `id` in the file.
    pub line: usize,
    pub id: String,
    /// The action's name, `wait <event>`, or `for_each`.
    pub kind: String,
    /// The effect level badge of an action: `read`, `propose`, `apply` or `approve`.
    pub level: Option<&'static str>,
    pub edges: Vec<Edge>,
    /// The diagnostics whose line falls inside this state.
    pub diagnostics: Vec<Diagnostic>,
}

/// One line of a state's outcomes: `outcome → target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub outcome: String,
    /// The transition's place in `on.<outcome>`, for the picker; zero for a
    /// default or an unhandled outcome.
    pub index: usize,
    pub target: String,
    pub guard: Option<String>,
    pub limit: Option<u32>,
    /// The short form (D78): the file writes no rule for this outcome.
    pub defaulted: bool,
    /// The outcome has no rule and no default: the run would stop on it.
    pub dangling: bool,
}

impl Edge {
    /// The line the Flow shows for this edge.
    #[must_use]
    pub fn label(&self) -> String {
        let mut text = format!("{} → {}", self.outcome, self.target);
        if let Some(guard) = &self.guard {
            text = format!("{text} if {guard}");
        }
        if let Some(limit) = self.limit {
            text = format!("{text} (limit {limit})");
        }
        text
    }
}

/// The Flow rows of a parsed file, in file order, nested `for_each` states
/// after their parent. Diagnostics go to the state whose line is the last one
/// at or before theirs; the ones with no such state come back as the second
/// value.
#[must_use]
pub fn flow_rows(
    definition: &Definition,
    diagnostics: &[Diagnostic],
) -> (Vec<FlowRow>, Vec<Diagnostic>) {
    let mut rows = Vec::new();
    push_states(&definition.states, 0, &mut rows);
    let mut unplaced = Vec::new();
    for diagnostic in diagnostics {
        let owner = rows
            .iter_mut()
            .filter(|row| diagnostic.line > 0 && row.line <= diagnostic.line)
            .max_by_key(|row| row.line);
        match owner {
            Some(row) => row.diagnostics.push(diagnostic.clone()),
            None => unplaced.push(diagnostic.clone()),
        }
    }
    (rows, unplaced)
}

fn push_states(states: &[State], depth: usize, rows: &mut Vec<FlowRow>) {
    for (index, state) in states.iter().enumerate() {
        let next = states
            .get(index + 1)
            .map_or_else(|| "end.done".to_owned(), |next| next.id.clone());
        let (kind, level, outcomes): (String, Option<&'static str>, Vec<&str>) = match &state.kind {
            StateKind::Action { action, .. } => {
                let spec = catalog::action(action);
                (
                    action.clone(),
                    spec.map(|spec| level_label(spec.level)),
                    spec.map_or_else(Vec::new, |spec| spec.outcomes.to_vec()),
                )
            }
            StateKind::Wait { event, timeout } => {
                let mut outcomes = catalog::wait_outcomes(event).unwrap_or(&[]).to_vec();
                if timeout.is_some() && !outcomes.contains(&"timeout") {
                    outcomes.push("timeout");
                }
                (format!("wait {event}"), None, outcomes)
            }
            StateKind::ForEach { .. } => ("for_each".to_owned(), None, vec!["done", "failed"]),
        };
        let edges = edges(state, &outcomes, &next);
        rows.push(FlowRow {
            depth,
            line: state.line,
            id: state.id.clone(),
            kind,
            level,
            edges,
            diagnostics: Vec::new(),
        });
        if let StateKind::ForEach { states, .. } = &state.kind {
            push_states(states, depth + 1, rows);
        }
    }
}

/// The edges of a state: every outcome its action or wait declares, then any
/// outcome the file names that the catalog does not list.
fn edges(state: &State, declared: &[&str], next: &str) -> Vec<Edge> {
    let mut outcomes: Vec<&str> = declared.to_vec();
    for rule in &state.on {
        if !outcomes.contains(&rule.outcome.as_str()) {
            outcomes.push(rule.outcome.as_str());
        }
    }
    let mut edges = Vec::new();
    for outcome in outcomes {
        match state.rule(outcome) {
            Some(rule) if !rule.transitions.is_empty() => {
                for (index, transition) in rule.transitions.iter().enumerate() {
                    edges.push(Edge {
                        outcome: outcome.to_owned(),
                        index,
                        target: target_text(&transition.to),
                        guard: transition.guard.clone(),
                        limit: transition.limit,
                        defaulted: false,
                        dangling: false,
                    });
                }
            }
            _ if DEFAULTED.contains(&outcome) => {
                let target = if outcome == "done" {
                    next
                } else {
                    "end.failed"
                };
                edges.push(Edge {
                    outcome: outcome.to_owned(),
                    index: 0,
                    target: target.to_owned(),
                    guard: None,
                    limit: None,
                    defaulted: true,
                    dangling: false,
                });
            }
            _ => edges.push(Edge {
                outcome: outcome.to_owned(),
                index: 0,
                target: "unhandled".to_owned(),
                guard: None,
                limit: None,
                defaulted: false,
                dangling: true,
            }),
        }
    }
    edges
}

fn target_text(target: &Target) -> String {
    match target {
        Target::State(id) => id.clone(),
        Target::End(End::Done) => "end.done".to_owned(),
        Target::End(End::Failed) => "end.failed".to_owned(),
        Target::End(End::Blocked) => "end.blocked".to_owned(),
    }
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "apply",
        Level::Pause => "approve",
    }
}

/// The text of a run's status, as the Runs tab shows it.
#[must_use]
pub fn status_label(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "Running",
        WorkflowRunStatus::Waiting => "Waiting",
        WorkflowRunStatus::AwaitingApproval => "Awaiting approval",
        WorkflowRunStatus::Done => "Done",
        WorkflowRunStatus::Failed => "Failed",
        WorkflowRunStatus::Blocked => "Blocked",
        WorkflowRunStatus::Cancelled => "Cancelled",
    }
}

/// A span in milliseconds, as a short word: `850 ms`, `3.2 s`, `2 min 4 s`.
#[must_use]
pub fn duration_label(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        // Tenths of a second, rounded, without floating point.
        let tenths = (ms + 50) / 100;
        format!("{}.{} s", tenths / 10, tenths % 10)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

/// How long a run ran, when it has ended.
#[must_use]
pub fn run_duration(run: &WorkflowRunSummary) -> Option<i64> {
    run.ended_at.map(|ended| ended - run.started_at)
}

/// How long a step took, when it has ended.
#[must_use]
pub fn step_duration(step: &WorkflowStepRecord) -> Option<i64> {
    step.ended_at.map(|ended| ended - step.started_at)
}

/// The step's outcome and where the run went: `done → next`, or `running` while
/// the step has not answered.
#[must_use]
pub fn outcome_line(step: &WorkflowStepRecord) -> String {
    match (&step.outcome, &step.next) {
        (Some(outcome), Some(next)) => format!("{outcome} → {next}"),
        (Some(outcome), None) => outcome.clone(),
        (None, _) => "no outcome yet".to_owned(),
    }
}

/// The "visit n" note of a step that ran more than once in the run.
#[must_use]
pub fn visit_label(visit: u32) -> Option<String> {
    (visit > 1).then(|| format!("visit {visit}"))
}

/// Pretty JSON for a context value or a step's input or output.
#[must_use]
pub fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// The part of a JSON block the page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub text: String,
    /// How many lines are folded away; zero when the block is whole.
    pub hidden: usize,
}

/// A block's text, folded to `SHOWN_LINES` lines unless it is `expanded` or
/// already that short.
#[must_use]
pub fn preview(text: &str, expanded: bool) -> Preview {
    let lines: Vec<&str> = text.lines().collect();
    if expanded || lines.len() <= SHOWN_LINES {
        return Preview {
            text: text.to_owned(),
            hidden: 0,
        };
    }
    Preview {
        text: lines[..SHOWN_LINES].join("\n"),
        hidden: lines.len() - SHOWN_LINES,
    }
}

/// The key of one JSON block of a run's timeline, for `toggle_expanded`.
#[must_use]
pub fn expand_key(run: i64, step: usize, part: &str) -> String {
    format!("run-{run}-step-{step}-{part}")
}

/// The key of a run's context block, for `toggle_expanded`.
#[must_use]
pub fn context_key(run: i64, part: &str) -> String {
    format!("run-{run}-{part}")
}

/// The three blocks of a run's context, in the order the page shows them.
#[must_use]
pub fn context_blocks(detail: &WorkflowRunDetail) -> [(&'static str, String); 3] {
    [
        ("Inputs", pretty_json(&detail.inputs)),
        ("Trigger", pretty_json(&detail.trigger)),
        ("Repository", pretty_json(&detail.repo)),
    ]
}

/// The text of the Input block: the recorded input, or a line saying none was kept.
#[must_use]
pub fn input_text(step: &WorkflowStepRecord) -> String {
    match &step.input {
        Some(value) => pretty_json(value),
        None => "No input recorded".to_owned(),
    }
}

/// The text of the Output block, or a line saying the step has not answered.
#[must_use]
pub fn output_text(step: &WorkflowStepRecord) -> String {
    match &step.output {
        Some(value) => pretty_json(value),
        None => "No output yet".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::WorkflowRunSummary;
    use serde_json::json;

    const SAMPLE: &str = r#"schema = 1
name = "ticket-branch"
description = "Create a branch from a ticket"

[inputs]
ticket = { type = "string", prompt = "Ticket key" }

[trigger]
on = "manual"

[[state]]
id = "name"
action = "exec"
with = { argv = ["ticket-branch-name", "${inputs.ticket}"] }
on.timeout = "end.failed"

[[state]]
id = "confirm"
action = "approve"
with = { summary = "Create branch ${states.name.output.text}?" }
on.approved = "create"
on.rejected = "end.done"

[[state]]
id = "create"
action = "branch.create"
with = { name = "${states.name.output.text}", checkout = true }
"#;

    fn info(path: &str, name: &str, source: &str, hash: &str) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: path.into(),
            trigger: "manual".into(),
            key: None,
            inputs: Vec::new(),
            hash: hash.into(),
            error: None,
            warnings: Vec::new(),
            source: source.into(),
        }
    }

    fn id(n: u64) -> OperationId {
        OperationId(n)
    }

    fn summary(run: i64, status: WorkflowRunStatus) -> WorkflowRunSummary {
        WorkflowRunSummary {
            run,
            workflow: "ticket-branch".into(),
            status,
            started_by: "human".into(),
            started_at: 1_000,
            ended_at: Some(4_500),
            end_reason: None,
            current_state: None,
        }
    }

    fn opened() -> (WorkflowsPage, Vec<WorkflowInfo>) {
        let listed = vec![info("/w/ticket.toml", "ticket-branch", SAMPLE, "h1")];
        let mut page = WorkflowsPage::default();
        assert!(page.select("/w/ticket.toml", &listed));
        (page, listed)
    }

    #[test]
    fn a_selected_file_loads_clean_with_its_hash_and_no_diagnostics() {
        let (page, _) = opened();
        assert_eq!(page.name(), Some("ticket-branch"));
        assert_eq!(page.text(), SAMPLE);
        assert!(!page.dirty());
        assert!(page.definition().is_some());
        assert!(page.diagnostics().is_empty());
    }

    #[test]
    fn typing_makes_the_buffer_dirty_and_reverting_clears_it() {
        let (mut page, _) = opened();
        page.edit(&format!("{SAMPLE}# note\n"));
        assert!(page.dirty());
        page.revert();
        assert!(!page.dirty());
        assert_eq!(page.text(), SAMPLE);
    }

    #[test]
    fn a_broken_buffer_keeps_its_one_parse_error_and_no_definition() {
        let (mut page, _) = opened();
        page.edit("schema = 1\n[[state]\n");
        assert!(page.definition().is_none());
        assert_eq!(page.diagnostics().len(), 1);
        assert!(page.diagnostics()[0].line > 0);
    }

    #[test]
    fn a_validation_error_keeps_the_definition_for_the_flow() {
        let broken = SAMPLE.replace(r#"on.approved = "create""#, r#"on.approved = "nowhere""#);
        let (definition, diagnostics) = analyse(&broken);
        assert!(definition.is_some());
        assert!(diagnostics.iter().any(|d| d.severity == Severity::Error));
    }

    #[test]
    fn errors_come_before_warnings() {
        let (_, diagnostics) =
            analyse(&SAMPLE.replace(r#"on.approved = "create""#, r#"on.approved = "nowhere""#));
        let severities: Vec<Severity> = diagnostics.iter().map(|d| d.severity).collect();
        let mut sorted = severities.clone();
        sorted.sort_by_key(|s| *s != Severity::Error);
        assert_eq!(severities, sorted);
    }

    #[test]
    fn switching_with_unsaved_changes_asks_and_keeps_the_buffer() {
        let (mut page, mut listed) = opened();
        listed.push(info("/w/other.toml", "other", "schema = 1", "h2"));
        page.edit("changed");
        assert!(!page.select("/w/other.toml", &listed));
        assert_eq!(page.asked_switch(), Some("/w/other.toml"));
        assert_eq!(page.path(), Some("/w/ticket.toml"));
        assert!(page.dirty());
        page.keep_editing();
        assert_eq!(page.asked_switch(), None);
        assert_eq!(page.text(), "changed");
    }

    #[test]
    fn discarding_opens_the_file_that_was_asked_for() {
        let (mut page, mut listed) = opened();
        listed.push(info("/w/other.toml", "other", "schema = 1", "h2"));
        page.edit("changed");
        page.select("/w/other.toml", &listed);
        page.discard_and_switch(&listed);
        assert_eq!(page.path(), Some("/w/other.toml"));
        assert!(!page.dirty());
        assert_eq!(page.asked_switch(), None);
    }

    #[test]
    fn a_listing_with_a_new_hash_updates_a_clean_buffer_only() {
        let (mut page, listed) = opened();
        let changed = vec![info(
            "/w/ticket.toml",
            "ticket-branch",
            "schema = 1\n",
            "h9",
        )];
        page.listed(&changed);
        assert_eq!(page.text(), "schema = 1\n");
        assert!(!page.dirty());

        page.edit("mine");
        page.listed(&listed);
        assert_eq!(page.text(), "mine");
        assert!(page.dirty());
    }

    #[test]
    fn a_file_the_listing_no_longer_has_closes() {
        let (mut page, _) = opened();
        page.listed(&[]);
        assert_eq!(page.path(), None);
        assert_eq!(page.text(), "");
    }

    #[test]
    fn a_save_sends_the_loaded_hash_and_a_saved_answer_clears_the_dirty_flag() {
        let (mut page, _) = opened();
        assert_eq!(page.save_request(false), None, "nothing to save yet");
        page.edit("schema = 1\n");
        let kind = page.save_request(false).expect("dirty buffer saves");
        assert_eq!(
            kind,
            CommandKind::SaveWorkflowFile {
                path: "/w/ticket.toml".into(),
                source: "schema = 1\n".into(),
                expected_hash: Some("h1".into()),
            }
        );
        page.track_save(id(7));
        assert!(page.is_saving());
        assert_eq!(page.save_state(), &SaveState::Saving);
        page.saved(id(7), "h2");
        assert!(!page.dirty());
        assert!(!page.is_saving());
        assert_eq!(page.save_state(), &SaveState::Idle);
        let again = page.save_request(false);
        assert_eq!(again, None, "a saved file is clean");
    }

    #[test]
    fn a_save_that_finds_the_file_changed_is_a_conflict_and_overwrite_takes_the_disk_hash() {
        let (mut page, _) = opened();
        page.edit("mine");
        let _ = page.save_request(false);
        page.track_save(id(3));
        page.operation_ended(
            id(3),
            OperationState::Failed,
            Some("the file changed on disk; reload it"),
        );
        assert_eq!(page.save_state(), &SaveState::Conflict);
        assert!(!page.is_saving());
        assert!(page.dirty(), "the buffer is kept for the user to decide");

        let overwrite = page.save_request(true).expect("overwrite sends");
        assert!(matches!(
            overwrite,
            CommandKind::SaveWorkflowFile { expected_hash: Some(ref hash), .. } if hash == "h1"
        ));
    }

    #[test]
    fn a_listing_that_saw_the_outside_edit_lets_overwrite_expect_the_new_hash() {
        let (mut page, _) = opened();
        page.edit("mine");
        page.listed(&[info("/w/ticket.toml", "ticket-branch", "theirs", "h5")]);
        assert_eq!(page.text(), "mine", "a dirty buffer is kept");
        let kind = page.save_request(true).expect("overwrite sends");
        assert!(matches!(
            kind,
            CommandKind::SaveWorkflowFile { expected_hash: Some(ref hash), .. } if hash == "h5"
        ));
    }

    #[test]
    fn reload_takes_the_listed_source_and_hash() {
        let (mut page, _) = opened();
        page.edit("mine");
        page.listed(&[info("/w/ticket.toml", "ticket-branch", "theirs", "h5")]);
        page.reload();
        assert_eq!(page.text(), "theirs");
        assert!(!page.dirty());
        assert_eq!(page.save_request(false), None);
    }

    #[test]
    fn another_failure_keeps_its_reason_and_a_cancel_is_quiet() {
        let (mut page, _) = opened();
        page.edit("x");
        let _ = page.save_request(false);
        page.track_save(id(4));
        page.operation_ended(id(4), OperationState::Failed, Some("disk full"));
        assert_eq!(page.save_state(), &SaveState::Failed("disk full".into()));

        let _ = page.save_request(false);
        page.track_save(id(5));
        page.operation_ended(id(5), OperationState::Cancelled, None);
        assert_eq!(page.save_state(), &SaveState::Idle);
    }

    #[test]
    fn a_done_operation_waits_for_the_saved_event() {
        let (mut page, _) = opened();
        page.edit("x");
        let _ = page.save_request(false);
        page.track_save(id(6));
        page.operation_ended(id(6), OperationState::Done, None);
        assert!(page.is_saving());
        page.saved(id(6), "h7");
        assert!(!page.dirty());
    }

    #[test]
    fn a_save_answer_for_another_operation_is_ignored() {
        let (mut page, _) = opened();
        page.edit("x");
        let _ = page.save_request(false);
        page.track_save(id(8));
        page.saved(id(99), "h");
        assert!(page.is_saving());
        assert!(page.dirty());
    }

    #[test]
    fn the_delete_confirmation_sends_the_loaded_hash_and_closes() {
        let (mut page, _) = opened();
        page.cancel_delete();
        assert!(!page.asking_delete());
        page.ask_delete();
        assert!(page.asking_delete());
        let kind = page.delete_request().expect("delete sends");
        assert_eq!(
            kind,
            CommandKind::DeleteWorkflowFile {
                path: "/w/ticket.toml".into(),
                expected_hash: "h1".into(),
            }
        );
        assert!(!page.asking_delete());
        page.deleted("/w/ticket.toml");
        assert_eq!(page.path(), None);
    }

    #[test]
    fn a_page_with_no_workflow_asks_for_nothing() {
        let mut page = WorkflowsPage::default();
        page.ask_delete();
        assert!(!page.asking_delete());
        assert_eq!(page.save_request(true), None);
        assert_eq!(page.delete_request(), None);
        assert_eq!(page.show_tab(Tab::Runs), None);
    }

    #[test]
    fn the_runs_tab_asks_for_fifty_runs_of_the_open_workflow() {
        let (mut page, _) = opened();
        assert_eq!(
            page.show_tab(Tab::Runs),
            Some(CommandKind::ListWorkflowRuns {
                workflow: "ticket-branch".into(),
                limit: 50,
            })
        );
        assert_eq!(page.tab(), Tab::Runs);
        assert_eq!(page.show_tab(Tab::Toml), None);
    }

    #[test]
    fn runs_answer_only_the_request_in_flight_for_the_same_workflow() {
        let (mut page, _) = opened();
        page.track_runs(id(10));
        page.runs_listed(
            id(9),
            "ticket-branch",
            vec![summary(1, WorkflowRunStatus::Done)],
        );
        assert_eq!(page.runs(), None, "a stale answer is dropped");
        page.runs_listed(id(10), "other", vec![]);
        assert_eq!(page.runs(), None, "another workflow's answer is dropped");
        page.runs_listed(
            id(10),
            "ticket-branch",
            vec![summary(1, WorkflowRunStatus::Done)],
        );
        assert_eq!(page.runs().map(<[WorkflowRunSummary]>::len), Some(1));
    }

    #[test]
    fn a_failed_runs_read_says_so() {
        let (mut page, _) = opened();
        page.track_runs(id(11));
        page.operation_ended(id(11), OperationState::Failed, Some("no repo"));
        assert!(page.runs_failed());
        assert_eq!(page.runs(), None);
    }

    #[test]
    fn a_run_loads_into_the_detail_and_a_finished_run_asks_again() {
        let (mut page, _) = opened();
        assert_eq!(
            page.open_run(3),
            Some(CommandKind::LoadWorkflowRun { run: 3 })
        );
        assert_eq!(page.selected_run(), Some(3));
        page.track_run(id(20));
        let detail = WorkflowRunDetail {
            summary: summary(3, WorkflowRunStatus::Done),
            definition: String::new(),
            inputs: json!({}),
            trigger: json!({}),
            repo: json!({}),
            steps: Vec::new(),
        };
        page.run_loaded(id(21), detail.clone());
        assert!(
            page.detail().is_none(),
            "another operation's answer is dropped"
        );
        page.run_loaded(id(20), detail);
        assert_eq!(page.detail().map(|d| d.summary.run), Some(3));
        assert_eq!(
            page.refresh_run(3),
            Some(CommandKind::LoadWorkflowRun { run: 3 })
        );
        assert_eq!(page.refresh_run(4), None);
    }

    #[test]
    fn choosing_another_run_drops_the_old_detail() {
        let (mut page, _) = opened();
        page.open_run(1);
        page.track_run(id(1));
        page.run_loaded(
            id(1),
            WorkflowRunDetail {
                summary: summary(1, WorkflowRunStatus::Done),
                definition: String::new(),
                inputs: json!(null),
                trigger: json!(null),
                repo: json!(null),
                steps: Vec::new(),
            },
        );
        page.open_run(2);
        assert!(page.detail().is_none());
    }

    #[test]
    fn expanded_blocks_toggle_by_key() {
        let mut page = WorkflowsPage::default();
        let key = expand_key(1, 0, "input");
        assert!(!page.is_expanded(&key));
        page.toggle_expanded(&key);
        assert!(page.is_expanded(&key));
        page.toggle_expanded(&key);
        assert!(!page.is_expanded(&key));
    }

    #[test]
    fn a_reset_forgets_the_open_workflow_and_its_runs() {
        let (mut page, _) = opened();
        page.track_runs(id(1));
        page.reset();
        assert_eq!(page, WorkflowsPage::default());
    }

    #[test]
    fn the_flow_lists_each_state_in_order_with_its_action_and_level() {
        let (definition, _) = analyse(SAMPLE);
        let definition = definition.expect("the sample parses");
        let (rows, unplaced) = flow_rows(&definition, &[]);
        assert!(unplaced.is_empty());
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["name", "confirm", "create"]);
        assert_eq!(rows[0].kind, "exec");
        assert_eq!(rows[0].level, Some("read"));
        assert_eq!(rows[1].kind, "approve");
        assert_eq!(rows[1].level, Some("approve"));
    }

    #[test]
    fn the_flow_shows_declared_outcomes_and_the_defaults_dimmed() {
        let (definition, _) = analyse(SAMPLE);
        let (rows, _) = flow_rows(&definition.expect("parses"), &[]);
        let name = &rows[0];
        let labels: Vec<(String, bool)> = name
            .edges
            .iter()
            .map(|edge| (edge.label(), edge.defaulted))
            .collect();
        // exec declares `done`, `failed`, `interrupted`, `timeout`; `timeout` is written.
        assert!(labels.contains(&("done → confirm".to_owned(), true)));
        assert!(labels.contains(&("timeout → end.failed".to_owned(), false)));
        assert!(labels.contains(&("failed → end.failed".to_owned(), true)));
        let confirm = &rows[1];
        assert!(
            confirm
                .edges
                .iter()
                .any(|e| e.label() == "approved → create" && !e.defaulted)
        );
        assert!(
            confirm
                .edges
                .iter()
                .any(|e| e.label() == "rejected → end.done")
        );
    }

    #[test]
    fn a_state_with_no_rule_and_no_default_is_a_dangling_edge() {
        let source = SAMPLE.replace(r#"on.rejected = "end.done""#, "");
        let (definition, _) = analyse(&source);
        let (rows, _) = flow_rows(&definition.expect("parses"), &[]);
        let dangling: Vec<&Edge> = rows[1].edges.iter().filter(|e| e.dangling).collect();
        assert_eq!(dangling.len(), 1);
        assert_eq!(dangling[0].outcome, "rejected");
    }

    #[test]
    fn the_last_state_defaults_to_end_done() {
        let (definition, _) = analyse(SAMPLE);
        let (rows, _) = flow_rows(&definition.expect("parses"), &[]);
        let create = rows.last().expect("three rows");
        assert!(
            create
                .edges
                .iter()
                .any(|e| e.label() == "done → end.done" && e.defaulted)
        );
    }

    #[test]
    fn a_diagnostic_lands_on_the_state_it_falls_in() {
        let source = SAMPLE.replace(r#"on.approved = "create""#, r#"on.approved = "nowhere""#);
        let (definition, diagnostics) = analyse(&source);
        let (rows, unplaced) = flow_rows(&definition.expect("parses"), &diagnostics);
        assert!(unplaced.is_empty());
        let confirm = rows.iter().find(|r| r.id == "confirm").expect("row");
        assert!(
            confirm
                .diagnostics
                .iter()
                .any(|d| d.message.contains("nowhere"))
        );
        assert!(rows[0].diagnostics.is_empty());
    }

    #[test]
    fn a_diagnostic_before_every_state_is_unplaced() {
        let definition = analyse(SAMPLE).0.expect("parses");
        let early = Diagnostic::warning(1, "header note");
        let (_, unplaced) = flow_rows(&definition, std::slice::from_ref(&early));
        assert_eq!(unplaced, vec![early]);
    }

    #[test]
    fn a_nested_for_each_state_is_indented_under_its_parent() {
        let source = r#"schema = 1
name = "each"

[trigger]
on = "manual"

[[state]]
id = "roots"
action = "exec"
with = { argv = ["roots"] }

[[state]]
id = "each"
for_each = "${states.roots.output.roots}"
as = "root"
states = [
  { id = "plan", action = "exec", with = { argv = ["plan", "${root}"] } },
  { id = "apply", action = "exec", with = { argv = ["apply", "${states.plan.output.text}"] } },
]
"#;
        let (definition, _) = analyse(source);
        let definition = definition.expect("the nested file parses");
        let (rows, _) = flow_rows(&definition, &[]);
        let shape: Vec<(usize, &str)> = rows.iter().map(|r| (r.depth, r.id.as_str())).collect();
        assert_eq!(
            shape,
            [(0, "roots"), (0, "each"), (1, "plan"), (1, "apply")]
        );
        assert_eq!(rows[1].kind, "for_each");
    }

    #[test]
    fn an_unparsed_file_has_no_flow_and_one_diagnostic() {
        let (definition, diagnostics) = analyse("name = ");
        assert!(definition.is_none());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostic_text(&diagnostics[0]).starts_with("line "));
    }

    #[test]
    fn a_diagnostic_without_a_line_says_its_message() {
        assert_eq!(
            diagnostic_text(&Diagnostic::error(0, "no states")),
            "no states"
        );
        assert_eq!(
            diagnostic_text(&Diagnostic::error(4, "bad key")),
            "line 4: bad key"
        );
    }

    #[test]
    fn conflicts_are_recognised_by_the_engine_wording_only() {
        assert!(is_conflict("The file changed on disk; reload it."));
        assert!(is_conflict("Changed on disk"));
        assert!(!is_conflict("permission denied"));
    }

    #[test]
    fn durations_read_as_ms_seconds_or_minutes() {
        assert_eq!(duration_label(850), "850 ms");
        assert_eq!(duration_label(3_200), "3.2 s");
        assert_eq!(duration_label(124_000), "2 min 4 s");
        assert_eq!(duration_label(-5), "0 ms");
    }

    #[test]
    fn a_run_and_a_step_last_from_start_to_end() {
        let run = summary(1, WorkflowRunStatus::Done);
        assert_eq!(run_duration(&run), Some(3_500));
        let mut open = run;
        open.ended_at = None;
        assert_eq!(run_duration(&open), None);
    }

    #[test]
    fn statuses_have_their_words() {
        assert_eq!(
            status_label(WorkflowRunStatus::AwaitingApproval),
            "Awaiting approval"
        );
        assert_eq!(status_label(WorkflowRunStatus::Cancelled), "Cancelled");
        assert_eq!(status_label(WorkflowRunStatus::Blocked), "Blocked");
    }

    #[test]
    fn visits_after_the_first_are_named() {
        assert_eq!(visit_label(1), None);
        assert_eq!(visit_label(3), Some("visit 3".to_owned()));
    }

    #[test]
    fn a_long_block_folds_to_six_lines_and_says_how_many_hide() {
        let text = (1..=9)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let folded = preview(&text, false);
        assert_eq!(folded.hidden, 3);
        assert_eq!(folded.text.lines().count(), 6);
        assert_eq!(
            preview(&text, true),
            Preview {
                text: text.clone(),
                hidden: 0
            }
        );
        assert_eq!(preview("a\nb", false).hidden, 0);
    }

    #[test]
    fn json_is_pretty_printed() {
        let text = pretty_json(&json!({"a": 1}));
        assert!(text.contains('\n'));
        assert!(text.contains("\"a\": 1"));
    }

    #[test]
    fn a_step_shows_its_input_or_says_none_was_kept() {
        let step = WorkflowStepRecord {
            state: "name".into(),
            visit: 1,
            action: Some("exec".into()),
            input: None,
            output: Some(json!({"text": "x"})),
            truncated: false,
            outcome: Some("done".into()),
            next: Some("confirm".into()),
            started_at: 0,
            ended_at: Some(120),
        };
        assert_eq!(input_text(&step), "No input recorded");
        assert!(output_text(&step).contains("\"text\": \"x\""));
        assert_eq!(outcome_line(&step), "done → confirm");
        assert_eq!(step_duration(&step), Some(120));
        let mut running = step;
        running.output = None;
        running.outcome = None;
        assert_eq!(output_text(&running), "No output yet");
        assert_eq!(outcome_line(&running), "no outcome yet");
    }

    #[test]
    fn the_context_shows_inputs_trigger_and_repo_in_that_order() {
        let detail = WorkflowRunDetail {
            summary: summary(1, WorkflowRunStatus::Done),
            definition: String::new(),
            inputs: json!({"ticket": "ENG-1"}),
            trigger: json!({"on": "manual"}),
            repo: json!({"branch": "main"}),
            steps: Vec::new(),
        };
        let blocks = context_blocks(&detail);
        let labels: Vec<&str> = blocks.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["Inputs", "Trigger", "Repository"]);
        assert!(blocks[0].1.contains("ENG-1"));
    }

    /// Three top-level states; `b` has two transitions on `done`.
    const FLOW: &str = "schema = 1\n# The header note stays.\nname = \"flow\"\n\n[trigger]\non = \"manual\"\n\n# First.\n[[state]]\nid = \"a\"\naction = \"noop\"\non.done = \"b\"\n\n[[state]]\nid = \"b\"\naction = \"noop\"\non.done = [\n  { to = \"end.done\" },\n  { to = \"a\", limit = 2 },\n]\n\n[[state]]\nid = \"c\"\naction = \"noop\"\n";

    fn flow_page(source: &str) -> WorkflowsPage {
        let listed = vec![info("/w/flow.toml", "flow", source, "h1")];
        let mut page = WorkflowsPage::default();
        assert!(page.select("/w/flow.toml", &listed));
        page
    }

    fn state_ids(page: &WorkflowsPage) -> Vec<String> {
        page.definition()
            .expect("the file parses")
            .states
            .iter()
            .map(|state| state.id.clone())
            .collect()
    }

    fn answer() -> WorkflowDryRun {
        WorkflowDryRun {
            error: None,
            missing_inputs: Vec::new(),
            steps: Vec::new(),
            stop: "end.done".into(),
        }
    }

    #[test]
    fn an_add_goes_after_the_selected_state_and_selects_the_new_one() {
        let mut page = flow_page(FLOW);
        page.select_state("a");
        assert!(page.apply_flow_edit(FlowEdit::Add));
        assert_eq!(state_ids(&page), ["a", "step", "b", "c"]);
        assert_eq!(page.selected(), Some("step"));
        assert!(page.dirty());
        assert!(page.text().contains("# The header note stays."));
    }

    #[test]
    fn an_add_with_no_selection_goes_to_the_end() {
        let mut page = flow_page(FLOW);
        assert!(page.apply_flow_edit(FlowEdit::Add));
        assert_eq!(state_ids(&page), ["a", "b", "c", "step"]);
        assert_eq!(page.selected(), Some("step"));
    }

    #[test]
    fn a_remove_selects_the_next_state_or_the_one_before_the_last() {
        let mut page = flow_page(FLOW);
        page.select_state("b");
        assert!(page.apply_flow_edit(FlowEdit::Remove));
        assert_eq!(state_ids(&page), ["a", "c"]);
        assert_eq!(page.selected(), Some("c"));
        page.select_state("c");
        assert!(page.apply_flow_edit(FlowEdit::Remove));
        assert_eq!(state_ids(&page), ["a"]);
        assert_eq!(page.selected(), Some("a"));
    }

    #[test]
    fn a_refused_edit_keeps_the_buffer_and_says_why() {
        let mut page = flow_page(FLOW);
        assert!(!page.apply_flow_edit(FlowEdit::Move(Direction::Up)));
        assert_eq!(page.flow_error(), Some("Select a state in the Flow first."));
        assert!(!page.dirty());
        assert_eq!(page.text(), FLOW);

        let mut single = flow_page(
            "schema = 1\nname = \"w\"\n\n[trigger]\non = \"manual\"\n\n[[state]]\nid = \"a\"\naction = \"noop\"\n",
        );
        single.select_state("a");
        assert!(!single.apply_flow_edit(FlowEdit::Remove));
        assert!(
            single
                .flow_error()
                .is_some_and(|error| error.contains("only state"))
        );
        assert!(!single.dirty());
    }

    #[test]
    fn a_move_keeps_the_selection_and_an_edge_move_is_refused() {
        let mut page = flow_page(FLOW);
        page.select_state("a");
        assert!(!page.apply_flow_edit(FlowEdit::Move(Direction::Up)));
        assert!(
            page.flow_error()
                .is_some_and(|error| error.contains("already the first"))
        );
        assert!(page.apply_flow_edit(FlowEdit::Move(Direction::Down)));
        assert_eq!(state_ids(&page), ["b", "a", "c"]);
        assert_eq!(page.selected(), Some("a"));
        assert_eq!(page.flow_error(), None);
    }

    #[test]
    fn a_flow_edit_is_one_change_that_revert_takes_back() {
        let mut page = flow_page(FLOW);
        page.select_state("a");
        assert!(page.apply_flow_edit(FlowEdit::Add));
        assert!(page.dirty());
        page.revert();
        assert!(!page.dirty());
        assert_eq!(page.text(), FLOW);
    }

    #[test]
    fn a_picked_target_sets_its_edge_and_default_clears_the_outcome() {
        let mut page = flow_page(FLOW);
        page.select_state("a");
        page.open_picker("a", "done", 0, (10., 20.));
        assert!(page.picker().is_some());
        assert!(page.pick_target(&Choice::Target("c".into())));
        assert!(page.picker().is_none());
        assert!(page.text().contains("on.done = \"c\""));

        page.select_state("b");
        page.open_picker("b", "done", 1, (0., 0.));
        assert!(page.pick_target(&Choice::Target("c".into())));
        let (rows, _) = flow_rows(page.definition().expect("parses"), &[]);
        let labels: Vec<String> = rows[1].edges.iter().map(Edge::label).collect();
        assert_eq!(
            labels,
            [
                "done → end.done",
                "done → c (limit 2)",
                "failed → end.failed"
            ]
        );

        page.open_picker("b", "done", 0, (0., 0.));
        assert!(page.pick_target(&Choice::Default));
        assert!(!page.text().contains("on.done = ["));
    }

    #[test]
    fn the_picker_opens_only_over_the_selected_state_and_another_state_closes_it() {
        let mut page = flow_page(FLOW);
        page.select_state("a");
        page.open_picker("b", "done", 0, (0., 0.));
        assert!(page.picker().is_none());

        page.open_picker("a", "done", 0, (1., 2.));
        page.select_state("a");
        assert!(
            page.picker().is_some(),
            "a click on the selected state keeps it"
        );
        page.select_state("b");
        assert!(page.picker().is_none());
        page.select_state("zz");
        assert_eq!(page.selected(), Some("b"), "an unknown state is ignored");
    }

    #[test]
    fn the_selection_survives_typing_and_a_broken_file_but_not_its_state_going() {
        let mut page = flow_page(FLOW);
        page.select_state("b");
        page.edit(&format!("{FLOW}# note\n"));
        assert_eq!(page.selected(), Some("b"));
        page.edit("schema = ");
        assert_eq!(
            page.selected(),
            Some("b"),
            "a file that does not parse keeps the place"
        );
        page.edit(&edit::remove_state(FLOW, "b").expect("b can go"));
        assert_eq!(page.selected(), None);
    }

    #[test]
    fn the_picker_offers_the_other_states_and_end_blocked_only_before_a_veto() {
        let definition = analyse(FLOW).0.expect("parses");
        assert_eq!(
            target_options(&definition, "a"),
            ["b", "c", "end.done", "end.failed"]
        );
        let before = analyse(&FLOW.replace("on = \"manual\"", "on = \"before.restack\""))
            .0
            .expect("parses");
        assert!(target_options(&before, "a").contains(&"end.blocked".to_owned()));
    }

    #[test]
    fn a_dry_run_is_asked_only_for_a_file_that_parses() {
        let mut broken = flow_page(FLOW);
        broken.edit("schema = ");
        assert_eq!(broken.dry_run_request(), None);

        let mut page = flow_page(FLOW);
        assert_eq!(
            page.dry_run_request(),
            Some(CommandKind::DryRunWorkflow {
                source: FLOW.into(),
                inputs: json!({}),
            })
        );
    }

    #[test]
    fn a_dry_run_answer_is_kept_for_its_request_and_stale_after_an_edit() {
        let mut page = flow_page(FLOW);
        let _ = page.dry_run_request();
        page.track_dry_run(id(5));
        assert!(page.dry_run_pending());
        page.dry_run_answered(id(4), answer());
        assert_eq!(
            page.dry_run_result(),
            None,
            "another operation's answer is dropped"
        );
        assert!(page.dry_run_pending());
        page.dry_run_answered(id(5), answer());
        assert_eq!(page.dry_run_result(), Some(&answer()));
        assert!(!page.dry_run_pending());
        assert!(!page.dry_run_stale());

        page.edit(&format!("{FLOW}# note\n"));
        assert_eq!(page.dry_run_result(), None);
        assert!(page.dry_run_stale());
        page.dismiss_dry_run();
        assert!(!page.dry_run_stale());
    }

    #[test]
    fn a_failed_dry_run_is_no_longer_pending() {
        let mut page = flow_page(FLOW);
        let _ = page.dry_run_request();
        page.track_dry_run(id(7));
        page.operation_ended(id(7), OperationState::Failed, Some("no"));
        assert!(!page.dry_run_pending());
    }

    #[test]
    fn the_flow_position_and_the_edge_places_follow_the_file() {
        let mut page = flow_page(FLOW);
        page.select_state("b");
        assert_eq!(
            page.state(true).flow,
            FlowPosition {
                parses: true,
                states: 3,
                selected: Some(1),
            }
        );
        let (rows, _) = flow_rows(page.definition().expect("parses"), &[]);
        let places: Vec<(String, usize)> = rows[1]
            .edges
            .iter()
            .map(|edge| (edge.label(), edge.index))
            .collect();
        assert_eq!(
            places,
            [
                ("done → end.done".to_owned(), 0),
                ("done → a (limit 2)".to_owned(), 1),
                ("failed → end.failed".to_owned(), 0),
            ]
        );
    }
}
