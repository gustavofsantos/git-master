//! The sidebar at the window's left: the repository destinations, the switch
//! between the two ways of grouping branches, the branch tree and the
//! worktrees. Each of the three sections folds under its header. Every destination is a row of its own; the stack tree is a
//! placeholder until its slice lands (design/roadmap.md). The worktrees show their state;
//! a double-click opens one in its own window (design/roadmap.md, W4).

use std::ops::Range;

use gm_schema::{Actor, BranchEntry, CommandKind, Feature, HistoryScope, StackBranch, StackState};
use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, Entity, FontWeight, Hsla, MouseButton,
    MouseDownEvent, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
    uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::branch_drag::{self, BranchZone};
use crate::branch_filter;
use crate::icon::{self, Icon};
use crate::local_review;
use crate::move_file::{self, DropTarget, DropZone};
use crate::name_fit::{self, name_label};
use crate::resize::{self, SIDEBAR};
use crate::resize_handle::{self, Edge};
use crate::sidebar::{self, Grouping, Row, Section};
use crate::stacks;
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::window_chrome;
use crate::workspace_list::{self, Open as WorkspaceOpen};
use crate::worktree_list;

const ROW_HEIGHT: f32 = 28.;
/// The height of a section header's row, its control included: the tallest control is 26 px.
const HEADER_ROW_HEIGHT: f32 = 26.;
/// The space above and below a section header's row (its `pt_1` and `pb_1`).
const HEADER_PADDING: f32 = 8.;
/// A section header's whole height: every header is this tall, so the folded sections are evenly spaced.
const HEADER_HEIGHT: f32 = HEADER_PADDING + HEADER_ROW_HEIGHT;
/// The branch list's floor: two rows. The column scrolls below this rather than cut the list to nothing.
const BRANCH_LIST_MIN_H: f32 = ROW_HEIGHT * 2.;
/// The most worktree rows shown at once. The section does not scroll yet,
/// so the rest are cut off.
const WORKTREE_ROWS_SHOWN: f32 = 6.;
/// The most workspace rows shown at once, as for the worktrees.
const WORKSPACE_ROWS_SHOWN: f32 = 6.;

impl StatusView {
    /// The sidebar at `width`. Its right edge is the strip that resizes it.
    ///
    /// The column scrolls as a whole when the sections are taller than the window. The branch
    /// list takes the height the other sections leave, never less than two rows, so it cannot
    /// be cut to nothing; every other child keeps its own height (`flex_none`).
    pub(crate) fn sidebar(&self, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let width_strip = resize_handle::handle(
            "sidebar-resize",
            Edge::Right,
            width,
            SIDEBAR,
            |this: &mut StatusView, width, _| this.sidebar_width = width,
            |this: &mut StatusView, _| this.sidebar_width = resize::SIDEBAR_DEFAULT,
            cx,
        );
        let branches_open = !self.sidebar_folded.is_folded(Section::Branches);
        div()
            .id("sidebar-column")
            .relative()
            .w(px(width))
            .flex_none()
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(theme::ground())
            .border_r_1()
            .border_color(theme::hairline())
            .child(self.section_header(Section::Repository, "Repository", None, cx))
            .children(
                (!self.sidebar_folded.is_folded(Section::Repository))
                    .then(|| self.repository_items(cx)),
            )
            .child(self.section_header(
                Section::Branches,
                "Branches",
                Some(new_branch_button(cx)),
                cx,
            ))
            .children(branches_open.then(|| self.branch_controls(cx)))
            .children(branches_open.then(|| self.branch_tree(cx)))
            .child(self.worktrees_section(cx))
            .child(self.workspaces_section(cx))
            .children(
                sidebar::drag_hint_shown(self.grouping(), self.sidebar_folded).then(footer_hint),
            )
            .child(width_strip)
    }

