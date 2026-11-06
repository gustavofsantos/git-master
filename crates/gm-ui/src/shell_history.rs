//! The history page (design/roadmap.md, H1 to H3): the commits of HEAD, newest first, one
//! 48 px row each, with its ref chips, subject, short id and relative time. The
//! next page loads as the list nears its end; a click selects a row by its id
//! and fills the detail panel on the right. Scope, graph and search come in
//! later sub-slices.

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use gm_schema::{
    Actor, Change, CommandKind, CommitDetail, CommitFile, CommitSummary, RefKind, RefLabel,
};
use gpui::{
    App, ClickEvent, Context, Div, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent,
    PathBuilder, Pixels, Stateful, Window, canvas, deferred, div, img, point, prelude::*, px,
    uniform_list,
};

use crate::action_registry::{Action, Args, Context as ActionContext};
use crate::avatar::{self, Avatars};
use crate::commit_threads;
use crate::history_graph::{self, Drawing, GraphRow, StackMarks, Tint};
use crate::history_scope;
use crate::icon::{self, Icon};
use crate::model::{CommitPage, CommitPane, HistoryList};
use crate::shell_list::change_letter;
use crate::shell_threads::chip;
use crate::stashes;
use crate::text_area::{TextArea, TextStyle};
use crate::theme;
use crate::threads;
use crate::view::{Screen, StatusView};
use crate::window_chrome;

const ROW_HEIGHT: f32 = 48.;
const TIME_WIDTH: f32 = 96.;
/// The row's side padding (`px_4`), where the graph column starts.
const ROW_PADDING: f32 = 16.;
/// The stroke of a graph line, the radius of a node and of the ring around HEAD's.
const LINE_WIDTH: f32 = 1.5;
/// The length of a dash, and of the gap after it, on a lag edge (D25).
const DASH: f32 = 3.0;
const NODE_RADIUS: f32 = 3.5;
const HEAD_RING_RADIUS: f32 = 6.;
const DETAIL_WIDTH: f32 = 360.;
/// The label column of the detail's fields, and the row height of their text.
const FIELD_LABEL_WIDTH: f32 = 56.;
const DETAIL_LINE_HEIGHT: f32 = 17.;
/// The side of an author's picture.
const AVATAR_SIZE: f32 = 24.;
/// The next page is asked for when the rows drawn reach this close to the end.
const PAGE_PREFETCH: usize = 20;

impl StatusView {
    /// The page for the main area: a header, then the list, or the one line
    /// that says why there is no list.
    pub(crate) fn history_page(&self, cx: &mut Context<Self>) -> Div {
        let list = match &self.model.history {
            HistoryList::Idle => message("Loading history…"),
            HistoryList::Failed => {
                message("The history could not be read. The status bar says why.")
            }
            HistoryList::Shown(listing) if !listing.loaded => message("Loading history…"),
            HistoryList::Shown(listing) if listing.history.unborn => {
                message("HEAD points at a branch with no commits yet.")
            }
            HistoryList::Shown(listing) if listing.history.commits.is_empty() => {
                message("No commits to show.")
            }
            HistoryList::Shown(listing) => history_list(listing.history.commits.len(), cx),
        };
        let count = match &self.model.history {
            HistoryList::Shown(listing) if listing.loaded => {
                count_label(listing.history.commits.len(), listing.history.more)
            }
            _ => String::new(),
        };
        let threads = self.commit_threads_section(cx);
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(theme::ground())
            .child(
                div()
                    .h(px(44.))
                    .px_4()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(div().font_weight(FontWeight::MEDIUM).child("History"))
                            .child(self.scope_pill(cx)),
                    )
                    .child(div().text_xs().text_color(theme::muted()).child(count)),
            )
            .child(if self.model.commit_stack.is_open() {
                self.commit_file_view(cx)
            } else {
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .flex()
                            .flex_col()
                            .child(list),
                    )
                    .child(commit_panel(
                        &self.model.commit,
                        &self.commit_texts,
                        &Panel::Commit,
                        None,
                        threads,
                        cx,
                    ))
            })
    }
}

impl StatusView {
    /// The scope pill of the history header (H6, D139): the scope on screen, and
    /// its menu below it while open. A line that cannot be chosen says why.
    fn scope_pill(&self, cx: &Context<Self>) -> Div {
        let head = self.head_name();
        let scope = self
            .model
            .history_scope
            .effective(head.as_deref(), &self.model.stacks);
        let pill = div()
            .id("history-scope")
            .px_2()
            .h(px(22.))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .border_1()
            .border_color(theme::hairline_strong())
            .text_xs()
            .text_color(theme::text())
            .cursor_pointer()
            .hover(|style| style.bg(theme::selected_row()))
            .child(history_scope::label(&scope))
            .child(icon::view(Icon::CaretDown, icon::SMALL, theme::muted()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.history_scope_menu = !this.history_scope_menu;
                    cx.notify();
                }),
            );
        let menu = self.history_scope_menu.then(|| {
            let lines = history_scope::options(&scope, head.as_deref(), &self.model.stacks);
            div()
                .absolute()
                .top(px(26.))
                .left_0()
                .w(px(260.))
                .py_1()
                .flex()
                .flex_col()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::hairline_strong())
                .rounded_md()
                .shadow_lg()
                .occlude()
                .children(lines.into_iter().map(|line| scope_line(line, cx)))
        });
        // Deferred, so it paints over the commit rows that come after the header.
        div()
            .relative()
            .child(pill)
            .children(menu.map(|menu| deferred(menu).with_priority(1)))
    }
}

