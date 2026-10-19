//! The sidebar at the window's left: the workspace destinations, the switch
//! between the two ways of grouping branches, the branch tree and the
//! worktrees. Only the Working Copy exists; the other destinations and the
//! stack tree are placeholders until their slices land (docs/07). The
//! worktrees show their state; a double-click opens one in its own window
//! (docs/07, W4).

use std::ops::Range;

use gm_schema::BranchEntry;
use gpui::{
    AnyElement, Context, Div, ElementId, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent,
    SharedString, Stateful, Subscription, Window, div, prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::branch_filter::{self, Scope};
use crate::icon::{self, Icon};
use crate::name_fit::{self, name_label};
use crate::sidebar::{self, Grouping, Row};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::window_chrome;
use crate::worktree_list;

pub(crate) const SIDEBAR_WIDTH: f32 = 240.;
const ROW_HEIGHT: f32 = 28.;
/// The most worktree rows shown at once. The section does not scroll yet,
/// so the rest are cut off.
const WORKTREE_ROWS_SHOWN: f32 = 6.;

/// The destinations under "Workspace" that do not exist yet, and the slice
/// that brings each one. Working Copy and History are rows of their own.
const PLACEHOLDERS: [(&str, &str); 3] = [
    ("Stashes", "Stashes"),
    ("Local review", "Local review"),
    ("Reflog", "Reflog"),
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
            .children(self.has_branches().then(|| self.branch_filter_bar(cx)))
            .child(self.branch_tree(cx))
            .child(self.worktrees_section(cx))
            .children((self.sidebar_grouping == Grouping::ByStack).then(footer_hint))
    }

    /// The WORKTREES section: the main checkout, then the linked worktrees.
    /// Empty until a listing has arrived.
    fn worktrees_section(&self, cx: &Context<Self>) -> Div {
        let root = self.model.status().map(|status| status.root.clone());
        let rows = worktree_list::rows(&self.model.worktrees, root.as_deref(), theme::LANE_COUNT);
        if rows.is_empty() {
            return div();
        }
        div()
            .flex_none()
            .max_h(px(ROW_HEIGHT * WORKTREE_ROWS_SHOWN + 40.))
            .overflow_hidden()
            .px_2()
            .pb_2()
            .child(section_title("Worktrees"))
            .children(rows.iter().map(|row| worktree_row(row, cx)))
    }

    fn workspace_items(&self, cx: &mut Context<Self>) -> Div {
        let count = self.model.entry_count();
        let working = self.screen == Screen::WorkingCopy;
        div()
            .px_2()
            .flex()
            .flex_col()
            .child(
                item_row("Working Copy", working)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.show_working_copy(cx);
                        }),
                    )
                    .child(
                        div()
                            .px_2()
                            .rounded_full()
                            .bg(theme::selection())
                            .text_color(theme::accent())
                            .text_xs()
                            .child(count.to_string()),
                    ),
            )
            .child(
                item_row("History", self.screen == Screen::History).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.run_action(Action::ShowHistory, &Args::None, window, cx);
                    }),
                ),
            )
            .children(PLACEHOLDERS.iter().map(|&(label, name)| {
                item_row(label, false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.notify_user(&format!("{name} is not available yet."), cx);
                    }),
                )
            }))
    }

    /// The two-way switch between "By stack" and "By name".
    fn grouping_switch(&self, cx: &mut Context<Self>) -> Div {
        let segment_for = |grouping: Grouping| {
            segment(grouping.label(), self.sidebar_grouping == grouping).on_mouse_down(
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
                .child(segment_for(Grouping::ByStack))
                .child(segment_for(Grouping::ByName)),
        )
    }

    /// Whether a listing has arrived with branches in it. The filter is only
    /// shown then.
    fn has_branches(&self) -> bool {
        !self.model.branches.known().is_empty()
    }

    /// The branches the sidebar shows: the filter's scope and query applied to
    /// the last listing. The branch picker and the palette still see them all.
    pub(crate) fn visible_branches(&self) -> Vec<&BranchEntry> {
        branch_filter::visible(
            self.model.branches.known(),
            &self.sidebar_filter,
            self.model.branches.user_email(),
        )
    }

    /// The filter's input, the scope toggle and the count of shown branches.
    fn branch_filter_bar(&self, cx: &mut Context<Self>) -> Div {
        let scope = self.sidebar_filter.scope(self.model.branches.user_email());
        let shown = self.visible_branches().len();
        let total = self.model.branches.known().len();
        let clear = (!self.sidebar_filter.query().is_empty()).then(|| {
            div()
                .id("sidebar-branch-clear")
                .flex_none()
                .cursor_pointer()
                .text_color(theme::muted())
                .child(icon::view(Icon::Close, icon::SMALL, theme::muted()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.sidebar_input.update(cx, TextArea::clear);
                    }),
                )
        });
        let input = div()
            .h(px(26.))
            .px_2()
            .gap_1()
            .flex()
            .flex_row()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme::hairline())
            .child(icon::view(Icon::MagnifyingGlass, icon::SMALL, theme::muted()).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(self.sidebar_input.clone()),
            )
            .children(clear);
        let scope_segment = |option: Scope| {
            let id = SharedString::from(format!("sidebar-scope-{}", option.id()));
            segment_with_id(id, option.label(), option == scope).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.choose_branch_scope(option, cx);
                }),
            )
        };
        let toggle = div()
            .p(px(2.))
            .w(px(120.))
            .flex()
            .flex_row()
            .rounded_md()
            .bg(theme::selected_row())
            .child(scope_segment(Scope::Mine))
            .child(scope_segment(Scope::All));
        let count = div()
            .text_xs()
            .text_color(theme::muted())
            .child(format!("{shown} of {total}"));
        div()
            .px_3()
            .pb_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(input)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(toggle)
                    .child(count),
            )
    }

    /// Sets the sidebar's scope to one the user picked, and keeps it for the next start.
    pub(crate) fn choose_branch_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.sidebar_filter.choose(scope);
        self.remember_branch_scope(cx);
        cx.notify();
    }

    /// Shows every branch and clears the query: the empty state's link and the
    /// "Show All Branches" action both come here.
    fn show_all_branches(&mut self, cx: &mut Context<Self>) {
        self.sidebar_filter.show_all();
        self.sidebar_input.update(cx, TextArea::clear);
        self.remember_branch_scope(cx);
        cx.notify();
    }

    /// Saves the chosen scope in the settings, unless the file already has it.
    fn remember_branch_scope(&mut self, cx: &mut Context<Self>) {
        let chosen = self.sidebar_filter.chosen();
        if self.settings.branch_scope == chosen {
            return;
        }
        self.settings.branch_scope = chosen;
        self.save_settings(cx);
    }

    /// The filter's input changed. Only its first line counts; the list follows it.
    fn on_sidebar_filter_changed(&mut self, input: &Entity<TextArea>, cx: &mut Context<Self>) {
        let text = input.read(cx).text().to_owned();
        if text.contains('\n') {
            // The input is one line; a line break is not part of a query.
            let first = text.lines().next().unwrap_or("").to_owned();
            input.update(cx, |area, cx| area.set_text(&first, cx));
            return;
        }
        // The caret blinks by notifying the area; redraw only on a real change.
        if text == self.sidebar_filter.query() {
            return;
        }
        self.sidebar_filter.set_query(&text);
        cx.notify();
    }

    /// Escape clears the query. The list stays as it is with nothing typed.
    fn on_sidebar_filter_event(&mut self, event: TextAreaEvent, cx: &mut Context<Self>) {
        if event == TextAreaEvent::Cancel {
            self.sidebar_input.update(cx, TextArea::clear);
        }
    }

    fn branch_tree(&self, cx: &mut Context<Self>) -> Div {
        let visible = self.visible_branches();
        if visible.is_empty() && self.has_branches() {
            return div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .child(no_match_state(cx));
        }
        let count = sidebar::rows(&visible, self.sidebar_grouping).len();
        div().flex_1().min_h(px(0.)).flex().flex_col().child(
            uniform_list(
                "sidebar-branches",
                count,
                cx.processor(
                    |this: &mut StatusView,
                     range: Range<usize>,
                     _window: &mut Window,
                     cx: &mut Context<StatusView>| {
                        let visible = this.visible_branches();
                        let rows = sidebar::rows(&visible, this.sidebar_grouping);
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
}

/// The empty state of the branch tree: nothing matches the filter, and a link
/// shows every branch again.
fn no_match_state(cx: &mut Context<StatusView>) -> Div {
    div()
        .px_4()
        .py_3()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_color(theme::muted()).child("No branches match"))
        .child(
            div()
                .id("sidebar-show-all-branches")
                .cursor_pointer()
                .text_color(theme::accent())
                .child("Show all branches")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.show_all_branches(cx);
                    }),
                ),
        )
}

