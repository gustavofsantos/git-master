//! The action registry: every action the window can run, with its stable id,
//! title, default shortcut, typed target and its "enabled now?" rule. Keys,
//! buttons, menus and the palette read it, and `plan` is the one path from a
//! run to the engine commands it sends. Nothing here touches GPUI, so the rules
//! are unit-tested on their own.

// `from_id` has no caller outside the tests yet.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use gm_schema::threads::MemoryMark;
use gm_schema::{
    BranchEntry, CommandKind, ConflictSite, DiffSide, Feature, FeatureSet, HunkId, ReviewComment,
    ReviewStatus, StageTarget, StashEntry, StatusEntry, WorkflowInfo, WorkflowInput, WorkspaceRow,
    WorktreeEntry,
};

use crate::compare::ChangesTab;
use crate::memories::{self, Placement};
use crate::move_file::{DropTarget, FOLDER_REASON, MISSING_FOLDER, SAME_BRANCH, SAME_WORKTREE};
use crate::stacks::Stacks;
use crate::stashes;
use crate::submodule::Submodules;
use crate::thread_curate::{self, ThreadField};
use crate::transform_run;
use crate::workflows_page::PageState;

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
    Workspaces,
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
            Category::Workspaces => "Workspaces",
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
    /// A workspace of the sidebar's list, by its folder (D183).
    Workspace,
    Commit,
    /// A manual workflow, as the engine listed it.
    Workflow,
    /// A built-in workflow template, as the engine listed it.
    Template,
    /// A stash of the Stashes page's list.
    Stash,
    /// A review comment of the Working Copy diff, by its id (B4c).
    ReviewComment,
    /// The thread the Threads page has selected, or the thread a row names (T6).
    Thread,
    /// The label the Labels page has selected, or the label a row names (P4).
    Label,
    /// The memory the Memories tab has selected (D190).
    Memory,
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
    /// The workspace whose folder is `path`, as the list gives it (D183).
    Workspace(String),
    /// The text to copy from a commit of the history page: its id, or its message.
    Commit(String),
    /// The name of a listed manual workflow.
    Workflow(String),
    /// The name of a listed built-in template.
    Template(String),
    /// A stash of the list: its index and commit id, which the engine checks,
    /// and its name `stash@{n}` for the dialogs.
    Stash {
        index: u32,
        oid: String,
        label: String,
    },
    /// The review comment with this id, as the listing gave it (B4c).
    ReviewComment(i64),
    /// A thread, by its uid, with its star as the page shows it (T6).
    Thread {
        uid: String,
        starred: bool,
    },
    /// A label, by its id, with its name and attachment count as the list shows them (P4).
    /// The count is what the delete confirmation says.
    Label {
        id: i64,
        name: String,
        attachments: u32,
    },
    /// A memory, by its uid, and whether it is a proposal (D190).
    Memory {
        uid: String,
        proposed: bool,
    },
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
    /// A path on disk, for the system file manager.
    Reveal(PathBuf),
    FocusCommitMessage,
    /// Opens the create-worktree dialog, with `branch` chosen as the branch
    /// to check out when there is one. Without it the dialog starts empty and
    /// the branch is typed or picked there.
    OpenWorktreeDialog {
        branch: Option<String>,
    },
    /// Opens the worktree or workspace folder at this path in its own window.
    OpenWorktree(String),
    /// Opens the New Workspace dialog (D183 (3)).
    OpenWorkspaceDialog,
    /// Opens the Remove Workspace dialog for the workspace at this path; the dialog inspects it
    /// and the removal is sent from there (D183 (4)).
    OpenRemoveWorkspaceDialog(String),
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
    /// Opens the restack dialog for the stack `branch` is in: it plans the
    /// restack and asks before anything moves (D14, D142).
    OpenRestackDialog {
        branch: String,
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
    /// Shows the Changes page on `tab` (D231 (1)). Reads nothing: the tab's own read follows.
    ShowChangesTab(ChangesTab),
    /// Opens Quick Open, which finds a file by part of its path (N1).
    OpenQuickOpen,
    /// Opens the Code page (N2).
    ShowCode,
    /// Opens the Health tab of the Code page and asks for the code health.
    ShowCodeHealth,
    /// Opens the text search page and puts the cursor in its box (N3).
    ShowSearch,
    /// Opens the Code page on a file of the worktree, at a line if given.
    ViewFile {
        path: String,
        line: Option<u32>,
    },
    /// Opens the provenance panel of the Code page for the lines picked in the viewer (P4, D144).
    ShowProvenance(crate::provenance_view::Asked),
    /// Opens the Workflows page and asks the engine for the workflow files.
    ShowWorkflows,
    /// Opens the Stashes page and asks the engine for the stashes (B2).
    ShowStashes,
    /// Opens the Reflog page and asks the engine for the first page of HEAD's reflog (B3).
    ShowReflog,
    /// Opens the Threads page and asks the engine for the first page of threads (T5).
    ShowThreads,
    /// What the Memories tab's actions do (D190).
    Memory(MemoryEffect),
    /// Opens the Labels page and asks the engine for the labels of the repository (P4).
    ShowLabels,
    /// Opens the Local review page and asks the engine for the review comments (B4d).
    ShowLocalReview,
    /// Opens the Inbox page (UI redesign, slice 1).
    ShowInbox,
    /// Undoes the last operation that can be undone; today the last restack.
    UndoLastOperation,
    /// Opens the Working Copy on the unstaged side of `path`, with the cursor on
    /// working-tree line `line` (B4d).
    ShowReviewInDiff {
        path: String,
        line: u32,
    },
    /// Opens the dialog that stashes the working copy's changes (B2, D94).
    OpenNewStashDialog,
    /// Opens the composer that comments on the working-tree lines `start_line` to
    /// `end_line` of `path` (B4c).
    OpenReviewComposer {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// Opens the composer that comments on the text selected in `path`, a surface
    /// that is not the working tree (D141). The window reads the text from the pane.
    OpenReviewComposerByText {
        path: String,
    },
    /// Opens the composer that edits the body of review comment `id` (B4c).
    OpenReviewEdit {
        id: i64,
    },
    /// Opens the dialog that asks before the stash `oid` at `index` is dropped (B2).
    OpenDropStashDialog {
        index: u32,
        oid: String,
        label: String,
    },
    /// Opens the dialog that edits one field of thread `uid`, or exports it (T6).
    OpenLinkThread(String),
    OpenThreadField {
        field: ThreadField,
        uid: String,
    },
    /// Reads thread `uid` as Markdown and copies it to the clipboard once it is read (T6).
    CopyThreadMarkdown(String),
    /// Opens every tool-call group of the thread shown (`open`), or closes every one.
    ThreadFolds {
        open: bool,
    },
    /// Opens the confirmation that deletes thread `uid` (T6).
    OpenDeleteThreadDialog(String),
    /// Opens the confirmation of a scrub of every stored thread (T6).
    OpenScrubDialog,
    /// Opens the Threads page filtered to the threads that touched this path (T6).
    ShowThreadsFor(String),
    /// Opens the Threads settings dialog (T7).
    OpenThreadsSettings,
    /// Opens one of the Labels page's dialogs (P4).
    OpenLabelDialog(LabelDialog),
    /// Opens the confirmation that moves the open workflow's file to the trash.
    OpenDeleteWorkflow,
    /// Asks the engine for a dry run of the open workflow's file.
    DryRunWorkflow,
    /// Turns the Gravatar lookup of author pictures on or off (D110).
    SetAuthorPictures(bool),
    /// Opens the new-branch dialog for a child of local branch `parent` (D138).
    OpenNewChildDialog {
        parent: String,
    },
    /// Opens the palette on the parent step of Set Parent… for `branch`.
    PickStackParent {
        branch: String,
    },
    /// Opens the palette on the target step of a move action, with the file
    /// at `path` selected, so the branch or worktree is picked there.
    PickMoveTarget {
        action: Action,
        path: String,
    },
    /// Moves the change of `path` to `target`, as a drop of that file on that
    /// row does (D109).
    MoveFileTo {
        path: String,
        target: DropTarget,
    },
}

/// What the window knows of the commit message suggestions and their workflows, for the rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageCtx<'a> {
    /// Whether a `transform.commit_message` workflow is loaded (`WF3d`): Suggest Message runs it.
    pub transform: bool,
    /// The message a workflow offered and the person has not used or dropped.
    pub suggestion: Option<&'a str>,
    /// The `prepare-commit` sections the Git config holds, as the listing gave them (`WF3e`); empty
    /// when there are none or the listing has not come yet.
    pub sections: &'a [String],
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

/// Where the lines picked to comment on are, for a review comment (B4c, D141).
/// Built by the window from the diff pane or the Code viewer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommentSpot<'a> {
    /// Nothing is picked: no file is shown, or the cursor is on a hunk header.
    #[default]
    Nothing,
    /// Lines of `path` that the working tree has at these numbers: the unstaged
    /// diff's new side and the Code viewer. `working` is the lowest and highest
    /// line, `None` when all are removed lines.
    Lines {
        path: &'a str,
        working: Option<(u32, u32)>,
    },
    /// Lines of `path` on a surface that is not the working tree (the staged side,
    /// a commit, a stash): the engine finds the text in the file now.
    Text { path: &'a str },
    /// The pick runs over more than one hunk, so its text is not one run of lines.
    Split,
}

/// What the window knows of the review comments, for the rules (B4c).
#[derive(Clone, Copy, Debug, Default)]
pub struct ReviewCtx<'a> {
    /// The comments the engine listed; `None` until the first listing.
    pub comments: Option<&'a [ReviewComment]>,
    /// Where the lines picked to comment on are.
    pub spot: CommentSpot<'a>,
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
    pub messages: MessageCtx<'a>,
    pub workflows: WorkflowCtx<'a>,
    /// The worktrees listed for the open repository, main checkout first.
    pub worktrees: &'a [WorktreeEntry],
    /// The workspaces the sidebar lists under the root, by name (D183).
    pub workspaces: &'a [WorkspaceRow],
    /// The stashes the Stashes page listed, newest first; `None` until the first listing.
    pub stashes: Option<&'a [StashEntry]>,
    /// Whether author pictures are looked up on Gravatar (D110).
    pub author_pictures: AuthorPictures,
    /// A fetch, pull or push with lease is in flight; the three refuse another one meanwhile.
    pub remote_running: bool,
    /// The review comments and the Working Copy diff's selection (B4c).
    pub review: ReviewCtx<'a>,
    /// The path of the selected row of the Working Copy list: the file a move takes.
    pub selected_file: Option<&'a str>,
    /// The thread selected on the Threads page, which the thread actions take (T6).
    pub selected_thread: Option<ThreadRef<'a>>,
    /// The label selected on the Labels page, which the label actions take (P4).
    pub selected_label: Option<LabelRef<'a>>,
    /// The memory the Memories tab has selected, which the memory actions take (D190).
    pub selected_memory: Option<MemoryRef<'a>>,
    /// The branch stacks and the branch whose parent is being chosen (D138).
    pub stack: StackCtx<'a>,
    /// The text file open in the Code viewer, and the lines picked in it (P4).
    pub code: CodeCtx<'a>,
    /// The submodules of the status and the last read of each (D175); `None` when no status is loaded.
    pub submodules: Option<&'a Submodules>,
    /// The preview features that are on (D169): an action of a feature that is off is hidden.
    pub features: FeatureSet,
    /// The last operation of the operation log that can be undone, as the Undo button names it;
    /// `None` when there is none. Today only a restack records an undo.
    pub undoable: Option<&'a str>,
}

/// What the Code viewer shows, for the rules: the text file on screen and the lines picked in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodeCtx<'a> {
    /// The repository-relative path of the text file on screen; `None` for any other kind.
    pub file: Option<&'a str>,
    /// The first and last picked line, 1-based, both ends inclusive.
    pub lines: Option<(u32, u32)>,
}

/// What the stack actions read: the last stack snapshot, and the branch Set
/// Parent… was run on, while its parent is picked.
#[derive(Clone, Copy, Debug, Default)]
pub struct StackCtx<'a> {
    pub stacks: Option<&'a Stacks>,
    pub subject: Option<&'a str>,
    /// The last branch listing, to tell a local branch from a remote one.
    pub branches: &'a [BranchEntry],
}

/// The thread the Threads page has selected: its uid and its star.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadRef<'a> {
    pub uid: &'a str,
    pub starred: bool,
}

/// The dialog a label action opens, with what it names (P4). Nothing is sent before its button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelDialog {
    /// The New Label dialog: a name, a description and a colour.
    New,
    /// The Rename Label dialog of label `id`, prefilled with its name.
    Rename { id: i64, name: String },
    /// The confirmation that deletes label `id`, which has `attachments` rows.
    Delete {
        id: i64,
        name: String,
        attachments: u32,
    },
}

/// The label the Labels page has selected: its id, its name and its attachment count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelRef<'a> {
    pub id: i64,
    pub name: &'a str,
    pub attachments: u32,
}

/// What a memory action does (D190): shows the tab, opens New Memory, or accepts or rejects a
/// proposed memory by its uid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryEffect {
    Show,
    New,
    Accept(String),
    Reject(String),
    /// Opens the Manage Contexts dialog (D191).
    ManageContexts,
    /// Records the person's `mark` on the memory `uid`, with no reason (D202). The palette sends
    /// it; the detail pane sends its own reason.
    Mark {
        uid: String,
        mark: MemoryMark,
    },
    /// Opens the picker of the contexts the memory moves into (D193 (8)).
    MoveToContext(String),
    /// Opens the picker of the contexts the memory is attached to.
    AttachToContext(String),
    /// Opens the detach of the memory from one of its contexts: the confirmation when it is in one
    /// context, else the picker of its contexts.
    DetachFromContext(String),
}

/// The memory the Memories tab has selected: its uid, whether it is a proposal, and where it
/// sits, which the moves between the repository and its contexts take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRef<'a> {
    pub uid: &'a str,
    pub proposed: bool,
    pub placement: Placement,
}

/// The specs, in the order of `Action`'s variants (a test keeps them in step).
const SPECS: [Spec; 125] = [
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
        "stack.set_parent",
        "Set Parent…",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "stack.choose_parent",
        "Choose Parent",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "stack.remove",
        "Remove From Stack",
        None,
        ArgKind::Branch,
        Category::Branches,
    ),
    spec(
        "stack.reparent_grandparent",
        "Reparent to Grandparent",
        None,
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
        "hook.convert",
        "Convert Hooks to Workflows",
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
        "workflow.open_in_editor",
        "Open Workflow in Editor",
        Some("⌘E"),
        ArgKind::None,
        Category::Workflows,
    ),
    spec(
        "workflow.copy_path",
        "Copy Workflow Path",
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
        "workflow.dry_run",
        "Dry Run Workflow",
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
        "changes.uncommitted",
        "Show Uncommitted Changes",
        None,
        ArgKind::None,
        Category::WorkingCopy,
    ),
    spec(
        "changes.branch",
        "Show Branch Changes",
        None,
        ArgKind::None,
        Category::WorkingCopy,
    ),
    spec(
        "changes.compare",
        "Compare Revisions…",
        None,
        ArgKind::None,
        Category::WorkingCopy,
    ),
    spec(
        "commit.show_pictures",
        "Show Author Pictures from Gravatar",
        None,
        ArgKind::None,
        Category::Commit,
    ),
    spec(
        "commit.hide_pictures",
        "Hide Author Pictures",
        None,
        ArgKind::None,
        Category::Commit,
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
        "code.health",
        "Show Code Health",
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
        "file.move_to_branch",
        "Move File to Branch…",
        None,
        ArgKind::Branch,
        Category::WorkingCopy,
    ),
    spec(
        "file.move_to_worktree",
        "Move File to Worktree…",
        None,
        ArgKind::Worktree,
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
        "combination.record",
        "Record Combination",
        None,
        ArgKind::None,
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
    spec(
        "stash.apply",
        "Apply Stash",
        None,
        ArgKind::Stash,
        Category::WorkingCopy,
    ),
    spec(
        "stash.pop",
        "Pop Stash",
        None,
        ArgKind::Stash,
        Category::WorkingCopy,
    ),
    spec(
        "stash.drop",
        "Drop Stash",
        None,
        ArgKind::Stash,
        Category::WorkingCopy,
    ),
    spec(
        "reflog.show",
        "Show Reflog",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "threads.show",
        "Show Threads",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "labels.show",
        "Show Labels",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "commit.checkout",
        "Check Out Commit (Detached)",
        None,
        ArgKind::Commit,
        Category::Branches,
    ),
    spec(
        "branch.new_from_commit",
        "New Branch From Commit…",
        None,
        ArgKind::Commit,
        Category::Branches,
    ),
    spec(
        "review.comment",
        "Comment on Selection",
        Some("⇧⌘M"),
        ArgKind::None,
        Category::WorkingCopy,
    ),
    spec(
        "review.resolve",
        "Resolve Comment",
        None,
        ArgKind::ReviewComment,
        Category::WorkingCopy,
    ),
    spec(
        "review.reopen",
        "Reopen Comment",
        None,
        ArgKind::ReviewComment,
        Category::WorkingCopy,
    ),
    spec(
        "review.delete",
        "Delete Comment",
        None,
        ArgKind::ReviewComment,
        Category::WorkingCopy,
    ),
    spec(
        "review.edit",
        "Edit Comment",
        None,
        ArgKind::ReviewComment,
        Category::WorkingCopy,
    ),
    spec(
        "review.show",
        "Show Local Review",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "review.open",
        "Open Comment in Diff",
        None,
        ArgKind::ReviewComment,
        Category::WorkingCopy,
    ),
    spec(
        "file.resolve_conflict",
        "Resolve Conflict",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "threads.star",
        "Star Thread",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.rename",
        "Rename Thread…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.note",
        "Add Note…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.tags",
        "Tags…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.link_commit",
        "Link Thread to Commit…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.link_thread",
        "Link to Thread…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.export",
        "Export Thread…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.copy_markdown",
        "Copy as Markdown",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.expand_all",
        "Expand All Tool Calls",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.collapse_all",
        "Collapse All Tool Calls",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.delete",
        "Delete Thread…",
        None,
        ArgKind::Thread,
        Category::Repository,
    ),
    spec(
        "threads.scrub",
        "Scrub Threads…",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "file.show_threads",
        "Show Threads for File",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "threads.settings",
        "Threads Settings…",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "labels.new",
        "New Label…",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "labels.rename",
        "Rename Label…",
        None,
        ArgKind::Label,
        Category::Repository,
    ),
    spec(
        "labels.delete",
        "Delete Label…",
        None,
        ArgKind::Label,
        Category::Repository,
    ),
    spec(
        "code.provenance",
        "Show Why This Code Is Here…",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "submodule.open",
        "Open Submodule",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "workspace.new",
        "New Workspace…",
        None,
        ArgKind::None,
        Category::Workspaces,
    ),
    spec(
        "workspace.open",
        "Open Workspace",
        None,
        ArgKind::Workspace,
        Category::Workspaces,
    ),
    spec(
        "workspace.reveal",
        "Reveal in File Manager",
        None,
        ArgKind::Workspace,
        Category::Workspaces,
    ),
    spec(
        "workspace.remove",
        "Remove Workspace",
        None,
        ArgKind::Workspace,
        Category::Workspaces,
    ),
    spec(
        "submodule.check_out_branch",
        "Check Out Branch",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "memories.show",
        "Show Memories",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "memories.new",
        "New Memory",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "memories.accept",
        "Accept Memory",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "memories.reject",
        "Reject Memory",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "memories.mark_useful",
        "Mark Memory Useful",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "memories.mark_wrong",
        "Mark Memory Wrong",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "contexts.manage",
        "Manage Contexts…",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "memories.move_to_context",
        "Move to Context…",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "memories.attach_to_context",
        "Attach to Context…",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "memories.detach_from_context",
        "Detach from Context…",
        None,
        ArgKind::Memory,
        Category::Repository,
    ),
    spec(
        "inbox.show",
        "Show Inbox",
        None,
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "app.settings",
        "Open Settings",
        Some("⌘,"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "operation.undo",
        "Undo Last Operation",
        Some("⌘Z"),
        ArgKind::None,
        Category::Repository,
    ),
    spec(
        "folder.stage",
        "Stage Folder",
        None,
        ArgKind::File,
        Category::WorkingCopy,
    ),
    spec(
        "folder.unstage",
        "Unstage Folder",
        None,
        ArgKind::File,
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
    SetStackParent,
    ChooseStackParent,
    RemoveFromStack,
    ReparentToGrandparent,
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
    ConvertHooks,
    RunWorkflow,
    ApproveWorkflowStep,
    RejectWorkflowStep,
    CancelWorkflowRun,
    NewWorkflowFromTemplate,
    ReloadWorkflows,
    OpenWorkflowsFolder,
    ShowWorkflows,
    OpenWorkflowInEditor,
    CopyWorkflowPath,
    DeleteWorkflow,
    DryRunWorkflow,
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
    /// Shows the Changes page on its Uncommitted tab (D231 (1)).
    ShowUncommittedChanges,
    /// Shows the Changes page on its Branch tab (D231 (1)).
    ShowBranchChanges,
    /// Shows the Changes page on its Compare tab (D231 (1)).
    CompareRevisions,
    ShowAuthorPictures,
    HideAuthorPictures,
    CopyCommitHash,
    CopyCommitMessage,
    DeleteMergedBranches,
    QuickOpen,
    ShowCode,
    ShowCodeHealth,
    ViewFile,
    MoveFileToBranch,
    MoveFileToWorktree,
    SearchText,
    DiscardAllChanges,
    RecordCombination,
    ShowStashes,
    NewStash,
    StashApply,
    StashPop,
    StashDrop,
    ShowReflog,
    ShowThreads,
    ShowLabels,
    CheckoutCommit,
    NewBranchFromCommit,
    CommentOnSelection,
    ResolveReviewComment,
    ReopenReviewComment,
    DeleteReviewComment,
    EditReviewComment,
    ShowLocalReview,
    OpenReviewComment,
    ResolveConflict,
    ThreadStar,
    ThreadRename,
    ThreadNote,
    ThreadTags,
    ThreadLinkCommit,
    ThreadLinkThread,
    ThreadExport,
    ThreadCopyMarkdown,
    ThreadExpandAll,
    ThreadCollapseAll,
    ThreadDelete,
    ThreadScrub,
    ShowThreadsForFile,
    ThreadsSettings,
    LabelNew,
    LabelRename,
    LabelDelete,
    /// Opens the provenance panel for the lines picked in the Code viewer (P4, D144).
    CodeProvenance,
    /// Opens a checked-out submodule's folder in its own window (D175).
    OpenSubmodule,
    /// Opens the New Workspace dialog (D183).
    NewWorkspace,
    /// Opens a workspace's folder in its own window (D183).
    OpenWorkspace,
    /// Opens a workspace's folder in the file manager (D183).
    RevealWorkspace,
    /// Opens the dialog that removes a workspace (D183).
    RemoveWorkspace,
    /// Checks out the branch a detached member of a workspace is meant to be on (D183).
    CheckOutMemberBranch,
    /// Opens the Memories tab of the Threads page (D190).
    ShowMemories,
    /// Opens the New Memory dialog (D190).
    NewMemory,
    /// Accepts the selected proposed memory as it is (D190).
    AcceptMemory,
    /// Rejects the selected proposed memory (D190).
    RejectMemory,
    /// Records the person's useful mark on the selected accepted memory (D202).
    MarkMemoryUseful,
    /// Records the person's wrong mark on the selected accepted memory (D202).
    MarkMemoryWrong,
    /// Opens the Manage Contexts dialog: the contexts, their repositories and their memories (D191).
    ManageContexts,
    /// Moves the selected repository memory into a context (D193 (8)).
    MoveMemoryToContext,
    /// Attaches the selected accepted context memory to another context (D193 (8)).
    AttachMemoryToContext,
    /// Detaches the selected context memory from the context its scope names (D193 (8)).
    DetachMemoryFromContext,
    /// Opens the Inbox page (UI redesign, slice 1).
    ShowInbox,
    /// Opens the settings dialog (D38). Planned: listed disabled until its slice lands.
    OpenSettings,
    /// Undoes the last operation of the operation log that can be undone (UI redesign, slice 1).
    UndoLastOperation,
    /// Stages every file of a folder of the Changes list, from its tri-state box (slice 2).
    StageFolder,
    /// Unstages every file of a folder of the Changes list, from its tri-state box (slice 2).
    UnstageFolder,
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
        (Action::ShowCodeHealth, _) => Effect::ShowCodeHealth,
        (Action::SearchText, _) => Effect::ShowSearch,
        (Action::ViewFile, Args::File(path)) => Effect::ViewFile {
            path: path.clone(),
            line: None,
        },
        (Action::ShowWorkflows, _) => Effect::ShowWorkflows,
        (Action::ShowStashes, _) => Effect::ShowStashes,
        (Action::ShowReflog, _) => Effect::ShowReflog,
        (Action::ShowThreads, _) => Effect::ShowThreads,
        (Action::ShowMemories, _) => Effect::Memory(MemoryEffect::Show),
        (Action::ShowLabels, _) => Effect::ShowLabels,
        (Action::ShowLocalReview, _) => Effect::ShowLocalReview,
        (Action::ShowInbox, _) => Effect::ShowInbox,
        _ => Effect::ShowHistory,
    }
}

/// The rule of Undo Last Operation: an open, idle repository whose operation log has an
/// operation that can be undone.
fn undo_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)?;
    idle(ctx)?;
    if ctx.undoable.is_some() {
        Ok(())
    } else {
        Err("Nothing to undo.")
    }
}