/// One line of the scope menu: a check on the scope on screen, the title, and
/// the reason when it cannot be chosen.
fn scope_line(line: history_scope::ScopeOption, cx: &Context<StatusView>) -> Stateful<Div> {
    let enabled = line.state.is_ok();
    let reason = line.state.err();
    div()
        .id(line.title)
        .px_3()
        .py_1()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .text_sm()
        .when(enabled, |row| {
            row.cursor_pointer()
                .hover(|style| style.bg(theme::selected_row()))
        })
        .when(!enabled, |row| row.text_color(theme::muted()))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .text_color(theme::accent())
                .children(
                    line.current
                        .then(|| icon::view(Icon::Check, icon::SMALL, theme::accent())),
                ),
        )
        .child(div().flex_1().child(line.title))
        .children(reason.map(|reason| div().text_xs().text_color(theme::muted()).child(reason)))
        .when_some(line.scope.filter(|_| enabled), |row, scope| {
            row.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.choose_history_scope(scope.clone(), cx);
                }),
            )
        })
}

fn message(text: &'static str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(text)
}

/// The rows, read from the listing as it is when they are drawn.
fn history_list(count: usize, cx: &mut Context<StatusView>) -> Div {
    div().flex_1().min_h(px(0.)).flex().flex_col().child(
        uniform_list(
            "history-commits",
            count,
            cx.processor(
                move |this: &mut StatusView,
                      range: Range<usize>,
                      _window: &mut Window,
                      cx: &mut Context<StatusView>| {
                    if range.end + PAGE_PREFETCH >= count {
                        this.request_history_page();
                    }
                    let HistoryList::Shown(listing) = &this.model.history else {
                        return Vec::new();
                    };
                    let now = now_secs();
                    let selected = listing.selected.as_deref();
                    let width = history_graph::graph_width(&listing.graph);
                    let marks = listing.graph.marks();
                    listing
                        .history
                        .commits
                        .get(range.clone())
                        .unwrap_or_default()
                        .iter()
                        .zip(range)
                        .map(|(commit, index)| {
                            let is_selected = selected == Some(commit.oid.as_str());
                            let graph = RowGraph {
                                row: listing.graph.row(index),
                                width,
                                marks,
                            };
                            commit_row(commit, index, now, is_selected, &graph, cx)
                        })
                        .collect()
                },
            ),
        )
        .flex_1()
        .min_h(px(0.)),
    )
}

/// What a commit row draws of the graph: its row of the layout, the width of the
/// graph column and the stack marks the layout was made with.
struct RowGraph<'a> {
    row: Option<&'a GraphRow>,
    width: f32,
    marks: &'a StackMarks,
}

fn commit_row(
    commit: &CommitSummary,
    index: usize,
    now: i64,
    selected: bool,
    graph: &RowGraph<'_>,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let graph_width = graph.width;
    // Each branch's badge follows its own chip, so it is never read as another's.
    let chips: Vec<Div> = commit
        .refs
        .iter()
        .flat_map(|label| {
            let behind = (label.kind == RefKind::Branch)
                .then(|| graph.marks.behind(&label.name))
                .flatten();
            std::iter::once(ref_chip(label)).chain(behind.map(behind_badge))
        })
        .collect();
    let graph = graph.row;
    let oid = commit.oid.clone();
    let is_head = commit.refs.iter().any(|label| label.kind == RefKind::Head);
    div()
        .id(index)
        .relative()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(theme::hairline())
        .when(selected, |row| row.bg(theme::selected_row()))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                this.select_commit(&oid, cx);
            }),
        )
        .when(graph_width > 0., |row| {
            row.child(div().flex_none().w(px(graph_width)))
                .when_some(graph, |row, graph| {
                    row.child(graph_cell(graph, graph_width, is_head))
                })
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .overflow_hidden()
                .children(chips)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(commit.subject.clone()),
                ),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::muted())
                .child(commit.short.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(px(TIME_WIDTH))
                .text_xs()
                .text_color(theme::muted())
                .text_right()
                .child(relative_time(now, commit.author_time)),
        )
}

