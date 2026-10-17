//! The branch button in the top bar and the floating list it opens: a filter,
//! then the local and remote branches. Enter or a click dispatches `Checkout`;
//! the view changes only when the engine reports the new status. A refused
//! checkout keeps the branch, and git's first line goes on the status line.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gm_schema::{Actor, BranchEntry, CommandKind};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Pixels, Stateful,
    UniformListScrollHandle, Window, canvas, div, prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::branches::{Line, Section};
use crate::icon::{self, Icon};
use crate::text_area::{Down, Newline, TextArea, TextAreaEvent, Up};
use crate::theme;
use crate::view::StatusView;

const ROW_HEIGHT: f32 = 24.;
const MENU_WIDTH: f32 = 360.;
/// Twelve rows are shown; a longer list scrolls.
const MAX_LIST_HEIGHT: f32 = ROW_HEIGHT * 12.;

pub(crate) struct BranchMenu {
    pub(crate) open: bool,
    pub(crate) filter: Entity<TextArea>,
    pub(crate) scroll: UniformListScrollHandle,
    /// The button's left and bottom edges in window coordinates, as it was last
    /// painted. The list hangs below it.
    anchor: Rc<Cell<(Pixels, Pixels)>>,
}

impl BranchMenu {
    pub(crate) fn new(filter: Entity<TextArea>) -> Self {
        Self {
            open: false,
            filter,
            scroll: UniformListScrollHandle::new(),
            anchor: Rc::new(Cell::new((px(0.), px(0.)))),
        }
    }
}

impl StatusView {
    /// The button at the top bar's left after the project button: the branch
    /// icon and name, or "(detached)" when HEAD has no branch.
    pub(crate) fn branch_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        let name = self.model.status().map_or_else(String::new, |status| {
            status
                .branch
                .head
                .clone()
                .unwrap_or_else(|| "(detached)".to_owned())
        });
        let anchor = Rc::clone(&self.branch_menu.anchor);
        // The canvas only measures the button; it paints nothing.
        let measure = canvas(
            move |bounds, _, _| {
                anchor.set((bounds.origin.x, bounds.origin.y + bounds.size.height));
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("branch-button")
            .relative()
            .h(px(26.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(measure)
            .child(icon::view(Icon::GitBranch, icon::SIZE, theme::text()))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
            .child(icon::view(Icon::CaretDown, icon::SMALL, theme::muted()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.toggle_branch_menu(window, cx);
                }),
            )
    }

    pub(crate) fn toggle_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.branch_menu.open {
            self.close_branch_menu(window, cx);
            return;
        }
        self.project_menu.open = false;
        self.branch_menu.open = true;
        self.branch_menu
            .filter
            .update(cx, |area, cx| area.set_text("", cx));
        self.branch_menu
            .filter
            .update(cx, |area, cx| area.focus(window, cx));
        // Always a fresh listing: the picker never shows an old one.
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListBranches);
        self.model.branches.request(id);
        cx.notify();
    }

