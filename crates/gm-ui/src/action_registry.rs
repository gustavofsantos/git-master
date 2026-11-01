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
    BranchEntry, CommandKind, DiffSide, HunkId, OperationId, ReviewComment, ReviewStatus,
    StageTarget, StashEntry, StatusEntry, WorkflowInfo, WorkflowInput, WorktreeEntry,
};

use crate::move_file::{DropTarget, FOLDER_REASON, MISSING_FOLDER, SAME_BRANCH, SAME_WORKTREE};
use crate::stacks::Stacks;
use crate::stashes;
use crate::thread_curate::{self, ThreadField};
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
    /// A stash of the Stashes page's list.
    Stash,
    /// A review comment of the Working Copy diff, by its id (B4c).
    ReviewComment,
    /// The thread the Threads page has selected, or the thread a row names (T6).
    Thread,
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
    /// Opens the Reflog page and asks the engine for the first page of HEAD's reflog (B3).
    ShowReflog,
    /// Opens the Threads page and asks the engine for the first page of threads (T5).
    ShowThreads,
    /// Opens the Local review page and asks the engine for the review comments (B4d).
    ShowLocalReview,
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
    /// Writes the open workflow's buffer to its file.
    SaveWorkflow,
    /// Drops the open workflow's unsaved changes.
    RevertWorkflow,
    /// Opens the confirmation that moves the open workflow's file to the trash.
    OpenDeleteWorkflow,
    /// Asks the engine for a dry run of the open workflow's buffer.
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
    pub hooks: HookCtx<'a>,
    pub workflows: WorkflowCtx<'a>,
    /// The worktrees listed for the open repository, main checkout first.
    pub worktrees: &'a [WorktreeEntry],
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
    /// The branch stacks and the branch whose parent is being chosen (D138).
    pub stack: StackCtx<'a>,
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

/// The specs, in the order of `Action`'s variants (a test keeps them in step).
const SPECS: [Spec; 95] = [
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
    ShowAuthorPictures,
    HideAuthorPictures,
    CopyCommitHash,
    CopyCommitMessage,
    DeleteMergedBranches,
    QuickOpen,
    ShowCode,
    ViewFile,
    MoveFileToBranch,
    MoveFileToWorktree,
    SearchText,
    DiscardAllChanges,
    ShowStashes,
    NewStash,
    StashApply,
    StashPop,
    StashDrop,
    ShowReflog,
    ShowThreads,
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
    ThreadExport,
    ThreadCopyMarkdown,
    ThreadExpandAll,
    ThreadCollapseAll,
    ThreadDelete,
    ThreadScrub,
    ShowThreadsForFile,
    ThreadsSettings,
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
        (Action::ShowReflog, _) => Effect::ShowReflog,
        (Action::ShowThreads, _) => Effect::ShowThreads,
        (Action::ShowLocalReview, _) => Effect::ShowLocalReview,
        _ => Effect::ShowHistory,
    }
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

/// The reason of an action the engine cannot run yet, whatever its target.
pub(crate) const NOT_AVAILABLE: Reason = "Not available yet";

impl Action {
    pub const ALL: [Action; 95] = [
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
        Action::ShowAuthorPictures,
        Action::HideAuthorPictures,
        Action::CopyCommitHash,
        Action::CopyCommitMessage,
        Action::DeleteMergedBranches,
        Action::QuickOpen,
        Action::ShowCode,
        Action::ViewFile,
        Action::MoveFileToBranch,
        Action::MoveFileToWorktree,
        Action::SearchText,
        Action::DiscardAllChanges,
        Action::ShowStashes,
        Action::NewStash,
        Action::StashApply,
        Action::StashPop,
        Action::StashDrop,
        Action::ShowReflog,
        Action::ShowThreads,
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
        Action::ThreadExport,
        Action::ThreadCopyMarkdown,
        Action::ThreadExpandAll,
        Action::ThreadCollapseAll,
        Action::ThreadDelete,
        Action::ThreadScrub,
        Action::ShowThreadsForFile,
        Action::ThreadsSettings,
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
        !matches!(self, Action::RenameBranch)
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

    /// Whether the palette lists the action at its top level. Choose Parent is
    /// only the second step of Set Parent…, so it is reached from there.
    #[must_use]
    pub fn listed_in_palette(self) -> bool {
        self != Action::ChooseStackParent
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
                | Action::DiscardAllChanges
                | Action::StashDrop
                | Action::DeleteReviewComment
                | Action::ThreadDelete
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
                | Action::ShowReflog
                | Action::ShowThreads
                | Action::ShowLocalReview
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
                | Action::SaveWorkflow
                | Action::RevertWorkflow
                | Action::DeleteWorkflow
                | Action::DryRunWorkflow
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
                | Action::ThreadExport
                | Action::ThreadDelete
                | Action::ThreadScrub
                | Action::ThreadsSettings
        )
    }

