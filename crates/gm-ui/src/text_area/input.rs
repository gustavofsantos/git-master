// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area/input.rs, MIT, same author.
// Copied into gm-ui from spikes/b-text (see spikes/b-text/FINDINGS.md).
//! IME and typed text: the platform's input handler for the area.

use std::ops::Range;

use gpui::{
    Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point, px, size,
};

use super::TextArea;
use crate::text_area_math::{offset_from_utf16, offset_to_utf16, rows_height};

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        self.editor.text().get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.editor.selection();
        Some(UTF16Selection {
            reversed: self.editor.has_selection() && self.editor.cursor() == selection.start,
            range: self.range_to_utf16(&selection),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_read_only() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.editor.selection());
        self.editor.set_cursor(range.start, false);
        self.editor.set_cursor(range.end, true);
        self.replace_selection(new_text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_read_only() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.editor.selection());
        self.editor.set_cursor(range.start, false);
        self.editor.set_cursor(range.end, true);
        self.editor.insert(new_text);
        let start = range.start.min(self.editor.len());
        let end = (start + new_text.len()).min(self.editor.len());
        self.marked_range = (!new_text.is_empty()).then_some(start..end);
        if let Some(selected) = new_selected_range_utf16 {
            let from = start + offset_from_utf16(new_text, selected.start);
            let to = start + offset_from_utf16(new_text, selected.end);
            self.editor.set_cursor(from.min(end), false);
            self.editor.set_cursor(to.min(end), true);
        }
        self.touched(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let shaped = self.fresh_shaped()?;
        let range = self.range_from_utf16(&range_utf16);
        let row = shaped.row_for_offset(range.start);
        let x = shaped.x_for_offset(range.start);
        let end_x = if shaped.row_for_offset(range.end) == row {
            shaped.x_for_offset(range.end)
        } else {
            x
        };
        let line_height = self.style.line_height;
        let top = bounds.top() + px(rows_height(row, line_height) - self.scroll_y);
        Some(Bounds::new(
            point(bounds.left() + px(x), top),
            size(px((end_x - x).max(2.)), px(line_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_point(point)?;
        Some(offset_to_utf16(self.editor.text(), offset))
    }
}
