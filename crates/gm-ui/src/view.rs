//! The Working Copy window: the top bar, the left column (filter, commit box,
//! file list), the diff pane on the right and the status bar. It sends
//! commands to the engine and renders the model; it never waits for git.

use std::time::Duration;

use gm_core::EngineHandle;
use gm_schema::{Actor, CommandKind, DiffSide, Event, FileDiff, StageTarget};
use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, FontWeight, MouseButton, MouseDownEvent, Render,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::changes::ToggleAction;
use crate::composer::{Composer, ComposerEvent};
use crate::diff_pane::{DiffPane, DiffPaneEvent};
use crate::icon::{self, Icon};
use crate::model::Model;
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::{FocusNext, FocusPrev};

/// The key context of the change list. Its keys work only while the list has
/// focus, so typing into the filter or the message is never taken as a key.
pub const KEY_CONTEXT: &str = "GitMaster";

/// How long a notice stays on the status bar.
const NOTICE_TIME: Duration = Duration::from_secs(4);

pub struct StatusView {
    pub(crate) engine: EngineHandle,
    pub(crate) model: Model,
    pub(crate) list_focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) filter: Entity<TextArea>,
    pub(crate) composer: Entity<Composer>,
    pub(crate) diff_pane: Entity<DiffPane>,
    _subscriptions: Vec<Subscription>,
}

impl StatusView {
    /// Opens `path` in the engine. Events are streamed into the view from a
    /// task on the UI thread; the subscription is made before the command so
    /// no event is missed.
    pub fn new(
        engine: EngineHandle,
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
        engine.dispatch(Actor::Human, CommandKind::OpenRepo { path });
        theme::follow(window.appearance());
        let list_focus = cx.focus_handle();
        list_focus.focus(window);
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
            cx.observe_window_appearance(window, |_, window, cx| {
                if theme::follow(window.appearance()) {
                    cx.notify();
                }
            }),
        ];
        let mut model = Model::default();
        model.begin_refresh();
        Self {
            engine,
            model,
            list_focus,
            scroll: UniformListScrollHandle::new(),
            filter,
            composer,
            diff_pane,
            _subscriptions: subscriptions,
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let reload = matches!(event, Event::StatusLoaded { .. });
        let committed = matches!(event, Event::Committed { .. });
        let shown = self.model.apply(event);
        if committed {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
            Self::expire_notice_later(self.model.notice_generation(), cx);
        }
        if let Some(diffs) = shown {
            self.show_diffs(diffs.unstaged, diffs.staged, cx);
        }
        if reload {
            self.load_selected(cx);
        }
        cx.notify();
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

    fn notify_user(&mut self, text: &str, cx: &mut Context<Self>) {
        let generation = self.model.set_notice(text.to_owned());
        Self::expire_notice_later(generation, cx);
        cx.notify();
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

    fn status_bar(&self) -> Div {
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
    }
}

impl Render for StatusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
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
            .child(self.top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(self.left_column(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .child(self.diff_pane.clone()),
                    ),
            )
            .child(self.status_bar())
    }
}
