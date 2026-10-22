//! The diff of the selected file, one side at a time (Staged or Unstaged), with
//! chunk actions in each hunk header and stage and unstage by line (D24). The
//! read-only pane (H4c) shows every file of a commit in one list instead. The
//! rows and the selection rules live in `diff_rows`; this file draws them and
//! maps keys and clicks onto them.

use std::ops::Range;

use gm_schema::{CommitFile, FileDiff, LineKind, StageTarget};
use gpui::{
    AnyElement, ClickEvent, Context, Div, EventEmitter, FocusHandle, FontWeight, Hsla, IntoElement,
    KeyBinding, Render, ScrollStrategy, ScrollWheelEvent, UniformListScrollHandle, Window, actions,
    div, prelude::*, px, uniform_list,
};

use crate::action_registry::{Action, Args};
use crate::diff_rows::{
    Apply, Cell, Diffs, Layout, LineKey, Mode, Press, Row, Selection, Side, SplitRow, SplitRows,
    StackFile, build_rows, build_stack, change_counts, changed_keys_between, concat_hunks,
    controls, default_side, file_at_row, file_notice, hunk_header_text, hunk_selection_count,
    hunks_of, max_scroll_columns, next_changed_line, next_hunk_header, no_newline_mark,
    notice_text, press, prev_changed_line, prev_hunk_header, scroll_padding, skip_columns,
    split_cell_on_cursor, split_pairs, split_rows, step_from_top,
};
use crate::icon::{self, Icon};
use crate::shell_history::{change_color, file_counts, path_label};
use crate::shell_list::change_letter;
use crate::theme;

/// The key context the pane's bindings are scoped to.
const CONTEXT: &str = "DiffPane";
/// The key context of the read-only pane of a commit (H4, H4c). Its keys are
/// listed in `COMMIT_KEYS`: no vim-style letters, and Up and Down scroll.
const CONTEXT_READ_ONLY: &str = "DiffPaneReadOnly";

/// One key of a diff pane: its keystroke, the action it runs (by name, see
/// `binding`) and what it does. `secondary` is what gpui reads as Cmd on macOS
/// and Ctrl elsewhere (`resolve_secondary`), so one table serves both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyRow {
    keys: &'static str,
    action: &'static str,
    /// Read by the tests; the bindings only need `keys` and `action`.
    #[cfg_attr(not(test), allow(dead_code))]
    what: &'static str,
}

/// The keys of the Working Copy diff (`CONTEXT`). No letter is bound on its
/// own: a letter key is Shift with the secondary modifier. Up and Down move the
/// cursor by changed line; Ctrl+Shift+N and B go to the next and the previous
/// hunk. The letters avoid the registry's shortcuts (⇧⌘R restack, ⇧⌘P push), the
/// text area's redo (⇧⌘Z), the palette and quit (⌘K, ⌘Q), and Ctrl+Shift+U, which
/// is the Unicode entry of `IBus` on GNOME: that key never reaches the app there.
const WORKING_KEYS: [KeyRow; 10] = [
    KeyRow {
        keys: "up",
        action: "PrevChangedLine",
        what: "Previous changed line",
    },
    KeyRow {
        keys: "down",
        action: "NextChangedLine",
        what: "Next changed line",
    },
    KeyRow {
        keys: "secondary-shift-n",
        action: "NextHunk",
        what: "Next hunk",
    },
    KeyRow {
        keys: "secondary-shift-b",
        action: "PrevHunk",
        what: "Previous hunk",
    },
    KeyRow {
        keys: "space",
        action: "ToggleLine",
        what: "Select or clear the line",
    },
    KeyRow {
        keys: "secondary-shift-v",
        action: "ToggleVisual",
        what: "Visual selection",
    },
    KeyRow {
        keys: "secondary-shift-a",
        action: "StageHunkUnderCursor",
        what: "Stage or unstage the chunk under the cursor",
    },
    KeyRow {
        keys: "secondary-shift-s",
        action: "StageSelectedLines",
        what: "Stage the selected lines",
    },
    KeyRow {
        keys: "secondary-shift-i",
        action: "UnstageSelectedLines",
        what: "Unstage the selected lines",
    },
    KeyRow {
        keys: "escape",
        action: "ClearSelection",
        what: "Clear the selection",
    },
];

