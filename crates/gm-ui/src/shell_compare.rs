//! The Branch and Compare tabs of the Changes page (D231): the tab bar over the left
//! column, the Base picker and the summary of the Branch tab, the changed files of the
//! shown range, and the read-only diff of the file picked. Every decision is in
//! `compare.rs`; this file draws it and sends what it asks for.

use std::ops::Range;

use gm_schema::{Actor, CommandKind, CommitFile, RangeMode};
use gpui::{
    Context, Div, Entity, Focusable, FontWeight, MouseButton, MouseDownEvent, Stateful,
    Subscription, Window, div, prelude::*, px, uniform_list,
};

use crate::compare::{self, BaseRow, ChangesTab, Field, RangeView, Summary};
use crate::context_menu::Target;
use crate::shell_history::file_row;
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::theme;
use crate::view::StatusView;

const TAB_HEIGHT: f32 = 32.;
const ROW_HEIGHT: f32 = 24.;
/// The label beside a Compare field: wide enough for "From" and "To".
const FIELD_LABEL_WIDTH: f32 = 40.;
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

    /// The tab area: the Base picker of the Branch tab, or the fields of the Compare tab.
    fn compare_tab_area(&self, cx: &Context<Self>) -> Div {
        match self.model.compare.tab {
            ChangesTab::Branch => self.base_picker(cx),
            ChangesTab::Compare => self.compare_fields(cx),
            ChangesTab::Uncommitted => div(),
        }
    }

    /// The **From** and **To** fields, then the **Mode** selector. A From still empty says
    /// what to type (D231 (3)).
    fn compare_fields(&self, cx: &Context<Self>) -> Div {
        let hint = self.model.compare.from.trim().is_empty().then(|| {
            div()
                .px_3()
                .pb_1()
                .text_color(theme::muted())
                .child("Type the older revision in From.")
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .py_1()
            .child(self.field_row(Field::From, cx))
            .child(self.field_row(Field::To, cx))
            .child(self.mode_selector(cx))
            .children(hint)
    }

    /// One field: its label and text box, with its suggestions under it while it has focus.
    /// **To** also carries the **Swap** button.
    fn field_row(&self, field: Field, cx: &Context<Self>) -> Div {
        let (label, area, scope) = match field {
            Field::From => ("From", &self.compare_from, "compare-from"),
            Field::To => ("To", &self.compare_to, "compare-to"),
        };
        // The keys the list uses reach it first, while the field has focus: the area never
        // sees them. Up and Down are taken only while the list is open.
        let view = cx.entity().downgrade();
        let (up, down, enter) = (view.clone(), view.clone(), view);
        let row = div()
            .mx_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .capture_action(move |_: &Up, _, cx| {
                let taken = up
                    .update(cx, |this, cx| this.move_compare_highlight(false, cx))
                    .unwrap_or(false);
                if taken {
                    cx.stop_propagation();
                }
            })
            .capture_action(move |_: &Down, _, cx| {
                let taken = down
                    .update(cx, |this, cx| this.move_compare_highlight(true, cx))
                    .unwrap_or(false);
                if taken {
                    cx.stop_propagation();
                }
            })
            .capture_action(move |_: &Newline, _, cx| {
                let _ = enter.update(cx, StatusView::enter_compare_field);
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex_none()
                    .w(px(FIELD_LABEL_WIDTH))
                    .text_color(theme::muted())
                    .child(label),
            )
            .child(div().flex_1().min_w(px(0.)).child(area.clone()))
            .children((field == Field::To).then(|| swap_button(cx)));
        let listed = self.model.compare.focus.listed() == Some(field);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(row)
            .children(listed.then(|| self.suggestion_list(field, scope, cx)))
    }

    /// The **Direct | Since common ancestor** switch (D231 (3)), with the look of the diff's
    /// **Unified | Split**.
    fn mode_selector(&self, cx: &Context<Self>) -> Div {
        let current = self.model.compare.mode;
        let segment = |mode: RangeMode, label: &'static str| {
            let active = current == mode;
            div()
                .id(label)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .when(active, |this| {
                    this.bg(theme::panel())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                })
                .when(!active, |this| this.text_color(theme::muted()))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.choose_compare_mode(mode, cx);
                    }),
                )
        };
        div().mx_3().my_1().flex_none().child(
            div()
                .p(px(2.))
                .flex()
                .flex_row()
                .rounded_md()
                .bg(theme::selected_row())
                .child(segment(RangeMode::Direct, "Direct"))
                .child(segment(
                    RangeMode::SinceCommonAncestor,
                    "Since common ancestor",
                )),
        )
    }

    /// The suggestions of `field` under it: the rows are at most `SUGGESTION_ROWS`, so they
    /// are not virtualized. Nothing is drawn when no name matches.
    fn suggestion_list(&self, field: Field, scope: &'static str, cx: &Context<Self>) -> Div {
        let rows = self.field_suggestions(field);
        if rows.is_empty() {
            return div();
        }
        let highlight = self.model.compare.focus.highlight();
        div()
            .mx_3()
            .mb_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_sm()
            .border_1()
            .border_color(theme::hairline())
            .children(rows.iter().enumerate().map(|(index, row)| {
                suggestion_row(scope, field, index, row, index == highlight, cx)
            }))
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

/// The Compare fields' subscriptions (D231 (3)): their text, Esc, and focus. Focus opens and
/// closes a field's suggestion list; leaving a field reads the revisions again.
pub(crate) fn subscriptions(
    from: &Entity<TextArea>,
    to: &Entity<TextArea>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    let mut subscriptions = Vec::new();
    for (field, area) in [(Field::From, from), (Field::To, to)] {
        let handle = area.read(cx).focus_handle(cx);
        subscriptions.push(cx.observe(area, move |this, area, cx| {
            this.on_compare_field_changed(field, &area, cx);
        }));
        subscriptions.push(cx.subscribe_in(
            area,
            window,
            move |this, _, event: &TextAreaEvent, _window, cx| {
                if *event == TextAreaEvent::Cancel {
                    this.close_compare_list(cx);
                }
            },
        ));
        subscriptions.push(cx.on_focus_in(&handle, window, move |this, _, cx| {
            this.on_compare_field_focus(field, cx);
        }));
        subscriptions.push(cx.on_focus_out(&handle, window, move |this, _, _, cx| {
            this.on_compare_field_blur(field, cx);
        }));
    }
    subscriptions
}

impl StatusView {
    /// The Compare tab was opened: an empty pair starts with **To** at `HEAD` (the docs say so),
    /// and the fields show what the state holds, which a new repository has emptied.
    pub(crate) fn prepare_compare_fields(&mut self, cx: &mut Context<Self>) {
        if self.model.compare.needs_defaults() {
            self.model.compare.set_to("HEAD".to_owned());
        }
        self.sync_compare_fields(cx);
    }

    /// Sets each field's text to the state's, where it differs. The areas then notify the
    /// change, which finds the text already in the state and does nothing.
    pub(crate) fn sync_compare_fields(&self, cx: &mut Context<Self>) {
        let fields = [
            (self.compare_from.clone(), self.model.compare.from.clone()),
            (self.compare_to.clone(), self.model.compare.to.clone()),
        ];
        for (area, text) in fields {
            if area.read(cx).text() != text {
                area.update(cx, |area, cx| area.set_text(&text, cx));
            }
        }
    }

    /// The field's text changed by typing. The state takes it; nothing is read until Enter or a
    /// blur. A line break is not part of a revision, so only the first line counts.
    pub(crate) fn on_compare_field_changed(
        &mut self,
        field: Field,
        area: &Entity<TextArea>,
        cx: &mut Context<Self>,
    ) {
        let text = area.read(cx).text().to_owned();
        if text.contains('\n') {
            let first = text.lines().next().unwrap_or("").to_owned();
            area.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; redraw only on a real change.
        let current = match field {
            Field::From => &self.model.compare.from,
            Field::To => &self.model.compare.to,
        };
        if text == *current {
            return;
        }
        self.model.compare.type_into(field, text);
        cx.notify();
    }

    /// A field gained focus: its suggestions open.
    pub(crate) fn on_compare_field_focus(&mut self, field: Field, cx: &mut Context<Self>) {
        self.model.compare.focus.focus(field);
        cx.notify();
    }

    /// A field lost focus: its list closes and the tab reads what the fields now say.
    pub(crate) fn on_compare_field_blur(&mut self, field: Field, cx: &mut Context<Self>) {
        self.model.compare.focus.blur(field);
        if self.model.compare.tab == ChangesTab::Compare {
            self.read_changes_tab(cx);
        }
        cx.notify();
    }

    /// Esc in a field: its open list closes.
    pub(crate) fn close_compare_list(&mut self, cx: &mut Context<Self>) {
        self.model.compare.focus.close();
        cx.notify();
    }

    /// The suggestions of `field` for the text it holds now.
    fn field_suggestions(&self, field: Field) -> Vec<compare::Suggestion> {
        let text = match field {
            Field::From => &self.model.compare.from,
            Field::To => &self.model.compare.to,
        };
        compare::suggestions(
            text,
            self.model.branches.known(),
            &self.model.compare.tags,
            compare::SUGGESTION_ROWS,
        )
    }

    /// Up or Down in an open list moves its highlight. `false` when no list is open, so the
    /// keys stay with the field.
    pub(crate) fn move_compare_highlight(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(field) = self.model.compare.focus.listed() else {
            return false;
        };
        let count = self.field_suggestions(field).len();
        self.model.compare.focus.move_highlight(down, count);
        cx.notify();
        true
    }

    /// Enter in a field: the highlighted suggestion is taken if a list is open with a row to
    /// take; otherwise the revisions are read as they are.
    pub(crate) fn enter_compare_field(&mut self, cx: &mut Context<Self>) {
        let picked = self.model.compare.focus.listed().and_then(|field| {
            let rows = self.field_suggestions(field);
            compare::taken(&rows, self.model.compare.focus.highlight()).map(|text| (field, text))
        });
        if let Some((field, text)) = picked {
            self.pick_compare_suggestion(field, &text, cx);
        } else {
            self.model.compare.focus.close();
            self.read_changes_tab(cx);
            cx.notify();
        }
    }

    /// A suggestion was clicked or taken with Enter: the field takes it, its list closes and the
    /// tab reads.
    pub(crate) fn pick_compare_suggestion(
        &mut self,
        field: Field,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.model.compare.set_field(field, text.to_owned());
        self.model.compare.focus.close();
        self.sync_compare_fields(cx);
        self.read_changes_tab(cx);
        cx.notify();
    }

    /// **Swap**: the fields exchange their text and the tab reads the change backwards.
    pub(crate) fn swap_compare_fields(&mut self, cx: &mut Context<Self>) {
        self.model.compare.swap();
        self.model.compare.focus.close();
        self.sync_compare_fields(cx);
        self.read_changes_tab(cx);
        cx.notify();
    }

    /// The **Mode** switch: a different mode reads the tab again.
    pub(crate) fn choose_compare_mode(&mut self, mode: RangeMode, cx: &mut Context<Self>) {
        if self.model.compare.mode == mode {
            return;
        }
        self.model.compare.set_mode(mode);
        self.read_changes_tab(cx);
        cx.notify();
    }
}

/// The **Swap** button: exchanges the two fields and reads the change backwards.
fn swap_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("compare-swap")
        .flex_none()
        .px_2()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .cursor_pointer()
        .child("Swap")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.swap_compare_fields(cx)),
        )
}

/// One row of a field's suggestion list: the text, its kind, and the highlight. A click takes it.
fn suggestion_row(
    scope: &'static str,
    field: Field,
    index: usize,
    row: &compare::Suggestion,
    highlighted: bool,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let text = row.text.clone();
    div()
        .id((scope, index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(highlighted, |item| item.bg(theme::selected_row()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(row.text.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child(row.kind.label()),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.pick_compare_suggestion(field, &text, cx);
            }),
        )
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
