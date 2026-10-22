//! The action registry: every action the window can run, with its stable id,
//! title, default shortcut, typed target and its "enabled now?" rule. Keys,
//! buttons, menus and the palette read it, and `plan` is the one path from a
//! run to the engine commands it sends. Nothing here touches GPUI, so the rules
//! are unit-tested on their own.

// `from_id` has no caller outside the tests yet.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use gm_schema::{
    CommandKind, DiffSide, HunkId, OperationId, StageTarget, StatusEntry, WorkflowInfo,
    WorkflowInput, WorktreeEntry,
};

use gm_core::workflow::edit::Direction;

use crate::branch_filter::Scope;
use crate::flow_inspector::PickerKind;
use crate::workflows_page::{FlowEdit, PageState};

/// Why an action is not enabled, as a short English sentence for the user.
pub type Reason = &'static str;

/// What a menu or the palette groups an action under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    WorkingCopy,
    Commit,
    Branches,
    Remote,
    Worktrees,
    Repository,
    Workflows,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::WorkingCopy => "Working Copy",
            Category::Commit => "Commit",
            Category::Branches => "Branches",
            Category::Remote => "Remote",
            Category::Worktrees => "Worktrees",
            Category::Repository => "Repository",
            Category::Workflows => "Workflows",
        }
    }
}

/// The kind of target an action takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    None,
    Branch,
    /// No target, or a branch as the start point when the run names one.
    OptionalBranch,
    File,
    Chunk,
    Worktree,
    Commit,
    /// A manual workflow, as the engine listed it.
    Workflow,
    /// A built-in workflow template, as the engine listed it.
    Template,
}

/// The target of one run. Its kind must match the action's `ArgKind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Args {
    None,
    Branch(String),
    File(String),
    /// A hunk of one side of `path`. The id is checked by the engine, so a
    /// stale one is refused there.
    Chunk {
        path: String,
        hunk: HunkId,
        side: DiffSide,
    },
    /// The worktree whose folder is `path`, as the list gives it.
    Worktree(String),
    /// The text to copy from a commit of the history page: its id, or its message.
    Commit(String),
    /// The name of a listed manual workflow.
    Workflow(String),
    /// The name of a listed built-in template.
    Template(String),
}

/// Static description of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub id: &'static str,
    pub title: &'static str,
    /// Display string of the default key, if the action has one.
    pub shortcut: Option<&'static str>,
    pub args: ArgKind,
    pub category: Category,
}

/// What a successful run does. The window performs it; nothing else differs
/// between a key, a button and a menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Engine commands, in order, all with `Actor::Human`.
    Send(Vec<CommandKind>),
    /// Text for the clipboard.
    Copy(String),
    /// The commit message for the clipboard, noticed as a message and not echoed.
    CopyMessage(String),
    /// Replaces the composer text with the suggestion.
    UseSuggestion(String),
    /// Drops the suggestion; the composer keeps its text.
    DiscardSuggestion,
    /// Hides the approval prompt of the repository hook with this command hash.
    DenyHook(String),
    /// A path on disk, for the system file manager.
    Reveal(PathBuf),
    FocusCommitMessage,
    /// Opens the create-worktree dialog, with `branch` chosen as the branch
    /// to check out when there is one. Without it the dialog starts empty and
    /// the branch is typed or picked there.
    OpenWorktreeDialog {
        branch: Option<String>,
    },
    /// Opens the worktree at this path in its own window.
    OpenWorktree(String),
    /// Opens the remove dialog for the worktree at this path; the dialog
    /// inspects it and the removal is sent from there.
    OpenRemoveDialog(String),
    /// Opens the prune dialog, which lists the stale records before anything is removed.
    OpenPruneDialog,
    /// Opens the new-branch dialog. `start` is the commit the new branch starts
    /// from; `None` is the checked-out branch.
    OpenNewBranchDialog {
        start: Option<String>,
    },
    /// Opens the delete-branch dialog, which asks before the branch goes.
    OpenDeleteBranchDialog(String),
    /// Opens the inputs dialog of a workflow that declares inputs; Run there sends it.
    OpenWorkflowInputs {
        workflow: String,
        inputs: Vec<WorkflowInput>,
    },
    /// Opens the delete-merged-branches dialog, which lists what Git would
    /// delete and asks before anything goes.
    OpenMergedBranchesDialog,
    /// Opens the discard dialog for the file at `path`, which says what will be
    /// lost before anything goes. `include_staged` also throws away the staged
    /// changes (D93).
    OpenDiscardFileDialog {
        path: String,
        include_staged: bool,
    },
    /// Opens the history page and asks the engine for the commits of HEAD.
    ShowHistory,
    /// Opens Quick Open, which finds a file by part of its path (N1).
    OpenQuickOpen,
    /// Opens the Code page (N2).
    ShowCode,
    /// Opens the text search page and puts the cursor in its box (N3).
    ShowSearch,
    /// Opens the Code page on a file of the worktree, at a line if given.
    ViewFile {
        path: String,
        line: Option<u32>,
    },
    /// Opens the Workflows page and asks the engine for the workflow files.
    ShowWorkflows,
    /// Opens the Stashes page and asks the engine for the stashes (B2).
    ShowStashes,
    /// Opens the dialog that stashes the working copy's changes (B2, D94).
    OpenNewStashDialog,
    /// Writes the open workflow's buffer to its file.
    SaveWorkflow,
    /// Drops the open workflow's unsaved changes.
    RevertWorkflow,
    /// Opens the confirmation that moves the open workflow's file to the trash.
    OpenDeleteWorkflow,
    /// Applies a structural edit to the open workflow's Flow.
    EditFlow(FlowEdit),
    /// Asks the engine for a dry run of the open workflow's buffer.
    DryRunWorkflow,
    /// Opens a popover of the Flow's inspector: the actions or the triggers.
    OpenInspectorPicker(PickerKind),
    /// Moves the keyboard to the field that adds an input.
    FocusWorkflowInput,
    /// Sets the sidebar's branches to the scope the window names (D53).
    SetBranchScope(Scope),
}

/// What the window knows of the prepare-commit hooks, for the rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookCtx<'a> {
    /// The run that is starting or running, if one is.
    pub running: Option<OperationId>,
    /// How many hooks apply to this branch and are not disabled; `None` until listed.
    pub applicable: Option<usize>,
    /// The message the last run offered and the user has not used or dropped.
    pub suggestion: Option<&'a str>,
    /// The first repository hook awaiting approval: its name and command hash.
    pub pending_approval: Option<(&'a str, &'a str)>,
}

/// What the window knows of the manual workflows, for the rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkflowCtx<'a> {
    /// The workflows the engine listed; `None` until the first listing.
    pub listed: Option<&'a [WorkflowInfo]>,
    /// The newest run waiting for an approval, and the run's id.
    pub awaiting: Option<(i64, &'a str)>,
    /// The newest run that has not ended.
    pub active: Option<i64>,
    /// The user's workflows folder, when the listing gave one.
    pub user_dir: Option<&'a str>,
    /// The built-in templates the listing gave, by name.
    pub templates: &'a [String],
    /// The Workflows page: whether it is the screen, what is open, its changes.
    pub page: PageState<'a>,
}

/// The window state an action's rule reads. Built by the window for each run.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// A repository is open (the welcome screen has none).
    pub repo_open: bool,
    /// Why a repository change must wait right now, if it must: an open or a
    /// checkout in flight.
    pub busy: Option<Reason>,
    pub entries: &'a [StatusEntry],
    pub staged_count: usize,
    /// The commit message as typed.
    pub message: &'a str,
    /// The checked-out branch; `None` when HEAD is detached or unborn.
    pub current_branch: Option<&'a str>,
    /// The repository's working tree root; `None` when no repository is open.
    pub root: Option<&'a str>,
    pub hooks: HookCtx<'a>,
    pub workflows: WorkflowCtx<'a>,
    /// The worktrees listed for the open repository, main checkout first.
    pub worktrees: &'a [WorktreeEntry],
    /// The scope in effect for the sidebar's branches now (D53).
    pub branch_scope: Scope,
    /// Whether Git has a `user.email` to match branches against.
    pub user_email_known: bool,
    /// A fetch, pull or push with lease is in flight; the three refuse another one meanwhile.
    pub remote_running: bool,
}