/// The keys of the commit's stacked diff (H4c, H4c-keys), bound in the read-only
/// context only. Up and Down scroll a row; the file keys wrap around.
const COMMIT_KEYS: [KeyRow; 7] = [
    KeyRow {
        keys: "secondary-shift-n",
        action: "NextFile",
        what: "Next file",
    },
    KeyRow {
        keys: "secondary-shift-p",
        action: "PrevFile",
        what: "Previous file",
    },
    KeyRow {
        keys: "up",
        action: "ScrollLineUp",
        what: "Scroll up one row",
    },
    KeyRow {
        keys: "down",
        action: "ScrollLineDown",
        what: "Scroll down one row",
    },
    KeyRow {
        keys: "pageup",
        action: "ScrollPageUp",
        what: "Scroll up one page",
    },
    KeyRow {
        keys: "pagedown",
        action: "ScrollPageDown",
        what: "Scroll down one page",
    },
    KeyRow {
        keys: "escape",
        action: "LeaveCommit",
        what: "Back to the commit",
    },
];

/// The keystroke as gpui reads it on the given platform: `secondary` is Cmd on
/// macOS and Ctrl elsewhere (gpui 0.2.2, `Keystroke::parse`).
#[cfg(test)]
fn resolve_secondary(keys: &str, mac: bool) -> String {
    keys.replacen("secondary", if mac { "cmd" } else { "ctrl" }, 1)
}

/// The keystroke as the registry writes shortcuts on macOS: ⌥, ⇧ and ⌘ in that
/// order, then the key in capitals (⇧⌘R, ⌥⌘C).
#[cfg(test)]
fn mac_glyph(keys: &str) -> String {
    let (mut alt, mut shift, mut cmd) = ("", "", "");
    let mut key = String::new();
    for part in keys.split('-') {
        match part {
            "secondary" | "cmd" => cmd = "⌘",
            "shift" => shift = "⇧",
            "alt" => alt = "⌥",
            other => key = other.to_uppercase(),
        }
    }
    format!("{alt}{shift}{cmd}{key}")
}

/// Height of every list row; `uniform_list` needs equal heights.
const ROW_HEIGHT: f32 = 22.;