/// The graph column of a row, laid over the row's whole height (borders
/// included) so a lane reaches the rows above and below it. The layout is
/// `history_graph::drawing`; this paints it.
fn graph_cell(row: &GraphRow, width: f32, is_head: bool) -> Div {
    let drawn = history_graph::drawing(row, ROW_HEIGHT);
    let node_tint = row.node_tint;
    let overflow = history_graph::overflow_label(row);
    div()
        .absolute()
        .top_0()
        .left(px(ROW_PADDING))
        .w(px(width))
        .h(px(ROW_HEIGHT))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _cx| {
                    paint_graph(window, bounds.origin, &drawn, node_tint, is_head);
                },
            )
            .absolute()
            .size_full(),
        )
        .when_some(overflow, |cell, label| {
            cell.child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(ROW_HEIGHT / 2. - 8.))
                    .text_xs()
                    .text_color(theme::muted())
                    .child(label),
            )
        })
}

fn tint_color(tint: Tint) -> Hsla {
    match tint {
        Tint::Trunk => theme::graph_trunk(),
        Tint::Slot(slot) => theme::graph_lane(slot),
        Tint::Lag => theme::warn_fg(),
    }
}

/// Paints the lines, then the node over them. HEAD's node has a ring, so it
/// does not rely on colour alone.
fn paint_graph(
    window: &mut Window,
    origin: gpui::Point<Pixels>,
    drawn: &Drawing,
    node_tint: Tint,
    is_head: bool,
) {
    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    for line in &drawn.lines {
        let mut path = PathBuilder::stroke(px(LINE_WIDTH));
        if line.dashed {
            path = path.dash_array(&[px(DASH), px(DASH)]);
        }
        path.move_to(at(line.from));
        if (line.from.0 - line.to.0).abs() < f32::EPSILON {
            path.line_to(at(line.to));
        } else {
            let bend = f32::midpoint(line.from.1, line.to.1);
            path.cubic_bezier_to(at(line.to), at((line.from.0, bend)), at((line.to.0, bend)));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, tint_color(line.tint));
        }
    }
    let Some(center) = drawn.node else {
        return;
    };
    let color = tint_color(node_tint);
    paint_circle(window, at(center), NODE_RADIUS, None, color);
    if is_head {
        paint_circle(
            window,
            at(center),
            HEAD_RING_RADIUS,
            Some(LINE_WIDTH),
            color,
        );
    }
}

/// A circle at `center`: filled, or a ring of the given stroke width.
fn paint_circle(
    window: &mut Window,
    center: gpui::Point<Pixels>,
    radius: f32,
    ring: Option<f32>,
    color: Hsla,
) {
    let mut path = ring.map_or_else(PathBuilder::fill, |width| PathBuilder::stroke(px(width)));
    let r = px(radius);
    let left = point(center.x - r, center.y);
    let right = point(center.x + r, center.y);
    let radii = point(r, r);
    path.move_to(left);
    path.arc_to(radii, px(0.), false, true, right);
    path.arc_to(radii, px(0.), false, true, left);
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// The badge of a branch tip that is behind its parent (D25): amber, and the
/// words say it, so the colour is not the only signal.
fn behind_badge(count: u32) -> Div {
    let commits = if count == 1 { "commit" } else { "commits" };
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .bg(theme::warn_bg())
        .text_color(theme::warn_fg())
        .child(format!("{count} {commits} behind parent"))
}

/// A ref as a small chip: HEAD in the accent colour, branches in the selection
/// colour, tags muted.
fn ref_chip(label: &RefLabel) -> Div {
    let (background, text) = match label.kind {
        RefKind::Head => (theme::accent(), theme::on_accent()),
        RefKind::Branch => (theme::selection(), theme::text()),
        RefKind::Tag => (theme::selected_row(), theme::muted()),
    };
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .bg(background)
        .text_color(text)
        .child(label.name.clone())
}

/// One of the detail panel's action buttons. Its rule is the registry's: a refused
/// button is muted and its reason is its tooltip; a click on it runs the action
/// through `run_action`, which says the reason when it is refused.
pub(crate) fn action_button(
    action: Action,
    id: &'static str,
    label: &'static str,
    target: &Args,
    ctx: &ActionContext<'_>,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let refusal = action.enabled(target, ctx).err();
    let colour = match refusal {
        Some(_) => theme::muted(),
        None if action.destructive() => theme::removed_fg(),
        None => theme::text(),
    };
    let button = div()
        .id(id)
        .flex_none()
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .text_color(colour)
        .child(label)
        .on_mouse_down(MouseButton::Left, {
            let target = target.clone();
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.run_action(action, &target, window, cx);
            })
        });
    match refusal {
        None => button.cursor_pointer(),
        Some(reason) => button.tooltip(window_chrome::hint(reason)),
    }
}

/// The read-only text areas of the detail panel: one per field of the selected
/// commit, so its text can be selected and copied. `show` fills them.
pub(crate) struct CommitTexts {
    subject: Entity<TextArea>,
    body: Entity<TextArea>,
    hash: Entity<TextArea>,
    author: Entity<TextArea>,
    date: Entity<TextArea>,
    parents: Entity<TextArea>,
    /// The author pictures, shared with the tasks that fetch them (D110).
    pub(crate) avatars: Avatars,
}

