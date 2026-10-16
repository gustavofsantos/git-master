// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area.rs, MIT, same author.
// Copied into gm-ui from spikes/b-text (see spikes/b-text/FINDINGS.md).
//! A multi-line text area: soft-wrapped, growing between a minimum and a
//! maximum number of rows, then scrolling. The text lives in `editor::Editor`;
//! this view only turns keys, the mouse and the IME into editor calls and asks
//! the element to draw the result.

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Action, App, Bounds, Context, CursorStyle, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, Point, SharedString, Window, div, prelude::*, px,
};

use crate::editor::Editor;
use crate::text_area_element::{Shaped, TextAreaElement};
use crate::text_area_math::{offset_from_utf16, offset_to_utf16, row_at_y, target_row};
use crate::theme;

mod input;
mod keys;
mod pointer;

pub use keys::bind_keys;
use keys::{
    Backspace, Cancel, Delete, DeleteToLineStart, DeleteWordLeft, DeleteWordRight, DocEnd,
    DocStart, Down, Left, LineEnd, LineStart, Newline, Redo, Right, SelectAll, SelectDocEnd,
    SelectDocStart, SelectDown, SelectLeft, SelectLineEnd, SelectLineStart, SelectRight, SelectUp,
    SelectWordLeft, SelectWordRight, ShowCharacterPalette, Undo, Up, WordLeft, WordRight,
};

const KEY_CONTEXT: &str = "TextArea";
const BASE_FONT_SIZE: f32 = 13.5;
/// A whole number of pixels, about 1.5 times the font size, so rows land on
/// pixel boundaries and text does not shimmer while scrolling.
pub const LINE_HEIGHT: f32 = 20.;
const BLINK: Duration = Duration::from_millis(530);

/// What the area asks its owner to do. Typing is read from the view. The
/// owner decides what a commit shortcut means; the area has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAreaEvent {
    Cancel,
}

pub struct TextArea {
    focus_handle: FocusHandle,
    editor: Editor,
    placeholder: SharedString,
    min_lines: usize,
    max_lines: usize,
    /// The IME's in-progress composition, underlined while it lasts.
    marked_range: Option<Range<usize>>,
    /// The layout of the last paint; `None` before the first one.
    shaped: Option<Rc<Shaped>>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_y: f32,
    content_height: f32,
    /// Set by every edit or move; the next paint scrolls the caret into view
    /// and clears it, so wheel scrolling is not undone by a repaint.
    reveal: bool,
    /// The x Up and Down keep returning to across rows of different lengths.
    preferred_x: Option<f32>,
    selecting: bool,
    caret_visible: bool,
    /// Bumped by every activity. A pending blink timer carries the value it
    /// was started with and does nothing if it no longer matches, so old
    /// timers cannot toggle the caret in the middle of typing.
    blink_gen: u64,
}

