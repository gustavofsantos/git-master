//! The window's state, with no GPUI in it: the status the engine reported, the
//! selected file, the diffs being loaded for it and the last message. The view
//! renders it and forwards keys to it.

use gm_schema::{
    CommitFile, CommitSummary, DiffSide, Event, FileDiff, History, OperationId, OperationState,
    RepoStatus, StatusEntry, WorktreeEntry,
};

use crate::branch_delete::Deleting;
use crate::branch_drag::BranchDrag;
use crate::branches::Branches;
use crate::changes::{Summary, summarize};
use crate::code_page::CodePage;
use crate::commit_threads::CommitThreads;
use crate::conflict_resolver::ResolverState;
use crate::file_discard::Discarding;
use crate::history_graph::{GraphLayout, StackMarks};
use crate::history_scope::ScopeChoice;
use crate::hook_run::Hooks;
use crate::icon::Icon;
use crate::local_review::LocalReview;
use crate::merged_branches::Merging;
use crate::move_file::MoveFile;
use crate::new_branch::BranchCreate;
use crate::new_stash::StashCreate;
use crate::quick_open::QuickOpen;
use crate::reflog::{DetachedCheckout, ReflogList};
use crate::remote_op::{Outcome, RemoteOp};
use crate::review::{ReviewList, ReviewWrite};
use crate::stacks::Stacks;
use crate::stash_apply::Applying;
use crate::stash_drop::Dropping;
use crate::stashes::StashList;
use crate::text_search::TextSearch;
use crate::thread_curate::{Outcome as ThreadOutcome, ThreadWork};
use crate::threads::{MarkdownCopy, ThreadsPage};
use crate::workflow_run::Workflows;
use crate::workflows_page::WorkflowsPage;
use crate::worktree_create::Creating;
use crate::worktree_prune::Pruning;
use crate::worktree_remove::Removal;

// Not `Eq`: the Workflows page holds a parsed definition, which is only `PartialEq`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    status: Option<RepoStatus>,
    /// Path of the selected file. Kept across status changes while it exists.
    selected: Option<String>,
    /// The two diffs being loaded for the selected file.
    diffs: Option<DiffSlots>,
    /// Why the last operation failed. Cleared by the next status.
    pub error: Option<String>,
    /// A transient line: a commit made, or a reason a commit did not start.
    pub notice: Option<String>,
    /// The icon drawn before the notice, if it has one.
    pub notice_icon: Option<Icon>,
    /// The folder the notice offers to open, as **Open Worktree** does (D109).
    pub notice_open: Option<String>,
    notice_generation: u64,
    pub loading: bool,
    /// The refresh the indicator follows, while it runs.
    refresh: Option<OperationId>,
    /// Set once `refresh` has run past the indicator delay.
    refresh_slow: bool,
    /// The last operation that reached a final state. Ids follow dispatch
    /// order, so every id at or below it has ended.
    settled: Option<OperationId>,
    /// The `OpenRepo` the window waits for. Its `StatusLoaded` replaces the
    /// repository and resets the view state that belonged to the old one.
    pending_open: Option<OperationId>,
    /// Events older than this belong to the repository being left and are dropped.
    floor: Option<OperationId>,
    /// Set by the open that just succeeded, until the view takes it.
    switched: Option<Switched>,
    /// The branch picker's listing, filter and checkout in flight.
    pub branches: Branches,
    /// The branch stacks the sidebar's By stack view draws, and the read in flight.
    pub stacks: Stacks,
    /// The scope of the History list chosen for this window, and the one on screen.
    pub(crate) history_scope: ScopeChoice,
    /// The branch dragged onto another to set its parent, while the drag runs.
    pub(crate) branch_drag: BranchDrag,
    /// The prepare-commit hooks: what is listed, the run and its suggestion.
    pub hooks: Hooks,
    /// The manual workflows: what is listed and the runs the engine reported.
    pub workflows: Workflows,
    /// The Workflows page: the open file, its buffer, its runs and its saves.
    pub workflows_page: WorkflowsPage,
    /// The worktrees of the open repository, main checkout first.
    pub worktrees: Vec<WorktreeEntry>,
    /// The fetch, pull or push with lease in flight, if any.
    pub remote: RemoteOp,
    /// The history page's listing: the request in flight, or what answered it.
    pub history: HistoryList,
    /// The commit detail panel: the read in flight, or what answered it.
    pub commit: CommitPane,
    /// The stacked diff of the open commit, which replaces the commit view (H4c).
    pub commit_stack: OpenStack,
    /// The Stashes page's detail panel: the read in flight, or the selected stash's answer (B2).
    pub stash_commit: CommitPane,
    /// The Stashes page's stacked diff of the selected stash (B2).
    pub stash_stack: OpenStack,
    /// The Stashes page's list: the read in flight, what answered it and the selection (B2).
    pub stashes: StashList,
    /// The Reflog page's list: the entries shown, the read in flight and the selection (B3).
    pub reflog: ReflogList,
    /// The Reflog page's detail panel: the read in flight, or the selected entry's answer (B3).
    pub reflog_commit: CommitPane,
    /// The Reflog page's stacked diff of the selected entry (B3).
    pub reflog_stack: OpenStack,
    /// The detached checkout started from the Reflog page, and its outcome (B3).
    pub reflog_checkout: DetachedCheckout,
    /// The Threads page: the list, the search, the selected thread and its entries (T5).
    pub threads: ThreadsPage,
    /// The threads linked to the commit the history page shows (H3).
    pub commit_threads: CommitThreads,
    /// The copy of a thread as Markdown in flight, and its outcome for the window (T6).
    pub thread_copy: MarkdownCopy,
    /// The write of a thread's curation in flight, and its outcome (T6).
    pub thread_work: ThreadWork,
    /// Quick Open: its query, the search in flight and the last answer (N1).
    pub quick_open: QuickOpen,
    /// The Code page: the folder tree and the file viewer (N2).
    pub code: CodePage,
    /// The text search page: the query, the options and the last answer (N3).
    pub text_search: TextSearch,
    /// Folders the Code page needs listed next; the window asks the engine.
    wanted_listings: Vec<String>,
    /// The engine finished reading the file list since the window last looked.
    files_indexed: bool,
    /// The create-worktree dialog's operation in flight, and its outcome.
    pub creating: Creating,
    /// The remove-worktree dialog's inspection, removal in flight and outcome.
    pub removal: Removal,
    /// The prune dialog's listing, prune in flight and outcome.
    pub pruning: Pruning,
    /// The new-branch dialog's create in flight, and its outcome.
    pub branch_create: BranchCreate,
    /// The delete-branch dialog's delete in flight, and its outcome.
    pub branch_delete: Deleting,
    /// The discard dialog's discard in flight, and its outcome.
    pub discarding: Discarding,
    /// The stash dialog's create in flight, and its outcome (B2).
    pub stash_create: StashCreate,
    /// The apply or pop of a stash in flight, and its outcome (B2).
    pub stash_apply: Applying,
    /// The drop dialog's drop in flight, and its outcome (B2).
    pub stash_drop: Dropping,
    /// The delete-merged-branches dialog's listing, delete in flight and outcome.
    pub merging: Merging,
    /// The review comments the engine listed, and the read in flight (B4c).
    pub review: ReviewList,
    /// The composer's comment write in flight, and its outcome (B4c).
    pub review_write: ReviewWrite,
    /// The Local review page: its filter, the selected comment and its texts (B4d).
    pub local_review: LocalReview,
    /// The conflict resolver of the file read last: its panes, choices and staged note (C3b).
    pub conflict: Option<ResolverState>,
    /// The read that opened the resolver, until the view takes it and shows the page.
    conflict_opened: Option<OperationId>,
    /// The file to read again after its recorded resolution was forgotten (D117).
    conflict_reread: Option<String>,
    /// Moving a changed file to a branch or a worktree: the drag, the hover check,
    /// the popover and the move in flight (D109).
    pub move_file: MoveFile,
}

/// The page a commit read belongs to: the history (H3, H4), the Stashes page
/// (B2) or the Reflog page (B3). Each keeps its own detail panel and stacked diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitPage {
    History,
    Stashes,
    Reflog,
}

/// The stacked diff of the open commit (H4c): every file's diff, shown in one
/// list. Only the answer to the request in flight is shown; an answer to a
/// superseded or closed request is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenStack {
    view: StackView,
    /// The diffs that arrived for the request in flight, until the view takes them.
    fresh: Option<Fresh>,
}

/// The answer to a stack request: the files it listed, how many more the detail
/// did not list, one diff per listed file, and the file the view starts at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fresh {
    pub files: Vec<CommitFile>,
    pub more: usize,
    pub diffs: Vec<FileDiff>,
    pub focus: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum StackView {
    /// No commit diff is open: the commit view is shown.
    #[default]
    Closed,
    Loading {
        id: OperationId,
        focus: usize,
        files: Vec<CommitFile>,
        more: usize,
    },
    Shown,
    Failed {
        message: String,
    },
}

impl OpenStack {
    /// Opens the stack of the commit's `files` (and `more` unlisted ones) as the
    /// request `id`, starting at file `focus`. Whatever was open before is replaced.
    pub fn request(&mut self, id: OperationId, focus: usize, files: Vec<CommitFile>, more: usize) {
        *self = Self {
            view: StackView::Loading {
                id,
                focus,
                files,
                more,
            },
            fresh: None,
        };
    }

    /// Back to the commit view; an answer still on its way is dropped.
    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// Whether the stack is open, whether read or still loading.
    pub fn is_open(&self) -> bool {
        !matches!(self.view, StackView::Closed)
    }

    pub fn is_shown(&self) -> bool {
        matches!(self.view, StackView::Shown)
    }

    /// git's message when the stack could not be read.
    pub fn failure(&self) -> Option<&str> {
        match &self.view {
            StackView::Failed { message } => Some(message),
            _ => None,
        }
    }

    /// The diffs that arrived since the last call, for the view to show once.
    pub fn take_fresh(&mut self) -> Option<Fresh> {
        self.fresh.take()
    }

    fn received(&mut self, id: OperationId, diffs: Vec<FileDiff>) {
        if let StackView::Loading {
            id: wanted,
            focus,
            files,
            more,
        } = std::mem::take(&mut self.view)
        {
            if wanted == id {
                self.view = StackView::Shown;
                self.fresh = Some(Fresh {
                    files,
                    more,
                    diffs,
                    focus,
                });
            } else {
                self.view = StackView::Loading {
                    id: wanted,
                    focus,
                    files,
                    more,
                };
            }
        }
    }

    fn failed(&mut self, id: OperationId, message: String) {
        if let StackView::Loading { id: wanted, .. } = self.view
            && wanted == id
        {
            self.view = StackView::Failed { message };
        }
    }

    /// Whether the request in flight is `id`.
    #[must_use]
    pub fn awaits(&self, id: OperationId) -> bool {
        matches!(self.view, StackView::Loading { id: wanted, .. } if wanted == id)
    }
}

/// The history page's listing. A reply to a request that was superseded is
/// dropped, so a page never lands in a list it was not read for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HistoryList {
    /// Nothing requested yet, or the page was closed by a repository switch.
    #[default]
    Idle,
    /// The first page could not be read; the reason is in the status bar.
    Failed,
    Shown(Listing),
}

/// The commit detail panel (H3). Only the answer to the request in flight is
/// shown; an answer to a superseded request is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CommitPane {
    /// No commit selected.
    #[default]
    Empty,
    /// The read of the selected commit, in flight.
    Loading(OperationId),
    Shown(gm_schema::CommitDetail),
    /// The read failed; the text is git's own message.
    Failed(String),
}

impl CommitPane {
    /// Starts showing the read `id`, which replaces whatever the panel held.
    pub fn request(&mut self, id: OperationId) {
        *self = Self::Loading(id);
    }

    /// Shows the answer when it is the read in flight.
    pub fn received(&mut self, id: OperationId, detail: gm_schema::CommitDetail) {
        if *self == Self::Loading(id) {
            *self = Self::Shown(detail);
        }
    }

    /// Shows the failure when it is the read in flight.
    pub fn failed(&mut self, id: OperationId, message: String) {
        if *self == Self::Loading(id) {
            *self = Self::Failed(message);
        }
    }

    /// Clears the panel when the read in flight was cancelled.
    pub fn cancelled(&mut self, id: OperationId) {
        if *self == Self::Loading(id) {
            *self = Self::Empty;
        }
    }

