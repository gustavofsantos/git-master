//! The diff of the selected file: unstaged hunks, then staged hunks, with
//! stage and unstage by hunk and by line (D24). The rows and the selection
//! rules live in `diff_rows`; this file draws them and maps keys and clicks
//! onto them.

use std::ops::Range;

use gm_schema::{FileDiff, LineKind, StageTarget};
use gpui::{
    AnyElement, ClickEvent, Context, Div, EventEmitter, FocusHandle, FontWeight, Hsla, IntoElement,
    KeyBinding, Render, ScrollStrategy, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};

use crate::diff_rows::{
    Apply, Diffs, LineKey, Press, Row, Selection, Side, build_rows, change_counts,
    changed_keys_between, hunk_header_text, hunk_selection_count, hunks_of, next_changed_line,
    next_hunk_header, press, prev_changed_line, prev_hunk_header, split_pairs,
};
use crate::icon::{self, Icon};
use crate::theme;

/// The key context the pane's bindings are scoped to.
const CONTEXT: &str = "DiffPane";

/// Height of every list row; `uniform_list` needs equal heights.
const ROW_HEIGHT: f32 = 22.;

const PAIR_WARNING: &str = "Par −/+ staged pela metade: o index ficaria sem a linha antiga e sem a nova, e pode não compilar.";

// Interim colours. theme.rs does not expose the runtime palette yet, so each
// role is a thin wrapper over an existing token; swap a wrapper's body for the
// matching `theme::` accessor once it lands.

fn ground() -> Hsla {
    theme::color(theme::BACKGROUND)
}

fn panel() -> Hsla {
    theme::color(theme::PANEL)
}

fn text() -> Hsla {
    theme::color(theme::TEXT)
}

fn muted() -> Hsla {
    theme::color(theme::MUTED)
}

fn accent() -> Hsla {
    theme::color(theme::UNTRACKED)
}

fn selected_row() -> Hsla {
    theme::color(theme::SELECTED)
}

fn added_fg() -> Hsla {
    theme::color(theme::STAGED)
}

fn added_bg() -> Hsla {
    theme::color(theme::STAGED).opacity(0.18)
}

fn removed_fg() -> Hsla {
    theme::color(theme::ERROR)
}

fn removed_bg() -> Hsla {
    theme::color(theme::ERROR).opacity(0.18)
}

fn warn_fg() -> Hsla {
    theme::color(theme::UNSTAGED)
}

