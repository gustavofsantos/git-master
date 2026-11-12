//! The Code page (design/code-navigation.md, N2): the folder tree on the left and a read-only
//! viewer of one file on the right. The tree and the viewer's state come from
//! `code_page.rs`; the engine lists a folder from the file list it holds in
//! memory and reads the file from disk. Every request goes out with an
//! `OperationId`, and an answer to a superseded one is dropped there.

use std::ops::Range;

use gm_schema::{Actor, CommandKind};
use gpui::{
    AnyElement, Context, Div, FontWeight, HighlightStyle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollStrategy, ScrollWheelEvent, Stateful, StyledText,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args, CommentSpot};
use crate::code_page::{Content, Piece, Shown, TreeRow, Viewer, styled};
use crate::health_page::{HealthState, HeatBand};
use crate::icon::{self, Icon};
use crate::resize::{self};
use crate::resize_handle::{self, Edge};
use crate::theme;
use crate::view::{Screen, StatusView};
use crate::window_chrome;

const TREE_ROW: f32 = 24.;
const INDENT: f32 = 14.;
const LINE_HEIGHT: f32 = 20.;
/// The width of a digit of the code font, in pixels, for the line numbers.
const DIGIT_WIDTH: f32 = 8.;
/// A wheel step with a sideways part moves the text this many columns.
const SCROLL_STEP: usize = 4;

/// The window's part of the Code page: the two scrolls and the sideways offset.
pub(crate) struct CodeView {
    pub(crate) tree_scroll: UniformListScrollHandle,
    pub(crate) lines_scroll: UniformListScrollHandle,
    /// The rows of the provenance panel (P4).
    pub(crate) provenance_scroll: UniformListScrollHandle,
    /// How many columns the text is scrolled to the right.
    pub(crate) columns: usize,
    /// The line brought into view by `file:line`, marked until another file opens.
    pub(crate) marked: Option<usize>,
    /// The tree is to scroll to the chosen file once its row exists: the
    /// folders above it may still be listing.
    pub(crate) reveal_scroll: bool,
    /// The lines picked to comment on, 0-based, as (where the press began, where
    /// it is now) (D141).
    pub(crate) picked: Option<(usize, usize)>,
    /// A press on a line number is held: moving over lines extends `picked`.
    pub(crate) picking: bool,
}

impl CodeView {
    pub(crate) fn new() -> Self {
        Self {
            tree_scroll: UniformListScrollHandle::new(),
            lines_scroll: UniformListScrollHandle::new(),
            provenance_scroll: UniformListScrollHandle::new(),
            columns: 0,
            marked: None,
            reveal_scroll: false,
            picked: None,
            picking: false,
        }
    }
}

impl StatusView {
    /// Opens the Code page. The file list is read again, so it is current, and
    /// the root of the tree is listed.
    pub(crate) fn show_code(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Code;
        self.engine.dispatch(Actor::Human, CommandKind::IndexFiles);
        self.request_health();
        if !self.model.code.has_root() {
            self.request_listing("");
        }
        cx.notify();
    }

    /// Asks the engine for the code health of HEAD (D225). While a read is in flight the request
    /// is remembered and asked again when the read ends (D229), since the read may answer an
    /// older HEAD. A read of the same HEAD and configuration is answered at once from the
    /// engine's cache.
    pub(crate) fn request_health(&mut self) {
        if self.model.health.pending() {
            self.model.health.want_again();
            return;
        }
        if self.model.status().is_none() {
            return;
        }
        let id = self
            .engine
            .dispatch(Actor::Human, CommandKind::LoadHealth { rev: None });
        self.model.health.requested(id);
    }

    /// Asks for the health again when a request came during the read that has just ended (D229).
    pub(crate) fn ask_health_again(&mut self) {
        if self.model.health.take_ask_again() {
            self.request_health();
        }
    }

