//! The sidebar at the window's left: the workspace destinations, the switch
//! between the two ways of grouping branches, and the branch tree. Only the
//! Working Copy exists; the other destinations and the stack tree are
//! placeholders until their slices land (docs/07).

use std::ops::Range;

use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::sidebar::{self, Grouping, Row};
use crate::theme;
use crate::view::StatusView;

pub(crate) const SIDEBAR_WIDTH: f32 = 240.;
const ROW_HEIGHT: f32 = 28.;

/// The destinations under "Workspace", and the slice that brings each one.
const WORKSPACE: [(&str, Option<&str>); 5] = [
    ("Working Copy", None),
    ("History", Some("History")),
    ("Stashes", Some("Stashes")),
    ("Local review", Some("Local review")),
    ("Reflog", Some("Reflog")),
];

impl StatusView {
    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .border_r_1()
            .border_color(theme::hairline())
            .child(section_title("Workspace"))
            .child(self.workspace_items(cx))
            .child(self.grouping_switch(cx))
            .child(self.branch_tree(cx))
            .children((self.sidebar_grouping == Grouping::ByStack).then(footer_hint))
    }

    fn workspace_items(&self, cx: &mut Context<Self>) -> Div {
        let count = self.model.entry_count();
        div()
            .px_2()
            .flex()
            .flex_col()
            .children(WORKSPACE.iter().map(|&(label, placeholder)| {
                let row = item_row(label, placeholder.is_none());
                match placeholder {
                    None => row.child(
                        div()
                            .px_2()
                            .rounded_full()
                            .bg(theme::selection())
                            .text_color(theme::accent())
                            .text_xs()
                            .child(count.to_string()),
                    ),
                    Some(name) => row.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.notify_user(&format!("{name} is not available yet."), cx);
                        }),
                    ),
                }
            }))
    }

    /// The two-way switch between "By stack" and "By name".
    fn grouping_switch(&self, cx: &mut Context<Self>) -> Div {
        let segment = |grouping: Grouping| {
            let active = self.sidebar_grouping == grouping;
            div()
                .id(grouping.label())
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .when(active, |this| {
                    this.bg(theme::panel())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                })
                .when(!active, |this| this.text_color(theme::muted()))
                .child(grouping.label())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.sidebar_grouping = grouping;
                        cx.notify();
                    }),
                )
        };
        div().px_3().py_3().child(
            div()
                .p(px(2.))
                .flex()
                .flex_row()
                .rounded_md()
                .bg(theme::selected_row())
                .child(segment(Grouping::ByStack))
                .child(segment(Grouping::ByName)),
        )
    }

    fn branch_tree(&self, cx: &mut Context<Self>) -> Div {
        let count = sidebar::rows(self.model.branches.known(), self.sidebar_grouping).len();
        div().flex_1().min_h(px(0.)).flex().flex_col().child(
            uniform_list(
                "sidebar-branches",
                count,
                cx.processor(
                    |this: &mut StatusView,
                     range: Range<usize>,
                     _window: &mut Window,
                     cx: &mut Context<StatusView>| {
                        let rows =
                            sidebar::rows(this.model.branches.known(), this.sidebar_grouping);
                        let head = this
                            .model
                            .status()
                            .and_then(|status| status.branch.head.clone());
                        rows.get(range.clone())
                            .unwrap_or_default()
                            .iter()
                            .zip(range)
                            .map(|(row, index)| tree_row(row, index, head.as_deref(), cx))
                            .collect()
                    },
                ),
            )
            .flex_1()
            .min_h(px(0.)),
        )
    }

    /// Checks out `name`, as the top bar's picker does.
    fn checkout_from_sidebar(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.run_action(
            Action::CheckoutBranch,
            &Args::Branch(name.to_owned()),
            window,
            cx,
        );
    }
}

fn section_title(text: &'static str) -> Div {
    div()
        .px_4()
        .pt_4()
        .pb_1()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_uppercase())
}

/// A workspace row; `selected` is the screen being shown.
fn item_row(label: &'static str, selected: bool) -> Stateful<Div> {
    div()
        .id(label)
        .h(px(ROW_HEIGHT))
        .px_2()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .rounded_md()
        .when(selected, |this| {
            this.bg(theme::panel()).font_weight(FontWeight::MEDIUM)
        })
        .when(!selected, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
        })
        .child(label)
}

/// A row of the branch tree. `head` is the branch HEAD is on.
fn tree_row(
    row: &Row<'_>,
    index: usize,
    head: Option<&str>,
    cx: &mut Context<StatusView>,
) -> AnyElement {
    match row {
        // The same height as every row, which a `uniform_list` needs; the title
        // sits at the bottom of it.
        Row::Header(title) => section_title(title)
            .h(px(ROW_HEIGHT))
            .flex()
            .items_end()
            .pt_0()
            .into_any_element(),
        Row::Note(text) => div()
            .px_4()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .text_color(theme::faint())
            .child(*text)
            .into_any_element(),
        Row::Group(prefix) => div()
            .px_4()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .text_color(theme::muted())
            .child(format!("{prefix}/"))
            .into_any_element(),
        Row::Branch {
            branch,
            label,
            indented,
        } => {
            let current = !branch.is_remote && head == Some(branch.name.as_str());
            let name = branch.name.clone();
            let inner = div()
                .id(("sidebar-branch", index))
                .size_full()
                .pl(px(if *indented { 24. } else { 8. }))
                .pr_2()
                .gap_2()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .rounded_md()
                .cursor_pointer()
                .when(current, |this| {
                    this.bg(theme::selected_row())
                        .text_color(theme::accent())
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!current, |this| {
                    this.hover(|style| style.bg(theme::selected_row()))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child((*label).to_owned()),
                )
                .children(current.then(|| {
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(theme::accent())
                        .text_color(theme::on_accent())
                        .text_xs()
                        .child("HEAD")
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.checkout_from_sidebar(&name, window, cx);
                    }),
                );
            // A `uniform_list` item takes no margin, so the inset is padding here.
            div()
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_2()
                .child(inner)
                .into_any_element()
        }
    }
}

/// The hint under the stack tree, for the gesture slice 4 adds.
fn footer_hint() -> Div {
    div()
        .px_4()
        .py_3()
        .border_t_1()
        .border_color(theme::hairline())
        .text_color(theme::muted())
        .child("Drag a branch onto another to set its parent.")
}
