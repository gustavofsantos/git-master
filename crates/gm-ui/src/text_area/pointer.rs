// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area/pointer.rs, MIT, same author.
// Copied into gm-ui from spikes/b-text (see spikes/b-text/FINDINGS.md).
//! The clipboard, the mouse and the wheel.

use gpui::{
    ClipboardItem, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Window,
    px,
};

use super::keys::{Copy, Cut, Paste};
use super::{LINE_HEIGHT, TextArea};
use crate::text_area_math::{ClickKind, click_kind, max_scroll};

impl TextArea {
    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.has_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.editor.selected_text().to_owned(),
            ));
        }
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.has_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.editor.selected_text().to_owned(),
            ));
            self.replace_selection("", window, cx);
        }
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text, window, cx);
        }
    }

    // Mouse.

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.selecting = true;
        if let Some(offset) = self.offset_for_point(event.position) {
            if event.modifiers.shift {
                self.editor.set_cursor(offset, true);
            } else {
                match click_kind(event.click_count) {
                    ClickKind::Caret => self.editor.set_cursor(offset, false),
                    ClickKind::Word => self.editor.select_word_at(offset),
                    ClickKind::Line => self.editor.select_line_at(offset),
                }
            }
        }
        self.touched(window, cx);
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        if let Some(offset) = self.offset_for_point(event.position)
            && offset != self.editor.cursor()
        {
            self.editor.set_cursor(offset, true);
            self.touched(window, cx);
        }
    }

    pub(super) fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self
            .last_bounds
            .map_or(0., |bounds| f32::from(bounds.size.height));
        let limit = max_scroll(self.content_height, viewport);
        if limit <= 0. {
            return;
        }
        let delta = f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
        let scroll = (self.scroll_y - delta).max(0.).min(limit);
        if (scroll - self.scroll_y).abs() > f32::EPSILON {
            self.scroll_y = scroll;
            cx.notify();
            cx.stop_propagation();
        }
    }
}