/// The rule of Stage Folder and Unstage Folder: an idle repository and a folder of the list
/// (its path with a trailing `/`) with a file the action changes. Gives those files' paths.
/// Conflicted files and submodules are left out: they have no box in the list.
fn folder_rule<'a>(action: Action, args: &Args, ctx: &Context<'a>) -> Result<Vec<&'a str>, Reason> {
    idle(ctx)?;
    let Args::File(folder) = args else {
        return Err("No target for this action");
    };
    let stage = action == Action::StageFolder;
    let files: Vec<&'a str> = ctx
        .entries
        .iter()
        .filter(|entry| crate::change_tree::folder_of(&entry.path) == folder.as_str())
        .filter(|entry| !entry.conflicted && entry.submodule.is_none())
        .filter(|entry| {
            if stage {
                has_unstaged(entry)
            } else {
                entry.staged.is_some()
            }
        })
        .map(|entry| entry.path.as_str())
        .collect();
    if !files.is_empty() {
        Ok(files)
    } else if stage {
        Err("Nothing unstaged in this folder")
    } else {
        Err("Nothing staged in this folder")
    }
}

/// The commands of Stage Folder and Unstage Folder: one per file the rule gives, in order.
fn folder_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    let stage = action == Action::StageFolder;
    Ok(Effect::Send(
        folder_rule(action, args, ctx)?
            .into_iter()
            .map(|path| {
                let target = StageTarget::File {
                    path: path.to_owned(),
                };
                if stage {
                    CommandKind::Stage { target }
                } else {
                    CommandKind::Unstage { target }
                }
            })
            .collect(),
    ))
}

/// The rule of Resolve Conflict: an open repository and a listed file that is
/// conflicted. Reading the conflict changes nothing, so a change in flight does not refuse it.
fn resolve_conflict_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    if !ctx.repo_open {
        return Err("No repository is open.");
    }
    if file_entry(args, ctx)?.conflicted {
        Ok(())
    } else {
        Err("This file has no conflict.")
    }
}

/// The reason of a thread action when no thread is selected.
pub(crate) const SELECT_THREAD: Reason = "Select a thread first.";
/// The reason of a label action when no label is selected.
pub(crate) const SELECT_LABEL: Reason = "Select a label first.";
/// The reason of a memory action when no memory is selected (D190).
pub(crate) const SELECT_MEMORY: Reason = "Select a proposed memory first.";
/// The reason of Accept or Reject on a memory that is not a proposal (D190).
const NOT_PROPOSED_MEMORY: Reason = "Only a proposed memory can be accepted or rejected.";

/// The target of a label action: the label the Labels page has selected, or the one a row names.
#[must_use]
pub(crate) fn label_args(label: LabelRef<'_>) -> Args {
    Args::Label {
        id: label.id,
        name: label.name.to_owned(),
        attachments: label.attachments,
    }
}

/// The target of a memory action: the memory the Memories tab has selected.
#[must_use]
pub(crate) fn memory_args(memory: MemoryRef<'_>) -> Args {
    Args::Memory {
        uid: memory.uid.to_owned(),
        proposed: memory.proposed,
    }
}

/// The rules of the memory actions on the selected memory. Accept and Reject need a proposal (D190).
/// Move, Attach and Detach need the placement the engine takes (D193 (8)): the memory model checks
/// it as the engine does, so the menu and the engine give the same answer.
fn memory_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)?;
    let Args::Memory { uid, proposed } = args else {
        return Err(SELECT_MEMORY);
    };
    if matches!(action, Action::AcceptMemory | Action::RejectMemory) {
        return if *proposed {
            Ok(())
        } else {
            Err(NOT_PROPOSED_MEMORY)
        };
    }
    let Some(memory) = ctx
        .selected_memory
        .filter(|memory| memory.uid == uid.as_str())
    else {
        return Err(SELECT_MEMORY);
    };
    match action {
        Action::MarkMemoryUseful | Action::MarkMemoryWrong => {
            memories::check_mark(memory.placement)
        }
        Action::MoveMemoryToContext => memories::check_move(memory.placement),
        Action::AttachMemoryToContext => memories::check_attach(memory.placement),
        _ => memories::check_detach(memory.placement),
    }
}

/// The rule of New Label: a repository is open. The dialog takes no target.
fn label_new_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)
}

/// The rule of Rename and Delete Label: a repository is open and a label is named.
fn label_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)?;
    match args {
        Args::Label { .. } => Ok(()),
        _ => Err(SELECT_LABEL),
    }
}

/// What the label actions do with their target: each opens its dialog, which asks before
/// anything is sent. Nothing is sent from the registry.
fn label_effect(action: Action, args: &Args) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (Action::LabelNew, _) => Effect::OpenLabelDialog(LabelDialog::New),
        (Action::LabelRename, Args::Label { id, name, .. }) => {
            Effect::OpenLabelDialog(LabelDialog::Rename {
                id: *id,
                name: name.clone(),
            })
        }
        (
            Action::LabelDelete,
            Args::Label {
                id,
                name,
                attachments,
            },
        ) => Effect::OpenLabelDialog(LabelDialog::Delete {
            id: *id,
            name: name.clone(),
            attachments: *attachments,
        }),
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The target of a thread action: the thread the page has selected, or the one a row names.
#[must_use]
pub(crate) fn thread_args(thread: ThreadRef<'_>) -> Args {
    Args::Thread {
        uid: thread.uid.to_owned(),
        starred: thread.starred,
    }
}

/// What the thread actions do with their target: a star is sent, the field
/// dialogs and the delete open their dialog, and the scrub and the file filter
/// open the page or its confirmation.
fn thread_effect(action: Action, args: &Args) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (Action::ThreadStar, Args::Thread { uid, starred }) => {
            Effect::Send(vec![thread_curate::star_command(uid, *starred)])
        }
        (
            Action::ThreadRename
            | Action::ThreadNote
            | Action::ThreadTags
            | Action::ThreadLinkCommit
            | Action::ThreadExport,
            Args::Thread { uid, .. },
        ) => Effect::OpenThreadField {
            field: thread_field(action),
            uid: uid.clone(),
        },
        (Action::ThreadLinkThread, Args::Thread { uid, .. }) => Effect::OpenLinkThread(uid.clone()),
        (Action::ThreadCopyMarkdown, Args::Thread { uid, .. }) => {
            Effect::CopyThreadMarkdown(uid.clone())
        }
        (Action::ThreadExpandAll, Args::Thread { .. }) => Effect::ThreadFolds { open: true },
        (Action::ThreadCollapseAll, Args::Thread { .. }) => Effect::ThreadFolds { open: false },
        (Action::ThreadDelete, Args::Thread { uid, .. }) => {
            Effect::OpenDeleteThreadDialog(uid.clone())
        }
        (Action::ThreadScrub, _) => Effect::OpenScrubDialog,
        (Action::ThreadsSettings, _) => Effect::OpenThreadsSettings,
        (Action::ShowThreadsForFile, Args::File(path)) => Effect::ShowThreadsFor(path.clone()),
        _ => return Err(NOT_AVAILABLE),
    })
}

/// What the memory actions do (D190): they show the tab, open New Memory, or accept or reject the
/// proposal they name.
fn memory_effect(action: Action, args: &Args) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (Action::NewMemory, _) => Effect::Memory(MemoryEffect::New),
        (Action::ManageContexts, _) => Effect::Memory(MemoryEffect::ManageContexts),
        (Action::AcceptMemory, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::Accept(uid.clone()))
        }
        (Action::RejectMemory, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::Reject(uid.clone()))
        }
        (Action::MarkMemoryUseful, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::Mark {
                uid: uid.clone(),
                mark: MemoryMark::Useful,
            })
        }
        (Action::MarkMemoryWrong, Args::Memory { uid, .. }) => Effect::Memory(MemoryEffect::Mark {
            uid: uid.clone(),
            mark: MemoryMark::Wrong,
        }),
        (Action::MoveMemoryToContext, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::MoveToContext(uid.clone()))
        }
        (Action::AttachMemoryToContext, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::AttachToContext(uid.clone()))
        }
        (Action::DetachMemoryFromContext, Args::Memory { uid, .. }) => {
            Effect::Memory(MemoryEffect::DetachFromContext(uid.clone()))
        }
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The field a thread action's dialog edits. Only the field actions reach it.
fn thread_field(action: Action) -> ThreadField {
    match action {
        Action::ThreadNote => ThreadField::Note,
        Action::ThreadTags => ThreadField::Tags,
        Action::ThreadLinkCommit => ThreadField::LinkCommit,
        Action::ThreadExport => ThreadField::Export,
        _ => ThreadField::Rename,
    }
}

/// The rule of the thread actions: a repository is open and a thread is named.
/// Changing a thread's text does not touch the worktree, so a change in flight does not refuse it.
fn thread_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)?;
    match args {
        Args::Thread { .. } => Ok(()),
        _ => Err(SELECT_THREAD),
    }
}

/// The rule of Reveal in File Manager: a repository is open and the file is in it.
fn reveal_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    if ctx.root.is_none() {
        return Err("No repository is open.");
    }
    file_entry(args, ctx).map(|_| ())
}

/// Resolve Conflict reads the file; the resolver's page opens when the answer comes.
fn resolve_conflict_effect(args: &Args) -> Result<Effect, Reason> {
    let Args::File(path) = args else {
        return Err("No target for this action");
    };
    Ok(Effect::Send(vec![CommandKind::ReadConflict {
        path: path.clone(),
        site: ConflictSite::WorkingCopy,
    }]))
}

/// Whether the action sets a preference of the window (the pictures).
fn is_choice(action: Action) -> bool {
    matches!(
        action,
        Action::ShowAuthorPictures | Action::HideAuthorPictures
    )
}

/// The effect of an action that sets a preference of the window.
fn choice_effect(action: Action) -> Effect {
    match action {
        Action::ShowAuthorPictures => Effect::SetAuthorPictures(true),
        _ => Effect::SetAuthorPictures(false),
    }
}

/// Whether author pictures are looked up on Gravatar now (D110).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorPictures {
    Shown,
    Hidden,
}

impl AuthorPictures {
    #[must_use]
    pub fn from_on(on: bool) -> Self {
        if on { Self::Shown } else { Self::Hidden }
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

/// The reason an action gives on the workspace this window shows (D183).
const IN_THIS_WINDOW: Reason = "Open in this window.";

/// The reason a file is not a workspace member, for Check Out Branch.
const NOT_A_MEMBER: Reason = "Not a workspace member.";

/// The reason a member is already on its branch, for Check Out Branch.
const ON_ITS_BRANCH: Reason = "It is on its branch.";

/// The workspace the run names, as the sidebar lists it (D183).
fn workspace_entry<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a WorkspaceRow, Reason> {
    match args {
        Args::Workspace(path) => ctx
            .workspaces
            .iter()
            .find(|row| &row.path == path)
            .ok_or("This workspace is not listed."),
        _ => Err("No target for this action"),
    }
}

/// Open refuses the workspace this window shows: it is already open (D183). Remove runs on it too
/// and closes the window once it is removed (D186 (3)). Reveal has no limit. Check Out Branch takes
/// a member, so it has its own rule.
fn workspace_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    if action == Action::CheckOutMemberBranch {
        return check_out_member_rule(args, ctx);
    }
    let row = workspace_entry(args, ctx)?;
    let here = ctx
        .root
        .is_some_and(|root| root.trim_end_matches('/') == row.path.trim_end_matches('/'));
    match action {
        Action::OpenWorkspace if here => Err(IN_THIS_WINDOW),
        _ => Ok(()),
    }
}

/// Check Out Branch: a submodule row of a workspace, which is on a detached HEAD (D183). Only the
/// submodule's read says where its HEAD is, so the rule waits for that read.
fn check_out_member_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    let Args::File(path) = args else {
        return Err("No target for this action");
    };
    let entry = submodule_entry(path, ctx).ok_or(NOT_A_MEMBER)?;
    let detail = ctx
        .submodules
        .and_then(|submodules| submodules.detail(&entry.path))
        .ok_or(SUBMODULE_READING)?;
    if !detail.checked_out {
        Err(NOT_CHECKED_OUT)
    } else if detail.detached {
        Ok(())
    } else {
        Err(ON_ITS_BRANCH)
    }
}

/// What the workspace actions and Check Out Branch do (D183).
fn workspace_effect(action: Action, args: &Args) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (Action::NewWorkspace, _) => Effect::OpenWorkspaceDialog,
        (Action::OpenWorkspace, Args::Workspace(path)) => Effect::OpenWorktree(path.clone()),
        (Action::RevealWorkspace, Args::Workspace(path)) => Effect::Reveal(PathBuf::from(path)),
        (Action::RemoveWorkspace, Args::Workspace(path)) => {
            Effect::OpenRemoveWorkspaceDialog(path.clone())
        }
        (Action::CheckOutMemberBranch, Args::File(path)) => {
            Effect::Send(vec![CommandKind::CheckOutMemberBranch {
                path: path.clone(),
            }])
        }
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The reason of an action the engine cannot run yet, whatever its target.
pub(crate) const NOT_AVAILABLE: Reason = "Not available yet";

/// Why a preview action is not enabled while its feature is off (D169).
pub(crate) const PREVIEW_OFF: Reason = "This is a preview feature.";