    /// Whether the read in flight is `id`.
    #[must_use]
    pub fn awaits(&self, id: OperationId) -> bool {
        *self == Self::Loading(id)
    }
}

/// What the history page holds, and the requests in flight for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The commits read so far, newest first.
    pub history: History,
    /// The graph lanes of `history.commits`: one row per commit, in step with
    /// them (H5).
    pub graph: GraphLayout,
    /// Whether the first page has been read. A refresh keeps the list until
    /// its answer comes.
    pub loaded: bool,
    /// The request for the first page, while it is in flight.
    pub top: Option<OperationId>,
    /// The request for the next page, while it is in flight.
    pub page: Option<OperationId>,
    /// A page failed, so no more pages are asked for until the list refreshes.
    pub page_failed: bool,
    /// The commit the user selected, by id. Kept across refreshes.
    pub selected: Option<String>,
}

impl HistoryList {
    /// Asks for the first page. A list already shown stays on screen until the
    /// answer replaces it, and any next-page request in flight is abandoned.
    pub fn request_top(&mut self, id: OperationId) {
        match self {
            Self::Shown(listing) => {
                listing.top = Some(id);
                listing.page = None;
                listing.page_failed = false;
            }
            _ => {
                *self = Self::Shown(Listing {
                    top: Some(id),
                    ..Listing::default()
                });
            }
        }
    }

    /// Whether a shown list may be refreshed now.
    #[must_use]
    pub fn can_refresh(&self) -> bool {
        matches!(self, Self::Shown(listing) if listing.loaded && listing.top.is_none())
    }

    /// Whether the next page should be asked for: the list is loaded, more
    /// commits follow, and nothing is in flight or has failed.
    #[must_use]
    pub fn can_page(&self) -> bool {
        matches!(
            self,
            Self::Shown(listing)
                if listing.loaded
                    && listing.history.more
                    && listing.top.is_none()
                    && listing.page.is_none()
                    && !listing.page_failed
        )
    }

    /// The oid of the last commit listed: the cursor the next page follows.
    #[must_use]
    pub fn last_oid(&self) -> Option<&str> {
        match self {
            Self::Shown(listing) => listing.history.commits.last().map(|c| c.oid.as_str()),
            _ => None,
        }
    }

    /// Lays the graph out again with the stack `marks` of a new snapshot (D139).
    pub fn remark(&mut self, marks: StackMarks) {
        if let Self::Shown(listing) = self {
            listing.graph.remark(marks, &listing.history.commits);
        }
    }

    /// Records the request for the next page.
    pub fn request_page(&mut self, id: OperationId) {
        if let Self::Shown(listing) = self {
            listing.page = Some(id);
        }
    }

    /// Applies an answer. A first page replaces the list, keeping the selection;
    /// a next page adds the commits the list does not hold yet. Anything else is dropped.
    pub fn received(&mut self, id: OperationId, history: History) {
        let Self::Shown(listing) = self else {
            return;
        };
        if listing.top == Some(id) {
            listing.top = None;
            listing.loaded = true;
            let marks = listing.graph.marks().clone();
            listing.graph = GraphLayout::with_marks(marks, &history.commits);
            listing.history = history;
        } else if listing.page == Some(id) {
            listing.page = None;
            let held: std::collections::BTreeSet<String> = listing
                .history
                .commits
                .iter()
                .map(|c| c.oid.clone())
                .collect();
            listing.history.more = history.more;
            let added: Vec<CommitSummary> = history
                .commits
                .into_iter()
                .filter(|commit| !held.contains(&commit.oid))
                .collect();
            listing.graph.extend(&added);
            listing.history.commits.extend(added);
        }
    }

    /// A request ended without an answer. A refresh that fails keeps the list
    /// it had; the first page failing shows the failure.
    pub fn failed(&mut self, id: OperationId) {
        let Self::Shown(listing) = self else {
            return;
        };
        if listing.top == Some(id) {
            listing.top = None;
            if !listing.loaded {
                *self = Self::Failed;
            }
        } else if listing.page == Some(id) {
            listing.page = None;
            listing.page_failed = true;
        }
    }

    /// Selects a commit by id.
    pub fn select(&mut self, oid: &str) {
        if let Self::Shown(listing) = self {
            listing.selected = Some(oid.to_owned());
        }
    }
}

/// A repository replaced by a successful open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switched {
    /// The root git resolved for the opened folder.
    pub root: String,
    /// Whether the previous repository had the same root.
    pub same_root: bool,
}