    /// The WORKTREES section: its header with the New Worktree control, then the
    /// main checkout and the linked worktrees. The section shows whenever a
    /// repository is open, so the control is there even with no linked worktree;
    /// the rows wait for a listing.
    fn worktrees_section(&self, cx: &Context<Self>) -> Div {
        let Some(status) = self.model.status() else {
            return div();
        };
        let rows = worktree_list::rows(
            &self.model.worktrees,
            Some(status.root.as_str()),
            theme::LANE_COUNT,
        );
        let header = self.section_header(
            Section::Worktrees,
            "Worktrees",
            Some(new_worktree_button(cx)),
            cx,
        );
        let header =
            move_file::accept_drops(header, Some(self.drop_zone(DropTarget::NewWorktree)), cx);
        let open = !self.sidebar_folded.is_folded(Section::Worktrees);
        div()
            .flex_none()
            .max_h(px(HEADER_HEIGHT + ROW_HEIGHT * WORKTREE_ROWS_SHOWN + 8.))
            .overflow_hidden()
            .child(header)
            .children(open.then(|| {
                div().px_2().pb_2().children(rows.iter().map(|row| {
                    let zone = self.drop_zone(DropTarget::Worktree {
                        path: row.entry.path.clone(),
                    });
                    worktree_row(row, zone, cx)
                }))
            }))
    }

    /// The WORKSPACES section (D183): its header with the New Workspace control, then the
    /// workspaces under the root. It shows in every window, since the list needs no repository.
    fn workspaces_section(&self, cx: &Context<Self>) -> Div {
        let here = self.model.status().map(|status| status.root.as_str());
        let rows = workspace_list::rows(self.model.workspaces.workspaces(), here);
        let header = self.section_header(
            Section::Workspaces,
            "Workspaces",
            Some(new_workspace_button(cx)),
            cx,
        );
        let open = !self.sidebar_folded.is_folded(Section::Workspaces);
        div()
            .flex_none()
            .max_h(px(HEADER_HEIGHT + ROW_HEIGHT * WORKSPACE_ROWS_SHOWN + 8.))
            .overflow_hidden()
            .child(header)
            .children(open.then(|| {
                div()
                    .px_2()
                    .pb_2()
                    .children(rows.iter().map(|row| workspace_row(row, cx)))
            }))
    }