    /// Opens the Code page on `path`, at `line` if given: the tree opens the
    /// folders above it, and the file is read.
    pub(crate) fn view_file(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        self.show_code(cx);
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ReadFile {
                path: path.to_owned(),
            },
        );
        self.model.code.file_requested(id, path, line);
        self.code_view.columns = 0;
        self.code_view.marked = None;
        self.code_view.picked = None;
        self.code_view
            .lines_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        for dir in self.model.code.reveal(path) {
            self.request_listing(&dir);
        }
        self.code_view.reveal_scroll = true;
        self.scroll_tree_to_selection();
        self.provenance_follows_pick();
        cx.notify();
    }

    /// Asks the engine for what folder `dir` holds.
    pub(crate) fn request_listing(&mut self, dir: &str) {
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ListDirectory {
                dir: dir.to_owned(),
            },
        );
        self.model.code.listing_requested(id, dir);
    }

    /// Opens or closes a folder of the tree, asking for its listing when it has none.
    fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.model.code.toggle(path) {
            self.request_listing(path);
        }
        cx.notify();
    }

    /// Reads a file for the viewer from a click in the tree.
    fn open_from_tree(&mut self, path: &str, cx: &mut Context<Self>) {
        let id = self.engine.dispatch(
            Actor::Human,
            CommandKind::ReadFile {
                path: path.to_owned(),
            },
        );
        self.model.code.file_requested(id, path, None);
        self.code_view.columns = 0;
        self.code_view.marked = None;
        self.code_view.picked = None;
        self.code_view
            .lines_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        self.provenance_follows_pick();
        cx.notify();
    }

    /// What follows an engine event for the Code page and Quick Open: listings
    /// the tree still needs, a file list that was read again, and the line a
    /// `file:line` named.
    pub(crate) fn after_file_event(&mut self, cx: &mut Context<Self>) {
        for dir in self.model.take_wanted_listings() {
            self.request_listing(&dir);
        }
        if self.model.take_files_indexed() {
            if self.model.quick_open.open {
                self.search_files();
            }
            if self.screen == Screen::Code {
                for dir in self.model.code.folders_to_refresh() {
                    self.request_listing(&dir);
                }
            }
        }
        if let Some(line) = self.model.code.take_goto() {
            self.code_view.marked = Some(line - 1);
            self.code_view
                .lines_scroll
                .scroll_to_item_strict(line.saturating_sub(4), ScrollStrategy::Top);
        }
        self.scroll_tree_to_selection();
        cx.notify();
    }

    /// Brings the row of the chosen file into view in the tree, once its row
    /// exists. A tree the user scrolls by hand is left alone.
    fn scroll_tree_to_selection(&mut self) {
        if !self.code_view.reveal_scroll {
            return;
        }
        let Some(selected) = self.model.code.selected.as_deref() else {
            return;
        };
        if let Some(at) = self
            .model
            .code
            .rows()
            .iter()
            .position(|row| row.path == selected)
        {
            self.code_view
                .tree_scroll
                .scroll_to_item(at, ScrollStrategy::Top);
            self.code_view.reveal_scroll = false;
        }
    }

    /// A wheel step with a sideways part moves the text.
    fn scroll_code_across(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dx = event.delta.pixel_delta(px(LINE_HEIGHT)).x;
        if dx == px(0.) {
            return;
        }
        let max = match self.model.code.on_screen().map(|shown| &shown.content) {
            Some(Content::Text(text)) => text.columns(),
            _ => 0,
        };
        self.code_view.columns = if dx < px(0.) {
            (self.code_view.columns + SCROLL_STEP).min(max)
        } else {
            self.code_view.columns.saturating_sub(SCROLL_STEP)
        };
        cx.notify();
    }

    /// The page for the main area: the tree, then the viewer.
    pub(crate) fn code_page(&self, cx: &mut Context<Self>) -> Div {
        // A click in the page takes the focus: GPUI runs the window's shortcuts only when the
        // focus is inside the app, and the file list's focus is not drawn on this page.
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_row()
            .bg(theme::editor())
            .track_focus(&self.code_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, _| this.code_focus.focus(window)),
            )
            .child(self.code_tree(cx))
            .child(self.code_viewer(cx))
            .children(self.provenance_panel(cx))
    }

    /// The tab strip of the Code page, the text search and the code health: one page with three
    /// tabs: Files, Search and Health. The tab shown is underlined, as the Threads page's tabs are.
    pub(crate) fn code_tabs(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .child(self.code_tab_button(
                Screen::Code,
                "Files",
                "code-tab-files",
                Action::ShowCode,
                cx,
            ))
            .child(self.code_tab_button(
                Screen::Search,
                "Search",
                "code-tab-search",
                Action::SearchText,
                cx,
            ))
            .child(self.code_tab_button(
                Screen::Health,
                "Health",
                "code-tab-health",
                Action::ShowCodeHealth,
                cx,
            ))
    }

    /// One tab of the strip: its name, underlined while its screen is shown. A click runs its
    /// action, which switches the screen.
    fn code_tab_button(
        &self,
        screen: Screen,
        label: &'static str,
        id: &'static str,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.screen == screen;
        div()
            .id(id)
            .cursor_pointer()
            .h(px(32.))
            .flex()
            .flex_row()
            .items_center()
            .border_b_2()
            .border_color(if active {
                theme::accent()
            } else {
                theme::panel()
            })
            .font_weight(if active {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            })
            .text_color(if active {
                theme::text()
            } else {
                theme::muted()
            })
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.run_action(action, &Args::None, window, cx);
                }),
            )
    }

    fn code_tree(&self, cx: &mut Context<Self>) -> Div {
        let code = &self.model.code;
        let summary = if code.indexing {
            "Reading the file list…".to_owned()
        } else if code.has_root() {
            crate::quick_open::file_count(code.files)
        } else {
            String::new()
        };
        let rows = code.rows();
        let width = self.code_tree_width;
        let width_strip = resize_handle::handle(
            "code-tree-resize",
            Edge::Right,
            width,
            resize::PANE,
            |this: &mut StatusView, width, _| this.code_tree_width = width,
            |this: &mut StatusView, _| this.code_tree_width = resize::CODE_TREE_DEFAULT,
            cx,
        );
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex_none()
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(self.code_tabs(cx))
                    .child(div().text_xs().text_color(theme::muted()).child(summary)),
            )
            .child(if rows.is_empty() {
                let text = if code.has_root() {
                    "No files."
                } else {
                    "Reading the file list…"
                };
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::muted())
                    .child(text)
                    .into_any_element()
            } else {
                uniform_list(
                    "code-tree",
                    rows.len(),
                    cx.processor(
                        |this: &mut StatusView,
                         range: Range<usize>,
                         _window: &mut Window,
                         cx: &mut Context<StatusView>| {
                            let rows = this.model.code.rows();
                            let selected = this.model.code.selected.clone();
                            let health = &this.model.health;
                            range
                                .filter_map(|index| {
                                    rows.get(index).map(|row| {
                                        let heat = heat_of(health, row);
                                        tree_row(
                                            row,
                                            index,
                                            selected.as_deref(),
                                            heat,
                                            health.of(),
                                            cx,
                                        )
                                    })
                                })
                                .collect()
                        },
                    ),
                )
                .track_scroll(self.code_view.tree_scroll.clone())
                .flex_1()
                .min_h(px(0.))
                .into_any_element()
            })
            .children(self.health_line())
            .child(width_strip)
    }

    /// The line about code health under the tree: while the history is read, when it is partial
    /// or when the read failed (D225). Nothing when there is nothing to say.
    fn health_line(&self) -> Option<Div> {
        let text = self.model.health.status_text()?;
        Some(
            div()
                .flex_none()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(theme::muted())
                .child(text),
        )
    }

    fn code_viewer(&self, cx: &mut Context<Self>) -> Div {
        let code = &self.model.code;
        let shown = code.on_screen();
        let header_path = match &code.viewer {
            Viewer::Loading { path, .. } | Viewer::Failed { path, .. } => Some(path.clone()),
            Viewer::Shown(shown) => Some(shown.path.clone()),
            Viewer::Empty => None,
        };
        let body: AnyElement = match (&code.viewer, shown) {
            (Viewer::Failed { message, .. }, _) => notice(message),
            (Viewer::Empty, _) => {
                notice("Choose a file in the tree, or press Ctrl/⌘+P to find one by name.")
            }
            (Viewer::Loading { .. }, None) => notice("Reading…"),
            (_, Some(shown)) => self.file_body(shown, cx),
            (Viewer::Shown(_), None) => notice(""),
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
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
                    .bg(theme::panel())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(theme::code_font())
                            .child(header_path.unwrap_or_default()),
                    )
                    .children(self.code_view.picked.map(|range| comment_button(range, cx)))
                    .children(shown.map(|shown| {
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(file_summary(shown))
                    })),
            )
            .child(body)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.code_view.picking = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.code_view.picking = false),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.open_code_menu(event.position, cx);
                }),
            )
    }

    /// The body of a file on screen: its lines, or one line saying why there are none.
    fn file_body(&self, shown: &Shown, cx: &mut Context<Self>) -> AnyElement {
        match &shown.content {
            Content::Text(text) if text.line_count() == 0 => notice("This file is empty."),
            Content::Text(text) => {
                let digits = text.line_count().to_string().len().max(3);
                let gutter = DIGIT_WIDTH * f32::from(u16::try_from(digits).unwrap_or(3)) + 24.;
                uniform_list(
                    "code-lines",
                    text.line_count(),
                    cx.processor(
                        move |this: &mut StatusView,
                              range: Range<usize>,
                              _window: &mut Window,
                              cx: &mut Context<StatusView>| {
                            let Some(shown) = this.model.code.on_screen() else {
                                return Vec::new();
                            };
                            let Content::Text(text) = &shown.content else {
                                return Vec::new();
                            };
                            let columns = this.code_view.columns;
                            let marked = this.code_view.marked;
                            let picked = this.code_view.picked;
                            range
                                .map(|index| {
                                    let pieces = styled(
                                        text.line(index),
                                        text.line_start(index),
                                        columns,
                                        &shown.spans,
                                    );
                                    let in_pick = picked.is_some_and(|(from, to)| {
                                        (from.min(to)..=from.max(to)).contains(&index)
                                    });
                                    code_line(
                                        index,
                                        &pieces,
                                        gutter,
                                        marked == Some(index) || in_pick,
                                        cx,
                                    )
                                })
                                .collect()
                        },
                    ),
                )
                .track_scroll(self.code_view.lines_scroll.clone())
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                    this.scroll_code_across(event, cx);
                }))
                .flex_1()
                .min_h(px(0.))
                .font_family(theme::code_font())
                .into_any_element()
            }
            Content::Binary => notice(&format!("Binary file, {}.", bytes_label(shown.size))),
            Content::Missing => notice("This file is not in the worktree now."),
            Content::Symlink(target) => notice(&format!("Symbolic link to {target}")),
            Content::Directory => notice("This is a folder."),
        }
    }
}