impl CommitTexts {
    pub(crate) fn new(avatars: Avatars, cx: &mut Context<StatusView>) -> Self {
        let detail = TextStyle {
            line_height: DETAIL_LINE_HEIGHT,
            size: 12.,
            ..TextStyle::default()
        };
        let subject = TextStyle {
            line_height: 22.,
            size: 16.,
            weight: FontWeight::SEMIBOLD,
            ..TextStyle::default()
        };
        let code = TextStyle {
            mono: true,
            ..detail
        };
        Self {
            subject: read_only_area(subject, cx),
            body: read_only_area(detail, cx),
            hash: read_only_area(code, cx),
            author: read_only_area(detail, cx),
            date: read_only_area(detail, cx),
            parents: read_only_area(code, cx),
            avatars,
        }
    }

    /// Puts the commit's fields in the areas. An area whose text is already
    /// the same is left alone, so its selection survives a repaint.
    fn show(&self, detail: &CommitDetail, cx: &mut Context<StatusView>) {
        let author = author_label(&detail.author_name, &detail.author_email);
        let parents = parents_label(&detail.parents);
        show_text(&self.subject, &detail.subject, cx);
        show_text(&self.body, &detail.body, cx);
        show_text(&self.hash, &detail.oid, cx);
        show_text(&self.author, &author, cx);
        show_text(&self.date, &detail.author_date, cx);
        show_text(&self.parents, &parents, cx);
    }
}

fn read_only_area(style: TextStyle, cx: &mut Context<StatusView>) -> Entity<TextArea> {
    cx.new(|cx| TextArea::new("", cx).read_only().with_style(style))
}

fn show_text(area: &Entity<TextArea>, text: &str, cx: &mut Context<StatusView>) {
    if area.read(cx).text() != text {
        area.update(cx, |area, cx| area.set_text(text, cx));
    }
}

/// What the detail panel shows: a commit of the history, a stash of the Stashes
/// page (B2), which is named `stash@{n}` and has no author or parents to show, or
/// an entry of the Reflog page (B3), which is named by its header. A stash's index,
/// and a reflog entry's header, are `None` only while none is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    Commit,
    Stash(Option<u32>),
    /// The entry's header, `HEAD@{n} · action`.
    Reflog(Option<String>),
}

impl Panel {
    fn title(&self) -> String {
        match self {
            Self::Commit => "SELECTED COMMIT".to_owned(),
            Self::Stash(Some(index)) => stashes::label(*index),
            Self::Stash(None) => "SELECTED STASH".to_owned(),
            Self::Reflog(Some(header)) => header.clone(),
            Self::Reflog(None) => "SELECTED ENTRY".to_owned(),
        }
    }

    fn empty_text(&self) -> &'static str {
        match self {
            Self::Commit => "Select a commit to see its details.",
            Self::Stash(_) => "Select a stash to see its details.",
            Self::Reflog(_) => "Select an entry to see its details.",
        }
    }

    /// Whether the panel shows the commit's author and parents, which a stash does not.
    fn shows_author(&self) -> bool {
        matches!(self, Self::Commit | Self::Reflog(_))
    }
}

/// The detail panel (H3): the selected commit's subject, body, hash, author,
/// date, parents and files. Its text is selectable; the Copy controls copy the
/// hash and the whole message. A file row opens its diff (H4). A stash shows
/// the same, without the author and the parents (B2), and `actions` (its
/// Apply, Pop and Drop buttons) above the rest.
pub(crate) fn commit_panel(
    pane: &CommitPane,
    texts: &CommitTexts,
    panel: &Panel,
    actions: Option<Div>,
    footer: Option<Div>,
    cx: &mut Context<StatusView>,
) -> Stateful<Div> {
    let content = match pane {
        CommitPane::Empty => message_text(panel.empty_text()).into_any_element(),
        CommitPane::Loading(_) => message_text("Loading commit…").into_any_element(),
        CommitPane::Failed(text) => div()
            .text_color(theme::error())
            .child(text.clone())
            .into_any_element(),
        CommitPane::Shown(detail) => commit_detail(detail, texts, panel, cx).into_any_element(),
    };
    div()
        .id("commit-detail")
        .flex_none()
        .w(px(DETAIL_WIDTH))
        .h_full()
        .min_h(px(0.))
        .overflow_y_scroll()
        .border_l_1()
        .border_color(theme::hairline())
        .bg(theme::panel())
        .p_4()
        .flex()
        .flex_col()
        .gap_3p5()
        .text_sm()
        .children(actions)
        .child(content)
        .children(footer)
}

fn message_text(text: &'static str) -> Div {
    div().text_color(theme::muted()).child(text)
}