    /// Asks the engine for the workspaces under the root (D183): when the window starts, when the
    /// section opens, and after a workspace is created or removed.
    pub(crate) fn request_workspaces(&mut self) {
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::ListWorkspaces { root: None });
        self.model.workspaces.request(id);
    }

    /// The header of a foldable section: a chevron, the title and an optional
    /// control at the end. A click on the header folds or opens the section;
    /// a click on the control does not reach it.
    fn section_header(
        &self,
        section: Section,
        title: &'static str,
        control: Option<Stateful<Div>>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let folded = self.sidebar_folded.is_folded(section);
        div()
            .id(title)
            .px_4()
            .pt_1()
            .pb_1()
            // One height for every header, whatever its control is, so the gaps between
            // the folded sections are the same (the Branches "+" is taller than the others).
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap_1()
            .flex()
            .flex_row()
            .items_center()
            .text_xs()
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|style| style.text_color(theme::text()))
            .child(div().flex_none().w(px(10.)).child(icon::view(
                if folded {
                    Icon::CaretRight
                } else {
                    Icon::CaretDown
                },
                icon::SMALL,
                theme::muted(),
            )))
            .child(div().flex_1().child(title.to_uppercase()))
            .children(control)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.sidebar_folded.toggle(section);
                    // The list is read again when its section opens, so it is not stale.
                    if section == Section::Workspaces && !this.sidebar_folded.is_folded(section) {
                        this.request_workspaces();
                    }
                    cx.notify();
                }),
            )
    }

    /// What the Branches section holds above the tree while it is open: the grouping switch and
    /// the search. Each keeps its height; the tree below takes the rest of the column.
    fn branch_controls(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .children(
                self.features
                    .is_enabled(Feature::Stacks)
                    .then(|| self.grouping_switch(cx)),
            )
            .children(self.has_branches().then(|| self.branch_filter_bar(cx)))
    }

    /// The Threads line of the Repository section, with its count once it is known (T5).
    fn threads_item(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        item_row("Threads", self.screen == Screen::Threads)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.run_action(Action::ShowThreads, &Args::None, window, cx);
                }),
            )
            .children(
                self.model
                    .threads
                    .total()
                    .map(|count| count_pill(count.to_string())),
            )
    }

    fn repository_items(&self, cx: &mut Context<Self>) -> Div {
        let count = self.model.entry_count();
        let working = self.screen == Screen::WorkingCopy;
        div()
            .flex_none()
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
                            .flex_none()
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
            .child(item_row("Code", self.screen == Screen::Code).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.run_action(Action::ShowCode, &Args::None, window, cx);
                }),
            ))
            .child(
                item_row("Search", self.screen == Screen::Search).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.run_action(Action::SearchText, &Args::None, window, cx);
                    }),
                ),
            )
            .child(
                item_row("Workflows", self.screen == Screen::Workflows).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.run_action(Action::ShowWorkflows, &Args::None, window, cx);
                    }),
                ),
            )
            .child(
                item_row("Stashes", self.screen == Screen::Stashes)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.run_action(Action::ShowStashes, &Args::None, window, cx);
                        }),
                    )
                    .children(
                        self.model
                            .stashes
                            .count()
                            .map(|count| count_pill(count.to_string())),
                    ),
            )
            .child(
                item_row("Reflog", self.screen == Screen::Reflog).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.run_action(Action::ShowReflog, &Args::None, window, cx);
                    }),
                ),
            )
            .child(self.threads_item(cx))
            .child(
                item_row("Labels", self.screen == Screen::Labels).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.run_action(Action::ShowLabels, &Args::None, window, cx);
                    }),
                ),
            )
            .child(
                item_row("Local review", self.screen == Screen::LocalReview)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.run_action(Action::ShowLocalReview, &Args::None, window, cx);
                        }),
                    )
                    .children(
                        local_review::open_pill(self.model.review.comments()).map(count_pill),
                    ),
            )
    }

    /// The grouping the tree shows now: the saved one, or By name while Stacks is off (D169).
    fn grouping(&self) -> Grouping {
        sidebar::effective_grouping(self.sidebar_grouping, self.features)
    }

    /// The two-way switch between "By stack" and "By name".
    fn grouping_switch(&self, cx: &mut Context<Self>) -> Div {
        let segment_for = |grouping: Grouping| {
            segment(grouping.label(), self.grouping() == grouping).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.sidebar_grouping = grouping;
                    cx.notify();
                }),
            )
        };
        div()
            .px_3()
            .py_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
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

    /// The branches the sidebar shows: the filter's query applied to the last
    /// listing. The branch picker and the palette still see them all.
    pub(crate) fn visible_branches(&self) -> Vec<&BranchEntry> {
        branch_filter::visible(self.model.branches.known(), &self.sidebar_filter)
    }

    /// The filter's input, the scope toggle and the count of shown branches.
    fn branch_filter_bar(&self, cx: &mut Context<Self>) -> Div {
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
            .child(count)
    }

    /// Clears the query so every branch shows again: the empty state's link
    /// comes here.
    fn show_all_branches(&mut self, cx: &mut Context<Self>) {
        self.sidebar_input.update(cx, TextArea::clear);
        cx.notify();
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

    /// The branch list. It is `flex_1` in the column with a floor of two rows, so it gets what
    /// the other sections leave and the column scrolls when that is less than the floor.
    fn branch_tree(&self, cx: &mut Context<Self>) -> Div {
        let visible = self.visible_branches();
        if visible.is_empty() && self.has_branches() {
            return div()
                .flex_1()
                .min_h(px(BRANCH_LIST_MIN_H))
                .flex()
                .flex_col()
                .child(no_match_state(cx));
        }
        let count = sidebar::rows(&visible, self.grouping(), &self.model.stacks).len();
        div()
            .flex_1()
            .min_h(px(BRANCH_LIST_MIN_H))
            .flex()
            .flex_col()
            .child(
                uniform_list(
                    "sidebar-branches",
                    count,
                    cx.processor(
                        |this: &mut StatusView,
                         range: Range<usize>,
                         _window: &mut Window,
                         cx: &mut Context<StatusView>| {
                            let visible = this.visible_branches();
                            let rows = sidebar::rows(&visible, this.grouping(), &this.model.stacks);
                            let head = this
                                .model
                                .status()
                                .and_then(|status| status.branch.head.clone());
                            let by_stack = this.grouping() == Grouping::ByStack;
                            rows.get(range.clone())
                                .unwrap_or_default()
                                .iter()
                                .zip(range)
                                .map(|(row, index)| {
                                    let zone = drop_target_of(&rows, index)
                                        .map(|target| this.drop_zone(target));
                                    // In By stack, a local branch row is dragged and takes a branch.
                                    let branch_zone = match row {
                                        Row::Branch { branch, .. }
                                            if by_stack && !branch.is_remote =>
                                        {
                                            Some(this.branch_zone(&branch.name))
                                        }
                                        _ => None,
                                    };
                                    tree_row(row, index, head.as_deref(), zone, branch_zone, cx)
                                })
                                .collect()
                        },
                    ),
                )
                .flex_1()
                .min_h(px(0.)),
            )
    }
}

