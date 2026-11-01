//! The provenance panel of the Code page (P4, D144): asks why the picked lines are as they
//! are, draws the rows of the answer and opens the thread or commit a row names. The state
//! and the rows are in `provenance_view.rs`; this file is the window's part.

use std::ops::Range;

use gm_schema::{Actor, CommandKind};
use gpui::{
    Context, Div, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px, uniform_list,
};

use crate::action_registry::CodeCtx;
use crate::code_page::Content;
use crate::provenance_view::{Asked, Jump, Row};
use crate::shell_code::picked_lines;
use crate::theme;
use crate::view::{Screen, StatusView};

/// The height of one row. Every row is one line, so the list can be virtualized.
const ROW_HEIGHT: f32 = 22.;
/// The width of the panel, fixed so a long row cannot widen it.
const PANEL_WIDTH: f32 = 360.;
/// The indent of one nesting level of rows.
const INDENT: f32 = 14.;

impl StatusView {
    /// Asks why lines `from` to `to` of `path` are as they are, and opens the panel (P4). A
    /// file other than the one on screen is opened first, with the lines picked.
    pub(crate) fn show_provenance(&mut self, ask: &Asked, cx: &mut Context<Self>) {
        let (path, from, to) = (ask.path.as_str(), ask.from, ask.to);
        let shown = self
            .model
            .code
            .on_screen()
            .is_some_and(|shown| shown.path == path);
        if shown && self.screen == Screen::Code {
            self.show_code(cx);
        } else {
            self.view_file(path, Some(from), cx);
            let first = usize::try_from(from.saturating_sub(1)).unwrap_or(0);
            let last = usize::try_from(to.saturating_sub(1)).unwrap_or(first);
            self.code_view.picked = Some((first, last));
        }
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ReadProvenance {
                path: path.to_owned(),
                from_line: from,
                to_line: to,
            },
        );
        self.model.provenance.request(path, from, to, id);
        cx.notify();
    }

    /// What the registry's rules read of the Code viewer: the text file on screen and the
    /// lines picked in it. Nothing on any other page.
    pub(crate) fn code_ctx(&self) -> CodeCtx<'_> {
        if self.screen != Screen::Code {
            return CodeCtx::default();
        }
        let Some(shown) = self.model.code.on_screen() else {
            return CodeCtx::default();
        };
        CodeCtx {
            file: matches!(shown.content, Content::Text(_)).then_some(shown.path.as_str()),
            lines: self.code_view.picked.map(picked_lines),
        }
    }

    /// After the pick or the file of the viewer changed: an answer about other lines stops
    /// showing, and the panel says the selection moved (P4).
    pub(crate) fn provenance_follows_pick(&mut self) {
        let code = self.code_ctx();
        let now = code
            .file
            .zip(code.lines)
            .map(|(path, (from, to))| (path.to_owned(), from, to));
        let now = now
            .as_ref()
            .map(|(path, from, to)| (path.as_str(), *from, *to));
        self.model.provenance.selection_moved(now);
    }

    /// Opens the thread or the commit a panel row names.
    fn open_provenance_jump(&mut self, jump: Jump, window: &mut Window, cx: &mut Context<Self>) {
        match jump {
            Jump::Thread(uid) => self.open_thread_from_commit(&uid, window, cx),
            Jump::Edit { uid, entry_id, seq } => {
                self.open_thread_at(&uid, Some((entry_id, seq)), window, cx);
            }
            Jump::Commit(oid) => {
                self.show_history(cx);
                self.select_commit(&oid, cx);
            }
        }
    }

    /// The panel on the right of the Code viewer, when it is open.
    pub(crate) fn provenance_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.model.provenance.is_open() {
            return None;
        }
        let asked = self.model.provenance.asked()?.clone();
        let rows = self.model.provenance.rows();
        let count = rows.len();
        let body = uniform_list(
            "provenance-rows",
            count,
            cx.processor(
                move |_this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    range
                        .filter_map(|index| {
                            rows.get(index).map(|row| provenance_row(row, index, cx))
                        })
                        .collect()
                },
            ),
        )
        .track_scroll(self.code_view.provenance_scroll.clone())
        .flex_1()
        .min_h(px(0.));
        Some(
            div()
                .flex_none()
                .w(px(PANEL_WIDTH))
                .min_w(px(PANEL_WIDTH))
                .max_w(px(PANEL_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(theme::hairline())
                .bg(theme::panel())
                .child(
                    div()
                        .flex_none()
                        .h(px(32.))
                        .px_3()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .border_b_1()
                        .border_color(theme::hairline())
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(theme::text())
                                .child("Why This Code Is Here"),
                        )
                        .child(close_button(cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_xs()
                        .font_family(theme::code_font())
                        .text_color(theme::muted())
                        .child(format!("{} lines {}-{}", asked.path, asked.from, asked.to)),
                )
                .child(body),
        )
    }
}

/// The header button that closes the panel.
fn close_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("provenance-close")
        .flex_none()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .cursor_pointer()
        .child("Close")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.model.provenance.close();
                cx.notify();
            }),
        )
}

/// One row of the panel. A row that names a thread or a commit is clickable.
fn provenance_row(row: &Row, index: usize, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let indent = f32::from(u16::try_from(row.depth()).unwrap_or(u16::MAX)) * INDENT;
    let mut line = div()
        .id(("provenance-row", index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .pl(px(indent + 12.))
        .pr_3()
        .text_xs()
        .font_family(theme::code_font());
    if let Some(jump) = row.jump() {
        line = line
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.open_provenance_jump(jump.clone(), window, cx);
                }),
            );
    }
    match row {
        Row::Label { name, .. } => line
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::button_border())
                    .whitespace_nowrap()
                    .child(name.clone()),
            )
            .child(text_cell(&row.line(), theme::muted())),
        Row::Failure(_) => line.child(text_cell(&row.line(), theme::error())),
        Row::Note(_) | Row::Cut(_) | Row::Entry { .. } => {
            line.child(text_cell(&row.line(), theme::muted()))
        }
        Row::Commit { .. } | Row::Thread { .. } | Row::Link { .. } | Row::Edit { .. } => {
            line.child(text_cell(&row.line(), theme::text()))
        }
    }
}

/// A text cell that takes the rest of the row and cuts what does not fit.
fn text_cell(text: &str, color: gpui::Hsla) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(color)
        .child(text.to_owned())
}
