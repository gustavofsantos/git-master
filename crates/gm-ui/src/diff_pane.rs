//! The diff of the selected file, one side at a time (Staged or Unstaged), with
//! chunk actions in each hunk header and stage and unstage by line (D24). The
//! rows and the selection rules live in `diff_rows`; this file draws them and
//! maps keys and clicks onto them.

use std::ops::Range;

use gm_schema::{FileDiff, LineKind, StageTarget};
use gpui::{
    AnyElement, ClickEvent, Context, Div, EventEmitter, FocusHandle, FontWeight, IntoElement,
    KeyBinding, Render, ScrollStrategy, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::diff_rows::{
    Apply, Diffs, LineKey, Mode, Press, Row, Selection, Side, build_rows, change_counts,
    changed_keys_between, controls, default_side, hunk_header_text, hunk_selection_count, hunks_of,
    next_changed_line, next_hunk_header, press, prev_changed_line, prev_hunk_header, split_pairs,
};
use crate::icon::{self, Icon};
use crate::theme;

/// The key context the pane's bindings are scoped to.
const CONTEXT: &str = "DiffPane";

/// Height of every list row; `uniform_list` needs equal heights.
const ROW_HEIGHT: f32 = 22.;

const PAIR_WARNING: &str = "Half-staged −/+ pair: the index would lose both the old and the new line, and may not compile.";

actions!(
    diff_pane,
    [
        NextChangedLine,
        PrevChangedLine,
        NextHunk,
        PrevHunk,
        ToggleLine,
        StageHunkUnderCursor,
        StageSelectedLines,
        UnstageSelectedLines,
        ClearSelection,
        ToggleVisual,
    ]
);

/// What the pane asks the shell to do; the shell turns it into a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffPaneEvent {
    /// A chunk action from a hunk header, run through the action registry.
    Chunk {
        action: Action,
        args: Args,
    },
    /// Stage or unstage of selected lines (D24), sent to the engine as is.
    Stage(StageTarget),
    Unstage(StageTarget),
}

/// Visual mode: where it started, and the selection made before it.
struct Visual {
    anchor: usize,
    base: Selection,
}

pub struct DiffPane {
    focus: FocusHandle,
    /// What the pane is for; decides which controls it draws and accepts.
    mode: Mode,
    scroll: UniformListScrollHandle,
    unstaged: Option<FileDiff>,
    staged: Option<FileDiff>,
    /// The side shown: its hunks are the only ones in `rows`.
    side: Side,
    rows: Vec<Row>,
    selection: Selection,
    /// Index into `rows`.
    cursor: usize,
    visual: Option<Visual>,
    /// The apply key pressed once while its selection splits a pair.
    pending: Option<Apply>,
    /// The hunk whose discard waits for its confirmation.
    discard_armed: Option<usize>,
}

impl EventEmitter<DiffPaneEvent> for DiffPane {}

