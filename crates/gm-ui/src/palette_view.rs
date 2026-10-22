//! The command palette's window side (slice 1c, item 4): the layer that Ctrl+K
//! opens, its one input, the virtualized list and the keys. The rows, the
//! ranking and the highlight come from `palette.rs`. A choice runs through
//! `run_action`, like a key, a button or a menu item. Ranking runs on the
//! background executor, so typing never waits for it.

use std::ops::Range;

use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, ScrollStrategy,
    Stateful, Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::FocusNext;
use crate::action_registry::{Action, ArgKind};
use crate::icon::{self, Icon};
use crate::name_fit::{self, name_label};
use crate::palette::{self, Candidate, Choice, Hit, Line, Palette, Stage};
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::theme;
use crate::view::StatusView;
use crate::window_chrome;

const ROW_HEIGHT: f32 = 24.;
const WIDTH: f32 = 560.;
/// The palette hangs this far below the top of the window.
const TOP: f32 = 72.;
/// Twelve lines are shown; a longer list scrolls.
const MAX_LINES: u16 = 12;

/// The window's part of the palette: the input it types into and the list's scroll.
pub(crate) struct PaletteView {
    pub(crate) filter: Entity<TextArea>,
    pub(crate) scroll: UniformListScrollHandle,
}

impl PaletteView {
    pub(crate) fn new(filter: Entity<TextArea>) -> Self {
        Self {
            filter,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// The subscriptions that keep the palette's input and the window in step: its
/// text and its Escape. Made with the window, before the first event.
pub(crate) fn subscriptions(
    filter: &Entity<TextArea>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![
        cx.observe(filter, |this, filter, cx| {
            this.on_palette_filter_changed(&filter, cx);
        }),
        cx.subscribe_in(
            filter,
            window,
            |this, _, event: &TextAreaEvent, window, cx| {
                this.on_palette_filter_event(*event, window, cx);
            },
        ),
    ]
}

impl StatusView {
    /// Ctrl+K: opens the palette, or closes it when it is open.
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.open {
            self.close_palette(window, cx);
        } else {
            self.open_palette(window, cx);
        }
    }

    /// Opens the top level, with the other menus closed and the branches asked for.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_menu.open = false;
        self.branch_menu.open = false;
        self.context_menu.open = None;
        self.palette.open();
        self.palette_view
            .filter
            .update(cx, |area, cx| area.set_text("", cx));
        self.palette_view
            .filter
            .update(cx, |area, cx| area.focus(window, cx));
        self.request_branch_list();
        self.refresh_palette(cx);
        cx.notify();
    }

    /// Closes the palette and gives the keys back to the change list.
    pub(crate) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette.close();
        self.list_focus.focus(window);
        cx.notify();
    }

    /// Builds the candidates from the window's state now, then ranks them on the
    /// background executor. The list keeps the last ranking that arrived until
    /// this one lands.
    pub(crate) fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let message = self.composer.read(cx).text(cx);
        let candidates = {
            let ctx = self.action_context(&message);
            palette::build(
                self.palette.stage(),
                self.palette.query(),
                &ctx,
                self.model.branches.known(),
            )
        };
        let job = self.palette.begin(candidates);
        cx.spawn(async move |this, cx| {
            let ranked = cx
                .background_executor()
                .spawn(async move { job.run() })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.palette.apply(ranked) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The input and the list start over: used when the query or the step changes.
    fn reset_palette_input(&mut self, cx: &mut Context<Self>) {
        self.palette_view
            .filter
            .update(cx, |area, cx| area.set_text("", cx));
        self.palette_view
            .scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.refresh_palette(cx);
        cx.notify();
    }

    /// Opens the argument step of `action`, where the target is chosen.
    fn enter_palette_pick(&mut self, action: Action, cx: &mut Context<Self>) {
        self.palette.enter_pick(action);
        self.reset_palette_input(cx);
    }

    /// Enter or a click on the highlighted row.
    fn choose_palette_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.palette.highlighted().cloned() else {
            return;
        };
        self.choose_candidate(candidate, window, cx);
    }

    fn choose_candidate(
        &mut self,
        candidate: Candidate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (candidate.choice, candidate.state) {
            // An action that takes a target opens the step where the target is chosen.
            (Choice::Pick(action), Ok(())) => self.enter_palette_pick(action, cx),
            (Choice::Run { action, args }, Ok(())) => {
                self.close_palette(window, cx);
                self.run_action(action, &args, window, cx);
            }
            // A refused row closes the palette and says why, as the menus do.
            (_, Err(reason)) => {
                self.close_palette(window, cx);
                self.notify_user(reason, cx);
            }
        }
    }

    /// Tab picks the argument of the highlighted action. On any other row it does nothing.
    fn palette_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.palette.highlighted().cloned() else {
            return;
        };
        if matches!(candidate.choice, Choice::Pick(_)) {
            self.choose_candidate(candidate, window, cx);
        }
    }