fn warn_bg() -> Hsla {
    theme::color(theme::UNSTAGED).opacity(0.18)
}

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
    scroll: UniformListScrollHandle,
    unstaged: Option<FileDiff>,
    staged: Option<FileDiff>,
    rows: Vec<Row>,
    selection: Selection,
    /// Index into `rows`.
    cursor: usize,
    visual: Option<Visual>,
    /// The apply key pressed once while its selection splits a pair.
    pending: Option<Apply>,
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
            scroll: UniformListScrollHandle::new(),
            unstaged: None,
            staged: None,
            rows: Vec::new(),
            selection: Selection::default(),
            cursor: 0,
            visual: None,
            pending: None,
        }
    }

    /// Replaces what is shown. `None` for a side means it has no diff (or is
    /// still loading). Selection state resets when the path changes and is kept
    /// when the same path is reloaded and a hunk id is still present.
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
        self.rows = build_rows(self.diffs());
        self.selection = kept;
        self.visual = None;
        self.pending = None;
        if same_file {
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        } else {
            self.cursor = next_changed_line(&self.rows, self.diffs(), 0).unwrap_or(0);
        }
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

    /// Stage or unstage the whole hunk, as the button does.
    fn stage_hunk(&mut self, side: Side, hunk_index: usize, cx: &mut Context<Self>) {
        self.pending = None;
        let (Some(path), Some(hunk)) = (self.path(), self.hunk_id(side, hunk_index)) else {
            return;
        };
        let target = StageTarget::Hunk { path, hunk };
        let event = match side {
            Side::Unstaged => DiffPaneEvent::Stage(target),
            Side::Staged => DiffPaneEvent::Unstage(target),
        };
        cx.emit(event);
    }

    fn stage_hunk_at_cursor(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        if let Some((side, hunk_index)) = self.rows.get(self.cursor).and_then(|row| row.hunk()) {
            self.stage_hunk(side, hunk_index, cx);
        } else {
            cx.notify();
        }
    }

    /// Stages (unstaged side) or unstages (staged side) the selected lines,
    /// one event per hunk, applying the two-press rule to a split pair.
    fn apply_lines(&mut self, action: Apply, cx: &mut Context<Self>) {
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

    /// Escape leaves visual mode when it is on, and clears the selection
    /// otherwise.
    fn escape(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        if self.visual.is_some() {
            self.visual = None;
        } else {
            self.selection.clear();
        }
        cx.notify();
    }

    fn toggle_visual(&mut self, cx: &mut Context<Self>) {
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
        let binary = self.unstaged.as_ref().is_some_and(|d| d.binary)
            || self.staged.as_ref().is_some_and(|d| d.binary);
        Some(if binary {
            "Binary file (n/a)"
        } else {
            "No changes"
        })
    }

    fn file_header(&self) -> Option<Div> {
        let path = self.path()?;
        let diffs = self.diffs();
        let mut parts = Vec::new();
        for (side, label) in [(Side::Unstaged, "unstaged"), (Side::Staged, "staged")] {
            let hunks = diffs.hunks(side);
            if !hunks.is_empty() {
                let (added, removed) = change_counts(hunks);
                parts.push(format!("{label} +{added} −{removed}"));
            }
        }
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .bg(panel())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(path))
                .child(div().text_color(muted()).child(parts.join(" · "))),
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
                .bg(warn_bg())
                .text_color(warn_fg())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(icon::view(Icon::Warning, icon::SMALL, warn_fg()))
                        .child(PAIR_WARNING),
                )
                .children(hint.map(|text| div().child(text))),
        )
    }

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
        let button = match side {
            Side::Unstaged => "Stage hunk",
            Side::Staged => "Unstage hunk",
        };
        let background = if ix == self.cursor {
            selected_row()
        } else {
            panel()
        };
        div()
            .h(px(ROW_HEIGHT))
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
                    .text_color(muted())
                    .child(hunk_header_text(hunk)),
            )
            .child(
                div()
                    .text_color(muted())
                    .child(format!("{selected} de {total} linhas")),
            )
            .child(
                div()
                    .id(("diff-hunk-button", ix))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(accent())
                    .text_color(accent())
                    .cursor_pointer()
                    .child(button)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.stage_hunk(side, hunk_index, cx);
                    })),
            )
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
            LineKind::Added => (added_bg(), added_fg(), "+"),
            LineKind::Removed => (removed_bg(), removed_fg(), "−"),
            LineKind::Context => (ground(), text(), " "),
        };
        let changed = line.kind != LineKind::Context;
        let selected = self.selection.contains((side, hunk_index, line_index));
        let background = if ix == self.cursor {
            selected_row()
        } else {
            tint
        };
        let number = |n: Option<u32>| {
            div()
                .w(px(44.))
                .flex_none()
                .text_right()
                .text_color(muted())
                .child(n.map_or_else(String::new, |n| n.to_string()))
        };
        let checkbox = if changed {
            let mark = if selected {
                icon::filled(Icon::CheckSquare, icon::SMALL, accent())
            } else {
                icon::view(Icon::Square, icon::SMALL, muted())
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
            Some(Row::Section { title, .. }) => div()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .px_3()
                .bg(ground())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted())
                .child(title)
                .into_any_element(),
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
                .text_color(muted())
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
                this.stage_hunk_at_cursor(cx);
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
            .bg(ground())
            .text_color(text())
            .font_family(theme::ui_font())
            .text_sm()
            .children(self.file_header())
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
}