/// The specs, in the order of `Action`'s variants (a test keeps them in step).
const SPECS: [Spec; 68] = [
    spec(
        "file.stage",
        "Stage File",
        Some("Space"),
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "file.unstage",
        "Unstage File",
        Some("Space"),
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.stage",
        "Stage Chunk",
        Some("⇧⌘A"),
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.unstage",
        "Unstage Chunk",
        Some("⇧⌘A"),
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "chunk.discard",
        "Discard Chunk",
        None,
        ArgKind::Chunk,
        Category::WorkingCopy,
    ),
    spec(
        "commit.create",
        "Commit",
        Some("⌘↵"),
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "repo.refresh",
        "Refresh",
        Some("R"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "branch.checkout",
        "Checkout Branch",
        Some("Enter"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "branch.copy_name",
        "Copy Branch Name",
        Some("⌥⌘C"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "file.copy_path",
        "Copy File Path",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "commit.focus_message",
        "Focus Commit Message",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "branch.new",
        "New Branch…",
        None,
        ArgKind::OptionalBranch,
        Category::Branches,
    ),
    spec(
        "branch.new_child",
        "New Child Branch",
        Some("⌘B"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "stack.restack",
        "Restack This Stack",
        Some("⇧⌘R"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "remote.push_with_lease",
        "Push With Lease",
        Some("⇧⌘P"),
        ArgKind::Branch,
        Category::Remote,
    ),
    spec(
        "branch.rename",
        "Rename Branch",
        Some("F2"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "branch.delete",
        "Delete Branch",
        Some("⌫"),
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "worktree.create",
        "Create Worktree",
        Some("⌥⌘W"),
        ArgKind::Branch,
        Category::Worktrees,
    ),
    spec(
        "worktree.new",
        "New Worktree…",
        None,
        ArgKind::None,
        Category::Worktrees,
    ),
    spec(
        "file.discard",
        "Discard Changes",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "file.reveal",
        "Reveal in File Manager",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "commit.suggest_message",
        "Suggest Commit Message",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "commit.use_suggestion",
        "Replace Message With Suggestion",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "commit.discard_suggestion",
        "Discard Suggestion",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "hook.cancel",
        "Cancel Hook Run",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "hook.approve",
        "Approve Repository Hook",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "hook.deny",
        "Deny Repository Hook",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "workflow.run",
        "Run Workflow…",
        None,
        ArgKind::Workflow,
        Category::Workflows,
    ),
    spec(
        "workflow.approve",
        "Approve Workflow Step",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.reject",
        "Reject Workflow Step",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.cancel",
        "Cancel Workflow Run",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.new",
        "New Workflow from Template…",
        None,
        ArgKind::Template,
        Category::Workflows,
    ),
    spec(
        "workflow.reload",
        "Reload Workflows",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.open_folder",
        "Open Workflows Folder",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.show",
        "Show Workflows",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.save",
        "Save Workflow",
        Some("⌘S"),
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.revert",
        "Revert Workflow",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.delete",
        "Delete Workflow…",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.add_state",
        "Add Workflow State",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.remove_state",
        "Remove Workflow State",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.move_state_up",
        "Move Workflow State Up",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.move_state_down",
        "Move Workflow State Down",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.dry_run",
        "Dry Run Workflow",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.set_action",
        "Set Workflow State Action",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.set_trigger",
        "Set Workflow Trigger",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.add_input",
        "Add Workflow Input",
        None,
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "remote.fetch",
        "Fetch",
        None,
        ArgKind::None,
        Category::Remote,
    ),
    spec("remote.pull", "Pull", None, ArgKind::None, Category::Remote),
    spec(
        "worktree.open",
        "Open Worktree",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.reveal",
        "Reveal Worktree in File Manager",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.copy_path",
        "Copy Worktree Path",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.remove",
        "Remove Worktree",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.lock",
        "Lock Worktree",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.unlock",
        "Unlock Worktree",
        None,
        ArgKind::Worktree,
        Category::Worktrees,
    ),
    spec(
        "worktree.prune",
        "Prune Worktrees",
        None,
        ArgKind::None,
        Category::Worktrees,
    ),
    spec(
        "history.show",
        "Show History",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "branch.show_mine",
        "Show Only My Branches",
        None,
        ArgKind::None,
        Category::Branches,
    ),
    spec(
        "branch.show_all",
        "Show All Branches",
        None,
        ArgKind::None,
        Category::Branches,
    ),
    spec(
        "commit.copy_hash",
        "Copy Commit Hash",
        None,
        ArgKind::Commit,
        Category::Commit,
    ),
    spec(
        "commit.copy_message",
        "Copy Commit Message",
        None,
        ArgKind::Commit,
        Category::Commit,
    ),
    spec(
        "branch.delete_merged",
        "Delete Merged Branches…",
        None,
        ArgKind::None,
        Category::Branches,
    ),
    spec(
        "files.quick_open",
        "Quick Open…",
        Some("⌘P"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "code.show",
        "Show Code",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "file.view",
        "View File",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "search.text",
        "Search Text…",
        Some("⇧⌘F"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "file.discard_all",
        "Discard All Changes",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "stash.show",
        "Show Stashes",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "stash.new",
        "Stash Changes…",
        None,
        ArgKind::None,
        Category::WorkingCopy,
    ),
];

const fn spec(
    id: &'static str,
    title: &'static str,
    shortcut: Option<&'static str>,
    args: ArgKind,
    category: Category,
) -> Spec {
    Spec {
        id,
        title,
        shortcut,
        args,
        category,
    }
}

/// Every action the window knows. Placeholders stay in the list so that menus
/// and the palette can show them disabled until their slice lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    StageFile,
    UnstageFile,
    StageChunk,
    UnstageChunk,
    DiscardChunk,
    Commit,
    Refresh,
    CheckoutBranch,
    CopyBranchName,
    CopyFilePath,
    FocusCommitMessage,
    NewBranch,
    NewChildBranch,
    Restack,
    PushWithLease,
    RenameBranch,
    DeleteBranch,
    CreateWorktree,
    NewWorktree,
    DiscardFile,
    RevealFile,
    SuggestMessage,
    UseSuggestion,
    DiscardSuggestion,
    CancelHook,
    ApproveHook,
    DenyHook,
    RunWorkflow,
    ApproveWorkflowStep,
    RejectWorkflowStep,
    CancelWorkflowRun,
    NewWorkflowFromTemplate,
    ReloadWorkflows,
    OpenWorkflowsFolder,
    ShowWorkflows,
    SaveWorkflow,
    RevertWorkflow,
    DeleteWorkflow,
    AddWorkflowState,
    RemoveWorkflowState,
    MoveWorkflowStateUp,
    MoveWorkflowStateDown,
    DryRunWorkflow,
    SetWorkflowStateCall,
    SetWorkflowTrigger,
    AddWorkflowInput,
    Fetch,
    Pull,
    OpenWorktree,
    RevealWorktree,
    CopyWorktreePath,
    RemoveWorktree,
    LockWorktree,
    UnlockWorktree,
    PruneWorktrees,
    ShowHistory,
    ShowOnlyMyBranches,
    ShowAllBranches,
    CopyCommitHash,
    CopyCommitMessage,
    DeleteMergedBranches,
    QuickOpen,
    ShowCode,
    ViewFile,
    SearchText,
    DiscardAllChanges,
    ShowStashes,
    NewStash,
}

/// The shortcut as the window shows it: the glyphs on macOS; elsewhere words,
/// with Cmd read as Ctrl, which is the key the `secondary` bindings use.
#[must_use]
pub fn shortcut_text(shortcut: &str) -> String {
    shortcut_words(shortcut, cfg!(target_os = "macos"))
}

fn shortcut_words(shortcut: &str, mac: bool) -> String {
    if mac {
        return shortcut.to_owned();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut key = String::new();
    for glyph in shortcut.chars() {
        match glyph {
            '⌘' => parts.push("Ctrl".to_owned()),
            '⌥' => parts.push("Alt".to_owned()),
            '⇧' => parts.push("Shift".to_owned()),
            '↵' => key.push_str("Enter"),
            '⌫' => key.push_str("Backspace"),
            other => key.push(other),
        }
    }
    parts.push(key);
    parts.join("+")
}

/// What the actions that open a page or a viewer do.
fn navigation_effect(action: Action, args: &Args) -> Effect {
    match (action, args) {
        (Action::QuickOpen, _) => Effect::OpenQuickOpen,
        (Action::ShowCode, _) => Effect::ShowCode,
        (Action::SearchText, _) => Effect::ShowSearch,
        (Action::ViewFile, Args::File(path)) => Effect::ViewFile {
            path: path.clone(),
            line: None,
        },
        (Action::ShowWorkflows, _) => Effect::ShowWorkflows,
        (Action::ShowStashes, _) => Effect::ShowStashes,
        _ => Effect::ShowHistory,
    }
}

/// The rule of Reveal in File Manager: a repository is open and the file is in it.
fn reveal_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    if ctx.root.is_none() {
        return Err("No repository is open.");
    }
    file_entry(args, ctx).map(|_| ())
}

/// The rule of the two sidebar scope actions: each is refused when it would
/// change nothing, and Mine is refused when Git has no email to match.
fn scope_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    if !ctx.repo_open {
        return Err("No repository is open.");
    }
    match action {
        Action::ShowOnlyMyBranches if !ctx.user_email_known => {
            Err("Git has no user.email to match branches against.")
        }
        Action::ShowOnlyMyBranches if ctx.branch_scope == Scope::Mine => {
            Err("Already showing only your branches.")
        }
        Action::ShowAllBranches if ctx.branch_scope == Scope::All => {
            Err("Already showing all branches.")
        }
        _ => Ok(()),
    }
}

/// The rule of a worktree action: whether its worktree allows it now.
fn worktree_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    let entry = worktree_entry(args, ctx)?;
    let here = ctx.root == Some(entry.path.as_str());
    let missing = entry.prunable.is_some();
    match action {
        Action::RemoveWorktree if entry.is_main => Err("The main checkout cannot be removed."),
        Action::LockWorktree if entry.locked.is_some() => Err("It is already locked."),
        Action::UnlockWorktree if entry.locked.is_none() => Err("It is not locked."),
        Action::OpenWorktree | Action::RevealWorktree if missing => {
            Err("This worktree's folder is missing.")
        }
        Action::RemoveWorktree if missing => {
            Err("Its folder is missing. Prune the worktree instead.")
        }
        Action::OpenWorktree | Action::RemoveWorktree if here => {
            Err("This window is open in this worktree.")
        }
        _ => Ok(()),
    }
}

/// The worktree the run names, as the list has it.
fn worktree_entry<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a WorktreeEntry, Reason> {
    match args {
        Args::Worktree(path) => ctx
            .worktrees
            .iter()
            .find(|entry| &entry.path == path)
            .ok_or("This worktree is not listed."),
        _ => Err("No target for this action"),
    }
}

/// The reason of an action the engine cannot run yet, whatever its target.
pub(crate) const NOT_AVAILABLE: Reason = "Not available yet";

impl Action {
    pub const ALL: [Action; 68] = [
        Action::StageFile,
        Action::UnstageFile,
        Action::StageChunk,
        Action::UnstageChunk,
        Action::DiscardChunk,
        Action::Commit,
        Action::Refresh,
        Action::CheckoutBranch,
        Action::CopyBranchName,
        Action::CopyFilePath,
        Action::FocusCommitMessage,
        Action::NewBranch,
        Action::NewChildBranch,
        Action::Restack,
        Action::PushWithLease,
        Action::RenameBranch,
        Action::DeleteBranch,
        Action::CreateWorktree,
        Action::NewWorktree,
        Action::DiscardFile,
        Action::RevealFile,
        Action::SuggestMessage,
        Action::UseSuggestion,
        Action::DiscardSuggestion,
        Action::CancelHook,
        Action::ApproveHook,
        Action::DenyHook,
        Action::RunWorkflow,
        Action::ApproveWorkflowStep,
        Action::RejectWorkflowStep,
        Action::CancelWorkflowRun,
        Action::NewWorkflowFromTemplate,
        Action::ReloadWorkflows,
        Action::OpenWorkflowsFolder,
        Action::ShowWorkflows,
        Action::SaveWorkflow,
        Action::RevertWorkflow,
        Action::DeleteWorkflow,
        Action::AddWorkflowState,
        Action::RemoveWorkflowState,
        Action::MoveWorkflowStateUp,
        Action::MoveWorkflowStateDown,
        Action::DryRunWorkflow,
        Action::SetWorkflowStateCall,
        Action::SetWorkflowTrigger,
        Action::AddWorkflowInput,
        Action::Fetch,
        Action::Pull,
        Action::OpenWorktree,
        Action::RevealWorktree,
        Action::CopyWorktreePath,
        Action::RemoveWorktree,
        Action::LockWorktree,
        Action::UnlockWorktree,
        Action::PruneWorktrees,
        Action::ShowHistory,
        Action::ShowOnlyMyBranches,
        Action::ShowAllBranches,
        Action::CopyCommitHash,
        Action::CopyCommitMessage,
        Action::DeleteMergedBranches,
        Action::QuickOpen,
        Action::ShowCode,
        Action::ViewFile,
        Action::SearchText,
        Action::DiscardAllChanges,
        Action::ShowStashes,
        Action::NewStash,
    ];

    #[must_use]
    pub fn spec(self) -> Spec {
        SPECS[self as usize]
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        self.spec().id
    }

    /// The shortcut as the window shows it (see `shortcut_text`), if any.
    #[must_use]
    pub fn shortcut_label(self) -> Option<String> {
        self.spec().shortcut.map(shortcut_text)
    }

    /// Whether the engine can run the action at all yet. A placeholder is
    /// listed disabled with `NOT_AVAILABLE` wherever it appears.
    #[must_use]
    pub fn available(self) -> bool {
        !matches!(
            self,
            Action::NewChildBranch | Action::Restack | Action::RenameBranch
        )
    }

    /// Whether the action throws away work. Menus show it in red and ask first.
    #[must_use]
    pub fn destructive(self) -> bool {
        matches!(
            self,
            Action::DiscardChunk
                | Action::DiscardFile
                | Action::DeleteBranch
                | Action::DeleteMergedBranches
                | Action::RemoveWorktree
                | Action::DiscardAllChanges
        )
    }

    /// Whether the action opens a page or a viewer, which `navigation_effect` plans.
    fn opens_a_page(self) -> bool {
        matches!(
            self,
            Action::ShowHistory
                | Action::QuickOpen
                | Action::ShowCode
                | Action::SearchText
                | Action::ViewFile
                | Action::ShowWorkflows
                | Action::ShowStashes
        )
    }

    /// Whether the action is only its target: copying or focusing needs no rule of its own.
    fn copies_its_target(self) -> bool {
        matches!(
            self,
            Action::CopyBranchName
                | Action::CopyFilePath
                | Action::FocusCommitMessage
                | Action::CopyCommitHash
                | Action::CopyCommitMessage
        )
    }

    /// Whether the rule of `worktree_rule` decides the action.
    fn is_worktree_rule(self) -> bool {
        matches!(
            self,
            Action::OpenWorktree
                | Action::RevealWorktree
                | Action::CopyWorktreePath
                | Action::RemoveWorktree
                | Action::LockWorktree
                | Action::UnlockWorktree
        )
    }

    /// Whether the rule of `stash_rule` decides the action.
    fn is_stash_rule(self) -> bool {
        matches!(self, Action::ShowStashes | Action::NewStash)
    }

    /// Whether the rules of `workflow_rule` and `workflow_effect` decide the action.
    fn is_workflow_rule(self) -> bool {
        matches!(
            self,
            Action::RunWorkflow
                | Action::ApproveWorkflowStep
                | Action::RejectWorkflowStep
                | Action::CancelWorkflowRun
                | Action::NewWorkflowFromTemplate
                | Action::ReloadWorkflows
                | Action::OpenWorkflowsFolder
                | Action::SaveWorkflow
                | Action::RevertWorkflow
                | Action::DeleteWorkflow
                | Action::AddWorkflowState
                | Action::RemoveWorkflowState
                | Action::MoveWorkflowStateUp
                | Action::MoveWorkflowStateDown
                | Action::DryRunWorkflow
                | Action::SetWorkflowStateCall
                | Action::SetWorkflowTrigger
                | Action::AddWorkflowInput
        )
    }

    /// Whether the action opens its own dialog, which asks before anything runs.
    /// A menu then does not ask a second time.
    #[must_use]
    pub fn opens_dialog(self) -> bool {
        matches!(
            self,
            Action::CreateWorktree
                | Action::NewWorktree
                | Action::RemoveWorktree
                | Action::PruneWorktrees
                | Action::NewBranch
                | Action::DeleteBranch
                | Action::DeleteMergedBranches
                | Action::DiscardFile
                | Action::DiscardAllChanges
                | Action::NewStash
        )
    }

    /// The action with this id, if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether the action can run now with `args`; the reason when it cannot.
    pub fn enabled(self, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
        if !self.takes(args) {
            return Err("No target for this action");
        }
        if !self.available() {
            return Err(NOT_AVAILABLE);
        }
        match self {
            Action::StageFile
            | Action::UnstageFile
            | Action::StageChunk
            | Action::UnstageChunk
            | Action::DiscardChunk
            | Action::Commit => change_rule(self, args, ctx),
            _ if self.is_worktree_rule() => worktree_rule(self, args, ctx),
            Action::Refresh
            | Action::CreateWorktree
            | Action::NewWorktree
            | Action::PruneWorktrees
            | Action::ShowHistory
            | Action::QuickOpen
            | Action::ShowCode
            | Action::SearchText
            | Action::ShowWorkflows => repository_open(ctx),
            Action::CheckoutBranch => {
                idle(ctx)?;
                match args {
                    Args::Branch(name) if ctx.current_branch == Some(name.as_str()) => {
                        Err("This is the current branch")
                    }
                    _ => Ok(()),
                }
            }
            Action::NewBranch | Action::DeleteBranch => branch_rule(self, args, ctx),
            Action::DeleteMergedBranches => idle(ctx),
            Action::RevealFile | Action::ViewFile => reveal_rule(args, ctx),
            Action::DiscardFile | Action::DiscardAllChanges => discard_rule(self, args, ctx),
            Action::ShowOnlyMyBranches | Action::ShowAllBranches => scope_rule(self, ctx),
            _ if self.copies_its_target() => Ok(()),
            Action::Fetch | Action::Pull | Action::PushWithLease => remote_rule(self, args, ctx),
            Action::SuggestMessage
            | Action::CancelHook
            | Action::UseSuggestion
            | Action::DiscardSuggestion
            | Action::ApproveHook
            | Action::DenyHook => hook_rule(self, ctx),
            _ if self.is_workflow_rule() => workflow_rule(self, args, ctx),
            _ if self.is_stash_rule() => stash_rule(self, ctx),
            // The placeholders, refused above by `available`.
            _ => Err(NOT_AVAILABLE),
        }
    }

    /// Runs the rule, then says what the run does. A refused run gives the reason.
    pub fn plan(self, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
        self.enabled(args, ctx)?;
        Ok(match (self, args) {
            (Action::StageFile, Args::File(path)) => Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::File { path: path.clone() },
            }]),
            (Action::UnstageFile, Args::File(path)) => Effect::Send(vec![CommandKind::Unstage {
                target: StageTarget::File { path: path.clone() },
            }]),
            (Action::StageChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::Stage {
                    target: StageTarget::Hunk {
                        path: path.clone(),
                        hunk: hunk.clone(),
                    },
                }])
            }
            (Action::UnstageChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::Unstage {
                    target: StageTarget::Hunk {
                        path: path.clone(),
                        hunk: hunk.clone(),
                    },
                }])
            }
            (Action::DiscardChunk, Args::Chunk { path, hunk, .. }) => {
                Effect::Send(vec![CommandKind::DiscardHunk {
                    path: path.clone(),
                    hunk: hunk.clone(),
                }])
            }
            (Action::Commit, _) => Effect::Send(vec![CommandKind::Commit {
                message: ctx.message.to_owned(),
            }]),
            (Action::Refresh, _) => Effect::Send(vec![CommandKind::RefreshStatus]),
            (Action::Fetch, _) => Effect::Send(vec![CommandKind::Fetch]),
            (Action::Pull, _) => Effect::Send(vec![CommandKind::Pull]),
            (Action::PushWithLease, _) => Effect::Send(vec![CommandKind::PushWithLease]),
            (Action::CheckoutBranch, Args::Branch(name)) => {
                Effect::Send(vec![CommandKind::Checkout {
                    branch: name.clone(),
                }])
            }
            (Action::CopyBranchName, Args::Branch(text))
            | (Action::CopyFilePath, Args::File(text))
            | (Action::CopyWorktreePath, Args::Worktree(text))
            | (Action::CopyCommitHash, Args::Commit(text)) => Effect::Copy(text.clone()),
            (Action::RevealFile, Args::File(path)) => {
                // `enabled` has checked that the root is there.
                Effect::Reveal(PathBuf::from(ctx.root.unwrap_or_default()).join(path))
            }
            (Action::FocusCommitMessage, _) => Effect::FocusCommitMessage,
            (Action::CreateWorktree, Args::Branch(name)) => Effect::OpenWorktreeDialog {
                branch: Some(name.clone()),
            },
            (Action::NewWorktree, _) => Effect::OpenWorktreeDialog { branch: None },
            (Action::OpenWorktree, Args::Worktree(path)) => Effect::OpenWorktree(path.clone()),
            (Action::RevealWorktree, Args::Worktree(path)) => Effect::Reveal(PathBuf::from(path)),
            (Action::RemoveWorktree, Args::Worktree(path)) => {
                Effect::OpenRemoveDialog(path.clone())
            }
            // The menu does not ask for a lock reason yet, so Git gets none.
            (Action::LockWorktree, Args::Worktree(path)) => {
                Effect::Send(vec![CommandKind::LockWorktree {
                    path: path.clone(),
                    reason: String::new(),
                }])
            }
            (Action::UnlockWorktree, Args::Worktree(path)) => {
                Effect::Send(vec![CommandKind::UnlockWorktree { path: path.clone() }])
            }
            (Action::PruneWorktrees, _) => Effect::OpenPruneDialog,
            (Action::NewStash, _) => Effect::OpenNewStashDialog,
            (action, _) if action.opens_a_page() => navigation_effect(action, args),
            (Action::ShowOnlyMyBranches, _) => Effect::SetBranchScope(Scope::Mine),
            (Action::ShowAllBranches, _) => Effect::SetBranchScope(Scope::All),
            (Action::CopyCommitMessage, Args::Commit(text)) => Effect::CopyMessage(text.clone()),
            (Action::NewBranch, Args::Branch(name)) => Effect::OpenNewBranchDialog {
                start: Some(name.clone()),
            },
            (Action::NewBranch, _) => Effect::OpenNewBranchDialog { start: None },
            (Action::DeleteBranch, Args::Branch(name)) => {
                Effect::OpenDeleteBranchDialog(name.clone())
            }
            (Action::DeleteMergedBranches, _) => Effect::OpenMergedBranchesDialog,
            // The dialog is the confirmation, so neither sends the discard itself.
            (Action::DiscardFile | Action::DiscardAllChanges, Args::File(path)) => {
                Effect::OpenDiscardFileDialog {
                    path: path.clone(),
                    include_staged: self == Action::DiscardAllChanges,
                }
            }
            (
                Action::SuggestMessage
                | Action::CancelHook
                | Action::UseSuggestion
                | Action::DiscardSuggestion
                | Action::ApproveHook
                | Action::DenyHook,
                _,
            ) => hook_effect(self, ctx)?,
            (action, _) if action.is_workflow_rule() => workflow_effect(action, args, ctx)?,
            // Only reached when `enabled` said Ok, which no placeholder does.
            _ => return Err(NOT_AVAILABLE),
        })
    }

    /// Whether `args` is the kind of target this action takes.
    fn takes(self, args: &Args) -> bool {
        matches!(
            (self.spec().args, args),
            (ArgKind::None, Args::None)
                | (ArgKind::OptionalBranch, Args::None | Args::Branch(_))
                | (ArgKind::Branch, Args::Branch(_))
                | (ArgKind::File, Args::File(_))
                | (ArgKind::Chunk, Args::Chunk { .. })
                | (ArgKind::Worktree, Args::Worktree(_))
                | (ArgKind::Commit, Args::Commit(_))
                | (ArgKind::Workflow, Args::Workflow(_))
                | (ArgKind::Template, Args::Template(_))
        )
    }
}