    /// Escape leaves an argument step; from the top level it closes the palette.
    pub(crate) fn on_palette_filter_event(
        &mut self,
        event: TextAreaEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event != TextAreaEvent::Cancel {
            return;
        }
        if self.palette.leave_pick() {
            self.reset_palette_input(cx);
        } else {
            self.close_palette(window, cx);
        }
    }

    /// The input changed. Only its first line counts; a change of query ranks again.
    pub(crate) fn on_palette_filter_changed(
        &mut self,
        filter: &Entity<TextArea>,
        cx: &mut Context<Self>,
    ) {
        if !self.palette.open {
            return;
        }
        let text = filter.read(cx).text().to_owned();
        if text.contains('\n') {
            // The input is one line; Enter is a choice, not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            filter.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; redraw only on a real change.
        if text == self.palette.query() {
            return;
        }
        self.palette.set_query(&text);
        self.palette_view
            .scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.refresh_palette(cx);
        cx.notify();
    }

    /// Up and down: moves the highlight and keeps it on screen.
    fn move_palette_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        self.palette.move_highlight(down);
        if let Some(line) = self.palette.highlight_line() {
            self.palette_view
                .scroll
                .scroll_to_item(line, ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// The palette and the layer under it that closes it on a click outside.
    /// Drawn last, so it sits above the whole window. Tab, the arrows and Enter
    /// are taken here, before the input sees them.
    pub(crate) fn palette_layer(&self, window: &Window, cx: &Context<Self>) -> Div {
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_palette(window, cx);
            }),
        );
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let view = cx.entity().downgrade();
        let tab = view.clone();
        let up = view.clone();
        let down = view.clone();
        let enter = view;
        let panel = self.palette_panel(left, cx);
        div().absolute().inset_0().child(outside).child(
            panel
                .capture_action(move |_: &FocusNext, window, cx| {
                    let _ = tab.update(cx, |this, cx| this.palette_tab(window, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &Up, _, cx| {
                    let _ = up.update(cx, |this, cx| this.move_palette_highlight(false, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &Down, _, cx| {
                    let _ = down.update(cx, |this, cx| this.move_palette_highlight(true, cx));
                    cx.stop_propagation();
                })
                .capture_action(move |_: &Newline, window, cx| {
                    let _ = enter.update(cx, |this, cx| this.choose_palette_row(window, cx));
                    cx.stop_propagation();
                }),
        )
    }

    /// The prompt of the current step, if it has one, and the key hints of the
    /// step.
    fn palette_step(&self) -> (Option<String>, &'static [&'static str]) {
        match self.palette.stage() {
            Stage::Actions => (
                None,
                &[
                    "↑↓ Navigate",
                    "Enter Run",
                    "Tab Choose argument",
                    "Esc Close",
                ],
            ),
            Stage::Pick(action) => {
                let what = match action.spec().args {
                    ArgKind::File => "a file",
                    ArgKind::Worktree => "a worktree",
                    ArgKind::Workflow => "a workflow",
                    ArgKind::Template => "a template",
                    ArgKind::Stash => "a stash",
                    _ => "a branch",
                };
                (
                    Some(format!("{}: choose {what}", action.spec().title)),
                    &["↑↓ Navigate", "Enter Run", "Esc Back"],
                )
            }
        }
    }

    /// The list, or a line saying there is nothing to show.
    fn palette_list(&self, lines: &[Line], cx: &Context<Self>) -> AnyElement {
        let muted = theme::muted();
        if lines.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_color(muted)
                .child("No matches.")
                .into_any_element()
        } else {
            let rows = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .min(MAX_LINES);
            let height = ROW_HEIGHT * f32::from(rows);
            uniform_list(
                "palette-lines",
                lines.len(),
                cx.processor(
                    |this: &mut StatusView,
                     range: Range<usize>,
                     _window: &mut Window,
                     cx: &mut Context<StatusView>| {
                        let lines = this.palette.lines();
                        lines
                            .get(range.clone())
                            .unwrap_or_default()
                            .iter()
                            .zip(range)
                            .map(|(line, index)| line_element(&this.palette, line, index, cx))
                            .collect()
                    },
                ),
            )
            .track_scroll(self.palette_view.scroll.clone())
            .h(px(height))
            .into_any_element()
        }
    }

    /// The panel: the step's prompt, the input with the result count, the list
    /// and the key hints.
    fn palette_panel(&self, left: f32, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let lines = self.palette.lines();
        let count = self.palette.count();
        let (prompt, hints) = self.palette_step();
        let summary = match count {
            1 => "1 result".to_owned(),
            n => format!("{n} results"),
        };
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
            .children(
                prompt.map(|text| div().px_3().pt_1().text_xs().text_color(muted).child(text)),
            )
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
                            .child(self.palette_view.filter.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .whitespace_nowrap()
                            .text_color(muted)
                            .child(summary),
                    ),
            )
            .child(self.palette_list(&lines, cx))
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .border_t_1()
                    .border_color(theme::hairline())
                    .children(hints.iter().map(|hint| div().child(*hint))),
            )
    }
}

/// One line of the list: a section's header, or a row.
fn line_element(
    palette: &Palette,
    line: &Line,
    index: usize,
    cx: &Context<StatusView>,
) -> AnyElement {
    match *line {
        Line::Header(section) => div()
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::muted())
            .child(section.title())
            .into_any_element(),
        Line::Row { hit, highlighted } => match palette.row(hit) {
            Some((candidate, found)) => {
                row_element(candidate, found, hit, highlighted, index, cx).into_any_element()
            }
            None => div().into_any_element(),
        },
    }
}

/// A row: its label with the matched parts in the accent colour, then its
/// shortcut or, when it cannot run, its reason. A click runs it, or opens its
/// argument step.
fn row_element(
    candidate: &Candidate,
    found: &Hit,
    position: usize,
    highlighted: bool,
    index: usize,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let enabled = candidate.state.is_ok();
    let colour = if enabled {
        theme::text()
    } else {
        theme::faint()
    };
    let note = match candidate.state {
        Err(reason) => Some(reason.to_owned()),
        Ok(()) => candidate.shortcut.clone(),
    };
    let full = candidate.label.clone();
    div()
        .id(("palette-row", index))
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .text_color(colour)
        .when(highlighted, |row| row.bg(theme::selected_row()))
        .when(enabled, |row| {
            row.cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
        })
        .when(name_fit::needs_hover(&full), |row| {
            row.tooltip(window_chrome::hint(full))
        })
        .child(name_label(&candidate.label, &found.ranges))
        .children(note.map(|text| {
            div()
                .flex_none()
                .text_xs()
                .whitespace_nowrap()
                .text_color(theme::muted())
                .child(text)
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.palette.select(position);
                this.choose_palette_row(window, cx);
            }),
        )
}
