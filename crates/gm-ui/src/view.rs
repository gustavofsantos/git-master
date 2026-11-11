//! The Working Copy window: the top bar, the left column (file list, commit
//! box), the diff pane on the right and the status bar. It sends commands to
//! the engine and renders the model; it never waits for git. Every action the
//! window runs goes through `run_action`, which checks the action registry.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gm_core::EngineHandle;
use gm_core::command_log::CommandLog;
use gm_core::watch::{Notice, Notices, WatchHandle};
use gm_schema::{
    Actor, CommandKind, ConflictSite, DiffSide, Event, FeatureSet, FileDiff, HistoryScope,
    OperationId, OperationState,
};
use gpui::{
    ClipboardItem, Context, Div, Entity, FocusHandle, KeyDownEvent, Render, ScrollHandle,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::action_registry::{
    self, Action, Args, CommentSpot, Context as ActionContext, Effect, LabelRef, MemoryRef,
    MessageCtx, Reason, ReviewCtx, ThreadRef, WorkflowCtx,
};
use crate::app_settings::{self, AppSettings};
use crate::avatar::Avatars;
use crate::branch_delete::DeleteDialog;
use crate::branch_filter::BranchFilter;
use crate::branch_menu::BranchMenu;
use crate::compare::ChangesTab;
use crate::composer::{Composer, ComposerEvent};
use crate::context_menu::ContextMenu;
use crate::contexts_dialog::ContextsDialog;
use crate::diff_pane::{DiffPane, DiffPaneEvent};
use crate::diff_rows::Layout;
use crate::editor_theme;
use crate::file_discard::DiscardDialog;
use crate::hook_convert::{self, CONVERT_TITLE};
use crate::icon;
use crate::memory_dialogs::MemoryDialog;
use crate::merged_branches::MergedDialog;
use crate::message_dialog::MessageDialog;
use crate::model::{CommitPage, Model, Switched};
use crate::move_file::MovePopover;
use crate::new_branch::NewBranchDialog;
use crate::new_stash::NewStashDialog;
use crate::palette::Palette;
use crate::palette_view::{self, PaletteView};
use crate::project_menu::ProjectMenu;
use crate::quick_open_view::{self, QuickOpenView};
use crate::recent::{self, Recents};
use crate::reflog;
use crate::resize;
use crate::restack_dialog::{RestackDialog, Transition};
use crate::review_composer::{InlineReply, ReviewDialog};
use crate::shell_code::CodeView;
use crate::shell_health::{self, HealthView};
use crate::shell_history::CommitTexts;
use crate::shell_labels::{self, LabelsView};
use crate::shell_memories::{self, MemoriesView, ThreadsTab};
use crate::shell_search::{self, SearchView};
use crate::shell_sidebar;
use crate::shell_threads::{self, ThreadsView};
use crate::sidebar::Folded;
use crate::stash_apply;
use crate::stash_drop::DropStashDialog;
use crate::stashes;
use crate::suggestion::Fill;
use crate::sync_state;
use crate::text_area::{TextArea, TextAreaEvent, TextStyle};
use crate::theme;
use crate::thread_dialogs::{ConfirmDialog, FieldDialog, LinkDialog};
use crate::threads_settings::ThreadsSettingsDialog;
use crate::title_bar;
use crate::transform_run::{self, Field};
use crate::welcome::{self, Startup, Welcome};
use crate::window_chrome;
use crate::workflow_editor;
use crate::workflow_inputs::WorkflowInputsDialog;
use crate::workflow_review::WorkflowReviewDialog;
use crate::workflow_run::fill_for_message;
use crate::workflows_page::is_conflict;
use crate::workspace_create::WorkspaceDialog;
use crate::workspace_remove::RemoveWorkspaceDialog;
use crate::worktree_create::WorktreeDialog;
use crate::worktree_prune::PruneDialog;
use crate::worktree_remove::RemoveDialog;
use crate::{
    CopyCurrentBranch, FocusNext, FocusPrev, OpenPalette, OpenQuickOpen, OpenSearch,
    RunWorkflowKey, StartPath, UndoLastOperation,
};
use gpui_component::input::InputState;

/// The key context of the change list. Its keys work only while the list has
/// focus, so typing into the message is never taken as a key.
pub const KEY_CONTEXT: &str = "GitMaster";

/// How long a notice stays on the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(4);

/// The widest the status bar's notice may be; a longer one is cut at its end.
const NOTICE_WIDTH: f32 = 480.;

/// How long a refresh runs before "Updating…" shows. Quicker ones never show it.
const UPDATING_DELAY: Duration = Duration::from_millis(300);

const WATCH_LIMITED: &str = "Watching is limited; refreshing when the window gains focus.";

/// What the main area shows: the Working Copy, the history page (H1), the
/// Workflows page (D76), the Code page (N2), the text search (N3) or the Stashes page (B2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Screen {
    #[default]
    WorkingCopy,
    /// What needs the person: derived items in sections (UI redesign, slice 6).
    Inbox,
    History,
    Workflows,
    /// The folder tree and the file viewer (N2).
    Code,
    /// The text search (N3).
    Search,
    /// The Health tab of the Code page: the hotspots, the dormancy breaks and the file details.
    Health,
    /// The stashes of the repository, with the selected stash's files and diffs (B2).
    Stashes,
    /// The entries of HEAD's reflog, with the selected entry's files and diffs (B3).
    Reflog,
    /// The threads of the agent sessions, with the selected thread's turns (T5).
    Threads,
    /// The labels of the repository, with the selected label's attachments (P4).
    Labels,
    /// The review comments of the repository, with one comment's Before and After (B4d).
    LocalReview,
    /// The conflict resolver of one conflicted file: the two sides, the result and Apply (C3b).
    Conflicts,
}

pub struct StatusView {
    pub(crate) engine: EngineHandle,
    /// Every git command that changed the repository, for the log panel.
    pub(crate) command_log: CommandLog,
    pub(crate) log_open: bool,
    pub(crate) log_scroll: ScrollHandle,
    pub(crate) model: Model,
    pub(crate) list_focus: FocusHandle,
    /// The Code page's focus: a click in the page holds it, so the window's keys reach the app.
    pub(crate) code_focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) composer: Entity<Composer>,
    pub(crate) diff_pane: Entity<DiffPane>,
    /// The read-only pane that shows one file of the open commit (H4).
    pub(crate) commit_diff_pane: Entity<DiffPane>,
    /// The read-only pane of the Branch and Compare tabs: one file of the shown range (D231 (4)).
    pub(crate) compare_diff_pane: Entity<DiffPane>,
    /// The sidebar's width, in logical pixels, as the user dragged it.
    pub(crate) sidebar_width: f32,
    /// The selectable fields of the commit detail panel (H3).
    pub(crate) commit_texts: CommitTexts,
    /// The left column's width, in logical pixels, as the user dragged it.
    pub(crate) left_width: f32,
    /// The width of the Code file tree, the Workflows list and the Threads list.
    pub(crate) code_tree_width: f32,
    pub(crate) workflow_list_width: f32,
    pub(crate) thread_list_width: f32,
    /// The commit message box's height, in logical pixels, as dragged.
    pub(crate) commit_height: f32,
    /// The preview features that are on (D169); a feature that is off has no surface here.
    pub(crate) features: FeatureSet,
    /// Which sidebar sections are folded.
    pub(crate) sidebar_folded: Folded,
    /// The sidebar's branch filter: the typed query (D53).
    pub(crate) sidebar_filter: BranchFilter,
    /// The input the sidebar's query is typed into.
    pub(crate) sidebar_input: Entity<TextArea>,
    /// The preferences as last read or saved. A save writes all of them, so a
    /// save for one preference keeps the others.
    pub(crate) settings: AppSettings,
    /// The main area's screen; the Working Copy unless the history is open.
    pub(crate) screen: Screen,
    pub(crate) project_menu: ProjectMenu,
    /// The branch button's list: filter, scroll and the button's place.
    pub(crate) branch_menu: BranchMenu,
    /// The right-click menu of a branch or a file row, when one is open.
    pub(crate) context_menu: ContextMenu,
    /// The command palette's state: its query, its step and its ranked rows.
    pub(crate) palette: Palette,
    pub(crate) palette_view: PaletteView,
    /// Quick Open's input and list scroll; its state is `model.quick_open`.
    pub(crate) quick_open_view: QuickOpenView,
    /// The Code page's scrolls; its state is `model.code`.
    pub(crate) code_view: CodeView,
    /// The text search page's boxes and scroll; its state is `model.text_search`.
    pub(crate) search_view: SearchView,
    /// The Health tab's filter box and list scroll; its state is `model.health`.
    pub(crate) health_view: HealthView,
    /// The Threads page's search box and list scroll; its state is `model.threads`.
    pub(crate) threads_view: ThreadsView,
    /// The Memories tab's search box, path box and edit; its state is `model.memories`.
    pub(crate) memories_view: MemoriesView,
    /// Which tab of the Threads page shows (D190). Each tab keeps its own state.
    pub(crate) threads_tab: ThreadsTab,
    /// The New Memory, Supersedes… or Delete dialog, while it is open (D190).
    pub(crate) memory_dialog: Option<MemoryDialog>,
    /// The Manage Contexts dialog, while it is open (D191).
    pub(crate) contexts_dialog: Option<ContextsDialog>,
    pub(crate) labels_view: LabelsView,
    /// The create-worktree dialog, while it is open.
    pub(crate) worktree_dialog: Option<WorktreeDialog>,
    /// The remove-worktree dialog, while it is open.
    pub(crate) removal_dialog: Option<RemoveDialog>,
    /// The New Workspace dialog while it is open (D183).
    pub(crate) workspace_dialog: Option<WorkspaceDialog>,
    /// The Remove Workspace dialog while it is open (D183).
    pub(crate) workspace_removal_dialog: Option<RemoveWorkspaceDialog>,
    /// The prune dialog, while it is open.
    pub(crate) prune_dialog: Option<PruneDialog>,
    /// The new-branch dialog, while it is open.
    pub(crate) new_branch_dialog: Option<NewBranchDialog>,
    /// The branch Set Parent… was run on, while the palette picks its parent.
    pub(crate) stack_subject: Option<String>,
    /// The Inbox item selected, by its key (UI redesign, slice 6).
    pub(crate) inbox_selected: Option<String>,
    /// The History scope pill's menu is open.
    pub(crate) history_scope_menu: bool,
    /// The stash dialog, while it is open (B2).
    pub(crate) new_stash_dialog: Option<NewStashDialog>,
    /// The review comment composer, while it is open (B4c).
    pub(crate) review_dialog: Option<ReviewDialog>,
    /// The Run review picker of the Review page, while it is open (S5, D219).
    pub(crate) review_run: Option<crate::review_run::RunDialog>,
    /// The inline Reply box under a thread on the Local review page (D198).
    pub(crate) review_reply: Option<InlineReply>,
    /// The inputs dialog of a workflow that declares inputs, while it is open.
    pub(crate) workflow_inputs_dialog: Option<WorkflowInputsDialog>,
    /// The "Run this version?" dialog, while the engine waits for a review of a script.
    pub(crate) workflow_review_dialog: Option<WorkflowReviewDialog>,
    /// The editor of the Workflows page's Script tab; its text is the page's buffer.
    pub(crate) workflow_source: Entity<InputState>,
    /// The subscription to `workflow_source`'s changes; replaced with the editor.
    pub(crate) workflow_source_sub: Subscription,
    /// Set when the page's buffer is another document than the editor's: the editor is then
    /// rebuilt on the next frame, so its undo history starts with the new file.
    pub(crate) workflow_editor_stale: bool,
    /// The Result pane of the conflict resolver; its text is the resolver's result (C3b).
    pub(crate) conflict_result: Entity<TextArea>,
    /// The delete-branch dialog, while it is open.
    pub(crate) delete_dialog: Option<DeleteDialog>,
    /// The discard dialog, while it is open.
    pub(crate) discard_dialog: Option<DiscardDialog>,
    /// The branch popover of a move, while it is open: its inputs (D109).
    pub(crate) move_popover: Option<MovePopover>,
    /// The drop dialog of a stash, while it is open (B2).
    pub(crate) drop_stash_dialog: Option<DropStashDialog>,
    /// The field dialog of a thread (rename, note, tags, link, export), while it is open (T6).
    pub(crate) thread_field: Option<FieldDialog>,
    /// The Link to Thread dialog, open while its picker is (D143).
    pub(crate) thread_link: Option<LinkDialog>,
    /// The confirmation of a thread delete or of a scrub, while it is open (T6).
    pub(crate) thread_confirm: Option<ConfirmDialog>,
    /// The New Label or Rename Label dialog, while it is open (P4).
    pub(crate) label_form: Option<crate::shell_labels::LabelForm>,
    /// The confirmation of a label delete, while it is open (P4).
    pub(crate) label_confirm: Option<crate::shell_labels::LabelConfirm>,
    /// The Threads settings dialog, while it is open (T7).
    pub(crate) threads_settings: Option<ThreadsSettingsDialog>,
    /// The message shown when a stash apply or pop, or a detached checkout, was not done.
    pub(crate) message_dialog: Option<MessageDialog>,
    /// The delete-merged-branches dialog, while it is open.
    pub(crate) merged_dialog: Option<MergedDialog>,
    /// The restack dialog, while it is open.
    pub(crate) restack_dialog: Option<RestackDialog>,
    /// The screen the restack conflict was opened from, which the dialog returns to (D154).
    pub(crate) restack_return: Screen,
    /// The recent repositories, newest first, as last saved or loaded.
    pub(crate) recents: Recents,
    /// Where the list is kept; `None` when there is no home directory.
    recent_store: Option<Arc<recent::Store>>,
    /// Numbers the saves so an older one never lands after a newer one.
    recent_generation: u64,
    /// The app's preferences file (D40); `None` when there is no home directory.
    settings_store: Option<Arc<app_settings::Store>>,
    settings_generation: u64,
    /// The open that starts the window, until it succeeds or fails.
    pub(crate) startup: Option<Startup>,
    /// Set while the window shows the welcome screen instead of a repository.
    pub(crate) welcome: Option<Welcome>,
    /// Asks the watcher for a refresh when the window gains focus.
    watch: WatchHandle,
    _subscriptions: Vec<Subscription>,
}