/// Both sides' hunks, borrowed from two optional diffs.
fn sides<'a>(unstaged: Option<&'a FileDiff>, staged: Option<&'a FileDiff>) -> Diffs<'a> {
    Diffs {
        unstaged: hunks_of(unstaged),
        staged: hunks_of(staged),
    }
}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            mode: Mode::Working,
            scroll: UniformListScrollHandle::new(),
            unstaged: None,
            staged: None,
            side: Side::Unstaged,
            rows: Vec::new(),
            selection: Selection::default(),
            cursor: 0,
            visual: None,
            pending: None,
            discard_armed: None,
        }
    }

    /// Replaces what is shown. `None` for a side means it has no diff (or is
    /// still loading). Selection state resets when the path changes and is kept
    /// when the same path is reloaded and a hunk id is still present. A new
    /// path opens on the side it has changes on; so does a reload whose
    /// current side has emptied.
    pub fn set_diffs(
        &mut self,
        unstaged: Option<FileDiff>,
        staged: Option<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        let new_path = unstaged
            .as_ref()
            .or(staged.as_ref())
            .map(|d| d.path.clone());
        let same_file = self.path() == new_path;
        let kept = if same_file {
            let old = sides(self.unstaged.as_ref(), self.staged.as_ref());
            let new = sides(unstaged.as_ref(), staged.as_ref());
            self.selection.retain_present(old, new)
        } else {
            Selection::default()
        };
        self.unstaged = unstaged;
        self.staged = staged;
        let current_empty = hunks_of(match self.side {
            Side::Unstaged => self.unstaged.as_ref(),
            Side::Staged => self.staged.as_ref(),
        })
        .is_empty();
        if !same_file || current_empty {
            self.side = default_side(
                hunks_of(self.unstaged.as_ref()),
                hunks_of(self.staged.as_ref()),
            );
        }
        self.rows = build_rows(self.diffs(), self.side);
        self.selection = kept;
        self.visual = None;
        self.pending = None;
        self.discard_armed = None;
        if same_file {
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        } else {
            self.cursor = next_changed_line(&self.rows, self.diffs(), 0).unwrap_or(0);
        }
        cx.notify();
    }

    /// Shows `side`. Selection, visual mode and a pending discard belong to the
    /// side they were made on, so they end here.
    fn select_side(&mut self, side: Side, cx: &mut Context<Self>) {
        if side == self.side {
            return;
        }
        self.side = side;
        self.rows = build_rows(self.diffs(), side);
        self.selection.clear();
        self.visual = None;
        self.pending = None;
        self.discard_armed = None;
        self.cursor = next_changed_line(&self.rows, self.diffs(), 0).unwrap_or(0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn diffs(&self) -> Diffs<'_> {
        sides(self.unstaged.as_ref(), self.staged.as_ref())
    }

    fn path(&self) -> Option<String> {
        self.unstaged
            .as_ref()
            .or(self.staged.as_ref())
            .map(|diff| diff.path.clone())
    }

    fn hunk_id(&self, side: Side, hunk_index: usize) -> Option<gm_schema::HunkId> {
        self.diffs()
            .hunk(side, hunk_index)
            .map(|hunk| hunk.id.clone())
    }

    /// Moves the cursor. In visual mode the selection becomes the base plus
    /// the changed lines between the anchor and the cursor.
    fn move_cursor(&mut self, row: usize, cx: &mut Context<Self>) {
        self.cursor = row;
        if let Some(visual) = &self.visual {
            let range = changed_keys_between(&self.rows, self.diffs(), visual.anchor, row);
            self.selection = Selection::with_range(&visual.base, range);
        }
        self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        cx.notify();
    }

    fn step_changed(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.pending = None;
        let target = if forward {
            next_changed_line(&self.rows, self.diffs(), self.cursor)
        } else {
            prev_changed_line(&self.rows, self.diffs(), self.cursor)
        };
        if let Some(row) = target {
            self.move_cursor(row, cx);
        }
    }

    fn step_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.pending = None;
        let target = if forward {
            next_hunk_header(&self.rows, self.cursor)
        } else {
            prev_hunk_header(&self.rows, self.cursor)
        };
        if let Some(row) = target {
            self.move_cursor(row, cx);
        }
    }

    /// Toggles one changed line. Leaves visual mode.
    fn toggle_key(&mut self, key: LineKey, cx: &mut Context<Self>) {
        if !controls(self.mode).line_selection {
            return;
        }
        self.pending = None;
        self.visual = None;
        let diffs = sides(self.unstaged.as_ref(), self.staged.as_ref());
        self.selection.toggle_line(diffs, key);
        cx.notify();
    }

    fn toggle_at_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(Row::Line {
            side,
            hunk_index,
            line_index,
        }) = self.rows.get(self.cursor).copied()
        {
            self.toggle_key((side, hunk_index, line_index), cx);
        } else {
            self.pending = None;
            self.visual = None;
            cx.notify();
        }
    }

    /// Emits a chunk action for one hunk of `side`.
    fn run_chunk(&mut self, action: Action, side: Side, hunk_index: usize, cx: &mut Context<Self>) {
        if !controls(self.mode).chunk_actions {
            return;
        }
        self.pending = None;
        let (Some(path), Some(hunk)) = (self.path(), self.hunk_id(side, hunk_index)) else {
            return;
        };
        let args = Args::Chunk {
            path,
            hunk,
            side: side.diff_side(),
        };
        cx.emit(DiffPaneEvent::Chunk { action, args });
    }

    /// Stage (unstaged side) or unstage (staged side) the hunk under the cursor.
    fn chunk_action_at_cursor(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        let Some((side, hunk_index)) = self.rows.get(self.cursor).map(|row| row.hunk()) else {
            cx.notify();
            return;
        };
        self.run_chunk(chunk_toggle(side), side, hunk_index, cx);
    }

    /// Stages or unstages the selected lines, one event per hunk, applying the
    /// two-press rule to a split pair.
    fn apply_lines(&mut self, action: Apply, cx: &mut Context<Self>) {
        if !controls(self.mode).line_selection {
            return;
        }
        let side = match action {
            Apply::Stage => Side::Unstaged,
            Apply::Unstage => Side::Staged,
        };
        let (splits, path, groups) = {
            let diffs = sides(self.unstaged.as_ref(), self.staged.as_ref());
            let splits = !split_pairs(diffs, &self.selection, side).is_empty();
            (
                splits,
                self.path(),
                self.selection.selected_lines_by_hunk(side),
            )
        };
        let (outcome, pending) = press(self.pending, action, splits);
        self.pending = pending;
        if let (Press::Apply, Some(path)) = (outcome, path) {
            {
                for (hunk_index, lines) in groups {
                    let Some(hunk) = self.hunk_id(side, hunk_index) else {
                        continue;
                    };
                    let target = StageTarget::Lines {
                        path: path.clone(),
                        hunk,
                        lines,
                    };
                    let event = match action {
                        Apply::Stage => DiffPaneEvent::Stage(target),
                        Apply::Unstage => DiffPaneEvent::Unstage(target),
                    };
                    cx.emit(event);
                }
            }
        }
        cx.notify();
    }

    /// Escape ends a pending discard or visual mode first, then clears the selection.
    fn escape(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        if self.discard_armed.take().is_none() && self.visual.take().is_none() {
            self.selection.clear();
        }
        cx.notify();
    }

    fn toggle_visual(&mut self, cx: &mut Context<Self>) {
        if !controls(self.mode).line_selection {
            return;
        }
        self.pending = None;
        if self.visual.take().is_none() {
            let base = self.selection.clone();
            let range = changed_keys_between(&self.rows, self.diffs(), self.cursor, self.cursor);
            self.selection = Selection::with_range(&base, range);
            self.visual = Some(Visual {
                anchor: self.cursor,
                base,
            });
        }
        cx.notify();
    }

    fn placeholder(&self) -> Option<&'static str> {
        if !self.rows.is_empty() {
            return None;
        }
        if self.path().is_none() {
            return Some("Select a file to see its diff.");
        }
        let binary = self.unstaged.as_ref().is_some_and(|d| d.binary)
            || self.staged.as_ref().is_some_and(|d| d.binary);
        Some(match (binary, self.side) {
            (true, _) => "Binary file (n/a)",
            (false, Side::Unstaged) => "No unstaged changes",
            (false, Side::Staged) => "No staged changes",
        })
    }

    /// The file's path and its `+ −` counts on the left, the side selector on
    /// the right, and under them a one-line summary of the side's chunks.
    fn file_header(&self, cx: &Context<Self>) -> Option<Div> {
        let path = self.path()?;
        let hunks = hunks_of(match self.side {
            Side::Unstaged => self.unstaged.as_ref(),
            Side::Staged => self.staged.as_ref(),
        });
        let (added, removed) = change_counts(hunks);
        let top = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .pt_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .min_w(px(0.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(theme::code_font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(path),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme::muted())
                            .child(format!("+{added} −{removed}")),
                    ),
            )
            .children(
                controls(self.mode)
                    .side_selector
                    .then(|| self.side_selector(cx)),
            );
        let summary = (!hunks.is_empty()).then(|| {
            div()
                .px_3()
                .pb_2()
                .text_color(theme::muted())
                .child(summary_text(hunks.len(), added, removed))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .bg(theme::panel())
                .border_b_1()
                .border_color(theme::hairline())
                .child(top)
                .children(summary),
        )
    }

    /// The two-way switch "Staged | Unstaged", styled like the sidebar's.
    fn side_selector(&self, cx: &Context<Self>) -> Div {
        let segment = |side: Side, label: &'static str| {
            let active = self.side == side;
            div()
                .id(label)
                .px_3()
                .h(px(24.))
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
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.select_side(side, cx);
                }))
        };
        div().flex_none().child(
            div()
                .p(px(2.))
                .flex()
                .flex_row()
                .rounded_md()
                .bg(theme::selected_row())
                .child(segment(Side::Staged, "Staged"))
                .child(segment(Side::Unstaged, "Unstaged")),
        )
    }

    /// The bar at the bottom of the pane while the selection splits a pair.
    fn warning_bar(&self) -> Option<Div> {
        let diffs = self.diffs();
        let splits = split_pairs(diffs, &self.selection, Side::Unstaged).len()
            + split_pairs(diffs, &self.selection, Side::Staged).len();
        if splits == 0 {
            return None;
        }
        let hint = self
            .pending
            .map(|_| "Press the same key again to apply anyway.");
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .bg(theme::warn_bg())
                .text_color(theme::warn_fg())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(icon::view(Icon::Warning, icon::SMALL, theme::warn_fg()))
                        .child(PAIR_WARNING),
                )
                .children(hint.map(|text| div().child(text))),
        )
    }

    /// A hunk's header row: its text, its line count, and on the right the chunk
    /// buttons. Discard asks first, inline in this row.
    fn hunk_row(
        &self,
        ix: usize,
        side: Side,
        hunk_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let diffs = self.diffs();
        let Some(hunk) = diffs.hunk(side, hunk_index) else {
            return div().into_any_element();
        };
        let (selected, total) = hunk_selection_count(diffs, &self.selection, side, hunk_index);
        let background = if ix == self.cursor {
            theme::cursor_row()
        } else {
            theme::hunk_header_bg()
        };
        let buttons = match self.discard_armed {
            Some(armed) if armed == hunk_index => div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_color(theme::removed_fg())
                        .child("Discard this chunk?"),
                )
                .child(
                    chunk_button(("chunk-confirm", ix), "Discard", true, theme::removed_fg())
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.discard_armed = None;
                            this.run_chunk(Action::DiscardChunk, Side::Unstaged, hunk_index, cx);
                        })),
                )
                .child(
                    chunk_button(("chunk-cancel", ix), "Cancel", true, theme::text()).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.discard_armed = None;
                            cx.notify();
                        }),
                    ),
                ),
            _ => {
                // Discard is for unstaged chunks only: on the staged side the
                // button is shown, muted, and does nothing.
                let discard_enabled = side == Side::Unstaged;
                let discard = chunk_button(
                    ("chunk-discard", ix),
                    "Discard Chunk",
                    discard_enabled,
                    if discard_enabled {
                        theme::text()
                    } else {
                        theme::muted()
                    },
                );
                let discard = if discard_enabled {
                    discard.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.discard_armed = Some(hunk_index);
                        cx.notify();
                    }))
                } else {
                    discard
                };
                let stage_label = chunk_label(side);
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(discard)
                    .child(
                        chunk_button(("chunk-stage", ix), stage_label, true, theme::text())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.run_chunk(chunk_toggle(side), side, hunk_index, cx);
                            })),
                    )
            }
        };
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .bg(background)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme::muted())
                    .child(hunk_header_text(hunk)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::muted())
                    .child(format!("{selected} of {total} lines")),
            )
            .children(controls(self.mode).chunk_actions.then_some(buttons))
            .into_any_element()
    }

    fn line_row(
        &self,
        ix: usize,
        side: Side,
        hunk_index: usize,
        line_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(line) = self
            .diffs()
            .hunk(side, hunk_index)
            .and_then(|hunk| hunk.lines.get(line_index))
        else {
            return div().into_any_element();
        };
        let (tint, text_colour, sign) = match line.kind {
            LineKind::Added => (theme::added_bg(), theme::added_fg(), "+"),
            LineKind::Removed => (theme::removed_bg(), theme::removed_fg(), "−"),
            LineKind::Context => (theme::editor(), theme::text(), " "),
        };
        let changed = line.kind != LineKind::Context;
        let selected = self.selection.contains((side, hunk_index, line_index));
        let background = if ix == self.cursor {
            theme::cursor_row()
        } else {
            tint
        };
        let number = |n: Option<u32>| {
            div()
                .w(px(44.))
                .flex_none()
                .text_right()
                .text_color(theme::gutter())
                .child(n.map_or_else(String::new, |n| n.to_string()))
        };
        let checkbox = if changed && controls(self.mode).line_selection {
            let mark = if selected {
                icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
            } else {
                icon::view(Icon::Square, icon::SMALL, theme::muted())
            };
            div()
                .id(("diff-check", ix))
                .mx_1()
                .flex_none()
                .cursor_pointer()
                .child(mark)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_key((side, hunk_index, line_index), cx);
                }))
                .into_any_element()
        } else {
            div().w(icon::SMALL).mx_1().flex_none().into_any_element()
        };
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .bg(background)
            .font_family(theme::code_font())
            .child(number(line.old_no))
            .child(number(line.new_no))
            .child(checkbox)
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .text_color(text_colour)
                    .child(sign),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(text_colour)
                    .child(line.text.clone()),
            )
            .into_any_element()
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(ix).copied() {
            Some(Row::HunkHeader { side, hunk_index }) => self.hunk_row(ix, side, hunk_index, cx),
            Some(Row::Line {
                side,
                hunk_index,
                line_index,
            }) => self.line_row(ix, side, hunk_index, line_index, cx),
            None => div().into_any_element(),
        }
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        uniform_list(
            "diff-rows",
            self.rows.len(),
            cx.processor(
                |this: &mut DiffPane,
                 range: Range<usize>,
                 _window: &mut Window,
                 cx: &mut Context<DiffPane>| {
                    range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(self.scroll.clone())
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }
}

/// The stage or unstage action for a chunk of `side`.
fn chunk_toggle(side: Side) -> Action {
    match side {
        Side::Unstaged => Action::StageChunk,
        Side::Staged => Action::UnstageChunk,
    }
}

fn chunk_label(side: Side) -> &'static str {
    match side {
        Side::Unstaged => "Stage Chunk",
        Side::Staged => "Unstage Chunk",
    }
}