impl Action {
    pub const ALL: [Action; 125] = [
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
        Action::SetStackParent,
        Action::ChooseStackParent,
        Action::RemoveFromStack,
        Action::ReparentToGrandparent,
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
        Action::ConvertHooks,
        Action::RunWorkflow,
        Action::ApproveWorkflowStep,
        Action::RejectWorkflowStep,
        Action::CancelWorkflowRun,
        Action::NewWorkflowFromTemplate,
        Action::ReloadWorkflows,
        Action::OpenWorkflowsFolder,
        Action::ShowWorkflows,
        Action::OpenWorkflowInEditor,
        Action::CopyWorkflowPath,
        Action::DeleteWorkflow,
        Action::DryRunWorkflow,
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
        Action::ShowUncommittedChanges,
        Action::ShowBranchChanges,
        Action::CompareRevisions,
        Action::ShowAuthorPictures,
        Action::HideAuthorPictures,
        Action::CopyCommitHash,
        Action::CopyCommitMessage,
        Action::DeleteMergedBranches,
        Action::QuickOpen,
        Action::ShowCode,
        Action::ShowCodeHealth,
        Action::ViewFile,
        Action::MoveFileToBranch,
        Action::MoveFileToWorktree,
        Action::SearchText,
        Action::DiscardAllChanges,
        Action::RecordCombination,
        Action::ShowStashes,
        Action::NewStash,
        Action::StashApply,
        Action::StashPop,
        Action::StashDrop,
        Action::ShowReflog,
        Action::ShowThreads,
        Action::ShowLabels,
        Action::CheckoutCommit,
        Action::NewBranchFromCommit,
        Action::CommentOnSelection,
        Action::ResolveReviewComment,
        Action::ReopenReviewComment,
        Action::DeleteReviewComment,
        Action::EditReviewComment,
        Action::ShowLocalReview,
        Action::OpenReviewComment,
        Action::ResolveConflict,
        Action::ThreadStar,
        Action::ThreadRename,
        Action::ThreadNote,
        Action::ThreadTags,
        Action::ThreadLinkCommit,
        Action::ThreadLinkThread,
        Action::ThreadExport,
        Action::ThreadCopyMarkdown,
        Action::ThreadExpandAll,
        Action::ThreadCollapseAll,
        Action::ThreadDelete,
        Action::ThreadScrub,
        Action::ShowThreadsForFile,
        Action::ThreadsSettings,
        Action::LabelNew,
        Action::LabelRename,
        Action::LabelDelete,
        Action::CodeProvenance,
        Action::OpenSubmodule,
        Action::NewWorkspace,
        Action::OpenWorkspace,
        Action::RevealWorkspace,
        Action::RemoveWorkspace,
        Action::CheckOutMemberBranch,
        Action::ShowMemories,
        Action::NewMemory,
        Action::AcceptMemory,
        Action::RejectMemory,
        Action::MarkMemoryUseful,
        Action::MarkMemoryWrong,
        Action::ManageContexts,
        Action::MoveMemoryToContext,
        Action::AttachMemoryToContext,
        Action::DetachMemoryFromContext,
        Action::ShowInbox,
        Action::OpenSettings,
        Action::UndoLastOperation,
        Action::StageFolder,
        Action::UnstageFolder,
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
        !matches!(self, Action::RenameBranch | Action::OpenSettings)
    }

    /// The actions that make or change a stack's metadata (D138).
    fn is_stack_action(self) -> bool {
        matches!(
            self,
            Action::NewChildBranch
                | Action::SetStackParent
                | Action::ChooseStackParent
                | Action::RemoveFromStack
                | Action::ReparentToGrandparent
                | Action::Restack
        )
    }

    /// The preview feature the action belongs to, if any (D169). Its menus, its palette
    /// entry and its toolbar button are hidden while the feature is off.
    #[must_use]
    pub fn feature(self) -> Option<Feature> {
        if self.is_stack_action() {
            Some(Feature::Stacks)
        } else if self == Action::ResolveConflict {
            Some(Feature::ConflictResolver)
        } else {
            None
        }
    }

    /// Whether a window with `features` offers the action at all: false only for a preview
    /// action whose feature is off.
    #[must_use]
    pub fn offered(self, features: FeatureSet) -> bool {
        match self.feature() {
            Some(feature) => features.is_enabled(feature),
            None => true,
        }
    }

    /// Whether the palette lists the action at its top level. Choose Parent is
    /// only the second step of Set Parent…, so it is reached from there.
    #[must_use]
    pub fn listed_in_palette(self) -> bool {
        !matches!(
            self,
            Action::ChooseStackParent | Action::StageFolder | Action::UnstageFolder
        )
    }

    /// Whether a menu asks before it runs the action, though it destroys
    /// nothing: Reparent to Grandparent changes the stack (docs/guide/stacks.md).
    #[must_use]
    pub fn asks_first(self) -> bool {
        self == Action::ReparentToGrandparent
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
                | Action::RemoveWorkspace
                | Action::DiscardAllChanges
                | Action::StashDrop
                | Action::DeleteReviewComment
                | Action::ThreadDelete
                | Action::LabelDelete
        )
    }

    /// Whether the action opens a page or a viewer, which `navigation_effect` plans.
    fn opens_a_page(self) -> bool {
        matches!(
            self,
            Action::ShowHistory
                | Action::QuickOpen
                | Action::ShowCode
                | Action::ShowCodeHealth
                | Action::SearchText
                | Action::ViewFile
                | Action::ShowWorkflows
                | Action::ShowStashes
                | Action::ShowReflog
                | Action::ShowThreads
                | Action::ShowMemories
                | Action::ShowLabels
                | Action::ShowLocalReview
                | Action::ShowInbox
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

    /// Whether `workspace_effect` plans the action (D183).
    fn is_workspace_effect(self) -> bool {
        matches!(
            self,
            Action::NewWorkspace
                | Action::OpenWorkspace
                | Action::RevealWorkspace
                | Action::RemoveWorkspace
                | Action::CheckOutMemberBranch
        )
    }

    /// Whether the rule of `workspace_rule` decides the action.
    fn is_workspace_rule(self) -> bool {
        matches!(
            self,
            Action::OpenWorkspace
                | Action::RevealWorkspace
                | Action::RemoveWorkspace
                | Action::CheckOutMemberBranch
        )
    }

    /// Whether the rule of `stash_rule` decides the action.
    fn is_stash_rule(self) -> bool {
        matches!(
            self,
            Action::ShowStashes
                | Action::NewStash
                | Action::StashApply
                | Action::StashPop
                | Action::StashDrop
        )
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
                | Action::OpenWorkflowInEditor
                | Action::CopyWorkflowPath
                | Action::DeleteWorkflow
                | Action::DryRunWorkflow
        )
    }

    /// Whether the action is one of the Labels page's write actions (P4), which `label_effect` plans.
    /// Whether the action is one of the memory actions that `memory_effect` plans.
    fn is_memory_effect(self) -> bool {
        matches!(
            self,
            Action::NewMemory
                | Action::AcceptMemory
                | Action::RejectMemory
                | Action::MarkMemoryUseful
                | Action::MarkMemoryWrong
                | Action::ManageContexts
                | Action::MoveMemoryToContext
                | Action::AttachMemoryToContext
                | Action::DetachMemoryFromContext
        )
    }

    /// Whether the action's rule is `memory_rule`: the ones that act on the selected memory.
    fn is_memory_rule(self) -> bool {
        matches!(
            self,
            Action::AcceptMemory
                | Action::RejectMemory
                | Action::MarkMemoryUseful
                | Action::MarkMemoryWrong
                | Action::MoveMemoryToContext
                | Action::AttachMemoryToContext
                | Action::DetachMemoryFromContext
        )
    }

    fn is_label_action(self) -> bool {
        matches!(
            self,
            Action::LabelNew | Action::LabelRename | Action::LabelDelete
        )
    }

    /// Whether the action is one of the Threads page's curation actions (T6), which `thread_effect` plans.
    fn is_thread_action(self) -> bool {
        matches!(
            self,
            Action::ThreadStar
                | Action::ThreadRename
                | Action::ThreadNote
                | Action::ThreadTags
                | Action::ThreadLinkCommit
                | Action::ThreadLinkThread
                | Action::ThreadExport
                | Action::ThreadCopyMarkdown
                | Action::ThreadExpandAll
                | Action::ThreadCollapseAll
                | Action::ThreadDelete
                | Action::ThreadScrub
                | Action::ShowThreadsForFile
                | Action::ThreadsSettings
        )
    }

    /// Whether `thread_rule` decides the action: the actions on one selected thread.
    fn is_thread_rule(self) -> bool {
        self.is_thread_action()
            && !matches!(
                self,
                Action::ThreadScrub | Action::ShowThreadsForFile | Action::ThreadsSettings
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
                | Action::NewWorkspace
                | Action::RemoveWorkspace
                | Action::NewBranch
                | Action::Restack
                | Action::NewChildBranch
                | Action::DeleteBranch
                | Action::DeleteMergedBranches
                | Action::DiscardFile
                | Action::DiscardAllChanges
                | Action::NewStash
                | Action::StashDrop
                | Action::NewBranchFromCommit
                | Action::CommentOnSelection
                | Action::EditReviewComment
                | Action::ThreadRename
                | Action::ThreadNote
                | Action::ThreadTags
                | Action::ThreadLinkCommit
                | Action::ThreadLinkThread
                | Action::ThreadExport
                | Action::ThreadDelete
                | Action::ThreadScrub
                | Action::ThreadsSettings
                | Action::LabelNew
                | Action::LabelRename
                | Action::LabelDelete
                | Action::NewMemory
                | Action::ManageContexts
                | Action::MoveMemoryToContext
                | Action::AttachMemoryToContext
                | Action::DetachMemoryFromContext
        )
    }

    /// The action with this id, if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether the action can run now with `args`; the reason when it cannot.
    pub fn enabled(self, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
        // A preview action whose feature is off never runs (D169); the menus do not list it.
        if !self.offered(ctx.features) {
            return Err(PREVIEW_OFF);
        }
        // A thread, label or memory action run with no target (the palette, with none) says so.
        self.missing_target(args).map_or(Ok(()), Err)?;
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
            | Action::ShowUncommittedChanges
            | Action::ShowBranchChanges
            | Action::CompareRevisions
            | Action::QuickOpen
            | Action::ShowCode
            | Action::ShowCodeHealth
            | Action::SearchText
            | Action::ShowWorkflows
            | Action::ShowReflog
            | Action::ShowThreads
            | Action::ShowLabels
            | Action::ShowLocalReview
            | Action::ShowInbox => repository_open(ctx),
            Action::UndoLastOperation => undo_rule(ctx),
            Action::StageFolder | Action::UnstageFolder => folder_rule(self, args, ctx).map(|_| ()),
            Action::LabelNew => label_new_rule(ctx),
            Action::LabelRename | Action::LabelDelete => label_rule(args, ctx),
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
            _ if self.is_stack_action() => stack_rule(self, args, ctx),
            // A checkout of a commit moves HEAD, so it waits for an idle repository, like a branch.
            Action::DeleteMergedBranches | Action::CheckoutCommit | Action::NewBranchFromCommit => {
                idle(ctx)
            }
            Action::RevealFile => reveal_rule(args, ctx),
            Action::ViewFile => view_file_rule(args, ctx),
            Action::OpenSubmodule => open_submodule_rule(args, ctx),
            _ if self.is_workspace_rule() => workspace_rule(self, args, ctx),
            Action::MoveFileToBranch | Action::MoveFileToWorktree => move_rule(self, args, ctx),
            Action::DiscardFile | Action::DiscardAllChanges => discard_rule(self, args, ctx),
            Action::RecordCombination => combination_rule(ctx),
            Action::ShowAuthorPictures if ctx.author_pictures == AuthorPictures::Shown => {
                Err("Author pictures are already shown.")
            }
            Action::HideAuthorPictures if ctx.author_pictures == AuthorPictures::Hidden => {
                Err("Author pictures are already hidden.")
            }
            Action::ShowAuthorPictures | Action::HideAuthorPictures => Ok(()),
            _ if self.copies_its_target() => Ok(()),
            Action::Fetch | Action::Pull | Action::PushWithLease => remote_rule(self, args, ctx),
            Action::SuggestMessage | Action::UseSuggestion | Action::DiscardSuggestion => {
                message_rule(self, ctx)
            }
            Action::ConvertHooks => convert_rule(ctx),
            _ if self.is_workflow_rule() => workflow_rule(self, args, ctx),
            Action::StashApply | Action::StashPop | Action::StashDrop => {
                stash_entry_rule(args, ctx)
            }
            _ if self.is_stash_rule() => stash_rule(self, ctx),
            Action::CodeProvenance => provenance_rule(ctx).map(|_| ()),
            Action::CommentOnSelection => comment_rule(ctx),
            Action::ResolveReviewComment
            | Action::ReopenReviewComment
            | Action::DeleteReviewComment
            | Action::EditReviewComment => review_entry_rule(self, args, ctx),
            Action::OpenReviewComment => open_target(args, ctx).map(|_| ()),
            Action::ResolveConflict => resolve_conflict_rule(args, ctx),
            _ if self.is_thread_rule() => thread_rule(args, ctx),
            Action::ThreadScrub
            | Action::ShowThreadsForFile
            | Action::ShowMemories
            | Action::ManageContexts
            | Action::NewMemory => repository_open(ctx),
            action if action.is_memory_rule() => memory_rule(action, args, ctx),
            // The settings and New Workspace dialogs need no repository.
            Action::ThreadsSettings | Action::NewWorkspace => Ok(()),
            // The placeholders, refused above by `available`.
            _ => Err(NOT_AVAILABLE),
        }
    }

    /// The reason a thread, label or memory action has no target, when it is run with none (the
    /// palette, with nothing selected).
    fn missing_target(self, args: &Args) -> Option<Reason> {
        if !matches!(args, Args::None) {
            return None;
        }
        match self.spec().args {
            ArgKind::Thread => Some(SELECT_THREAD),
            ArgKind::Label => Some(SELECT_LABEL),
            ArgKind::Memory => Some(SELECT_MEMORY),
            _ => None,
        }
    }

    /// Runs the rule, then says what the run does. A refused run gives the reason.
    pub fn plan(self, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
        self.enabled(args, ctx)?;
        Ok(match (self, args) {
            (
                Action::StageFile
                | Action::UnstageFile
                | Action::StageChunk
                | Action::UnstageChunk
                | Action::DiscardChunk
                | Action::Commit,
                _,
            ) => change_effect(self, args, ctx)?,
            (Action::Refresh, _) => Effect::Send(vec![CommandKind::RefreshStatus]),
            (Action::RecordCombination, _) => Effect::Send(vec![CommandKind::RecordCombination]),
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
            (Action::LockWorktree | Action::UnlockWorktree, Args::Worktree(path)) => {
                worktree_lock_effect(self, path)
            }
            (Action::PruneWorktrees, _) => Effect::OpenPruneDialog,
            (Action::NewStash, _) => Effect::OpenNewStashDialog,
            (Action::StashApply | Action::StashPop | Action::StashDrop, _) => {
                stash_effect(self, args)?
            }
            (Action::CodeProvenance, _) => provenance_effect(ctx)?,
            (Action::OpenSubmodule, _) => open_submodule_effect(args, ctx)?,
            (action, _) if action.is_workspace_effect() => workspace_effect(action, args)?,
            (Action::CommentOnSelection, _) => comment_effect(ctx)?,
            (
                Action::EditReviewComment
                | Action::ResolveReviewComment
                | Action::ReopenReviewComment
                | Action::DeleteReviewComment
                | Action::OpenReviewComment,
                _,
            ) => review_entry_effect(self, args, ctx)?,
            (Action::MoveFileToBranch | Action::MoveFileToWorktree, _) => {
                move_effect(self, args, ctx)?
            }
            (Action::ResolveConflict, _) => resolve_conflict_effect(args)?,
            (Action::ShowUncommittedChanges, _) => Effect::ShowChangesTab(ChangesTab::Uncommitted),
            (Action::ShowBranchChanges, _) => Effect::ShowChangesTab(ChangesTab::Branch),
            (Action::CompareRevisions, _) => Effect::ShowChangesTab(ChangesTab::Compare),
            (action, _) if action.opens_a_page() => navigation_effect(action, args),
            (action, _) if is_choice(action) => choice_effect(action),
            (Action::CopyCommitMessage, Args::Commit(text)) => Effect::CopyMessage(text.clone()),
            (action, _) if action == Action::NewBranch || action.is_stack_action() => {
                branch_effect(action, args, ctx)?
            }
            (Action::CheckoutCommit, Args::Commit(oid)) => {
                Effect::Send(vec![CommandKind::CheckoutCommit { oid: oid.clone() }])
            }
            (Action::NewBranchFromCommit, Args::Commit(oid)) => Effect::OpenNewBranchDialog {
                start: Some(oid.clone()),
            },
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
            (Action::SuggestMessage | Action::UseSuggestion | Action::DiscardSuggestion, _) => {
                message_effect(self, ctx)?
            }
            (Action::ConvertHooks, _) => Effect::Send(vec![CommandKind::ConvertHooksToWorkflows]),
            (Action::UndoLastOperation, _) => Effect::UndoLastOperation,
            (Action::StageFolder | Action::UnstageFolder, _) => folder_effect(self, args, ctx)?,
            (action, _) if action.is_workflow_rule() => workflow_effect(action, args, ctx)?,
            (action, _) if action.is_thread_action() => thread_effect(action, args)?,
            (action, _) if action.is_label_action() => label_effect(action, args)?,
            (action, _) if action.is_memory_effect() => memory_effect(action, args)?,
            // Only reached when `enabled` said Ok, which no placeholder does.
            _ => return Err(NOT_AVAILABLE),
        })
    }

    /// Whether the action opens the palette's target step when the file menu runs it.
    fn opens_target_pick(self) -> bool {
        matches!(self, Action::MoveFileToBranch | Action::MoveFileToWorktree)
    }

    /// Whether `args` is the kind of target this action takes. The move actions
    /// also take the file, from the file menu, which then picks the target.
    fn takes(self, args: &Args) -> bool {
        if self.opens_target_pick() && matches!(args, Args::File(_)) {
            return true;
        }
        matches!(
            (self.spec().args, args),
            (ArgKind::None, Args::None)
                | (ArgKind::OptionalBranch, Args::None | Args::Branch(_))
                | (ArgKind::Branch, Args::Branch(_))
                | (ArgKind::File, Args::File(_))
                | (ArgKind::Chunk, Args::Chunk { .. })
                | (ArgKind::Worktree, Args::Worktree(_))
                | (ArgKind::Workspace, Args::Workspace(_))
                | (ArgKind::Commit, Args::Commit(_))
                | (ArgKind::Workflow, Args::Workflow(_))
                | (ArgKind::Template, Args::Template(_))
                | (ArgKind::Stash, Args::Stash { .. })
                | (ArgKind::ReviewComment, Args::ReviewComment(_))
                | (ArgKind::Thread, Args::Thread { .. })
                | (ArgKind::Label, Args::Label { .. })
                | (ArgKind::Memory, Args::Memory { .. })
        )
    }
}

/// The rules of the stack actions (D138). Each takes a local branch and waits
/// for an idle repository. Choose Parent takes the parent of the branch Set
/// Parent… was run on; the engine checks loops and shared history.
fn stack_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let Args::Branch(name) = args else {
        return Err("No target for this action");
    };
    let stacks = ctx.stack.stacks;
    let local = !ctx
        .stack
        .branches
        .iter()
        .any(|branch| branch.is_remote && branch.name == *name);
    match action {
        Action::NewChildBranch | Action::SetStackParent if !local => {
            Err("Stacks are made of local branches.")
        }
        Action::ChooseStackParent => match ctx.stack.subject {
            None => Err("Run Set Parent… on a branch first."),
            Some(subject) if subject == name => Err("A branch cannot be its own parent."),
            Some(_) if !local => Err("The parent must be a local branch."),
            Some(subject)
                if stacks
                    .and_then(|stacks| stacks.branch(subject))
                    .and_then(|entry| entry.parent.as_deref())
                    == Some(name.as_str()) =>
            {
                Err("This is already its parent.")
            }
            Some(_) => Ok(()),
        },
        Action::Restack => match stacks {
            None => Err("The stacks are still being read."),
            Some(stacks) if stacks.branch(name).is_none() => Err("This branch is in no stack."),
            Some(_) => Ok(()),
        },
        Action::RemoveFromStack if !stacks.is_some_and(|stacks| stacks.has_parent(name)) => {
            Err("This branch has no parent.")
        }
        Action::ReparentToGrandparent => match stacks.and_then(|stacks| stacks.branch(name)) {
            Some(entry) if entry.state.parent_left() => Ok(()),
            Some(entry) if entry.parent.is_some() => Err("The parent is neither gone nor merged."),
            _ => Err("This branch has no parent."),
        },
        _ => Ok(()),
    }
}