/// A count at the end of a row: an accent pill.
fn count_pill(count: String) -> Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .bg(theme::selection())
        .text_color(theme::accent())
        .text_xs()
        .child(count)
}

/// The empty state of the branch tree: nothing matches the search, and a link
/// clears it.
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
                .child("Clear the search")
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

/// The "+" of the Branches header: starts a new branch.
fn new_branch_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("sidebar-new-branch")
        .w(px(26.))
        .h(px(26.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .text_color(theme::muted())
        .hover(|style| style.bg(theme::selected_row()))
        .child("+")
        .tooltip(window_chrome::hint(Action::NewBranch.spec().title))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.run_action(Action::NewBranch, &Args::None, window, cx);
            }),
        )
}

/// The "+" of the Worktrees header: a plain glyph that brightens on hover.
fn new_worktree_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("worktrees-new")
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .text_sm()
        .text_color(theme::muted())
        .hover(|style| style.bg(theme::selected_row()).text_color(theme::text()))
        .child("+")
        .tooltip(window_chrome::hint("New Worktree…"))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.run_action(Action::NewWorktree, &Args::None, window, cx);
            }),
        )
}

/// The "+" of the Workspaces header, which opens New Workspace (D183).
fn new_workspace_button(cx: &Context<StatusView>) -> Stateful<Div> {
    div()
        .id("workspaces-new")
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .text_sm()
        .text_color(theme::muted())
        .hover(|style| style.bg(theme::selected_row()).text_color(theme::text()))
        .child("+")
        .tooltip(window_chrome::hint(Action::NewWorkspace.spec().title))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.run_action(Action::NewWorkspace, &Args::None, window, cx);
            }),
        )
}

/// The "×" that shows on hover of a linked worktree row and removes it through
/// the same dialog as the menu. Its click does not reach the row, so a double
/// click on it does not open the worktree.
fn remove_button(path: &str, group: SharedString, cx: &Context<StatusView>) -> Stateful<Div> {
    let target = path.to_owned();
    div()
        .id(SharedString::from(format!("worktree-remove-{path}")))
        .w(px(16.))
        .h(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .invisible()
        .group_hover(group, gpui::Styled::visible)
        .hover(|style| style.bg(theme::selected_row()))
        .child(icon::view(Icon::Close, icon::SMALL, theme::muted()))
        .tooltip(window_chrome::hint("Remove Worktree"))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let args = Args::Worktree(target.clone());
                this.run_action(Action::RemoveWorktree, &args, window, cx);
            }),
        )
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

/// A Repository section row; `selected` is the screen being shown.
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
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
}

