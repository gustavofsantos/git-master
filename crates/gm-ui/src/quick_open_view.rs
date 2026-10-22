//! The window side of Quick Open (design/code-navigation.md, N1): the layer that Ctrl/⌘+P opens,
//! its one input, the virtualized list of files and the keys. The state, the
//! highlight and the status line come from `quick_open.rs`. The engine finds the
//! files; each keystroke sends a search, and an answer to an older one is dropped
//! by its `OperationId`, so typing never waits for a result.

use std::ops::Range;

use gm_schema::{Actor, CommandKind, FileHit};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, ScrollStrategy,
    Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::icon::{self, Icon};
use crate::name_fit::name_text;
use crate::quick_open::{split_name, split_ranges};
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::theme;
use crate::view::StatusView;

const ROW_HEIGHT: f32 = 28.;
const WIDTH: f32 = 640.;
/// Quick Open hangs this far below the top of the window.
const TOP: f32 = 72.;
/// Ten rows are shown; the rest scroll.
const MAX_ROWS: usize = 10;

/// The window's part of Quick Open: the input it types into and the list's scroll.
pub(crate) struct QuickOpenView {
    pub(crate) input: Entity<TextArea>,
    pub(crate) scroll: UniformListScrollHandle,
}

impl QuickOpenView {
    pub(crate) fn new(input: Entity<TextArea>) -> Self {
        Self {
            input,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// The subscriptions that keep the input and the window in step: its text and
/// its Escape. Made with the window, before the first event.
pub(crate) fn subscriptions(
    input: &Entity<TextArea>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![
        cx.observe(input, |this, input, cx| {
            this.on_quick_open_changed(&input, cx);
        }),
        cx.subscribe_in(
            input,
            window,
            |this, _, event: &TextAreaEvent, window, cx| {
                if *event == TextAreaEvent::Cancel {
                    this.close_quick_open(window, cx);
                }
            },
        ),
    ]
}

impl StatusView {
    /// Opens Quick Open empty, asks the engine to read the file list again, so a
    /// file made a moment ago is found, and counts the files for the hint.
    pub(crate) fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_menu.open = false;
        self.branch_menu.open = false;
        self.context_menu.open = None;
        self.palette.close();
        self.model.quick_open.open();
        self.quick_open_view
            .input
            .update(cx, |area, cx| area.set_text("", cx));
        self.quick_open_view
            .input
            .update(cx, |area, cx| area.focus(window, cx));
        self.quick_open_view
            .scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.engine.dispatch(Actor::Human, CommandKind::IndexFiles);
        self.search_files();
        cx.notify();
    }

    /// Closes Quick Open and gives the keys back to the list.
    pub(crate) fn close_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.quick_open.close();
        self.list_focus.focus(window);
        cx.notify();
    }

    /// Sends a search for the text typed. A search in flight is superseded by
    /// the engine, and its late answer is dropped here.
    pub(crate) fn search_files(&mut self) {
        let query = self.model.quick_open.query().to_owned();
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::SearchFiles { query });
        self.model.quick_open.requested(id);
    }

    /// The input changed. Only its first line counts; a changed query searches.
    fn on_quick_open_changed(&mut self, input: &Entity<TextArea>, cx: &mut Context<Self>) {
        if !self.model.quick_open.open {
            return;
        }
        let text = input.read(cx).text().to_owned();
        if text.contains('\n') {
            // The input is one line; Enter is a choice, not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            input.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; search only on a real change.
        if !self.model.quick_open.set_query(&text) {
            return;
        }
        self.quick_open_view
            .scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.search_files();
        cx.notify();
    }

    /// Up and down: moves the highlight and keeps it on screen.
    fn move_quick_open_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        self.model
            .quick_open
            .move_highlight(if down { 1 } else { -1 });
        self.quick_open_view
            .scroll
            .scroll_to_item(self.model.quick_open.highlighted(), ScrollStrategy::Top);
        cx.notify();
    }

    /// Enter or a click: opens the highlighted file on the Code page, at the
    /// line the query named.
    fn choose_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.model.quick_open.choice() else {
            return;
        };
        self.close_quick_open(window, cx);
        self.view_file(&choice.path, choice.line, cx);
    }

    /// The layer: the panel, and under it a layer that closes it on a click
    /// outside. Drawn last, so it sits above the whole window. The arrows and
    /// Enter are taken here, before the input sees them.
    pub(crate) fn quick_open_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_quick_open(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let view = cx.entity().downgrade();
        let up = view.clone();
        let down = view.clone();
        let enter = view;
        div().absolute().inset_0().child(outside).child(
            self.quick_open_panel(left, cx)
                .capture_action(move |_: &Up, _, cx| {
                    let _ = up.update(cx, |this, cx| this.move_quick_open_highlight(false, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &Down, _, cx| {
                    let _ = down.update(cx, |this, cx| this.move_quick_open_highlight(true, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &Newline, window, cx| {
                    let _ = enter.update(cx, |this, cx| this.choose_quick_open(window, cx));
                    cx.stop_propagation();
                }),
        )
    }

    /// The panel: the input, the list or a line saying there is nothing to show,
    /// the status line and the key hints.
    fn quick_open_panel(&self, left: f32, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let quick = &self.model.quick_open;
        let count = quick.hits().len();
        div()
            .absolute()
            .top(px(TOP))
            .left(px(left))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .py_1()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(icon::view(Icon::MagnifyingGlass, icon::SIZE, muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(self.quick_open_view.input.clone()),
                    ),
            )
            .children((count > 0).then(|| self.quick_open_list(count, cx)))
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .when(count > 0, |footer| {
                        footer.border_t_1().border_color(theme::hairline())
                    })
                    .child(div().child(quick.status()))
                    .child(div().child("↑↓ Navigate   Enter Open   Esc Close")),
            )
    }

    fn quick_open_list(&self, count: usize, cx: &Context<Self>) -> AnyElement {
        let rows = u16::try_from(count.min(MAX_ROWS)).unwrap_or(1);
        uniform_list(
            "quick-open-rows",
            count,
            cx.processor(
                |this: &mut StatusView,
                 range: Range<usize>,
                 _window: &mut Window,
                 cx: &mut Context<StatusView>| {
                    let quick = &this.model.quick_open;
                    quick
                        .hits()
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(hit, index)| file_row(hit, index, index == quick.highlighted(), cx))
                        .collect()
                },
            ),
        )
        .track_scroll(self.quick_open_view.scroll.clone())
        .h(px(ROW_HEIGHT * f32::from(rows)))
        .into_any_element()
    }
}

/// A row: the file name, then its folder fainter, the matched letters in the
/// accent colour. A click opens the file.
fn file_row(
    hit: &FileHit,
    index: usize,
    highlighted: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let (folder, name) = split_name(&hit.path);
    let (folder_marks, name_marks) = split_ranges(&hit.path, &hit.ranges);
    let folder = folder.trim_end_matches('/');
    div()
        .id(("quick-open-row", index))
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(highlighted, |row| row.bg(theme::selected_row()))
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_weight(FontWeight::MEDIUM)
                .child(name_text(name, &name_marks)),
        )
        .children((!folder.is_empty()).then(|| {
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_xs()
                .text_color(theme::muted())
                .child(name_text(folder, &folder_marks))
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.model.quick_open.highlight(index);
                this.choose_quick_open(window, cx);
            }),
        )
}