/// The listeners that keep the sidebar's query input and the window in step.
/// Made with the window, before the first event.
pub(crate) fn subscriptions(
    input: &Entity<TextArea>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![
        cx.observe(input, |this, input, cx| {
            this.on_sidebar_filter_changed(&input, cx);
        }),
        cx.subscribe_in(
            input,
            window,
            |this, _, event: &TextAreaEvent, _window, cx| {
                this.on_sidebar_filter_event(*event, cx);
            },
        ),
    ]
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

/// One segment of a two-way switch. `active` is the one shown as chosen.
fn segment(label: &'static str, active: bool) -> Stateful<Div> {
    segment_with_id(label, label, active)
}

fn segment_with_id(id: impl Into<ElementId>, label: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
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
        .child(label)
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
        Row::Group(prefix) => {
            let text = format!("{prefix}/");
            div()
                .id(("sidebar-group", index))
                .px_4()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .text_color(theme::muted())
                .when(name_fit::needs_hover(&text), |this| {
                    this.tooltip(window_chrome::hint(text.clone()))
                })
                .child(name_label(&text, &[]))
                .into_any_element()
        }
        Row::Branch {
            branch,
            label,
            indented,
        } => {
            let current = !branch.is_remote && head == Some(branch.name.as_str());
            let name = branch.name.clone();
            let full = branch.name.clone();
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
                // The tooltip shows the whole name: the label has lost its prefix
                // under a group, and the name may be cut.
                .when(name_fit::needs_hover(&full), |this| {
                    this.tooltip(window_chrome::hint(full))
                })
                .child(name_label(label, &[]))
                .children(current.then(|| {
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .bg(theme::accent())
                        .text_color(theme::on_accent())
                        .text_xs()
                        .child("HEAD")
                }))
                // A left click does nothing: there is no branch view yet, and
                // switching on click is not what a click on a name should mean.
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_branch_menu(&name, event.position, cx);
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

/// One worktree: its colour mark, its name, and the tags that say it is the
/// window's own or that its folder is gone. A double-click opens the others in
/// a window of their own.
fn worktree_row(row: &worktree_list::Row<'_>, cx: &Context<StatusView>) -> Stateful<Div> {
    let action = match worktree_list::open_target(row) {
        worktree_list::Open::Nothing => None,
        worktree_list::Open::Missing => Some(Err("This worktree's folder is missing.")),
        worktree_list::Open::Path(path) => Some(Ok(path.to_owned())),
    };
    let menu_path = row.entry.path.clone();
    let id = SharedString::from(format!("sidebar-worktree-{menu_path}"));
    div()
        .id(id)
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_2()
        .flex()
        .flex_row()
        .items_center()
        .rounded_md()
        .when(row.here, |this| this.bg(theme::selection()))
        .when(name_fit::needs_hover(&row.name), |this| {
            this.tooltip(window_chrome::hint(row.name.clone()))
        })
        .child(lane_mark(theme::lane(row.lane)))
        .child(name_label(&row.name, &[]).when(row.here, |this| {
            this.text_color(theme::accent())
                .font_weight(FontWeight::MEDIUM)
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_worktree_menu(&menu_path, event.position, cx);
            }),
        )
        .children(
            row.here
                .then(|| tag("here", theme::on_accent(), theme::accent())),
        )
        .children(
            row.missing
                .then(|| tag("missing", theme::warn_fg(), theme::warn_bg())),
        )
        .children(
            row.entry
                .locked
                .is_some()
                .then(|| tag("locked", theme::warn_fg(), theme::warn_bg())),
        )
        .when_some(action, |this, action| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count != 2 {
                        return;
                    }
                    match &action {
                        Ok(path) => this.open_worktree(path, cx),
                        Err(reason) => this.notify_user(reason, cx),
                    }
                }),
            )
        })
}

/// A small window glyph in the worktree's colour: an outline with a title bar.
fn lane_mark(color: Hsla) -> Div {
    div()
        .w(px(14.))
        .h(px(12.))
        .flex_none()
        .rounded_sm()
        .border_1()
        .border_color(color)
        .child(div().h(px(3.)).w_full().bg(color))
}

fn tag(text: &'static str, text_color: Hsla, background: Hsla) -> Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .bg(background)
        .text_color(text_color)
        .text_xs()
        .child(text)
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