/// The two sides of the selected file, ready to show. `None` means the side
/// has no diff, or its load failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diffs {
    pub unstaged: Option<FileDiff>,
    pub staged: Option<FileDiff>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    /// Waiting for the `LoadDiff` operation with this id.
    Waiting(OperationId),
    Settled(Option<FileDiff>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffSlots {
    path: String,
    unstaged: Slot,
    staged: Slot,
}

impl DiffSlots {
    /// The side whose load is `id`, if one is still waiting for it.
    fn waiting_side(&self, id: OperationId) -> Option<DiffSide> {
        if self.unstaged == Slot::Waiting(id) {
            Some(DiffSide::Unstaged)
        } else if self.staged == Slot::Waiting(id) {
            Some(DiffSide::Staged)
        } else {
            None
        }
    }

    fn ready(&self) -> Option<Diffs> {
        match (&self.unstaged, &self.staged) {
            (Slot::Settled(unstaged), Slot::Settled(staged)) => Some(Diffs {
                unstaged: unstaged.clone(),
                staged: staged.clone(),
            }),
            _ => None,
        }
    }
}

impl Model {
    /// The detail panel of `page`.
    #[must_use]
    pub fn commit_pane(&self, page: CommitPage) -> &CommitPane {
        match page {
            CommitPage::History => &self.commit,
            CommitPage::Stashes => &self.stash_commit,
            CommitPage::Reflog => &self.reflog_commit,
        }
    }

    /// The stacked diff of `page`.
    #[must_use]
    pub fn commit_stack(&self, page: CommitPage) -> &OpenStack {
        match page {
            CommitPage::History => &self.commit_stack,
            CommitPage::Stashes => &self.stash_stack,
            CommitPage::Reflog => &self.reflog_stack,
        }
    }

    pub fn commit_stack_mut(&mut self, page: CommitPage) -> &mut OpenStack {
        match page {
            CommitPage::History => &mut self.commit_stack,
            CommitPage::Stashes => &mut self.stash_stack,
            CommitPage::Reflog => &mut self.reflog_stack,
        }
    }

    /// Folds one engine event into the state. Returns both diffs once the
    /// selected file's two loads have both finished (succeeded or failed).
    #[must_use]
    #[allow(clippy::too_many_lines)] // one arm per event; the create's arm adds one more
    pub fn apply(&mut self, event: Event) -> Option<Diffs> {
        if self
            .floor
            .is_some_and(|floor| event_id(&event).is_some_and(|id| id < floor))
        {
            return None;
        }
        if let Event::OperationChanged { id, state, .. } = &event
            && state.is_terminal()
        {
            self.end_refresh(*id);
            self.stacks.settled(*id);
        }
        match event {
            Event::StatusLoaded { id, status } => {
                // A successful checkout ends with its status; the repaint is the same as any other.
                let _ = self.branches.finish_checkout(id);
                let old_row = self.selected_row();
                if self.pending_open == Some(id) {
                    self.pending_open = None;
                    self.floor = Some(id);
                    let same_root = self
                        .status
                        .as_ref()
                        .is_some_and(|old| old.root == status.root);
                    self.switched = Some(Switched {
                        root: status.root.clone(),
                        same_root,
                    });
                    self.reset_for_repo();
                }
                self.status = Some(status);
                // Every answer about the old status may be stale now (D109).
                self.move_file.hover.status_changed();
                self.reconcile_selection(old_row);
                self.error = None;
                self.loading = false;
                None
            }
            // A worktree create's outcome belongs to its dialog, not to the status bar.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.creating.owns(id) => {
                self.creating.settle(state, detail);
                None
            }
            // A remove's inspection and outcome belong to its dialog, not to the status bar.
            Event::WorktreeInspected { id, inspection } if self.removal.owns(id) => {
                self.removal.inspected(inspection);
                None
            }
            Event::OperationChanged {
                id, state, detail, ..
            } if self.removal.owns(id) => {
                self.removal.settle(state, detail);
                None
            }
            // A branch create or delete belongs to its dialog, not to the status bar.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.branch_create.owns(id) => {
                self.branch_create.settle(state, detail);
                None
            }
            Event::OperationChanged {
                id, state, detail, ..
            } if self.branch_delete.owns(id) => {
                self.branch_delete.settle(state, detail);
                None
            }
            // A move's preview answers the hover check of its row (D109).
            Event::MovePreviewed { id, preview } => {
                let _ = self.move_file.answered(id, preview);
                None
            }
            // A finished move is the status bar's notice, with its Open Worktree offer.
            Event::ChangesMoved { id, moved } => {
                if let Some(notice) = self.move_file.moved(id, &moved) {
                    self.set_notice(notice.text);
                    self.notice_icon = Some(Icon::CheckCircle);
                    self.notice_open = notice.open_worktree;
                }
                None
            }
            // A preview that ended without an answer belongs to its row, not to the status bar.
            Event::OperationChanged { id, state, .. } if self.move_file.hover.awaits(id) => {
                self.move_file.hover.ended(id, state);
                None
            }
            // A move that failed stays in its popover, or else says why in the status bar.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.move_file.owns(id) => {
                if let Some(message) = self.move_file.settle(id, state, detail) {
                    self.error = Some(message);
                }
                None
            }
            // A file discard's outcome belongs to its dialog, not to the status bar.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.discarding.owns(id) => {
                self.discarding.settle(state, detail);
                None
            }
            // A stash create's outcome belongs to its dialog, not to the status bar (B2).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.stash_create.owns(id) => {
                self.stash_create.settle(state, detail);
                None
            }
            // An apply's or a pop's outcome belongs to its message, not to the status bar (B2).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.stash_apply.owns(id) => {
                self.stash_apply.settle(state, detail);
                None
            }
            // A drop's outcome belongs to its dialog (B2).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.stash_drop.owns(id) => {
                self.stash_drop.settle(state, detail);
                None
            }
            // A thread write's outcome belongs to its dialog, or to the status bar (T6).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.thread_work.owns(id) => {
                self.thread_work.settle(state, detail);
                None
            }
            // A detached checkout's outcome belongs to its message, not to the status bar (B3).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.reflog_checkout.owns(id) => {
                self.reflog_checkout.settle(state, detail);
                None
            }
            // A comment write's outcome belongs to the composer, not to the status bar (B4c).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.review_write.owns(id) => {
                self.review_write.settle(state, detail);
                None
            }
            // A failed comments read is the window's error, as any failed read is (B4c).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.review.owns(id) => {
                self.review.ended(id, state, detail);
                if let Some(message) = self.review.failure() {
                    self.error = Some(message.to_owned());
                }
                None
            }
            // A failed list read belongs to the Stashes page, which shows it there.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.stashes.owns(id) => {
                self.stashes.ended(id, state, detail);
                None
            }
            // A failed read of the Reflog page is shown on the page (B3).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.reflog.owns(id) => {
                self.reflog.ended(id, state, detail);
                None
            }
            // A failed read of the Threads page is shown on the page (T5).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.threads.owns(id) => {
                self.threads.ended(id, state, detail);
                None
            }
            // A commit's threads that could not be read are simply not shown (H3).
            Event::OperationChanged { id, state, .. }
                if self.commit_threads.owns(id) && state.is_terminal() =>
            {
                self.commit_threads.ended(id);
                None
            }
            // A copy of a thread as Markdown that failed says why, in the notice (T6).
            Event::OperationChanged {
                id, state, detail, ..
            } if self.thread_copy.owns(id)
                && state.is_terminal()
                && state != OperationState::Done =>
            {
                if matches!(state, OperationState::Failed | OperationState::Vetoed) {
                    self.thread_copy.failed(id, detail);
                } else {
                    self.thread_copy.cancelled(id);
                }
                None
            }
            Event::PrunableListed { id, candidates } if self.pruning.owns(id) => {
                self.pruning.listed(candidates);
                None
            }
            Event::MergedBranchesListed { id, base, branches } if self.merging.owns(id) => {
                self.merging.listed(base, branches);
                None
            }
            Event::MergedBranchesDeleted {
                id,
                deleted,
                failures,
            } if self.merging.owns(id) => {
                self.merging.reported(deleted.len(), failures);
                None
            }
            Event::OperationChanged {
                id, state, detail, ..
            } if self.merging.owns(id) => {
                self.merging.settle(state, detail);
                None
            }
            Event::OperationChanged {
                id, state, detail, ..
            } if self.pruning.owns(id) => {
                self.pruning.settle(state, detail);
                None
            }
            // A hook run's failure belongs to the composer's chip, not to the status bar.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.hooks.owns(id) => {
                self.hooks.ended(id, state, detail);
                None
            }
            // A remote operation's answer is the status bar's notice, or its error.
            Event::OperationChanged {
                id, state, detail, ..
            } if self.remote.owns(id) => {
                match self.remote.settle(id, state, detail) {
                    Some(Outcome::Done(message)) => {
                        self.set_notice(message);
                    }
                    Some(Outcome::Failed(message)) => self.error = Some(message),
                    None => {}
                }
                None
            }
            Event::OperationChanged {
                id,
                state: OperationState::Failed,
                detail,
                ..
            } => {
                let text = detail
                    .clone()
                    .unwrap_or_else(|| "git could not read the commit".to_owned());
                self.commit.failed(id, text.clone());
                self.reflog_commit.failed(id, text.clone());
                self.stash_commit.failed(id, text);
                let diff = detail
                    .clone()
                    .unwrap_or_else(|| "git could not read the commit's diff".to_owned());
                self.commit_stack.failed(id, diff.clone());
                self.reflog_stack.failed(id, diff.clone());
                self.stash_stack.failed(id, diff);
                // A refused checkout keeps the branch; git's reason, first line only, is the notice.
                if self.branches.finish_checkout(id) {
                    self.error = Some(checkout_message(detail.as_deref()));
                    return None;
                }
                let _ = self.branches.list_failed(id);
                if self.pending_open == Some(id) {
                    self.pending_open = None;
                }
                self.error = Some(detail.unwrap_or_else(|| "the operation failed".to_owned()));
                self.loading = false;
                self.settle(id, None, None)
            }
            Event::DiffLoaded { id, side, diff } => self.settle(id, Some(side), Some(diff)),
            Event::Committed { oid, .. } => {
                let short: String = oid.chars().take(7).collect();
                self.set_notice(format!("Committed {short}"));
                self.notice_icon = Some(Icon::CheckCircle);
                None
            }
            Event::BranchesLoaded { id, branches, .. } => {
                let _ = self.branches.listed(id, branches);
                None
            }
            Event::StacksLoaded { id, stacks } => {
                self.history.remark(StackMarks::from_snapshot(&stacks));
                self.stacks.loaded(id, stacks);
                None
            }
            Event::WorktreesLoaded { worktrees, .. } => {
                self.worktrees = worktrees;
                None
            }
            Event::HistoryLoaded { id, history } => {
                self.history.received(id, history);
                // A list made before the first snapshot has no marks yet.
                if let Some(snapshot) = self.stacks.snapshot() {
                    self.history.remark(StackMarks::from_snapshot(snapshot));
                }
                None
            }
            Event::CommitDetailLoaded { id, detail } => {
                if self.stash_commit.awaits(id) {
                    self.stash_commit.received(id, detail);
                } else if self.reflog_commit.awaits(id) {
                    self.reflog_commit.received(id, detail);
                } else {
                    self.commit.received(id, detail);
                }
                None
            }
            Event::CommitStackLoaded { id, diffs } => {
                if self.stash_stack.awaits(id) {
                    self.stash_stack.received(id, diffs);
                } else if self.reflog_stack.awaits(id) {
                    self.reflog_stack.received(id, diffs);
                } else {
                    self.commit_stack.received(id, diffs);
                }
                None
            }
            Event::ThreadsListed { id, threads, total } => {
                if self.commit_threads.owns(id) {
                    self.commit_threads.listed(id, threads);
                } else {
                    self.threads.listed(id, threads, total);
                }
                None
            }
            Event::ThreadSearchResults { id, hits } => {
                self.threads.searched(id, hits);
                None
            }
            Event::ThreadRead { id, detail } => {
                self.threads.read(id, detail);
                None
            }
            Event::ThreadEditRead { id, edit, state } => {
                self.threads.edit_read(id, edit.id, state);
                None
            }
            // The Markdown of a copy is kept for the window to put on the clipboard (T6).
            Event::ThreadMarkdown { id, markdown, .. } => {
                self.thread_copy.answered(id, markdown);
                None
            }
            // The payload of an export or a scrub ends its write (T6). The view asks for the list again on a change.
            Event::ThreadExported { id, path } => {
                self.thread_work.payload(id, ThreadOutcome::Exported(path));
                None
            }
            Event::ThreadsScrubbed { id, changed } => {
                self.thread_work.payload(id, ThreadOutcome::Scrubbed(changed));
                None
            }
            Event::StashesListed { id, stashes } => {
                self.list_stashes(id, stashes);
                None
            }
            Event::ReviewCommentsListed { id, comments } => {
                let _ = self.review.listed(id, comments);
                if let Some(listed) = self.review.comments() {
                    self.local_review.keep_listed(listed);
                }
                None
            }
            Event::ReviewSnapshotText {
                id, comment, lines, ..
            } => {
                self.local_review.received_before(id, comment, lines);
                None
            }
            Event::ReviewCurrentText {
                id, comment, lines, ..
            } => {
                self.local_review.received_after(id, comment, lines);
                None
            }
            Event::ReviewChange {
                id,
                comment,
                change,
                ..
            } => {
                self.local_review.received_change(id, comment, change);
                None
            }
            // The conflict resolver's view is not built yet (D114, slice C3).
            Event::ReflogLoaded {
                id,
                skip,
                entries,
                has_more,
            } => {
                self.list_reflog(id, skip, entries, has_more);
                None
            }
            Event::FilesFound { id, search } => {
                self.quick_open.received(id, search);
                None
            }
            Event::DirectoryListed { id, listing } => {
                let wanted = self.code.listed(id, listing);
                self.wanted_listings.extend(wanted);
                None
            }
            Event::FileRead { id, file } => {
                self.code.file_received(id, file);
                None
            }
            Event::FileHighlighted { id, spans } => {
                self.code.file_highlighted(id, spans);
                None
            }
            Event::TextFound { id, result } => {
                self.text_search.received(id, result);
                None
            }
            Event::FilesIndexed { .. } => {
                self.files_indexed = true;
                None
            }
            // An operation that ends without a status (vetoed, cancelled) ends its checkout too.
            Event::OperationChanged {
                id,
                state,
                detail,
                ..
            } => {
                self.workflows_page
                    .operation_ended(id, state, detail.as_deref());
                if matches!(
                    state,
                    OperationState::Done | OperationState::Vetoed | OperationState::Cancelled
                ) {
                    let _ = self.branches.finish_checkout(id);
                }
                if matches!(
                    state,
                    OperationState::Failed | OperationState::Vetoed | OperationState::Cancelled
                ) {
                    self.history.failed(id);
                    self.quick_open.failed(id);
                    self.code.listing_failed(id);
                }
                if state == OperationState::Failed {
                    self.text_search.failed(
                        id,
                        detail.as_deref().unwrap_or("the search could not be run"),
                    );
                    self.code
                        .file_failed(id, detail.as_deref().unwrap_or("the file could not be read"));
                }
                if state == OperationState::Cancelled {
                    self.commit.cancelled(id);
                    self.stash_commit.cancelled(id);
                    self.reflog_commit.cancelled(id);
                    self.code.file_cancelled(id);
                    self.text_search.cancelled(id);
                }
                None
            }
            Event::HooksListed { hooks, .. } => {
                self.hooks.listed(hooks);
                None
            }
            Event::HookStarted { id, hook, step, of } => {
                self.hooks.step_started(id, hook, step, of);
                None
            }
            Event::HookFinished {
                id,
                message,
                repo_changed,
            } => {
                self.hooks.answered(id, message, repo_changed);
                None
            }
            Event::WorkflowsListed {
                workflows,
                ignored,
                user_dir,
                templates,
                ..
            } => {
                self.workflows_page.listed(&workflows);
                self.workflows.listed(workflows, ignored, user_dir, templates);
                None
            }
            Event::WorkflowRunsListed { id, workflow, runs } => {
                self.workflows_page.runs_listed(id, &workflow, runs);
                None
            }
            Event::WorkflowRunLoaded { id, detail } => {
                self.workflows_page.run_loaded(id, *detail);
                None
            }
            Event::WorkflowDryRun { id, dry_run } => {
                self.workflows_page.dry_run_answered(id, *dry_run);
                None
            }
            Event::WorkflowChecked { id, diagnostics } => {
                self.workflows_page.checked(id, diagnostics);
                None
            }
            Event::WorkflowFileSaved { id, hash, .. } => {
                self.workflows_page.saved(id, &hash);
                None
            }
            Event::WorkflowFileDeleted { path, .. } => {
                self.workflows_page.deleted(&path);
                None
            }
            Event::WorkflowCreated { path, .. } => {
                self.workflows.created(path);
                None
            }
            Event::WorkflowReviewRequested {
                name,
                hash,
                source,
                inputs,
                ..
            } => {
                self.workflows.review_requested(name, hash, source, inputs);
                None
            }
            Event::WorkflowRunChanged {
                run,
                workflow,
                status,
                state,
            } => {
                self.workflows.run_changed(run, workflow, status, state);
                None
            }
            Event::WorkflowApprovalRequested {
                run,
                state,
                summary,
            } => {
                self.workflows.approval_requested(run, state, summary);
                None
            }
            Event::WorkflowNotice { run, text } => {
                self.workflows.notice(run, text);
                None
            }
            Event::WorkflowRunFinished { run, end, reason } => {
                self.workflows.finished(run, end, reason);
                None
            }
            // The failure reason arrives on the operation's own event; a cancel has no text.
            // An inspection nobody asked for is dropped; pruning's preview is shown by its
            // dialog (W6), not here yet.
            Event::HookFailed { .. }
            | Event::HookCancelled { .. }
            | Event::WorktreeInspected { .. }
            | Event::PrunableListed { .. }
            | Event::MergedBranchesListed { .. }
            | Event::MergedBranchesDeleted { .. }
            // A suggested message is the composer's to place; the window does that.
            | Event::WorkflowMessageSuggested { .. }
            // The list is read again by the view on a change, so the status bar has nothing to say.
            | Event::ThreadsChanged => None,
            // The read opens the resolver; the staged note is the resolver's own.
            Event::ConflictRead { id, read } => {
                // A read after a forget keeps the note that says so.
                let after_forget = self
                    .conflict
                    .as_ref()
                    .is_some_and(|conflict| conflict.needs_reread() && conflict.path() == read.path);
                self.conflict = Some(if after_forget {
                    ResolverState::reread_after_forget(&read)
                } else {
                    ResolverState::new(&read)
                });
                self.conflict_opened = Some(id);
                None
            }
            Event::ConflictResolved { .. } => {
                if let Some(conflict) = self.conflict.as_mut() {
                    conflict.on_event(&event);
                }
                None
            }
            Event::RecordedResolutionForgotten { .. } => {
                self.recorded_forgotten(&event);
                None
            }
        }
    }

    /// Folds a forgotten recorded resolution into the resolver. When it is the
    /// resolver's file, the file is asked for again (see `take_conflict_reread`).
    fn recorded_forgotten(&mut self, event: &Event) {
        if let Some(conflict) = self.conflict.as_mut()
            && conflict.on_event(event)
        {
            self.conflict_reread = Some(conflict.path().to_owned());
        }
    }

    /// Applies a stash listing. When the selected stash is gone, its detail and
    /// diff go with it, so the panel never shows a stash the list does not have.
    fn list_stashes(&mut self, id: OperationId, stashes: Vec<gm_schema::StashEntry>) {
        let had_selection = self.stashes.selected().is_some();
        if self.stashes.listed(id, stashes) && had_selection && self.stashes.selected().is_none() {
            self.stash_commit = CommitPane::Empty;
            self.stash_stack.close();
        }
    }

    /// Applies a reflog page. When the selected entry is gone after the first
    /// page, its detail and diff go with it, as a stash's do (B3).
    fn list_reflog(
        &mut self,
        id: OperationId,
        skip: u32,
        entries: Vec<gm_schema::ReflogEntry>,
        has_more: bool,
    ) {
        let had_selection = self.reflog.has_selection();
        if self.reflog.listed(id, skip, entries, has_more)
            && had_selection
            && !self.reflog.has_selection()
        {
            self.reflog_commit = CommitPane::Empty;
            self.reflog_stack.close();
        }
    }

    /// Waits for the `OpenRepo` `id`. Until it succeeds or fails, the current
    /// repository stays on screen.
    pub fn request_open(&mut self, id: OperationId) {
        self.pending_open = Some(id);
        self.loading = true;
    }

    /// Whether an open is in flight. Nothing may change the repository meanwhile.
    #[must_use]
    pub fn is_opening(&self) -> bool {
        self.pending_open.is_some()
    }

    /// The repository a successful open replaced, once. It is taken, so the
    /// caller acts on it only once.
    pub fn take_switched(&mut self) -> Option<Switched> {
        self.switched.take()
    }

    /// The read that opened the resolver since the view last asked, if one did. The
    /// view then shows the Conflicts page; it is taken, so it acts once.
    pub fn take_conflict_opened(&mut self) -> Option<OperationId> {
        self.conflict_opened.take()
    }

    /// The file whose recorded resolution was forgotten and must be read again, since
    /// the last ask. The view sends `ReadConflict` for it, once.
    pub fn take_conflict_reread(&mut self) -> Option<String> {
        self.conflict_reread.take()
    }

    /// Clears what belonged to the repository being left. The status stays
    /// until the new one replaces it.
    fn reset_for_repo(&mut self) {
        self.conflict = None;
        self.conflict_opened = None;
        self.conflict_reread = None;
        self.selected = None;
        self.diffs = None;
        self.notice = None;
        self.notice_icon = None;
        self.notice_open = None;
        self.move_file = MoveFile::default();
        self.history = HistoryList::default();
        self.commit = CommitPane::Empty;
        self.stacks.clear();
        self.history_scope.reset();
        self.stashes = StashList::default();
        self.stash_commit = CommitPane::Empty;
        self.stash_stack = OpenStack::default();
        self.review = ReviewList::default();
        self.local_review = LocalReview::default();
        self.reflog = ReflogList::default();
        self.reflog_commit = CommitPane::Empty;
        self.reflog_stack = OpenStack::default();
        self.threads = ThreadsPage::default();
        self.commit_threads = CommitThreads::default();
        self.thread_copy = MarkdownCopy::default();
        self.quick_open = QuickOpen::default();
        self.code.reset();
        self.text_search = TextSearch::default();
        self.wanted_listings.clear();
        self.files_indexed = false;
    }

    /// The folders the Code page needs listed, once: the window asks for each.
    pub fn take_wanted_listings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted_listings)
    }

    /// Whether the file list was read again since the last call: whoever shows
    /// a search or a folder asks the engine again.
    pub fn take_files_indexed(&mut self) -> bool {
        std::mem::take(&mut self.files_indexed)
    }

    /// Resolves the diff side waiting for `id`. A `DiffLoaded` must name the
    /// side it waits on; a failure names none.
    fn settle(
        &mut self,
        id: OperationId,
        side: Option<DiffSide>,
        diff: Option<FileDiff>,
    ) -> Option<Diffs> {
        let slots = self.diffs.as_mut()?;
        let waiting = slots.waiting_side(id)?;
        if side.is_some_and(|given| given != waiting) {
            return None;
        }
        let slot = match waiting {
            DiffSide::Unstaged => &mut slots.unstaged,
            DiffSide::Staged => &mut slots.staged,
        };
        *slot = Slot::Settled(diff);
        slots.ready()
    }

    /// Records that the diffs of `path` are being loaded by these operations.
    /// Replaces any earlier request; its replies are dropped from now on.
    pub fn expect_diffs(&mut self, path: String, unstaged: OperationId, staged: OperationId) {
        self.diffs = Some(DiffSlots {
            path,
            unstaged: Slot::Waiting(unstaged),
            staged: Slot::Waiting(staged),
        });
    }

    pub fn forget_diffs(&mut self) {
        self.diffs = None;
    }

    /// Whether the diffs shown or being loaded are those of `path`.
    #[must_use]
    pub fn diffs_are_for(&self, path: &str) -> bool {
        self.diffs.as_ref().is_some_and(|slots| slots.path == path)
    }

    /// Starts tracking the refresh `id`. False when it has already ended: the
    /// watcher's notice can arrive after the event that ends the refresh.
    pub fn begin_refresh(&mut self, id: OperationId) -> bool {
        if self.settled.is_some_and(|done| id <= done) {
            return false;
        }
        self.refresh = Some(id);
        self.refresh_slow = false;
        true
    }

    /// Marks the refresh `id` as slow, once the indicator delay has passed.
    /// True when it is still running, so the indicator should show.
    pub fn refresh_is_slow(&mut self, id: OperationId) -> bool {
        if self.refresh != Some(id) {
            return false;
        }
        self.refresh_slow = true;
        true
    }

    /// Whether "Updating…" shows: a refresh has run past the indicator delay.
    /// Quick refreshes never show it, so they do not flash.
    pub fn updating(&self) -> bool {
        self.refresh_slow
    }

    fn end_refresh(&mut self, id: OperationId) {
        self.settled = Some(self.settled.map_or(id, |done| done.max(id)));
        if self.refresh == Some(id) {
            self.refresh = None;
            self.refresh_slow = false;
        }
    }

    /// Shows `text` on the notice line. Returns a generation for `expire_notice`.
    pub fn set_notice(&mut self, text: String) -> u64 {
        self.notice = Some(text);
        self.notice_icon = None;
        self.notice_open = None;
        self.notice_generation += 1;
        self.notice_generation
    }

    /// Clears the notice if it is still the one `generation` named.
    pub fn expire_notice(&mut self, generation: u64) {
        if self.notice_generation == generation {
            self.notice = None;
            self.notice_icon = None;
            self.notice_open = None;
        }
    }

    pub fn notice_generation(&self) -> u64 {
        self.notice_generation
    }

    /// Selects the file on `row`. Returns whether the selected file changed.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.entries().get(row).map(|entry| entry.path.clone()) {
            Some(path) => self.choose(path),
            None => false,
        }
    }

    /// Moves the selection to the next (or previous) file. Returns whether the
    /// selected file changed.
    pub fn select_file_step(&mut self, down: bool) -> bool {
        let count = self.entry_count();
        let target = match (self.selected_row(), down) {
            (Some(row), true) => row.checked_add(1).filter(|&next| next < count),
            (Some(row), false) => row.checked_sub(1),
            (None, true) => (count > 0).then_some(0),
            (None, false) => count.checked_sub(1),
        };
        target.is_some_and(|row| self.select_row(row))
    }

    #[must_use]
    pub fn status(&self) -> Option<&RepoStatus> {
        self.status.as_ref()
    }

    /// The status entries, or none before the first status arrives.
    #[must_use]
    pub fn entries(&self) -> &[StatusEntry] {
        self.status
            .as_ref()
            .map_or(&[], |status| status.entries.as_slice())
    }

    #[must_use]
    pub fn summary(&self) -> Summary {
        summarize(self.entries())
    }

    #[must_use]
    pub fn selected_path(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The row of the selected file in the list.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        let path = self.selected.as_deref()?;
        self.entries().iter().position(|entry| entry.path == path)
    }

    /// The count of changed paths.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries().len()
    }

    /// Selects `path`. Returns whether the selected file changed.
    fn choose(&mut self, path: String) -> bool {
        let changed = self.selected.as_deref() != Some(path.as_str());
        self.selected = Some(path);
        changed
    }

    /// Keeps the selection by path while the file is listed. When it is gone,
    /// the file now at its old row is selected, or the last one.
    fn reconcile_selection(&mut self, old_row: Option<usize>) {
        if self.selected.is_none() || self.selected_row().is_some() {
            return;
        }
        let count = self.entry_count();
        let next = old_row.filter(|_| count > 0).map(|row| row.min(count - 1));
        self.selected = next.map(|row| self.entries()[row].path.clone());
    }
}