/// The rule of the actions that create or delete a local branch. A delete
/// refuses the branch that is checked out, and one another worktree has.
fn branch_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let Args::Branch(name) = args else {
        return Ok(());
    };
    let current = ctx.current_branch == Some(name.as_str());
    let elsewhere = ctx
        .worktrees
        .iter()
        .any(|entry| entry.branch.as_deref() == Some(name.as_str()));
    match action {
        Action::DeleteBranch if current => Err("Cannot delete the checked-out branch"),
        Action::DeleteBranch if elsewhere => Err("Checked out in another worktree"),
        _ => Ok(()),
    }
}

/// The rules of the working copy's changes: staging and unstaging a file or a
/// hunk, discarding a hunk and committing. Each needs an idle repository.
fn change_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    match (action, args) {
        (Action::StageFile, _) => {
            let entry = file_entry(args, ctx)?;
            if has_unstaged(entry) {
                Ok(())
            } else {
                Err("Nothing unstaged in this file")
            }
        }
        (Action::UnstageFile, _) => {
            let entry = file_entry(args, ctx)?;
            if entry.staged.is_some() {
                Ok(())
            } else {
                Err("Nothing staged in this file")
            }
        }
        (
            Action::StageChunk,
            Args::Chunk {
                side: DiffSide::Unstaged,
                ..
            },
        )
        | (
            Action::DiscardChunk,
            Args::Chunk {
                side: DiffSide::Unstaged,
                ..
            },
        )
        | (
            Action::UnstageChunk,
            Args::Chunk {
                side: DiffSide::Staged,
                ..
            },
        ) => Ok(()),
        (Action::StageChunk, _) => Err("This chunk is already staged"),
        (Action::UnstageChunk, _) => Err("This chunk is not staged"),
        (Action::DiscardChunk, _) => Err("Only unstaged chunks can be discarded"),
        // Commit, the only action left.
        _ => {
            if ctx.message.trim().is_empty() {
                Err("Nothing to commit: the message is empty.")
            } else if ctx.staged_count == 0 {
                Err("Nothing is staged.")
            } else {
                Ok(())
            }
        }
    }
}

/// The rule of the two discard actions: an idle repository and a listed file
/// that is not conflicted, with something to throw away. Discard Changes needs
/// an unstaged change or an untracked file; Discard All Changes needs any change.
fn discard_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let entry = file_entry(args, ctx)?;
    if entry.conflicted {
        return Err("Resolve the conflict first.");
    }
    match action {
        Action::DiscardAllChanges if entry.staged.is_some() || has_unstaged(entry) => Ok(()),
        Action::DiscardAllChanges => Err("Nothing to discard"),
        _ if has_unstaged(entry) => Ok(()),
        _ => Err("Nothing unstaged to discard"),
    }
}

/// The rules of the prepare-commit actions: what the window's hook state allows.
fn hook_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    match action {
        Action::SuggestMessage => {
            idle(ctx)?;
            if ctx.hooks.running.is_some() {
                return Err("A hook is already running.");
            }
            match ctx.hooks.applicable {
                None => Err("Hooks are still being read."),
                Some(0) => Err("No prepare-commit hook applies to this repository."),
                Some(_) => Ok(()),
            }
        }
        Action::CancelHook => ctx.hooks.running.map(|_| ()).ok_or("No hook is running."),
        Action::UseSuggestion | Action::DiscardSuggestion => ctx
            .hooks
            .suggestion
            .map(|_| ())
            .ok_or("There is no suggestion."),
        Action::ApproveHook | Action::DenyHook => ctx
            .hooks
            .pending_approval
            .map(|_| ())
            .ok_or("No repository hook waits for approval."),
        _ => Err(NOT_AVAILABLE),
    }
}

/// The rules of the manual workflow actions.
fn workflow_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    match action {
        Action::RunWorkflow => workflow_run_rule(args, ctx).map(|_| ()),
        Action::ApproveWorkflowStep | Action::RejectWorkflowStep => ctx
            .workflows
            .awaiting
            .map(|_| ())
            .ok_or("No workflow run waits for approval."),
        Action::NewWorkflowFromTemplate => template_rule(args, ctx),
        Action::ReloadWorkflows => repository_open(ctx),
        Action::OpenWorkflowsFolder => workflows_folder(ctx).map(|_| ()),
        Action::SaveWorkflow => {
            if page_open(ctx)?.dirty {
                Ok(())
            } else {
                Err("Nothing to save: the file has no changes.")
            }
        }
        Action::RevertWorkflow => {
            if page_open(ctx)?.dirty {
                Ok(())
            } else {
                Err("Nothing to revert: the file has no changes.")
            }
        }
        Action::DeleteWorkflow => page_open(ctx).map(|_| ()),
        Action::AddWorkflowState
        | Action::RemoveWorkflowState
        | Action::MoveWorkflowStateUp
        | Action::MoveWorkflowStateDown => flow_edit_rule(action, ctx),
        Action::DryRunWorkflow => dry_run_rule(ctx),
        Action::SetWorkflowStateCall => {
            inspector_rule(ctx)?;
            match page_open(ctx)?.flow.selected {
                Some(_) => Ok(()),
                None => Err("Select a state in the Flow first."),
            }
        }
        Action::SetWorkflowTrigger | Action::AddWorkflowInput => inspector_rule(ctx),
        _ => ctx
            .workflows
            .active
            .map(|_| ())
            .ok_or("No workflow run is active."),
    }
}