/// What New Branch… and the stack actions open or send.
fn branch_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    let Args::Branch(name) = args else {
        return match action {
            Action::NewBranch => Ok(Effect::OpenNewBranchDialog { start: None }),
            _ => Err("No target for this action"),
        };
    };
    Ok(match action {
        Action::NewBranch => Effect::OpenNewBranchDialog {
            start: Some(name.clone()),
        },
        Action::NewChildBranch => Effect::OpenNewChildDialog {
            parent: name.to_owned(),
        },
        Action::SetStackParent => Effect::PickStackParent {
            branch: name.to_owned(),
        },
        Action::ChooseStackParent => Effect::Send(vec![CommandKind::SetStackParent {
            branch: ctx.stack.subject.ok_or(NOT_AVAILABLE)?.to_owned(),
            parent: name.to_owned(),
        }]),
        Action::RemoveFromStack => Effect::Send(vec![CommandKind::ClearStackParent {
            branch: name.to_owned(),
        }]),
        Action::Restack => Effect::OpenRestackDialog {
            branch: name.to_owned(),
        },
        Action::ReparentToGrandparent => Effect::Send(vec![CommandKind::ReparentToGrandparent {
            branch: name.to_owned(),
        }]),
        _ => return Err(NOT_AVAILABLE),
    })
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
    // A submodule has no hunks; its row stages the commit it has checked out (D175).
    if let Args::Chunk { path, .. } = args
        && submodule_entry(path, ctx).is_some()
    {
        return Err(SUBMODULE_HUNKS);
    }
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

/// The reasons a submodule row gives for the actions that act on its files (D175).
const SUBMODULE_DISCARD: Reason = "Open the submodule to discard.";
const SUBMODULE_VIEW: Reason = "Open the submodule instead.";
const SUBMODULE_MOVE: Reason = "A submodule cannot be moved.";
const SUBMODULE_HUNKS: Reason = "A submodule has no diff hunks.";
const SUBMODULE_READING: Reason = "The submodule is still being read.";
const NOT_CHECKED_OUT: Reason = "This submodule is not checked out.";
const NOT_A_SUBMODULE: Reason = "Only a submodule opens in its own window.";

/// The listed submodule row the path names, if it is one (D175).
fn submodule_entry<'a>(path: &str, ctx: &Context<'a>) -> Option<&'a StatusEntry> {
    ctx.entries
        .iter()
        .find(|entry| entry.path == path && entry.submodule.is_some())
}

/// Open Submodule: a submodule row whose folder is checked out (D175). Only the
/// submodule's read says whether it is, so the rule waits for that read.
fn open_submodule_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    if !ctx.repo_open {
        return Err("No repository is open.");
    }
    let entry = file_entry(args, ctx)?;
    if entry.submodule.is_none() {
        return Err(NOT_A_SUBMODULE);
    }
    match ctx
        .submodules
        .and_then(|submodules| submodules.detail(&entry.path))
    {
        Some(detail) if detail.checked_out => Ok(()),
        Some(_) => Err(NOT_CHECKED_OUT),
        None => Err(SUBMODULE_READING),
    }
}

/// Opens the submodule's folder, `<repository root>/<path>`, in its own window (D175).
fn open_submodule_effect(args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    let Args::File(path) = args else {
        return Err("No target for this action");
    };
    let root = ctx.root.ok_or("No repository is open.")?;
    Ok(Effect::OpenWorktree(
        PathBuf::from(root)
            .join(path)
            .to_string_lossy()
            .into_owned(),
    ))
}

/// View File: the reveal rule, and a submodule has no file to view (D175).
fn view_file_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    reveal_rule(args, ctx)?;
    match args {
        Args::File(path) if submodule_entry(path, ctx).is_some() => Err(SUBMODULE_VIEW),
        _ => Ok(()),
    }
}

/// The rule of the two discard actions: an idle repository and a listed file
/// that is not conflicted, with something to throw away. Discard Changes needs
/// an unstaged change or an untracked file; Discard All Changes needs any change.
/// A submodule row has nothing to discard here (D175).
fn discard_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let entry = file_entry(args, ctx)?;
    if entry.conflicted {
        return Err("Resolve the conflict first.");
    }
    if entry.submodule.is_some() {
        return Err(SUBMODULE_DISCARD);
    }
    match action {
        Action::DiscardAllChanges if entry.staged.is_some() || has_unstaged(entry) => Ok(()),
        Action::DiscardAllChanges => Err("Nothing to discard"),
        _ if has_unstaged(entry) => Ok(()),
        _ => Err("Nothing unstaged to discard"),
    }
}

pub(crate) const NO_NEW_COMMIT: Reason = "No member has a new commit.";

/// The rule of Record Combination (D180(3)): an idle repository, and a member whose
/// commit is new. A detached member is refused by the engine, which names it.
fn combination_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let changed = ctx.entries.iter().any(|entry| {
        entry
            .submodule
            .as_ref()
            .is_some_and(|change| change.commit_changed)
    });
    if changed { Ok(()) } else { Err(NO_NEW_COMMIT) }
}

/// The rule of the two move actions (D109): an idle repository, a changed file
/// that can move, and a target that can take it. The file is the one the run
/// names, or else the selected row of the Working Copy.
fn move_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let path = match args {
        Args::File(path) => path.as_str(),
        _ => selected_file(ctx)?,
    };
    move_source(path, ctx)?;
    match (action, args) {
        (Action::MoveFileToBranch, Args::Branch(name)) => move_to_branch(name, ctx),
        (Action::MoveFileToWorktree, Args::Worktree(path)) => move_to_worktree(path, ctx),
        // The file menu names the file only; the target is picked next.
        (_, Args::File(_)) => Ok(()),
        _ => Err("No target for this action"),
    }
}

/// What a move action does once its rule passes: a file named alone opens the
/// target step, and a target names the move, which the window sends as a drop (D109).
fn move_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    Ok(match (action, args) {
        (_, Args::File(path)) => Effect::PickMoveTarget {
            action,
            path: path.clone(),
        },
        (Action::MoveFileToBranch, Args::Branch(name)) => Effect::MoveFileTo {
            path: selected_file(ctx)?.to_owned(),
            target: DropTarget::Branch { name: name.clone() },
        },
        (Action::MoveFileToWorktree, Args::Worktree(path)) => Effect::MoveFileTo {
            path: selected_file(ctx)?.to_owned(),
            target: DropTarget::Worktree { path: path.clone() },
        },
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The file a move takes: a listed, unconflicted file, and not a folder.
fn move_source<'a>(path: &str, ctx: &Context<'a>) -> Result<&'a StatusEntry, Reason> {
    if path.ends_with('/') {
        return Err(FOLDER_REASON);
    }
    let entry = ctx
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .ok_or("That file is not in the working copy")?;
    if entry.conflicted {
        return Err("Resolve the conflict first.");
    }
    if entry.submodule.is_some() {
        return Err(SUBMODULE_MOVE);
    }
    Ok(entry)
}

/// The file a palette run takes when it names only the target: the selected row.
fn selected_file<'a>(ctx: &Context<'a>) -> Result<&'a str, Reason> {
    ctx.selected_file
        .ok_or("Select a changed file to move first.")
}

/// A branch takes a move unless it is the branch of the window or is checked out elsewhere.
fn move_to_branch(name: &str, ctx: &Context<'_>) -> Result<(), Reason> {
    if ctx.current_branch == Some(name) {
        return Err(SAME_BRANCH);
    }
    if ctx
        .worktrees
        .iter()
        .any(|entry| entry.branch.as_deref() == Some(name))
    {
        return Err("Checked out in another worktree");
    }
    Ok(())
}

/// A worktree takes a move unless it is the window's own, or its folder is gone.
fn move_to_worktree(path: &str, ctx: &Context<'_>) -> Result<(), Reason> {
    let entry = ctx
        .worktrees
        .iter()
        .find(|entry| entry.path == path)
        .ok_or("This worktree is not listed.")?;
    if ctx.root == Some(entry.path.as_str()) {
        return Err(SAME_WORKTREE);
    }
    if entry.prunable.is_some() {
        return Err(MISSING_FOLDER);
    }
    Ok(())
}

/// Convert Hooks to Workflows is offered while a `prepare-commit` section is in the Git config
/// (`WF3e`, D160). The sections are the listing's, `<scope> gitmaster.prepare-commit.<name>`.
#[must_use]
pub fn convert_offered(sections: &[String]) -> bool {
    !sections.is_empty()
}

/// The rule of Convert Hooks to Workflows: offered while a section is listed (`WF3e`).
fn convert_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    if convert_offered(ctx.messages.sections) {
        Ok(())
    } else {
        Err("No prepare-commit section is in your Git config.")
    }
}

/// The rules of the commit message actions: Suggest Message needs a commit transform (`WF3d`, `WF3e`),
/// and a suggestion can be used or dropped while it waits.
fn message_rule(action: Action, ctx: &Context<'_>) -> Result<(), Reason> {
    match action {
        Action::SuggestMessage => {
            idle(ctx)?;
            if ctx.messages.transform {
                Ok(())
            } else {
                Err(NO_SUGGESTER)
            }
        }
        Action::UseSuggestion | Action::DiscardSuggestion => ctx
            .messages
            .suggestion
            .map(|_| ())
            .ok_or("There is no suggestion."),
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
        Action::OpenWorkflowInEditor | Action::CopyWorkflowPath => page_file(ctx).map(|_| ()),
        Action::DeleteWorkflow => page_open(ctx).map(|_| ()),
        Action::DryRunWorkflow => dry_run_rule(ctx),
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
        (Action::OpenWorkflowInEditor, _) => {
            Effect::Send(vec![CommandKind::OpenWorkflowInEditor {
                path: page_file(ctx)?.to_owned(),
            }])
        }
        (Action::CopyWorkflowPath, _) => Effect::Copy(page_file(ctx)?.to_owned()),
        (Action::DeleteWorkflow, _) => Effect::OpenDeleteWorkflow,
        (Action::DryRunWorkflow, _) => Effect::DryRunWorkflow,
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

/// The rule of Dry Run: the page has an open file. A save in flight does not stop it; the dry
/// run walks the buffer, not the file.
fn dry_run_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    let page = ctx.workflows.page;
    if !page.shown {
        return Err("Open the Workflows page first.");
    }
    if page.name.is_none() {
        return Err("No workflow is open.");
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
    Ok(page)
}

/// The path of the open workflow's file, for the actions that hand it to another program. A
/// built-in workflow has no file.
fn page_file<'a>(ctx: &Context<'a>) -> Result<&'a str, Reason> {
    page_open(ctx)?
        .file
        .ok_or("A built-in workflow has no file: use Copy to Edit first.")
}

/// The reason of the workflows folder when the engine has no folder to give.
const NO_WORKFLOWS_FOLDER: Reason = "No workflows folder: HOME is not set";
/// Suggest Message needs a workflow on the commit message phase; there is no other suggester (`WF3e`).
const NO_SUGGESTER: Reason =
    "No workflow suggests commit messages. Add one on the transform.commit_message phase.";

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

/// What the working copy's change actions send. Each is refused by `change_rule`
/// before it gets here, so a target of the wrong kind is never sent.
fn change_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    Ok(match (action, args) {
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
        _ => return Err(NOT_AVAILABLE),
    })
}

/// What apply, pop and drop do with the listed stash they name. The drop opens
/// its dialog, which is the confirmation, so it sends nothing itself.
fn stash_effect(action: Action, args: &Args) -> Result<Effect, Reason> {
    let Args::Stash { index, oid, label } = args else {
        return Err("No target for this action");
    };
    Ok(match action {
        Action::StashApply => Effect::Send(vec![CommandKind::ApplyStash {
            index: *index,
            oid: oid.clone(),
        }]),
        Action::StashPop => Effect::Send(vec![CommandKind::PopStash {
            index: *index,
            oid: oid.clone(),
        }]),
        Action::StashDrop => Effect::OpenDropStashDialog {
            index: *index,
            oid: oid.clone(),
            label: label.clone(),
        },
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The reason of apply, pop and drop when the Stashes page has not listed the
/// stashes yet, so there is no stash to name.
pub(crate) const OPEN_STASHES: Reason = "Open the Stashes page first.";

/// The rule of apply, pop and drop: an idle repository, a listing, and the
/// stash the run names in it. The engine checks the index and the commit id
/// again, so a list that has moved on is refused there too.
fn stash_entry_rule(args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    let Args::Stash { index, oid, .. } = args else {
        return Err("No target for this action");
    };
    let listed = ctx.stashes.ok_or(OPEN_STASHES)?;
    if listed
        .iter()
        .any(|entry| entry.index == *index && entry.oid == *oid)
    {
        Ok(())
    } else {
        Err("This stash is no longer listed. Open the Stashes page again.")
    }
}

/// The target of apply, pop or drop: the listed stash, by its index and commit id.
#[must_use]
pub(crate) fn stash_args(entry: &StashEntry) -> Args {
    Args::Stash {
        index: entry.index,
        oid: entry.oid.clone(),
        label: stashes::label(entry.index),
    }
}

/// The rule of Show Why This Code Is Here…: a text file open in the Code viewer, with lines
/// picked in it. Returns the file and the lines.
fn provenance_rule<'a>(ctx: &Context<'a>) -> Result<(&'a str, u32, u32), Reason> {
    repository_open(ctx)?;
    let Some(path) = ctx.code.file else {
        return Err("Open a text file in the Code viewer first.");
    };
    let Some((from, to)) = ctx.code.lines else {
        return Err("Select lines of the file first.");
    };
    Ok((path, from, to))
}

/// What Show Why This Code Is Here… opens: the provenance panel for the picked lines.
fn provenance_effect(ctx: &Context<'_>) -> Result<Effect, Reason> {
    let (path, from, to) = provenance_rule(ctx)?;
    Ok(Effect::ShowProvenance(crate::provenance_view::Asked {
        path: path.to_owned(),
        from,
        to,
    }))
}

/// What locking or unlocking the worktree at `path` sends. The menu does not ask for a lock
/// reason yet, so Git gets none.
fn worktree_lock_effect(action: Action, path: &str) -> Effect {
    if action == Action::LockWorktree {
        Effect::Send(vec![CommandKind::LockWorktree {
            path: path.to_owned(),
            reason: String::new(),
        }])
    } else {
        Effect::Send(vec![CommandKind::UnlockWorktree {
            path: path.to_owned(),
        }])
    }
}

/// The rule of Comment on Selection: an idle repository and picked lines the
/// working tree has, or whose text the engine can look for in the file.
fn comment_rule(ctx: &Context<'_>) -> Result<(), Reason> {
    idle(ctx)?;
    match ctx.review.spot {
        CommentSpot::Nothing => Err("Select lines to comment on first."),
        CommentSpot::Lines { working: None, .. } => Err("Those lines are not in the working tree."),
        CommentSpot::Split => Err("Select lines of one hunk to comment on."),
        CommentSpot::Lines { .. } | CommentSpot::Text { .. } => Ok(()),
    }
}

/// What Comment on Selection opens: the composer for the working-tree lines, or
/// for the picked text.
fn comment_effect(ctx: &Context<'_>) -> Result<Effect, Reason> {
    comment_rule(ctx)?;
    match ctx.review.spot {
        CommentSpot::Lines {
            path,
            working: Some((start_line, end_line)),
        } => Ok(Effect::OpenReviewComposer {
            path: path.to_owned(),
            start_line,
            end_line,
        }),
        CommentSpot::Text { path } => Ok(Effect::OpenReviewComposerByText {
            path: path.to_owned(),
        }),
        _ => Err(NOT_AVAILABLE),
    }
}

/// The rule of resolve, reopen, edit and delete: a repository, a listing that
/// has the comment, and the status the action needs.
fn review_entry_rule(action: Action, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
    repository_open(ctx)?;
    let comment = listed_comment(args, ctx)?;
    match action {
        Action::ResolveReviewComment if comment.status == ReviewStatus::Resolved => {
            Err("This comment is already resolved.")
        }
        Action::ReopenReviewComment if comment.status == ReviewStatus::Open => {
            Err("This comment is already open.")
        }
        _ => Ok(()),
    }
}

/// The comment the run names, as the listing on screen has it.
fn listed_comment<'a>(args: &Args, ctx: &Context<'a>) -> Result<&'a ReviewComment, Reason> {
    let Args::ReviewComment(id) = args else {
        return Err("No target for this action");
    };
    let listed: &'a [ReviewComment] = ctx
        .review
        .comments
        .ok_or("The comments are still being read.")?;
    listed
        .iter()
        .find(|comment| comment.id == *id)
        .ok_or("This comment is no longer listed.")
}

/// What the comment actions do once their rule has passed: open the editor, send
/// the write, or open the diff. The card asks once before a delete gets here, so
/// the command is sent at once.
fn review_entry_effect(action: Action, args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    if action == Action::OpenReviewComment {
        return open_target(args, ctx);
    }
    let Args::ReviewComment(id) = args else {
        return Err("No target for this action");
    };
    Ok(match action {
        Action::EditReviewComment => Effect::OpenReviewEdit { id: *id },
        Action::ResolveReviewComment => {
            Effect::Send(vec![CommandKind::ResolveReviewComment { id: *id }])
        }
        Action::ReopenReviewComment => {
            Effect::Send(vec![CommandKind::ReopenReviewComment { id: *id }])
        }
        Action::DeleteReviewComment => {
            Effect::Send(vec![CommandKind::DeleteReviewComment { id: *id }])
        }
        _ => return Err(NOT_AVAILABLE),
    })
}