/// The notice for a refused checkout: git's first non-empty line.
fn checkout_message(detail: Option<&str>) -> String {
    detail
        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("the checkout failed")
        .to_owned()
}

/// The operation an event answers; `None` for an event of a workflow run, which
/// belongs to the run, not to an operation.
fn event_id(event: &Event) -> Option<OperationId> {
    match event {
        Event::StatusLoaded { id, .. }
        | Event::OperationChanged { id, .. }
        | Event::DiffLoaded { id, .. }
        | Event::Committed { id, .. }
        | Event::BranchesLoaded { id, .. }
        | Event::StacksLoaded { id, .. }
        | Event::HistoryLoaded { id, .. }
        | Event::CommitDetailLoaded { id, .. }
        | Event::CommitStackLoaded { id, .. }
        | Event::FilesFound { id, .. }
        | Event::DirectoryListed { id, .. }
        | Event::FileRead { id, .. }
        | Event::FileHighlighted { id, .. }
        | Event::TextFound { id, .. }
        | Event::FilesIndexed { id, .. }
        | Event::WorktreesLoaded { id, .. }
        | Event::WorktreeInspected { id, .. }
        | Event::MovePreviewed { id, .. }
        | Event::ChangesMoved { id, .. }
        | Event::PrunableListed { id, .. }
        | Event::MergedBranchesListed { id, .. }
        | Event::MergedBranchesDeleted { id, .. }
        | Event::StashesListed { id, .. }
        | Event::ReflogLoaded { id, .. }
        | Event::HooksListed { id, .. }
        | Event::HookStarted { id, .. }
        | Event::HookFinished { id, .. }
        | Event::HookFailed { id, .. }
        | Event::HookCancelled { id, .. }
        | Event::WorkflowsListed { id, .. }
        | Event::WorkflowCreated { id, .. }
        | Event::WorkflowReviewRequested { id, .. }
        | Event::WorkflowRunsListed { id, .. }
        | Event::WorkflowRunLoaded { id, .. }
        | Event::WorkflowDryRun { id, .. }
        | Event::WorkflowChecked { id, .. }
        | Event::WorkflowFileSaved { id, .. }
        | Event::WorkflowFileDeleted { id, .. }
        | Event::ReviewCommentsListed { id, .. }
        | Event::ReviewSnapshotText { id, .. }
        | Event::ReviewCurrentText { id, .. }
        | Event::ReviewChange { id, .. }
        | Event::ThreadsListed { id, .. }
        | Event::ThreadSearchResults { id, .. }
        | Event::ThreadRead { id, .. }
        | Event::ThreadEditRead { id, .. }
        | Event::ThreadMarkdown { id, .. }
        | Event::ThreadExported { id, .. }
        | Event::ThreadsScrubbed { id, .. }
        | Event::ConflictRead { id, .. }
        | Event::ConflictResolved { id, .. }
        | Event::RecordedResolutionForgotten { id, .. } => Some(*id),
        Event::ThreadsChanged
        | Event::WorkflowRunChanged { .. }
        | Event::WorkflowMessageSuggested { .. }
        | Event::WorkflowNotice { .. }
        | Event::WorkflowApprovalRequested { .. }
        | Event::WorkflowRunFinished { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Actor, BranchInfo, Change};

    fn entry(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged: None,
            unstaged: Some(Change::Modified),
            untracked: false,
            conflicted: false,
        }
    }

    fn status(paths: &[&str]) -> RepoStatus {
        RepoStatus {
            root: "/r".to_owned(),
            branch: BranchInfo::default(),
            entries: paths.iter().map(|p| entry(p)).collect(),
        }
    }

    fn loaded(paths: &[&str]) -> Event {
        Event::StatusLoaded {
            id: OperationId(1),
            status: status(paths),
        }
    }

    fn file_diff(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        }
    }

    fn diff_loaded(id: u64, side: DiffSide, path: &str) -> Event {
        Event::DiffLoaded {
            id: OperationId(id),
            side,
            diff: file_diff(path),
        }
    }

    fn failed(id: u64, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: detail.map(str::to_owned),
        }
    }

    /// A model with `paths` loaded and `path` selected, plus two pending diffs.
    fn selected(paths: &[&str], path: &str) -> Model {
        let mut model = Model::default();
        let _ = model.apply(loaded(paths));
        let row = model
            .entries()
            .iter()
            .position(|entry| entry.path == path)
            .expect("path is listed");
        let _ = model.select_row(row);
        model.expect_diffs(path.to_owned(), OperationId(10), OperationId(11));
        model
    }

    #[test]
    fn a_status_replaces_the_old_one_and_ends_loading() {
        let mut model = Model {
            loading: true,
            error: Some("old".into()),
            ..Model::default()
        };
        let _ = model.apply(loaded(&["a", "b", "c"]));
        assert_eq!(model.entry_count(), 3);
        assert!(!model.loading);
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_failed_hook_run_shows_on_the_chip_and_not_in_the_status_bar() {
        use crate::hook_run::Phase;

        let mut model = Model::default();
        model.hooks.start(OperationId(9), String::new());
        let _ = model.apply(failed(9, Some("hook llm exited with code 2")));
        assert_eq!(model.error, None);
        assert_eq!(
            model.hooks.run.as_ref().map(|run| run.phase.clone()),
            Some(Phase::Failed("hook llm exited with code 2".into()))
        );
    }

    #[test]
    fn a_failed_operation_sets_the_error_and_ends_loading() {
        let mut model = Model {
            loading: true,
            ..Model::default()
        };
        let _ = model.apply(failed(2, Some("not supported")));
        assert_eq!(model.error.as_deref(), Some("not supported"));
        assert!(!model.loading);
    }

    #[test]
    fn a_failed_operation_without_detail_still_says_so() {
        let mut model = Model::default();
        let _ = model.apply(failed(2, None));
        assert_eq!(model.error.as_deref(), Some("the operation failed"));
    }

    #[test]
    fn selection_stays_on_the_same_path_when_rows_move() {
        let mut model = selected(&["a/x", "b", "c"], "b");
        let _ = model.apply(loaded(&["z", "b", "c", "a/x"]));
        assert_eq!(model.selected_path(), Some("b"));
        assert_eq!(model.selected_row(), Some(1));
    }

    #[test]
    fn a_vanished_selection_moves_to_the_nearest_file() {
        let mut model = selected(&["a", "b", "c"], "b");
        let _ = model.apply(loaded(&["a", "c"]));
        assert_eq!(model.selected_path(), Some("c"));
        let _ = model.apply(loaded(&["a"]));
        assert_eq!(model.selected_path(), Some("a"));
        let _ = model.apply(loaded(&[]));
        assert_eq!(model.selected_path(), None);
        assert_eq!(model.selected_row(), None);
    }

    #[test]
    fn an_empty_list_has_no_selection() {
        let mut model = Model::default();
        assert!(!model.select_file_step(true));
        assert!(!model.select_file_step(false));
        assert_eq!(model.selected_path(), None);
    }

    #[test]
    fn steps_move_through_the_files_and_stop_at_both_ends() {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["dir/a", "dir/b"]));
        assert!(model.select_file_step(true));
        assert_eq!(model.selected_path(), Some("dir/a"));
        assert!(model.select_file_step(true));
        assert_eq!(model.selected_path(), Some("dir/b"));
        assert!(!model.select_file_step(true));
        assert!(model.select_file_step(false));
        assert_eq!(model.selected_path(), Some("dir/a"));
        assert!(!model.select_file_step(false));
    }

    #[test]
    fn both_diffs_are_returned_once_both_arrive() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        let ready = model.apply(diff_loaded(11, DiffSide::Staged, "a"));
        assert_eq!(
            ready,
            Some(Diffs {
                unstaged: Some(file_diff("a")),
                staged: Some(file_diff("a")),
            })
        );
        // A repeated reply for a settled side is ignored.
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
    }

    #[test]
    fn a_stale_diff_is_dropped() {
        let mut model = selected(&["a", "b"], "a");
        model.expect_diffs("b".into(), OperationId(20), OperationId(21));
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert_eq!(model.apply(diff_loaded(11, DiffSide::Staged, "a")), None);
        assert_eq!(model.apply(diff_loaded(20, DiffSide::Unstaged, "b")), None);
        assert!(
            model
                .apply(diff_loaded(21, DiffSide::Staged, "b"))
                .is_some()
        );
    }

    #[test]
    fn a_diff_on_the_wrong_side_is_dropped() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Staged, "a")), None);
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
    }

    #[test]
    fn one_side_failing_while_the_other_loads_still_shows_both() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(10, Some("not supported"))), None);
        assert_eq!(model.error.as_deref(), Some("not supported"));
        let ready = model.apply(diff_loaded(11, DiffSide::Staged, "a"));
        assert_eq!(
            ready,
            Some(Diffs {
                unstaged: None,
                staged: Some(file_diff("a")),
            })
        );
    }

    #[test]
    fn both_sides_failing_is_ready_with_nothing() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(11, None)), None);
        assert_eq!(model.apply(failed(10, None)), Some(Diffs::default()));
    }

    #[test]
    fn a_failure_of_an_unrelated_operation_settles_nothing() {
        let mut model = selected(&["a"], "a");
        assert_eq!(model.apply(failed(99, Some("stage failed"))), None);
        assert_eq!(model.error.as_deref(), Some("stage failed"));
        assert!(model.diffs_are_for("a"));
    }

    #[test]
    fn a_new_request_drops_the_old_ids() {
        let mut model = selected(&["a", "b"], "a");
        model.forget_diffs();
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert!(!model.diffs_are_for("b"));
    }

    #[test]
    fn a_commit_notice_shows_the_short_id_and_expires_by_generation() {
        let mut model = Model::default();
        let _ = model.apply(Event::Committed {
            id: OperationId(3),
            oid: "0123456789abcdef".into(),
        });
        assert_eq!(model.notice.as_deref(), Some("Committed 0123456"));
        let generation = model.notice_generation();
        let _ = model.set_notice("nothing staged".into());
        model.expire_notice(generation);
        assert_eq!(model.notice.as_deref(), Some("nothing staged"));
    }

    fn status_at(root: &str, paths: &[&str]) -> RepoStatus {
        RepoStatus {
            root: root.to_owned(),
            branch: BranchInfo::default(),
            entries: paths.iter().map(|p| entry(p)).collect(),
        }
    }

    /// A model showing repository `/r` with `a` selected and a notice up, with
    /// `a`'s diffs pending as ids 10 and 11.
    fn busy() -> Model {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["a", "dir/b"]));
        let _ = model.set_notice("nothing staged".into());
        let _ = model.select_row(0);
        model.expect_diffs("a".to_owned(), OperationId(10), OperationId(11));
        model
    }

    #[test]
    fn a_successful_open_resets_what_belonged_to_the_old_repo() {
        let mut model = busy();
        model.request_open(OperationId(20));
        assert!(model.is_opening());
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["x"]),
        });
        assert!(!model.is_opening());
        assert_eq!(model.selected_path(), None);
        assert!(!model.diffs_are_for("a"));
        assert_eq!(model.notice, None);
        assert_eq!(model.entries().len(), 1);
        assert_eq!(
            model.take_switched(),
            Some(Switched {
                root: "/b".into(),
                same_root: false,
            })
        );
        assert_eq!(model.take_switched(), None);
    }

    #[test]
    fn reopening_the_same_root_says_so() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/r", &["a"]),
        });
        assert_eq!(model.take_switched().map(|s| s.same_root), Some(true));
    }

    #[test]
    fn a_failed_open_keeps_the_current_repo_and_its_state() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(failed(20, Some("not a git repository")));
        assert!(!model.is_opening());
        assert_eq!(model.error.as_deref(), Some("not a git repository"));
        assert_eq!(model.status().map(|s| s.root.as_str()), Some("/r"));
        assert_eq!(model.selected_path(), Some("a"));
        assert!(model.diffs_are_for("a"));
        assert_eq!(model.take_switched(), None);
    }

    #[test]
    fn a_diff_requested_for_the_old_repo_is_dropped_after_the_switch() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["a"]),
        });
        // Both replies to the old repo's diff requests arrive late.
        assert_eq!(model.apply(diff_loaded(10, DiffSide::Unstaged, "a")), None);
        assert_eq!(model.apply(diff_loaded(11, DiffSide::Staged, "a")), None);
        assert!(!model.diffs_are_for("a"));
    }

    #[test]
    fn an_event_older_than_the_switch_changes_nothing() {
        let mut model = busy();
        model.request_open(OperationId(20));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(20),
            status: status_at("/b", &["x"]),
        });
        let _ = model.apply(failed(5, Some("stale failure")));
        assert_eq!(model.error, None);
    }

    fn branch(name: &str, current: bool) -> gm_schema::BranchEntry {
        gm_schema::BranchEntry {
            name: name.to_owned(),
            is_remote: false,
            is_current: current,
            upstream: None,
            author_email: None,
        }
    }

    #[test]
    fn a_refused_checkout_shows_git_first_line_and_keeps_the_branch() {
        let mut model = Model::default();
        let _ = model.apply(loaded(&["a"]));
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(failed(
            7,
            Some("error: local changes would be overwritten\n\tapi/routes.rs\nAborting"),
        ));
        assert_eq!(
            model.error.as_deref(),
            Some("error: local changes would be overwritten")
        );
        assert!(!model.branches.checkout_in_flight());
        assert_eq!(model.status().map(|s| s.root.as_str()), Some("/r"));
        assert!(!model.is_opening());
    }

    #[test]
    fn a_refused_checkout_without_detail_says_so() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(failed(7, Some("  \n")));
        assert_eq!(model.error.as_deref(), Some("the checkout failed"));
    }

    #[test]
    fn a_successful_checkout_ends_with_its_status() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(7),
            status: status_at("/r", &["a"]),
        });
        assert!(!model.branches.checkout_in_flight());
        assert_eq!(model.error, None);
    }

    #[test]
    fn the_worktree_list_is_replaced_by_each_listing() {
        let entry = |path: &str| WorktreeEntry {
            path: path.into(),
            head: None,
            branch: None,
            detached: true,
            bare: false,
            is_main: false,
            locked: None,
            prunable: None,
        };
        let mut model = Model::default();
        let _ = model.apply(Event::WorktreesLoaded {
            id: OperationId(1),
            worktrees: vec![entry("/a"), entry("/b")],
        });
        assert_eq!(model.worktrees.len(), 2);
        let _ = model.apply(Event::WorktreesLoaded {
            id: OperationId(2),
            worktrees: vec![entry("/a")],
        });
        assert_eq!(model.worktrees, vec![entry("/a")]);
    }

    #[test]
    fn a_vetoed_checkout_ends_too() {
        let mut model = Model::default();
        model.branches.begin_checkout(OperationId(7));
        let _ = model.apply(Event::OperationChanged {
            id: OperationId(7),
            actor: Actor::Human,
            state: OperationState::Vetoed,
            detail: None,
        });
        assert!(!model.branches.checkout_in_flight());
    }

    #[test]
    fn a_listing_reply_to_an_older_request_is_dropped() {
        let mut model = Model::default();
        model.branches.request(OperationId(2));
        let _ = model.apply(Event::BranchesLoaded {
            id: OperationId(1),
            branches: vec![branch("old", true)],
            user_email: None,
        });
        assert!(model.branches.lines().is_empty());
        let _ = model.apply(Event::BranchesLoaded {
            id: OperationId(2),
            branches: vec![branch("new", true)],
            user_email: None,
        });
        assert_eq!(
            model.branches.highlighted().map(|b| b.name.as_str()),
            Some("new")
        );
    }

    #[test]
    fn a_failed_listing_is_recorded_as_a_failed_listing() {
        let mut model = Model::default();
        model.branches.request(OperationId(2));
        let _ = model.apply(failed(2, Some("not a repository")));
        assert!(model.branches.is_failed());
        assert_eq!(model.error.as_deref(), Some("not a repository"));
    }

    fn done(id: u64) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }
    }

    #[test]
    fn a_refresh_does_not_set_loading() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(4)));
        assert!(!model.loading);
    }

    #[test]
    fn updating_shows_only_once_the_refresh_is_slow() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(9)));
        assert!(!model.updating(), "a fresh refresh does not show it");
        assert!(model.refresh_is_slow(OperationId(9)));
        assert!(model.updating());
        let _ = model.apply(done(9));
        assert!(!model.updating(), "it ends with the refresh");
    }

    #[test]
    fn a_quick_refresh_never_shows_updating() {
        let mut model = Model::default();
        assert!(model.begin_refresh(OperationId(9)));
        let _ = model.apply(done(9));
        assert!(
            !model.refresh_is_slow(OperationId(9)),
            "the timer finds it over"
        );
        assert!(!model.updating());
    }

    #[test]
    fn a_notice_that_arrives_after_its_refresh_ended_is_ignored() {
        let mut model = Model::default();
        let _ = model.apply(done(9));
        assert!(!model.begin_refresh(OperationId(9)));
        assert!(!model.begin_refresh(OperationId(8)));
        assert!(model.begin_refresh(OperationId(10)));
    }
}

