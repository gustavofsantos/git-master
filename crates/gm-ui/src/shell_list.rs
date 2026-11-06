//! The left column of the Working Copy window: the heading, the flat change
//! list and, at its bottom, the commit box.

use std::ops::Range;

use gm_schema::{Change, StatusEntry};
use gpui::{
    Context, Div, Hsla, MouseButton, MouseDownEvent, Stateful, Window, div, prelude::*, px,
    uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::icon::{self, Icon};
use crate::move_file;
use crate::resize::{self, LEFT};
use crate::resize_handle::{self, Edge};
use crate::submodule::{self, Submodules};
use crate::theme;
use crate::view::{KEY_CONTEXT, StatusView};
use crate::{CloseContextMenu, OpenContextMenu, Refresh, SelectNext, SelectPrev, ToggleStage};

const ROW_HEIGHT: f32 = 24.;

impl StatusView {
    /// The left column at `width`, with its commit box `commit_height` tall at
    /// its bottom. Its right edge is the strip that resizes the column.
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
            .child(self.change_list(cx))
            .child(self.commit_block(commit_height, cx))
            .child(width_strip)
    }

    /// "Working Copy", and the number of changed files muted beside it.
    fn heading(&self) -> Div {
        let count = self.model.entry_count();
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Working Copy"),
            )
            .child(
                div()
                    .text_color(theme::muted())
                    .child(format!("{count} changed")),
            )
    }

    /// The file list. It is always rendered and always holds the list focus,
    /// so the list keys have a place to go even when there is nothing to list.
    fn change_list(&self, cx: &Context<Self>) -> Div {
        let count = self.model.entry_count();
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.list_focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step_file(true, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step_file(false, cx)))
            .on_action(cx.listener(|this, _: &Refresh, window, cx| {
                this.run_action(Action::Refresh, &Args::None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleStage, window, cx| {
                this.toggle_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenContextMenu, window, cx| {
                this.open_selected_file_menu(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseContextMenu, _, cx| {
                this.close_context_menu(cx);
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
                            let submodules = &this.model.submodules;
                            entries
                                .get(range.clone())
                                .unwrap_or_default()
                                .iter()
                                .zip(range)
                                .map(|(entry, index)| {
                                    file_row(entry, selected == Some(index), index, submodules, cx)
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
            Some(_) => None,
        }
    }
}

/// One file: its status letter and its full path. A discreet dot after the
/// letter marks a file that has something staged. A submodule has the folder
/// icon instead of the letter, and its branch and marks after its path (D175).
fn file_row(
    entry: &StatusEntry,
    selected: bool,
    index: usize,
    submodules: &Submodules,
    cx: &Context<StatusView>,
) -> Stateful<Div> {
    let (letter, colour) = status_letter(entry);
    let label = match entry.orig_path.as_deref() {
        Some(orig) => format!("{}  ← {orig}", entry.path),
        None => entry.path.clone(),
    };
    let marks = entry
        .submodule
        .map(|change| submodule::marks(change, submodules.detail(&entry.path)).join(" · "));
    let is_submodule = entry.submodule.is_some();
    let path = entry.path.clone();
    let staged_mark =
        (entry.staged.is_some() && !entry.conflicted && !entry.untracked).then(|| {
            div()
                .id(("staged-mark", index))
                .size(px(5.))
                .flex_none()
                .rounded_full()
                .bg(theme::staged())
        });
    let row = div()
        .id(("file-row", index))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(ROW_HEIGHT))
        .px_3()
        .when(selected, |row| row.bg(theme::selected_row()))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.select_row(index, cx);
                // A double-click on a submodule opens it in its own window (D175).
                if event.click_count == 2 && is_submodule {
                    this.run_action(Action::OpenSubmodule, &Args::File(path.clone()), window, cx);
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.open_file_menu(index, event.position, window, cx);
            }),
        )
        .child(match entry.submodule {
            Some(_) => div()
                .w(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .child(icon::view(Icon::Folder, icon::SMALL, theme::muted()))
                .into_any_element(),
            None => div()
                .w(px(14.))
                .flex_none()
                .font_family(theme::code_font())
                .text_color(colour)
                .child(letter.to_string())
                .into_any_element(),
        })
        .child(div().w(px(6.)).flex_none().children(staged_mark))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(theme::code_font())
                .child(label),
        )
        .children(marks.map(|text| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_xs()
                .text_color(theme::muted())
                .child(text)
        }));
    // A changed file is dragged onto a sidebar row to move its change (D109).
    move_file::drag_source(row, &entry.path, cx)
}

/// The letter that stands for the file: `?` untracked, `U` conflicted, else the
/// unstaged change, else the staged one. Its colour is that side's.
fn status_letter(entry: &StatusEntry) -> (char, Hsla) {
    if entry.untracked {
        return ('?', theme::untracked());
    }
    if entry.conflicted {
        return ('U', theme::error());
    }
    match (entry.unstaged, entry.staged) {
        (Some(change), _) => (change_letter(change), theme::unstaged()),
        (None, Some(change)) => (change_letter(change), theme::staged()),
        (None, None) => (' ', theme::muted()),
    }
}

pub(crate) fn change_letter(change: Change) -> char {
    match change {
        Change::Added => 'A',
        Change::Modified => 'M',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::Copied => 'C',
        Change::TypeChanged => 'T',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: "a.txt".into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    #[test]
    fn the_letter_is_the_unstaged_change_first() {
        assert_eq!(
            status_letter(&entry(Some(Change::Added), Some(Change::Modified))).0,
            'M'
        );
        assert_eq!(status_letter(&entry(Some(Change::Added), None)).0, 'A');
        assert_eq!(status_letter(&entry(None, Some(Change::Deleted))).0, 'D');
    }

    #[test]
    fn every_change_kind_has_its_letter() {
        let letters: Vec<char> = [
            Change::Added,
            Change::Modified,
            Change::Deleted,
            Change::Renamed,
            Change::Copied,
            Change::TypeChanged,
        ]
        .into_iter()
        .map(change_letter)
        .collect();
        assert_eq!(letters, ['A', 'M', 'D', 'R', 'C', 'T']);
    }

    #[test]
    fn untracked_and_conflicted_have_their_own_letters() {
        let untracked = StatusEntry {
            untracked: true,
            ..entry(None, None)
        };
        let conflicted = StatusEntry {
            conflicted: true,
            ..entry(None, None)
        };
        assert_eq!(status_letter(&untracked).0, '?');
        assert_eq!(status_letter(&conflicted).0, 'U');
    }
}
