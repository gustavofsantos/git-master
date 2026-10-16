//! The status window's view: a header for the branch and a virtualized list
//! of changed paths. It sends commands to the engine and renders the model;
//! it never waits for git.

use std::ops::Range;

use gm_core::EngineHandle;
use gm_schema::{Actor, CommandKind, Event, StatusEntry};
use gpui::{
    Context, Div, FocusHandle, FontWeight, Render, ScrollStrategy, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};

use crate::model::{Model, entry_label};
use crate::theme::{self, color};
use crate::{Refresh, SelectNext, SelectPrev};

/// The key context the key bindings in `lib.rs` are scoped to.
pub const KEY_CONTEXT: &str = "GitMaster";

pub struct StatusView {
    engine: EngineHandle,
    model: Model,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
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
        let focus = cx.focus_handle();
        focus.focus(window);
        let model = Model {
            loading: true,
            ..Model::default()
        };
        Self {
            engine,
            model,
            focus,
            scroll: UniformListScrollHandle::new(),
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        self.model.apply(event);
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.model.begin_refresh();
        self.engine
            .dispatch(Actor::Human, CommandKind::RefreshStatus);
        cx.notify();
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        if down {
            self.model.select_next();
        } else {
            self.model.select_prev();
        }
        self.scroll
            .scroll_to_item(self.model.selected, ScrollStrategy::Top);
        cx.notify();
    }

    fn header(&self) -> Div {
        let (branch, upstream, sync, changes) = match &self.model.status {
            Some(status) => {
                let branch = status
                    .branch
                    .head
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_owned());
                let upstream = status
                    .branch
                    .upstream
                    .clone()
                    .unwrap_or_else(|| "no upstream".to_owned());
                let sync = format!("↑{} ↓{}", status.branch.ahead, status.branch.behind);
                let count = status.entries.len();
                let noun = if count == 1 { "change" } else { "changes" };
                (branch, upstream, sync, format!("{count} {noun}"))
            }
            None => (String::new(), String::new(), String::new(), String::new()),
        };
        let loading = if self.model.loading { "Loading…" } else { "" };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .px_4()
            .py_3()
            .bg(color(theme::PANEL))
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(branch),
            )
            .child(div().text_color(color(theme::MUTED)).child(upstream))
            .child(div().font_family(theme::code_font()).child(sync))
            .child(div().text_color(color(theme::MUTED)).child(changes))
            .child(div().flex_1())
            .child(div().text_color(color(theme::MUTED)).child(loading))
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        let count = self.model.entry_count();
        let placeholder = match &self.model.status {
            None if self.model.loading => Some("Loading…"),
            None => Some("No repository is open."),
            Some(_) if count == 0 => Some("Working tree clean."),
            Some(_) => None,
        };
        if let Some(text) = placeholder {
            return div()
                .flex_1()
                .px_4()
                .py_3()
                .text_color(color(theme::MUTED))
                .child(text);
        }
        let list = uniform_list(
            "status-entries",
            count,
            cx.processor(
                |this: &mut StatusView,
                 range: Range<usize>,
                 _window: &mut Window,
                 _cx: &mut Context<StatusView>| {
                    let selected = this.model.selected;
                    let Some(status) = &this.model.status else {
                        return Vec::new();
                    };
                    status
                        .entries
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(entry, index)| row(entry, index == selected))
                        .collect()
                },
            ),
        )
        .track_scroll(self.scroll.clone())
        .flex_1()
        .min_h(px(0.));
        div().flex_1().flex().flex_col().child(list)
    }
}

impl Render for StatusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.model.error.clone().map(|message| {
            div()
                .px_4()
                .py_2()
                .bg(color(theme::PANEL))
                .text_color(color(theme::ERROR))
                .child(message)
        });
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::BACKGROUND))
            .text_color(color(theme::TEXT))
            .font_family(theme::ui_font())
            .text_sm()
            .child(self.header())
            .children(error)
            .child(self.body(cx))
    }
}

/// One changed path: the two status letters, then the path, and for a rename
/// the path it came from.
fn row(entry: &StatusEntry, selected: bool) -> Div {
    let (staged, unstaged) = entry_label(entry);
    let path = match &entry.orig_path {
        Some(orig) => format!("{}  ← {orig}", entry.path),
        None => entry.path.clone(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .h(px(24.))
        .when(selected, |row| row.bg(color(theme::SELECTED)))
        .child(
            div()
                .font_family(theme::code_font())
                .text_color(color(letter_color(staged, theme::STAGED)))
                .child(staged.to_string()),
        )
        .child(
            div()
                .font_family(theme::code_font())
                .text_color(color(letter_color(unstaged, theme::UNSTAGED)))
                .child(unstaged.to_string()),
        )
        .child(div().child(path))
}

/// A letter's color: its side's color, or a marker color for the special ones.
fn letter_color(letter: char, side: u32) -> u32 {
    match letter {
        '?' => theme::UNTRACKED,
        'U' => theme::ERROR,
        ' ' => theme::MUTED,
        _ => side,
    }
}