#[cfg(test)]
mod history_list_tests {
    use super::*;

    fn commit(oid: &str) -> CommitSummary {
        CommitSummary {
            oid: oid.into(),
            parents: Vec::new(),
            short: oid.chars().take(7).collect(),
            subject: oid.into(),
            refs: Vec::new(),
            author_time: 0,
        }
    }

    fn page(oids: &[&str], more: bool) -> History {
        History {
            commits: oids.iter().map(|oid| commit(oid)).collect(),
            unborn: false,
            more,
        }
    }

    fn shown(list: &HistoryList) -> &Listing {
        match list {
            HistoryList::Shown(listing) => listing,
            other => panic!("expected a shown list, got {other:?}"),
        }
    }

    fn oids(list: &HistoryList) -> Vec<String> {
        shown(list)
            .history
            .commits
            .iter()
            .map(|c| c.oid.clone())
            .collect()
    }

    #[test]
    fn the_first_page_is_shown() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(4));
        list.received(OperationId(4), page(&["c", "b"], true));
        assert_eq!(oids(&list), ["c", "b"]);
        assert!(shown(&list).loaded);
        assert!(list.can_page());
    }

    #[test]
    fn the_graph_has_one_row_per_commit_across_pages_and_refreshes() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["c", "b"], true));
        assert_eq!(shown(&list).graph.len(), 2);

        // A page that repeats a commit adds only the ones the list lacks.
        list.request_page(OperationId(2));
        list.received(OperationId(2), page(&["b", "a"], false));
        assert_eq!(oids(&list), ["c", "b", "a"]);
        assert_eq!(shown(&list).graph.len(), 3);

        // A refresh replaces the commits, and the graph with them.
        list.request_top(OperationId(3));
        list.received(OperationId(3), page(&["d", "c"], true));
        assert_eq!(shown(&list).graph.len(), 2);
    }

    #[test]
    fn a_reply_to_an_older_request_is_dropped() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(4));
        list.request_top(OperationId(9));
        list.received(OperationId(4), page(&["old"], false));
        assert!(!shown(&list).loaded);
        assert_eq!(shown(&list).top, Some(OperationId(9)));
    }

    #[test]
    fn a_failed_first_page_shows_the_failure() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(4));
        list.failed(OperationId(4));
        assert_eq!(list, HistoryList::Failed);
    }

    #[test]
    fn a_failed_refresh_keeps_the_list_it_had() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(4));
        list.received(OperationId(4), page(&["c"], false));
        list.request_top(OperationId(5));
        list.failed(OperationId(5));
        assert_eq!(oids(&list), ["c"]);
        assert!(shown(&list).top.is_none());
    }

    #[test]
    fn a_next_page_adds_the_commits_after_the_last() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["c", "b"], true));
        assert_eq!(list.last_oid(), Some("b"));
        list.request_page(OperationId(2));
        list.received(OperationId(2), page(&["a"], false));
        assert_eq!(oids(&list), ["c", "b", "a"]);
        assert!(!list.can_page());
    }

    #[test]
    fn a_page_never_repeats_a_commit_already_listed() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["c", "b"], true));
        list.request_page(OperationId(2));
        list.received(OperationId(2), page(&["b", "a"], false));
        assert_eq!(oids(&list), ["c", "b", "a"]);
    }

    #[test]
    fn a_refresh_puts_the_new_top_above_and_keeps_the_selection() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["b", "a"], false));
        list.select("a");
        list.request_top(OperationId(2));
        list.received(OperationId(2), page(&["new", "b", "a"], false));
        assert_eq!(oids(&list), ["new", "b", "a"]);
        assert_eq!(shown(&list).selected.as_deref(), Some("a"));
    }

    #[test]
    fn a_refresh_drops_a_page_still_in_flight() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["c"], true));
        list.request_page(OperationId(2));
        list.request_top(OperationId(3));
        list.received(OperationId(2), page(&["b"], false));
        assert_eq!(oids(&list), ["c"]);
        assert!(!list.can_page());
    }

    #[test]
    fn a_failed_page_stops_paging_until_a_refresh() {
        let mut list = HistoryList::default();
        list.request_top(OperationId(1));
        list.received(OperationId(1), page(&["c"], true));
        list.request_page(OperationId(2));
        list.failed(OperationId(2));
        assert!(!list.can_page());
        list.request_top(OperationId(3));
        list.received(OperationId(3), page(&["c"], true));
        assert!(list.can_page());
    }
}