    /// The action with this id, if any.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Whether the action can run now with `args`; the reason when it cannot.
    pub fn enabled(self, args: &Args, ctx: &Context<'_>) -> Result<(), Reason> {
        // A thread action run with no thread (the palette, with none selected) says so.
        if self.spec().args == ArgKind::Thread && matches!(args, Args::None) {
            return Err(SELECT_THREAD);
        }
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
            | Action::ShowWorkflows
            | Action::ShowReflog
            | Action::ShowThreads
            | Action::ShowLocalReview => repository_open(ctx),
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
            Action::RevealFile | Action::ViewFile => reveal_rule(args, ctx),
            Action::MoveFileToBranch | Action::MoveFileToWorktree => move_rule(self, args, ctx),
            Action::DiscardFile | Action::DiscardAllChanges => discard_rule(self, args, ctx),
            Action::ShowAuthorPictures if ctx.author_pictures == AuthorPictures::Shown => {
                Err("Author pictures are already shown.")
            }
            Action::HideAuthorPictures if ctx.author_pictures == AuthorPictures::Hidden => {
                Err("Author pictures are already hidden.")
            }
            Action::ShowAuthorPictures | Action::HideAuthorPictures => Ok(()),
            _ if self.copies_its_target() => Ok(()),
            Action::Fetch | Action::Pull | Action::PushWithLease => remote_rule(self, args, ctx),
            Action::SuggestMessage
            | Action::CancelHook
            | Action::UseSuggestion
            | Action::DiscardSuggestion
            | Action::ApproveHook
            | Action::DenyHook => hook_rule(self, ctx),
            _ if self.is_workflow_rule() => workflow_rule(self, args, ctx),
            Action::StashApply | Action::StashPop | Action::StashDrop => {
                stash_entry_rule(args, ctx)
            }
            _ if self.is_stash_rule() => stash_rule(self, ctx),
            Action::CommentOnSelection => comment_rule(ctx),
            Action::ResolveReviewComment
            | Action::ReopenReviewComment
            | Action::DeleteReviewComment
            | Action::EditReviewComment => review_entry_rule(self, args, ctx),
            Action::OpenReviewComment => open_target(args, ctx).map(|_| ()),
            Action::ResolveConflict => resolve_conflict_rule(args, ctx),
            Action::ThreadStar
            | Action::ThreadRename
            | Action::ThreadNote
            | Action::ThreadTags
            | Action::ThreadLinkCommit
            | Action::ThreadExport
            | Action::ThreadCopyMarkdown
            | Action::ThreadExpandAll
            | Action::ThreadCollapseAll
            | Action::ThreadDelete => thread_rule(args, ctx),
            Action::ThreadScrub | Action::ShowThreadsForFile => repository_open(ctx),
            // The settings dialog needs no repository.
            Action::ThreadsSettings => Ok(()),
            // The placeholders, refused above by `available`.
            _ => Err(NOT_AVAILABLE),
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
            (Action::StashApply | Action::StashPop | Action::StashDrop, _) => {
                stash_effect(self, args)?
            }
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
            (action, _) if action.is_thread_action() => thread_effect(action, args)?,
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
                | (ArgKind::Commit, Args::Commit(_))
                | (ArgKind::Workflow, Args::Workflow(_))
                | (ArgKind::Template, Args::Template(_))
                | (ArgKind::Stash, Args::Stash { .. })
                | (ArgKind::ReviewComment, Args::ReviewComment(_))
                | (ArgKind::Thread, Args::Thread { .. })
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
        (Action::SaveWorkflow, _) => Effect::SaveWorkflow,
        (Action::RevertWorkflow, _) => Effect::RevertWorkflow,
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
            selected_file: None,
            selected_thread: None,
            stack: crate::action_registry::StackCtx::default(),
            busy: None,
            entries,
            staged_count: entries.iter().filter(|e| e.staged.is_some()).count(),
            message,
            current_branch: Some("main"),
            root: Some("/repo"),
            hooks: crate::action_registry::HookCtx::default(),
            workflows: WorkflowCtx::default(),
            worktrees: &[],
            author_pictures: AuthorPictures::Hidden,
            remote_running: false,
            stashes: None,
            review: ReviewCtx::default(),
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
            stack: crate::action_registry::StackCtx::default(),
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
                Action::StashDrop,
                Action::DeleteReviewComment,
                Action::ThreadDelete,
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
            selected_file: None,
            selected_thread: None,
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
    fn show_stashes_needs_an_open_repository_and_plans_the_page() {
        assert_eq!(
            Action::ShowStashes.plan(&Args::None, &ctx(&[], "")),
            Ok(Effect::ShowStashes)
        );
        let closed = Context {
            repo_open: false,
            selected_file: None,
            selected_thread: None,
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
            stack: crate::action_registry::StackCtx::default(),
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
        assert_eq!(Action::SaveWorkflow.spec().shortcut, Some("⌘S"));
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
            summary: None,
            snapshot: None,
            after_snapshot: None,
            location: Some(gm_schema::ReviewLocation {
                start_line: 1,
                end_line: 1,
            }),
            outdated: false,
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
    fn the_thread_dialogs_open_a_dialog_and_only_delete_is_destructive() {
        for action in [
            Action::ThreadRename,
            Action::ThreadNote,
            Action::ThreadTags,
            Action::ThreadLinkCommit,
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
}