/// Columns the split view moves per wheel step, sideways (D41).
const SCROLL_STEP: usize = 4;

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
        PrevFile,
        NextFile,
        LeaveCommit,
        ScrollLineUp,
        ScrollLineDown,
        ScrollPageUp,
        ScrollPageDown,
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
    /// Escape in a read-only pane: back to the commit view.
    LeaveCommit,
    /// The user chose a layout in the header (D39); the shell saves it.
    LayoutChosen(Layout),
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
    /// How the rows are drawn (D39). The cursor and the selection always follow
    /// `rows`; `split` is the same hunks paired for the split view.
    layout: Layout,
    split: SplitRows,
    /// Columns the split cells are scrolled sideways by. Shared by both sides.
    scroll_columns: usize,
    selection: Selection,
    /// Index into `rows`.
    cursor: usize,
    visual: Option<Visual>,
    /// The apply key pressed once while its selection splits a pair.
    pending: Option<Apply>,
    /// The hunk whose discard waits for its confirmation.
    discard_armed: Option<usize>,
    /// A read-only pane's stack (H4c): the files in order, how many more the
    /// detail did not list, and the row each file starts at in each layout.
    stack_files: Vec<CommitFile>,
    stack_more: usize,
    unified_starts: Vec<usize>,
    split_starts: Vec<usize>,
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
        Self::with_mode(Mode::Working, cx)
    }

    /// A pane that only shows one file of a commit (H4).
    pub fn read_only(cx: &mut Context<Self>) -> Self {
        Self::with_mode(Mode::ReadOnly, cx)
    }

    fn with_mode(mode: Mode, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            mode,
            scroll: UniformListScrollHandle::new(),
            unstaged: None,
            staged: None,
            side: Side::Unstaged,
            rows: Vec::new(),
            layout: Layout::Unified,
            split: SplitRows::default(),
            scroll_columns: 0,
            selection: Selection::default(),
            cursor: 0,
            visual: None,
            pending: None,
            discard_armed: None,
            stack_files: Vec::new(),
            stack_more: 0,
            unified_starts: Vec::new(),
            split_starts: Vec::new(),
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
        self.rebuild_rows();
        self.selection = kept;
        self.visual = None;
        self.pending = None;
        self.discard_armed = None;
        if !same_file {
            self.scroll_columns = 0;
        }
        if same_file {
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        } else {
            self.cursor = next_changed_line(&self.rows, self.diffs(), 0).unwrap_or(0);
        }
        cx.notify();
    }

    /// Shows every file of a commit (H4c) in one list, the first `files.len()`
    /// of them, and scrolls it so that file `focus` is at the top. `diffs` has
    /// one diff per file, in order. Nothing is selected and there is no cursor.
    pub fn show_stack(
        &mut self,
        files: Vec<CommitFile>,
        more: usize,
        diffs: Vec<FileDiff>,
        focus: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (hunks, ranges) = concat_hunks(diffs);
        let spec: Vec<StackFile> = files
            .iter()
            .zip(ranges)
            .map(|(file, hunks)| {
                let renamed = file.old_path.is_some();
                let notice = file_notice(file.change, renamed, file.binary, !hunks.is_empty());
                StackFile { hunks, notice }
            })
            .collect();
        let stacked = build_stack(&hunks, &spec, more);
        self.stack_files = files;
        self.stack_more = more;
        self.unified_starts = stacked.unified_starts;
        self.split_starts = stacked.split_starts;
        self.unstaged = Some(FileDiff {
            path: String::new(),
            old_path: None,
            binary: false,
            hunks,
        });
        self.staged = None;
        self.side = Side::Unstaged;
        self.rows = stacked.rows;
        self.split = SplitRows {
            rows: stacked.split,
            of_unified: Vec::new(),
        };
        self.scroll_columns = 0;
        self.selection.clear();
        self.visual = None;
        self.pending = None;
        self.discard_armed = None;
        self.cursor = 0;
        self.scroll_to_file(focus);
        // The list measures its rows on the frame after its rows change, and a
        // scroll asked for on the first frame can be lost. Asking again on the
        // next frame puts the file at the top.
        let pane = cx.entity();
        window.on_next_frame(move |_, cx| {
            pane.update(cx, |pane, cx| {
                pane.scroll_to_file(focus);
                cx.notify();
            });
        });
        cx.notify();
    }

    /// The row each file starts at in `layout`, in a stack.
    fn starts_for(&self, layout: Layout) -> &[usize] {
        match layout {
            Layout::Unified => &self.unified_starts,
            Layout::Split => &self.split_starts,
        }
    }

    /// The row at the top of the list. A pending scroll counts as done: its row is
    /// where the list will be. The offset is read, not the child bounds, which a
    /// uniform list does not keep.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn top_row(&self) -> usize {
        let state = self.scroll.0.borrow();
        if let Some(deferred) = state.deferred_scroll_to_item.as_ref() {
            return deferred.item_index;
        }
        let rows = -f32::from(state.base_handle.offset().y) / ROW_HEIGHT;
        rows.max(0.).floor() as usize
    }

    /// Scrolls a stacked pane so that file `file` is at the top. Strict: a header
    /// already in view still moves to the top, which a plain scroll does not do.
    fn scroll_to_file(&self, file: usize) {
        if let Some(&start) = self.starts_for(self.layout).get(file) {
            self.scroll
                .scroll_to_item_strict(start, ScrollStrategy::Top);
        }
    }

    /// Next or previous file of a stack (ctrl-shift-n, ctrl-shift-p). Wraps from
    /// the last file to the first and back; see `diff_rows::step_from_top`.
    fn step_stack_file(&self, forward: bool, cx: &mut Context<Self>) {
        if self.mode != Mode::ReadOnly {
            return;
        }
        if let Some(file) = step_from_top(self.starts_for(self.layout), self.top_row(), forward) {
            self.scroll_to_file(file);
            cx.notify();
        }
    }

    /// Scrolls a stack by `rows` rows, clamped to the list. Up and Down move one
    /// row, `PageUp` and `PageDown` a page of visible rows.
    fn scroll_rows(&self, rows: isize, cx: &mut Context<Self>) {
        if self.mode != Mode::ReadOnly {
            return;
        }
        let last = self.row_count().saturating_sub(1);
        let target = self.top_row().saturating_add_signed(rows).min(last);
        self.scroll
            .scroll_to_item_strict(target, ScrollStrategy::Top);
        cx.notify();
    }

    /// Blank rows after the last row of a stacked list, so that the last file can
    /// be scrolled to the top of the page (see `diff_rows::scroll_padding`). The
    /// page's height is the window's: a taller guess only adds blank rows.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn padding_rows(&self, window: &Window) -> usize {
        if self.mode != Mode::ReadOnly {
            return 0;
        }
        let Some(&last) = self.starts_for(self.layout).last() else {
            return 0;
        };
        let visible = (f32::from(window.viewport_size().height) / ROW_HEIGHT).floor() as usize;
        scroll_padding(self.row_count().saturating_sub(last), visible)
    }

    /// The number of rows a page of the list shows, at least one.
    fn page_rows(&self) -> isize {
        let height = self.scroll.0.borrow().base_handle.bounds().size.height;
        let rows = (f32::from(height) / ROW_HEIGHT).floor();
        #[allow(clippy::cast_possible_truncation)]
        let rows = rows as isize;
        rows.max(1)
    }

    /// Shows `side`. Selection, visual mode and a pending discard belong to the
    /// side they were made on, so they end here.
    fn select_side(&mut self, side: Side, cx: &mut Context<Self>) {
        if side == self.side {
            return;
        }
        self.side = side;
        self.rebuild_rows();
        self.scroll_columns = 0;
        self.selection.clear();
        self.visual = None;
        self.pending = None;
        self.discard_armed = None;
        self.cursor = next_changed_line(&self.rows, self.diffs(), 0).unwrap_or(0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Rebuilds the unified rows and their split pairing for the shown side.
    fn rebuild_rows(&mut self) {
        self.rows = build_rows(self.diffs(), self.side);
        self.split = split_rows(self.diffs(), self.side);
    }

    /// The row the cursor is on, as an index into the list being drawn. The
    /// cursor walks the unified rows, so in the split view it is mapped to the
    /// split row that holds it.
    fn cursor_ix(&self) -> usize {
        match self.layout {
            Layout::Unified => self.cursor,
            Layout::Split => self.split.of_unified.get(self.cursor).copied().unwrap_or(0),
        }
    }

    /// The line the cursor is on, or `None` on a hunk header.
    fn cursor_line(&self) -> Option<LineKey> {
        match self.rows.get(self.cursor).copied() {
            Some(Row::Line {
                side,
                hunk_index,
                line_index,
            }) => Some((side, hunk_index, line_index)),
            _ => None,
        }
    }

    fn row_count(&self) -> usize {
        match self.layout {
            Layout::Unified => self.rows.len(),
            Layout::Split => self.split.rows.len(),
        }
    }

    /// Shows the layout `layout` (D39) without saying so to the shell: used
    /// when the saved choice is loaded.
    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if layout == self.layout {
            return;
        }
        if self.mode == Mode::ReadOnly {
            // The stack keeps the file at the top: its header is where it starts in the new layout.
            let file = file_at_row(self.starts_for(self.layout), self.top_row());
            self.layout = layout;
            self.scroll_to_file(file);
        } else {
            self.layout = layout;
            self.scroll
                .scroll_to_item(self.cursor_ix(), ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// The user picked a layout in the header: shows it and tells the shell,
    /// which saves the choice.
    fn choose_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if !controls(self.mode).layout_selector || layout == self.layout {
            return;
        }
        self.set_layout(layout, cx);
        cx.emit(DiffPaneEvent::LayoutChosen(layout));
    }

    /// A wheel step with a sideways part moves the split cells (D41). The
    /// vertical part is left to the list.
    fn scroll_across(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.layout != Layout::Split {
            return;
        }
        let dx = event.delta.pixel_delta(px(ROW_HEIGHT)).x;
        if dx == px(0.) {
            return;
        }
        let max = max_scroll_columns(self.diffs(), self.side);
        self.scroll_columns = if dx < px(0.) {
            (self.scroll_columns + SCROLL_STEP).min(max)
        } else {
            self.scroll_columns.saturating_sub(SCROLL_STEP)
        };
        cx.notify();
    }

    /// The "Unified | Split" switch in the file header (D39).
    fn layout_selector(&self, cx: &Context<Self>) -> Div {
        let segment = |layout: Layout, label: &'static str| {
            let active = self.layout == layout;
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
                    this.choose_layout(layout, cx);
                }))
        };
        div().flex_none().child(
            div()
                .p(px(2.))
                .flex()
                .flex_row()
                .rounded_md()
                .bg(theme::selected_row())
                .child(segment(Layout::Unified, "Unified"))
                .child(segment(Layout::Split, "Split")),
        )
    }

    /// One row of the split list: a hunk header, or a pair of cells.
    fn render_ix(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        if ix >= self.row_count() {
            // Padding past the last row (see `padding_rows`): blank, one row high.
            return div().h(px(ROW_HEIGHT)).w_full().into_any_element();
        }
        match self.layout {
            Layout::Unified => self.render_row(ix, cx),
            Layout::Split => match self.split.rows.get(ix).copied() {
                Some(SplitRow::HunkHeader { side, hunk_index }) => {
                    self.hunk_row(ix, side, hunk_index, cx)
                }
                Some(SplitRow::Pair { old, new }) => self.pair_row(ix, old, new, cx),
                Some(SplitRow::Banner(row)) => self.banner_row(row),
                None => div().into_any_element(),
            },
        }
    }

    /// A split row: the old side's cell, a divider, the new side's cell. A side
    /// with no line is a plain panel.
    fn pair_row(
        &self,
        ix: usize,
        old: Option<Cell>,
        new: Option<Cell>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Only the cell of the cursor's own line is on the cursor (a −/+ pair has two).
        let cursor = self.cursor_line();
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .child(self.split_cell(ix, 0, old, split_cell_on_cursor(old, cursor), cx))
            .child(self.split_cell(ix, 1, new, split_cell_on_cursor(new, cursor), cx))
            .into_any_element()
    }

    /// One cell of a split row. `which` is 0 for the old side and 1 for the new.
    /// The gutter toggles this cell's own line (D41); the text is shifted by the
    /// shared scroll and cut at the cell's edge, with no wrap.
    fn split_cell(
        &self,
        ix: usize,
        which: usize,
        cell: Option<Cell>,
        on_cursor: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let base = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .font_family(theme::code_font());
        let base = if which == 0 {
            base.border_r_1().border_color(theme::hairline())
        } else {
            base
        };
        let Some(cell) = cell else {
            return base.bg(theme::panel());
        };
        let (side, hunk_index, line_index) = cell.key;
        let Some(line) = self
            .diffs()
            .hunk(side, hunk_index)
            .and_then(|hunk| hunk.lines.get(line_index))
        else {
            return base;
        };
        let (tint, text_colour, sign) = match line.kind {
            LineKind::Added => (theme::added_bg(), theme::added_fg(), "+"),
            LineKind::Removed => (theme::removed_bg(), theme::removed_fg(), "−"),
            LineKind::Context => (theme::editor(), theme::text(), " "),
        };
        let changed = line.kind != LineKind::Context;
        let selected = self.selection.contains(cell.key);
        let background = if on_cursor { theme::cursor_row() } else { tint };
        let number = if which == 0 { line.old_no } else { line.new_no };
        let checkbox = if changed && controls(self.mode).line_selection {
            let mark = if selected {
                icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
            } else {
                icon::view(Icon::Square, icon::SMALL, theme::muted())
            };
            div()
                .id(("diff-split-check", ix * 2 + which))
                .mx_1()
                .flex_none()
                .cursor_pointer()
                .child(mark)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_key(cell.key, cx);
                }))
                .into_any_element()
        } else {
            div().w(icon::SMALL).mx_1().flex_none().into_any_element()
        };
        base.bg(background)
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .text_right()
                    .text_color(theme::gutter())
                    .child(number.map_or_else(String::new, |n| n.to_string())),
            )
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
                    .child(code_with_mark(
                        skip_columns(&line.text, self.scroll_columns).to_owned(),
                        text_colour,
                        line.no_newline,
                    )),
            )
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
        self.scroll
            .scroll_to_item(self.cursor_ix(), ScrollStrategy::Center);
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
        let Some((side, hunk_index)) = self.rows.get(self.cursor).and_then(|row| row.hunk()) else {
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

    /// Escape in a read-only pane goes back to the commit view.
    fn leave_commit(&self, cx: &mut Context<Self>) {
        if self.mode == Mode::ReadOnly {
            cx.emit(DiffPaneEvent::LeaveCommit);
        }
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
        // A stack has rows whenever it lists a file, so no rows means no files.
        if self.mode == Mode::ReadOnly {
            return Some("This commit changes no files.");
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .children(
                        controls(self.mode)
                            .side_selector
                            .then(|| self.side_selector(cx)),
                    )
                    .children(
                        controls(self.mode)
                            .layout_selector
                            .then(|| self.layout_selector(cx)),
                    ),
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
                        // A flexible box lets the text wrap inside the pane's width.
                        .child(div().flex_1().min_w(px(0.)).child(PAIR_WARNING)),
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
        let background = if ix == self.cursor_ix() {
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
            .children(controls(self.mode).line_selection.then(|| {
                div()
                    .flex_none()
                    .text_color(theme::muted())
                    .child(format!("{selected} of {total} lines"))
            }))
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
        let background = if ix == self.cursor_ix() {
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
                    .child(code_with_mark(
                        line.text.clone(),
                        text_colour,
                        line.no_newline,
                    )),
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
            Some(row @ (Row::FileHeader { .. } | Row::Notice { .. } | Row::MoreFiles { .. })) => {
                self.banner_row(row)
            }
            None => div().into_any_element(),
        }
    }

    /// A row of a stack that is not a diff line (H4c): a file's header, a notice,
    /// or the count of files past the limit. Each is one list row high.
    fn banner_row(&self, row: Row) -> AnyElement {
        match row {
            Row::FileHeader { file } => self.file_row(file),
            Row::Notice { notice, .. } => text_row(&notice_text(notice), theme::muted()),
            Row::MoreFiles { count } => text_row(
                &format!("and {count} more files, not shown."),
                theme::muted(),
            ),
            Row::HunkHeader { .. } | Row::Line { .. } => div().into_any_element(),
        }
    }

    /// A file's header in a stack: its change letter, path (old → new for a
    /// rename) and `+ −` counts.
    fn file_row(&self, file: usize) -> AnyElement {
        let Some(meta) = self.stack_files.get(file) else {
            return div().into_any_element();
        };
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .text_center()
                    .font_family(theme::code_font())
                    .text_color(change_color(meta.change))
                    .child(change_letter(meta.change).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(theme::code_font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(path_label(meta)),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme::code_font())
                    .text_color(theme::muted())
                    .child(file_counts(meta)),
            )
            .into_any_element()
    }

    /// The header above a stack: how many files it lists, and the layout switch.
    fn stack_header(&self, cx: &Context<Self>) -> Div {
        let count = self.stack_files.len();
        let label = match (count, self.stack_more > 0) {
            (1, false) => "1 file".to_owned(),
            (count, false) => format!("{count} files"),
            (count, true) => format!("First {count} files"),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .py_2()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::hairline())
            .child(div().text_color(theme::muted()).child(label))
            .child(self.layout_selector(cx))
    }

    fn list(&self, cx: &mut Context<Self>, padding: usize) -> AnyElement {
        uniform_list(
            "diff-rows",
            self.row_count() + padding,
            cx.processor(
                |this: &mut DiffPane,
                 range: Range<usize>,
                 _window: &mut Window,
                 cx: &mut Context<DiffPane>| {
                    range.map(|ix| this.render_ix(ix, cx)).collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(self.scroll.clone())
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
            this.scroll_across(event, cx);
        }))
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }
}

/// A list row of plain text, one row high.
fn text_row(text: &str, colour: Hsla) -> AnyElement {
    div()
        .h(px(ROW_HEIGHT))
        .w_full()
        .px_3()
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(colour)
        .child(text.to_owned())
        .into_any_element()
}

/// A line's text, then git's no-newline mark after it in muted colour when the
/// line has no newline at the end of its file. The text is not cut short by the
/// mark: both are kept on one line and the cell clips the end.
fn code_with_mark(text: String, colour: Hsla, no_newline: bool) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(div().flex_none().text_color(colour).child(text))
        .when(no_newline, |row| {
            row.child(
                div()
                    .flex_none()
                    .ml_2()
                    .text_color(theme::muted())
                    .child(no_newline_mark(true)),
            )
        })
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
    cx.bind_keys(read_only_key_bindings());
}

/// The bindings of the Working Copy diff, from `WORKING_KEYS`.
fn key_bindings() -> Vec<KeyBinding> {
    table_bindings(&WORKING_KEYS, CONTEXT)
}

/// The bindings of a read-only pane, from `COMMIT_KEYS`. Only navigation is
/// bound: no key here reaches a stage, unstage or discard action, because none of
/// them is bound in this context.
fn read_only_key_bindings() -> Vec<KeyBinding> {
    table_bindings(&COMMIT_KEYS, CONTEXT_READ_ONLY)
}

fn table_bindings(table: &[KeyRow], context: &'static str) -> Vec<KeyBinding> {
    table
        .iter()
        .map(|row| binding(row.action, row.keys, context))
        .collect()
}

/// One binding: the action named in a table row, on its keystroke, in `context`.
fn binding(action: &str, keys: &str, context: &'static str) -> KeyBinding {
    let context = Some(context);
    match action {
        "PrevChangedLine" => KeyBinding::new(keys, PrevChangedLine, context),
        "NextChangedLine" => KeyBinding::new(keys, NextChangedLine, context),
        "NextHunk" => KeyBinding::new(keys, NextHunk, context),
        "PrevHunk" => KeyBinding::new(keys, PrevHunk, context),
        "ToggleLine" => KeyBinding::new(keys, ToggleLine, context),
        "ToggleVisual" => KeyBinding::new(keys, ToggleVisual, context),
        "StageHunkUnderCursor" => KeyBinding::new(keys, StageHunkUnderCursor, context),
        "StageSelectedLines" => KeyBinding::new(keys, StageSelectedLines, context),
        "UnstageSelectedLines" => KeyBinding::new(keys, UnstageSelectedLines, context),
        "ClearSelection" => KeyBinding::new(keys, ClearSelection, context),
        "NextFile" => KeyBinding::new(keys, NextFile, context),
        "PrevFile" => KeyBinding::new(keys, PrevFile, context),
        "ScrollLineUp" => KeyBinding::new(keys, ScrollLineUp, context),
        "ScrollLineDown" => KeyBinding::new(keys, ScrollLineDown, context),
        "ScrollPageUp" => KeyBinding::new(keys, ScrollPageUp, context),
        "ScrollPageDown" => KeyBinding::new(keys, ScrollPageDown, context),
        "LeaveCommit" => KeyBinding::new(keys, LeaveCommit, context),
        other => panic!("no diff pane action named {other}"),
    }
}

impl Render for DiffPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.placeholder() {
            Some(message) => div()
                .flex_1()
                .px_3()
                .py_2()
                .text_color(theme::muted())
                .child(message)
                .into_any_element(),
            None => self.list(cx, self.padding_rows(window)),
        };
        let context = match self.mode {
            Mode::Working => CONTEXT,
            Mode::ReadOnly => CONTEXT_READ_ONLY,
        };
        div()
            .key_context(context)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PrevFile, _, cx| this.step_stack_file(false, cx)))
            .on_action(cx.listener(|this, _: &NextFile, _, cx| this.step_stack_file(true, cx)))
            .on_action(cx.listener(|this, _: &ScrollLineUp, _, cx| this.scroll_rows(-1, cx)))
            .on_action(cx.listener(|this, _: &ScrollLineDown, _, cx| this.scroll_rows(1, cx)))
            .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| {
                let page = this.page_rows();
                this.scroll_rows(-page, cx);
            }))
            .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| {
                let page = this.page_rows();
                this.scroll_rows(page, cx);
            }))
            .on_action(cx.listener(|this, _: &LeaveCommit, _, cx| this.leave_commit(cx)))
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
            .children(if self.mode == Mode::ReadOnly {
                Some(self.stack_header(cx))
            } else {
                self.file_header(cx)
            })
            .child(body)
            .children(self.warning_bar())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_binding_parses_and_is_bound_once() {
        assert_eq!(key_bindings().len(), WORKING_KEYS.len());
        assert_eq!(read_only_key_bindings().len(), COMMIT_KEYS.len());
        for binding in key_bindings().iter().chain(read_only_key_bindings().iter()) {
            assert_eq!(binding.keystrokes().len(), 1);
        }
        for table in [&WORKING_KEYS[..], &COMMIT_KEYS[..]] {
            let mut seen: Vec<&str> = table.iter().map(|row| row.keys).collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), table.len(), "a keystroke is listed twice");
        }
    }

    #[test]
    fn no_bare_letter_is_bound_in_a_diff_pane() {
        // A letter on its own would be typed by the user, or fire on one keystroke.
        for row in WORKING_KEYS.iter().chain(COMMIT_KEYS.iter()) {
            let letter = row.keys.rsplit('-').next().unwrap_or(row.keys);
            let is_letter = letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic());
            assert!(
                !is_letter || row.keys.starts_with("secondary-shift-"),
                "{} is a letter without Ctrl or Cmd and Shift",
                row.keys
            );
        }
    }

    #[test]
    fn the_secondary_modifier_is_cmd_on_macos_and_ctrl_elsewhere() {
        assert_eq!(resolve_secondary("secondary-shift-n", true), "cmd-shift-n");
        assert_eq!(
            resolve_secondary("secondary-shift-n", false),
            "ctrl-shift-n"
        );
        assert_eq!(resolve_secondary("pagedown", true), "pagedown");
        for mac in [true, false] {
            for row in WORKING_KEYS.iter().chain(COMMIT_KEYS.iter()) {
                assert!(!resolve_secondary(row.keys, mac).contains("secondary"));
            }
        }
    }

    #[test]
    fn the_keys_of_each_context_are_unique_on_both_platforms() {
        for mac in [true, false] {
            for table in [&WORKING_KEYS[..], &COMMIT_KEYS[..]] {
                let mut seen: Vec<String> = table
                    .iter()
                    .map(|row| resolve_secondary(row.keys, mac))
                    .collect();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), table.len(), "mac: {mac}");
            }
        }
    }

    #[test]
    fn no_diff_key_is_a_registry_shortcut_on_macos() {
        // The registry's glyphs are macOS glyphs, ⌘ being the secondary modifier.
        // The commit's previous file is the one exception: ⇧⌘P is Push With Lease's
        // label. Nothing binds that label, and its key is an open question (design/decisions.md).
        // The chunk actions are this pane's own: their label is the key bound here.
        let registry: Vec<&str> = Action::ALL
            .iter()
            .filter(|action| !matches!(action, Action::StageChunk | Action::UnstageChunk))
            .filter_map(|action| action.spec().shortcut)
            .collect();
        for row in WORKING_KEYS.iter().chain(COMMIT_KEYS.iter()) {
            if !row.keys.contains("secondary") {
                continue;
            }
            let glyph = mac_glyph(row.keys);
            if glyph == "⇧⌘P" && row.action == "PrevFile" {
                continue;
            }
            assert!(
                !registry.contains(&glyph.as_str()),
                "{glyph} is a registry shortcut"
            );
        }
    }

    #[test]
    fn the_commit_keys_are_the_listed_ones_and_no_vim_letter_is_bound() {
        assert_eq!(
            COMMIT_KEYS.iter().map(|row| row.keys).collect::<Vec<_>>(),
            [
                "secondary-shift-n",
                "secondary-shift-p",
                "up",
                "down",
                "pageup",
                "pagedown",
                "escape",
            ]
        );
        for row in COMMIT_KEYS {
            assert!(!matches!(row.keys, "j" | "k" | "h" | "l" | "n" | "p"));
        }
    }

    #[test]
    fn the_chunk_actions_show_the_key_they_are_bound_to() {
        // The palette and the menus show the registry's shortcut: it is the key bound here.
        let key = |action: &str| {
            WORKING_KEYS
                .iter()
                .find(|row| row.action == action)
                .map(|row| mac_glyph(row.keys))
        };
        assert_eq!(
            key("StageHunkUnderCursor").as_deref(),
            Action::StageChunk.spec().shortcut
        );
        assert_eq!(
            key("StageHunkUnderCursor").as_deref(),
            Action::UnstageChunk.spec().shortcut
        );
    }

    #[test]
    fn up_and_down_move_the_working_copy_cursor_by_changed_line() {
        let action = |keys: &str| {
            WORKING_KEYS
                .iter()
                .find(|row| row.keys == keys)
                .map(|row| row.action)
        };
        assert_eq!(action("up"), Some("PrevChangedLine"));
        assert_eq!(action("down"), Some("NextChangedLine"));
    }

    #[test]
    fn the_scroll_keys_are_each_bound_once_in_the_read_only_pane() {
        assert_eq!(
            read_only_key_bindings()
                .iter()
                .filter(|binding| binding.action().as_any().is::<ScrollLineUp>())
                .count(),
            1
        );
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
