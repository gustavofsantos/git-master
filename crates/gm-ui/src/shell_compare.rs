//! The Branch and Compare tabs of the Changes page (D231): the tab bar over the left
//! column, the Base picker and the summary of the Branch tab, the changed files of the
//! shown range, and the read-only diff of the file picked. Every decision is in
//! `compare.rs`; this file draws it and sends what it asks for.

use std::ops::Range;

use gm_schema::{Actor, CommandKind, CommitFile};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px,
    uniform_list,
};

use crate::compare::{self, BaseRow, ChangesTab, RangeView, Summary};
use crate::context_menu::Target;
use crate::shell_history::file_row;
use crate::theme;
use crate::view::StatusView;

const TAB_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 24.;
/// The Base picker shows twelve rows; a longer list scrolls.
const PICKER_ROWS: u8 = 12;

impl StatusView {
    /// The three tabs at the top of the left column (D231 (1)). The active one is underlined.
    pub(crate) fn changes_tab_bar(&self, cx: &Context<Self>) -> Div {
        let current = self.model.compare.tab;
        div()
            .px_3()
            .h(px(TAB_HEIGHT))
            .flex_none()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .border_b_1()
            .border_color(theme::hairline())
            .children(ChangesTab::ALL.into_iter().map(|tab| {
                let selected = tab == current;
                div()
                    .id(tab.label())
                    .px_2()
                    .h(px(TAB_HEIGHT - 4.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::hairline()
                    })
                    .when(selected, |item| item.font_weight(FontWeight::MEDIUM))
                    .when(!selected, |item| item.text_color(theme::muted()))
                    .child(tab.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.show_changes_tab(tab, cx);
                        }),
                    )
            }))
    }

    /// The Branch and Compare column below the tab bar: the tab area, the summary and the
    /// changed files.
    pub(crate) fn compare_column(&self, cx: &Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(self.compare_tab_area(cx))
            .child(self.compare_summary())
            .child(self.compare_list(cx))
    }

    /// The tab area: the Base picker of the Branch tab. The Compare tab's fields come
    /// with its own slice; until then it says what to type first.
    fn compare_tab_area(&self, cx: &Context<Self>) -> Div {
        match self.model.compare.tab {
            ChangesTab::Branch => self.base_picker(cx),
            _ => div()
                .px_3()
                .py_2()
                .text_color(theme::muted())
                .child("Type the older revision in From."),
        }
    }

    /// The Base button, showing the base's name, and the list it opens (D231 (2)).
    fn base_picker(&self, cx: &Context<Self>) -> Div {
        let shown_from = self
            .model
            .compare
            .branch_view
            .range()
            .map(|range| range.from_name.as_str());
        let label = compare::base_label(self.model.compare.base.as_deref(), shown_from);
        let button = div()
            .id("compare-base")
            .mx_3()
            .my_2()
            .px_2()
            .h(px(ROW_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .cursor_pointer()
            .child(div().flex_none().text_color(theme::muted()).child("Base"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.toggle_base_picker(cx)),
            );
        div().flex_none().flex().flex_col().child(button).children(
            self.model
                .compare
                .base_open
                .then(|| self.base_picker_list(cx)),
        )
    }

    /// The rows of the Base picker, virtualized and at most `PICKER_ROWS` tall.
    fn base_picker_list(&self, cx: &Context<Self>) -> Div {
        let rows = self.base_rows();
        let count = rows.len();
        let chosen = self.model.compare.base.clone();
        let visible = u8::try_from(count).map_or(PICKER_ROWS, |n| n.min(PICKER_ROWS));
        let height = ROW_HEIGHT * f32::from(visible);
        div()
            .mx_3()
            .mb_2()
            .h(px(height))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .child(
                uniform_list(
                    "compare-base-rows",
                    count,
                    cx.processor(
                        move |_this: &mut StatusView,
                              range: Range<usize>,
                              _window: &mut Window,
                              cx: &mut Context<StatusView>| {
                            rows.get(range.clone())
                                .unwrap_or_default()
                                .iter()
                                .zip(range)
                                .map(|(row, index)| {
                                    let selected = row.name == chosen;
                                    base_row(row, index, selected, cx).into_any_element()
                                })
                                .collect()
                        },
                    ),
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }

    /// The Base picker's rows, the stack parent first and the default branch at the top.
    fn base_rows(&self) -> Vec<BaseRow> {
        let head = self.head_name();
        let parent = compare::stack_parent(self.model.stacks.snapshot(), head.as_deref());
        compare::base_rows(
            self.model.branches.known(),
            parent.as_deref(),
            head.as_deref(),
        )
    }

    /// The summary line of the shown range: its header and totals, the read in progress, or
    /// git's message (D231 (2), (3)).
    fn compare_summary(&self) -> Div {
        let line = div().px_3().py_2().flex().flex_col().gap_0p5();
        let Some(view) = self.model.compare.tab_view() else {
            return line;
        };
        match compare::summary(self.model.compare.tab, view) {
            Summary::Idle => line,
            Summary::Reading => line.text_color(theme::muted()).child("Reading…"),
            Summary::Failed(message) => line.text_color(theme::error()).child(message),
            Summary::Shown { header, totals } => line
                .child(div().font_weight(FontWeight::MEDIUM).child(header))
                .child(div().text_xs().text_color(theme::muted()).child(totals)),
        }
    }

    /// The changed files of the shown range, each opening its diff on a click. A range with
    /// no files says why, in place of the list.
    fn compare_list(&self, cx: &Context<Self>) -> Div {
        let tab = self.model.compare.tab;
        let shown = self.model.compare.tab_view().and_then(RangeView::range);
        let Some(range) = shown else {
            return div().flex_1().min_h(px(0.));
        };
        if range.files.is_empty() {
            return div()
                .px_3()
                .py_2()
                .text_color(theme::muted())
                .child(compare::empty_notice(tab, range));
        }
        let count = range.files.len();
        let more = compare::more_line(range.more_files);
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .track_focus(&self.list_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window: &mut Window, _| {
                    this.list_focus.focus(window);
                }),
            )
            .child(
                uniform_list(
                    "compare-files",
                    count,
                    cx.processor(
                        |this: &mut StatusView,
                         range: Range<usize>,
                         _window: &mut Window,
                         cx: &mut Context<StatusView>| {
                            let Some(shown) =
                                this.model.compare.tab_view().and_then(RangeView::range)
                            else {
                                return Vec::new();
                            };
                            shown
                                .files
                                .get(range.clone())
                                .unwrap_or_default()
                                .iter()
                                .zip(range)
                                .map(|(file, index)| {
                                    compare_row(file, index, cx).into_any_element()
                                })
                                .collect()
                        },
                    ),
                )
                .flex_1()
                .min_h(px(0.)),
            )
            .children(more.map(|text| {
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(text)
            }))
    }

    /// The diff of the file picked, read-only, in the main area of the two tabs (D231 (4)).
    /// The pane stays in the tree while the diff loads, so it keeps the keys.
    pub(crate) fn compare_diff_area(&self) -> Div {
        let stack = &self.model.compare_stack;
        let loading = !stack.is_shown();
        let notice = match (stack.failure(), stack.is_open()) {
            (Some(message), _) => Some((theme::error(), message.to_owned())),
            (None, false) => Some((theme::muted(), "Select a file to see its diff.".to_owned())),
            (None, true) if loading => Some((theme::muted(), "Loading the diff…".to_owned())),
            (None, true) => None,
        };
        let pane = div()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(loading, |this| this.h(px(0.)))
            .when(!loading, gpui::Styled::flex_1)
            .child(self.compare_diff_pane.clone());
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .children(notice.map(|(color, text)| div().px_4().py_3().text_color(color).child(text)))
            .child(pane)
    }

    /// Asks the Base picker to open or close. It opens with the branches read again, so
    /// the list is current.
    pub(crate) fn toggle_base_picker(&mut self, cx: &mut Context<Self>) {
        self.model.compare.toggle_base_menu();
        if self.model.compare.base_open {
            self.request_branch_list();
        }
        cx.notify();
    }

    /// Takes a base from the picker and reads the Branch tab against it.
    pub(crate) fn pick_base(&mut self, base: Option<String>, cx: &mut Context<Self>) {
        self.model.compare.set_base(base);
        self.read_changes_tab(cx);
        cx.notify();
    }

    /// Opens the stacked diff of the changed file `index` of the shown range, with that file at
    /// the top (D231 (4)). The read is the range's diff, cut into its files.
    pub(crate) fn select_compare_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(range) = self.model.compare.tab_view().and_then(RangeView::range) else {
            return;
        };
        if index >= range.files.len() {
            return;
        }
        let (base, tip) = (range.base.clone(), range.tip.clone());
        let (files, more) = (range.files.clone(), range.more_files);
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadRevisionStack {
                base,
                tip,
                files: files.clone(),
            },
        );
        self.model.compare_stack.request(id, index, files, more);
        cx.notify();
    }

    /// A click on a changed file: opens its diff and gives the diff pane the keys.
    pub(crate) fn open_compare_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_compare_file(index, cx);
        let handle = self.compare_diff_pane.read(cx).focus_handle();
        window.focus(&handle);
    }

    /// Escape in the diff: the file's diff closes and the list is shown alone.
    pub(crate) fn close_compare_file(&mut self, cx: &mut Context<Self>) {
        self.model.compare_stack.close();
        cx.notify();
    }
}

/// One changed file of the shown range. A right click opens its menu (Copy File Path).
fn compare_row(file: &CommitFile, index: usize, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let path = file.path.clone();
    file_row(file, index, "compare-file", cx, move |this, window, cx| {
        this.open_compare_file(index, window, cx);
    })
    .on_mouse_down(
        MouseButton::Right,
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.open_menu(Target::CompareFile(path.clone()), event.position, cx);
        }),
    )
}

/// One row of the Base picker: the name, the **parent** mark, and the chosen base highlighted.
fn base_row(
    row: &BaseRow,
    index: usize,
    selected: bool,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let label = row
        .name
        .clone()
        .unwrap_or_else(|| "Default branch".to_owned());
    let name = row.name.clone();
    div()
        .id(("compare-base-row", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(selected, |item| item.bg(theme::selected_row()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
        .children(row.parent.then(|| {
            div()
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child("parent")
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.pick_base(name.clone(), cx);
            }),
        )
}
