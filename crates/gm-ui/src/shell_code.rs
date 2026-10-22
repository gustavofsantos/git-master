//! The Code page (design/code-navigation.md, N2): the folder tree on the left and a read-only
//! viewer of one file on the right. The tree and the viewer's state come from
//! `code_page.rs`; the engine lists a folder from the file list it holds in
//! memory and reads the file from disk. Every request goes out with an
//! `OperationId`, and an answer to a superseded one is dropped there.

use std::ops::Range;

use gm_schema::{Actor, CommandKind};
use gpui::{
    AnyElement, Context, Div, MouseButton, MouseDownEvent, ScrollStrategy, ScrollWheelEvent,
    Stateful, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::code_page::{Content, Shown, TreeRow, Viewer, visible};
use crate::theme;
use crate::view::{Screen, StatusView};

const TREE_WIDTH: f32 = 300.;
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
    /// How many columns the text is scrolled to the right.
    pub(crate) columns: usize,
    /// The line brought into view by `file:line`, marked until another file opens.
    pub(crate) marked: Option<usize>,
    /// The tree is to scroll to the chosen file once its row exists: the
    /// folders above it may still be listing.
    pub(crate) reveal_scroll: bool,
}

impl CodeView {
    pub(crate) fn new() -> Self {
        Self {
            tree_scroll: UniformListScrollHandle::new(),
            lines_scroll: UniformListScrollHandle::new(),
            columns: 0,
            marked: None,
            reveal_scroll: false,
        }
    }
}

impl StatusView {
    /// Opens the Code page. The file list is read again, so it is current, and
    /// the root of the tree is listed.
    pub(crate) fn show_code(&mut self, cx: &mut Context<Self>) {
        self.screen = Screen::Code;
        self.engine.dispatch(Actor::Human, CommandKind::IndexFiles);
        if !self.model.code.has_root() {
            self.request_listing("");
        }
        cx.notify();
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
        self.code_view
            .lines_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
        for dir in self.model.code.reveal(path) {
            self.request_listing(&dir);
        }
        self.code_view.reveal_scroll = true;
        self.scroll_tree_to_selection();
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
        self.code_view
            .lines_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
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
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_row()
            .bg(theme::editor())
            .child(self.code_tree(cx))
            .child(self.code_viewer(cx))
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
        div()
            .flex_none()
            .w(px(TREE_WIDTH))
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
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Files"))
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
                            range
                                .filter_map(|index| {
                                    rows.get(index)
                                        .map(|row| tree_row(row, index, selected.as_deref(), cx))
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
                    .children(shown.map(|shown| {
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(file_summary(shown))
                    })),
            )
            .child(body)
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
                              _cx: &mut Context<StatusView>| {
                            let Some(Content::Text(text)) =
                                this.model.code.on_screen().map(|s| &s.content)
                            else {
                                return Vec::new();
                            };
                            let columns = this.code_view.columns;
                            let marked = this.code_view.marked;
                            range
                                .map(|index| {
                                    code_line(
                                        index,
                                        &visible(text.line(index), columns),
                                        gutter,
                                        marked == Some(index),
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

/// One row of the tree: indent, a marker for a folder, the name. A click opens
/// a folder or reads a file.
fn tree_row(
    row: &TreeRow,
    index: usize,
    selected: Option<&str>,
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
                .child(if !is_dir {
                    ""
                } else if row.expanded {
                    "▾"
                } else {
                    "▸"
                }),
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

/// One line of the file: its number, then its text from the scrolled column.
fn code_line(index: usize, text: &str, gutter: f32, marked: bool) -> AnyElement {
    div()
        .h(px(LINE_HEIGHT))
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
                .child((index + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(text.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