impl TextArea {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            editor: Editor::new(),
            placeholder: placeholder.to_owned().into(),
            min_lines: 3,
            max_lines: 10,
            marked_range: None,
            shaped: None,
            last_bounds: None,
            scroll_y: 0.,
            content_height: 0.,
            reveal: false,
            preferred_x: None,
            selecting: false,
            caret_visible: true,
            blink_gen: 0,
        }
    }

    /// Sets the number of rows the box shows when empty and when full.
    #[must_use]
    pub fn with_lines(mut self, min: usize, max: usize) -> Self {
        self.min_lines = min;
        self.max_lines = max;
        self
    }

    pub fn text(&self) -> &str {
        self.editor.text()
    }

    /// Replaces the whole content and puts the caret at its end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.editor.set_text(text);
        self.marked_range = None;
        self.shaped = None;
        self.preferred_x = None;
        self.reveal = true;
        self.caret_visible = true;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    /// Focuses the area and starts its caret blinking.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.restart_blink(window, cx);
    }

    // What the element reads.

    pub(crate) fn marked_range(&self) -> Option<&Range<usize>> {
        self.marked_range.as_ref()
    }

    pub(crate) fn placeholder(&self) -> &SharedString {
        &self.placeholder
    }

    pub(crate) fn selection(&self) -> Range<usize> {
        self.editor.selection()
    }

    pub(crate) fn cursor(&self) -> usize {
        self.editor.cursor()
    }

    pub(crate) fn caret_visible(&self) -> bool {
        self.caret_visible
    }

    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub(crate) fn scroll_state(&self) -> (f32, bool) {
        (self.scroll_y, self.reveal)
    }

    pub(crate) fn line_limits(&self) -> (usize, usize) {
        (self.min_lines, self.max_lines)
    }

    /// Keeps what the last paint decided: the layout for hit-testing, the
    /// scroll offset after revealing the caret, and the content height.
    pub(crate) fn record_layout(
        &mut self,
        shaped: Option<Rc<Shaped>>,
        bounds: Bounds<Pixels>,
        scroll: f32,
        content_height: f32,
    ) {
        self.shaped = shaped;
        self.last_bounds = Some(bounds);
        self.scroll_y = scroll;
        self.content_height = content_height;
        self.reveal = false;
    }

    // Blinking.

    fn restart_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink_gen = self.blink_gen.wrapping_add(1);
        Self::schedule_blink(self.blink_gen, window, cx);
    }

    fn schedule_blink(generation: u64, window: &Window, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            executor.timer(BLINK).await;
            this.update_in(cx, |this, window, cx| {
                this.blink_tick(generation, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Toggles the caret and arms the next tick, but only while focused and
    /// without a selection. Any other state leaves the caret solid and lets
    /// the chain end; the next key press, click or focus restarts it.
    fn blink_tick(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.blink_gen {
            return;
        }
        if !self.focus_handle.is_focused(window) || self.editor.has_selection() {
            if !self.caret_visible {
                self.caret_visible = true;
                cx.notify();
            }
            return;
        }
        self.caret_visible = !self.caret_visible;
        cx.notify();
        Self::schedule_blink(generation, window, cx);
    }

    /// Common tail of every edit and caret move.
    fn touched(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.preferred_x = None;
        self.reveal = true;
        self.restart_blink(window, cx);
        cx.notify();
    }

    // Layout queries. The layout is from the last paint; if the text has
    // changed since, there is no honest answer.

    fn fresh_shaped(&self) -> Option<Rc<Shaped>> {
        self.shaped
            .as_ref()
            .filter(|shaped| shaped.text.as_ref() == self.editor.text())
            .cloned()
    }

    fn offset_for_point(&self, position: Point<Pixels>) -> Option<usize> {
        let shaped = self.fresh_shaped()?;
        let bounds = self.last_bounds?;
        let y = f32::from(position.y - bounds.top()) + self.scroll_y;
        let row = row_at_y(y, LINE_HEIGHT, shaped.rows.len());
        Some(shaped.offset_at(row, f32::from(position.x - bounds.left())))
    }

    // Registering one action.

    fn motion<A: Action>(el: Div, cx: &mut Context<Self>, f: fn(&mut Editor)) -> Div {
        el.on_action(cx.listener(move |this, _: &A, window, cx| {
            f(&mut this.editor);
            this.touched(window, cx);
        }))
    }

    fn edit<A: Action>(el: Div, cx: &mut Context<Self>, f: fn(&mut Editor)) -> Div {
        el.on_action(cx.listener(move |this, _: &A, window, cx| {
            f(&mut this.editor);
            this.marked_range = None;
            this.touched(window, cx);
        }))
    }

    fn vertical_action<A: Action>(el: Div, cx: &mut Context<Self>, up: bool, extend: bool) -> Div {
        el.on_action(cx.listener(move |this, _: &A, window, cx| {
            this.vertical(up, extend, window, cx);
        }))
    }

    fn row_action<A: Action>(el: Div, cx: &mut Context<Self>, end: bool, extend: bool) -> Div {
        el.on_action(cx.listener(move |this, _: &A, window, cx| {
            this.row_edge(end, extend, window, cx);
        }))
    }

    fn replace_selection(&mut self, text: &str, window: &Window, cx: &mut Context<Self>) {
        self.editor.insert(text);
        self.marked_range = None;
        self.touched(window, cx);
    }

    // Moves that need the wrapped layout.

    fn vertical(&mut self, up: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let cursor = self.editor.cursor();
        let Some(shaped) = self.fresh_shaped() else {
            if up {
                self.editor.move_up(extend);
            } else {
                self.editor.move_down(extend);
            }
            self.touched(window, cx);
            return;
        };
        let row = shaped.row_for_offset(cursor);
        let x = self
            .preferred_x
            .unwrap_or_else(|| shaped.x_for_offset(cursor));
        let target = match target_row(row, up, shaped.rows.len()) {
            Some(row) => shaped.offset_at(row, x),
            None if up => 0,
            None => self.editor.len(),
        };
        self.editor.set_cursor(target, extend);
        self.touched(window, cx);
        self.preferred_x = Some(x);
    }

    /// Home and End go to the edge of the visual row, not the logical line.
    fn row_edge(&mut self, end: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let cursor = self.editor.cursor();
        let target = self.fresh_shaped().and_then(|shaped| {
            let row = shaped.rows.get(shaped.row_for_offset(cursor))?;
            Some(if end {
                row.caret_limit(&shaped.text)
            } else {
                row.start
            })
        });
        match target {
            Some(offset) => self.editor.set_cursor(offset, extend),
            None if end => self.editor.line_end(extend),
            None => self.editor.line_start(extend),
        }
        self.touched(window, cx);
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        let text = self.editor.text();
        offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        let text = self.editor.text();
        offset_from_utf16(text, range.start)..offset_from_utf16(text, range.end)
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let el = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .px(px(10.))
            .py(px(8.))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .font_family(theme::ui_font())
            .text_size(px(BASE_FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(theme::text())
            // The border is always 1px; only its colour changes, so focus
            // never moves the layout.
            .border_b_1()
            .border_color(if focused {
                theme::text()
            } else {
                theme::muted()
            });
        let el = Self::motion::<Left>(el, cx, |e| e.move_left(false));
        let el = Self::motion::<Right>(el, cx, |e| e.move_right(false));
        let el = Self::motion::<SelectLeft>(el, cx, |e| e.move_left(true));
        let el = Self::motion::<SelectRight>(el, cx, |e| e.move_right(true));
        let el = Self::motion::<WordLeft>(el, cx, |e| e.word_left(false));
        let el = Self::motion::<WordRight>(el, cx, |e| e.word_right(false));
        let el = Self::motion::<SelectWordLeft>(el, cx, |e| e.word_left(true));
        let el = Self::motion::<SelectWordRight>(el, cx, |e| e.word_right(true));
        let el = Self::motion::<DocStart>(el, cx, |e| e.doc_start(false));
        let el = Self::motion::<DocEnd>(el, cx, |e| e.doc_end(false));
        let el = Self::motion::<SelectDocStart>(el, cx, |e| e.doc_start(true));
        let el = Self::motion::<SelectDocEnd>(el, cx, |e| e.doc_end(true));
        let el = Self::motion::<SelectAll>(el, cx, Editor::select_all);
        let el = Self::edit::<Newline>(el, cx, |e| e.insert("\n"));
        let el = Self::edit::<Backspace>(el, cx, Editor::backspace);
        let el = Self::edit::<Delete>(el, cx, Editor::delete);
        let el = Self::edit::<DeleteWordLeft>(el, cx, Editor::delete_word_left);
        let el = Self::edit::<DeleteWordRight>(el, cx, Editor::delete_word_right);
        let el = Self::edit::<DeleteToLineStart>(el, cx, Editor::delete_to_line_start);
        let el = Self::edit::<Undo>(el, cx, |e| {
            e.undo();
        });
        let el = Self::edit::<Redo>(el, cx, |e| {
            e.redo();
        });
        let el = Self::vertical_action::<Up>(el, cx, true, false);
        let el = Self::vertical_action::<Down>(el, cx, false, false);
        let el = Self::vertical_action::<SelectUp>(el, cx, true, true);
        let el = Self::vertical_action::<SelectDown>(el, cx, false, true);
        let el = Self::row_action::<LineStart>(el, cx, false, false);
        let el = Self::row_action::<LineEnd>(el, cx, true, false);
        let el = Self::row_action::<SelectLineStart>(el, cx, false, true);
        let el = Self::row_action::<SelectLineEnd>(el, cx, true, true);
        el.on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, window, _| {
                window.show_character_palette();
            }))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(TextAreaEvent::Cancel)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            // The clip keeps scrolled text inside the rows; the padding sits
            // outside it so no half row shows above or below.
            .child(
                div()
                    .w_full()
                    .h_full()
                    .overflow_hidden()
                    .child(TextAreaElement { area: cx.entity() }),
            )
    }
}

impl EventEmitter<TextAreaEvent> for TextArea {}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