/// The drop target a sidebar row stands for, when it takes files: a local branch,
/// a prefix group of local branches, or the Local header. Remote branches, their
/// groups and the other headers take nothing.
fn drop_target_of(rows: &[Row<'_>], index: usize) -> Option<DropTarget> {
    let remote = rows[..index]
        .iter()
        .rev()
        .find_map(|row| match row {
            Row::Header(title) => Some(*title == "Remote"),
            _ => None,
        })
        .unwrap_or(false);
    if remote {
        return None;
    }
    match &rows[index] {
        Row::Header("Local") => Some(DropTarget::NewBranchGroup { prefix: None }),
        Row::Group(prefix) => Some(DropTarget::NewBranchGroup {
            prefix: Some((*prefix).to_owned()),
        }),
        Row::Branch { branch, .. } if !branch.is_remote => Some(DropTarget::Branch {
            name: branch.name.clone(),
        }),
        _ => None,
    }
}

/// A row of the branch tree. `head` is the branch HEAD is on. `zone` is the row's
/// target for a dragged file; `branch_zone` is set on a local branch row of the
/// By stack view, which is dragged and takes a dragged branch.
fn tree_row(
    row: &Row<'_>,
    index: usize,
    head: Option<&str>,
    zone: Option<DropZone>,
    branch_zone: Option<BranchZone>,
    cx: &mut Context<StatusView>,
) -> AnyElement {
    match row {
        // The same height as every row, which a `uniform_list` needs; the title
        // sits at the bottom of it.
        Row::Header(title) => {
            let header = section_title(title)
                .h(px(ROW_HEIGHT))
                .flex()
                .items_end()
                .pt_0();
            move_file::accept_drops(header, zone, cx).into_any_element()
        }
        Row::Note(text) => div()
            .px_4()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme::faint())
            .child(*text)
            .into_any_element(),
        Row::Group(prefix) => {
            let text = format!("{prefix}/");
            let group = div()
                .id(("sidebar-group", index))
                .px_4()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .text_color(theme::muted())
                .when(name_fit::needs_hover(&text), |this| {
                    this.tooltip(window_chrome::hint(text.clone()))
                })
                .child(name_label(&text, &[]));
            move_file::accept_drops(group, zone, cx).into_any_element()
        }
        Row::Branch {
            branch,
            label,
            indent,
            stack,
            looped,
        } => {
            let current = !branch.is_remote && head == Some(branch.name.as_str());
            let name = branch.name.clone();
            let full = branch.name.clone();
            let indent = f32::from(u16::try_from(*indent).unwrap_or(u16::MAX).min(12));
            let inner = div()
                .id(("sidebar-branch", index))
                .size_full()
                .pl(px(8. + 16. * indent))
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
                .children(stack_marks(*stack, *looped, index))
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
                .on_mouse_down(MouseButton::Left, show_history_of(&name, cx))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_branch_menu(&name, event.position, cx);
                    }),
                );
            let inner = move_file::accept_drops(inner, zone, cx);
            let inner = branch_drag::branch_zone(inner, branch_zone, cx);
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

/// The left click on a branch row: on the History page it shows the branch's
/// history (One ref, D34); it never checks the branch out.
fn show_history_of(
    name: &str,
    cx: &Context<StatusView>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
    let name = name.to_owned();
    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
        if this.screen == Screen::History {
            let scope = HistoryScope::Ref { name: name.clone() };
            this.choose_history_scope(scope, cx);
        }
    })
}

/// The stack indicators after a branch name (D138): the amber dot of a stale
/// branch, its state tag, `↑N` against the parent and `restack N` on a parent
/// that moved. Each says what it means on hover; the shape and the text carry
/// the state, never the colour alone.
fn stack_marks(stack: Option<&StackBranch>, looped: bool, index: usize) -> Vec<AnyElement> {
    let mut marks = Vec::new();
    if looped {
        marks.push(
            hinted_tag(
                ("stack-loop", index),
                "parent loop",
                "The recorded parents of this branch lead back to it. Set Parent… or Remove From Stack to end the loop.",
                theme::warn_fg(),
                theme::warn_bg(),
            )
            .into_any_element(),
        );
    }
    let Some(entry) = stack else {
        return marks;
    };
    if entry.state == StackState::Stale {
        marks.push(
            div()
                .id(("stack-stale", index))
                .flex_none()
                .w(px(8.))
                .h(px(8.))
                .rounded_full()
                .bg(theme::warn_fg())
                .tooltip(window_chrome::hint(stacks::stale_hint(entry)))
                .into_any_element(),
        );
    }
    if let Some((text, hint)) = stacks::state_tag(entry) {
        marks.push(
            hinted_tag(
                ("stack-state", index),
                text,
                hint,
                theme::warn_fg(),
                theme::warn_bg(),
            )
            .into_any_element(),
        );
    }
    if let Some((text, hint)) = stacks::ahead_label(entry) {
        marks.push(
            div()
                .id(("stack-ahead", index))
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .tooltip(window_chrome::hint(hint))
                .child(text)
                .into_any_element(),
        );
    }
    if let Some((text, hint)) = stacks::restack_label(entry) {
        marks.push(
            hinted_tag(
                ("stack-restack", index),
                text,
                hint,
                theme::accent(),
                theme::selection(),
            )
            .into_any_element(),
        );
    }
    marks
}