#[cfg(test)]
mod commit_pane_tests {
    use super::*;
    use gm_schema::{Actor, CommitDetail, OperationState};

    fn detail(subject: &str) -> CommitDetail {
        CommitDetail {
            subject: subject.into(),
            ..CommitDetail::default()
        }
    }

    fn shown_subject(pane: &CommitPane) -> &str {
        match pane {
            CommitPane::Shown(detail) => &detail.subject,
            other => panic!("expected the detail to be shown, got {other:?}"),
        }
    }

    fn changed(id: u64, state: OperationState, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    fn answered(id: u64, subject: &str) -> Event {
        Event::CommitDetailLoaded {
            id: OperationId(id),
            detail: detail(subject),
        }
    }

    #[test]
    fn nothing_is_shown_until_a_commit_is_selected() {
        assert_eq!(Model::default().commit, CommitPane::Empty);
    }

    #[test]
    fn the_answer_to_the_request_in_flight_is_shown() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));

        let _ = model.apply(answered(4, "feat: x"));

        assert_eq!(shown_subject(&model.commit), "feat: x");
    }

    #[test]
    fn an_answer_to_a_superseded_request_is_dropped() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));
        model.commit.request(OperationId(5));

        let _ = model.apply(answered(4, "stale"));

        assert_eq!(model.commit, CommitPane::Loading(OperationId(5)));
    }

    #[test]
    fn a_failed_read_shows_gits_message() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));

        let _ = model.apply(changed(
            4,
            OperationState::Failed,
            Some("fatal: bad object"),
        ));

        assert_eq!(model.commit, CommitPane::Failed("fatal: bad object".into()));
    }

    #[test]
    fn a_failed_read_without_text_still_says_so() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));

        let _ = model.apply(changed(4, OperationState::Failed, None));

        assert!(matches!(&model.commit, CommitPane::Failed(text) if !text.is_empty()));
    }

    #[test]
    fn a_superseded_cancellation_leaves_the_current_request_alone() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));
        model.commit.request(OperationId(5));

        let _ = model.apply(changed(4, OperationState::Cancelled, None));

        assert_eq!(model.commit, CommitPane::Loading(OperationId(5)));
    }

    #[test]
    fn a_cancelled_current_read_clears_the_pane() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));

        let _ = model.apply(changed(4, OperationState::Cancelled, None));

        assert_eq!(model.commit, CommitPane::Empty);
    }

    #[test]
    fn a_repository_switch_empties_the_pane() {
        let mut model = Model::default();
        model.commit.request(OperationId(4));
        let _ = model.apply(answered(4, "feat: x"));

        model.reset_for_repo();

        assert_eq!(model.commit, CommitPane::Empty);
    }
}

#[cfg(test)]
mod commit_stack_tests {
    use super::*;
    use gm_schema::{Actor, FileDiff, OperationState};

    fn diff(path: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            old_path: None,
            binary: false,
            hunks: Vec::new(),
        }
    }

    fn listed(paths: &[&str]) -> Vec<CommitFile> {
        paths
            .iter()
            .map(|path| CommitFile {
                path: (*path).into(),
                old_path: None,
                change: gm_schema::Change::Modified,
                added: 1,
                deleted: 0,
                binary: false,
            })
            .collect()
    }

    #[test]
    fn the_answer_to_the_request_in_flight_is_taken_once_with_its_focus() {
        let mut model = Model::default();
        model
            .commit_stack
            .request(OperationId(4), 1, listed(&["a.txt", "b.txt"]), 0);
        assert!(model.commit_stack.is_open());
        assert!(!model.commit_stack.is_shown());
        assert_eq!(
            model.apply(Event::CommitStackLoaded {
                id: OperationId(4),
                diffs: vec![diff("a.txt"), diff("b.txt")],
            }),
            None
        );
        let fresh = model.commit_stack.take_fresh().unwrap();
        assert_eq!(fresh.focus, 1);
        assert_eq!(fresh.files.len(), 2);
        assert_eq!(fresh.diffs.len(), 2);
        assert_eq!(model.commit_stack.take_fresh(), None);
        assert!(model.commit_stack.is_shown());
    }

    #[test]
    fn an_answer_to_a_superseded_request_is_dropped() {
        let mut model = Model::default();
        model
            .commit_stack
            .request(OperationId(4), 2, listed(&["old.txt"]), 0);
        model
            .commit_stack
            .request(OperationId(5), 0, listed(&["new.txt"]), 0);
        let _ = model.apply(Event::CommitStackLoaded {
            id: OperationId(4),
            diffs: vec![diff("old.txt")],
        });
        assert_eq!(model.commit_stack.take_fresh(), None);
        assert!(!model.commit_stack.is_shown());
    }

    #[test]
    fn closing_the_stack_drops_the_answer_still_on_its_way() {
        let mut model = Model::default();
        model
            .commit_stack
            .request(OperationId(4), 0, listed(&["a.txt"]), 0);
        model.commit_stack.close();
        let _ = model.apply(Event::CommitStackLoaded {
            id: OperationId(4),
            diffs: vec![diff("a.txt")],
        });
        assert_eq!(model.commit_stack.take_fresh(), None);
        assert!(!model.commit_stack.is_open());
    }

    #[test]
    fn a_failed_stack_read_keeps_gits_message() {
        let mut model = Model::default();
        model
            .commit_stack
            .request(OperationId(4), 0, listed(&["a.txt"]), 0);
        let _ = model.apply(Event::OperationChanged {
            id: OperationId(4),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: Some("fatal: bad object".into()),
        });
        assert_eq!(model.commit_stack.failure(), Some("fatal: bad object"));
    }
}

#[cfg(test)]
mod stash_tests {
    use super::*;
    use gm_schema::{Actor, CommitDetail, FileDiff, StashEntry};

    fn stash(index: u32, oid: &str) -> StashEntry {
        StashEntry {
            index,
            oid: oid.repeat(40),
            message: format!("On main: {index}"),
            branch: Some("main".into()),
            time: 0,
        }
    }

    fn detail(oid: &str) -> CommitDetail {
        CommitDetail {
            oid: oid.repeat(40),
            subject: format!("stash {oid}"),
            ..CommitDetail::default()
        }
    }