/// Whether the event ends a remote operation (a fetch, a pull or a push) that the model owns.
fn is_remote_end(event: &Event, model: &Model) -> bool {
    matches!(
        event,
        Event::OperationChanged { id, state, .. }
            if state.is_terminal() && model.remote.owns(*id)
    )
}

impl StatusView {
    /// Opens `path` in the engine. Events are streamed into the view from a
    /// task on the UI thread; the subscription is made before the command so
    /// no event is missed.
    #[allow(clippy::too_many_lines)] // the window's whole initial state
    pub fn new(
        engine: EngineHandle,
        command_log: CommandLog,
        watch: (WatchHandle, Notices),
        start: StartPath,
        features: FeatureSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (watch, notices) = watch;
        let events = engine.subscribe();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update_in(cx, |view, window, cx| view.on_event(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self::follow_command_log(&command_log, cx);
        Self::follow_watch(notices, cx);
        Self::follow_background_fetch(cx);
        let may_fall_back = !start.from_argument;
        // The preferences are read where the window starts, so a repository's own `gitmaster.*`
        // keys override the global ones for it.
        let start_dir = std::path::PathBuf::from(&start.path);
        let open = engine.dispatch(Actor::Human, CommandKind::OpenRepo { path: start.path });
        if theme::follow(window.appearance()) {
            editor_theme::apply(cx);
        }
        let list_focus = cx.focus_handle();
        list_focus.focus(window);
        let code_focus = cx.focus_handle();
        let recent_store = recent::default_file().map(|file| Arc::new(recent::Store::new(file)));
        let recents = recent_store
            .as_ref()
            .map(|store| store.load())
            .unwrap_or_default();
        let project_menu = ProjectMenu::new(cx.focus_handle());
        let branch_filter = cx.new(|cx| TextArea::new("Filter branches", cx).with_lines(1, 1));
        let branch_menu = BranchMenu::new(branch_filter.clone());
        let palette_filter =
            cx.new(|cx| TextArea::new("Type a command, branch or file", cx).with_lines(1, 1));
        let quick_open_input = cx.new(|cx| {
            TextArea::new("Find a file by name  (name:line opens at a line)", cx).with_lines(1, 1)
        });
        let search_query =
            cx.new(|cx| TextArea::new("Search the text of the files", cx).with_lines(1, 1));
        let search_paths = cx.new(|cx| {
            TextArea::new("Files to search: src crates/ui !docs  (empty: all)", cx).with_lines(1, 1)
        });
        let health_query = cx
            .new(|cx| TextArea::new("Filter by folder, such as src/billing", cx).with_lines(1, 1));
        let threads_query = cx.new(|cx| TextArea::new("Search the threads", cx).with_lines(1, 1));
        let memories_query = cx.new(|cx| TextArea::new("Search the memories", cx).with_lines(1, 1));
        let memory_path =
            cx.new(|cx| TextArea::new("A file path, such as src/lib.rs", cx).with_lines(1, 1));
        let memory_mark_reason =
            cx.new(|cx| TextArea::new("Why, in one line (optional)", cx).with_lines(1, 1));
        let labels_query = cx.new(|cx| TextArea::new("Filter the labels", cx).with_lines(1, 1));
        let sidebar_input = cx.new(|cx| TextArea::new("Find a branch", cx).with_lines(1, 1));
        let composer = cx.new(Composer::new);
        let diff_pane = cx.new(DiffPane::new);
        let commit_diff_pane = cx.new(DiffPane::read_only);
        let compare_diff_pane = cx.new(DiffPane::read_only);
        let workflow_source = workflow_editor::new_editor("", window, cx);
        let workflow_source_sub = workflow_editor::subscribe(&workflow_source, window, cx);
        let conflict_result = cx.new(|cx| {
            TextArea::new("Result", cx)
                .with_lines(24, usize::MAX)
                .with_style(TextStyle {
                    mono: true,
                    ..TextStyle::default()
                })
        });
        // The saved preferences are read here, before the first frame, so the
        // commit diff opens in the layout the user last chose (D40).
        let settings_store =
            app_settings::default_file().map(|file| Arc::new(app_settings::Store::new(file)));
        let settings = settings_store
            .as_ref()
            .map(|store| store.load(&start_dir))
            .unwrap_or_default();
        let avatars = Avatars::new(
            settings_store
                .as_ref()
                .and_then(|store| store.dir())
                .map(|dir| dir.join("avatars")),
            settings.author_pictures,
        );
        let commit_texts = CommitTexts::new(avatars, cx);
        // Both diffs open in the saved layout (D39, D40).
        diff_pane.update(cx, |pane, cx| pane.set_layout(settings.diff_layout, cx));
        commit_diff_pane.update(cx, |pane, cx| pane.set_layout(settings.diff_layout, cx));
        compare_diff_pane.update(cx, |pane, cx| pane.set_layout(settings.diff_layout, cx));
        // The commit message box's focus: an empty box with something staged asks the workflows (`WF3d`).
        let composer_focus = composer.read(cx).focus_handle(cx);
        let mut subscriptions = vec![
            cx.observe(&branch_filter, |this, filter, cx| {
                this.on_branch_filter_changed(&filter, cx);
            }),
            cx.observe(&conflict_result, |this, _, cx| {
                this.on_conflict_result_changed(cx);
            }),
            cx.subscribe_in(
                &branch_filter,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    this.on_branch_filter_event(*event, window, cx);
                },
            ),
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &ComposerEvent, window, cx| {
                    this.on_composer_event(*event, window, cx);
                },
            ),
            cx.subscribe_in(
                &diff_pane,
                window,
                |this, _, event: &DiffPaneEvent, window, cx| {
                    this.on_diff_pane_event(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &commit_diff_pane,
                window,
                |this, _, event: &DiffPaneEvent, window, cx| {
                    this.on_diff_pane_event(event, window, cx);
                },
            ),
            // Escape in the compare diff closes its file; the history's close is for its own pane.
            cx.subscribe_in(
                &compare_diff_pane,
                window,
                |this, _, event: &DiffPaneEvent, window, cx| match event {
                    DiffPaneEvent::LeaveCommit => this.close_compare_file(cx),
                    other => this.on_diff_pane_event(other, window, cx),
                },
            ),
            // Coming back to the window is a reason to refresh; the watcher debounces it.
            cx.observe_window_activation(window, |this, window, _cx| {
                if window.is_window_active() {
                    this.watch.request();
                    this.request_hook_sections();
                }
            }),
            cx.on_focus_in(&composer_focus, window, |this, _, cx| {
                this.on_composer_focus(cx);
            }),
            // Also fires when the compositor answers the decoration request, so
            // the title bar appears or goes as soon as the grant is known.
            cx.observe_window_appearance(window, |_, window, cx| {
                if theme::follow(window.appearance()) {
                    editor_theme::apply(cx);
                }
                cx.notify();
            }),
        ];
        subscriptions.extend(palette_view::subscriptions(&palette_filter, window, cx));
        subscriptions.extend(quick_open_view::subscriptions(
            &quick_open_input,
            window,
            cx,
        ));
        subscriptions.extend(shell_search::subscriptions(
            &search_query,
            &search_paths,
            cx,
        ));
        subscriptions.extend(shell_health::subscriptions(&health_query, cx));
        subscriptions.extend(shell_threads::subscriptions(&threads_query, cx));
        subscriptions.extend(shell_memories::subscriptions(&memories_query, cx));
        subscriptions.extend(shell_labels::subscriptions(&labels_query, cx));
        subscriptions.extend(shell_sidebar::subscriptions(&sidebar_input, window, cx));
        let mut model = Model::default();
        model.request_open(open);
        // The workspaces under the root are listed when the window starts (D183 (2)).
        let listing = engine.dispatch(Actor::Human, CommandKind::ListWorkspaces { root: None });
        model.workspaces.request(listing);
        Self {
            engine,
            command_log,
            log_open: false,
            log_scroll: ScrollHandle::new(),
            model,
            list_focus,
            code_focus,
            scroll: UniformListScrollHandle::new(),
            composer,
            diff_pane,
            commit_diff_pane,
            compare_diff_pane,
            sidebar_width: resize::SIDEBAR_DEFAULT,
            commit_texts,
            left_width: resize::LEFT_DEFAULT,
            code_tree_width: resize::CODE_TREE_DEFAULT,
            workflow_list_width: resize::WORKFLOW_LIST_DEFAULT,
            thread_list_width: resize::THREAD_LIST_DEFAULT,
            commit_height: resize::COMMIT_DEFAULT,
            features,
            sidebar_folded: Folded::default(),
            sidebar_filter: BranchFilter::default(),
            sidebar_input,
            settings,
            screen: Screen::default(),
            project_menu,
            branch_menu,
            context_menu: ContextMenu::default(),
            palette: Palette::default(),
            palette_view: PaletteView::new(palette_filter),
            quick_open_view: QuickOpenView::new(quick_open_input),
            code_view: CodeView::new(),
            search_view: SearchView::new(search_query, search_paths),
            health_view: HealthView::new(health_query, cx.focus_handle()),
            threads_view: ThreadsView::new(threads_query),
            memories_view: MemoriesView::new(memories_query, memory_path, memory_mark_reason),
            threads_tab: ThreadsTab::default(),
            memory_dialog: None,
            contexts_dialog: None,
            labels_view: LabelsView::new(labels_query),
            worktree_dialog: None,
            removal_dialog: None,
            workspace_dialog: None,
            workspace_removal_dialog: None,
            prune_dialog: None,
            new_branch_dialog: None,
            stack_subject: None,
            inbox_selected: None,
            history_scope_menu: false,
            new_stash_dialog: None,
            review_dialog: None,
            review_run: None,
            review_reply: None,
            workflow_inputs_dialog: None,
            workflow_review_dialog: None,
            workflow_source,
            workflow_source_sub,
            workflow_editor_stale: true,
            conflict_result,
            delete_dialog: None,
            discard_dialog: None,
            move_popover: None,
            drop_stash_dialog: None,
            thread_field: None,
            thread_link: None,
            thread_confirm: None,
            label_form: None,
            label_confirm: None,
            threads_settings: None,
            message_dialog: None,
            merged_dialog: None,
            restack_dialog: None,
            restack_return: Screen::default(),
            recents,
            recent_store,
            recent_generation: 0,
            settings_store,
            settings_generation: 0,
            startup: Some(Startup {
                id: open,
                may_fall_back,
            }),
            welcome: None,
            watch,
            _subscriptions: subscriptions,
        }
    }

    /// Fetches in the background every `BACKGROUND_FETCH` while a branch with an upstream is
    /// open and nothing remote runs (UI redesign, slice 1). Its end is not shown; a failure is
    /// in the Git log.
    fn follow_background_fetch(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(sync_state::BACKGROUND_FETCH)
                    .await;
                if this.update(cx, StatusView::background_fetch).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn background_fetch(&mut self, cx: &mut Context<Self>) {
        let due = sync_state::background_fetch_due(self.standing(), self.model.status().is_some());
        if !due || self.busy_reason().is_some() {
            return;
        }
        let id = self.engine.dispatch(Actor::Human, CommandKind::Fetch);
        self.model.remote.begin_quiet(id);
        cx.notify();
    }

    /// Applies the watcher's notices on the UI thread: the refresh indicator
    /// and, once, the notice that watching is limited.
    fn follow_watch(notices: Notices, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Some(notice) = notices.next().await {
                if this
                    .update(cx, |view, cx| view.on_watch_notice(&notice, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_watch_notice(&mut self, notice: &Notice, cx: &mut Context<Self>) {
        match notice {
            Notice::Refreshing(id) => self.start_refresh_indicator(*id, cx),
            Notice::Degraded { .. } => self.notify_user(WATCH_LIMITED, cx),
            Notice::Watching { .. } => {}
        }
    }

    /// Tracks a refresh and shows "Updating…" only if it outlasts `UPDATING_DELAY`.
    fn start_refresh_indicator(&mut self, id: OperationId, cx: &mut Context<Self>) {
        if !self.model.begin_refresh(id) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UPDATING_DELAY).await;
            let _ = this.update(cx, |view, cx| {
                if view.model.refresh_is_slow(id) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Repaints when the adapter records a command, and keeps an open panel
    /// scrolled to the newest one.
    fn follow_command_log(log: &CommandLog, cx: &mut Context<Self>) {
        let changes = log.changes();
        cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                if this.update(cx, Self::on_command_logged).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether `event` is a restack a workflow made while the restack dialog shows a plan, which
    /// the plan may now be out of date for (`WF4c`). Read before the event is applied.
    fn workflow_moved_under_dialog(&self, event: &Event) -> bool {
        matches!(
            event,
            Event::RestackFinished { id, .. }
                if self.model.restacking.outdated_by(*id) && self.restack_dialog.is_some()
        )
    }

    fn on_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let reload = matches!(event, Event::StatusLoaded { .. });
        let stacks_loaded = matches!(event, Event::StacksLoaded { .. });
        let moved_by_workflow = self.workflow_moved_under_dialog(&event);
        let listed = matches!(event, Event::BranchesLoaded { .. });
        let stashes_listed = matches!(event, Event::StashesListed { .. });
        let threads_flags = shell_threads::follow_flags(&event);
        let relation_changed = shell_threads::changed_relation(&event);
        let labels_changed = shell_labels::follows_change(&event);
        let review_listed = matches!(event, Event::ReviewCommentsListed { .. });
        let stash_stale = stashes::is_stale_refusal(&event);
        let committed = matches!(event, Event::Committed { .. });
        let detail_loaded = matches!(event, Event::CommitDetailLoaded { .. });
        let file_event = is_file_event(&event);
        let transform = transform_answer(&event);
        self.model.transforms.settle_event(&event);
        let workflow_refresh = self.on_workflow_event(&event, cx);
        let workflows_listed = matches!(event, Event::WorkflowsListed { .. });
        let remote_ended = is_remote_end(&event, &self.model);
        let head_before = self.head_name();
        let failed_start = self.take_settled_startup(&event);
        let converted = convert_answer(&event);
        self.follow_memories(&event, cx);
        self.follow_contexts(&event, cx);
        let shown = self.model.apply(event);
        self.ask_health_again();
        if moved_by_workflow {
            self.replan_after_workflow_restack(cx);
        }
        if let Some(text) = converted {
            self.open_message(CONVERT_TITLE.to_owned(), text, window, cx);
        }
        if stacks_loaded && self.model.restacking.take_resume() {
            self.engine
                .dispatch(Actor::Human, CommandKind::ResumeRestack);
        }
        // A read that opened the resolver shows its page.
        if self.model.take_conflict_opened().is_some() {
            self.show_conflicts(cx);
        }
        // A restack conflict opened or closed: the dialog and the resolver take turns (D154).
        match self.model.restacking.take_transition() {
            Some(Transition::Opened) => self.open_restack_resolver(cx),
            Some(Transition::Returned) => self.return_to_restack_dialog(window, cx),
            None => {}
        }
        self.reread_forgotten_conflict();
        self.follow_review(review_listed, reload, cx);
        if stash_stale {
            self.request_stash_list();
        }
        self.follow_threads(threads_flags, reload, cx);
        self.follow_relation(relation_changed.as_ref());
        self.follow_labels(labels_changed);
        if detail_loaded {
            self.show_commit_texts(cx);
        }
        if file_event {
            self.after_file_event(cx);
        }
        self.show_fresh_commit_stacks(window, cx);
        self.show_fresh_compare_stack(window, cx);
        self.settle_dialogs(window, cx);
        // A transform's answer follows the same rule for the composer and for the branch name (`WF3d`).
        if let Some((id, value, changed)) = transform {
            self.on_transform_finished(id, value, changed, cx);
        }
        // A workflow's message follows the suggestion rule: an empty composer takes it, otherwise
        // it waits as the suggestion, which Replace and Discard answer.
        // On the welcome screen the error belongs to the screen, not the status bar.
        if let Some(welcome) = self.welcome.as_mut()
            && let Some(error) = self.model.error.take()
        {
            welcome.error = Some(error);
        }
        if committed {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
            Self::expire_notice_later(self.model.notice_generation(), cx);
        }
        if remote_ended {
            Self::expire_notice_later(self.model.notice_generation(), cx);
        }
        if let Some(switched) = self.model.take_switched() {
            self.welcome = None;
            self.on_switched(switched, cx);
        }
        if let Some(startup) = failed_start {
            self.on_start_failed(startup, cx);
        }
        self.follow_workflows(workflows_listed, cx);
        if let Some(diffs) = shown {
            self.show_diffs(diffs.unstaged, diffs.staged, cx);
        }
        // The palette's offers come from the status, the branch and workflow listings.
        if self.palette.open && (reload || listed || stashes_listed || workflow_refresh) {
            self.refresh_palette(cx);
        }
        if reload {
            self.after_status(head_before.as_deref(), cx);
        }
        // The watcher refreshes the status when HEAD or a ref moves; the open
        // history re-reads its top then, so an outside commit shows up.
        self.follow_history(reload, stacks_loaded);
        // The Stashes page reads its list again after every status refresh (B2). A
        // repository open ends with a status too, so it reads the list then.
        if reload && self.screen == Screen::Stashes {
            self.request_stash_list();
        }
        // The Reflog page reads its first page again after every status refresh too (B3).
        if reload && self.screen == Screen::Reflog {
            self.request_reflog_top();
        }
        cx.notify();
    }

    /// The review comments follow the engine: the diff pane gets each listing, and
    /// every status refresh asks for the list again (B4c).
    fn follow_review(&mut self, listed: bool, reload: bool, cx: &mut Context<Self>) {
        if listed {
            self.sync_review_comments(cx);
            // A publish always lists the comments after it, so the scorecards follow here (S5).
            self.request_scorecards();
            // The After text is read from the file, so it follows each new listing (B4d).
            if self.screen == Screen::LocalReview {
                self.request_review_texts();
            }
        }
        if reload {
            self.request_review_list();
        }
    }

    /// Shows the commit stack each page's read has just filled, in the commit diff pane.
    fn show_fresh_commit_stacks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for page in [CommitPage::History, CommitPage::Stashes, CommitPage::Reflog] {
            if let Some(fresh) = self.model.commit_stack_mut(page).take_fresh() {
                self.commit_diff_pane.update(cx, |pane, cx| {
                    pane.show_stack(
                        fresh.files,
                        fresh.more,
                        fresh.diffs,
                        fresh.focus,
                        window,
                        cx,
                    );
                });
            }
        }
    }

    /// The Branch and Compare tabs' stacked diff: the request a range read asked for is sent, and
    /// the diffs that arrived are shown in the compare pane (D231 (4)).
    fn show_fresh_compare_stack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = self.model.compare.take_stack_request() {
            let id = self.engine.dispatch(
                Actor::Human,
                CommandKind::LoadRevisionStack {
                    base: request.base,
                    tip: request.tip,
                    files: request.files.clone(),
                },
            );
            self.model
                .compare_stack
                .request(id, 0, request.files, request.more);
        }
        if let Some(fresh) = self.model.compare_stack.take_fresh() {
            self.compare_diff_pane.update(cx, |pane, cx| {
                pane.show_stack(
                    fresh.files,
                    fresh.more,
                    fresh.diffs,
                    fresh.focus,
                    window,
                    cx,
                );
            });
        }
    }

    /// Shows the outcome of each dialog's operation that the model settled.
    fn settle_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_pending_workflow_review(window, cx);
        if let Some(outcome) = self.model.creating.take() {
            self.on_worktree_created(outcome, window, cx);
        }
        if let Some(outcome) = self.model.removal.take_outcome() {
            self.on_worktree_removal_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.workspace_making.take() {
            self.on_workspace_created(outcome, window, cx);
        }
        if let Some(outcome) = self.model.workspace_removal.take_outcome() {
            self.on_workspace_removal_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.pruning.take_outcome() {
            self.on_worktrees_pruned(outcome, window, cx);
        }
        if let Some(outcome) = self.model.branch_create.take() {
            self.on_branch_created(outcome, window, cx);
        }
        if let Some(outcome) = self.model.branch_delete.take() {
            self.on_branch_delete_settled(outcome, window, cx);
        }
        self.sync_move_popover(window);
        if let Some(outcome) = self.model.discarding.take() {
            self.on_file_discard_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.restacking.take_outcome() {
            self.on_restack_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.merging.take_outcome() {
            self.on_merged_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.stash_create.take() {
            self.on_stash_created(outcome, window, cx);
        }
        if let Some(outcome) = self.model.review_write.take() {
            self.on_review_written(outcome, window, cx);
        }
        if let Some(outcome) = self.model.stash_apply.take() {
            self.on_stash_applied(outcome, window, cx);
        }
        if let Some(outcome) = self.model.thread_work.take() {
            self.on_thread_work(outcome, window, cx);
        }
        if let Some(outcome) = self.model.label_writes.take() {
            self.on_label_write(outcome, window, cx);
        }
        if let Some(outcome) = self.model.stash_drop.take() {
            self.on_stash_dropped(outcome, window, cx);
        }
        if let Some(outcome) = self.model.reflog_checkout.take() {
            self.on_reflog_checkout_settled(outcome, window, cx);
        }
    }

    /// The row under the top bar: the sidebar and the page the screen names.
    fn main_area(
        &self,
        sidebar_width: f32,
        left_width: f32,
        commit_height: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_row()
            .child(self.sidebar(sidebar_width, cx))
            .children(
                (self.screen == Screen::WorkingCopy)
                    .then(|| self.left_column(left_width, commit_height, cx)),
            )
            .children(
                (self.screen == Screen::WorkingCopy
                    && self.model.compare.tab == ChangesTab::Uncommitted)
                    .then(|| {
                        div().flex_1().min_w(px(0.)).min_h(px(0.)).child(
                            if self.model.selected_submodule().is_some() {
                                self.submodule_pane(cx).into_any_element()
                            } else {
                                self.diff_pane.clone().into_any_element()
                            },
                        )
                    }),
            )
            .children(
                (self.screen == Screen::WorkingCopy
                    && self.model.compare.tab != ChangesTab::Uncommitted)
                    .then(|| self.compare_diff_area()),
            )
            .children((self.screen == Screen::Inbox).then(|| self.inbox_page(cx)))
            .children((self.screen == Screen::History).then(|| self.history_page(cx)))
            .children((self.screen == Screen::Workflows).then(|| self.workflows_page(cx)))
            .children((self.screen == Screen::Code).then(|| self.code_page(cx)))
            .children((self.screen == Screen::Search).then(|| self.search_page(cx)))
            .children((self.screen == Screen::Health).then(|| self.health_page(cx)))
            .children((self.screen == Screen::Stashes).then(|| self.stashes_page(cx)))
            .children((self.screen == Screen::Reflog).then(|| self.reflog_page(cx)))
            .children((self.screen == Screen::Threads).then(|| self.threads_page(cx)))
            .children((self.screen == Screen::Labels).then(|| self.labels_page(cx)))
            .children((self.screen == Screen::LocalReview).then(|| self.local_review_page(cx)))
            .children((self.screen == Screen::Conflicts).then(|| self.conflicts_page(cx)))
    }

    pub(crate) fn head_name(&self) -> Option<String> {
        self.model
            .status()
            .and_then(|status| status.branch.head.clone())
    }

    /// Asks for the branch list the sidebar draws. Not while the picker waits
    /// for its own: a newer request would make that reply stale.
    pub(crate) fn request_branch_list(&mut self) {
        if self.branch_menu.open {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListBranches);
        self.model.branches.request(id);
    }

    /// What every status refresh asks for again: the stacks, the branch list and
    /// the Git config's hook sections when HEAD moved (the tree marks the new branch), and the diff.
    fn after_status(&mut self, head_before: Option<&str>, cx: &mut Context<Self>) {
        self.request_stacks();
        self.request_submodules();
        self.request_review_files();
        if matches!(self.screen, Screen::Code | Screen::Search | Screen::Health) {
            self.request_health();
        }
        if self.head_name().as_deref() != head_before {
            self.request_branch_list();
            self.request_hook_sections();
        }
        self.load_selected(cx);
        // The Branch and Compare tabs follow the repository: a commit, a fetch or a checkout (D231).
        self.read_changes_tab(cx);
    }

    /// Asks for the branch stacks the By stack view draws (D138), after every status
    /// refresh, since any moved ref can change a stack's states. Not while a read is
    /// in flight: its answer is as fresh, since the engine runs in order.
    pub(crate) fn request_stacks(&mut self) {
        if self.model.status().is_none() || self.model.stacks.is_waiting() {
            return;
        }
        let id = self.engine.dispatch(Actor::Human, CommandKind::LoadStacks);
        self.model.stacks.request(id);
    }

    /// Asks the engine for the `prepare-commit` sections of the Git config (`WF3e`), for Convert
    /// Hooks to Workflows. Nothing is asked before a repository is open.
    pub(crate) fn request_hook_sections(&mut self) {
        if self.model.status().is_some() {
            self.engine
                .dispatch(Actor::Human, CommandKind::ListHookSections);
        }
    }

    /// The workflow events the window acts on, before the model takes them. A
    /// suggested message goes to the composer or waits as a suggestion, and a
    /// finished run asks for the listing again. Returns whether the palette's
    /// offers change with the event.
    fn on_workflow_event(&mut self, event: &Event, cx: &mut Context<Self>) -> bool {
        match event {
            Event::WorkflowMessageSuggested { message, .. } => {
                self.place_workflow_message(message, cx);
                false
            }
            Event::WorkflowRunFinished { run, .. } => {
                self.request_workflow_list();
                self.workflow_run_ended(*run, cx);
                true
            }
            // A save refused because the file changed on disk: the listing is read
            // again, so Overwrite expects the hash now on disk.
            Event::OperationChanged {
                state: OperationState::Failed,
                detail: Some(detail),
                ..
            } if self.model.workflows_page.is_saving() && is_conflict(detail) => {
                self.request_workflow_list();
                false
            }
            // The new file joins the listing, and its folder opens so the user can edit it.
            Event::WorkflowCreated { path, .. } => {
                self.request_workflow_list();
                if let Some(folder) = std::path::Path::new(path).parent() {
                    cx.reveal_path(folder);
                }
                false
            }
            // A copy of a built-in is selected once the listing has it; its folder stays closed.
            Event::WorkflowCopied { .. } => {
                self.request_workflow_list();
                false
            }
            Event::WorkflowsListed { .. }
            | Event::WorkflowRunChanged { .. }
            | Event::WorkflowApprovalRequested { .. }
            | Event::WorkflowWantsReview { .. }
            | Event::WorkflowRequestDropped { .. }
            | Event::WorkflowGateUnreviewed { .. } => true,
            _ => false,
        }
    }

    /// The Workflows page after the engine's answer is in the model: the first
    /// listing opens the first workflow, and the buffer shows what the page holds.
    fn follow_workflows(&mut self, listed: bool, cx: &mut Context<Self>) {
        if !listed {
            self.sync_workflow_source(cx);
            return;
        }
        self.bind_workflow_keys(cx);
        // A template just copied opens in the page once the listing has it, so the edit can
        // start; the page keeps it open when the screen is shown later.
        let fresh = self.model.workflows.fresh.clone().filter(|path| {
            self.model
                .workflows
                .listed
                .as_deref()
                .is_some_and(|listed| listed.iter().any(|info| info.path == *path))
        });
        if let Some(path) = fresh {
            self.model.workflows.fresh = None;
            self.select_workflow(&path, cx);
        }
        if self.screen == Screen::Workflows {
            self.open_first_workflow(cx);
        }
        self.sync_workflow_source(cx);
    }

    /// Puts a message a workflow suggested in the composer when it is empty, and
    /// otherwise offers it as the suggestion, which Replace and Discard answer.
    fn place_workflow_message(&mut self, message: &str, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).text(cx);
        match fill_for_message(&typed, message) {
            Fill::Apply(text) => self.composer.update(cx, |c, cx| c.set_text(&text, cx)),
            Fill::Suggest(text) => self.model.suggestion.offer(text),
        }
    }

    /// The commit message box got focus (`WF3d`, D155(1)): an empty box with something staged
    /// asks the engine once per staged state, which fires `composer.opened` and runs the
    /// commit message transforms. Staging alone asks nothing.
    fn on_composer_focus(&mut self, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).text(cx);
        let staged = self.model.status().and_then(|status| {
            transform_run::staged_key(
                status.branch.oid.as_deref(),
                status
                    .entries
                    .iter()
                    .filter(|entry| entry.staged.is_some())
                    .map(|entry| entry.path.as_str()),
            )
        });
        if !self
            .model
            .transforms
            .composer_opened(&typed, staged.as_deref())
        {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ComposerOpened);
        self.model.transforms.start(id, Field::CommitMessage, typed);
    }

    /// Cancel of a transform in flight for `field` (`WF3d`): the engine stops its run and answers
    /// with no value, so the field stays as it was and nothing is reported as an error.
    pub(crate) fn cancel_transform(&mut self, field: Field, cx: &mut Context<Self>) {
        if let Some(op) = self.model.transforms.cancel_target(field) {
            self.engine
                .dispatch(Actor::Human, CommandKind::CancelOperation { op });
        }
        cx.notify();
    }

    /// A transform answered (`WF3d`). The commit message goes to the composer when its text is as it
    /// was sent and nothing changed, and otherwise waits as the suggestion, which Replace and
    /// Discard answer. The branch name fills the New Branch dialog the same way, or waits as its
    /// suggestion. No value does nothing.
    fn on_transform_finished(
        &mut self,
        id: OperationId,
        value: Option<String>,
        changed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.model.transforms.field_of(id) else {
            return;
        };
        match field {
            Field::CommitMessage => {
                let typed = self.composer.read(cx).text(cx);
                match self.model.transforms.finish(id, &typed, value, changed) {
                    Some((_, Fill::Apply(text))) => self
                        .composer
                        .update(cx, |composer, cx| composer.set_text(&text, cx)),
                    Some((_, Fill::Suggest(text))) => self.model.suggestion.offer(text),
                    None => {}
                }
            }
            Field::BranchName => {
                let typed = self
                    .new_branch_dialog
                    .as_ref()
                    .map(|dialog| dialog.name.read(cx).text().to_owned())
                    .unwrap_or_default();
                let fill = self.model.transforms.finish(id, &typed, value, changed);
                if let (Some(dialog), Some((_, fill))) = (self.new_branch_dialog.as_mut(), fill) {
                    match fill {
                        Fill::Apply(text) => {
                            dialog.name.update(cx, |area, cx| area.set_text(&text, cx));
                        }
                        Fill::Suggest(text) => dialog.name_suggestion = Some(text),
                    }
                }
            }
        }
        cx.notify();
    }

    /// Asks the engine for the manual workflows of the open repository. Nothing
    /// is asked before one is open.
    pub(crate) fn request_workflow_list(&mut self) {
        if self.model.status().is_some() {
            self.engine
                .dispatch(Actor::Human, CommandKind::ListWorkflows);
        }
    }

    /// A successful open replaced the repository: the view state of the old
    /// one goes, and the new root joins the recent list.
    fn on_switched(&mut self, switched: Switched, cx: &mut Context<Self>) {
        // The comments of the repository left are gone with the model's reset.
        self.sync_review_comments(cx);
        self.model.commit_stack.close();
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        if !switched.same_root {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
        }
        self.show_diffs(None, None, cx);
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.model.branches.clear();
        self.model.suggestion.drop_suggestion();
        self.model.hook_sections.clear();
        self.model.transforms.reset();
        self.model.workflows.reset();
        self.model.workflows_page.reset();
        self.request_branch_list();
        self.request_hook_sections();
        self.request_workflow_list();
        if self.screen == Screen::History {
            self.refresh_history();
        }
        if self.screen == Screen::Code {
            self.show_code(cx);
        }
        if self.screen == Screen::Health {
            self.request_health();
        }
        // The boxes of the search and the health filter belong to the repository left behind.
        for area in [
            &self.search_view.query,
            &self.search_view.paths,
            &self.health_view.query,
        ] {
            area.update(cx, |area, cx| area.set_text("", cx));
        }
        self.remember(PathBuf::from(switched.root), cx);
    }

    /// Opens the history page and asks for the commits of HEAD. The Stashes
    /// page's stacked diff closes, since the diff pane is shared.
    pub(crate) fn show_history(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::History;
        self.model.stash_stack.close();
        self.model.reflog_stack.close();
        self.show_commit_texts(cx);
        self.refresh_history();
        cx.notify();
    }

    /// Shows the Working Copy again; the history listing is kept for the next visit.
    pub(crate) fn show_working_copy(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::WorkingCopy;
        cx.notify();
    }

    /// Shows the Changes page on `tab` (D231 (1)) and reads what the tab shows. A change of tab
    /// drops the stacked diff of the old one and closes the Base picker.
    pub(crate) fn show_changes_tab(&mut self, tab: ChangesTab, cx: &mut Context<Self>) {
        if tab != self.model.compare.tab {
            self.model.compare.forget_stack();
            self.model.compare_stack.close();
        }
        self.model.compare.set_tab(tab);
        self.screen = Screen::WorkingCopy;
        self.read_changes_tab(cx);
        cx.notify();
    }

    /// Asks the engine for the read the shown tab needs (D231 (4b)): the Branch tab's range, or the
    /// Compare tab's once both fields are filled. Uncommitted reads nothing here. A read already in
    /// flight is superseded by the engine, so a new one is sent without checking.
    pub(crate) fn read_changes_tab(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.model.compare.active_command() else {
            return;
        };
        let id = self.engine.dispatch(Actor::Human, kind);
        match self.model.compare.tab {
            ChangesTab::Branch => self.model.request_branch_view(id),
            ChangesTab::Compare => self.model.request_compare_view(id),
            ChangesTab::Uncommitted => {}
        }
        cx.notify();
    }

    /// Replaces the composer text with the suggestion, which is then dropped.
    fn use_suggestion(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_text(text, cx));
        self.model.suggestion.drop_suggestion();
    }

    /// Shows the Inbox. Its items come from the state the window holds; the review comments
    /// are read again, since only the Review page reads them otherwise.
    pub(crate) fn show_inbox(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Inbox;
        self.request_review_list();
        cx.notify();
    }

    /// Asks for the first page of the scope to read now (D139). A list already
    /// shown stays until the answer comes.
    pub(crate) fn refresh_history(&mut self) {
        let head = self.head_name();
        let scope = self
            .model
            .history_scope
            .effective(head.as_deref(), &self.model.stacks);
        self.model.history_scope.loaded(scope.clone());
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadHistory { scope });
        self.model.history.request_top(id);
    }

    /// Reads the History list again when it is open and a status refresh came, or
    /// a stack snapshot changed the scope to read (the first one does, when HEAD
    /// is in a stack).
    fn follow_history(&mut self, reload: bool, stacks_loaded: bool) {
        if self.screen != Screen::History || !self.model.history.can_refresh() {
            return;
        }
        let head = self.head_name();
        let stale = stacks_loaded
            && self
                .model
                .history_scope
                .stale(head.as_deref(), &self.model.stacks);
        if reload || stale {
            self.refresh_history();
        }
    }

    /// Shows the history of `scope`, chosen in the pill's menu or by a click on a
    /// sidebar branch on the History page.
    pub(crate) fn choose_history_scope(&mut self, scope: HistoryScope, cx: &mut Context<Self>) {
        self.history_scope_menu = false;
        self.model.history_scope.choose(scope);
        self.refresh_history();
        cx.notify();
    }

    /// Asks for the page after the last commit listed, when there is one to ask for.
    pub(crate) fn request_history_page(&mut self) {
        if !self.model.history.can_page() {
            return;
        }
        let Some(after) = self.model.history.last_oid().map(str::to_owned) else {
            return;
        };
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadHistoryPage { after });
        self.model.history.request_page(id);
    }

    /// Saves the commit diff's layout in the background (D40). The choice takes
    /// effect on screen before `~/.gitconfig` is written.
    fn save_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        self.settings.diff_layout = layout;
        self.save_settings(cx);
    }

    /// Writes the preferences in the background, as the generation after the last save.
    pub(crate) fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.settings_store.clone() else {
            return;
        };
        self.settings_generation += 1;
        let generation = self.settings_generation;
        let settings = self.settings.clone();
        cx.background_spawn(async move {
            if let Err(error) = store.save(&settings, generation) {
                eprintln!("gitmaster: could not save the settings: {error}");
            }
        })
        .detach();
    }

    /// Puts `root` first in the recent list and saves the list in the background.
    fn remember(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.recents.push(root);
        let Some(store) = self.recent_store.clone() else {
            return;
        };
        self.recent_generation += 1;
        let generation = self.recent_generation;
        let recents = self.recents.clone();
        cx.background_spawn(async move {
            if let Err(error) = store.save(&recents, generation) {
                eprintln!("gitmaster: could not save the recent repositories: {error}");
            }
        })
        .detach();
    }

    fn on_composer_event(
        &mut self,
        event: ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::CommitRequested => {
                self.run_action(Action::Commit, &Args::None, window, cx);
            }
        }
    }

    fn on_diff_pane_event(
        &mut self,
        event: &DiffPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DiffPaneEvent::Chunk { action, args } => {
                self.run_action(*action, args, window, cx);
            }
            DiffPaneEvent::Stage(target) => self.send_line_command(
                CommandKind::Stage {
                    target: target.clone(),
                },
                cx,
            ),
            DiffPaneEvent::Unstage(target) => self.send_line_command(
                CommandKind::Unstage {
                    target: target.clone(),
                },
                cx,
            ),
            DiffPaneEvent::LeaveCommit => self.close_commit_file(cx),
            DiffPaneEvent::Review { action, args } => self.run_action(*action, args, window, cx),
            DiffPaneEvent::ReplyTo(id) => self.open_review_reply(*id, window, cx),
            // One layout for both diffs: the other pane follows, and the choice is saved (D40).
            DiffPaneEvent::LayoutChosen(layout) => {
                self.diff_pane
                    .update(cx, |pane, cx| pane.set_layout(*layout, cx));
                self.commit_diff_pane
                    .update(cx, |pane, cx| pane.set_layout(*layout, cx));
                self.compare_diff_pane
                    .update(cx, |pane, cx| pane.set_layout(*layout, cx));
                self.save_layout(*layout, cx);
            }
        }
    }

    /// Sends a line-level stage or unstage (D24), which the registry does not
    /// list. It waits for an open or a checkout like every other change.
    fn send_line_command(&mut self, kind: CommandKind, cx: &mut Context<Self>) {
        if let Some(reason) = self.busy_reason() {
            self.notify_user(reason, cx);
            return;
        }
        self.engine.dispatch(Actor::Human, kind);
        cx.notify();
    }

    /// Reloads the staged and unstaged diffs of the selected file. The pane is
    /// cleared first only when the file changed.
    pub(crate) fn load_selected(&mut self, cx: &mut Context<Self>) {
        if self.model.is_opening() {
            return;
        }
        // A submodule row shows its commits, not a diff, so no diff is read for it (D175).
        let Some(path) = self.model.diff_path().map(str::to_owned) else {
            self.model.forget_diffs();
            self.show_diffs(None, None, cx);
            return;
        };
        if !self.model.diffs_are_for(&path) {
            self.show_diffs(None, None, cx);
        }
        let load = |side| CommandKind::LoadDiff {
            path: path.clone(),
            side,
        };
        let unstaged = self.engine.dispatch(Actor::Human, load(DiffSide::Unstaged));
        let staged = self.engine.dispatch(Actor::Human, load(DiffSide::Staged));
        self.model.expect_diffs(path, unstaged, staged);
    }

    fn show_diffs(
        &self,
        unstaged: Option<FileDiff>,
        staged: Option<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        self.diff_pane
            .update(cx, |pane, cx| pane.set_diffs(unstaged, staged, cx));
    }

    pub(crate) fn step_file(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.model.select_file_step(down) {
            self.load_selected(cx);
        }
        if let Some(row) = self.model.selected_row().and_then(|index| {
            crate::change_tree::row_of(&crate::change_tree::rows(self.model.entries()), index)
        }) {
            self.scroll.scroll_to_item(row, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub(crate) fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.model.select_row(row) {
            self.load_selected(cx);
        }
        cx.notify();
    }

    /// Space: stages the selected file, or unstages it when nothing is left unstaged.
    pub(crate) fn toggle_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.model.selected_path().map(str::to_owned) else {
            return;
        };
        let action = self
            .model
            .entries()
            .iter()
            .find(|entry| entry.path == path)
            .map(action_registry::toggle_file_action);
        if let Some(action) = action {
            self.run_action(action, &Args::File(path), window, cx);
        }
    }

    pub(crate) fn focus_commit_message(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    /// The one path from a run to the engine: checks the action's rule, then
    /// does what it says, with `Actor::Human`. A refused run says why.
    pub(crate) fn run_action(
        &mut self,
        action: Action,
        args: &Args,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = self.composer.read(cx).text(cx);
        let spot = self.comment_spot(cx);
        let planned = action.plan(args, &self.action_context_with(&message, spot));
        match planned {
            // A preview action whose feature is off does nothing at all, not even a notice (D169).
            Err(action_registry::PREVIEW_OFF) => {}
            Err(reason) => self.notify_user(reason, cx),
            Ok(Effect::Send(kinds)) => self.send_commands(action, kinds, cx),
            Ok(
                effect @ (Effect::OpenReviewComposer { .. }
                | Effect::OpenReviewComposerByText { .. }),
            ) => {
                self.open_comment_composer(effect, window, cx);
            }
            Ok(Effect::OpenReviewEdit { id }) => self.open_review_edit(id, window, cx),
            Ok(Effect::CopyThreadMarkdown(uid)) => self.copy_thread_markdown(uid, cx),
            Ok(Effect::ThreadFolds { open }) => self.fold_thread_groups(open, cx),
            Ok(Effect::Copy(text)) => self.copy_text(text.clone(), &format!("Copied {text}"), cx),
            Ok(Effect::CopyMessage(text)) => self.copy_text(text, "Copied commit message", cx),
            Ok(Effect::Reveal(path)) => self.reveal(action, &path, cx),
            Ok(Effect::OpenWorkflowInputs { workflow, inputs }) => {
                self.open_workflow_inputs_dialog(&workflow, &inputs, window, cx);
            }
            Ok(Effect::FocusCommitMessage) => self.focus_commit_message(window, cx),
            Ok(Effect::UseSuggestion(text)) => self.use_suggestion(&text, cx),
            Ok(Effect::DiscardSuggestion) => self.model.suggestion.drop_suggestion(),
            Ok(Effect::OpenWorktreeDialog { branch }) => {
                self.open_worktree_dialog(branch.as_deref(), window, cx);
            }
            Ok(Effect::OpenWorktree(path)) => self.open_worktree(&path, cx),
            Ok(Effect::OpenRemoveDialog(path)) => self.open_remove_dialog(&path, window, cx),
            Ok(Effect::OpenWorkspaceDialog) => self.open_workspace_dialog(window, cx),
            Ok(Effect::OpenRemoveWorkspaceDialog(path)) => {
                self.open_remove_workspace_dialog(&path, window, cx);
            }
            Ok(Effect::OpenPruneDialog) => self.open_prune_dialog(cx),
            Ok(
                effect @ (Effect::OpenNewBranchDialog { .. }
                | Effect::OpenNewChildDialog { .. }
                | Effect::PickStackParent { .. }),
            ) => self.open_branch_effect(effect, window, cx),
            Ok(Effect::OpenDeleteBranchDialog(branch)) => {
                self.open_delete_branch_dialog(&branch, window, cx);
            }
            Ok(Effect::OpenMergedBranchesDialog) => self.open_merged_dialog(window, cx),
            Ok(Effect::OpenRestackDialog { branch }) => {
                self.open_restack_dialog(&branch, window, cx);
            }
            Ok(Effect::OpenDiscardFileDialog {
                path,
                include_staged,
            }) => self.open_discard_file_dialog(&path, include_staged, window, cx),
            Ok(Effect::ShowHistory) => self.show_history(cx),
            Ok(Effect::ShowChangesTab(tab)) => self.show_changes_tab(tab, cx),
            Ok(Effect::ShowStashes) => self.show_stashes(cx),
            Ok(Effect::ShowReflog) => self.show_reflog(cx),
            Ok(Effect::ShowThreads) => self.show_threads(window, cx),
            Ok(Effect::Memory(effect)) => self.run_memory_effect(effect, window, cx),
            Ok(Effect::ShowLabels) => self.show_labels(window, cx),
            Ok(Effect::ShowLocalReview) => self.show_local_review(cx),
            Ok(Effect::ShowInbox) => self.show_inbox(cx),
            Ok(Effect::UndoLastOperation) => self.submit_restack_undo(cx),
            Ok(Effect::ShowReviewInDiff { path, line }) => {
                self.show_review_in_diff(&path, line, cx);
            }
            Ok(Effect::OpenNewStashDialog) => self.open_new_stash_dialog(window, cx),
            Ok(Effect::OpenDropStashDialog { index, oid, label }) => {
                self.open_drop_stash_dialog(index, &oid, &label, window, cx);
            }
            Ok(Effect::OpenThreadField { field, uid }) => {
                self.open_thread_field(field, &uid, window, cx);
            }
            Ok(Effect::OpenLinkThread(uid)) => self.open_link_thread(&uid, window, cx),
            Ok(Effect::OpenDeleteThreadDialog(uid)) => {
                self.open_delete_thread_dialog(&uid, window, cx);
            }
            Ok(Effect::OpenScrubDialog) => self.open_scrub_dialog(window, cx),
            Ok(Effect::OpenLabelDialog(dialog)) => self.open_label_dialog(dialog, window, cx),
            Ok(Effect::ShowThreadsFor(path)) => self.show_threads_for(&path, window, cx),
            Ok(Effect::OpenThreadsSettings) => self.open_threads_settings(window, cx),
            Ok(Effect::ShowWorkflows) => self.show_workflows(cx),
            Ok(Effect::OpenQuickOpen) => self.open_quick_open(window, cx),
            Ok(Effect::ShowCode) => self.show_code(cx),
            Ok(Effect::ShowCodeHealth) => self.show_code_health(cx),
            Ok(Effect::ShowSearch) => self.show_search(window, cx),
            Ok(Effect::ViewFile { path, line }) => self.view_file(&path, line, cx),
            Ok(Effect::ShowProvenance(ask)) => self.show_provenance(&ask, cx),
            Ok(Effect::SaveWorkflow) => self.save_workflow(false, cx),
            Ok(Effect::RevertWorkflow) => self.revert_workflow(cx),
            Ok(Effect::OpenDeleteWorkflow) => self.ask_delete_workflow(cx),
            Ok(Effect::DryRunWorkflow) => self.dry_run_workflow(cx),
            Ok(Effect::SetAuthorPictures(on)) => self.set_author_pictures(on, cx),
            Ok(Effect::PickMoveTarget {
                action: target,
                path,
            }) => self.pick_move_target(target, &path, window, cx),
            Ok(Effect::MoveFileTo { path, target }) => {
                self.move_file_to(&path, target, window, cx);
            }
        }
        cx.notify();
    }

    /// Sends the engine commands of a run, in order, all as the human, and tracks
    /// the ones whose outcome the window shows: an apply or a pop, a checkout, a
    /// remote operation, a detached checkout and a refresh.
    fn send_commands(&mut self, action: Action, kinds: Vec<CommandKind>, cx: &mut Context<Self>) {
        if action == Action::Refresh {
            self.request_branch_list();
            self.request_hook_sections();
        }
        for kind in kinds {
            // While a restack conflict is open, the resolver's events carry its worktree; a
            // read of the open checkout would be taken for it, so it is refused (D154).
            if matches!(
                kind,
                CommandKind::ReadConflict {
                    site: ConflictSite::WorkingCopy,
                    ..
                }
            ) && self.model.restacking.resolving().is_some()
            {
                self.notify_user("Finish or cancel the restack conflict first.", cx);
                continue;
            }
            let applying = stash_apply::tracked(&kind);
            let detached = reflog::checkout_target(&kind).map(str::to_owned);
            let checkout = matches!(kind, CommandKind::Checkout { .. });
            let refresh = matches!(kind, CommandKind::RefreshStatus);
            let remote = matches!(
                kind,
                CommandKind::Fetch | CommandKind::Pull | CommandKind::PushWithLease
            );
            let transform = match &kind {
                CommandKind::RunTransform { field, current } => {
                    Field::from_name(field).map(|field| (field, current.clone()))
                }
                _ => None,
            };
            let id = self.engine.dispatch(Actor::Human, kind);
            // A new commit message request replaces the suggestion waiting for the person.
            if matches!(transform, Some((Field::CommitMessage, _))) {
                self.model.suggestion.drop_suggestion();
            }
            if let Some((field, sent)) = transform {
                self.model.transforms.start(id, field, sent);
            }
            if let Some((apply, index)) = applying {
                self.model.stash_apply.start(id, apply, index);
            }
            if let Some(oid) = detached {
                self.model.reflog_checkout.start(id, &oid);
            }
            if checkout {
                self.model.branches.begin_checkout(id);
            }
            if remote {
                self.model.remote.begin(id, action);
            }
            if refresh {
                self.start_refresh_indicator(id, cx);
            }
        }
    }

    /// Opens `path` in the file manager. The workflows folder is made first, so a
    /// fresh machine without one still opens something.
    fn reveal(&mut self, action: Action, path: &std::path::Path, cx: &mut Context<Self>) {
        if action == Action::OpenWorkflowsFolder
            && let Err(error) = std::fs::create_dir_all(path)
        {
            let text = format!("Could not create {}: {error}", path.display());
            self.notify_user(&text, cx);
            return;
        }
        cx.reveal_path(path);
    }

    /// The effects that start a branch: the new-branch dialog, as itself or for a
    /// child, and the parent step of Set Parent….
    fn open_branch_effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::OpenNewBranchDialog { start } => {
                self.open_new_branch_dialog(start.as_deref(), None, window, cx);
            }
            Effect::OpenNewChildDialog { parent } => {
                self.open_new_branch_dialog(None, Some(&parent), window, cx);
            }
            Effect::PickStackParent { branch } => {
                self.stack_subject = Some(branch);
                self.open_palette_at_pick(Action::ChooseStackParent, window, cx);
            }
            _ => {}
        }
    }

    /// Copy Branch Name on the checked-out branch: the `⌥⌘C` shortcut.
    pub(crate) fn copy_current_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let head = self
            .model
            .status()
            .and_then(|status| status.branch.head.clone());
        match head {
            Some(name) => self.run_action(Action::CopyBranchName, &Args::Branch(name), window, cx),
            None => self.notify_user("HEAD is detached: there is no branch name to copy.", cx),
        }
    }

    /// What the registry's rules read, for a run made now, with no diff selection.
    pub(crate) fn action_context<'a>(&'a self, message: &'a str) -> ActionContext<'a> {
        self.action_context_with(message, CommentSpot::Nothing)
    }

    /// What the registry's rules read, for a run made now. `spot` is where the
    /// Working Copy diff's cursor or selection is (see `comment_spot`).
    pub(crate) fn action_context_with<'a>(
        &'a self,
        message: &'a str,
        spot: CommentSpot<'a>,
    ) -> ActionContext<'a> {
        ActionContext {
            repo_open: self.model.status().is_some(),
            features: self.features,
            undoable: self
                .model
                .restacking
                .record()
                .is_some()
                .then_some("Undo restack"),
            selected_file: self.model.selected_path(),
            stack: crate::action_registry::StackCtx {
                stacks: Some(&self.model.stacks),
                subject: self.stack_subject.as_deref(),
                branches: self.model.branches.known(),
            },
            selected_thread: self
                .model
                .threads
                .selected_summary()
                .map(|thread| ThreadRef {
                    uid: &thread.uid,
                    starred: thread.starred,
                }),
            selected_label: self.model.labels.selected_label().map(|label| LabelRef {
                id: label.id,
                name: &label.name,
                attachments: label.attachments,
            }),
            selected_memory: self
                .model
                .memories
                .selected_memory()
                .map(|memory| MemoryRef {
                    uid: &memory.uid,
                    proposed: memory.status == gm_schema::threads::MemoryStatus::Proposed,
                    placement: crate::memories::Placement::of(memory),
                }),
            code: self.code_ctx(),
            submodules: Some(&self.model.submodules),
            busy: self.busy_reason(),
            remote_running: self.model.remote.in_flight().is_some(),
            entries: self.model.entries(),
            staged_count: self.model.summary().staged,
            message,
            current_branch: self
                .model
                .status()
                .and_then(|status| status.branch.head.as_deref()),
            root: self.model.status().map(|status| status.root.as_str()),
            worktrees: &self.model.worktrees,
            workspaces: self.model.workspaces.workspaces(),
            author_pictures: crate::action_registry::AuthorPictures::from_on(
                self.settings.author_pictures,
            ),
            stashes: self
                .model
                .stashes
                .is_loaded()
                .then(|| self.model.stashes.entries()),
            review: ReviewCtx {
                comments: self.model.review.comments(),
                spot,
            },
            messages: MessageCtx {
                transform: transform_run::loaded(
                    self.model.workflows.listed.as_deref(),
                    Field::CommitMessage,
                ),
                suggestion: self.model.suggestion.current(),
                sections: &self.model.hook_sections,
            },
            workflows: WorkflowCtx {
                listed: self.model.workflows.listed.as_deref(),
                awaiting: self.model.workflows.awaiting_approval(),
                active: self.model.workflows.active(),
                user_dir: self.model.workflows.user_dir.as_deref(),
                templates: &self.model.workflows.templates,
                page: self
                    .model
                    .workflows_page
                    .state(self.screen == Screen::Workflows),
            },
        }
    }

    /// Puts `text` on the clipboard and says `notice`.
    fn copy_text(&mut self, text: String, notice: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notify_user(notice, cx);
    }

    pub(crate) fn notify_user(&mut self, text: &str, cx: &mut Context<Self>) {
        let generation = self.model.set_notice(text.to_owned());
        Self::expire_notice_later(generation, cx);
        cx.notify();
    }

    /// Why a repository change must wait: an open or a checkout is in flight.
    /// Changes would run against the repository being left, the one being
    /// opened, or a branch that is about to change.
    pub(crate) fn busy_reason(&self) -> Option<Reason> {
        if self.model.is_opening() {
            Some("Wait for the repository to open.")
        } else if self.model.branches.checkout_in_flight() {
            Some("Wait for the branch checkout to finish.")
        } else {
            None
        }
    }

    fn expire_notice_later(generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            let _ = this.update(cx, |view, cx| {
                view.model.expire_notice(generation);
                cx.notify();
            });
        })
        .detach();
    }

    /// Tab and shift-Tab: list, then diff pane, then composer.
    fn cycle_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        // The Branch and Compare tabs have no composer: the list and the diff take turns.
        if self.screen == Screen::WorkingCopy && self.model.compare.tab != ChangesTab::Uncommitted {
            if self.list_focus.is_focused(window) {
                let handle = self.compare_diff_pane.read(cx).focus_handle();
                window.focus(&handle);
            } else {
                self.list_focus.focus(window);
            }
            cx.notify();
            return;
        }
        let in_list = self.list_focus.is_focused(window);
        let in_composer = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let current = if in_list {
            0
        } else if in_composer {
            2
        } else {
            1
        };
        let next = if forward {
            (current + 1) % 3
        } else {
            (current + 2) % 3
        };
        match next {
            0 => self.list_focus.focus(window),
            1 => {
                let handle = self.diff_pane.read(cx).focus_handle();
                window.focus(&handle);
            }
            _ => self
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx)),
        }
        cx.notify();
    }

    fn top_bar(&self, window: &Window, cx: &Context<Self>) -> Div {
        let loading = if self.model.loading {
            "Loading…"
        } else if self.model.updating() {
            "Updating…"
        } else {
            ""
        };
        window_chrome::header_bar(window)
            .h(title_bar::HEIGHT)
            .child(window_chrome::no_drag_shrinking(self.title_block(cx)))
            .child(match self.context_zone(cx) {
                Some(zone) => window_chrome::no_drag_shrinking(zone).flex_1(),
                None => div().flex_1(),
            })
            .child(div().flex_none().text_color(theme::muted()).child(loading))
            .child(window_chrome::no_drag(self.toolbar(cx)))
            .children(
                window_chrome::draws_frame(window).then(|| window_chrome::window_controls(window)),
            )
    }

    fn status_bar(&self, window: &Window, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let corners = window_chrome::inner_radii(window);
        let drag_status = self
            .drag_status_text()
            .or_else(|| self.branch_drag_status());
        let (message, colour, tint) = match (&self.model.error, &drag_status, &self.model.notice) {
            (Some(error), _, _) => (error.clone(), theme::removed_fg(), theme::removed_bg()),
            (None, Some(hint), _) => (hint.clone(), theme::text(), theme::panel()),
            (None, None, Some(notice)) => (notice.clone(), theme::warn_fg(), theme::warn_bg()),
            (None, None, None) => (String::new(), muted, theme::panel()),
        };
        let offer = if self.model.error.is_none() && drag_status.is_none() {
            self.open_worktree_offer(cx)
        } else {
            None
        };
        let notice_icon = match &self.model.error {
            None => self.model.notice_icon,
            Some(_) => None,
        };
        div()
            .h(px(26.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(div().flex_1())
            // A long message is cut at the end, so the log button stays on the bar.
            .child(
                div()
                    .flex_none()
                    .max_w(px(NOTICE_WIDTH))
                    .px_2()
                    .rounded_sm()
                    .bg(tint)
                    .text_color(colour)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .children(notice_icon.map(|icon| icon::view(icon, icon::SMALL, colour)))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(message),
                    )
                    .children(offer)
                    .children(self.gate_review_link(cx)),
            )
            .children(self.undo_button(cx))
            .child(self.command_log_button(cx))
    }
}