fn commit_detail(
    detail: &CommitDetail,
    texts: &CommitTexts,
    panel: &Panel,
    cx: &mut Context<StatusView>,
) -> Div {
    let copy_message = copy_button(
        "commit-copy-message",
        "Copy message",
        cx.listener(|this, _: &ClickEvent, window, cx| this.copy_commit_message(window, cx)),
    );
    let mut column = div()
        .flex()
        .flex_col()
        .gap_3p5()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(section_label(&panel.title()))
                .child(copy_message),
        )
        .child(texts.subject.clone());
    if !detail.body.is_empty() {
        column = column.child(texts.body.clone());
    }
    let copy_hash = copy_button(
        "commit-copy-hash",
        "Copy",
        cx.listener(|this, _: &ClickEvent, window, cx| this.copy_commit_hash(window, cx)),
    );
    column
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .text_xs()
                .child(field("Hash", texts.hash.clone(), Some(copy_hash)))
                .children(panel.shows_author().then(|| {
                    person_field(
                        "Author",
                        avatar_disc(&texts.avatars, &detail.author_name, &detail.author_email),
                        texts.author.clone(),
                    )
                }))
                .children(
                    panel
                        .shows_author()
                        .then(|| co_author_rows(&detail.body, &texts.avatars)),
                )
                .child(field("Date", texts.date.clone(), None))
                .children(
                    panel
                        .shows_author()
                        .then(|| field("Parents", texts.parents.clone(), None)),
                ),
        )
        .child(section_label(&format!(
            "FILES ({})",
            detail.files.len() + detail.more_files
        )))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .text_xs()
                .children(
                    detail
                        .files
                        .iter()
                        .enumerate()
                        .map(|(ix, file)| file_row(file, ix, cx)),
                )
                .children((detail.more_files > 0).then(|| {
                    div()
                        .text_color(theme::muted())
                        .child(more_files_label(detail.more_files))
                })),
        )
}

/// A labelled field of the detail: its label, then its selectable text, then an
/// optional control such as Copy.
fn field(label: &'static str, value: Entity<TextArea>, action: Option<Stateful<Div>>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .child(field_label(label).flex_none().w(px(FIELD_LABEL_WIDTH)))
        .child(div().flex_1().min_w(px(0.)).child(value))
        .children(action)
}

/// The author's field: the picture, then the selectable name and email.
fn person_field(label: &'static str, disc: Div, value: Entity<TextArea>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .child(field_label(label).flex_none().w(px(FIELD_LABEL_WIDTH)))
        .child(disc)
        .child(div().flex_1().min_w(px(0.)).child(value))
}

/// One row for each `Co-authored-by` trailer of the message: the picture, the
/// name and the email. Nothing when the message has none.
fn co_author_rows(body: &str, avatars: &Avatars) -> Div {
    let people = avatar::co_authors(body);
    let mut rows = div().flex().flex_col().gap_2();
    for (ix, person) in people.iter().enumerate() {
        rows = rows.child(
            div()
                .id(("co-author", ix))
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .child(
                    field_label(if ix == 0 { "With" } else { "" })
                        .flex_none()
                        .w(px(FIELD_LABEL_WIDTH)),
                )
                .child(avatar_disc(avatars, &person.name, &person.email))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(author_label(&person.name, &person.email)),
                ),
        );
    }
    rows
}

/// A person's picture as a disc, or their initials on a colour taken from the
/// email while there is no picture.
fn avatar_disc(avatars: &Avatars, name: &str, email: &str) -> Div {
    let disc = div()
        .size(px(AVATAR_SIZE))
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();
    match avatars.picture(email) {
        Some(path) => disc.child(img(path).size_full().object_fit(gpui::ObjectFit::Cover)),
        None => disc
            .bg(gpui::hsla(avatar::hue(email), 0.45, 0.42, 1.))
            .text_color(gpui::white())
            .text_size(px(AVATAR_SIZE * 0.4))
            .font_weight(FontWeight::SEMIBOLD)
            .child(avatar::initials(name)),
    }
}

/// A small text control with a border, for the Copy actions.
fn copy_button(
    id: &'static str,
    text: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px_1p5()
        .h(px(20.))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_xs()
        .text_color(theme::muted())
        .cursor_pointer()
        .child(text)
        .on_click(on_click)
}

fn section_label(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn field_label(text: &'static str) -> Div {
    div().text_color(theme::muted()).child(text)
}

/// One changed file: its change letter, its path (old to new for a rename) and
/// its line counts. A click opens the file's diff (H4).
fn file_row(file: &CommitFile, ix: usize, cx: &mut Context<StatusView>) -> Stateful<Div> {
    let (letter, color) = (change_letter(file.change), change_color(file.change));
    div()
        .id(("commit-file", ix))
        .cursor_pointer()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .text_center()
                .font_family(theme::code_font())
                .text_color(color)
                .child(letter.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(path_label(file)),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_color(theme::muted())
                .child(file_counts(file)),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_commit_file(ix, window, cx);
        }))
}

pub(crate) fn change_color(change: Change) -> Hsla {
    match change {
        Change::Added => theme::staged(),
        Change::Modified | Change::TypeChanged => theme::unstaged(),
        Change::Deleted => theme::error(),
        Change::Renamed | Change::Copied => theme::untracked(),
    }
}