    fn changed(id: u64, state: OperationState, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    #[test]
    fn a_stash_listing_reaches_the_list_and_its_count() {
        let mut model = Model::default();
        model.stashes.request(OperationId(3));
        let _ = model.apply(Event::StashesListed {
            id: OperationId(3),
            stashes: vec![stash(0, "a"), stash(1, "b")],
        });
        assert_eq!(model.stashes.count(), Some(2));
    }

    #[test]
    fn a_failed_stash_read_belongs_to_the_page_and_not_the_status_bar() {
        let mut model = Model::default();
        model.stashes.request(OperationId(3));
        let _ = model.apply(changed(3, OperationState::Failed, Some("fatal: no stash")));
        assert_eq!(model.stashes.failure(), Some("fatal: no stash"));
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_stash_detail_fills_the_stash_panel_and_leaves_the_history_one() {
        let mut model = Model::default();
        model.stash_commit.request(OperationId(7));
        model.commit.request(OperationId(8));
        let _ = model.apply(Event::CommitDetailLoaded {
            id: OperationId(7),
            detail: detail("a"),
        });
        assert!(matches!(model.stash_commit, CommitPane::Shown(_)));
        assert_eq!(model.commit, CommitPane::Loading(OperationId(8)));
        assert_eq!(model.commit_pane(CommitPage::Stashes), &model.stash_commit);
        assert_eq!(model.commit_pane(CommitPage::History), &model.commit);
    }

    #[test]
    fn a_stash_stack_answer_goes_to_the_stash_stack() {
        let mut model = Model::default();
        model.stash_stack.request(OperationId(4), 0, Vec::new(), 0);
        model.commit_stack.request(OperationId(5), 0, Vec::new(), 0);
        let _ = model.apply(Event::CommitStackLoaded {
            id: OperationId(4),
            diffs: vec![FileDiff {
                path: "a.txt".into(),
                old_path: None,
                binary: false,
                hunks: Vec::new(),
            }],
        });
        assert!(model.stash_stack.is_shown());
        assert!(model.commit_stack.is_open());
        assert!(!model.commit_stack.is_shown());
    }

    #[test]
    fn a_selected_stash_that_is_gone_takes_its_panel_with_it() {
        let mut model = Model::default();
        model.stashes.request(OperationId(3));
        let _ = model.apply(Event::StashesListed {
            id: OperationId(3),
            stashes: vec![stash(0, "a")],
        });
        assert!(model.stashes.select(&stash(0, "a").oid));
        model.stash_commit.request(OperationId(4));
        let _ = model.apply(Event::CommitDetailLoaded {
            id: OperationId(4),
            detail: detail("a"),
        });
        // The stash was popped: the list no longer has it.
        model.stashes.request(OperationId(5));
        let _ = model.apply(Event::StashesListed {
            id: OperationId(5),
            stashes: Vec::new(),
        });
        assert_eq!(model.stashes.selected(), None);
        assert_eq!(model.stash_commit, CommitPane::Empty);
    }

    #[test]
    fn a_new_repository_starts_with_no_stash_list_and_no_stash_panel() {
        let mut model = Model::default();
        model.stashes.request(OperationId(3));
        let _ = model.apply(Event::StashesListed {
            id: OperationId(3),
            stashes: vec![stash(0, "a")],
        });
        model.stash_commit.request(OperationId(4));
        model.request_open(OperationId(9));
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(9),
            status: RepoStatus {
                root: "/other".into(),
                ..RepoStatus::default()
            },
        });
        assert_eq!(model.stashes, StashList::default());
        assert_eq!(model.stash_commit, CommitPane::Empty);
    }

    #[test]
    fn a_stash_create_settles_on_its_own_operation_only() {
        let mut model = Model::default();
        model.stash_create.start(OperationId(6));
        let _ = model.apply(changed(5, OperationState::Done, None));
        assert!(model.stash_create.is_pending());
        let _ = model.apply(changed(6, OperationState::Failed, Some("No local changes")));
        assert_eq!(
            model.stash_create.take(),
            Some(Err("No local changes".to_owned()))
        );
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_conflicting_apply_is_its_trackers_outcome_and_not_the_status_bars() {
        use crate::stash_apply::{Kind, Outcome};
        let mut model = Model::default();
        model.stash_apply.start(OperationId(9), Kind::Apply, 1);
        let text = "CONFLICT (content): Merge conflict in a.txt";
        let _ = model.apply(changed(9, OperationState::Failed, Some(text)));
        assert_eq!(model.error, None);
        assert_eq!(model.notice, None);
        assert_eq!(
            model.stash_apply.take(),
            Some(Outcome::Failed {
                kind: Kind::Apply,
                index: 1,
                text: text.to_owned(),
            })
        );
    }

    #[test]
    fn a_pop_is_announced_by_its_tracker_and_the_listing_after_it_is_shown() {
        use crate::stash_apply::{Kind, Outcome};
        let mut model = Model::default();
        model.stashes.request(OperationId(8));
        model.stash_apply.start(OperationId(9), Kind::Pop, 0);
        let _ = model.apply(changed(9, OperationState::Executing, None));
        assert_eq!(model.stash_apply.take(), None);
        let _ = model.apply(changed(9, OperationState::Done, None));
        assert_eq!(
            model.stash_apply.take(),
            Some(Outcome::Done {
                kind: Kind::Pop,
                index: 0,
            })
        );
        // The stash-write listing comes after the operation and replaces the list.
        let _ = model.apply(Event::StashesListed {
            id: OperationId(9),
            stashes: Vec::new(),
        });
        assert_eq!(model.stashes.count(), Some(0));
    }

    #[test]
    fn a_drop_outcome_is_its_dialogs_and_a_refused_drop_keeps_the_message() {
        use crate::stash_drop::Outcome;
        let mut model = Model::default();
        model.stash_drop.start(OperationId(12), 2);
        let _ = model.apply(changed(
            12,
            OperationState::Failed,
            Some(gm_schema::STASH_LIST_CHANGED),
        ));
        assert_eq!(model.error, None);
        assert_eq!(
            model.stash_drop.take(),
            Some(Outcome::Failed(gm_schema::STASH_LIST_CHANGED.to_owned()))
        );
    }

    fn reflog_entry(index: u32, oid: char, time: i64) -> gm_schema::ReflogEntry {
        gm_schema::ReflogEntry {
            index,
            oid: oid.to_string().repeat(40),
            action: "commit".into(),
            detail: format!("entry {index}"),
            subject: None,
            time,
        }
    }

    fn reflog_failed(id: u64, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: gm_schema::Actor::Human,
            state: gm_schema::OperationState::Failed,
            detail: detail.map(str::to_owned),
        }
    }

    #[test]
    fn a_reflog_page_is_applied_only_when_it_answers_the_read_in_flight() {
        let mut model = Model::default();
        model.reflog.request(OperationId(4));
        let answer = |id: u64| Event::ReflogLoaded {
            id: OperationId(id),
            skip: 0,
            entries: vec![reflog_entry(0, 'a', 1)],
            has_more: true,
        };
        let _ = model.apply(answer(3));
        assert!(!model.reflog.is_loaded());
        let _ = model.apply(answer(4));
        assert!(model.reflog.is_loaded());
        assert_eq!(model.reflog.entries().len(), 1);
        assert!(model.reflog.can_page());
    }

    #[test]
    fn a_failed_reflog_read_is_shown_on_its_page_and_not_in_the_status_bar() {
        let mut model = Model::default();
        model.reflog.request(OperationId(5));
        let _ = model.apply(reflog_failed(5, Some("fatal: bad revision")));
        assert_eq!(model.error, None);
        assert_eq!(model.reflog.failure(), Some("fatal: bad revision"));
    }

    #[test]
    fn a_detached_checkout_that_fails_goes_to_its_message_and_not_the_status_bar() {
        use crate::reflog::CheckoutOutcome;

        let mut model = Model::default();
        model.reflog_checkout.start(OperationId(6), &"a".repeat(40));
        let git = "error: Your local changes would be overwritten by checkout.";
        let _ = model.apply(reflog_failed(6, Some(git)));
        assert_eq!(model.error, None);
        assert_eq!(
            model.reflog_checkout.take(),
            Some(CheckoutOutcome::Failed {
                short: "aaaaaaa".into(),
                text: git.into(),
            })
        );
    }

    #[test]
    fn a_reload_that_drops_the_selected_entry_clears_its_detail_and_diff() {
        let mut model = Model::default();
        model.reflog.request(OperationId(1));
        let _ = model.apply(Event::ReflogLoaded {
            id: OperationId(1),
            skip: 0,
            entries: vec![reflog_entry(0, 'a', 1), reflog_entry(1, 'b', 2)],
            has_more: false,
        });
        assert!(
            model
                .reflog
                .select(&crate::reflog::EntryKey::of(&reflog_entry(1, 'b', 2)))
        );
        model.reflog_commit = CommitPane::Loading(OperationId(2));
        // The same entry is kept across a reload: its detail stays.
        model.reflog.request(OperationId(3));
        let _ = model.apply(Event::ReflogLoaded {
            id: OperationId(3),
            skip: 0,
            entries: vec![reflog_entry(0, 'c', 3), reflog_entry(1, 'b', 2)],
            has_more: false,
        });
        assert_eq!(model.reflog_commit, CommitPane::Loading(OperationId(2)));
        // Once it is gone, the panel goes with it.
        model.reflog.request(OperationId(4));
        let _ = model.apply(Event::ReflogLoaded {
            id: OperationId(4),
            skip: 0,
            entries: vec![reflog_entry(0, 'c', 3)],
            has_more: false,
        });
        assert_eq!(model.reflog_commit, CommitPane::Empty);
        assert!(!model.reflog.has_selection());
    }

    fn listed_comment(id: i64, text: &str) -> gm_schema::ReviewComment {
        gm_schema::ReviewComment {
            id,
            path: "a.txt".into(),
            body: "note".into(),
            status: gm_schema::ReviewStatus::Open,
            anchor: gm_schema::ReviewAnchor {
                text: text.into(),
                before: Vec::new(),
                after: Vec::new(),
                start_line: 2,
            },
            created_at: 0,
            resolved_at: None,
            summary: None,
            snapshot: Some("abc".into()),
            after_snapshot: None,
            location: Some(gm_schema::ReviewLocation {
                start_line: 2,
                end_line: 2,
            }),
            outdated: false,
        }
    }

    #[test]
    fn the_local_review_texts_are_kept_for_the_selection_and_a_listing_without_it_ends_it() {
        use crate::local_review::Version;
        let mut model = Model::default();
        let _ = model.apply(Event::ReviewCommentsListed {
            id: OperationId(1),
            comments: vec![listed_comment(7, "b")],
        });
        let comment = model.review.comment(7).cloned().expect("listed");
        assert!(model.local_review.select(&comment));
        model.local_review.expect(OperationId(5), OperationId(6));

        let _ = model.apply(Event::ReviewSnapshotText {
            id: OperationId(4),
            comment: 7,
            path: "a.txt".into(),
            lines: vec!["stale".into()],
        });
        assert_eq!(model.local_review.lines(Version::Before), None);
        let _ = model.apply(Event::ReviewSnapshotText {
            id: OperationId(5),
            comment: 7,
            path: "a.txt".into(),
            lines: vec!["b".into()],
        });
        let _ = model.apply(Event::ReviewCurrentText {
            id: OperationId(6),
            comment: 7,
            path: "a.txt".into(),
            lines: vec!["B".into()],
        });
        assert_eq!(
            model.local_review.lines(Version::Before),
            Some(["b".to_owned()].as_slice())
        );
        assert_eq!(
            model.local_review.lines(Version::After),
            Some(["B".to_owned()].as_slice())
        );

        let _ = model.apply(Event::ReviewCommentsListed {
            id: OperationId(9),
            comments: vec![listed_comment(8, "other")],
        });
        assert_eq!(model.local_review.selected_id(), None);
    }

    #[test]
    fn a_handled_comment_takes_both_texts_from_its_change_and_drops_a_late_one() {
        use crate::local_review::{self, Version};
        let mut handled = listed_comment(7, "b");
        handled.status = gm_schema::ReviewStatus::Handled;
        handled.after_snapshot = Some("def".into());
        let mut model = Model::default();
        let _ = model.apply(Event::ReviewCommentsListed {
            id: OperationId(1),
            comments: vec![handled],
        });
        let comment = model.review.comment(7).cloned().expect("listed");
        assert!(local_review::reads_change(&comment));
        assert!(model.local_review.select(&comment));
        model.local_review.expect_change(OperationId(5));

        let change = |before: &str, after: &str| {
            Some(gm_schema::ReviewChange {
                before: vec![before.into()],
                after: vec![after.into()],
                changed: before != after,
            })
        };
        let _ = model.apply(Event::ReviewChange {
            id: OperationId(4),
            comment: 7,
            path: "a.txt".into(),
            change: change("stale", "stale"),
        });
        assert_eq!(model.local_review.lines(Version::Before), None);
        assert_eq!(model.local_review.lines(Version::After), None);
        let _ = model.apply(Event::ReviewChange {
            id: OperationId(5),
            comment: 7,
            path: "a.txt".into(),
            change: change("b", "B"),
        });
        assert_eq!(
            model.local_review.lines(Version::Before),
            Some(["b".to_owned()].as_slice())
        );
        assert_eq!(
            model.local_review.lines(Version::After),
            Some(["B".to_owned()].as_slice())
        );
    }
}

#[cfg(test)]
mod move_file_tests {
    use gm_schema::{Actor, BranchInfo, MoveBlocker, MoveOutcome, MovePreview, OperationState};

    use super::*;
    use crate::move_file::{DropTarget, Moving, PopoverState};