/// "2 chunks, 3 insertions, 4 deletions", with the singular where it is one.
fn summary_text(chunks: usize, insertions: usize, deletions: usize) -> String {
    let noun = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    format!(
        "{}, {}, {}",
        noun(chunks, "chunk", "chunks"),
        noun(insertions, "insertion", "insertions"),
        noun(deletions, "deletion", "deletions"),
    )
}

/// A small outlined button for a chunk header.
fn chunk_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    enabled: bool,
    text: gpui::Hsla,
) -> gpui::Stateful<Div> {
    let button = div()
        .id(id)
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(theme::button_border())
        .text_color(text)
        .child(label);
    if enabled {
        button.cursor_pointer()
    } else {
        button
    }
}

/// Binds the pane's keys; called once at startup.
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys(key_bindings());
}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("j", NextChangedLine, Some(CONTEXT)),
        KeyBinding::new("down", NextChangedLine, Some(CONTEXT)),
        KeyBinding::new("k", PrevChangedLine, Some(CONTEXT)),
        KeyBinding::new("up", PrevChangedLine, Some(CONTEXT)),
        KeyBinding::new("n", NextHunk, Some(CONTEXT)),
        KeyBinding::new("p", PrevHunk, Some(CONTEXT)),
        KeyBinding::new("space", ToggleLine, Some(CONTEXT)),
        KeyBinding::new("a", StageHunkUnderCursor, Some(CONTEXT)),
        KeyBinding::new("s", StageSelectedLines, Some(CONTEXT)),
        KeyBinding::new("u", UnstageSelectedLines, Some(CONTEXT)),
        KeyBinding::new("escape", ClearSelection, Some(CONTEXT)),
        KeyBinding::new("V", ToggleVisual, Some(CONTEXT)),
    ]
}