impl StatusView {
    /// "Undo ⌘Z" on the status bar, while the operation log's last undoable operation can be
    /// undone. A registry action, so the key and the palette run the same rule.
    fn undo_button(&self, cx: &Context<Self>) -> Option<gpui::Stateful<Div>> {
        let ctx = self.action_context("");
        let title = ctx.undoable?.to_owned();
        let reason = Action::UndoLastOperation.enabled(&Args::None, &ctx).err();
        let shortcut = Action::UndoLastOperation
            .shortcut_label()
            .unwrap_or_default();
        let button = div()
            .id("undo-last-operation")
            .flex_none()
            .px_2()
            .rounded_sm()
            .flex()
            .flex_row()
            .gap_1()
            .text_color(if reason.is_some() {
                theme::faint()
            } else {
                theme::text()
            })
            .child(title)
            .child(div().text_color(theme::muted()).child(shortcut));
        Some(match reason {
            Some(reason) => button.tooltip(window_chrome::hint(reason)),
            None => button
                .cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        this.run_action(Action::UndoLastOperation, &Args::None, window, cx);
                    }),
                ),
        })
    }

    /// The stored sizes of the sidebar, the left column and the commit area,
    /// clamped again at each frame: a window resized after a drag may have moved
    /// the limits. The left column is fitted beside the sidebar's clamped width,
    /// so the main area keeps its room.
    fn fitted_sizes(&self, window: &Window) -> (f32, f32, f32) {
        let viewport = window.viewport_size();
        let sidebar_width = resize::SIDEBAR.clamp(self.sidebar_width, f32::from(viewport.width));
        let left_width =
            resize::left_beside_sidebar(self.left_width, sidebar_width, f32::from(viewport.width));
        let commit_height = resize::COMMIT.clamp(self.commit_height, f32::from(viewport.height));
        (sidebar_width, left_width, commit_height)
    }
}