    fn done(id: u64) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Done,
            detail: None,
        }
    }

    fn failed(id: u64, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state: OperationState::Failed,
            detail: detail.map(str::to_owned),
        }
    }

    fn branch(name: &str) -> DropTarget {
        DropTarget::Branch {
            name: name.to_owned(),
        }
    }

    /// A model with a drag of `a.txt` resting on the branch `feature`, its check sent as `id`.
    fn checking(id: u64) -> Model {
        let mut model = Model::default();
        model.move_file.begin_drag("a.txt");
        model
            .move_file
            .hover
            .enter(branch("feature"), std::time::Instant::now());
        model.move_file.hover.sent(OperationId(id));
        model
    }

    #[test]
    fn a_preview_answers_the_check_of_its_row_and_a_late_one_is_dropped() {
        let mut model = checking(1);
        let _ = model.apply(Event::MovePreviewed {
            id: OperationId(9),
            preview: MovePreview::default(),
        });
        assert_eq!(model.move_file.hover.preview(&branch("feature")), None);
        let blocked = MovePreview {
            blockers: vec![MoveBlocker::BranchExists],
        };
        let _ = model.apply(Event::MovePreviewed {
            id: OperationId(1),
            preview: blocked.clone(),
        });
        assert_eq!(
            model.move_file.hover.preview(&branch("feature")),
            Some(&blocked)
        );
    }

    #[test]
    fn a_preview_that_fails_is_not_shown_as_an_error_and_is_not_asked_again_at_once() {
        let mut model = checking(2);
        let _ = model.apply(failed(2, Some("git refused")));
        assert_eq!(model.error, None);
        assert_eq!(
            model
                .move_file
                .hover
                .due(std::time::Instant::now() + std::time::Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn a_new_status_makes_every_answer_ask_again() {
        let mut model = checking(3);
        let _ = model.apply(Event::MovePreviewed {
            id: OperationId(3),
            preview: MovePreview::default(),
        });
        let _ = model.apply(Event::StatusLoaded {
            id: OperationId(4),
            status: RepoStatus {
                root: "/r".to_owned(),
                branch: BranchInfo::default(),
                entries: Vec::new(),
            },
        });
        assert_eq!(model.move_file.hover.preview(&branch("feature")), None);
    }

    #[test]
    fn a_worktree_move_says_it_and_offers_to_open_the_worktree() {
        let mut model = Model::default();
        let id = OperationId(5);
        model.move_file.sent_move(Moving {
            id,
            file: "a.txt".into(),
            target: DropTarget::Worktree {
                path: "/w/ui".into(),
            },
            worktree: "ui".into(),
            from_popover: false,
        });
        let _ = model.apply(Event::ChangesMoved {
            id,
            moved: MoveOutcome::default(),
        });
        assert_eq!(model.notice.as_deref(), Some("Moved a.txt to ui."));
        assert_eq!(model.notice_icon, Some(Icon::CheckCircle));
        assert_eq!(model.notice_open.as_deref(), Some("/w/ui"));
        let _ = model.apply(done(5));
        assert!(model.move_file.moving.is_none());
        model.set_notice("Something else".to_owned());
        assert_eq!(model.notice_open, None);
    }

    #[test]
    fn a_branch_move_closes_its_popover_and_names_the_commit_branch() {
        let mut model = Model::default();
        let id = OperationId(6);
        model
            .move_file
            .open_popover(PopoverState::existing("a.txt", "feature"));
        model.move_file.sent_move(Moving {
            id,
            file: "a.txt".into(),
            target: branch("feature"),
            worktree: String::new(),
            from_popover: true,
        });
        let _ = model.apply(Event::ChangesMoved {
            id,
            moved: MoveOutcome {
                commit: Some("abc1234".into()),
                kept: None,
            },
        });
        assert_eq!(model.notice.as_deref(), Some("Committed a.txt to feature."));
        assert_eq!(model.notice_open, None);
        assert!(model.move_file.popover.is_none());
    }

    #[test]
    fn a_failed_branch_move_stays_in_its_popover_with_the_reason() {
        let mut model = Model::default();
        let id = OperationId(7);
        model
            .move_file
            .open_popover(PopoverState::existing("a.txt", "feature"));
        model.move_file.sent_move(Moving {
            id,
            file: "a.txt".into(),
            target: branch("feature"),
            worktree: String::new(),
            from_popover: true,
        });
        let _ = model.apply(failed(7, Some("the branch moved")));
        assert!(model.move_file.popover.is_some());
        assert_eq!(
            model.move_file.popover_error.as_deref(),
            Some("the branch moved")
        );
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_failed_worktree_move_is_an_error_of_the_status_bar() {
        let mut model = Model::default();
        let id = OperationId(8);
        model.move_file.sent_move(Moving {
            id,
            file: "a.txt".into(),
            target: DropTarget::Worktree {
                path: "/w/ui".into(),
            },
            worktree: "ui".into(),
            from_popover: false,
        });
        let _ = model.apply(failed(8, Some("a conflict")));
        assert_eq!(model.error.as_deref(), Some("a conflict"));
        assert!(model.move_file.moving.is_none());
    }

    #[test]
    fn a_move_in_flight_is_owned_by_its_own_operation_only() {
        let mut model = Model::default();
        model.move_file.sent_move(Moving {
            id: OperationId(10),
            file: "a.txt".into(),
            target: branch("feature"),
            worktree: String::new(),
            from_popover: false,
        });
        assert!(model.move_file.owns(OperationId(10)));
        let _ = model.apply(done(11));
        assert!(model.move_file.owns(OperationId(10)));
    }

    fn conflict_read(path: &str, id: u64) -> Event {
        use gm_schema::{ConflictFileRead, ConflictOperation, ConflictSide};
        let side = |branch: &str, role: &str| ConflictSide {
            branch: branch.into(),
            role: role.into(),
            subject: "Add the parser".into(),
        };
        Event::ConflictRead {
            id: OperationId(id),
            read: ConflictFileRead {
                path: path.into(),
                operation: ConflictOperation::Merge,
                left: side("main", "current branch"),
                right: side("feature/x", "merged branch"),
                text: "<<<<<<< main\nleft\n=======\nright\n>>>>>>> feature/x\n".into(),
                has_base: false,
                recorded: None,
            },
        }
    }

    #[test]
    fn a_conflict_read_opens_the_resolver_on_its_file_and_asks_for_the_page() {
        let mut model = Model::default();
        let _ = model.apply(conflict_read("a.txt", 3));
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert_eq!(conflict.path(), "a.txt");
        assert_eq!(conflict.counter_label(), "0 of 1 conflict resolved");
        assert_eq!(model.take_conflict_opened(), Some(OperationId(3)));
        assert_eq!(
            model.take_conflict_opened(),
            None,
            "the page is asked for once"
        );
    }

    #[test]
    fn a_resolved_event_marks_only_the_resolvers_own_file_as_staged() {
        let mut model = Model::default();
        let _ = model.apply(conflict_read("a.txt", 3));
        let _ = model.apply(Event::ConflictResolved {
            id: OperationId(4),
            path: "b.txt".into(),
        });
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert!(!conflict.is_staged());
        assert_eq!(conflict.applied_note(), None);
        let _ = model.apply(Event::ConflictResolved {
            id: OperationId(5),
            path: "a.txt".into(),
        });
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert!(conflict.is_staged());
        assert_eq!(conflict.applied_note(), Some("Resolved and staged."));
    }

    #[test]
    fn a_resolved_event_with_no_resolver_open_is_ignored() {
        let mut model = Model::default();
        let _ = model.apply(Event::ConflictResolved {
            id: OperationId(4),
            path: "a.txt".into(),
        });
        assert!(model.conflict.is_none());
        assert_eq!(model.error, None);
    }

    fn conflict_read_recorded(path: &str, id: u64) -> Event {
        let Event::ConflictRead { id, mut read } = conflict_read(path, id) else {
            unreachable!("conflict_read builds a ConflictRead");
        };
        read.recorded = Some(gm_schema::RecordedResolution {
            text: "plain\n".into(),
        });
        Event::ConflictRead { id, read }
    }

    #[test]
    fn a_forgotten_resolution_of_the_open_file_asks_for_one_read_and_the_read_keeps_the_note() {
        let mut model = Model::default();
        let _ = model.apply(conflict_read_recorded("a.txt", 3));
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert_eq!(
            conflict.recorded_banner(),
            Some("Resolved by recorded resolution")
        );
        let _ = model.apply(Event::RecordedResolutionForgotten {
            id: OperationId(4),
            path: "a.txt".into(),
        });
        assert_eq!(model.take_conflict_reread(), Some("a.txt".to_owned()));
        assert_eq!(
            model.take_conflict_reread(),
            None,
            "the read is asked for once"
        );
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert_eq!(conflict.recorded_banner(), None);
        assert!(conflict.needs_reread());
        let _ = model.apply(conflict_read("a.txt", 5));
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert!(!conflict.needs_reread());
        assert_eq!(
            conflict.applied_note(),
            Some("Recorded resolution forgotten. The conflict is open again.")
        );
    }

    #[test]
    fn a_forgotten_resolution_of_another_file_asks_for_no_read() {
        let mut model = Model::default();
        let _ = model.apply(conflict_read_recorded("a.txt", 3));
        let _ = model.apply(Event::RecordedResolutionForgotten {
            id: OperationId(4),
            path: "b.txt".into(),
        });
        assert_eq!(model.take_conflict_reread(), None);
        let conflict = model.conflict.as_ref().expect("the resolver is open");
        assert!(conflict.recorded_banner().is_some());
    }
}

#[cfg(test)]
mod thread_copy_tests {
    use super::*;
    use gm_schema::{Actor, OperationState, ThreadSummary};

    fn changed(id: u64, state: OperationState, detail: Option<&str>) -> Event {
        Event::OperationChanged {
            id: OperationId(id),
            actor: Actor::Human,
            state,
            detail: detail.map(str::to_owned),
        }
    }

    fn summary(uid: &str) -> ThreadSummary {
        ThreadSummary {
            uid: uid.to_owned(),
            harness: "claude-code".to_owned(),
            harness_version: None,
            session_id: "s".to_owned(),
            worktree: "/repo".to_owned(),
            branch: None,
            model: None,
            title: "T".to_owned(),
            note: String::new(),
            starred: false,
            tags: Vec::new(),
            started_at: 0,
            last_event_at: 1,
            ended_at: None,
            entry_count: 0,
            edit_count: 0,
            redaction_count: 0,
        }
    }

    #[test]
    fn a_copied_thread_is_kept_for_the_window_once_its_answer_comes() {
        let mut model = Model::default();
        model.thread_copy.sent(OperationId(4));
        let _ = model.apply(Event::ThreadMarkdown {
            id: OperationId(4),
            uid: "u".to_owned(),
            markdown: "# T\n".to_owned(),
        });
        // The success that follows the answer changes nothing.
        let _ = model.apply(changed(4, OperationState::Done, None));
        assert_eq!(model.thread_copy.take(), Some(Ok("# T\n".to_owned())));
        assert_eq!(model.thread_copy.take(), None);
    }

    #[test]
    fn a_failed_copy_keeps_the_engines_message_and_leaves_the_status_bar_alone() {
        let mut model = Model::default();
        model.thread_copy.sent(OperationId(5));
        let _ = model.apply(changed(
            5,
            OperationState::Failed,
            Some("invalid redaction pattern"),
        ));
        assert_eq!(model.error, None);
        assert_eq!(
            model.thread_copy.take(),
            Some(Err("invalid redaction pattern".to_owned()))
        );
    }

    #[test]
    fn a_cancelled_copy_says_nothing() {
        let mut model = Model::default();
        model.thread_copy.sent(OperationId(6));
        let _ = model.apply(changed(6, OperationState::Cancelled, None));
        assert_eq!(model.thread_copy.take(), None);
        assert_eq!(model.error, None);
    }

    #[test]
    fn a_commit_threads_answer_goes_to_the_commit_panel_and_not_to_the_page() {
        let mut model = Model::default();
        model.commit_threads.request("abc", OperationId(7));
        let _ = model.apply(Event::ThreadsListed {
            id: OperationId(7),
            threads: vec![summary("t1")],
            total: 3,
        });
        assert_eq!(model.commit_threads.threads_of("abc").len(), 1);
        assert_eq!(model.threads.total(), None, "the page's list is untouched");
    }
}