/// The 1-based lines of the file a pick covers, lowest first.
pub(crate) fn picked_lines((from, to): (usize, usize)) -> (u32, u32) {
    let line = |index: usize| u32::try_from(index + 1).unwrap_or(u32::MAX);
    (line(from.min(to)), line(from.max(to)))
}

/// The header button that comments on the lines picked in the viewer.
fn comment_button(range: (usize, usize), cx: &Context<StatusView>) -> Stateful<Div> {
    let (start, end) = picked_lines(range);
    div()
        .id("code-comment")
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
        .child(format!(
            "Comment on {}",
            crate::review::range_label(start, end)
        ))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.run_action(Action::CommentOnSelection, &Args::None, window, cx);
            }),
        )
}

impl StatusView {
    /// A press on line number `index`: starts a pick there, or with Shift extends the last.
    fn pick_code_start(&mut self, index: usize, shift: bool, cx: &mut Context<Self>) {
        self.code_view.picked = match self.code_view.picked {
            Some((from, _)) if shift => Some((from, index)),
            _ => Some((index, index)),
        };
        self.code_view.picking = true;
        self.provenance_follows_pick();
        cx.notify();
    }

    /// The pointer moved over line `index` with the press held.
    fn pick_code_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.code_view.picking {
            return;
        }
        if let Some((from, to)) = self.code_view.picked
            && to != index
        {
            self.code_view.picked = Some((from, index));
            self.provenance_follows_pick();
            cx.notify();
        }
    }

    /// The file and lines the Code viewer has picked, for a review comment (D141): the
    /// viewer shows the file on disk, so its line numbers are the working tree's.
    pub(crate) fn code_comment_spot(&self) -> CommentSpot<'_> {
        let (Some(range), Some(shown)) = (self.code_view.picked, self.model.code.on_screen())
        else {
            return CommentSpot::Nothing;
        };
        let (start, end) = picked_lines(range);
        CommentSpot::Lines {
            path: &shown.path,
            working: Some((start, end)),
        }
    }
}

