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
use gm_schema::{Actor, CommandKind, DiffSide, Event, FileDiff, OperationId};
use gpui::{
    ClipboardItem, Context, Div, Entity, FocusHandle, Render, ScrollHandle, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::action_registry::{
    self, Action, Args, Context as ActionContext, Effect, HookCtx, Reason,
};
use crate::app_settings::{self, AppSettings};
use crate::branch_menu::BranchMenu;
use crate::composer::{Composer, ComposerEvent};
use crate::context_menu::ContextMenu;
use crate::diff_pane::{DiffPane, DiffPaneEvent};
use crate::diff_rows::Layout;
use crate::hook_run::Fill;
use crate::icon::{self, Icon};
use crate::model::{Model, Switched};
use crate::palette::Palette;
use crate::palette_view::{self, PaletteView};
use crate::project_menu::ProjectMenu;
use crate::recent::{self, Recents};
use crate::resize;
use crate::sidebar::Grouping;
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::title_bar;
use crate::welcome::{self, Startup, Welcome};
use crate::window_chrome;
use crate::worktree_create::WorktreeDialog;
use crate::worktree_prune::PruneDialog;
use crate::worktree_remove::RemoveDialog;
use crate::{CopyCurrentBranch, FocusNext, FocusPrev, OpenPalette, StartPath};

/// The key context of the change list. Its keys work only while the list has
/// focus, so typing into the message is never taken as a key.
pub const KEY_CONTEXT: &str = "GitMaster";

/// How long a notice stays on the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(4);

/// How long a refresh runs before "Updating…" shows. Quicker ones never show it.
const UPDATING_DELAY: Duration = Duration::from_millis(300);

const WATCH_LIMITED: &str = "Watching is limited; refreshing when the window gains focus.";

/// What the main area shows: the Working Copy, or the history page (H1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Screen {
    #[default]
    WorkingCopy,
    History,
}