/// The target of Open Comment in Diff: the comment's file and its first line now.
/// Refused for an outdated comment, and for a file the Working Copy does not list.
fn open_target(args: &Args, ctx: &Context<'_>) -> Result<Effect, Reason> {
    repository_open(ctx)?;
    let comment = listed_comment(args, ctx)?;
    let location = comment.location.ok_or("This comment is outdated.")?;
    if !ctx.entries.iter().any(|entry| entry.path == comment.path) {
        return Err("This file has no changes to show in the Working Copy.");
    }
    Ok(Effect::ShowReviewInDiff {
        path: comment.path.clone(),
        line: location.start_line,
    })
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

/// What a commit message action does, given the suggestion in `ctx`.
fn message_effect(action: Action, ctx: &Context<'_>) -> Result<Effect, Reason> {
    Ok(match action {
        Action::SuggestMessage => Effect::Send(vec![transform_run::suggest_message(ctx.message)]),
        Action::UseSuggestion => match ctx.messages.suggestion {
            Some(text) => Effect::UseSuggestion(text.to_owned()),
            None => return Err("There is no suggestion."),
        },
        Action::DiscardSuggestion => Effect::DiscardSuggestion,
        _ => return Err(NOT_AVAILABLE),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gm_schema::{Change, OperationId, WorkflowInfo, WorkflowInput};

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            messages: crate::action_registry::MessageCtx::default(),
            workflows: WorkflowCtx::default(),
            worktrees: &[],
            workspaces: &[],
            author_pictures: AuthorPictures::Hidden,
            remote_running: false,
            stashes: None,
            review: ReviewCtx::default(),
            code: CodeCtx::default(),
            submodules: None,
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
    fn a_folder_box_stages_or_unstages_each_file_directly_in_the_folder() {
        let entries = [
            entry("src/a.rs", None, Some(Change::Modified)),
            entry("src/b.rs", Some(Change::Modified), None),
            entry("src/ui/c.rs", None, Some(Change::Modified)),
            entry("README.md", None, Some(Change::Modified)),
        ];
        let context = ctx(&entries, "");
        let folder = Args::File("src/".into());
        assert_eq!(
            Action::StageFolder.plan(&folder, &context),
            Ok(Effect::Send(vec![CommandKind::Stage {
                target: StageTarget::File {
                    path: "src/a.rs".into()
                }
            }]))
        );
        assert_eq!(
            Action::UnstageFolder.plan(&folder, &context),
            Ok(Effect::Send(vec![CommandKind::Unstage {
                target: StageTarget::File {
                    path: "src/b.rs".into()
                }
            }]))
        );
        let staged = [entry("lib/x.rs", Some(Change::Added), None)];
        assert_eq!(
            Action::StageFolder.enabled(&Args::File("lib/".into()), &ctx(&staged, "")),
            Err("Nothing unstaged in this folder")
        );
        assert!(!Action::StageFolder.listed_in_palette());
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
        let placeholders = [Action::RenameBranch];
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            root: None,
            ..ctx(&[], "")
        };
        for action in [Action::QuickOpen, Action::ShowCode, Action::ShowCodeHealth] {
            assert_eq!(
                action.enabled(&Args::None, &closed),
                Err("No repository is open.")
            );
        }
        assert_eq!(Action::QuickOpen.id(), "files.quick_open");
        assert_eq!(Action::QuickOpen.spec().shortcut, Some("⌘P"));
        assert_eq!(Action::ShowCode.id(), "code.show");
        assert_eq!(
            Action::ShowCodeHealth.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowCodeHealth)
        );
        assert_eq!(Action::ShowCodeHealth.id(), "code.health");
        assert_eq!(Action::ShowCodeHealth.spec().title, "Show Code Health");
        assert_eq!(Action::ShowCodeHealth.spec().shortcut, None);
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
                Action::StashDrop,
                Action::DeleteReviewComment,
                Action::ThreadDelete,
                Action::LabelDelete,
                Action::RemoveWorkspace,
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            workspaces: &[],
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
            workspaces: &[],
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
            workspaces: &[],
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
            workspaces: &[],
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
            workspaces: &[],
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            workspaces: &[],
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowHistory.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ShowHistory.id(), "history.show");
    }

    #[test]
    fn the_changes_tab_actions_need_a_repository_and_plan_their_tab() {
        let cases = [
            (Action::ShowUncommittedChanges, ChangesTab::Uncommitted),
            (Action::ShowBranchChanges, ChangesTab::Branch),
            (Action::CompareRevisions, ChangesTab::Compare),
        ];
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        for (action, tab) in cases {
            assert_eq!(
                action.plan(&Args::None, &ctx(&[], "")),
                Ok(Effect::ShowChangesTab(tab)),
                "{action:?}"
            );
            assert_eq!(
                action.enabled(&Args::None, &closed),
                Err("No repository is open."),
                "{action:?}"
            );
            assert_eq!(action.shortcut_label(), None, "{action:?}");
            assert!(action.listed_in_palette(), "{action:?}");
        }
        assert_eq!(Action::CompareRevisions.spec().title, "Compare Revisions…");
        assert_eq!(Action::ShowBranchChanges.id(), "changes.branch");
    }

    #[test]
    fn show_stashes_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowStashes.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowStashes)
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
    fn show_reflog_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowReflog.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowReflog)
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowReflog.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ShowReflog.id(), "reflog.show");
        assert_eq!(Action::ShowReflog.spec().title, "Show Reflog");
        assert_eq!(Action::ShowReflog.spec().category, Category::Repository);
        assert_eq!(Action::ShowReflog.spec().shortcut, None);
        assert!(!Action::ShowReflog.opens_dialog());
    }

    #[test]
    fn checkout_commit_needs_an_idle_repository_and_sends_the_detached_checkout() {
        let oid = "e".repeat(40);
        let target = Args::Commit(oid.clone());
        assert_eq!(
            Action::CheckoutCommit.plan(&target, &ctx(&[], "")),
            Ok(Effect::Send(vec![CommandKind::CheckoutCommit {
                oid: oid.clone()
            }]))
        );
        let busy = Context {
            busy: Some("Wait for the repository to open."),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::CheckoutCommit.enabled(&target, &busy),
            Err("Wait for the repository to open.")
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::CheckoutCommit.enabled(&target, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::CheckoutCommit.enabled(&Args::None, &ctx(&[], "")),
            Err("No target for this action")
        );
        // Git refuses a checkout that would lose changes, so nothing asks first.
        assert!(!Action::CheckoutCommit.destructive());
        assert!(!Action::CheckoutCommit.opens_dialog());
    }

    #[test]
    fn new_branch_from_commit_opens_the_dialog_starting_at_the_commit() {
        let oid = "f".repeat(40);
        let target = Args::Commit(oid.clone());
        assert_eq!(
            Action::NewBranchFromCommit.plan(&target, &ctx(&[], "")),
            Ok(Effect::OpenNewBranchDialog { start: Some(oid) })
        );
        assert!(Action::NewBranchFromCommit.opens_dialog());
        assert_eq!(Action::NewBranchFromCommit.spec().args, ArgKind::Commit);
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::NewBranchFromCommit.enabled(&target, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn show_threads_needs_an_open_repository_and_has_no_key() {
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowThreads.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::ShowThreads.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowThreads)
        );
        assert_eq!(Action::ShowThreads.id(), "threads.show");
        assert_eq!(Action::ShowThreads.spec().title, "Show Threads");
        assert_eq!(Action::ShowThreads.spec().shortcut, None);
        assert!(!Action::ShowThreads.opens_dialog());
    }

    #[test]
    fn show_labels_needs_an_open_repository_and_has_no_key() {
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowLabels.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::ShowLabels.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowLabels)
        );
        assert_eq!(Action::ShowLabels.id(), "labels.show");
        assert_eq!(Action::ShowLabels.spec().title, "Show Labels");
        assert_eq!(Action::ShowLabels.spec().category, Category::Repository);
        assert_eq!(Action::ShowLabels.spec().shortcut, None);
        assert!(!Action::ShowLabels.opens_dialog());
        assert!(Action::ShowLabels.available());
        assert!(Action::ShowLabels.listed_in_palette());
        assert!(
            Action::ALL.contains(&Action::ShowLabels),
            "the palette lists it"
        );
        assert_eq!(Action::from_id("labels.show"), Some(Action::ShowLabels));
    }

    #[test]
    fn show_why_needs_a_text_file_and_picked_lines_in_the_code_viewer() {
        let mut picked = ctx(&[], "");
        picked.code = CodeCtx {
            file: Some("src/a.rs"),
            lines: Some((3, 5)),
        };
        assert_eq!(Action::CodeProvenance.enabled(&Args::None, &picked), Ok(()));
        assert_eq!(
            Action::CodeProvenance.plan(&Args::None, &picked),
            Ok(Effect::ShowProvenance(crate::provenance_view::Asked {
                path: "src/a.rs".into(),
                from: 3,
                to: 5,
            }))
        );
        let mut no_pick = ctx(&[], "");
        no_pick.code.file = Some("src/a.rs");
        assert_eq!(
            Action::CodeProvenance.enabled(&Args::None, &no_pick),
            Err("Select lines of the file first.")
        );
        assert_eq!(
            Action::CodeProvenance.enabled(&Args::None, &ctx(&[], "")),
            Err("Open a text file in the Code viewer first.")
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::CodeProvenance.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::CodeProvenance.id(), "code.provenance");
        assert_eq!(
            Action::CodeProvenance.spec().title,
            "Show Why This Code Is Here…"
        );
        assert_eq!(Action::CodeProvenance.spec().category, Category::Repository);
        assert_eq!(Action::CodeProvenance.spec().shortcut, None);
        assert!(!Action::CodeProvenance.opens_dialog());
        assert!(
            Action::ALL.contains(&Action::CodeProvenance),
            "the palette lists it"
        );
        assert_eq!(
            Action::from_id("code.provenance"),
            Some(Action::CodeProvenance)
        );
    }

    fn label_ctx(selected: Option<LabelRef<'_>>) -> Context<'_> {
        Context {
            selected_label: selected,
            selected_memory: None,
            ..ctx(&[], "")
        }
    }

    #[test]
    fn the_label_actions_have_their_ids_titles_targets_and_no_keys() {
        let expected = [
            (Action::LabelNew, "labels.new", "New Label…", ArgKind::None),
            (
                Action::LabelRename,
                "labels.rename",
                "Rename Label…",
                ArgKind::Label,
            ),
            (
                Action::LabelDelete,
                "labels.delete",
                "Delete Label…",
                ArgKind::Label,
            ),
        ];
        for (action, id, title, args) in expected {
            assert_eq!(action.id(), id);
            assert_eq!(action.spec().title, title);
            assert_eq!(action.spec().args, args, "{action:?}");
            assert_eq!(action.spec().category, Category::Repository);
            assert_eq!(action.spec().shortcut, None, "{action:?}");
            assert!(action.available());
            assert!(action.listed_in_palette());
            assert!(Action::ALL.contains(&action));
            assert_eq!(Action::from_id(id), Some(action));
        }
        assert!(Action::LabelNew.opens_dialog());
        assert!(Action::LabelRename.opens_dialog());
        assert!(Action::LabelDelete.opens_dialog());
        assert!(Action::LabelDelete.destructive());
        assert!(!Action::LabelRename.destructive());
    }

    #[test]
    fn new_label_needs_an_open_repository_and_opens_its_dialog() {
        assert_eq!(
            Action::LabelNew.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::OpenLabelDialog(LabelDialog::New))
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::LabelNew.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    /// A context with a repository open and the memory given selected, or none.
    fn memory_ctx(selected: Option<MemoryRef<'_>>) -> Context<'_> {
        Context {
            selected_memory: selected,
            ..ctx(&[], "")
        }
    }

    #[test]
    fn the_memory_actions_have_their_ids_titles_and_targets() {
        let expected = [
            (
                Action::ShowMemories,
                "memories.show",
                "Show Memories",
                ArgKind::None,
            ),
            (
                Action::NewMemory,
                "memories.new",
                "New Memory",
                ArgKind::None,
            ),
            (
                Action::AcceptMemory,
                "memories.accept",
                "Accept Memory",
                ArgKind::Memory,
            ),
            (
                Action::RejectMemory,
                "memories.reject",
                "Reject Memory",
                ArgKind::Memory,
            ),
            (
                Action::MarkMemoryUseful,
                "memories.mark_useful",
                "Mark Memory Useful",
                ArgKind::Memory,
            ),
            (
                Action::MarkMemoryWrong,
                "memories.mark_wrong",
                "Mark Memory Wrong",
                ArgKind::Memory,
            ),
            (
                Action::ManageContexts,
                "contexts.manage",
                "Manage Contexts…",
                ArgKind::None,
            ),
            (
                Action::MoveMemoryToContext,
                "memories.move_to_context",
                "Move to Context…",
                ArgKind::Memory,
            ),
            (
                Action::AttachMemoryToContext,
                "memories.attach_to_context",
                "Attach to Context…",
                ArgKind::Memory,
            ),
            (
                Action::DetachMemoryFromContext,
                "memories.detach_from_context",
                "Detach from Context…",
                ArgKind::Memory,
            ),
        ];
        for (action, id, title, args) in expected {
            assert_eq!(action.id(), id);
            assert_eq!(action.spec().title, title);
            assert_eq!(action.spec().args, args);
            assert_eq!(action.spec().shortcut, None);
            assert_eq!(Action::from_id(id), Some(action));
        }
        assert!(Action::NewMemory.opens_dialog());
        assert!(!Action::AcceptMemory.opens_dialog());
    }

    #[test]
    fn a_mark_is_planned_only_for_the_selected_accepted_memory() {
        let none = memory_ctx(None);
        assert_eq!(
            Action::MarkMemoryUseful.enabled(&Args::None, &none),
            Err(SELECT_MEMORY)
        );
        let proposal = memory_ctx(Some(MemoryRef {
            uid: "m1",
            proposed: true,
            placement: own(gm_schema::threads::MemoryStatus::Proposed),
        }));
        let proposed_target = memory_args(MemoryRef {
            uid: "m1",
            proposed: true,
            placement: own(gm_schema::threads::MemoryStatus::Proposed),
        });
        assert_eq!(
            Action::MarkMemoryWrong.enabled(&proposed_target, &proposal),
            Err("Only an accepted memory can be marked.")
        );
        let accepted = memory_ctx(Some(MemoryRef {
            uid: "m1",
            proposed: false,
            placement: own(gm_schema::threads::MemoryStatus::Accepted),
        }));
        let target = memory_args(MemoryRef {
            uid: "m1",
            proposed: false,
            placement: own(gm_schema::threads::MemoryStatus::Accepted),
        });
        assert_eq!(Action::MarkMemoryUseful.enabled(&target, &accepted), Ok(()));
        assert_eq!(
            Action::MarkMemoryUseful.plan(&target, &accepted),
            Ok(Effect::Memory(MemoryEffect::Mark {
                uid: "m1".into(),
                mark: MemoryMark::Useful,
            }))
        );
        assert_eq!(
            Action::MarkMemoryWrong.plan(&target, &accepted),
            Ok(Effect::Memory(MemoryEffect::Mark {
                uid: "m1".into(),
                mark: MemoryMark::Wrong,
            }))
        );
        let other = memory_args(MemoryRef {
            uid: "m2",
            proposed: false,
            placement: own(gm_schema::threads::MemoryStatus::Accepted),
        });
        assert_eq!(
            Action::MarkMemoryWrong.enabled(&other, &accepted),
            Err(SELECT_MEMORY)
        );
    }

    #[test]
    fn show_and_new_memory_need_an_open_repository_and_plan_the_tab_and_the_dialog() {
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowMemories.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::NewMemory.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::ShowMemories.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::Memory(MemoryEffect::Show))
        );
        assert_eq!(
            Action::NewMemory.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::Memory(MemoryEffect::New))
        );
    }

    #[test]
    fn accept_and_reject_act_on_the_selected_proposal_or_say_why_not() {
        let none = memory_ctx(None);
        assert_eq!(
            Action::AcceptMemory.enabled(&Args::None, &none),
            Err(SELECT_MEMORY)
        );
        assert_eq!(
            Action::RejectMemory.plan(&Args::None, &none),
            Err(SELECT_MEMORY)
        );
        let accepted = memory_ctx(Some(MemoryRef {
            uid: "m1",
            proposed: false,
            placement: own(gm_schema::threads::MemoryStatus::Accepted),
        }));
        let accepted_target = memory_args(MemoryRef {
            uid: "m1",
            proposed: false,
            placement: own(gm_schema::threads::MemoryStatus::Accepted),
        });
        assert_eq!(
            Action::RejectMemory.enabled(&accepted_target, &accepted),
            Err("Only a proposed memory can be accepted or rejected.")
        );
        let proposal = memory_ctx(Some(MemoryRef {
            uid: "m1",
            proposed: true,
            placement: own(gm_schema::threads::MemoryStatus::Proposed),
        }));
        let target = memory_args(MemoryRef {
            uid: "m1",
            proposed: true,
            placement: own(gm_schema::threads::MemoryStatus::Proposed),
        });
        assert_eq!(Action::AcceptMemory.enabled(&target, &proposal), Ok(()));
        assert_eq!(
            Action::AcceptMemory.plan(&target, &proposal),
            Ok(Effect::Memory(MemoryEffect::Accept("m1".into())))
        );
        assert_eq!(
            Action::RejectMemory.plan(&target, &proposal),
            Ok(Effect::Memory(MemoryEffect::Reject("m1".into())))
        );
        let closed = Context {
            repo_open: false,
            ..memory_ctx(Some(MemoryRef {
                uid: "m1",
                proposed: true,
                placement: own(gm_schema::threads::MemoryStatus::Proposed),
            }))
        };
        assert_eq!(
            Action::AcceptMemory.enabled(&target, &closed),
            Err("No repository is open.")
        );
    }

    /// The placement of a repository memory in the given status: in no context, with no links.
    fn own(status: gm_schema::threads::MemoryStatus) -> Placement {
        Placement {
            in_context: false,
            status,
            linked: false,
            contexts: 0,
        }
    }

    #[test]
    fn the_context_moves_follow_the_placement_the_engine_takes() {
        use gm_schema::threads::MemoryStatus;
        let own_accepted = MemoryRef {
            uid: "m1",
            proposed: false,
            placement: own(MemoryStatus::Accepted),
        };
        let target = memory_args(own_accepted);
        assert_eq!(
            Action::MoveMemoryToContext.plan(&target, &memory_ctx(Some(own_accepted))),
            Ok(Effect::Memory(MemoryEffect::MoveToContext("m1".into())))
        );
        let in_context = MemoryRef {
            uid: "m1",
            proposed: false,
            placement: Placement {
                in_context: true,
                status: MemoryStatus::Accepted,
                linked: false,
                contexts: 1,
            },
        };
        let inside = memory_ctx(Some(in_context));
        assert_eq!(
            Action::MoveMemoryToContext.enabled(&target, &inside),
            Ok(()),
            "a memory of a context moves to another context too (D222 (1))"
        );
        assert_eq!(
            Action::AttachMemoryToContext.plan(&target, &inside),
            Ok(Effect::Memory(MemoryEffect::AttachToContext("m1".into())))
        );
        assert_eq!(
            Action::DetachMemoryFromContext.plan(&target, &inside),
            Ok(Effect::Memory(MemoryEffect::DetachFromContext("m1".into())))
        );
        let proposed_in_context = MemoryRef {
            uid: "m1",
            proposed: true,
            placement: Placement {
                in_context: true,
                status: MemoryStatus::Proposed,
                linked: false,
                contexts: 1,
            },
        };
        assert_eq!(
            Action::AttachMemoryToContext.enabled(
                &memory_args(proposed_in_context),
                &memory_ctx(Some(proposed_in_context))
            ),
            Err("Only an accepted memory can be attached to another context.")
        );
        assert_eq!(
            Action::DetachMemoryFromContext.enabled(&target, &memory_ctx(Some(own_accepted))),
            Err("This memory is in this repository only, so it has no context to detach from.")
        );
        assert_eq!(
            Action::MoveMemoryToContext.enabled(&Args::None, &memory_ctx(None)),
            Err(SELECT_MEMORY)
        );
        let closed = Context {
            repo_open: false,
            ..memory_ctx(Some(own_accepted))
        };
        assert_eq!(
            Action::MoveMemoryToContext.enabled(&target, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn manage_contexts_needs_an_open_repository_and_opens_its_dialog() {
        assert!(Action::ManageContexts.opens_dialog());
        assert_eq!(
            Action::ManageContexts.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::Memory(MemoryEffect::ManageContexts))
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ManageContexts.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn rename_and_delete_act_on_the_selected_label_or_say_none_is_selected() {
        let none = label_ctx(None);
        assert_eq!(
            Action::LabelRename.enabled(&Args::None, &none),
            Err("Select a label first.")
        );
        assert_eq!(
            Action::LabelDelete.plan(&Args::None, &none),
            Err("Select a label first.")
        );
        let selected = label_ctx(Some(LabelRef {
            id: 5,
            name: "flaky",
            attachments: 3,
        }));
        let target = label_args(LabelRef {
            id: 5,
            name: "flaky",
            attachments: 3,
        });
        assert_eq!(
            Action::LabelRename.plan(&target, &selected),
            Ok(Effect::OpenLabelDialog(LabelDialog::Rename {
                id: 5,
                name: "flaky".into(),
            }))
        );
        assert_eq!(
            Action::LabelDelete.plan(&target, &selected),
            Ok(Effect::OpenLabelDialog(LabelDialog::Delete {
                id: 5,
                name: "flaky".into(),
                attachments: 3,
            }))
        );
        let closed = Context {
            repo_open: false,
            ..label_ctx(None)
        };
        assert_eq!(
            Action::LabelDelete.enabled(&target, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn the_label_actions_take_only_a_label_target() {
        let target = Args::Label {
            id: 1,
            name: "a".into(),
            attachments: 0,
        };
        assert!(Action::LabelRename.takes(&target));
        assert!(!Action::LabelRename.takes(&Args::File("a".into())));
        assert!(!Action::LabelNew.takes(&target));
        assert_eq!(
            Action::LabelRename.enabled(&Args::File("a".into()), &label_ctx(None)),
            Err("No target for this action")
        );
    }

    #[test]
    fn the_reflog_actions_have_their_ids_titles_categories_and_no_keys() {
        for (action, id, title) in [
            (Action::ShowReflog, "reflog.show", "Show Reflog"),
            (
                Action::CheckoutCommit,
                "commit.checkout",
                "Check Out Commit (Detached)",
            ),
            (
                Action::NewBranchFromCommit,
                "branch.new_from_commit",
                "New Branch From Commit…",
            ),
        ] {
            assert_eq!(action.id(), id);
            assert_eq!(action.spec().title, title);
            assert_eq!(action.spec().shortcut, None);
            assert!(action.available());
        }
        assert_eq!(Action::CheckoutCommit.spec().category, Category::Branches);
        assert_eq!(
            Action::NewBranchFromCommit.spec().category,
            Category::Branches
        );
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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

    fn stash(index: u32, oid: &str) -> StashEntry {
        StashEntry {
            index,
            oid: oid.repeat(40),
            message: format!("On main: stash {index}"),
            branch: Some("main".into()),
            time: 1_000,
        }
    }

    fn with_stashes(stashes: &[StashEntry]) -> Context<'_> {
        Context {
            stashes: Some(stashes),
            review: ReviewCtx::default(),
            ..ctx(&[], "")
        }
    }

    #[test]
    fn apply_pop_and_drop_have_their_ids_titles_category_and_no_keys() {
        for (action, id, title) in [
            (Action::StashApply, "stash.apply", "Apply Stash"),
            (Action::StashPop, "stash.pop", "Pop Stash"),
            (Action::StashDrop, "stash.drop", "Drop Stash"),
        ] {
            assert_eq!(action.id(), id);
            assert_eq!(action.spec().title, title);
            assert_eq!(action.spec().args, ArgKind::Stash);
            assert_eq!(action.spec().category, Category::WorkingCopy);
            assert_eq!(action.spec().shortcut, None);
            assert_eq!(Action::from_id(id), Some(action));
        }
    }

    #[test]
    fn apply_and_pop_send_the_listed_stash_by_its_index_and_commit_id() {
        let stashes = [stash(0, "a"), stash(1, "b")];
        let context = with_stashes(&stashes);
        let target = stash_args(&stashes[1]);
        assert_eq!(
            Action::StashApply.plan(&target, &context),
            Ok(Effect::Send(vec![CommandKind::ApplyStash {
                index: 1,
                oid: "b".repeat(40),
            }]))
        );
        assert_eq!(
            Action::StashPop.plan(&target, &context),
            Ok(Effect::Send(vec![CommandKind::PopStash {
                index: 1,
                oid: "b".repeat(40),
            }]))
        );
    }

    #[test]
    fn apply_and_pop_are_not_refused_for_a_dirty_tree_git_decides_that() {
        let stashes = [stash(0, "a")];
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let dirty = Context {
            entries: &entries,
            staged_count: 0,
            ..with_stashes(&stashes)
        };
        let target = stash_args(&stashes[0]);
        assert_eq!(Action::StashApply.enabled(&target, &dirty), Ok(()));
        assert_eq!(Action::StashPop.enabled(&target, &dirty), Ok(()));
    }

    #[test]
    fn drop_opens_its_dialog_and_never_sends_the_command() {
        let stashes = [stash(0, "a"), stash(1, "b")];
        let target = stash_args(&stashes[1]);
        assert_eq!(
            Action::StashDrop.plan(&target, &with_stashes(&stashes)),
            Ok(Effect::OpenDropStashDialog {
                index: 1,
                oid: "b".repeat(40),
                label: "stash@{1}".into(),
            })
        );
        assert!(Action::StashDrop.opens_dialog());
        assert!(Action::StashDrop.destructive());
        for action in [Action::StashApply, Action::StashPop] {
            assert!(!action.opens_dialog(), "{action:?}");
            assert!(!action.destructive(), "{action:?}");
        }
    }

    #[test]
    fn a_stash_action_needs_a_listing_an_idle_repository_and_the_listed_stash() {
        let stashes = [stash(0, "b")];
        let target = stash_args(&stashes[0]);
        let unlisted = ctx(&[], "");
        for action in [Action::StashApply, Action::StashPop, Action::StashDrop] {
            assert_eq!(
                action.enabled(&target, &unlisted),
                Err("Open the Stashes page first."),
                "{action:?}"
            );
            let closed = Context {
                repo_open: false,
                selected_file: None,
                selected_thread: None,
                selected_label: None,
                selected_memory: None,
                features: FeatureSet::all(),
                undoable: None,
                stack: crate::action_registry::StackCtx::default(),
                root: None,
                ..with_stashes(&stashes)
            };
            assert_eq!(
                action.enabled(&target, &closed),
                Err("No repository is open."),
                "{action:?}"
            );
            let busy = Context {
                busy: Some("Wait for the branch checkout to finish."),
                ..with_stashes(&stashes)
            };
            assert_eq!(
                action.enabled(&target, &busy),
                Err("Wait for the branch checkout to finish."),
                "{action:?}"
            );
            assert_eq!(action.enabled(&target, &with_stashes(&stashes)), Ok(()));
        }
    }

    #[test]
    fn a_stash_that_moved_or_is_not_listed_is_refused_with_the_list_to_read_again() {
        let stashes = [stash(0, "b")];
        let context = with_stashes(&stashes);
        // The stash `b` was at index 1 when the list was read; `stash@{1}` is now gone.
        let moved = Args::Stash {
            index: 1,
            oid: "b".repeat(40),
            label: "stash@{1}".into(),
        };
        let unknown = Args::Stash {
            index: 0,
            oid: "z".repeat(40),
            label: "stash@{0}".into(),
        };
        for action in [Action::StashApply, Action::StashPop, Action::StashDrop] {
            for args in [&moved, &unknown] {
                assert_eq!(
                    action.enabled(args, &context),
                    Err("This stash is no longer listed. Open the Stashes page again."),
                    "{action:?}"
                );
            }
        }
    }

    #[test]
    fn the_stash_actions_take_only_a_stash_target() {
        let stashes = [stash(0, "a")];
        let context = with_stashes(&stashes);
        for action in [Action::StashApply, Action::StashPop, Action::StashDrop] {
            for args in [Args::None, Args::Branch("main".into())] {
                assert_eq!(
                    action.enabled(&args, &context),
                    Err("No target for this action"),
                    "{action:?}"
                );
            }
        }
    }

    #[test]
    fn the_picture_actions_switch_the_lookup_and_refuse_a_no_op() {
        let hidden = Context {
            author_pictures: AuthorPictures::Hidden,
            ..ctx(&[], "")
        };
        let shown = Context {
            author_pictures: AuthorPictures::Shown,
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowAuthorPictures.plan(&Args::None, &hidden),
            Ok(Effect::SetAuthorPictures(true))
        );
        assert_eq!(
            Action::HideAuthorPictures.plan(&Args::None, &shown),
            Ok(Effect::SetAuthorPictures(false))
        );
        assert!(
            Action::ShowAuthorPictures
                .enabled(&Args::None, &shown)
                .is_err()
        );
        assert!(
            Action::HideAuthorPictures
                .enabled(&Args::None, &hidden)
                .is_err()
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::PruneWorktrees.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    fn with_messages(messages: MessageCtx<'_>) -> Context<'_> {
        Context {
            messages,
            ..ctx(&[], "draft")
        }
    }

    #[test]
    fn suggesting_needs_a_commit_transform_and_says_how_to_add_one() {
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_messages(MessageCtx::default())),
            Err(
                "No workflow suggests commit messages. Add one on the transform.commit_message phase."
            )
        );
    }

    #[test]
    fn suggesting_runs_the_transform_when_one_is_loaded() {
        let loaded = MessageCtx {
            transform: true,
            ..MessageCtx::default()
        };
        assert_eq!(
            Action::SuggestMessage.enabled(&Args::None, &with_messages(loaded)),
            Ok(())
        );
        assert_eq!(
            Action::SuggestMessage.plan(&Args::None, &with_messages(loaded)),
            Ok(Effect::Send(vec![CommandKind::RunTransform {
                field: "commit_message".into(),
                current: "draft".into(),
            }]))
        );
    }

    #[test]
    fn a_suggestion_is_used_or_dropped_and_needs_to_exist() {
        let offered = MessageCtx {
            suggestion: Some("feat: x"),
            ..MessageCtx::default()
        };
        assert_eq!(
            Action::UseSuggestion.plan(&Args::None, &with_messages(offered)),
            Ok(Effect::UseSuggestion("feat: x".into()))
        );
        assert_eq!(
            Action::DiscardSuggestion.plan(&Args::None, &with_messages(offered)),
            Ok(Effect::DiscardSuggestion)
        );
        assert_eq!(
            Action::UseSuggestion.enabled(&Args::None, &with_messages(MessageCtx::default())),
            Err("There is no suggestion.")
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
            reviewed: false,
            auto_approve: false,
            enabled: false,
            title: None,
            location: gm_schema::WorkflowLocation::User,
            role: gm_schema::WorkflowRole::Workflow,
            description: None,
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
        let page = |shown, name, file| PageState { shown, name, file };
        let closed = with_workflows(WorkflowCtx::default());
        assert_eq!(
            Action::OpenWorkflowInEditor.enabled(&Args::None, &closed),
            Err("Open the Workflows page first.")
        );
        let open = with_workflows(WorkflowCtx {
            page: page(true, Some("w"), Some("/w/w.lua")),
            ..WorkflowCtx::default()
        });
        assert_eq!(
            Action::OpenWorkflowInEditor.plan(&Args::None, &open),
            Ok(Effect::Send(vec![CommandKind::OpenWorkflowInEditor {
                path: "/w/w.lua".into(),
            }]))
        );
        assert_eq!(
            Action::CopyWorkflowPath.plan(&Args::None, &open),
            Ok(Effect::Copy("/w/w.lua".into()))
        );
        assert_eq!(
            Action::DeleteWorkflow.plan(&Args::None, &open),
            Ok(Effect::OpenDeleteWorkflow)
        );
        // A built-in workflow is open, and it has no file to hand to an editor.
        let built_in = with_workflows(WorkflowCtx {
            page: page(true, Some("w"), None),
            ..WorkflowCtx::default()
        });
        for action in [Action::OpenWorkflowInEditor, Action::CopyWorkflowPath] {
            assert_eq!(
                action.enabled(&Args::None, &built_in),
                Err("A built-in workflow has no file: use Copy to Edit first.")
            );
        }
    }

    #[test]
    fn dry_run_needs_an_open_file() {
        let open = || {
            with_workflows(WorkflowCtx {
                page: PageState {
                    shown: true,
                    name: Some("w"),
                    ..PageState::default()
                },
                ..WorkflowCtx::default()
            })
        };
        assert_eq!(
            Action::DryRunWorkflow.plan(&Args::None, &open()),
            Ok(Effect::DryRunWorkflow)
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
    fn show_workflows_needs_a_repository_and_opens_the_page() {
        let open = ctx(&[], "");
        assert_eq!(
            Action::ShowWorkflows.plan(&Args::None, &open),
            Ok(Effect::ShowWorkflows)
        );
        assert_eq!(Action::ShowWorkflows.id(), "workflow.show");
        assert_eq!(Action::OpenWorkflowInEditor.spec().shortcut, Some("⌘E"));
        assert_eq!(
            Action::OpenWorkflowInEditor.spec().title,
            "Open Workflow in Editor"
        );
        assert_eq!(Action::CopyWorkflowPath.id(), "workflow.copy_path");
    }

    #[test]
    fn the_dry_run_action_has_its_id_title_and_no_key() {
        assert_eq!(Action::DryRunWorkflow.id(), "workflow.dry_run");
        assert_eq!(Action::DryRunWorkflow.spec().title, "Dry Run Workflow");
        assert_eq!(Action::DryRunWorkflow.spec().shortcut, None);
        assert_eq!(Action::DryRunWorkflow.spec().category, Category::Workflows);
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
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

    fn lines(path: &str, working: Option<(u32, u32)>) -> CommentSpot<'_> {
        CommentSpot::Lines { path, working }
    }

    fn review_ctx<'a>(spot: CommentSpot<'a>, comments: Option<&'a [ReviewComment]>) -> Context<'a> {
        Context {
            review: ReviewCtx { comments, spot },
            ..ctx(&[], "")
        }
    }

    fn comment_with(id: i64, status: ReviewStatus) -> ReviewComment {
        ReviewComment {
            id,
            path: "a.txt".into(),
            body: "note".into(),
            status,
            anchor: gm_schema::ReviewAnchor {
                text: "x".into(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 1,
            },
            created_at: 0,
            resolved_at: None,
            snapshot: None,
            after_snapshot: None,
            author: None,
            kind: None,
            severity: None,
            needs_answer: false,
            suggested_fix: None,
            label: None,
            decorations: Vec::new(),
            scorecard: None,
            location: Some(gm_schema::ReviewLocation {
                start_line: 1,
                end_line: 1,
            }),
            outdated: false,
            replies: Vec::new(),
            workspace: None,
        }
    }

    #[test]
    fn comment_on_selection_needs_picked_lines_the_working_tree_has_or_text_to_look_for() {
        let shown = review_ctx(lines("a.txt", Some((12, 15))), None);
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &shown),
            Ok(())
        );
        let none = review_ctx(CommentSpot::Nothing, None);
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &none),
            Err("Select lines to comment on first.")
        );
        let removed = review_ctx(lines("a.txt", None), None);
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &removed),
            Err("Those lines are not in the working tree.")
        );
        let staged = review_ctx(CommentSpot::Text { path: "a.txt" }, None);
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &staged),
            Ok(())
        );
        let split = review_ctx(CommentSpot::Split, None);
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &split),
            Err("Select lines of one hunk to comment on.")
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..review_ctx(lines("a.txt", Some((1, 1))), None)
        };
        assert_eq!(
            Action::CommentOnSelection.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn comment_on_selection_plans_the_composer_for_the_lines_it_covers() {
        let shown = review_ctx(lines("src/a.rs", Some((12, 15))), None);
        assert_eq!(
            Action::CommentOnSelection.plan(&Args::None, &shown),
            Ok(Effect::OpenReviewComposer {
                path: "src/a.rs".into(),
                start_line: 12,
                end_line: 15,
            })
        );
        assert_eq!(
            Action::CommentOnSelection.plan(&Args::None, &review_ctx(CommentSpot::Nothing, None)),
            Err("Select lines to comment on first.")
        );
    }

    #[test]
    fn comment_on_selection_by_text_plans_the_composer_for_the_picked_text() {
        let staged = review_ctx(CommentSpot::Text { path: "src/a.rs" }, None);
        assert_eq!(
            Action::CommentOnSelection.plan(&Args::None, &staged),
            Ok(Effect::OpenReviewComposerByText {
                path: "src/a.rs".into(),
            })
        );
    }

    #[test]
    fn the_comment_action_is_listed_with_its_key_and_opens_a_dialog() {
        assert_eq!(Action::CommentOnSelection.id(), "review.comment");
        assert_eq!(Action::CommentOnSelection.spec().shortcut, Some("⇧⌘M"));
        assert_eq!(Action::CommentOnSelection.spec().args, ArgKind::None);
        assert_eq!(
            Action::CommentOnSelection.spec().category,
            Category::WorkingCopy
        );
        assert!(Action::CommentOnSelection.opens_dialog());
        assert!(!Action::CommentOnSelection.destructive());
    }

    #[test]
    fn resolve_reopen_edit_and_delete_need_the_listed_comment_in_the_right_status() {
        let open = [comment_with(4, ReviewStatus::Open)];
        let done = [comment_with(5, ReviewStatus::Resolved)];
        let listed = review_ctx(CommentSpot::Nothing, Some(&open));
        assert_eq!(
            Action::ResolveReviewComment.plan(&Args::ReviewComment(4), &listed),
            Ok(Effect::Send(vec![CommandKind::ResolveReviewComment {
                id: 4
            }]))
        );
        assert_eq!(
            Action::ReopenReviewComment.plan(&Args::ReviewComment(4), &listed),
            Err("This comment is already open.")
        );
        assert_eq!(
            Action::DeleteReviewComment.plan(&Args::ReviewComment(4), &listed),
            Ok(Effect::Send(vec![CommandKind::DeleteReviewComment {
                id: 4
            }]))
        );
        assert_eq!(
            Action::EditReviewComment.plan(&Args::ReviewComment(4), &listed),
            Ok(Effect::OpenReviewEdit { id: 4 })
        );
        let closed = review_ctx(CommentSpot::Nothing, Some(&done));
        assert_eq!(
            Action::ResolveReviewComment.enabled(&Args::ReviewComment(5), &closed),
            Err("This comment is already resolved.")
        );
        assert_eq!(
            Action::ReopenReviewComment.plan(&Args::ReviewComment(5), &closed),
            Ok(Effect::Send(vec![CommandKind::ReopenReviewComment {
                id: 5
            }]))
        );
    }

    #[test]
    fn a_comment_action_needs_the_listing_and_a_listed_comment() {
        let not_read = review_ctx(CommentSpot::Nothing, None);
        assert_eq!(
            Action::DeleteReviewComment.enabled(&Args::ReviewComment(4), &not_read),
            Err("The comments are still being read.")
        );
        let listed = [comment_with(4, ReviewStatus::Open)];
        let gone = review_ctx(CommentSpot::Nothing, Some(&listed));
        assert_eq!(
            Action::EditReviewComment.enabled(&Args::ReviewComment(9), &gone),
            Err("This comment is no longer listed.")
        );
        assert_eq!(
            Action::EditReviewComment.enabled(&Args::File("a.txt".into()), &gone),
            Err("No target for this action")
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..review_ctx(CommentSpot::Nothing, Some(&listed))
        };
        assert_eq!(
            Action::ResolveReviewComment.enabled(&Args::ReviewComment(4), &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn delete_comment_is_destructive_and_the_review_actions_have_their_ids() {
        assert!(Action::DeleteReviewComment.destructive());
        assert!(!Action::DeleteReviewComment.opens_dialog());
        assert_eq!(Action::ResolveReviewComment.id(), "review.resolve");
        assert_eq!(Action::ReopenReviewComment.id(), "review.reopen");
        assert_eq!(Action::DeleteReviewComment.id(), "review.delete");
        assert_eq!(Action::EditReviewComment.id(), "review.edit");
        for action in [
            Action::ResolveReviewComment,
            Action::ReopenReviewComment,
            Action::DeleteReviewComment,
            Action::EditReviewComment,
        ] {
            assert_eq!(action.spec().args, ArgKind::ReviewComment);
            assert_eq!(action.spec().shortcut, None);
        }
    }

    fn open_ctx<'a>(
        entries: &'a [StatusEntry],
        listed: Option<&'a [ReviewComment]>,
    ) -> Context<'a> {
        Context {
            review: ReviewCtx {
                comments: listed,
                spot: CommentSpot::Nothing,
            },
            ..ctx(entries, "")
        }
    }

    #[test]
    fn show_local_review_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowLocalReview.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowLocalReview)
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ShowLocalReview.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(Action::ShowLocalReview.id(), "review.show");
        assert_eq!(Action::ShowLocalReview.spec().title, "Show Local Review");
        assert_eq!(
            Action::ShowLocalReview.spec().category,
            Category::Repository
        );
        assert_eq!(Action::ShowLocalReview.spec().shortcut, None);
        assert!(!Action::ShowLocalReview.opens_dialog());
        assert!(Action::ShowLocalReview.available());
    }

    #[test]
    fn open_comment_in_diff_names_the_file_and_the_first_line_of_a_current_comment() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let listed = [comment_with(4, ReviewStatus::Open)];
        let context = open_ctx(&entries, Some(&listed));
        assert_eq!(
            Action::OpenReviewComment.enabled(&Args::ReviewComment(4), &context),
            Ok(())
        );
        assert_eq!(
            Action::OpenReviewComment.plan(&Args::ReviewComment(4), &context),
            Ok(Effect::ShowReviewInDiff {
                path: "a.txt".into(),
                line: 1,
            })
        );
        assert_eq!(Action::OpenReviewComment.id(), "review.open");
        assert_eq!(
            Action::OpenReviewComment.spec().title,
            "Open Comment in Diff"
        );
        assert_eq!(
            Action::OpenReviewComment.spec().args,
            ArgKind::ReviewComment
        );
        assert!(!Action::OpenReviewComment.destructive());
    }

    #[test]
    fn open_comment_in_diff_is_refused_when_the_comment_or_its_file_cannot_be_shown() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let mut outdated = comment_with(4, ReviewStatus::Open);
        outdated.location = None;
        outdated.outdated = true;
        let stale = [outdated];
        assert_eq!(
            Action::OpenReviewComment
                .plan(&Args::ReviewComment(4), &open_ctx(&entries, Some(&stale))),
            Err("This comment is outdated.")
        );
        let current = [comment_with(4, ReviewStatus::Open)];
        assert_eq!(
            Action::OpenReviewComment.plan(&Args::ReviewComment(4), &open_ctx(&[], Some(&current))),
            Err("This file has no changes to show in the Working Copy.")
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
            selected_label: None,
            selected_memory: None,
            features: FeatureSet::all(),
            undoable: None,
            stack: crate::action_registry::StackCtx::default(),
            ..open_ctx(&entries, Some(&current))
        };
        assert_eq!(
            Action::OpenReviewComment.enabled(&Args::ReviewComment(4), &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::OpenReviewComment.enabled(&Args::ReviewComment(4), &open_ctx(&entries, None)),
            Err("The comments are still being read.")
        );
        assert_eq!(
            Action::OpenReviewComment
                .enabled(&Args::ReviewComment(9), &open_ctx(&entries, Some(&current))),
            Err("This comment is no longer listed.")
        );
        assert_eq!(
            Action::OpenReviewComment.enabled(&Args::None, &open_ctx(&entries, Some(&current))),
            Err("No target for this action")
        );
    }

    fn move_ctx<'a>(
        entries: &'a [StatusEntry],
        worktrees: &'a [WorktreeEntry],
        selected: Option<&'a str>,
    ) -> Context<'a> {
        Context {
            selected_file: selected,
            worktrees,
            ..ctx(entries, "")
        }
    }

    #[test]
    fn the_move_actions_have_their_ids_titles_targets_and_no_keys() {
        assert_eq!(Action::MoveFileToBranch.id(), "file.move_to_branch");
        assert_eq!(
            Action::MoveFileToBranch.spec().title,
            "Move File to Branch…"
        );
        assert_eq!(Action::MoveFileToBranch.spec().args, ArgKind::Branch);
        assert_eq!(Action::MoveFileToWorktree.id(), "file.move_to_worktree");
        assert_eq!(
            Action::MoveFileToWorktree.spec().title,
            "Move File to Worktree…"
        );
        assert_eq!(Action::MoveFileToWorktree.spec().args, ArgKind::Worktree);
        for action in [Action::MoveFileToBranch, Action::MoveFileToWorktree] {
            assert_eq!(action.spec().shortcut, None);
            assert_eq!(action.spec().category, Category::WorkingCopy);
            assert!(action.available());
            assert!(!action.destructive());
        }
    }

    #[test]
    fn a_branch_move_takes_the_selected_file_and_sends_it_to_the_drop_path() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = move_ctx(&entries, &[], Some("a.txt"));
        assert_eq!(
            Action::MoveFileToBranch.plan(&Args::Branch("feature".into()), &context),
            Ok(Effect::MoveFileTo {
                path: "a.txt".into(),
                target: DropTarget::Branch {
                    name: "feature".into()
                },
            })
        );
    }

    #[test]
    fn a_move_needs_a_selected_changed_file_in_an_idle_repository() {
        let mut conflicted = entry("c.txt", None, Some(Change::Modified));
        conflicted.conflicted = true;
        let entries = [entry("a.txt", None, Some(Change::Modified)), conflicted];
        let target = Args::Branch("feature".into());
        assert_eq!(
            Action::MoveFileToBranch.enabled(&target, &move_ctx(&entries, &[], None)),
            Err("Select a changed file to move first.")
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&target, &move_ctx(&entries, &[], Some("c.txt"))),
            Err("Resolve the conflict first.")
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&target, &move_ctx(&entries, &[], Some("gone.txt"))),
            Err("That file is not in the working copy")
        );
        let busy = Context {
            busy: Some("Wait for the refresh to finish."),
            ..move_ctx(&entries, &[], Some("a.txt"))
        };
        assert_eq!(
            Action::MoveFileToBranch.enabled(&target, &busy),
            Err("Wait for the refresh to finish.")
        );
        let closed = Context {
            repo_open: false,
            root: None,
            ..move_ctx(&entries, &[], Some("a.txt"))
        };
        assert_eq!(
            Action::MoveFileToWorktree.enabled(&Args::Worktree("/w/ui".into()), &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&target, &move_ctx(&entries, &[], Some("a.txt"))),
            Ok(())
        );
    }

    #[test]
    fn a_folder_cannot_be_moved_as_one_from_the_palette_or_the_menu() {
        let entries = [untracked("new/")];
        let context = move_ctx(&entries, &[], Some("new/"));
        assert_eq!(
            Action::MoveFileToBranch.enabled(&Args::Branch("feature".into()), &context),
            Err(FOLDER_REASON)
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&Args::File("new/".into()), &context),
            Err(FOLDER_REASON)
        );
    }

    #[test]
    fn a_branch_target_refuses_the_branch_of_the_window_and_one_checked_out_elsewhere() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let others = [listed("/w/ui", "ui", false)];
        let context = move_ctx(&entries, &others, Some("a.txt"));
        let refuse =
            |name: &str| Action::MoveFileToBranch.enabled(&Args::Branch(name.into()), &context);
        assert_eq!(refuse("main"), Err(SAME_BRANCH));
        assert_eq!(refuse("ui"), Err("Checked out in another worktree"));
        assert_eq!(refuse("feature"), Ok(()));
    }

    #[test]
    fn a_worktree_target_refuses_the_window_own_worktree_a_missing_folder_and_an_unlisted_one() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let mut missing = listed("/w/gone", "gone", false);
        missing.prunable = Some("gitdir points nowhere".into());
        let others = [
            listed("/repo", "main", true),
            listed("/w/ui", "ui", false),
            missing,
        ];
        let context = move_ctx(&entries, &others, Some("a.txt"));
        let refuse =
            |path: &str| Action::MoveFileToWorktree.enabled(&Args::Worktree(path.into()), &context);
        assert_eq!(refuse("/repo"), Err(SAME_WORKTREE));
        assert_eq!(refuse("/w/gone"), Err(MISSING_FOLDER));
        assert_eq!(refuse("/w/none"), Err("This worktree is not listed."));
        assert_eq!(refuse("/w/ui"), Ok(()));
        assert_eq!(
            Action::MoveFileToWorktree.plan(&Args::Worktree("/w/ui".into()), &context),
            Ok(Effect::MoveFileTo {
                path: "a.txt".into(),
                target: DropTarget::Worktree {
                    path: "/w/ui".into()
                },
            })
        );
    }

    #[test]
    fn the_file_menu_names_the_file_and_the_palette_then_picks_the_target() {
        let entries = [entry("a.txt", None, Some(Change::Modified))];
        let context = move_ctx(&entries, &[], None);
        assert_eq!(
            Action::MoveFileToWorktree.enabled(&Args::File("a.txt".into()), &context),
            Ok(())
        );
        assert_eq!(
            Action::MoveFileToBranch.plan(&Args::File("a.txt".into()), &context),
            Ok(Effect::PickMoveTarget {
                action: Action::MoveFileToBranch,
                path: "a.txt".into(),
            })
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&Args::File("gone.txt".into()), &context),
            Err("That file is not in the working copy")
        );
        assert_eq!(
            Action::MoveFileToBranch.enabled(&Args::Worktree("/w/ui".into()), &context),
            Err("No target for this action")
        );
        assert_eq!(
            Action::MoveFileToWorktree.enabled(&Args::Branch("feature".into()), &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn the_refusal_reasons_of_the_rules_are_the_drop_sentences() {
        assert_eq!(SAME_BRANCH, "This is the branch you are on");
        assert_eq!(SAME_WORKTREE, "This is the worktree you are in");
        assert_eq!(MISSING_FOLDER, "the worktree folder is missing");
    }

    #[test]
    fn resolve_conflict_is_enabled_only_for_a_conflicted_file_entry() {
        let file = Args::File("a.txt".into());
        let conflicted = [StatusEntry {
            conflicted: true,
            ..entry("a.txt", None, Some(Change::Modified))
        }];
        assert_eq!(
            Action::ResolveConflict.enabled(&file, &ctx(&conflicted, "")),
            Ok(())
        );
        let modified = [entry("a.txt", None, Some(Change::Modified))];
        assert_eq!(
            Action::ResolveConflict.enabled(&file, &ctx(&modified, "")),
            Err("This file has no conflict.")
        );
        assert_eq!(
            Action::ResolveConflict.enabled(&Args::File("gone.txt".into()), &ctx(&conflicted, "")),
            Err("That file is not in the working copy")
        );
        assert_eq!(
            Action::ResolveConflict.enabled(&Args::None, &ctx(&conflicted, "")),
            Err("No target for this action")
        );
        let closed = Context {
            repo_open: false,
            ..ctx(&conflicted, "")
        };
        assert_eq!(
            Action::ResolveConflict.enabled(&file, &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn resolve_conflict_reads_the_file_even_while_a_change_is_in_flight() {
        let file = Args::File("a.txt".into());
        let conflicted = [StatusEntry {
            conflicted: true,
            ..entry("a.txt", None, Some(Change::Modified))
        }];
        let busy = Context {
            busy: Some("Wait for the repository to open."),
            ..ctx(&conflicted, "")
        };
        assert_eq!(Action::ResolveConflict.enabled(&file, &busy), Ok(()));
        assert_eq!(
            Action::ResolveConflict.plan(&file, &ctx(&conflicted, "")),
            Ok(Effect::Send(vec![CommandKind::ReadConflict {
                path: "a.txt".into(),
                site: ConflictSite::WorkingCopy,
            }]))
        );
    }

    #[test]
    fn resolve_conflict_has_its_id_title_target_and_no_keys() {
        assert_eq!(Action::ResolveConflict.id(), "file.resolve_conflict");
        assert_eq!(Action::ResolveConflict.spec().title, "Resolve Conflict");
        assert_eq!(Action::ResolveConflict.spec().args, ArgKind::File);
        assert_eq!(
            Action::ResolveConflict.spec().category,
            Category::WorkingCopy
        );
        assert_eq!(Action::ResolveConflict.spec().shortcut, None);
        assert!(Action::ResolveConflict.available());
        assert!(!Action::ResolveConflict.destructive());
        assert!(!Action::ResolveConflict.opens_dialog());
    }
    #[test]
    fn the_thread_actions_have_their_ids_and_targets_and_no_shortcut() {
        let expected = [
            (Action::ThreadStar, "threads.star", ArgKind::Thread),
            (Action::ThreadRename, "threads.rename", ArgKind::Thread),
            (Action::ThreadNote, "threads.note", ArgKind::Thread),
            (Action::ThreadTags, "threads.tags", ArgKind::Thread),
            (
                Action::ThreadLinkCommit,
                "threads.link_commit",
                ArgKind::Thread,
            ),
            (
                Action::ThreadLinkThread,
                "threads.link_thread",
                ArgKind::Thread,
            ),
            (Action::ThreadExport, "threads.export", ArgKind::Thread),
            (
                Action::ThreadCopyMarkdown,
                "threads.copy_markdown",
                ArgKind::Thread,
            ),
            (Action::ThreadDelete, "threads.delete", ArgKind::Thread),
            (Action::ThreadScrub, "threads.scrub", ArgKind::None),
            (
                Action::ThreadExpandAll,
                "threads.expand_all",
                ArgKind::Thread,
            ),
            (
                Action::ThreadCollapseAll,
                "threads.collapse_all",
                ArgKind::Thread,
            ),
            (
                Action::ShowThreadsForFile,
                "file.show_threads",
                ArgKind::File,
            ),
        ];
        for (action, id, args) in expected {
            assert_eq!(action.id(), id);
            assert_eq!(action.spec().args, args, "{id}");
            assert_eq!(action.spec().shortcut, None, "{id}");
            assert!(action.available(), "{id}");
        }
        assert_eq!(
            Action::ShowThreadsForFile.spec().title,
            "Show Threads for File"
        );
        assert_eq!(
            Action::ShowThreadsForFile.spec().category,
            Category::WorkingCopy
        );
        assert_eq!(Action::ThreadDelete.spec().title, "Delete Thread…");
    }

    #[test]
    fn the_tool_call_folds_of_a_thread_plan_their_effect_and_need_a_thread() {
        let context = ctx(&[], "");
        let thread = thread_args(ThreadRef {
            uid: "u1",
            starred: false,
        });
        assert_eq!(
            Action::ThreadExpandAll.plan(&thread, &context),
            Ok(Effect::ThreadFolds { open: true })
        );
        assert_eq!(
            Action::ThreadCollapseAll.plan(&thread, &context),
            Ok(Effect::ThreadFolds { open: false })
        );
        assert_eq!(
            Action::ThreadCollapseAll.plan(&Args::None, &context),
            Err("Select a thread first.")
        );
    }

    #[test]
    fn a_thread_action_with_no_thread_selected_says_so_whatever_the_repository() {
        let context = ctx(&[], "");
        for action in [
            Action::ThreadStar,
            Action::ThreadRename,
            Action::ThreadNote,
            Action::ThreadTags,
            Action::ThreadLinkCommit,
            Action::ThreadLinkThread,
            Action::ThreadExport,
            Action::ThreadCopyMarkdown,
            Action::ThreadExpandAll,
            Action::ThreadCollapseAll,
            Action::ThreadDelete,
        ] {
            assert_eq!(
                action.enabled(&Args::None, &context),
                Err("Select a thread first."),
                "{action:?}"
            );
            assert_eq!(
                action.plan(&Args::None, &context),
                Err("Select a thread first.")
            );
        }
    }

    #[test]
    fn thread_actions_and_the_file_filter_need_an_open_repository() {
        let closed = Context {
            repo_open: false,
            ..ctx(&[], "")
        };
        let thread = thread_args(ThreadRef {
            uid: "u1",
            starred: false,
        });
        assert_eq!(
            Action::ThreadStar.enabled(&thread, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::ThreadScrub.enabled(&Args::None, &closed),
            Err("No repository is open.")
        );
        assert_eq!(
            Action::ShowThreadsForFile.enabled(&Args::File("a.txt".into()), &closed),
            Err("No repository is open.")
        );
    }

    #[test]
    fn a_star_sends_the_opposite_of_the_star_shown() {
        let context = ctx(&[], "");
        let unstarred = thread_args(ThreadRef {
            uid: "u1",
            starred: false,
        });
        let starred = thread_args(ThreadRef {
            uid: "u1",
            starred: true,
        });
        assert_eq!(
            Action::ThreadStar.plan(&unstarred, &context),
            Ok(Effect::Send(vec![CommandKind::UpdateThread {
                uid: "u1".into(),
                title: None,
                note: None,
                starred: Some(true),
            }]))
        );
        assert!(matches!(
            Action::ThreadStar.plan(&starred, &context),
            Ok(Effect::Send(kinds)) if matches!(
                kinds.as_slice(),
                [CommandKind::UpdateThread { starred: Some(false), .. }]
            )
        ));
    }

    #[test]
    fn each_field_action_opens_its_dialog_and_delete_its_confirmation() {
        let context = ctx(&[], "");
        let target = thread_args(ThreadRef {
            uid: "u1",
            starred: false,
        });
        let fields = [
            (Action::ThreadRename, ThreadField::Rename),
            (Action::ThreadNote, ThreadField::Note),
            (Action::ThreadTags, ThreadField::Tags),
            (Action::ThreadLinkCommit, ThreadField::LinkCommit),
            (Action::ThreadExport, ThreadField::Export),
        ];
        for (action, field) in fields {
            assert_eq!(
                action.plan(&target, &context),
                Ok(Effect::OpenThreadField {
                    field,
                    uid: "u1".into(),
                }),
                "{action:?}"
            );
        }
        assert_eq!(
            Action::ThreadDelete.plan(&target, &context),
            Ok(Effect::OpenDeleteThreadDialog("u1".into()))
        );
        assert_eq!(
            Action::ThreadScrub.plan(&Args::None, &context),
            Ok(Effect::OpenScrubDialog)
        );
        assert_eq!(
            Action::ShowThreadsForFile.plan(&Args::File("src/a.rs".into()), &context),
            Ok(Effect::ShowThreadsFor("src/a.rs".into()))
        );
    }

    #[test]
    fn link_to_thread_opens_the_picker_of_the_thread_and_is_not_a_field_dialog() {
        let context = ctx(&[], "");
        let target = thread_args(ThreadRef {
            uid: "u1",
            starred: false,
        });
        assert_eq!(
            Action::from_id("threads.link_thread"),
            Some(Action::ThreadLinkThread)
        );
        assert_eq!(Action::ThreadLinkThread.spec().title, "Link to Thread…");
        assert_eq!(Action::ThreadLinkThread.spec().shortcut, None);
        assert!(Action::ThreadLinkThread.opens_dialog());
        assert!(!Action::ThreadLinkThread.destructive());
        assert_eq!(
            Action::ThreadLinkThread.plan(&target, &context),
            Ok(Effect::OpenLinkThread("u1".into()))
        );
        assert_eq!(
            Action::ThreadLinkThread.plan(&Args::None, &context),
            Err(SELECT_THREAD)
        );
        assert!(Action::ALL.contains(&Action::ThreadLinkThread));
    }

    #[test]
    fn the_thread_dialogs_open_a_dialog_and_only_delete_is_destructive() {
        for action in [
            Action::ThreadRename,
            Action::ThreadNote,
            Action::ThreadTags,
            Action::ThreadLinkCommit,
            Action::ThreadLinkThread,
            Action::ThreadExport,
            Action::ThreadDelete,
            Action::ThreadScrub,
        ] {
            assert!(action.opens_dialog(), "{action:?}");
        }
        assert!(!Action::ThreadStar.opens_dialog());
        assert!(!Action::ShowThreadsForFile.opens_dialog());
        assert!(Action::ThreadDelete.destructive());
        for action in [
            Action::ThreadStar,
            Action::ThreadRename,
            Action::ThreadScrub,
        ] {
            assert!(!action.destructive(), "{action:?}");
        }
    }

    #[test]
    fn the_file_filter_needs_a_file_as_its_target() {
        let context = ctx(&[], "");
        assert_eq!(
            Action::ShowThreadsForFile.enabled(&Args::File("a.txt".into()), &context),
            Ok(())
        );
        assert_eq!(
            Action::ShowThreadsForFile.enabled(&Args::None, &context),
            Err("No target for this action")
        );
    }

    #[test]
    fn the_threads_settings_dialog_opens_without_a_repository_and_has_its_id_and_title() {
        let closed = ctx(&[], "");
        let action = Action::ThreadsSettings;
        assert_eq!(action.id(), "threads.settings");
        assert_eq!(action.spec().title, "Threads Settings…");
        assert_eq!(action.spec().shortcut, None);
        assert_eq!(action.spec().args, ArgKind::None);
        assert!(action.opens_dialog());
        assert_eq!(action.enabled(&Args::None, &closed), Ok(()));
        assert_eq!(Action::from_id("threads.settings"), Some(action));
        assert_eq!(
            action.plan(&Args::None, &closed),
            Ok(Effect::OpenThreadsSettings)
        );
    }

    fn stack_snapshot(state: gm_schema::StackState) -> Stacks {
        let entry = |name: &str, parent: Option<&str>, state| gm_schema::StackBranch {
            name: name.into(),
            parent: parent.map(str::to_owned),
            depth: u32::from(parent.is_some()),
            state,
            ahead: 0,
            behind: 0,
            restack: 0,
            commits: Vec::new(),
            base: None,
            parent_tip: None,
        };
        let mut stacks = Stacks::default();
        stacks.loaded(
            OperationId(1),
            gm_schema::StackSnapshot {
                stacks: vec![gm_schema::Stack {
                    branches: vec![
                        entry("main", None, gm_schema::StackState::Root),
                        entry("topic", Some("main"), state),
                    ],
                }],
                looped: Vec::new(),
            },
        );
        stacks
    }

    fn remote_entry(name: &str) -> BranchEntry {
        BranchEntry {
            name: name.into(),
            is_remote: true,
            is_current: false,
            upstream: None,
            author_email: None,
        }
    }

    #[test]
    fn new_child_branch_opens_the_child_dialog_for_a_local_branch_only() {
        let remotes = [remote_entry("origin/main")];
        let context = Context {
            features: FeatureSet::all(),
            undoable: None,
            stack: StackCtx {
                branches: &remotes,
                ..StackCtx::default()
            },
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::NewChildBranch.plan(&Args::Branch("main".into()), &context),
            Ok(Effect::OpenNewChildDialog {
                parent: "main".into()
            })
        );
        assert_eq!(
            Action::NewChildBranch.enabled(&Args::Branch("origin/main".into()), &context),
            Err("Stacks are made of local branches.")
        );
        assert!(Action::NewChildBranch.available());
        assert!(Action::NewChildBranch.opens_dialog());
    }

    #[test]
    fn restack_opens_its_dialog_for_a_branch_in_a_stack_and_refuses_others() {
        let stacks = stack_snapshot(gm_schema::StackState::Stale);
        let known = Context {
            features: FeatureSet::all(),
            undoable: None,
            stack: StackCtx {
                stacks: Some(&stacks),
                subject: None,
                branches: &[],
            },
            ..ctx(&[], "")
        };
        let main = Args::Branch("main".into());
        assert_eq!(
            Action::Restack.plan(&main, &known),
            Ok(Effect::OpenRestackDialog {
                branch: "main".into()
            })
        );
        assert_eq!(
            Action::Restack.enabled(&Args::Branch("lone".into()), &known),
            Err("This branch is in no stack.")
        );
        assert_eq!(
            Action::Restack.enabled(&main, &ctx(&[], "")),
            Err("The stacks are still being read.")
        );
        assert!(Action::Restack.opens_dialog());
    }

    #[test]
    fn set_parent_picks_the_parent_next_and_choose_parent_sends_the_write() {
        let stacks = stack_snapshot(gm_schema::StackState::Current);
        let context = ctx(&[], "");
        assert_eq!(
            Action::SetStackParent.plan(&Args::Branch("lone".into()), &context),
            Ok(Effect::PickStackParent {
                branch: "lone".into()
            })
        );
        assert_eq!(
            Action::ChooseStackParent.enabled(&Args::Branch("main".into()), &context),
            Err("Run Set Parent… on a branch first.")
        );
        let picking = |subject| Context {
            features: FeatureSet::all(),
            undoable: None,
            stack: StackCtx {
                stacks: Some(&stacks),
                subject: Some(subject),
                branches: &[],
            },
            ..ctx(&[], "")
        };
        assert_eq!(
            Action::ChooseStackParent.plan(&Args::Branch("main".into()), &picking("lone")),
            Ok(Effect::Send(vec![CommandKind::SetStackParent {
                branch: "lone".into(),
                parent: "main".into(),
            }]))
        );
        assert_eq!(
            Action::ChooseStackParent.enabled(&Args::Branch("lone".into()), &picking("lone")),
            Err("A branch cannot be its own parent.")
        );
        assert_eq!(
            Action::ChooseStackParent.enabled(&Args::Branch("main".into()), &picking("topic")),
            Err("This is already its parent.")
        );
        assert!(!Action::ChooseStackParent.listed_in_palette());
        assert!(Action::SetStackParent.listed_in_palette());
    }

    #[test]
    fn remove_and_reparent_follow_the_stack_state() {
        let current = stack_snapshot(gm_schema::StackState::Current);
        let merged = stack_snapshot(gm_schema::StackState::ParentMerged);
        let with = |stacks| Context {
            features: FeatureSet::all(),
            undoable: None,
            stack: StackCtx {
                stacks: Some(stacks),
                ..StackCtx::default()
            },
            ..ctx(&[], "")
        };
        let topic = Args::Branch("topic".into());
        let main = Args::Branch("main".into());
        assert_eq!(
            Action::RemoveFromStack.plan(&topic, &with(&current)),
            Ok(Effect::Send(vec![CommandKind::ClearStackParent {
                branch: "topic".into()
            }]))
        );
        assert_eq!(
            Action::RemoveFromStack.enabled(&main, &with(&current)),
            Err("This branch has no parent.")
        );
        assert_eq!(
            Action::ReparentToGrandparent.enabled(&topic, &with(&current)),
            Err("The parent is neither gone nor merged.")
        );
        assert_eq!(
            Action::ReparentToGrandparent.plan(&topic, &with(&merged)),
            Ok(Effect::Send(vec![CommandKind::ReparentToGrandparent {
                branch: "topic".into()
            }]))
        );
        assert_eq!(
            Action::ReparentToGrandparent.enabled(&main, &with(&merged)),
            Err("This branch has no parent.")
        );
        assert!(Action::ReparentToGrandparent.asks_first());
        assert!(!Action::ReparentToGrandparent.destructive());
    }

    #[test]
    fn convert_is_offered_only_while_a_prepare_commit_section_exists() {
        assert!(!convert_offered(&[]));
        let listed = ["local gitmaster.prepare-commit.team".to_owned()];
        assert!(convert_offered(&listed));
        assert_eq!(
            Action::ConvertHooks.enabled(&Args::None, &with_messages(MessageCtx::default())),
            Err("No prepare-commit section is in your Git config.")
        );
        let offered = MessageCtx {
            sections: &listed,
            ..MessageCtx::default()
        };
        assert_eq!(
            Action::ConvertHooks.plan(&Args::None, &with_messages(offered)),
            Ok(Effect::Send(vec![CommandKind::ConvertHooksToWorkflows]))
        );
    }

    #[test]
    fn a_preview_action_belongs_to_its_feature_and_the_rest_to_none() {
        assert_eq!(Action::Restack.feature(), Some(Feature::Stacks));
        assert_eq!(Action::NewChildBranch.feature(), Some(Feature::Stacks));
        assert_eq!(
            Action::ResolveConflict.feature(),
            Some(Feature::ConflictResolver)
        );
        assert_eq!(Action::Commit.feature(), None);
        assert_eq!(Action::Fetch.feature(), None);
        let preview = Action::ALL
            .iter()
            .filter(|action| action.feature().is_some())
            .count();
        assert_eq!(preview, 7);
    }

    #[test]
    fn with_no_preview_feature_on_the_palette_and_menus_leave_the_preview_actions_out() {
        let off = Context {
            features: FeatureSet::none(),
            ..ctx(&[], "")
        };
        let offered: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|action| action.listed_in_palette() && action.offered(off.features))
            .collect();
        assert!(offered.iter().all(|action| action.feature().is_none()));
        assert!(!offered.contains(&Action::Restack));
        assert!(!offered.contains(&Action::ResolveConflict));
        assert!(offered.contains(&Action::Commit));
        assert_eq!(
            Action::Restack.enabled(&Args::Branch("main".into()), &off),
            Err(PREVIEW_OFF)
        );
        assert_eq!(
            Action::ResolveConflict.enabled(&Args::File("a.txt".into()), &off),
            Err(PREVIEW_OFF)
        );
        assert!(matches!(
            Action::Restack.plan(&Args::Branch("main".into()), &off),
            Err(PREVIEW_OFF)
        ));
    }

    #[test]
    fn each_preview_feature_turns_on_only_its_own_actions() {
        let (stacks_only, _) = gm_schema::features::resolve(
            false,
            [("GITMASTER_ENABLE_STACKS".to_owned(), "1".to_owned())],
        );
        assert!(Action::Restack.offered(stacks_only));
        assert!(!Action::ResolveConflict.offered(stacks_only));
        let (resolver_only, _) = gm_schema::features::resolve(
            false,
            [(
                "GITMASTER_ENABLE_CONFLICT_RESOLVER".to_owned(),
                "1".to_owned(),
            )],
        );
        assert!(!Action::Restack.offered(resolver_only));
        assert!(Action::ResolveConflict.offered(resolver_only));
    }

    #[test]
    fn with_every_preview_feature_on_the_actions_are_offered_as_before() {
        let on = ctx(&[], "");
        assert!(
            Action::ALL
                .into_iter()
                .all(|action| action.offered(on.features))
        );
        assert_eq!(
            Action::Restack.enabled(&Args::Branch("main".into()), &on),
            Err("The stacks are still being read.")
        );
    }

    fn submodule_row(path: &str) -> StatusEntry {
        StatusEntry {
            submodule: Some(gm_schema::SubmoduleChange::default()),
            ..entry(path, None, Some(Change::Modified))
        }
    }

    /// The submodules of a status listing `path`, with its read answered: checked out or not.
    fn read_submodules(path: &str, checked_out: bool) -> Submodules {
        let mut submodules = Submodules::default();
        submodules.status_changed(&[submodule_row(path)]);
        submodules.request(path.to_owned(), OperationId(1));
        submodules.loaded(
            OperationId(1),
            gm_schema::SubmoduleDetail {
                path: path.to_owned(),
                branch: checked_out.then(|| "main".to_owned()),
                detached: false,
                head: None,
                recorded: None,
                checked_out,
                commits: Vec::new(),
                more: false,
                missing_commits: 0,
                repo: None,
            },
        );
        submodules
    }

    fn with_submodules<'a>(entries: &'a [StatusEntry], submodules: &'a Submodules) -> Context<'a> {
        Context {
            submodules: Some(submodules),
            ..ctx(entries, "")
        }
    }

    #[test]
    fn open_submodule_needs_a_checked_out_submodule_row() {
        let entries = [
            submodule_row("libs/x"),
            entry("a.rs", None, Some(Change::Modified)),
        ];
        let out = read_submodules("libs/x", true);
        let closed = read_submodules("libs/x", false);
        let unread = Submodules::default();
        let args = Args::File("libs/x".into());
        assert_eq!(
            Action::OpenSubmodule.enabled(&args, &with_submodules(&entries, &out)),
            Ok(())
        );
        assert_eq!(
            Action::OpenSubmodule.enabled(&args, &with_submodules(&entries, &closed)),
            Err("This submodule is not checked out.")
        );
        assert_eq!(
            Action::OpenSubmodule.enabled(&args, &with_submodules(&entries, &unread)),
            Err("The submodule is still being read.")
        );
        assert_eq!(
            Action::OpenSubmodule
                .enabled(&Args::File("a.rs".into()), &with_submodules(&entries, &out)),
            Err("Only a submodule opens in its own window.")
        );
        assert_eq!(
            Action::OpenSubmodule.enabled(&Args::None, &with_submodules(&entries, &out)),
            Err("No target for this action")
        );
        let closed_repo = Context {
            repo_open: false,
            ..with_submodules(&entries, &out)
        };
        assert_eq!(
            Action::OpenSubmodule.enabled(&args, &closed_repo),
            Err("No repository is open.")
        );
    }

    #[test]
    fn open_submodule_plans_the_folder_in_its_own_window() {
        let entries = [submodule_row("libs/x")];
        let out = read_submodules("libs/x", true);
        assert_eq!(
            Action::OpenSubmodule.plan(
                &Args::File("libs/x".into()),
                &with_submodules(&entries, &out)
            ),
            Ok(Effect::OpenWorktree("/repo/libs/x".to_owned()))
        );
        assert_eq!(Action::OpenSubmodule.id(), "submodule.open");
        assert_eq!(Action::OpenSubmodule.spec().title, "Open Submodule");
    }

    #[test]
    fn a_submodule_row_refuses_discard_view_move_and_hunks() {
        let entries = [submodule_row("libs/x")];
        let out = read_submodules("libs/x", true);
        let context = with_submodules(&entries, &out);
        let args = Args::File("libs/x".into());
        for action in [Action::DiscardFile, Action::DiscardAllChanges] {
            assert_eq!(
                action.enabled(&args, &context),
                Err("Open the submodule to discard."),
                "{action:?}"
            );
        }
        assert_eq!(
            Action::ViewFile.enabled(&args, &context),
            Err("Open the submodule instead.")
        );
        for action in [Action::MoveFileToBranch, Action::MoveFileToWorktree] {
            assert_eq!(
                action.enabled(&args, &context),
                Err("A submodule cannot be moved."),
                "{action:?}"
            );
        }
        let chunk = Args::Chunk {
            path: "libs/x".into(),
            hunk: HunkId("h1".into()),
            side: DiffSide::Unstaged,
        };
        assert_eq!(
            Action::DiscardChunk.enabled(&chunk, &context),
            Err("A submodule has no diff hunks.")
        );
    }

    #[test]
    fn record_combination_needs_a_member_with_a_new_commit() {
        let moved = StatusEntry {
            submodule: Some(gm_schema::SubmoduleChange {
                commit_changed: true,
                ..gm_schema::SubmoduleChange::default()
            }),
            ..entry("billing", None, Some(Change::Modified))
        };
        let same = submodule_row("checkout");
        let plain = entry("a.rs", None, Some(Change::Modified));
        let moved_entries = [same.clone(), moved];
        let same_entries = [same, plain];

        assert_eq!(
            Action::RecordCombination.enabled(&Args::None, &ctx(&moved_entries, "")),
            Ok(())
        );
        assert_eq!(
            Action::RecordCombination.enabled(&Args::None, &ctx(&same_entries, "")),
            Err(NO_NEW_COMMIT)
        );
        assert_eq!(
            Action::RecordCombination.enabled(&Args::None, &ctx(&[], "")),
            Err(NO_NEW_COMMIT)
        );
        assert!(NO_NEW_COMMIT.chars().count() < 30);
    }

    #[test]
    fn record_combination_is_in_the_palette_as_a_working_copy_action() {
        assert_eq!(Action::RecordCombination.id(), "combination.record");
        assert_eq!(Action::RecordCombination.spec().title, "Record Combination");
        assert_eq!(
            Action::RecordCombination.spec().category,
            Category::WorkingCopy
        );
        assert!(Action::RecordCombination.listed_in_palette());
        assert!(!Action::RecordCombination.destructive());
        assert_eq!(
            Action::RecordCombination.plan(&Args::None, &ctx(&[], "")),
            Err(NO_NEW_COMMIT)
        );
        let moved = StatusEntry {
            submodule: Some(gm_schema::SubmoduleChange {
                commit_changed: true,
                ..gm_schema::SubmoduleChange::default()
            }),
            ..entry("billing", None, Some(Change::Modified))
        };
        let entries = [moved];
        assert_eq!(
            Action::RecordCombination.plan(&Args::None, &ctx(&entries, "")),
            Ok(Effect::Send(vec![CommandKind::RecordCombination]))
        );
    }

    #[test]
    fn a_submodule_row_still_stages_and_a_file_keeps_its_actions() {
        let entries = [
            submodule_row("libs/x"),
            entry("a.rs", None, Some(Change::Modified)),
        ];
        let out = read_submodules("libs/x", true);
        let context = with_submodules(&entries, &out);
        assert_eq!(
            Action::StageFile.enabled(&Args::File("libs/x".into()), &context),
            Ok(())
        );
        let file = Args::File("a.rs".into());
        assert_eq!(Action::ViewFile.enabled(&file, &context), Ok(()));
        assert_eq!(Action::DiscardFile.enabled(&file, &context), Ok(()));
        // A move that names only the file opens the target step, as it does for any file.
        assert_eq!(Action::MoveFileToBranch.enabled(&file, &context), Ok(()));
    }

    fn workspace_listed(name: &str, path: &str) -> gm_schema::WorkspaceRow {
        gm_schema::WorkspaceRow {
            name: name.into(),
            path: path.into(),
            members: 2,
        }
    }

    /// The submodules of a status listing `path` with its read answered, as a checked-out member,
    /// detached or on its branch, or as a folder that is not checked out.
    fn member_read(path: &str, checked_out: bool, detached: bool) -> Submodules {
        let mut submodules = Submodules::default();
        submodules.status_changed(&[submodule_row(path)]);
        submodules.request(path.to_owned(), OperationId(1));
        submodules.loaded(
            OperationId(1),
            gm_schema::SubmoduleDetail {
                path: path.to_owned(),
                branch: (checked_out && !detached).then(|| "feat".to_owned()),
                detached,
                head: None,
                recorded: None,
                checked_out,
                commits: Vec::new(),
                more: false,
                missing_commits: 0,
                repo: None,
            },
        );
        submodules
    }

    #[test]
    fn new_workspace_needs_no_repository_and_opens_the_dialog() {
        let closed = Context {
            repo_open: false,
            root: None,
            ..ctx(&[], "")
        };
        assert_eq!(Action::NewWorkspace.enabled(&Args::None, &closed), Ok(()));
        assert_eq!(
            Action::NewWorkspace.plan(&Args::None, &closed),
            Ok(Effect::OpenWorkspaceDialog)
        );
        assert_eq!(Action::NewWorkspace.spec().title, "New Workspace…");
        assert_eq!(Action::NewWorkspace.spec().category, Category::Workspaces);
        assert!(Action::NewWorkspace.opens_dialog());
    }

    #[test]
    fn open_and_remove_refuse_the_workspace_this_window_shows_with_a_short_reason() {
        let rows = [
            workspace_listed("billing", "/w/billing"),
            workspace_listed("checkout", "/w/checkout"),
        ];
        let context = Context {
            workspaces: &rows,
            root: Some("/w/checkout"),
            ..ctx(&[], "")
        };
        let here = Args::Workspace("/w/checkout".into());
        let other = Args::Workspace("/w/billing".into());
        assert_eq!(
            Action::OpenWorkspace.enabled(&here, &context),
            Err("Open in this window.")
        );
        assert_eq!(Action::RevealWorkspace.enabled(&here, &context), Ok(()));
        assert_eq!(Action::RemoveWorkspace.enabled(&here, &context), Ok(()));
        assert_eq!(Action::RemoveWorkspace.enabled(&other, &context), Ok(()));
        assert_eq!(
            Action::RemoveWorkspace.plan(&here, &context),
            Ok(Effect::OpenRemoveWorkspaceDialog("/w/checkout".into()))
        );
        assert_eq!(
            Action::RemoveWorkspace.plan(&other, &context),
            Ok(Effect::OpenRemoveWorkspaceDialog("/w/billing".into()))
        );
        assert_eq!(
            Action::OpenWorkspace.plan(&other, &context),
            Ok(Effect::OpenWorktree("/w/billing".into()))
        );
        assert_eq!(
            Action::RevealWorkspace.plan(&other, &context),
            Ok(Effect::Reveal(PathBuf::from("/w/billing")))
        );
        assert!("Open in this window.".chars().count() < 30);
        assert_eq!(
            Action::RemoveWorkspace.spec().category,
            Category::Workspaces
        );
    }

    #[test]
    fn a_workspace_that_is_not_listed_is_refused() {
        assert_eq!(
            Action::OpenWorkspace.enabled(&Args::Workspace("/w/gone".into()), &ctx(&[], "")),
            Err("This workspace is not listed.")
        );
    }

    #[test]
    fn check_out_branch_is_enabled_only_for_a_detached_member() {
        let entries = [
            submodule_row("billing"),
            entry("a.rs", None, Some(Change::Modified)),
        ];
        let args = Args::File("billing".into());
        let detached = member_read("billing", true, true);
        let on_branch = member_read("billing", true, false);
        let closed = member_read("billing", false, false);
        let unread = Submodules::default();
        assert_eq!(
            Action::CheckOutMemberBranch.enabled(&args, &with_submodules(&entries, &detached)),
            Ok(())
        );
        assert_eq!(
            Action::CheckOutMemberBranch.enabled(&args, &with_submodules(&entries, &on_branch)),
            Err("It is on its branch.")
        );
        assert_eq!(
            Action::CheckOutMemberBranch.enabled(&args, &with_submodules(&entries, &closed)),
            Err("This submodule is not checked out.")
        );
        assert_eq!(
            Action::CheckOutMemberBranch.enabled(&args, &with_submodules(&entries, &unread)),
            Err("The submodule is still being read.")
        );
        assert_eq!(
            Action::CheckOutMemberBranch.enabled(
                &Args::File("a.rs".into()),
                &with_submodules(&entries, &detached)
            ),
            Err("Not a workspace member.")
        );
        assert_eq!(
            Action::CheckOutMemberBranch.plan(&args, &with_submodules(&entries, &detached)),
            Ok(Effect::Send(vec![CommandKind::CheckOutMemberBranch {
                path: "billing".into()
            }]))
        );
        assert_eq!(
            Action::CheckOutMemberBranch.spec().title,
            "Check Out Branch"
        );
        assert_eq!(
            Action::CheckOutMemberBranch.spec().category,
            Category::WorkingCopy
        );
    }
}
