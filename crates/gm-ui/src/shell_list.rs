//! The left column of the Working Copy window: the heading, the filter, the
//! commit box and the change list.

use std::ops::Range;

use gm_schema::{Change, StatusEntry};
use gpui::{
    Context, Div, Hsla, MouseButton, MouseDownEvent, Pixels, Window, div, prelude::*, px,
    uniform_list,
};

use crate::changes::{Check, Row};
use crate::icon::{self, Icon};
use crate::model::entry_label;
use crate::resize::{self, LEFT};
use crate::resize_handle::{self, Edge};
use crate::theme;
use crate::view::{KEY_CONTEXT, StatusView};
use crate::{FocusFilter, Refresh, SelectNext, SelectPrev, ToggleStage};

const ROW_HEIGHT: f32 = 24.;

impl StatusView {
    /// The left column at `width`, with its commit box `commit_height` tall.
    /// Its right edge and the commit box's bottom edge are the two strips.
    pub(crate) fn left_column(
        &self,
        width: f32,
        commit_height: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let width_strip = resize_handle::handle(
            "left-column-resize",
            Edge::Right,
            width,
            LEFT,
            |this: &mut StatusView, width, _| this.left_width = width,
            |this: &mut StatusView, _| this.left_width = resize::LEFT_DEFAULT,
            cx,
        );
        div()
            .relative()
            .w(px(width))
            .flex_none()
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::hairline())
            .child(self.heading())
            .child(self.filter_line())
            .child(self.commit_block(commit_height, cx))
            .child(self.change_list(cx))
            .child(width_strip)
    }

    fn heading(&self) -> Div {
        let muted = theme::muted();
        let summary = self.model.summary();
        let files = self.model.entry_count();
        let noun = if files == 1 { "file" } else { "files" };
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Working Copy"),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(format!("{files} {noun} · {} staged", summary.staged)),
            )
    }

    fn filter_line(&self) -> Div {
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme::hairline())
            .child(icon::view(
                Icon::MagnifyingGlass,
                icon::SIZE,
                theme::muted(),
            ))
            .child(div().flex_1().min_w(px(0.)).child(self.filter.clone()))
    }

    /// The file list. It is always rendered and always holds the list focus,
    /// so the list keys have a place to go even when there is nothing to list.
    fn change_list(&self, cx: &Context<Self>) -> Div {
        let count = self.model.tree().row_count();
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.list_focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step_file(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step_file(false, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &ToggleStage, _, cx| this.toggle_selected(cx)))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.focus_filter(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window: &mut Window, _| {
                    this.list_focus.focus(window);
                }),
            )
            .child(match self.placeholder() {
                Some(text) => div()
                    .px_3()
                    .py_2()
                    .text_color(theme::muted())
                    .child(text)
                    .into_any_element(),
                None => uniform_list(
                    "change-rows",
                    count,
                    cx.processor(
                        |this: &mut StatusView,
                         range: Range<usize>,
                         _window: &mut Window,
                         cx: &mut Context<StatusView>| {
                            let selected = this.model.selected_row();
                            let entries = this.model.entries();
                            this.model
                                .tree()
                                .rows()
                                .get(range.clone())
                                .unwrap_or_default()
                                .iter()
                                .zip(range)
                                .map(|(row, index)| {
                                    row_element(row, entries, selected == Some(index), index, cx)
                                })
                                .collect()
                        },
                    ),
                )
                .track_scroll(self.scroll.clone())
                .flex_1()
                .min_h(px(0.))
                .into_any_element(),
            })
    }

    fn placeholder(&self) -> Option<&'static str> {
        match self.model.status() {
            None if self.model.loading => Some("Loading…"),
            None => Some("No repository is open."),
            Some(_) if self.model.entry_count() == 0 => Some("Working tree clean."),
            Some(_) if self.model.tree().row_count() == 0 => Some("No file matches the filter."),
            Some(_) => None,
        }
    }
}

