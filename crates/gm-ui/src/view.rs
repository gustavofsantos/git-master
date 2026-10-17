//! The Working Copy window: the top bar, the left column (filter, commit box,
//! file list), the diff pane on the right and the status bar. It sends
//! commands to the engine and renders the model; it never waits for git.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gm_core::EngineHandle;
use gm_core::command_log::CommandLog;
use gm_schema::{Actor, CommandKind, DiffSide, Event, FileDiff, StageTarget};
use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, FontWeight, MouseButton, MouseDownEvent, Render,
    ScrollHandle, Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::changes::ToggleAction;
use crate::composer::{Composer, ComposerEvent};
use crate::diff_pane::{DiffPane, DiffPaneEvent};
use crate::icon::{self, Icon};
use crate::model::{Model, Switched};
use crate::project_menu::ProjectMenu;
use crate::recent::{self, Recents};
use crate::resize;
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::window_chrome;
use crate::{FocusNext, FocusPrev};

/// The key context of the change list. Its keys work only while the list has
/// focus, so typing into the filter or the message is never taken as a key.
pub const KEY_CONTEXT: &str = "GitMaster";

/// How long a notice stays on the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(4);

pub struct StatusView {
    pub(crate) engine: EngineHandle,
    /// Every git command that changed the repository, for the log panel.
    pub(crate) command_log: CommandLog,
    pub(crate) log_open: bool,
    pub(crate) log_scroll: ScrollHandle,
    pub(crate) model: Model,
    pub(crate) list_focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) filter: Entity<TextArea>,
    pub(crate) composer: Entity<Composer>,
    pub(crate) diff_pane: Entity<DiffPane>,
    /// The left column's width, in logical pixels, as the user dragged it.
    pub(crate) left_width: f32,
    /// The commit message box's height, in logical pixels, as dragged.
    pub(crate) commit_height: f32,
    pub(crate) project_menu: ProjectMenu,
    /// The recent repositories, newest first, as last saved or loaded.
    pub(crate) recents: Recents,
    /// Where the list is kept; `None` when there is no home directory.
    recent_store: Option<Arc<recent::Store>>,
    /// Numbers the saves so an older one never lands after a newer one.
    recent_generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl StatusView {
    /// Opens `path` in the engine. Events are streamed into the view from a
    /// task on the UI thread; the subscription is made before the command so
    /// no event is missed.
    pub fn new(
        engine: EngineHandle,
        command_log: CommandLog,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = engine.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self::follow_command_log(&command_log, cx);
        let open = engine.dispatch(Actor::Human, CommandKind::OpenRepo { path });
        theme::follow(window.appearance());
        let list_focus = cx.focus_handle();
        list_focus.focus(window);
        let recent_store = recent::default_file().map(|file| Arc::new(recent::Store::new(file)));
        let recents = recent_store
            .as_ref()
            .map(|store| store.load())
            .unwrap_or_default();
        let project_menu = ProjectMenu::new(cx.focus_handle());
        let filter = cx.new(|cx| TextArea::new("Filter", cx).with_lines(1, 1));
        let composer = cx.new(Composer::new);
        let diff_pane = cx.new(DiffPane::new);
        let subscriptions = vec![
            cx.observe(&filter, |this, filter, cx| {
                this.on_filter_changed(&filter, cx);
            }),
            cx.subscribe_in(
                &filter,
                window,
                |this, _, event: &TextAreaEvent, window, cx| {
                    this.on_filter_event(*event, window, cx);
                },
            ),
            cx.subscribe(&composer, |this, _, event: &ComposerEvent, cx| {
                this.on_composer_event(*event, cx);
            }),
            cx.subscribe(&diff_pane, Self::on_diff_pane_event),
            // Also fires when the compositor answers the decoration request, so
            // the title bar appears or goes as soon as the grant is known.
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::follow(window.appearance());
                cx.notify();
            }),
        ];
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
            filter,
            composer,
            diff_pane,
            left_width: resize::LEFT_DEFAULT,
            commit_height: resize::COMMIT_DEFAULT,
            project_menu,
            recents,
            recent_store,
            recent_generation: 0,
            _subscriptions: subscriptions,
        }
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

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let reload = matches!(event, Event::StatusLoaded { .. });
        let committed = matches!(event, Event::Committed { .. });
        let shown = self.model.apply(event);
        if committed {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
            Self::expire_notice_later(self.model.notice_generation(), cx);
        }
        if let Some(switched) = self.model.take_switched() {
            self.on_switched(switched, cx);
        }
        if let Some(diffs) = shown {
            self.show_diffs(diffs.unstaged, diffs.staged, cx);
        }
        if reload {
            self.load_selected(cx);
        }
        cx.notify();
    }

    /// A successful open replaced the repository: the view state of the old
    /// one goes, and the new root joins the recent list.
    fn on_switched(&mut self, switched: Switched, cx: &mut Context<Self>) {
        self.filter.update(cx, |area, cx| area.set_text("", cx));
        if !switched.same_root {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
        }
        self.show_diffs(None, None, cx);
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.remember(PathBuf::from(switched.root), cx);
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

    fn on_filter_changed(&mut self, filter: &Entity<TextArea>, cx: &mut Context<Self>) {
        let text = filter.read(cx).text().to_owned();
        if text.contains('\n') {
            // The filter is one line; the area still takes enter as a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            filter.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; redraw only on a real change.
        if text == self.model.filter() {
            return;
        }
        if self.model.set_filter(&text) {
            self.load_selected(cx);
        }
        cx.notify();
    }

    fn on_filter_event(
        &mut self,
        event: TextAreaEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event == TextAreaEvent::Cancel {
            self.filter.update(cx, TextArea::clear);
            self.list_focus.focus(window);
        }
    }

    fn on_composer_event(&mut self, event: ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::CommitRequested => self.commit(cx),
        }
    }

    fn on_diff_pane_event(
        &mut self,
        _: Entity<DiffPane>,
        event: &DiffPaneEvent,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_while_opening(cx) {
            return;
        }
        let kind = match event {
            DiffPaneEvent::Stage(target) => CommandKind::Stage {
                target: target.clone(),
            },
            DiffPaneEvent::Unstage(target) => CommandKind::Unstage {
                target: target.clone(),
            },
        };
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

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.model.begin_refresh();
        self.engine
            .dispatch(Actor::Human, CommandKind::RefreshStatus);
        cx.notify();
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

    pub(crate) fn toggle_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        if self.model.toggle_collapsed(folder) {
            self.load_selected(cx);
        }
        cx.notify();
    }

    /// Stages or unstages the rows a checkbox covers, one command per file.
    pub(crate) fn toggle_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.refuse_while_opening(cx) {
            return;
        }
        let kinds: Vec<CommandKind> = match self.model.toggle_action(row) {
            ToggleAction::Stage(paths) => paths
                .into_iter()
                .map(|path| CommandKind::Stage {
                    target: StageTarget::File { path },
                })
                .collect(),
            ToggleAction::Unstage(paths) => paths
                .into_iter()
                .map(|path| CommandKind::Unstage {
                    target: StageTarget::File { path },
                })
                .collect(),
        };
        for kind in kinds {
            self.engine.dispatch(Actor::Human, kind);
        }
        cx.notify();
    }

    pub(crate) fn toggle_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.model.selected_row() {
            self.toggle_row(row, cx);
        }
    }

    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |area, cx| area.focus(window, cx));
    }

    /// Commits the message when there is one and something is staged; otherwise
    /// says why nothing happened.
    pub(crate) fn commit(&mut self, cx: &mut Context<Self>) {
        if self.refuse_while_opening(cx) {
            return;
        }
        let message = self.composer.read(cx).text(cx);
        if message.trim().is_empty() {
            self.notify_user("Nothing to commit: the message is empty.", cx);
            return;
        }
        if self.model.summary().staged == 0 {
            self.notify_user("Nothing is staged.", cx);
            return;
        }
        self.engine
            .dispatch(Actor::Human, CommandKind::Commit { message });
        cx.notify();
    }

    pub(crate) fn notify_user(&mut self, text: &str, cx: &mut Context<Self>) {
        let generation = self.model.set_notice(text.to_owned());
        Self::expire_notice_later(generation, cx);
        cx.notify();
    }

    /// Changes to the repository wait for an open in flight: they would run
    /// against the repository being left or the one being opened.
    fn refuse_while_opening(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.model.is_opening() {
            return false;
        }
        self.notify_user("Wait for the repository to open.", cx);
        true
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

    /// Tab and shift-Tab: list (and filter), then diff pane, then composer.
    fn cycle_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let in_list = self.list_focus.is_focused(window)
            || self.filter.read(cx).focus_handle(cx).is_focused(window);
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

    fn top_bar(&self, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let (branch, upstream, ahead, behind) = match self.model.status() {
            Some(status) => (
                status
                    .branch
                    .head
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_owned()),
                status.branch.upstream.clone().unwrap_or_default(),
                status.branch.ahead,
                status.branch.behind,
            ),
            None => (String::new(), String::new(), 0, 0),
        };
        let loading = if self.model.loading { "Loading…" } else { "" };
        // Same action as the `r` key: the refresh command, through `refresh`.
        let refresh = div()
            .id("top-refresh")
            .flex()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded_sm()
            .cursor_pointer()
            .text_color(muted)
            .hover(|style| style.bg(theme::selected_row()))
            .child(icon::view(Icon::ArrowClockwise, icon::SIZE, muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.refresh(cx)),
            );
        div()
            .h(px(36.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(self.project_button(cx))
            .child(icon::labelled(
                Icon::GitBranch,
                icon::SIZE,
                theme::text(),
                div().font_weight(FontWeight::SEMIBOLD).child(branch),
            ))
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
            .child(refresh)
    }

    fn status_bar(&self, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
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
            .child(
                div()
                    .px_2()
                    .rounded_sm()
                    .bg(tint)
                    .text_color(colour)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .children(notice_icon.map(|icon| icon::view(icon, icon::SMALL, colour)))
                    .child(message),
            )
            .child(self.command_log_button(cx))
    }
}

impl Render for StatusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drawn only when the compositor left the frame to the app; under
        // server decorations the title bar is the compositor's.
        let title_bar =
            window_chrome::draws_frame(window).then(|| window_chrome::title_bar(window));
        // The stored sizes are clamped again here, because a window resized
        // after a drag may have moved the limits.
        let viewport = window.viewport_size();
        let left_width = resize::LEFT.clamp(self.left_width, f32::from(viewport.width));
        let commit_height = resize::COMMIT.clamp(self.commit_height, f32::from(viewport.height));
        div()
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
            .children(title_bar)
            .child(self.top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(self.left_column(left_width, commit_height, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .child(self.diff_pane.clone()),
                    ),
            )
            .child(self.status_bar(cx))
            .children(self.log_open.then(|| self.command_log_panel(cx)))
            .children(
                self.project_menu
                    .open
                    .then(|| self.project_menu_layer(window, cx)),
            )
            .children(window_chrome::resize_layer(window))
    }
}