/// A small tag that explains itself on hover.
fn hinted_tag(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    text_color: Hsla,
    background: Hsla,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px_1()
        .rounded_sm()
        .bg(background)
        .text_color(text_color)
        .text_xs()
        .whitespace_nowrap()
        .tooltip(window_chrome::hint(hint.into()))
        .child(text.into())
}

/// One worktree: its colour mark, its name, and the tags that say it is the
/// window's own or that its folder is gone. A double-click opens the others in
/// a window of their own.
fn worktree_row(
    row: &worktree_list::Row<'_>,
    zone: DropZone,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let action = match worktree_list::open_target(row) {
        worktree_list::Open::Nothing => None,
        worktree_list::Open::Missing => Some(Err("This worktree's folder is missing.")),
        worktree_list::Open::Path(path) => Some(Ok(path.to_owned())),
    };
    let menu_path = row.entry.path.clone();
    let id = SharedString::from(format!("sidebar-worktree-{menu_path}"));
    // The main checkout and the window's own worktree cannot be removed here.
    let group = SharedString::from(format!("worktree-row-{}", row.entry.path));
    let remove = (!row.entry.is_main && !row.here)
        .then(|| remove_button(&row.entry.path, group.clone(), cx));
    let row_div = div()
        .id(id)
        .group(group)
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
        .children(remove);
    move_file::accept_drops(row_div, Some(zone), cx)
}

/// One workspace: its name, the tags that say it is the window's own or that it is ephemeral.
/// A click opens the others in a window of their own; the right-click menu has the rest.
fn workspace_row(row: &workspace_list::Row<'_>, cx: &Context<StatusView>) -> Stateful<Div> {
    let name = row.workspace.name.clone();
    let menu_path = row.workspace.path.clone();
    let open = match workspace_list::open_target(row) {
        WorkspaceOpen::Nothing => None,
        WorkspaceOpen::Path(path) => Some(path.to_owned()),
    };
    div()
        .id(SharedString::from(format!(
            "sidebar-workspace-{}",
            row.workspace.path
        )))
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_2()
        .flex()
        .flex_row()
        .items_center()
        .rounded_md()
        .when(row.here, |this| this.bg(theme::selection()))
        .when(open.is_some(), gpui::Styled::cursor_pointer)
        .when(name_fit::needs_hover(&name), |this| {
            this.tooltip(window_chrome::hint(name.clone()))
        })
        .child(name_label(&name, &[]).when(row.here, |this| {
            this.text_color(theme::accent())
                .font_weight(FontWeight::MEDIUM)
        }))
        .children(
            row.here
                .then(|| tag("here", theme::on_accent(), theme::accent())),
        )
        .children(
            row.ephemeral
                .then(|| tag("ephemeral", theme::muted(), theme::selected_row())),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_workspace_menu(&menu_path, event.position, cx);
            }),
        )
        .when_some(open, |this, path| {
            this.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.open_worktree(&path, cx);
            }))
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

/// The hint under the stack tree, for the drag that sets a parent.
fn footer_hint() -> Div {
    div()
        .flex_none()
        .px_4()
        .py_2()
        .border_t_1()
        .border_color(theme::hairline())
        .text_color(theme::muted())
        .child("Drag a branch onto another to set its parent.")
}