/// What a manual workflow action sends, given the workflow state in `ctx`.
fn workflow_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (Action::RunWorkflow, Args::Workflow(name)) => {
            let workflow = workflow_run_rule(args, ctx)?;
            if workflow.inputs.is_empty() {
                Effect::Send(vec![CommandKind::RunWorkflow {
                    name: name.clone(),
                    inputs: BTreeMap::new(),
                }])
            } else {
                Effect::OpenWorkflowInputs {
                    workflow: name.clone(),
                    inputs: workflow.inputs.clone(),
                }
            }
        }
        (Action::NewWorkflowFromTemplate, Args::Template(name)) => {
            Effect::Send(vec![CommandKind::CreateWorkflowFromTemplate {
                template: name.clone(),
            }])
        }
        (Action::ReloadWorkflows, _) => Effect::Send(vec![CommandKind::ListWorkflows]),
        (Action::SaveWorkflow, _) => Effect::SaveWorkflow,
        (Action::RevertWorkflow, _) => Effect::RevertWorkflow,
        (Action::DeleteWorkflow, _) => Effect::OpenDeleteWorkflow,
        (Action::AddWorkflowState, _) => Effect::EditFlow(FlowEdit::Add),
        (Action::RemoveWorkflowState, _) => Effect::EditFlow(FlowEdit::Remove),
        (Action::MoveWorkflowStateUp, _) => Effect::EditFlow(FlowEdit::Move(Direction::Up)),
        (Action::MoveWorkflowStateDown, _) => Effect::EditFlow(FlowEdit::Move(Direction::Down)),
        (Action::DryRunWorkflow, _) => Effect::DryRunWorkflow,
        (Action::SetWorkflowStateCall, _) => Effect::OpenInspectorPicker(PickerKind::Action),
        (Action::SetWorkflowTrigger, _) => Effect::OpenInspectorPicker(PickerKind::Trigger),
        (Action::AddWorkflowInput, _) => Effect::FocusWorkflowInput,
        (Action::OpenWorkflowsFolder, _) => Effect::Reveal(PathBuf::from(workflows_folder(ctx)?)),
        (Action::ApproveWorkflowStep | Action::RejectWorkflowStep, _) => {
            let Some((run, _)) = ctx.workflows.awaiting else {
                return Err("No workflow run waits for approval.");
            };
            Effect::Send(vec![CommandKind::ApproveWorkflowStep {
                run,
                approved: action == Action::ApproveWorkflowStep,
            }])
        }
        (Action::CancelWorkflowRun, _) => match ctx.workflows.active {
            Some(run) => Effect::Send(vec![CommandKind::CancelWorkflowRun { run }]),
            None => return Err("No workflow run is active."),
        },
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The rule of the Flow's structural edits: an open file that parses, a state
/// selected for the remove and the moves, and a move that has room.
fn flow_edit_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    let page = page_open(ctx)?;
    if !page.flow.parses {
        return Err("Fix the file in the TOML tab first.");
    }
    if action == Action::AddWorkflowState {
        return Ok(());
    }
    let Some(index) = page.flow.selected else {
        return Err("Select a state in the Flow first.");
    };
    match action {
        Action::RemoveWorkflowState if page.flow.states == 1 => {
            Err("A workflow needs at least one state.")
        }
        Action::MoveWorkflowStateUp if index == 0 => Err("This state is already the first."),
        Action::MoveWorkflowStateDown if index + 1 == page.flow.states => {
            Err("This state is already the last.")
        }
        _ => Ok(()),
    }
}

/// The rule of the inspector's pickers and its new-input field: the Flow tab
/// shows, and the file parses so the inspector has its values.
fn inspector_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    let page = page_open(ctx)?;
    if !page.flow.shown {
        return Err("Open the Flow tab first.");
    }
    if !page.flow.parses {
        return Err("Fix the file in the TOML tab first.");
    }
    Ok(())
}

/// The rule of Dry Run: the page has an open file that parses. A save in flight
/// does not stop it; the dry run walks the buffer, not the file.
fn dry_run_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    let page = ctx.workflows.page;
    if !page.shown {
        return Err("Open the Workflows page first.");
    }
    if page.name.is_none() {
        return Err("No workflow is open.");
    }
    if !page.flow.parses {
        return Err("Fix the file in the TOML tab first.");
    }
    Ok(())
}

/// The open workflow of the Workflows page, when the page is the screen and a
/// workflow is open. Otherwise the reason the page cannot act.
fn page_open<'a>(ctx: &Context<'a>) -> Result<PageState<'a>, Reason> {
    let page = ctx.workflows.page;
    if !page.shown {
        return Err("Open the Workflows page first.");
    }
    if page.name.is_none() {
        return Err("No workflow is open.");
    }
    if page.saving {
        return Err("Wait for the save to finish.");
    }
    Ok(page)
}

/// The reason of the workflows folder when the engine has no folder to give.
const NO_WORKFLOWS_FOLDER: Reason = "No workflows folder: HOME is not set";

/// The rule of Open Workflows Folder: the listing gave a folder to open.
fn workflows_folder<'a>(ctx: &Context<'a>) -> Result<&'a str, Reason> {
    ctx.workflows.user_dir.ok_or(NO_WORKFLOWS_FOLDER)
}

/// The rule of New Workflow from Template: the listing named the template.
fn template_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    let Args::Template(name) = args else {
        return Err("No target for this action");
    };
    let templates = ctx.workflows.templates;
    if templates.is_empty() {
        return Err("No templates listed yet");
    }
    if templates.iter().any(|template| template == name) {
        Ok(())
    } else {
        Err("No template has that name.")
    }
}

/// The rule of Run Workflow: a repository is open and the named workflow is
/// listed, loads and is triggered by hand. A workflow with inputs may run: its
/// run asks for them first.
fn workflow_run_rule<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a WorkflowInfo, Reason> {
    repository_open(ctx)?;
    let Args::Workflow(name) = args else {
        return Err("No target for this action");
    };
    let listed = ctx
        .workflows
        .listed
        .ok_or("Workflows are still being read.")?;
    let workflow = listed
        .iter()
        .find(|workflow| workflow.name == *name)
        .ok_or("No workflow has that name.")?;
    if workflow.error.is_some() {
        return Err("This workflow does not load: fix its file first.");
    }
    if workflow.trigger != "manual" {
        return Err("Only manual workflows can be run from here.");
    }
    // The run reads the file on disk, so the edit in the page must be saved first.
    if ctx.workflows.page.dirty && ctx.workflows.page.name == Some(workflow.name.as_str()) {
        return Err("Save first");
    }
    Ok(workflow)
}

/// The action the Space key runs on a file: stage what is unstaged, otherwise
/// unstage. Its enabled rule is the one of that action.
#[must_use]
pub fn toggle_file_action(entry: &StatusEntry) -> Action {
    if has_unstaged(entry) {
        Action::StageFile
    } else {
        Action::UnstageFile
    }
}

/// Whether the working tree differs from the index for this entry. An
/// untracked file counts.
#[must_use]
pub fn has_unstaged(entry: &StatusEntry) -> bool {
    entry.unstaged.is_some() || entry.untracked || entry.conflicted
}

/// Refuses while no repository is open.
/// The rules of the Stashes page and of Stash Changes. The page needs a
/// repository; stashing needs an idle one with something to stash.
fn stash_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    match action {
        Action::NewStash => {
            idle(ctx)?;
            if ctx.entries.is_empty() {
                Err("No local changes to stash.")
            } else {
                Ok(())
            }
        }
        _ => repository_open(ctx),
    }
}

fn repository_open(ctx: &Context<'_>) -> Result<(), Reason> {
    if ctx.repo_open {
        Ok(())
    } else {
        Err("No repository is open.")
    }
}

/// The rule of the remote commands: a repository, no change in flight and no
/// other remote operation. Pull and push need a checked-out branch, and push
/// takes only that branch, so the engine never has to guess one.
fn remote_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    if ctx.remote_running {
        return Err("A remote operation is running");
    }
    if action == Action::Fetch {
        return Ok(());
    }
    let Some(branch) = ctx.current_branch else {
        return Err("Check out a branch first: HEAD is detached.");
    };
    match args {
        Args::Branch(name) if name.as_str() != branch => {
            Err("Only the checked-out branch can be pushed.")
        }
        _ => Ok(()),
    }
}

/// Refuses while an open or a checkout is in flight.
fn idle(ctx: &Context<'_>) -> Result<(), Reason> {
    if !ctx.repo_open {
        return Err("No repository is open.");
    }
    ctx.busy.map_or(Ok(()), Err)
}

fn file_entry<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a StatusEntry, Reason> {
    let Args::File(path) = args else {
        return Err("No target for this action");
    };
    ctx.entries
        .iter()
        .find(|entry| entry.path == *path)
        .ok_or("That file is not in the working copy")
}