    /// Closes the list and gives the keys back to the change list.
    pub(crate) fn close_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_menu.open = false;
        self.list_focus.focus(window);
        cx.notify();
    }

    /// Closes the list, then checks out `name` through the registry.
    fn choose_branch(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_branch_menu(window, cx);
        self.run_action(
            Action::CheckoutBranch,
            &Args::Branch(name.to_owned()),
            window,
            cx,
        );
    }

    /// Enter: checks out the highlighted branch. Nothing happens with no match.
    fn enter_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.model.branches.highlighted() else {
            return;
        };
        let name = branch.name.clone();
        self.choose_branch(&name, window, cx);
    }

    /// Up and down: moves the highlight and keeps it on screen.
    fn move_branch_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        self.model.branches.move_highlight(down);
        let lines = self.model.branches.lines();
        if let Some(row) = lines.iter().position(|line| {
            matches!(
                line,
                Line::Branch {
                    highlighted: true,
                    ..
                }
            )
        }) {
            self.branch_menu
                .scroll
                .scroll_to_item(row, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// The filter changed. Only the first line counts, and the list starts over.
    pub(crate) fn on_branch_filter_changed(
        &mut self,
        filter: &Entity<TextArea>,
        cx: &mut Context<Self>,
    ) {
        let text = filter.read(cx).text().to_owned();
        if text.contains('\n') {
            // The filter is one line; Enter is handled as a choice, not a line break.
            let first = text.lines().next().unwrap_or("").to_owned();
            filter.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; redraw only on a real change.
        if text == self.model.branches.filter() {
            return;
        }
        self.model.branches.set_filter(&text);
        self.branch_menu
            .scroll
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    /// Escape clears a typed filter first; with nothing typed it closes the list.
    pub(crate) fn on_branch_filter_event(
        &mut self,
        event: TextAreaEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event != TextAreaEvent::Cancel {
            return;
        }
        // The area's own text, not the model's copy: the model follows it one notification later.
        if self.branch_menu.filter.read(cx).text().is_empty() {
            self.close_branch_menu(window, cx);
        } else {
            self.branch_menu.filter.update(cx, TextArea::clear);
        }
    }

    /// The list and the layer under it that closes it on a click outside.
    /// Drawn last, so it sits above the whole window.
    pub(crate) fn branch_menu_layer(&self, cx: &Context<Self>) -> Div {
        let (left, bottom) = self.branch_menu.anchor.get();
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_branch_menu(window, cx);
            }),
        );
        div()
            .absolute()
            .inset_0()
            .child(outside)
            .child(self.branch_list(left, bottom + px(4.), cx))
    }

    fn branch_list(&self, left: Pixels, top: Pixels, cx: &Context<Self>) -> Div {
        let muted = theme::muted();
        let lines = self.model.branches.lines();
        let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let body = if self.model.branches.is_failed() {
            div()
                .px_3()
                .py_2()
                .text_color(muted)
                .child("Could not list the branches.")
                .into_any_element()
        } else if lines.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_color(muted)
                .child("Loading…")
                .into_any_element()
        } else {
            let height = (ROW_HEIGHT * f32::from(rows)).min(MAX_LIST_HEIGHT);
            uniform_list(
                "branch-rows",
                lines.len(),
                cx.processor(
                    |this: &mut StatusView,
                     range: Range<usize>,
                     _window: &mut Window,
                     cx: &mut Context<StatusView>| {
                        let lines = this.model.branches.lines();
                        lines
                            .get(range.clone())
                            .unwrap_or_default()
                            .iter()
                            .zip(range)
                            .map(|(line, index)| line_element(line, index, cx))
                            .collect()
                    },
                ),
            )
            .track_scroll(self.branch_menu.scroll.clone())
            .h(px(height))
            .into_any_element()
        };
        // Keys typed while the filter has focus reach the list through capture,
        // so the area never sees the ones the list uses.
        let view = cx.entity().downgrade();
        let up = view.clone();
        let down = view.clone();
        let enter = view;
        let filter_row = div()
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
                    .child(self.branch_menu.filter.clone()),
            );
        div()
            .absolute()
            .left(left)
            .top(top)
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .py_1()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::hairline_strong())
            .rounded_md()
            .shadow_lg()
            .occlude()
            .capture_action(move |_: &Up, _, cx| {
                let _ = up.update(cx, |this, cx| this.move_branch_highlight(false, cx));
                cx.stop_propagation();
            })
            .capture_action(move |_: &Down, _, cx| {
                let _ = down.update(cx, |this, cx| this.move_branch_highlight(true, cx));
                cx.stop_propagation();
            })
            .capture_action(move |_: &Newline, window, cx| {
                let _ = enter.update(cx, |this, cx| this.enter_branch(window, cx));
                cx.stop_propagation();
            })
            .child(filter_row)
            .child(body)
    }
}

/// One row of the list. Headers and "None" rows are inert; a branch row
/// checks out on click.
fn line_element(line: &Line<'_>, index: usize, cx: &Context<StatusView>) -> AnyElement {
    let muted = theme::muted();
    match line {
        Line::Header(section) => header_row(*section).into_any_element(),
        Line::Empty(_) => div()
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .pl(px(34.))
            .text_color(muted)
            .child("None")
            .into_any_element(),
        Line::Branch {
            branch,
            highlighted,
        } => branch_row(branch, *highlighted, index, cx).into_any_element(),
    }
}

fn header_row(section: Section) -> Div {
    div()
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .items_center()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(section.title())
}

/// The name as git lists it, the check on the current branch, and the upstream
/// in muted text when there is one. The highlighted row has the selection tint.
fn branch_row(
    branch: &BranchEntry,
    highlighted: bool,
    index: usize,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let muted = theme::muted();
    let name = branch.name.clone();
    let current = branch.is_current;
    div()
        .id(("branch", index))
        .h(px(ROW_HEIGHT))
        .px_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(highlighted, |row| row.bg(theme::selected_row()))
        .hover(|style| style.bg(theme::selected_row()))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.choose_branch(&name, window, cx);
            }),
        )
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .children(current.then(|| icon::view(Icon::CheckCircle, icon::SMALL, muted))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .child(branch.name.clone()),
        )
        .children(
            branch
                .upstream
                .clone()
                .map(|upstream| div().text_xs().text_color(muted).child(upstream)),
        )
}