impl Render for StatusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_thread_rows();
        let (sidebar_width, left_width, commit_height) = self.fitted_sizes(window);
        window_chrome::sync_client_inset(window);
        self.refresh_workflow_editor(window, cx);
        let app = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .text_color(theme::text())
            .font_family(theme::ui_font())
            .text_sm()
            // Esc ends a file drag, as gpui itself does not: the drop then has no file to move.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.model.move_file.drag.is_some() {
                    this.end_drag(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                this.cycle_focus(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrev, window, cx| {
                this.cycle_focus(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| {
                this.run_action(Action::SearchText, &Args::None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenQuickOpen, window, cx| {
                if this.model.quick_open.open {
                    this.close_quick_open(window, cx);
                } else {
                    this.run_action(Action::QuickOpen, &Args::None, window, cx);
                }
            }))
            // The Code viewer has no key context of its own: its pick is commented on here.
            .on_action(cx.listener(
                |this, _: &crate::diff_pane::CommentOnSelection, window, cx| {
                    if this.screen == Screen::Code && this.review_dialog.is_none() {
                        this.run_action(Action::CommentOnSelection, &Args::None, window, cx);
                    }
                },
            ))
            .on_action(cx.listener(Self::save_workflow_action))
            .on_action(cx.listener(|this, action: &RunWorkflowKey, window, cx| {
                this.run_action(
                    Action::RunWorkflow,
                    &Args::Workflow(action.name.clone()),
                    window,
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &UndoLastOperation, window, cx| {
                this.run_action(Action::UndoLastOperation, &Args::None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyCurrentBranch, window, cx| {
                this.copy_current_branch(window, cx);
            }))
            // With no repository, the welcome screen replaces every part below.
            .children(self.welcome.is_none().then(|| self.top_bar(window, cx)))
            .children(
                (self.welcome.is_some())
                    .then(|| welcome::header(window))
                    .flatten(),
            )
            .children(
                self.welcome
                    .is_none()
                    .then(|| self.main_area(sidebar_width, left_width, commit_height, cx)),
            )
            .children(
                self.welcome
                    .is_none()
                    .then(|| self.notices_strip(cx))
                    .flatten(),
            )
            .children(self.welcome.is_none().then(|| self.status_bar(window, cx)))
            .children(
                self.welcome
                    .as_ref()
                    .map(|welcome| self.welcome_body(welcome, cx)),
            )
            .children(self.log_open.then(|| self.command_log_panel(cx)))
            .children(self.project_menu.open.then(|| self.project_menu_layer(cx)))
            .children(self.branch_menu.open.then(|| self.branch_menu_layer(cx)))
            .children(
                self.context_menu
                    .open
                    .is_some()
                    .then(|| self.context_menu_layer(cx)),
            )
            .children(self.palette.open.then(|| self.palette_layer(window, cx)))
            .children(
                self.model
                    .quick_open
                    .open
                    .then(|| self.quick_open_layer(window, cx)),
            )
            .children(self.dialog_layers(window, cx));
        window_chrome::framed(window, app)
    }
}

impl StatusView {
    /// The dialogs that open over the window, in the order they stack: the last is on top.
    fn dialog_layers(&self, window: &Window, cx: &Context<Self>) -> Vec<Div> {
        let mut layers = Vec::new();
        if self.worktree_dialog.is_some() {
            layers.push(self.worktree_layer(window, cx));
        }
        if self.removal_dialog.is_some() {
            layers.push(self.removal_layer(window, cx));
        }
        if self.workspace_dialog.is_some() {
            layers.push(self.workspace_layer(window, cx));
        }
        if self.workspace_removal_dialog.is_some() {
            layers.push(self.workspace_removal_layer(window, cx));
        }
        if self.prune_dialog.is_some() {
            layers.push(self.prune_layer(window, cx));
        }
        layers.extend(self.new_branch_layer(window, cx));
        layers.extend(self.stash_layers(window, cx));
        layers.extend(self.thread_layers(window, cx));
        layers.extend(self.memory_layers(window, cx));
        layers.extend(self.contexts_layers(window, cx));
        layers.extend(self.label_layers(window, cx));
        layers.extend(self.threads_settings_layer(window, cx));
        layers.extend(self.review_composer_layer(window, cx));
        layers.extend(self.workflow_inputs_layer(window, cx));
        layers.extend(self.workflow_review_layer(window, cx));
        if self.delete_dialog.is_some() {
            layers.push(self.delete_branch_layer(window, cx));
        }
        layers.extend(self.discard_file_layer(window, cx));
        layers.extend(self.move_popover_layer(cx));
        if self.merged_dialog.is_some() {
            layers.push(self.merged_layer(window, cx));
        }
        if self.review_run.is_some() {
            layers.push(self.review_run_layer(window, cx));
        }
        if self.restack_dialog.is_some() {
            layers.push(self.restack_layer(window, cx));
        }
        layers
    }
}

/// Whether the event is one of the file list, folder or viewer: those change the Code page.
fn is_file_event(event: &Event) -> bool {
    matches!(
        event,
        Event::FilesFound { .. }
            | Event::DirectoryListed { .. }
            | Event::FileRead { .. }
            | Event::FileHighlighted { .. }
            | Event::FilesIndexed { .. }
    )
}

/// The text of the dialog that shows a conversion's answer (`WF3e`), when the event is one.
fn convert_answer(event: &Event) -> Option<String> {
    match event {
        Event::HooksConverted {
            written,
            skipped,
            sections,
            ..
        } => Some(hook_convert::convert_report(written, skipped, sections)),
        _ => None,
    }
}

/// The answer of a transform operation, when the event is one (`WF3d`).
fn transform_answer(event: &Event) -> Option<(OperationId, Option<String>, bool)> {
    match event {
        Event::TransformFinished {
            id, value, changed, ..
        } => Some((*id, value.clone(), *changed)),
        _ => None,
    }
}