/// One row of the list. A folder row toggles its check and, from its chevron,
/// its expansion; a file row selects on click and toggles from its checkbox.
fn row_element(
    row: &Row,
    entries: &[StatusEntry],
    selected: bool,
    index: usize,
    cx: &Context<StatusView>,
) -> Div {
    match row {
        Row::Dir {
            path,
            name,
            depth,
            expanded,
            check,
        } => {
            let folder = path.clone();
            let caret = if *expanded {
                Icon::CaretDown
            } else {
                Icon::CaretRight
            };
            let folder_icon = if *expanded {
                Icon::FolderOpen
            } else {
                Icon::Folder
            };
            let chevron = div()
                .flex_none()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.toggle_folder(&folder, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(icon::view(caret, icon::SIZE, theme::muted()));
            base_row(*depth)
                .child(chevron)
                .child(checkbox(*check, index, cx))
                .child(icon::view(folder_icon, icon::SIZE, theme::muted()))
                .child(div().min_w(px(0.)).child(name.clone()))
        }
        Row::File {
            entry_index,
            name,
            depth,
            check,
            ..
        } => {
            let entry = entries.get(*entry_index);
            let (staged, unstaged) = entry.map_or((' ', ' '), entry_label);
            let label = match entry.and_then(|entry| entry.orig_path.as_deref()) {
                Some(orig) => format!("{name}  ← {orig}"),
                None => name.clone(),
            };
            base_row(*depth)
                .when(selected, |row| row.bg(theme::selected_row()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.select_row(index, cx);
                    }),
                )
                .child(checkbox(*check, index, cx))
                .child(
                    div()
                        .w(px(14.))
                        .font_family(theme::code_font())
                        .text_color(letter_colour(staged, theme::staged()))
                        .child(staged.to_string()),
                )
                .child(
                    div()
                        .w(px(14.))
                        .font_family(theme::code_font())
                        .text_color(letter_colour(unstaged, theme::unstaged()))
                        .child(unstaged.to_string()),
                )
                .child(icon::view(file_icon(entry), icon::SIZE, theme::muted()))
                .child(div().min_w(px(0.)).child(label))
        }
    }
}

/// The file's kind: added (or untracked), deleted, or any other change.
fn file_icon(entry: Option<&StatusEntry>) -> Icon {
    let Some(entry) = entry else {
        return Icon::File;
    };
    if entry.untracked || entry.staged == Some(Change::Added) {
        Icon::FilePlus
    } else if entry.staged == Some(Change::Deleted) || entry.unstaged == Some(Change::Deleted) {
        Icon::FileMinus
    } else {
        Icon::File
    }
}

fn base_row(depth: usize) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(ROW_HEIGHT))
        .pl(indent(depth))
        .pr_3()
}

/// The tri-state checkbox: checked, mixed or unchecked. Clicking it stages or
/// unstages the row without selecting it.
fn checkbox(check: Check, index: usize, cx: &Context<StatusView>) -> Div {
    let (glyph, colour) = match check {
        Check::Checked => (Icon::CheckSquare, theme::accent()),
        Check::Mixed => (Icon::MinusSquare, theme::accent()),
        Check::Unchecked => (Icon::Square, theme::muted()),
    };
    let image = if check == Check::Checked {
        icon::filled(glyph, icon::SIZE, colour)
    } else {
        icon::view(glyph, icon::SIZE, colour)
    };
    div()
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.toggle_row(index, cx);
                cx.stop_propagation();
            }),
        )
        .child(image)
}

/// A letter's colour: its side's colour, or a marker colour for the special ones.
fn letter_colour(letter: char, side: Hsla) -> Hsla {
    match letter {
        '?' => theme::untracked(),
        'U' => theme::error(),
        ' ' => theme::muted(),
        _ => side,
    }
}

/// Indentation for a row at `depth`, in steps of 14 pixels.
fn indent(depth: usize) -> Pixels {
    let steps = u16::try_from(depth).unwrap_or(u16::MAX);
    px(8. + f32::from(steps) * 14.)
}