pub struct StatusView {
    pub(crate) engine: EngineHandle,
    /// Every git command that changed the repository, for the log panel.
    pub(crate) command_log: CommandLog,
    pub(crate) log_open: bool,
    pub(crate) log_scroll: ScrollHandle,
    pub(crate) model: Model,
    pub(crate) list_focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) composer: Entity<Composer>,
    pub(crate) diff_pane: Entity<DiffPane>,
    /// The read-only pane that shows one file of the open commit (H4).
    pub(crate) commit_diff_pane: Entity<DiffPane>,
    /// The left column's width, in logical pixels, as the user dragged it.
    pub(crate) left_width: f32,
    /// The commit message box's height, in logical pixels, as dragged.
    pub(crate) commit_height: f32,
    /// How the sidebar's branch tree is grouped.
    pub(crate) sidebar_grouping: Grouping,
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
    /// The create-worktree dialog, while it is open.
    pub(crate) worktree_dialog: Option<WorktreeDialog>,
    /// The remove-worktree dialog, while it is open.
    pub(crate) removal_dialog: Option<RemoveDialog>,
    /// The prune dialog, while it is open.
    pub(crate) prune_dialog: Option<PruneDialog>,
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
        let may_fall_back = !start.from_argument;
        let open = engine.dispatch(Actor::Human, CommandKind::OpenRepo { path: start.path });
        theme::follow(window.appearance());
        let list_focus = cx.focus_handle();
        list_focus.focus(window);
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
        let composer = cx.new(Composer::new);
        let diff_pane = cx.new(DiffPane::new);
        let commit_diff_pane = cx.new(DiffPane::read_only);
        // The saved preferences are read here, before the first frame, so the
        // commit diff opens in the layout the user last chose (D40).
        let settings_store =
            app_settings::default_file().map(|file| Arc::new(app_settings::Store::new(file)));
        let settings = settings_store
            .as_ref()
            .map(|store| store.load())
            .unwrap_or_default();
        // Both diffs open in the saved layout (D39, D40).
        diff_pane.update(cx, |pane, cx| pane.set_layout(settings.diff_layout, cx));
        commit_diff_pane.update(cx, |pane, cx| pane.set_layout(settings.diff_layout, cx));
        let mut subscriptions = vec![
            cx.observe(&branch_filter, |this, filter, cx| {
                this.on_branch_filter_changed(&filter, cx);
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
            // Coming back to the window is a reason to refresh; the watcher debounces it.
            cx.observe_window_activation(window, |this, window, _cx| {
                if window.is_window_active() {
                    this.watch.request();
                    this.request_hook_list();
                }
            }),
            // Also fires when the compositor answers the decoration request, so
            // the title bar appears or goes as soon as the grant is known.
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::follow(window.appearance());
                cx.notify();
            }),
        ];
        subscriptions.extend(palette_view::subscriptions(&palette_filter, window, cx));
        let mut model = Model::default();
        model.request_open(open);
        Self {
            engine,
            command_log,
            log_open: false,
            log_scroll: ScrollHandle::new(),
            model,
            list_focus,
            scroll: UniformListScrollHandle::new(),
            composer,
            diff_pane,
            commit_diff_pane,
            left_width: resize::LEFT_DEFAULT,
            commit_height: resize::COMMIT_DEFAULT,
            sidebar_grouping: Grouping::default(),
            screen: Screen::default(),
            project_menu,
            branch_menu,
            context_menu: ContextMenu::default(),
            palette: Palette::default(),
            palette_view: PaletteView::new(palette_filter),
            worktree_dialog: None,
            removal_dialog: None,
            prune_dialog: None,
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

    fn on_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let reload = matches!(event, Event::StatusLoaded { .. });
        let listed = matches!(event, Event::BranchesLoaded { .. });
        let committed = matches!(event, Event::Committed { .. });
        let finished = matches!(event, Event::HookFinished { .. });
        let head_before = self.head_name();
        let failed_start = self.take_settled_startup(&event);
        let shown = self.model.apply(event);
        if let Some(diff) = self.model.commit_file.take_fresh_diff() {
            self.commit_diff_pane
                .update(cx, |pane, cx| pane.show_read_only(diff, cx));
        }
        if let Some(outcome) = self.model.creating.take() {
            self.on_worktree_created(outcome, window, cx);
        }
        if let Some(outcome) = self.model.removal.take_outcome() {
            self.on_worktree_removal_settled(outcome, window, cx);
        }
        if let Some(outcome) = self.model.pruning.take_outcome() {
            self.on_worktrees_pruned(outcome, window, cx);
        }
        // A finished hook run fills the composer only while it still holds what the run was
        // started from; otherwise the answer waits as a suggestion (see hook_run).
        if finished {
            let typed = self.composer.read(cx).text(cx);
            if let Some(Fill::Apply(message)) = self.model.hooks.take_fill(&typed) {
                self.composer
                    .update(cx, |composer, cx| composer.set_text(&message, cx));
            }
        }
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
        if let Some(switched) = self.model.take_switched() {
            self.welcome = None;
            self.on_switched(switched, cx);
        }
        if let Some(startup) = failed_start {
            self.on_start_failed(startup, cx);
        }
        if let Some(diffs) = shown {
            self.show_diffs(diffs.unstaged, diffs.staged, cx);
        }
        // The palette's offers come from the status and the branch listing.
        if self.palette.open && (reload || listed) {
            self.refresh_palette(cx);
        }
        if reload {
            // A checkout moves HEAD; the tree marks the new branch.
            if self.head_name() != head_before {
                self.request_branch_list();
                self.request_hook_list();
            }
            self.load_selected(cx);
        }
        // The watcher refreshes the status when HEAD or a ref moves; the open
        // history re-reads its top then, so an outside commit shows up.
        if reload && self.screen == Screen::History && self.model.history.can_refresh() {
            self.refresh_history();
        }
        cx.notify();
    }

    fn head_name(&self) -> Option<String> {
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

    /// Asks the engine for the prepare-commit hooks of the open repository. Nothing
    /// is asked before one is open.
    pub(crate) fn request_hook_list(&mut self) {
        if self.model.status().is_some() {
            self.engine.dispatch(Actor::Human, CommandKind::ListHooks);
        }
    }

    /// A successful open replaced the repository: the view state of the old
    /// one goes, and the new root joins the recent list.
    fn on_switched(&mut self, switched: Switched, cx: &mut Context<Self>) {
        self.model.commit_file.close();
        if !switched.same_root {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
        }
        self.show_diffs(None, None, cx);
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.model.branches.clear();
        self.model.hooks.reset();
        self.request_branch_list();
        self.request_hook_list();
        if self.screen == Screen::History {
            self.refresh_history();
        }
        self.remember(PathBuf::from(switched.root), cx);
    }

    /// Opens the history page and asks for the commits of HEAD.
    pub(crate) fn show_history(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::History;
        self.refresh_history();
        cx.notify();
    }

    /// Shows the Working Copy again; the history listing is kept for the next visit.
    pub(crate) fn show_working_copy(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::WorkingCopy;
        cx.notify();
    }

    /// Asks for the first page. A list already shown stays until the answer comes.
    pub(crate) fn refresh_history(&mut self) {
        let id = self.engine.dispatch(Actor::Human, CommandKind::LoadHistory);
        self.model.history.request_top(id);
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
    /// effect on screen before the file is written.
    fn save_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        let Some(store) = self.settings_store.clone() else {
            return;
        };
        self.settings_generation += 1;
        let generation = self.settings_generation;
        let settings = AppSettings {
            diff_layout: layout,
        };
        cx.background_spawn(async move {
            if let Err(error) = store.save(settings, generation) {
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
            DiffPaneEvent::StepFile { forward } => self.step_commit_file(*forward, cx),
            DiffPaneEvent::LeaveCommit => self.close_commit_file(cx),
            // One layout for both diffs: the other pane follows, and the choice is saved (D40).
            DiffPaneEvent::LayoutChosen(layout) => {
                self.diff_pane
                    .update(cx, |pane, cx| pane.set_layout(*layout, cx));
                self.commit_diff_pane
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
    fn load_selected(&mut self, cx: &mut Context<Self>) {
        if self.model.is_opening() {
            return;
        }
        let Some(path) = self.model.selected_path().map(str::to_owned) else {
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
        if let Some(row) = self.model.selected_row() {
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
        let planned = action.plan(args, &self.action_context(&message));
        match planned {
            Err(reason) => self.notify_user(reason, cx),
            Ok(Effect::Send(kinds)) => {
                if action == Action::Refresh {
                    self.request_branch_list();
                    self.request_hook_list();
                }
                for kind in kinds {
                    let checkout = matches!(kind, CommandKind::Checkout { .. });
                    let refresh = matches!(kind, CommandKind::RefreshStatus);
                    let prepare = match &kind {
                        CommandKind::RunPrepareCommit { message } => Some(message.clone()),
                        _ => None,
                    };
                    let id = self.engine.dispatch(Actor::Human, kind);
                    if checkout {
                        self.model.branches.begin_checkout(id);
                    }
                    if let Some(sent) = prepare {
                        self.model.hooks.start(id, sent);
                    }
                    if refresh {
                        self.start_refresh_indicator(id, cx);
                    }
                }
            }
            Ok(Effect::Copy(text)) => {
                let notice = format!("Copied {text}");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.notify_user(&notice, cx);
            }
            Ok(Effect::Reveal(path)) => cx.reveal_path(&path),
            Ok(Effect::FocusCommitMessage) => self.focus_commit_message(window, cx),
            Ok(Effect::UseSuggestion(text)) => {
                self.composer
                    .update(cx, |composer, cx| composer.set_text(&text, cx));
                self.model.hooks.drop_suggestion();
            }
            Ok(Effect::DiscardSuggestion) => self.model.hooks.drop_suggestion(),
            Ok(Effect::DenyHook(hash)) => self.model.hooks.deny(&hash),
            Ok(Effect::OpenWorktreeDialog { branch }) => {
                self.open_worktree_dialog(Some(&branch), window, cx);
            }
            Ok(Effect::OpenWorktree(path)) => self.open_worktree(&path, cx),
            Ok(Effect::OpenRemoveDialog(path)) => self.open_remove_dialog(&path, window, cx),
            Ok(Effect::OpenPruneDialog) => self.open_prune_dialog(cx),
            Ok(Effect::ShowHistory) => self.show_history(cx),
        }
        cx.notify();
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

    /// What the registry's rules read, for a run made now.
    pub(crate) fn action_context<'a>(&'a self, message: &'a str) -> ActionContext<'a> {
        ActionContext {
            repo_open: self.model.status().is_some(),
            busy: self.busy_reason(),
            entries: self.model.entries(),
            staged_count: self.model.summary().staged,
            message,
            current_branch: self
                .model
                .status()
                .and_then(|status| status.branch.head.as_deref()),
            root: self.model.status().map(|status| status.root.as_str()),
            worktrees: &self.model.worktrees,
            hooks: HookCtx {
                running: self.model.hooks.in_flight(),
                applicable: self.model.hooks.applicable(),
                suggestion: self.model.hooks.suggestion.as_deref(),
                pending_approval: self
                    .model
                    .hooks
                    .pending_approval()
                    .map(|hook| (hook.name.as_str(), hook.command_hash.as_str())),
            },
        }
    }

    pub(crate) fn notify_user(&mut self, text: &str, cx: &mut Context<Self>) {
        let generation = self.model.set_notice(text.to_owned());
        Self::expire_notice_later(generation, cx);
        cx.notify();
    }

    /// Why a repository change must wait: an open or a checkout is in flight.
    /// Changes would run against the repository being left, the one being
    /// opened, or a branch that is about to change.
    fn busy_reason(&self) -> Option<Reason> {
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
        let muted = theme::muted();
        let (upstream, ahead, behind) = match self.model.status() {
            Some(status) => (
                status.branch.upstream.clone().unwrap_or_default(),
                status.branch.ahead,
                status.branch.behind,
            ),
            None => (String::new(), 0, 0),
        };
        let loading = if self.model.loading {
            "Loading…"
        } else if self.model.updating() {
            "Updating…"
        } else {
            ""
        };
        window_chrome::header_bar(window)
            .h(title_bar::HEIGHT)
            .child(window_chrome::no_drag(self.title_block(cx)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_color(muted)
                    .child(icon::view(Icon::ArrowUp, icon::SMALL, muted))
                    .child(ahead.to_string())
                    .child(icon::view(Icon::ArrowDown, icon::SMALL, muted))
                    .child(behind.to_string()),
            )
            .child(div().text_color(muted).child(upstream))
            .child(div().flex_1())
            .child(div().text_color(muted).child(loading))
            .child(window_chrome::no_drag(self.toolbar(cx)))
            .children(
                window_chrome::draws_frame(window).then(|| window_chrome::window_controls(window)),
            )
    }

    fn status_bar(&self, window: &Window, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let corners = window_chrome::inner_radii(window);
        let root = self
            .model
            .status()
            .map(|status| status.root.clone())
            .unwrap_or_default();
        let summary = self.model.summary();
        let counts = format!(
            "{} staged · {} unstaged · {} untracked",
            summary.staged, summary.unstaged, summary.untracked
        );
        let (message, colour, tint) = match (&self.model.error, &self.model.notice) {
            (Some(error), _) => (error.clone(), theme::removed_fg(), theme::removed_bg()),
            (None, Some(notice)) => (notice.clone(), theme::warn_fg(), theme::warn_bg()),
            (None, None) => (String::new(), muted, theme::panel()),
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
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_color(theme::faint())
                    .child(root),
            )
            .child(div().text_color(muted).child(counts))
            .child(div().flex_1())
            // A long message is cut at the end, so the log button stays on the bar.
            .child(
                div()
                    .min_w(px(0.))
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
                    ),
            )
            .child(self.command_log_button(cx))
    }
}

impl Render for StatusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The stored sizes are clamped again here, because a window resized
        // after a drag may have moved the limits.
        let viewport = window.viewport_size();
        let left_width = resize::LEFT.clamp(self.left_width, f32::from(viewport.width));
        let commit_height = resize::COMMIT.clamp(self.commit_height, f32::from(viewport.height));
        window_chrome::sync_client_inset(window);
        let app = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .text_color(theme::text())
            .font_family(theme::ui_font())
            .text_sm()
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                this.cycle_focus(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrev, window, cx| {
                this.cycle_focus(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.toggle_palette(window, cx);
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
            .children(self.welcome.is_none().then(|| {
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(self.sidebar(cx))
                    .children(
                        (self.screen == Screen::WorkingCopy)
                            .then(|| self.left_column(left_width, commit_height, cx)),
                    )
                    .children((self.screen == Screen::WorkingCopy).then(|| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .child(self.diff_pane.clone())
                    }))
                    .children((self.screen == Screen::History).then(|| self.history_page(cx)))
            }))
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
                    .then(|| self.context_menu_layer(window, cx)),
            )
            .children(self.palette.open.then(|| self.palette_layer(window, cx)))
            .children(
                self.worktree_dialog
                    .is_some()
                    .then(|| self.worktree_layer(window, cx)),
            )
            .children(
                self.removal_dialog
                    .is_some()
                    .then(|| self.removal_layer(window, cx)),
            )
            .children(
                self.prune_dialog
                    .is_some()
                    .then(|| self.prune_layer(window, cx)),
            );
        window_chrome::framed(window, app)
    }
}