impl Render for DiffPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.placeholder() {
            Some(message) => div()
                .flex_1()
                .px_3()
                .py_2()
                .text_color(theme::muted())
                .child(message)
                .into_any_element(),
            None => self.list(cx),
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NextChangedLine, _, cx| this.step_changed(true, cx)))
            .on_action(cx.listener(|this, _: &PrevChangedLine, _, cx| this.step_changed(false, cx)))
            .on_action(cx.listener(|this, _: &NextHunk, _, cx| this.step_hunk(true, cx)))
            .on_action(cx.listener(|this, _: &PrevHunk, _, cx| this.step_hunk(false, cx)))
            .on_action(cx.listener(|this, _: &ToggleLine, _, cx| this.toggle_at_cursor(cx)))
            .on_action(cx.listener(|this, _: &StageHunkUnderCursor, _, cx| {
                this.chunk_action_at_cursor(cx);
            }))
            .on_action(cx.listener(|this, _: &StageSelectedLines, _, cx| {
                this.apply_lines(Apply::Stage, cx);
            }))
            .on_action(cx.listener(|this, _: &UnstageSelectedLines, _, cx| {
                this.apply_lines(Apply::Unstage, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| this.escape(cx)))
            .on_action(cx.listener(|this, _: &ToggleVisual, _, cx| this.toggle_visual(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor())
            .text_color(theme::text())
            .font_family(theme::ui_font())
            .text_sm()
            .children(self.file_header(cx))
            .child(body)
            .children(self.warning_bar())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_binding_parses() {
        assert_eq!(key_bindings().len(), 12);
    }

    #[test]
    fn the_summary_counts_chunks_insertions_and_deletions_in_the_plural_and_singular() {
        assert_eq!(summary_text(2, 3, 4), "2 chunks, 3 insertions, 4 deletions");
        assert_eq!(summary_text(1, 1, 0), "1 chunk, 1 insertion, 0 deletions");
    }

    #[test]
    fn chunk_actions_follow_the_side() {
        assert_eq!(chunk_toggle(Side::Unstaged), Action::StageChunk);
        assert_eq!(chunk_toggle(Side::Staged), Action::UnstageChunk);
        assert_eq!(chunk_label(Side::Staged), "Unstage Chunk");
    }
}