/// The line counts of a file: `+20 −2`, `+44`, `−3`, `±0`, or `bin` for a binary file.
#[must_use]
pub fn file_counts(file: &CommitFile) -> String {
    if file.binary {
        return "bin".to_owned();
    }
    let mut parts = Vec::new();
    if file.added > 0 {
        parts.push(format!("+{}", file.added));
    }
    if file.deleted > 0 {
        parts.push(format!("\u{2212}{}", file.deleted));
    }
    if parts.is_empty() {
        "\u{b1}0".to_owned()
    } else {
        parts.join(" ")
    }
}

/// The path a row shows: the new path, or `old → new` for a rename.
#[must_use]
pub fn path_label(file: &CommitFile) -> String {
    match &file.old_path {
        Some(old) => format!("{old} \u{2192} {}", file.path),
        None => file.path.clone(),
    }
}

#[must_use]
pub fn more_files_label(count: usize) -> String {
    format!("and {count} more")
}

/// The message as Git stores it: the subject, then a blank line and the body
/// when there is one.
#[must_use]
pub fn full_message(subject: &str, body: &str) -> String {
    if body.is_empty() {
        subject.to_owned()
    } else {
        format!("{subject}\n\n{body}")
    }
}

/// The parents, abbreviated and comma-separated; `none` for a root commit.
#[must_use]
pub fn parents_label(parents: &[String]) -> String {
    if parents.is_empty() {
        "none".to_owned()
    } else {
        parents.join(", ")
    }
}

/// The author as Git writes it: the name, then the email in angle brackets.
#[must_use]
pub fn author_label(name: &str, email: &str) -> String {
    format!("{name} <{email}>")
}