/// What a hook action does, given the state of the hooks in `ctx`.
fn hook_effect(action: Action, ctx: &Context<'_>) -> Result<Effect, Reason> {
    Ok(match action {
        Action::SuggestMessage => Effect::Send(vec![CommandKind::RunPrepareCommit {
            message: ctx.message.to_owned(),
        }]),
        Action::CancelHook => match ctx.hooks.running {
            Some(run) => Effect::Send(vec![CommandKind::CancelHook { run }]),
            None => return Err("No hook is running."),
        },
        Action::UseSuggestion => match ctx.hooks.suggestion {
            Some(text) => Effect::UseSuggestion(text.to_owned()),
            None => return Err("There is no suggestion."),
        },
        Action::DiscardSuggestion => Effect::DiscardSuggestion,
        Action::ApproveHook | Action::DenyHook => {
            let Some((name, hash)) = ctx.hooks.pending_approval else {
                return Err("No repository hook waits for approval.");
            };
            if action == Action::ApproveHook {
                Effect::Send(vec![CommandKind::ApproveHook {
                    name: name.to_owned(),
                    command_hash: hash.to_owned(),
                }])
            } else {
                Effect::DenyHook(hash.to_owned())
            }
        }
        _ => return Err(NOT_AVAILABLE),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::workflows_page::FlowPosition;
    use gm_schema::{Change, WorkflowInfo, WorkflowInput};

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
        }
    }

    fn untracked(path: &str) -> StatusEntry {
        StatusEntry {
            untracked: true,
            ..entry(path, None, None)
        }
    }

    fn ctx<'a>(entries: &'a [StatusEntry], message: &'a str) -> Context<'a> {
        Context {
            repo_open: true,
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
            workflows: WorkflowCtx::default(),
            worktrees: &[],
            branch_scope: Scope::All,
            user_email_known: false,
            remote_running: false,
        }
    }

    fn chunk(side: DiffSide) -> Args {
        Args::Chunk {
            path: "a.txt".into(),
            hunk: HunkId("h1".into()),
            side,
        }
    }

    #[test]
    fn ids_are_unique_and_every_action_has_a_title() {
        let ids: HashSet<&str> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        for action in Action::ALL {
            assert!(!action.spec().title.is_empty());
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("nope"), None);
    }

    #[test]
    fn the_branch_menu_shortcuts_are_the_designs() {
        let expected = [
            (Action::CheckoutBranch, "Enter"),
            (Action::NewChildBranch, "⌘B"),
            (Action::CreateWorktree, "⌥⌘W"),
            (Action::Restack, "⇧⌘R"),
            (Action::PushWithLease, "⇧⌘P"),
            (Action::CopyBranchName, "⌥⌘C"),
            (Action::RenameBranch, "F2"),
            (Action::DeleteBranch, "⌫"),
        ];
        for (action, shortcut) in expected {
            assert_eq!(action.spec().shortcut, Some(shortcut), "{action:?}");
        }
    }

    #[test]
    fn shortcuts_read_as_words_off_macos_with_cmd_as_ctrl() {
        assert_eq!(shortcut_words("⌘↵", false), "Ctrl+Enter");
        assert_eq!(shortcut_words("⌥⌘C", false), "Alt+Ctrl+C");
        assert_eq!(shortcut_words("⇧⌘P", false), "Shift+Ctrl+P");
        assert_eq!(shortcut_words("⌫", false), "Backspace");
        assert_eq!(shortcut_words("Space", false), "Space");
        assert_eq!(shortcut_words("F2", false), "F2");
        assert_eq!(shortcut_words("⌥⌘C", true), "⌥⌘C");
    }

    #[test]
    fn the_spec_table_follows_the_order_of_the_variants() {
        for (index, action) in Action::ALL.into_iter().enumerate() {
            assert_eq!(action as usize, index);
        }
    }

    #[test]
    fn ids_are_dotted_lowercase_and_stable() {
        assert_eq!(Action::StageFile.id(), "file.stage");
        assert_eq!(Action::DiscardChunk.id(), "chunk.discard");
        assert_eq!(Action::Commit.id(), "commit.create");
        assert_eq!(Action::Refresh.id(), "repo.refresh");
        assert_eq!(Action::CheckoutBranch.id(), "branch.checkout");
    }

    #[test]
    fn placeholders_are_disabled_with_the_same_reason_whatever_the_target() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = ctx(&entries, "msg");
        let placeholders = [
            Action::NewChildBranch,
            Action::Restack,
            Action::RenameBranch,
        ];
        for action in placeholders {
            let args = match action.spec().args {
                ArgKind::Branch => Args::Branch("main".into()),
                ArgKind::File => Args::File("a.txt".into()),
                _ => Args::None,
            };
            assert_eq!(action.enabled(&args, &context), Err("Not available yet"));
            assert_eq!(action.plan(&args, &context), Err("Not available yet"));
        }
    }

    #[test]
    fn available_is_false_exactly_for_the_placeholders_that_refuse_every_target() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = ctx(&entries, "msg");
        for action in Action::ALL {
            let args = match action.spec().args {
                ArgKind::Branch => Args::Branch("main".into()),
                ArgKind::File => Args::File("a.txt".into()),
                _ => Args::None,
            };
            let refused = action.enabled(&args, &context) == Err(NOT_AVAILABLE);
            assert_eq!(action.available(), !refused, "{action:?}");
        }
    }

    #[test]
    fn fetch_pull_and_push_are_enabled_on_a_branch_and_send_their_command() {
        let context = ctx(&[], "");
        assert!(Action::Fetch.available());
        assert!(Action::Pull.available());
        assert!(Action::PushWithLease.available());
        assert_eq!(
            Action::Fetch.plan(&Args::None, &context),
            Ok(Effect::Send(vec![CommandKind::Fetch]))
        );
        assert_eq!(
            Action::Pull.plan(&Args::None, &context),
            Ok(Effect::Send(vec![CommandKind::Pull]))
        );
        assert_eq!(
            Action::PushWithLease.plan(&Args::Branch("main".into()), &context),
            Ok(Effect::Send(vec![CommandKind::PushWithLease]))
        );
    }

    #[test]
    fn a_remote_operation_in_flight_refuses_all_three_with_one_reason() {
        let entries: [StatusEntry; 0] = [];
        let running = Context {
            remote_running: true,
            ..ctx(&entries, "")
        };
        for (action, args) in [
            (Action::Fetch, Args::None),
            (Action::Pull, Args::None),
            (Action::PushWithLease, Args::Branch("main".into())),
        ] {
            assert_eq!(
                action.enabled(&args, &running),
                Err("A remote operation is running"),
                "{action:?}"
            );
            assert_eq!(
                action.plan(&args, &running),
                Err("A remote operation is running"),
                "{action:?}"
            );
        }
    }

    #[test]
    fn remote_commands_need_a_repository_and_a_branch_to_act_on() {
        let entries: [StatusEntry; 0] = [];
        let closed = Context {
            repo_open: false,
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::Fetch.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        let detached = Context {
            current_branch: None,
            ..ctx(&entries, "")
        };
        assert_eq!(Action::Fetch.enabled(&Args::None, &detached), Ok(()));
        assert_eq!(
            Action::Pull.enabled(&Args::None, &detached),
            Err("Check out a branch first: HEAD is detached.")
        );
        assert_eq!(
            Action::PushWithLease.enabled(&Args::Branch(String::new()), &detached),
            Err("Check out a branch first: HEAD is detached.")
        );
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::Fetch.enabled(&Args::None, &busy),
            Err("Wait for the branch checkout to finish.")
        );
    }

    #[test]
    fn push_with_lease_pushes_only_the_checked_out_branch() {
        let entries: [StatusEntry; 0] = [];
        let context = ctx(&entries, "");
        assert_eq!(
            Action::PushWithLease.enabled(&Args::Branch("main".into()), &context),
            Ok(())
        );
        assert_eq!(
            Action::PushWithLease.enabled(&Args::Branch("feature".into()), &context),
            Err("Only the checked-out branch can be pushed.")
        );
    }

    #[test]
    fn create_worktree_is_enabled_for_a_branch_and_plans_the_dialog() {
        let context = ctx(&[], "");
        let args = Args::Branch("feature".into());
        assert_eq!(Action::CreateWorktree.enabled(&args, &context), Ok(()));
        assert_eq!(
            Action::CreateWorktree.plan(&args, &context),
            Ok(Effect::OpenWorktreeDialog {
                branch: Some("feature".into())
            })
        );
        assert!(Action::CreateWorktree.available());
    }

    #[test]
    fn new_worktree_needs_only_an_open_repository_and_opens_the_dialog_empty() {
        let context = ctx(&[], "");
        assert_eq!(Action::NewWorktree.enabled(&Args::None, &context), Ok(()));
        assert_eq!(
            Action::NewWorktree.plan(&Args::None, &context),
            Ok(Effect::OpenWorktreeDialog { branch: None })
        );
        assert!(Action::NewWorktree.available());
        assert!(Action::NewWorktree.opens_dialog());
        assert_eq!(Action::NewWorktree.spec().args, ArgKind::None);
        assert_eq!(Action::NewWorktree.spec().category, Category::Worktrees);
        assert_eq!(Action::NewWorktree.spec().title, "New Worktree…");
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::NewWorktree.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::NewWorktree.enabled(&Args::Branch("main".into()), &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn the_worktree_actions_that_act_on_a_listed_worktree_take_a_worktree() {
        let on_a_worktree = [
            Action::OpenWorktree,
            Action::RevealWorktree,
            Action::CopyWorktreePath,
            Action::RemoveWorktree,
            Action::LockWorktree,
            Action::UnlockWorktree,
        ];
        for action in on_a_worktree {
            assert_eq!(action.spec().args, ArgKind::Worktree, "{}", action.id());
        }
        // Create, New and Prune work on the repository, not on a listed worktree.
        assert_eq!(Action::CreateWorktree.spec().args, ArgKind::Branch);
        assert_eq!(Action::NewWorktree.spec().args, ArgKind::None);
        assert_eq!(Action::PruneWorktrees.spec().args, ArgKind::None);
    }

    #[test]
    fn a_worktree_action_takes_only_a_worktree_target() {
        let context = ctx(&[], "");
        let target = Args::Worktree("/repo-feature".into());
        assert_eq!(
            Action::RemoveWorktree.enabled(&Args::None, &context),
            Err("No target for this action")
        );
        assert_eq!(
            Action::RemoveWorktree.enabled(&Args::Branch("main".into()), &context),
            Err("No target for this action")
        );
        // A listed worktree's rule decides; this one is not listed, so it says so.
        assert_eq!(
            Action::RemoveWorktree.enabled(&target, &context),
            Err("This worktree is not listed.")
        );
    }

    #[test]
    fn create_worktree_needs_an_open_repository_and_a_branch() {
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::CreateWorktree.enabled(&Args::Branch("main".into()), &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::CreateWorktree.enabled(&Args::None, &ctx(&[], "")),
            Err("No target for this action")
        );
    }

    #[test]
    fn a_run_with_the_wrong_kind_of_target_is_refused() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::Refresh.enabled(&Args::File("a".into()), &context),
            Err("No target for this action")
        );
        assert_eq!(
            Action::StageFile.enabled(&Args::None, &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn stage_file_needs_an_unstaged_side_and_unstage_needs_a_staged_one() {
        let entries = [
            entry("staged.txt", Some(Change::Added), None),
            entry("both.txt", Some(Change::Modified), Some(Change::Modified)),
            untracked("new.txt"),
        ];
        let context = ctx(&entries, "");
        let file = |p: &str| Args::File(p.into());
        assert_eq!(
            Action::StageFile.enabled(&file("staged.txt"), &context),
            Err("Nothing unstaged in this file")
        );
        assert_eq!(
            Action::StageFile.enabled(&file("both.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageFile.enabled(&file("new.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::UnstageFile.enabled(&file("new.txt"), &context),
            Err("Nothing staged in this file")
        );
        assert_eq!(
            Action::UnstageFile.enabled(&file("both.txt"), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageFile.enabled(&file("gone.txt"), &context),
            Err("That file is not in the working copy")
        );
    }

    #[test]
    fn stage_file_plans_the_engine_command_for_the_path() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let effect = Action::StageFile
            .plan(&Args::File("a.txt".into()), &ctx(&entries, ""))
            .unwrap();
        assert_eq!(
            effect,
            Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::File {
                    path: "a.txt".into()
                }
            }])
        );
    }

    #[test]
    fn space_stages_what_is_unstaged_and_otherwise_unstages() {
        assert_eq!(
            toggle_file_action(&entry("a", Some(Change::Modified), Some(Change::Modified))),
            Action::StageFile
        );
        assert_eq!(
            toggle_file_action(&entry("a", Some(Change::Added), None)),
            Action::UnstageFile
        );
        assert_eq!(toggle_file_action(&untracked("a")), Action::StageFile);
    }

    #[test]
    fn commit_says_why_it_cannot_run() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&entries, "  ")),
            Err("Nothing to commit: the message is empty.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&[], "subject")),
            Err("Nothing is staged.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &ctx(&entries, "subject")),
            Ok(())
        );
    }

    #[test]
    fn copying_a_commit_takes_the_commit_it_names() {
        let context = ctx(&[], "");
        let hash = Args::Commit("3f2a9c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39".into());
        assert_eq!(
            Action::CopyCommitHash.plan(&hash, &context),
            Ok(Effect::Copy(
                "3f2a9c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39".into()
            ))
        );
        assert_eq!(
            Action::CopyCommitMessage.plan(&Args::Commit("fix: a\n\nbody".into()), &context),
            Ok(Effect::CopyMessage("fix: a\n\nbody".into()))
        );
        assert_eq!(
            Action::CopyCommitHash.enabled(&Args::None, &context),
            Err("No target for this action")
        );
        assert_eq!(
            Action::CopyCommitMessage.enabled(&Args::File("a.txt".into()), &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn commit_sends_the_message_as_typed() {
        let entries = [entry("a.txt", Some(Change::Added), None)];
        let effect = Action::Commit
            .plan(&Args::None, &ctx(&entries, "Fix it\n\nBody"))
            .unwrap();
        assert_eq!(
            effect,
            Effect::Send(vec![CommandKind::Commit {
                message: "Fix it\n\nBody".into()
            }])
        );
    }

    #[test]
    fn a_busy_repository_refuses_changes_but_not_refresh_or_copy() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&entries, "msg")
        };
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::StageFile.enabled(&file, &busy),
            Err("Wait for the branch checkout to finish.")
        );
        assert_eq!(Action::Refresh.enabled(&Args::None, &busy), Ok(()));
        assert_eq!(
            Action::CopyFilePath.plan(&file, &busy),
            Ok(Effect::Copy("a.txt".into()))
        );
        assert_eq!(
            Action::FocusCommitMessage.plan(&Args::None, &busy),
            Ok(Effect::FocusCommitMessage)
        );
    }

    #[test]
    fn no_repository_refuses_refresh_and_changes() {
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::Refresh.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::Commit.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn stage_and_unstage_chunk_follow_the_side_they_are_shown_on() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::StageChunk.enabled(&chunk(DiffSide::Unstaged), &context),
            Ok(())
        );
        assert_eq!(
            Action::StageChunk.enabled(&chunk(DiffSide::Staged), &context),
            Err("This chunk is already staged")
        );
        assert_eq!(
            Action::UnstageChunk.enabled(&chunk(DiffSide::Staged), &context),
            Ok(())
        );
        assert_eq!(
            Action::UnstageChunk.enabled(&chunk(DiffSide::Unstaged), &context),
            Err("This chunk is not staged")
        );
        assert_eq!(
            Action::StageChunk.plan(&chunk(DiffSide::Unstaged), &context),
            Ok(Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::Hunk {
                    path: "a.txt".into(),
                    hunk: HunkId("h1".into()),
                }
            }]))
        );
    }

    #[test]
    fn discard_chunk_is_only_for_unstaged_chunks_and_sends_the_discard() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::DiscardChunk.enabled(&chunk(DiffSide::Staged), &context),
            Err("Only unstaged chunks can be discarded")
        );
        assert_eq!(
            Action::DiscardChunk.plan(&chunk(DiffSide::Unstaged), &context),
            Ok(Effect::Send(vec![CommandKind::DiscardHunk {
                path: "a.txt".into(),
                hunk: HunkId("h1".into()),
            }]))
        );
    }

    #[test]
    fn reveal_needs_a_repository_and_a_listed_file() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::RevealFile.plan(&file, &ctx(&entries, "")),
            Ok(Effect::Reveal(PathBuf::from("/repo/a.txt")))
        );
        let closed = Context {
            repo_open: false,
            root: None,
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::RevealFile.enabled(&file, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::RevealFile.enabled(&Args::File("gone.txt".into()), &ctx(&entries, "")),
            Err("That file is not in the working copy")
        );
    }

    #[test]
    fn view_file_opens_the_viewer_on_a_listed_file_and_refuses_the_rest() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::ViewFile.plan(&file, &ctx(&entries, "")),
            Ok(Effect::ViewFile {
                path: "a.txt".into(),
                line: None
            })
        );
        assert_eq!(
            Action::ViewFile.enabled(&Args::File("gone.txt".into()), &ctx(&entries, "")),
            Err("That file is not in the working copy")
        );
        let closed = Context {
            repo_open: false,
            root: None,
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::ViewFile.enabled(&file, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ViewFile.id(), "file.view");
    }

    #[test]
    fn quick_open_and_show_code_need_a_repository_and_open_their_screens() {
        assert_eq!(
            Action::QuickOpen.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::OpenQuickOpen)
        );
        assert_eq!(
            Action::ShowCode.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowCode)
        );
        let closed = Context {
            repo_open: false,
            root: None,
            ..ctx(&[], "")
        };
        for action in [Action::QuickOpen, Action::ShowCode] {
            assert_eq!(
                action.enabled(&Args::None, &closed),
                Err("No repository is open.")
            );
        }
        assert_eq!(Action::QuickOpen.id(), "files.quick_open");
        assert_eq!(Action::QuickOpen.spec().shortcut, Some("⌘P"));
        assert_eq!(Action::ShowCode.id(), "code.show");
        assert_eq!(
            Action::SearchText.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowSearch)
        );
        assert_eq!(
            Action::SearchText.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::SearchText.id(), "search.text");
        assert_eq!(Action::SearchText.spec().shortcut, Some("⇧⌘F"));
    }

    #[test]
    fn only_throwing_work_away_is_destructive() {
        let destructive: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|action| action.destructive())
            .collect();
        assert_eq!(
            destructive,
            [
                Action::DiscardChunk,
                Action::DeleteBranch,
                Action::DiscardFile,
                Action::RemoveWorktree,
                Action::DeleteMergedBranches,
                Action::DiscardAllChanges,
            ]
        );
    }

    #[test]
    fn delete_merged_branches_opens_its_dialog_and_needs_an_idle_repository() {
        let action = Action::DeleteMergedBranches;
        assert_eq!(action.id(), "branch.delete_merged");
        assert_eq!(action.spec().title, "Delete Merged Branches…");
        assert_eq!(action.spec().category, Category::Branches);
        assert_eq!(action.spec().args, ArgKind::None);
        assert_eq!(action.spec().shortcut, None);
        assert!(action.opens_dialog());
        assert!(action.destructive());
        assert_eq!(
            action.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::OpenMergedBranchesDialog)
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            action.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&[], "")
        };
        assert_eq!(
            action.enabled(&Args::None, &busy),
            Err("Wait for the branch checkout to finish.")
        );
    }

    #[test]
    fn checkout_refuses_the_current_branch_and_sends_the_others() {
        let context = ctx(&[], "");
        let branch = |name: &str| Args::Branch(name.into());
        assert_eq!(
            Action::CheckoutBranch.enabled(&branch("main"), &context),
            Err("This is the current branch")
        );
        assert_eq!(
            Action::CheckoutBranch.plan(&branch("feature"), &context),
            Ok(Effect::Send(vec![CommandKind::Checkout {
                branch: "feature".into()
            }]))
        );
        assert_eq!(
            Action::CopyBranchName.plan(&branch("feature"), &context),
            Ok(Effect::Copy("feature".into()))
        );
    }

    fn listed(path: &str, branch: &str, is_main: bool) -> WorktreeEntry {
        WorktreeEntry {
            path: path.to_owned(),
            head: Some("abc".to_owned()),
            branch: Some(branch.to_owned()),
            detached: false,
            bare: false,
            is_main,
            locked: None,
            prunable: None,
        }
    }

    #[test]
    fn remove_refuses_the_main_checkout_the_windows_own_and_a_missing_folder() {
        let worktrees = [
            listed("/r", "main", true),
            listed("/r-feature", "feature", false),
            WorktreeEntry {
                prunable: Some(String::new()),
                ..listed("/r-gone", "gone", false)
            },
        ];
        let open_in = |root: &'static str| Context {
            root: Some(root),
            worktrees: &worktrees,
            ..ctx(&[], "")
        };
        let args = |path: &str| Args::Worktree(path.to_owned());
        assert_eq!(
            Action::RemoveWorktree.enabled(&args("/r"), &open_in("/r-feature")),
            Err("The main checkout cannot be removed.")
        );
        assert_eq!(
            Action::RemoveWorktree.enabled(&args("/r-feature"), &open_in("/r-feature")),
            Err("This window is open in this worktree.")
        );
        assert_eq!(
            Action::RemoveWorktree.enabled(&args("/r-gone"), &open_in("/r")),
            Err("Its folder is missing. Prune the worktree instead.")
        );
        assert_eq!(
            Action::RemoveWorktree.plan(&args("/r-feature"), &open_in("/r")),
            Ok(Effect::OpenRemoveDialog("/r-feature".to_owned()))
        );
    }

    #[test]
    fn open_reveal_and_copy_run_on_a_listed_worktree_only() {
        let worktrees = [
            listed("/r", "main", true),
            listed("/r-feature", "feature", false),
        ];
        let context = Context {
            root: Some("/r"),
            worktrees: &worktrees,
            ..ctx(&[], "")
        };
        let target = Args::Worktree("/r-feature".to_owned());
        assert_eq!(
            Action::OpenWorktree.plan(&target, &context),
            Ok(Effect::OpenWorktree("/r-feature".to_owned()))
        );
        assert_eq!(
            Action::RevealWorktree.plan(&target, &context),
            Ok(Effect::Reveal(PathBuf::from("/r-feature")))
        );
        assert_eq!(
            Action::CopyWorktreePath.plan(&target, &context),
            Ok(Effect::Copy("/r-feature".to_owned()))
        );
        let unknown = Args::Worktree("/elsewhere".to_owned());
        assert_eq!(
            Action::CopyWorktreePath.enabled(&unknown, &context),
            Err("This worktree is not listed.")
        );
        let own = Context {
            root: Some("/r-feature"),
            worktrees: &worktrees,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::OpenWorktree.enabled(&target, &own),
            Err("This window is open in this worktree.")
        );
    }

    #[test]
    fn lock_and_unlock_follow_the_lock_state_and_send_the_path() {
        let target = Args::Worktree("/r-feature".to_owned());
        let unlocked = [
            listed("/r", "main", true),
            listed("/r-feature", "feature", false),
        ];
        let context = Context {
            root: Some("/r"),
            worktrees: &unlocked,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::LockWorktree.plan(&target, &context),
            Ok(Effect::Send(vec![CommandKind::LockWorktree {
                path: "/r-feature".to_owned(),
                reason: String::new(),
            }]))
        );
        assert_eq!(
            Action::UnlockWorktree.enabled(&target, &context),
            Err("It is not locked.")
        );

        let mut locked_entry = listed("/r-feature", "feature", false);
        locked_entry.locked = Some("usb".to_owned());
        let locked = [listed("/r", "main", true), locked_entry];
        let context = Context {
            root: Some("/r"),
            worktrees: &locked,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::LockWorktree.enabled(&target, &context),
            Err("It is already locked.")
        );
        assert_eq!(
            Action::UnlockWorktree.plan(&target, &context),
            Ok(Effect::Send(vec![CommandKind::UnlockWorktree {
                path: "/r-feature".to_owned(),
            }]))
        );
    }

    #[test]
    fn new_branch_is_enabled_with_or_without_a_branch_and_plans_the_dialog() {
        let context = ctx(&[], "");
        assert_eq!(Action::NewBranch.enabled(&Args::None, &context), Ok(()));
        assert_eq!(
            Action::NewBranch.plan(&Args::None, &context),
            Ok(Effect::OpenNewBranchDialog { start: None })
        );
        assert_eq!(
            Action::NewBranch.plan(&Args::Branch("feature".into()), &context),
            Ok(Effect::OpenNewBranchDialog {
                start: Some("feature".into())
            })
        );
        assert!(Action::NewBranch.available());
        assert!(Action::NewBranch.opens_dialog());
        assert_eq!(Action::NewBranch.spec().args, ArgKind::OptionalBranch);
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::NewBranch.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn delete_branch_refuses_the_checked_out_branch_and_one_another_worktree_has() {
        let worktrees = [
            listed("/r", "main", true),
            listed("/r-feature", "feature", false),
        ];
        let context = Context {
            root: Some("/r"),
            worktrees: &worktrees,
            ..ctx(&[], "")
        };
        let branch = |name: &str| Args::Branch(name.to_owned());
        assert_eq!(
            Action::DeleteBranch.enabled(&branch("main"), &context),
            Err("Cannot delete the checked-out branch")
        );
        assert_eq!(
            Action::DeleteBranch.enabled(&branch("feature"), &context),
            Err("Checked out in another worktree")
        );
        assert_eq!(
            Action::DeleteBranch.enabled(&branch("topic"), &context),
            Ok(())
        );
        assert_eq!(
            Action::DeleteBranch.plan(&branch("topic"), &context),
            Ok(Effect::OpenDeleteBranchDialog("topic".into()))
        );
        assert!(Action::DeleteBranch.available());
        assert!(Action::DeleteBranch.destructive());
        assert!(Action::DeleteBranch.opens_dialog());
    }

    #[test]
    fn delete_branch_needs_a_branch_target_and_no_busy_repository() {
        let entries: [StatusEntry; 0] = [];
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&entries, "")
        };
        assert_eq!(
            Action::DeleteBranch.enabled(&Args::Branch("topic".into()), &busy),
            Err("Wait for the branch checkout to finish.")
        );
        assert_eq!(
            Action::DeleteBranch.enabled(&Args::None, &ctx(&entries, "")),
            Err("No target for this action")
        );
    }

    #[test]
    fn discard_changes_needs_an_unstaged_change_or_an_untracked_file() {
        let file = Args::File("a.txt".into());
        let modified = [entry("a.txt", None, Some(Change::Modified))];
        let both = [entry(
            "a.txt",
            Some(Change::Modified),
            Some(Change::Modified),
        )];
        let new = [untracked("a.txt")];
        let staged_only = [entry("a.txt", Some(Change::Modified), None)];
        assert!(Action::DiscardFile.available());
        assert_eq!(
            Action::DiscardFile.enabled(&file, &ctx(&modified, "")),
            Ok(())
        );
        assert_eq!(Action::DiscardFile.enabled(&file, &ctx(&both, "")), Ok(()));
        assert_eq!(Action::DiscardFile.enabled(&file, &ctx(&new, "")), Ok(()));
        assert_eq!(
            Action::DiscardFile.enabled(&file, &ctx(&staged_only, "")),
            Err("Nothing unstaged to discard")
        );
    }

    #[test]
    fn discard_all_changes_needs_any_change_in_the_file() {
        let file = Args::File("a.txt".into());
        let staged_only = [entry("a.txt", Some(Change::Added), None)];
        let new = [untracked("a.txt")];
        let clean = [entry("a.txt", None, None)];
        assert!(Action::DiscardAllChanges.available());
        assert_eq!(
            Action::DiscardAllChanges.enabled(&file, &ctx(&staged_only, "")),
            Ok(())
        );
        assert_eq!(
            Action::DiscardAllChanges.enabled(&file, &ctx(&new, "")),
            Ok(())
        );
        assert_eq!(
            Action::DiscardAllChanges.enabled(&file, &ctx(&clean, "")),
            Err("Nothing to discard")
        );
    }

    #[test]
    fn both_discard_actions_refuse_a_conflicted_file_and_say_so() {
        let file = Args::File("a.txt".into());
        let conflicted = [StatusEntry {
            conflicted: true,
            ..entry("a.txt", None, Some(Change::Modified))
        }];
        for action in [Action::DiscardFile, Action::DiscardAllChanges] {
            assert_eq!(
                action.enabled(&file, &ctx(&conflicted, "")),
                Err("Resolve the conflict first."),
                "{action:?}"
            );
        }
    }

    #[test]
    fn both_discard_actions_refuse_an_unknown_file_a_busy_repository_and_a_closed_one() {
        let modified = [entry("a.txt", None, Some(Change::Modified))];
        let file = Args::File("a.txt".into());
        let other = Args::File("b.txt".into());
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&modified, "")
        };
        let closed = Context {
            repo_open: false,
            ..ctx(&modified, "")
        };
        for action in [Action::DiscardFile, Action::DiscardAllChanges] {
            assert_eq!(
                action.enabled(&other, &ctx(&modified, "")),
                Err("That file is not in the working copy"),
                "{action:?}"
            );
            assert_eq!(
                action.enabled(&file, &busy),
                Err("Wait for the branch checkout to finish."),
                "{action:?}"
            );
            assert_eq!(
                action.enabled(&file, &closed),
                Err("No repository is open."),
                "{action:?}"
            );
        }
    }

    #[test]
    fn discard_actions_open_the_dialog_and_never_send_the_command() {
        let modified = [entry("a.txt", None, Some(Change::Modified))];
        let file = Args::File("a.txt".into());
        assert_eq!(
            Action::DiscardFile.plan(&file, &ctx(&modified, "")),
            Ok(Effect::OpenDiscardFileDialog {
                path: "a.txt".into(),
                include_staged: false,
            })
        );
        assert_eq!(
            Action::DiscardAllChanges.plan(&file, &ctx(&modified, "")),
            Ok(Effect::OpenDiscardFileDialog {
                path: "a.txt".into(),
                include_staged: true,
            })
        );
        for action in [Action::DiscardFile, Action::DiscardAllChanges] {
            assert!(action.destructive(), "{action:?}");
            assert!(action.opens_dialog(), "{action:?}");
            assert_eq!(action.spec().args, ArgKind::File, "{action:?}");
            assert_eq!(action.spec().category, Category::WorkingCopy, "{action:?}");
            assert_eq!(action.spec().shortcut, None, "{action:?}");
        }
        assert_eq!(Action::DiscardAllChanges.id(), "file.discard_all");
        assert_eq!(
            Action::DiscardAllChanges.spec().title,
            "Discard All Changes"
        );
    }

    #[test]
    fn show_history_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowHistory.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowHistory)
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowHistory.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ShowHistory.id(), "history.show");
    }

    #[test]
    fn show_stashes_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowStashes.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowStashes)
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowStashes.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ShowStashes.id(), "stash.show");
        assert_eq!(Action::ShowStashes.spec().title, "Show Stashes");
        assert_eq!(Action::ShowStashes.spec().category, Category::Repository);
        assert_eq!(Action::ShowStashes.spec().shortcut, None);
        assert!(!Action::ShowStashes.opens_dialog());
    }

    #[test]
    fn stash_changes_opens_its_dialog_and_needs_changes_to_stash() {
        let modified = [entry("a.txt", None, Some(Change::Modified))];
        assert_eq!(
            Action::NewStash.plan(&Args::None, &ctx(&modified, "")),
            Ok(Effect::OpenNewStashDialog)
        );
        assert_eq!(Action::NewStash.id(), "stash.new");
        assert_eq!(Action::NewStash.spec().title, "Stash Changes…");
        assert_eq!(Action::NewStash.spec().category, Category::WorkingCopy);
        assert_eq!(Action::NewStash.spec().shortcut, None);
        assert!(Action::NewStash.opens_dialog());
        assert!(!Action::NewStash.destructive());
        assert_eq!(
            Action::NewStash.enabled(&Args::None, &ctx(&[], "")),
            Err("No local changes to stash.")
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&modified, "")
        };
        assert_eq!(
            Action::NewStash.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        let busy = Context {
            busy: Some("Wait for the branch checkout to finish."),
            ..ctx(&modified, "")
        };
        assert_eq!(
            Action::NewStash.enabled(&Args::None, &busy),
            Err("Wait for the branch checkout to finish.")
        );
    }

    #[test]
    fn the_branch_scope_actions_switch_to_the_other_scope() {
        let mine = Context {
            branch_scope: Scope::Mine,
            user_email_known: true,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowAllBranches.plan(&Args::None, &mine),
            Ok(Effect::SetBranchScope(Scope::All))
        );
        assert_eq!(
            Action::ShowOnlyMyBranches.enabled(&Args::None, &mine),
            Err("Already showing only your branches.")
        );
        let all = Context {
            branch_scope: Scope::All,
            user_email_known: true,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowOnlyMyBranches.plan(&Args::None, &all),
            Ok(Effect::SetBranchScope(Scope::Mine))
        );
        assert_eq!(
            Action::ShowAllBranches.enabled(&Args::None, &all),
            Err("Already showing all branches.")
        );
        assert_eq!(Action::ShowOnlyMyBranches.id(), "branch.show_mine");
        assert_eq!(Action::ShowAllBranches.id(), "branch.show_all");
    }

    #[test]
    fn show_only_my_branches_needs_a_known_email_and_a_repository() {
        // Without an email, Mine would show every branch: the run is refused and says why.
        let no_email = ctx(&[], "");
        assert_eq!(
            Action::ShowOnlyMyBranches.enabled(&Args::None, &no_email),
            Err("Git has no user.email to match branches against.")
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowAllBranches.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn prune_opens_its_dialog_and_needs_an_open_repository() {
        assert_eq!(
            Action::PruneWorktrees.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::OpenPruneDialog)
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::PruneWorktrees.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    fn with_hooks(hooks: HookCtx<'_>) -> Context<'_> {
        Context {
            hooks,
            ..ctx(&[], "draft")
        }
    }

    #[test]
    fn suggesting_waits_for_the_listing_and_needs_a_hook_that_applies() {
        let none = HookCtx::default();
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(none)),
            Err("Hooks are still being read.")
        );
        let nothing = HookCtx {
            applicable: Some(0),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(nothing)),
            Err("No prepare-commit hook applies to this repository.")
        );
        let one = HookCtx {
            applicable: Some(1),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.plan(&Args::None, &with_hooks(one)),
            Ok(Effect::Send(vec![CommandKind::RunPrepareCommit {
                message: "draft".into(),
            }]))
        );
    }

    #[test]
    fn a_second_run_is_refused_and_cancel_names_the_running_one() {
        let running = HookCtx {
            running: Some(OperationId(5)),
            applicable: Some(1),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_hooks(running)),
            Err("A hook is already running.")
        );
        assert_eq!(
            Action::CancelHook.plan(&Args::None, &with_hooks(running)),
            Ok(Effect::Send(vec![CommandKind::CancelHook {
                run: OperationId(5),
            }]))
        );
        assert_eq!(
            Action::CancelHook.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("No hook is running.")
        );
    }

    #[test]
    fn a_suggestion_is_used_or_dropped_and_needs_to_exist() {
        let offered = HookCtx {
            suggestion: Some("feat: x"),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::UseSuggestion.plan(&Args::None, &with_hooks(offered)),
            Ok(Effect::UseSuggestion("feat: x".into()))
        );
        assert_eq!(
            Action::DiscardSuggestion.plan(&Args::None, &with_hooks(offered)),
            Ok(Effect::DiscardSuggestion)
        );
        assert_eq!(
            Action::UseSuggestion.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("There is no suggestion.")
        );
    }

    #[test]
    fn approval_sends_the_listed_hash_and_deny_stays_local() {
        let waiting = HookCtx {
            pending_approval: Some(("team", "abc")),
            ..HookCtx::default()
        };
        assert_eq!(
            Action::ApproveHook.plan(&Args::None, &with_hooks(waiting)),
            Ok(Effect::Send(vec![CommandKind::ApproveHook {
                name: "team".into(),
                command_hash: "abc".into(),
            }]))
        );
        assert_eq!(
            Action::DenyHook.plan(&Args::None, &with_hooks(waiting)),
            Ok(Effect::DenyHook("abc".into()))
        );
        assert_eq!(
            Action::ApproveHook.enabled(&Args::None, &with_hooks(HookCtx::default())),
            Err("No repository hook waits for approval.")
        );
    }

    fn workflow(name: &str, trigger: &str, inputs: &[&str], error: Option<&str>) -> WorkflowInfo {
        WorkflowInfo {
            name: name.into(),
            path: format!("/w/{name}.yml"),
            trigger: trigger.into(),
            key: None,
            inputs: inputs
                .iter()
                .map(|input| WorkflowInput {
                    name: (*input).into(),
                    prompt: None,
                })
                .collect(),
            hash: "h".into(),
            error: error.map(str::to_owned),
            warnings: Vec::new(),
            source: String::new(),
        }
    }

    fn with_workflows(workflows: WorkflowCtx<'_>) -> Context<'_> {
        Context {
            workflows,
            ..ctx(&[], "")
        }
    }

    #[test]
    fn the_page_actions_need_the_page_with_an_open_file() {
        let page = |shown, name, dirty, saving| PageState {
            shown,
            name,
            dirty,
            saving,
            ..PageState::default()
        };
        let closed = with_workflows(WorkflowCtx::default());
        assert_eq!(
            Action::SaveWorkflow.enabled(&Args::None, &closed),
            Err("Open the Workflows page first.")
        );
        let clean = with_workflows(WorkflowCtx {
            page: page(true, Some("w"), false, false),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::SaveWorkflow.enabled(&Args::None, &clean),
            Err("Nothing to save: the file has no changes.")
        );
        assert_eq!(
            Action::RevertWorkflow.enabled(&Args::None, &clean),
            Err("Nothing to revert: the file has no changes.")
        );
        assert_eq!(
            Action::DeleteWorkflow.plan(&Args::None, &clean),
            Ok(Effect::OpenDeleteWorkflow)
        );
        let dirty = with_workflows(WorkflowCtx {
            page: page(true, Some("w"), true, false),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::SaveWorkflow.plan(&Args::None, &dirty),
            Ok(Effect::SaveWorkflow)
        );
        assert_eq!(
            Action::RevertWorkflow.plan(&Args::None, &dirty),
            Ok(Effect::RevertWorkflow)
        );
        let saving = with_workflows(WorkflowCtx {
            page: page(true, Some("w"), true, true),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::SaveWorkflow.enabled(&Args::None, &saving),
            Err("Wait for the save to finish.")
        );
    }

    #[test]
    fn run_waits_for_the_save_of_the_open_workflow_only() {
        let listed = [
            workflow("w", "manual", &[], None),
            workflow("other", "manual", &[], None),
        ];
        let dirty_w = with_workflows(WorkflowCtx {
            listed: Some(&listed),
            page: PageState {
                shown: true,
                name: Some("w"),
                dirty: true,
                saving: false,
                ..PageState::default()
            },
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("w".into()), &dirty_w),
            Err("Save first")
        );
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("other".into()), &dirty_w),
            Ok(())
        );
    }

    #[test]
    fn the_flow_edits_need_a_parsing_file_and_a_selected_state_for_the_moves() {
        let flow = |parses, states, selected| FlowPosition {
            parses,
            states,
            selected,
            shown: true,
        };
        let page = |flow| {
            with_workflows(WorkflowCtx {
                page: PageState {
                    shown: true,
                    name: Some("w"),
                    flow,
                    ..PageState::default()
                },
                ..WorkflowCtx::default()
            })
        };
        let broken = page(flow(false, 3, None));
        assert_eq!(
            Action::AddWorkflowState.enabled(&Args::None, &broken),
            Err("Fix the file in the TOML tab first.")
        );
        let unselected = page(flow(true, 3, None));
        assert_eq!(
            Action::AddWorkflowState.plan(&Args::None, &unselected),
            Ok(Effect::EditFlow(FlowEdit::Add))
        );
        assert_eq!(
            Action::RemoveWorkflowState.enabled(&Args::None, &unselected),
            Err("Select a state in the Flow first.")
        );
        let first = page(flow(true, 3, Some(0)));
        assert_eq!(
            Action::MoveWorkflowStateUp.enabled(&Args::None, &first),
            Err("This state is already the first.")
        );
        assert_eq!(
            Action::MoveWorkflowStateDown.plan(&Args::None, &first),
            Ok(Effect::EditFlow(FlowEdit::Move(Direction::Down)))
        );
        assert_eq!(
            Action::RemoveWorkflowState.plan(&Args::None, &first),
            Ok(Effect::EditFlow(FlowEdit::Remove))
        );
        let last = page(flow(true, 3, Some(2)));
        assert_eq!(
            Action::MoveWorkflowStateDown.enabled(&Args::None, &last),
            Err("This state is already the last.")
        );
        let only = page(flow(true, 1, Some(0)));
        assert_eq!(
            Action::RemoveWorkflowState.enabled(&Args::None, &only),
            Err("A workflow needs at least one state.")
        );
    }

    #[test]
    fn dry_run_needs_an_open_file_that_parses() {
        let open = |parses| {
            with_workflows(WorkflowCtx {
                page: PageState {
                    shown: true,
                    name: Some("w"),
                    flow: FlowPosition {
                        parses,
                        states: 1,
                        selected: None,
                        shown: true,
                    },
                    ..PageState::default()
                },
                ..WorkflowCtx::default()
            })
        };
        assert_eq!(
            Action::DryRunWorkflow.plan(&Args::None, &open(true)),
            Ok(Effect::DryRunWorkflow)
        );
        assert_eq!(
            Action::DryRunWorkflow.enabled(&Args::None, &open(false)),
            Err("Fix the file in the TOML tab first.")
        );
        assert_eq!(
            Action::DryRunWorkflow.enabled(&Args::None, &with_workflows(WorkflowCtx::default())),
            Err("Open the Workflows page first.")
        );
        let no_file = with_workflows(WorkflowCtx {
            page: PageState {
                shown: true,
                ..PageState::default()
            },
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::DryRunWorkflow.enabled(&Args::None, &no_file),
            Err("No workflow is open.")
        );
    }

    #[test]
    fn the_inspector_actions_need_the_flow_tab_a_parsing_file_and_a_state_for_the_action() {
        let page = |shown, parses, selected| {
            with_workflows(WorkflowCtx {
                page: PageState {
                    shown: true,
                    name: Some("w"),
                    flow: FlowPosition {
                        parses,
                        states: 2,
                        selected,
                        shown,
                    },
                    ..PageState::default()
                },
                ..WorkflowCtx::default()
            })
        };
        let on_toml = page(false, true, Some(0));
        assert_eq!(
            Action::SetWorkflowTrigger.enabled(&Args::None, &on_toml),
            Err("Open the Flow tab first.")
        );
        let broken = page(true, false, Some(0));
        assert_eq!(
            Action::AddWorkflowInput.enabled(&Args::None, &broken),
            Err("Fix the file in the TOML tab first.")
        );
        let unselected = page(true, true, None);
        assert_eq!(
            Action::SetWorkflowStateCall.enabled(&Args::None, &unselected),
            Err("Select a state in the Flow first.")
        );
        assert_eq!(
            Action::SetWorkflowStateCall.plan(&Args::None, &page(true, true, Some(0))),
            Ok(Effect::OpenInspectorPicker(PickerKind::Action))
        );
        assert_eq!(
            Action::SetWorkflowTrigger.plan(&Args::None, &page(true, true, None)),
            Ok(Effect::OpenInspectorPicker(PickerKind::Trigger))
        );
        assert_eq!(
            Action::AddWorkflowInput.plan(&Args::None, &page(true, true, None)),
            Ok(Effect::FocusWorkflowInput)
        );
    }

    #[test]
    fn the_inspector_actions_have_their_ids_titles_and_no_keys() {
        assert_eq!(Action::SetWorkflowStateCall.id(), "workflow.set_action");
        assert_eq!(Action::SetWorkflowTrigger.id(), "workflow.set_trigger");
        assert_eq!(Action::AddWorkflowInput.id(), "workflow.add_input");
        assert_eq!(Action::AddWorkflowInput.spec().title, "Add Workflow Input");
        for action in [
            Action::SetWorkflowStateCall,
            Action::SetWorkflowTrigger,
            Action::AddWorkflowInput,
        ] {
            assert_eq!(action.spec().shortcut, None, "{action:?}");
            assert_eq!(action.spec().category, Category::Workflows, "{action:?}");
            assert_eq!(action.spec().args, ArgKind::None, "{action:?}");
        }
    }

    #[test]
    fn the_flow_and_dry_run_actions_have_their_ids_titles_and_no_keys() {
        assert_eq!(Action::AddWorkflowState.id(), "workflow.add_state");
        assert_eq!(Action::AddWorkflowState.spec().title, "Add Workflow State");
        assert_eq!(Action::RemoveWorkflowState.id(), "workflow.remove_state");
        assert_eq!(Action::MoveWorkflowStateUp.id(), "workflow.move_state_up");
        assert_eq!(
            Action::MoveWorkflowStateDown.id(),
            "workflow.move_state_down"
        );
        assert_eq!(Action::DryRunWorkflow.id(), "workflow.dry_run");
        assert_eq!(Action::DryRunWorkflow.spec().title, "Dry Run Workflow");
        for action in [
            Action::AddWorkflowState,
            Action::RemoveWorkflowState,
            Action::MoveWorkflowStateUp,
            Action::MoveWorkflowStateDown,
            Action::DryRunWorkflow,
        ] {
            assert_eq!(action.spec().shortcut, None, "{action:?}");
            assert_eq!(action.spec().category, Category::Workflows, "{action:?}");
            assert_eq!(action.spec().args, ArgKind::None, "{action:?}");
        }
    }

    #[test]
    fn show_workflows_needs_a_repository_and_opens_the_page() {
        let open = ctx(&[], "");
        assert_eq!(
            Action::ShowWorkflows.plan(&Args::None, &open),
            Ok(Effect::ShowWorkflows)
        );
        assert_eq!(Action::ShowWorkflows.id(), "workflow.show");
        assert_eq!(Action::SaveWorkflow.spec().shortcut, Some("⌘S"));
    }

    #[test]
    fn the_workflow_actions_have_their_ids_and_category() {
        assert_eq!(Action::RunWorkflow.id(), "workflow.run");
        assert_eq!(Action::RunWorkflow.spec().title, "Run Workflow…");
        assert_eq!(Action::ApproveWorkflowStep.id(), "workflow.approve");
        assert_eq!(Action::RejectWorkflowStep.id(), "workflow.reject");
        assert_eq!(Action::CancelWorkflowRun.id(), "workflow.cancel");
        assert_eq!(Category::Workflows.label(), "Workflows");
        assert_eq!(Action::RunWorkflow.spec().args, ArgKind::Workflow);
    }

    #[test]
    fn run_needs_an_open_repository_and_a_listing() {
        let target = Args::Workflow("w".into());
        assert_eq!(
            Action::RunWorkflow.enabled(&target, &with_workflows(WorkflowCtx::default())),
            Err("Workflows are still being read.")
        );
        let closed = Context {
            repo_open: false,
            ..with_workflows(WorkflowCtx::default())
        };
        assert_eq!(
            Action::RunWorkflow.enabled(&target, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn run_says_why_a_listed_workflow_cannot_run() {
        let listed = [
            workflow("broken", "manual", &[], Some("line 3: bad key")),
            workflow("asks", "manual", &["ticket"], None),
            workflow("auto", "commit", &[], None),
        ];
        let ctx = with_workflows(WorkflowCtx {
            listed: Some(&listed),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("broken".into()), &ctx),
            Err("This workflow does not load: fix its file first.")
        );
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("asks".into()), &ctx),
            Ok(())
        );
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("auto".into()), &ctx),
            Err("Only manual workflows can be run from here.")
        );
        assert_eq!(
            Action::RunWorkflow.enabled(&Args::Workflow("missing".into()), &ctx),
            Err("No workflow has that name.")
        );
    }

    #[test]
    fn run_sends_the_manual_workflow_with_no_inputs() {
        let listed = [workflow("suggest-commit-message", "manual", &[], None)];
        let ctx = with_workflows(WorkflowCtx {
            listed: Some(&listed),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::RunWorkflow.plan(&Args::Workflow("suggest-commit-message".into()), &ctx),
            Ok(Effect::Send(vec![CommandKind::RunWorkflow {
                name: "suggest-commit-message".into(),
                inputs: BTreeMap::new(),
            }]))
        );
    }

    #[test]
    fn run_with_inputs_opens_the_inputs_dialog_instead_of_sending() {
        let listed = [workflow("asks", "manual", &["ticket", "title"], None)];
        let ctx = with_workflows(WorkflowCtx {
            listed: Some(&listed),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::RunWorkflow.plan(&Args::Workflow("asks".into()), &ctx),
            Ok(Effect::OpenWorkflowInputs {
                workflow: "asks".into(),
                inputs: vec![
                    WorkflowInput {
                        name: "ticket".into(),
                        prompt: None,
                    },
                    WorkflowInput {
                        name: "title".into(),
                        prompt: None,
                    },
                ],
            })
        );
    }

    #[test]
    fn the_new_workflow_actions_have_their_ids_titles_and_category() {
        assert_eq!(Action::NewWorkflowFromTemplate.id(), "workflow.new");
        assert_eq!(
            Action::NewWorkflowFromTemplate.spec().title,
            "New Workflow from Template…"
        );
        assert_eq!(
            Action::NewWorkflowFromTemplate.spec().args,
            ArgKind::Template
        );
        assert_eq!(Action::ReloadWorkflows.id(), "workflow.reload");
        assert_eq!(Action::ReloadWorkflows.spec().title, "Reload Workflows");
        assert_eq!(Action::OpenWorkflowsFolder.id(), "workflow.open_folder");
        assert_eq!(
            Action::OpenWorkflowsFolder.spec().title,
            "Open Workflows Folder"
        );
        for action in [
            Action::NewWorkflowFromTemplate,
            Action::ReloadWorkflows,
            Action::OpenWorkflowsFolder,
        ] {
            assert_eq!(action.spec().category, Category::Workflows, "{action:?}");
        }
    }

    #[test]
    fn a_template_is_picked_from_the_listed_ones() {
        let names = ["commit-message".to_owned()];
        let none: &[String] = &[];
        let unlisted = with_workflows(WorkflowCtx::default());
        assert_eq!(
            Action::NewWorkflowFromTemplate
                .enabled(&Args::Template("commit-message".into()), &unlisted),
            Err("No templates listed yet")
        );
        let listed = with_workflows(WorkflowCtx {
            templates: &names,
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::NewWorkflowFromTemplate.plan(&Args::Template("commit-message".into()), &listed),
            Ok(Effect::Send(vec![
                CommandKind::CreateWorkflowFromTemplate {
                    template: "commit-message".into(),
                }
            ]))
        );
        assert_eq!(
            Action::NewWorkflowFromTemplate.enabled(&Args::Template("nope".into()), &listed),
            Err("No template has that name.")
        );
        let empty = with_workflows(WorkflowCtx {
            templates: none,
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::NewWorkflowFromTemplate.enabled(&Args::Template("x".into()), &empty),
            Err("No templates listed yet")
        );
    }

    #[test]
    fn reload_needs_an_open_repository_and_asks_for_the_listing() {
        assert_eq!(
            Action::ReloadWorkflows.plan(&Args::None, &with_workflows(WorkflowCtx::default())),
            Ok(Effect::Send(vec![CommandKind::ListWorkflows]))
        );
        let closed = Context {
            repo_open: false,
            ..with_workflows(WorkflowCtx::default())
        };
        assert_eq!(
            Action::ReloadWorkflows.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn open_folder_reveals_the_listed_folder_or_says_why_there_is_none() {
        let known = with_workflows(WorkflowCtx {
            user_dir: Some("/home/u/.config/gitmaster/workflows"),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::OpenWorkflowsFolder.plan(&Args::None, &known),
            Ok(Effect::Reveal(PathBuf::from(
                "/home/u/.config/gitmaster/workflows"
            )))
        );
        assert_eq!(
            Action::OpenWorkflowsFolder
                .enabled(&Args::None, &with_workflows(WorkflowCtx::default())),
            Err("No workflows folder: HOME is not set")
        );
    }

    #[test]
    fn approve_and_reject_need_a_run_waiting_and_send_its_answer() {
        let idle = with_workflows(WorkflowCtx::default());
        assert_eq!(
            Action::ApproveWorkflowStep.enabled(&Args::None, &idle),
            Err("No workflow run waits for approval.")
        );
        let waiting = with_workflows(WorkflowCtx {
            awaiting: Some((4, "Create branch x?")),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::ApproveWorkflowStep.plan(&Args::None, &waiting),
            Ok(Effect::Send(vec![CommandKind::ApproveWorkflowStep {
                run: 4,
                approved: true,
            }]))
        );
        assert_eq!(
            Action::RejectWorkflowStep.plan(&Args::None, &waiting),
            Ok(Effect::Send(vec![CommandKind::ApproveWorkflowStep {
                run: 4,
                approved: false,
            }]))
        );
    }

    #[test]
    fn cancel_needs_an_active_run_and_names_it() {
        assert_eq!(
            Action::CancelWorkflowRun.enabled(&Args::None, &with_workflows(WorkflowCtx::default())),
            Err("No workflow run is active.")
        );
        let running = with_workflows(WorkflowCtx {
            active: Some(7),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::CancelWorkflowRun.plan(&Args::None, &running),
            Ok(Effect::Send(vec![CommandKind::CancelWorkflowRun {
                run: 7
            }]))
        );
    }
}