/// A line of text on its own, for what has no lines to show.
fn notice(text: &str) -> AnyElement {
    div()
        .px_4()
        .py_3()
        .text_color(theme::muted())
        .child(text.to_owned())
        .into_any_element()
}

/// The size, and a note when only the start of the file is shown.
fn file_summary(shown: &Shown) -> String {
    let size = bytes_label(shown.size);
    if shown.truncated {
        format!("{size}, showing the first part")
    } else {
        size
    }
}

/// A size in bytes as a short label: `812 bytes`, `4.2 KB`, `1.5 MB`.
#[must_use]
pub(crate) fn bytes_label(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if size >= MB {
        format!("{}.{} MB", size / MB, size % MB * 10 / MB)
    } else if size >= KB {
        format!("{}.{} KB", size / KB, size % KB * 10 / KB)
    } else if size == 1 {
        "1 byte".to_owned()
    } else {
        format!("{size} bytes")
    }
}

/// The heat of a row: the file's band and rank, or for a folder the hottest file inside it.
fn heat_of(health: &HealthState, row: &TreeRow) -> Option<(HeatBand, u32)> {
    if row.is_dir {
        health.folder_band(&row.path)
    } else {
        health.file_band(&row.path)
    }
}

/// One row of the tree: indent, a marker for a folder, the name, and a heat dot when the row
/// is in the hottest tenth (`heat`, out of `of` files). A click opens a folder or reads a file.
fn tree_row(
    row: &TreeRow,
    index: usize,
    selected: Option<&str>,
    heat: Option<(HeatBand, u32)>,
    of: usize,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let path = row.path.clone();
    let is_dir = row.is_dir;
    let chosen = !is_dir && selected == Some(row.path.as_str());
    div()
        .id(("code-row", index))
        .h(px(TREE_ROW))
        .pl(px(
            8. + INDENT * f32::from(u16::try_from(row.depth).unwrap_or(0))
        ))
        .pr_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .when(chosen, |r| r.bg(theme::selected_row()))
        .hover(|style| style.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .text_xs()
                .text_color(theme::muted())
                .children(is_dir.then(|| {
                    icon::view(
                        if row.expanded {
                            Icon::CaretDown
                        } else {
                            Icon::CaretRight
                        },
                        icon::SMALL,
                        theme::muted(),
                    )
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .when(is_dir, |name| name.font_weight(gpui::FontWeight::MEDIUM))
                .child(row.name.clone()),
        )
        .children(heat.map(|(band, rank)| heat_dot(index, band, rank, of, is_dir)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                if is_dir {
                    this.toggle_folder(&path, cx);
                } else {
                    this.open_from_tree(&path, cx);
                }
            }),
        )
}

/// The heat dot after a row's name. Its tooltip gives the rank of the file, or of the hottest
/// file inside the folder.
fn heat_dot(index: usize, band: HeatBand, rank: u32, of: usize, is_dir: bool) -> Stateful<Div> {
    let text = if is_dir {
        format!("Hottest file inside: rank {rank} of {of}")
    } else {
        format!("Heat: rank {rank} of {of}")
    };
    div()
        .id(("code-heat", index))
        .flex_none()
        .ml_1()
        .w(px(6.))
        .h(px(6.))
        .rounded_full()
        .bg(theme::heat(band))
        .tooltip(window_chrome::hint(text))
}

/// The pieces of a line as one run of text with a color per piece that has a role.
pub(crate) fn colored(pieces: &[Piece]) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for piece in pieces {
        let start = text.len();
        text.push_str(&piece.text);
        if let Some(role) = piece.role {
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(theme::syntax(role)),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}

/// One line of the file: its number, then its text from the scrolled column, colored by role.
fn code_line(
    index: usize,
    pieces: &[Piece],
    gutter: f32,
    marked: bool,
    cx: &Context<StatusView>,
) -> AnyElement {
    div()
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                this.pick_code_to(index, cx);
            }
        }))
        .h(px(LINE_HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .when(marked, |row| row.bg(theme::selected_row()))
        .child(
            div()
                .flex_none()
                .w(px(gutter))
                .pr_3()
                .text_right()
                .text_xs()
                .text_color(theme::gutter())
                .cursor_pointer()
                .child((index + 1).to_string())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.pick_code_start(index, event.modifiers.shift, cx);
                    }),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(colored(pieces)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_is_numbered_from_one_whichever_way_it_was_dragged() {
        assert_eq!(picked_lines((6, 10)), (7, 11));
        assert_eq!(picked_lines((10, 6)), (7, 11));
        assert_eq!(picked_lines((0, 0)), (1, 1));
    }

    #[test]
    fn sizes_read_in_bytes_kilobytes_and_megabytes() {
        assert_eq!(bytes_label(0), "0 bytes");
        assert_eq!(bytes_label(1), "1 byte");
        assert_eq!(bytes_label(812), "812 bytes");
        assert_eq!(bytes_label(1024), "1.0 KB");
        assert_eq!(bytes_label(4300), "4.1 KB");
        assert_eq!(bytes_label(1536 * 1024), "1.5 MB");
    }
}