impl StatusView {
    /// Selects the commit `oid` and asks for its detail. The selection survives a
    /// refresh of the list. Selecting another commit supersedes the read in flight.
    pub(crate) fn select_commit(&mut self, oid: &str, cx: &mut Context<Self>) {
        self.model.history.select(oid);
        self.model.commit_stack.close();
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitDetail {
                oid: oid.to_owned(),
            },
        );
        self.model.commit.request(id);
        self.request_commit_threads(oid);
        cx.notify();
    }

    /// The page whose commit the detail panel and the stacked diff show: the
    /// Stashes or the Reflog page while it is on screen, else the history.
    pub(crate) fn commit_page(&self) -> CommitPage {
        match self.screen {
            Screen::Stashes => CommitPage::Stashes,
            Screen::Reflog => CommitPage::Reflog,
            _ => CommitPage::History,
        }
    }

    /// Fills the detail panel's text areas with the shown commit of the page on screen.
    pub(crate) fn show_commit_texts(&self, cx: &mut Context<Self>) {
        if let CommitPane::Shown(detail) = self.model.commit_pane(self.commit_page()) {
            self.commit_texts.show(detail, cx);
            self.fetch_pictures(detail, cx);
        }
    }

    /// Asks Gravatar, in the background, for the pictures of the commit's author and
    /// co-authors that are not known yet; the panel repaints as each one arrives.
    fn fetch_pictures(&self, detail: &CommitDetail, cx: &mut Context<Self>) {
        let co_authors = avatar::co_authors(&detail.body);
        let mut emails = vec![detail.author_email.as_str()];
        emails.extend(co_authors.iter().map(|person| person.email.as_str()));
        let avatars = self.commit_texts.avatars.clone();
        let (dir, wanted) = avatars.to_fetch(&emails);
        let Some(dir) = dir else {
            return;
        };
        for email in wanted {
            let (avatars, dir) = (avatars.clone(), dir.clone());
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_executor()
                    .spawn({
                        let (dir, email) = (dir.clone(), email.clone());
                        async move { avatar::fetch(&dir, &email) }
                    })
                    .await;
                avatars.record(&dir, &email, found);
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// Turns the Gravatar lookup on or off and remembers the choice (D110).
    pub(crate) fn set_author_pictures(&mut self, on: bool, cx: &mut Context<Self>) {
        self.commit_texts.avatars.set_enabled(on);
        if self.settings.author_pictures != on {
            self.settings.author_pictures = on;
            self.save_settings(cx);
        }
        self.show_commit_texts(cx);
        cx.notify();
    }

    /// The Threads section of the commit panel (H3): the threads linked to the commit
    /// shown, each opening its thread on the Threads page. Nothing when it has none.
    pub(crate) fn commit_threads_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let CommitPane::Shown(detail) = &self.model.commit else {
            return None;
        };
        let threads = self.model.commit_threads.threads_of(&detail.oid);
        if threads.is_empty() {
            return None;
        }
        let now = now_secs();
        let rows: Vec<Stateful<Div>> = threads
            .iter()
            .enumerate()
            .map(|(index, thread)| {
                let target = commit_threads::target(thread);
                let caption = commit_threads::workspace_caption(thread);
                div()
                    .id(("commit-thread", index))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w(px(0.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| match &target {
                            commit_threads::Target::Thread(uid) => {
                                this.open_thread_from_commit(uid, window, cx);
                            }
                            commit_threads::Target::Workspace(folder) => {
                                this.open_worktree(folder, cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(threads::title(thread)),
                            )
                            .child(chip(thread.harness.clone()))
                            .children(caption.map(|caption| {
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_color(theme::muted())
                                    .child(caption)
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(threads::relative(now, thread.last_event_at)),
                    )
            })
            .collect();
        let more = self.model.commit_threads.is_full().then(|| {
            div()
                .text_xs()
                .text_color(theme::muted())
                .child("and more. Open Threads to see them all.")
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(section_label("THREADS"))
                .child(div().flex().flex_col().gap_3().text_xs().children(rows))
                .children(more),
        )
    }

    /// The Copy control by the hash: copies the full commit id.
    pub(crate) fn copy_commit_hash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let oid = match self.model.commit_pane(self.commit_page()) {
            CommitPane::Shown(detail) => detail.oid.clone(),
            _ => return,
        };
        self.run_action(Action::CopyCommitHash, &Args::Commit(oid), window, cx);
    }

    /// The Copy control by the message: copies the subject and the body.
    pub(crate) fn copy_commit_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = match self.model.commit_pane(self.commit_page()) {
            CommitPane::Shown(detail) => full_message(&detail.subject, &detail.body),
            _ => return,
        };
        self.run_action(
            Action::CopyCommitMessage,
            &Args::Commit(message),
            window,
            cx,
        );
    }

    /// Opens the stacked diff of the selected commit, with file `focus` at the top
    /// (H4c). The read is the whole commit's patch, against its first parent.
    pub(crate) fn select_commit_file(&mut self, focus: usize, cx: &mut Context<Self>) {
        let page = self.commit_page();
        let (oid, files, more) = match self.model.commit_pane(page) {
            CommitPane::Shown(detail) if focus < detail.files.len() => {
                (detail.oid.clone(), detail.files.clone(), detail.more_files)
            }
            _ => return,
        };
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::LoadCommitStack {
                oid,
                files: files.clone(),
            },
        );
        self.model
            .commit_stack_mut(page)
            .request(id, focus, files, more);
        cx.notify();
    }

    /// A click on a file row: opens the stack at that file and gives the diff pane the keys.
    pub(crate) fn open_commit_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_commit_file(index, cx);
        let handle = self.commit_diff_pane.read(cx).focus_handle();
        window.focus(&handle);
    }

    /// Back to the commit view, from its button or Escape.
    pub(crate) fn close_commit_file(&mut self, cx: &mut Context<Self>) {
        let page = self.commit_page();
        self.model.commit_stack_mut(page).close();
        cx.notify();
    }

    /// The stacked diff of the commit in place of its list and panel (H4c): a
    /// header with the way back, the commit, then the diff of every file, a
    /// loading line or git's message.
    pub(crate) fn commit_file_view(&self, cx: &mut Context<Self>) -> Div {
        let owner = self.commit_page();
        let (short, subject) = match self.model.commit_pane(owner) {
            CommitPane::Shown(detail) => (detail.short.clone(), detail.subject.clone()),
            _ => (String::new(), String::new()),
        };
        let back_label = match owner {
            CommitPage::History => "Back to commit",
            CommitPage::Stashes => "Back to stash",
            CommitPage::Reflog => "Back to entry",
        };
        let back = div()
            .id("commit-file-back")
            .px_2()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(theme::button_border())
            .cursor_pointer()
            .child(back_label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_commit_file(cx)));
        let header = div()
            .h(px(36.))
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .min_w(px(0.))
            .border_b_1()
            .border_color(theme::hairline())
            .child(back)
            .child(
                div()
                    .flex_none()
                    .font_family(theme::code_font())
                    .text_color(theme::muted())
                    .child(short),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(subject),
            );
        let stack = self.model.commit_stack(owner);
        let body = if let Some(message) = stack.failure() {
            div()
                .px_4()
                .py_3()
                .text_color(theme::error())
                .child(message.to_owned())
        } else {
            // The pane stays in the tree while the diffs load, so it keeps the
            // keys. Its box is empty until the stack is shown.
            let loading = !stack.is_shown();
            let pane = div()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .when(loading, |this| this.h(px(0.)))
                .when(!loading, gpui::Styled::flex_1)
                .child(self.commit_diff_pane.clone());
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .children(loading.then(|| {
                    div()
                        .px_4()
                        .py_3()
                        .text_color(theme::muted())
                        .child("Loading the commit's files…")
                }))
                .child(pane)
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(body)
    }
}

pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

/// The count in the header: "1 commit", "12 commits", or "First N commits"
/// while more pages follow.
#[must_use]
pub fn count_label(count: usize, more: bool) -> String {
    if more {
        format!("First {count} commits")
    } else if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// How long ago `then` was, as the list shows it in English. A time in the
/// future (clock skew) reads as "just now".
#[must_use]
pub fn relative_time(now: i64, then: i64) -> String {
    let age = (now - then).max(0);
    if age < MINUTE {
        "just now".to_owned()
    } else if age < HOUR {
        format!("{} min ago", age / MINUTE)
    } else if age < DAY {
        format!("{} h ago", age / HOUR)
    } else if age < 2 * DAY {
        "yesterday".to_owned()
    } else if age < WEEK {
        ago(age / DAY, "day")
    } else if age < MONTH {
        ago(age / WEEK, "week")
    } else if age < YEAR {
        ago(age / MONTH, "month")
    } else {
        ago(age / YEAR, "year")
    }
}

fn ago(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{Change, CommitFile};

    #[test]
    fn recent_times_are_in_minutes_and_hours() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 59), "just now");
        assert_eq!(relative_time(now, now - 5 * MINUTE), "5 min ago");
        assert_eq!(relative_time(now, now - 3 * HOUR), "3 h ago");
    }

    #[test]
    fn a_day_is_yesterday_and_then_counted() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now - DAY - HOUR), "yesterday");
        assert_eq!(relative_time(now, now - 2 * DAY), "2 days ago");
        assert_eq!(relative_time(now, now - 6 * DAY), "6 days ago");
    }

    #[test]
    fn weeks_months_and_years_are_singular_at_one() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now - WEEK), "1 week ago");
        assert_eq!(relative_time(now, now - 3 * WEEK), "3 weeks ago");
        assert_eq!(relative_time(now, now - MONTH), "1 month ago");
        assert_eq!(relative_time(now, now - 2 * YEAR), "2 years ago");
    }

    #[test]
    fn a_time_in_the_future_reads_as_just_now() {
        assert_eq!(relative_time(100, 500), "just now");
    }

    fn file(path: &str, added: u32, deleted: u32, binary: bool) -> CommitFile {
        CommitFile {
            path: path.into(),
            old_path: None,
            change: Change::Modified,
            added,
            deleted,
            binary,
        }
    }

    #[test]
    fn counts_show_the_added_and_deleted_lines() {
        assert_eq!(file_counts(&file("a", 20, 2, false)), "+20 \u{2212}2");
        assert_eq!(file_counts(&file("a", 44, 0, false)), "+44");
        assert_eq!(file_counts(&file("a", 0, 3, false)), "\u{2212}3");
        assert_eq!(file_counts(&file("a", 0, 0, false)), "\u{b1}0");
    }

    #[test]
    fn a_binary_file_says_bin_instead_of_counts() {
        assert_eq!(file_counts(&file("logo.png", 0, 0, true)), "bin");
    }

    #[test]
    fn a_rename_shows_the_old_path_before_the_new_one() {
        let renamed = CommitFile {
            path: "src/new.rs".into(),
            old_path: Some("src/old.rs".into()),
            change: Change::Renamed,
            added: 0,
            deleted: 0,
            binary: false,
        };
        assert_eq!(path_label(&renamed), "src/old.rs \u{2192} src/new.rs");
        assert_eq!(
            path_label(&file("api/routes.rs", 1, 0, false)),
            "api/routes.rs"
        );
    }

    #[test]
    fn the_list_says_how_many_files_it_leaves_out() {
        assert_eq!(more_files_label(50), "and 50 more");
        assert_eq!(more_files_label(1), "and 1 more");
    }

    #[test]
    fn parents_are_listed_short_and_a_root_has_none() {
        assert_eq!(parents_label(&[]), "none");
        assert_eq!(
            parents_label(&["6b0b1eb".into(), "41a9853".into()]),
            "6b0b1eb, 41a9853"
        );
    }

    #[test]
    fn the_author_shows_the_name_then_the_email() {
        assert_eq!(
            author_label("Zo\u{eb} \u{dc}nal", "zoe@example.test"),
            "Zo\u{eb} \u{dc}nal <zoe@example.test>"
        );
    }

    #[test]
    fn the_message_copied_is_the_subject_and_the_body_after_a_blank_line() {
        assert_eq!(
            full_message("feat: select text", "First paragraph.\n\nSecond."),
            "feat: select text\n\nFirst paragraph.\n\nSecond."
        );
    }

    #[test]
    fn a_message_without_a_body_copies_only_the_subject() {
        assert_eq!(full_message("fix: typo", ""), "fix: typo");
    }

    #[test]
    fn a_stash_panel_is_named_by_its_index_and_a_commit_panel_is_not() {
        assert_eq!(Panel::Commit.title(), "SELECTED COMMIT");
        assert_eq!(Panel::Stash(Some(2)).title(), "stash@{2}");
        assert_eq!(Panel::Stash(None).title(), "SELECTED STASH");
        assert_eq!(
            Panel::Stash(None).empty_text(),
            "Select a stash to see its details."
        );
        assert_eq!(
            Panel::Commit.empty_text(),
            "Select a commit to see its details."
        );
    }

    #[test]
    fn the_count_says_when_the_list_is_cut() {
        assert_eq!(count_label(0, false), "0 commits");
        assert_eq!(count_label(1, false), "1 commit");
        assert_eq!(count_label(42, false), "42 commits");
        assert_eq!(count_label(200, true), "First 200 commits");
        assert_eq!(count_label(200, false), "200 commits");
    }
}
