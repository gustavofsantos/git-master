// Ported from rvw (local checkout ~/Projects/rvw): crates/rvw-gui/src/text_area_element.rs, MIT, same author.
// Copied into gm-ui from spikes/b-text (see spikes/b-text/FINDINGS.md).
//! The custom element of the text area: it shapes the wrapped text, reports
//! its height to the layout engine, keeps the caret in view and paints.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, Corners, Element, ElementId, ElementInputHandler, Entity,
    Focusable, Font, GlobalElementId, Hsla, IntoElement, LayoutId, PaintQuad, Pixels, SharedString,
    Size, Style, TextAlign, TextRun, UnderlineStyle, Window, WrappedLine, fill, point, px,
    relative, size,
};

use crate::text_area::TextArea;
use crate::text_area_math::{
    LogicalSpan, VisualLine, build_visual_lines, clamp_scroll, row_for_offset, rows_height,
    scroll_to_caret, selection_segments, visible_rows,
};
use crate::theme;

/// Room kept at the right edge so a caret after the last glyph of a full row
/// is not clipped.
const CARET_WIDTH: f32 = 2.;
/// Extra highlight past the end of a selected row, for its newline.
const NEWLINE_WIDTH: f32 = 6.;

fn selection_color() -> Hsla {
    theme::selection()
}

/// The font the text is shaped with, read from the inherited text style.
#[derive(Clone)]
struct Face {
    font: Font,
    size: Pixels,
}

impl Face {
    fn current(window: &Window) -> Self {
        let style = window.text_style();
        Self {
            font: style.font(),
            size: style.font_size.to_pixels(window.rem_size()),
        }
    }

    fn runs(
        text_len: usize,
        color: Hsla,
        marked: Option<&Range<usize>>,
        font: &Font,
    ) -> Vec<TextRun> {
        let run = TextRun {
            len: text_len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let Some(marked) = marked.filter(|m| m.start < m.end && m.end <= text_len) else {
            return vec![run];
        };
        let underline = UnderlineStyle {
            color: Some(color),
            thickness: px(1.),
            wavy: false,
        };
        [
            TextRun {
                len: marked.start,
                ..run.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(underline),
                ..run.clone()
            },
            TextRun {
                len: text_len - marked.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect()
    }

    fn shape(
        &self,
        window: &Window,
        text: &SharedString,
        wrap: Option<Pixels>,
        color: Hsla,
        marked: Option<&Range<usize>>,
    ) -> Option<Vec<WrappedLine>> {
        let runs = Self::runs(text.len(), color, marked, &self.font);
        window
            .text_system()
            .shape_text(text.clone(), self.size, &runs, wrap, None)
            .ok()
            .map(|lines| lines.into_iter().collect())
    }
}

/// The wrap width for a box `width` wide.
fn wrap_width(width: Pixels) -> Pixels {
    (width - px(CARET_WIDTH)).max(px(0.))
}

fn row_count(lines: &[WrappedLine]) -> usize {
    lines
        .iter()
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum()
}

/// The text laid out at one width: what hit-testing and vertical movement
/// read after the frame is painted.
pub struct Shaped {
    pub text: SharedString,
    lines: Vec<WrappedLine>,
    pub rows: Vec<VisualLine>,
    /// Byte offset where each logical line starts.
    line_starts: Vec<usize>,
    /// First row of each logical line.
    first_rows: Vec<usize>,
    /// X of each row's start inside its unwrapped line.
    row_x: Vec<f32>,
}

impl Shaped {
    fn new(text: SharedString, lines: Vec<WrappedLine>) -> Self {
        let mut spans = Vec::with_capacity(lines.len());
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut start = 0;
        for line in &lines {
            let wraps = line
                .wrap_boundaries()
                .iter()
                .filter_map(|b| line.runs().get(b.run_ix)?.glyphs.get(b.glyph_ix))
                .map(|glyph| glyph.index)
                .collect();
            line_starts.push(start);
            spans.push(LogicalSpan {
                start,
                len: line.len(),
                wraps,
            });
            start += line.len() + 1;
        }
        let rows = build_visual_lines(&spans);
        let mut first_rows = vec![0; lines.len()];
        for (index, row) in rows.iter().enumerate().rev() {
            first_rows[row.logical] = index;
        }
        let row_x = rows
            .iter()
            .map(|row| {
                let local = row.start - line_starts[row.logical];
                f32::from(lines[row.logical].unwrapped_layout.x_for_index(local))
            })
            .collect();
        Self {
            text,
            lines,
            rows,
            line_starts,
            first_rows,
            row_x,
        }
    }

    /// The row showing `offset`.
    pub fn row_for_offset(&self, offset: usize) -> usize {
        row_for_offset(&self.rows, offset)
    }

    /// X of `offset` from the left edge of its row.
    pub fn x_for_offset(&self, offset: usize) -> f32 {
        let Some(row) = self.rows.get(self.row_for_offset(offset)) else {
            return 0.;
        };
        let local = offset.min(row.end) - self.line_starts[row.logical];
        let x = self.lines[row.logical].unwrapped_layout.x_for_index(local);
        (f32::from(x) - self.row_x[self.row_for_offset(offset)]).max(0.)
    }

    /// The offset nearest to `x` on `row`, never past the row's caret limit.
    pub fn offset_at(&self, row: usize, x: f32) -> usize {
        let Some(line) = self.rows.get(row) else {
            return self.text.len();
        };
        let start = self.line_starts[line.logical];
        let target = px(x.max(0.) + self.row_x[row]);
        let local = self.lines[line.logical]
            .unwrapped_layout
            .closest_index_for_x(target);
        (start + local).clamp(line.start, line.caret_limit(&self.text).max(line.start))
    }
}

pub struct TextAreaElement {
    pub area: Entity<TextArea>,
}

pub struct Prepaint {
    line_height: f32,
    shaped: Option<Rc<Shaped>>,
    placeholder: Vec<WrappedLine>,
    selection: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    scroll: f32,
    content_height: f32,
    origin_y: Pixels,
}

impl IntoElement for TextAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextAreaElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let area = self.area.read(cx);
        let text: SharedString = area.text().to_owned().into();
        let marked = area.marked_range().cloned();
        let (min, max) = area.line_limits();
        let line_height = area.line_height();
        let read_only = area.is_read_only();
        let face = Face::current(window);

        // Full width and full height of the owner. A filter line has no
        // definite height, so there the height is the measured one, and so
        // is a read-only area, which sizes itself to its text.
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if !read_only {
            style.size.height = relative(1.).into();
        }
        let layout = window.request_measured_layout(
            style,
            move |known, available, window, _cx| -> Size<Pixels> {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let wrap = width.map(wrap_width);
                let rows = face
                    .shape(window, &text, wrap, Hsla::default(), marked.as_ref())
                    .map_or(1, |lines| row_count(&lines));
                let height = rows_height(visible_rows(rows, min, max), line_height);
                size(width.unwrap_or(px(0.)), px(height))
            },
        );
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let area = self.area.read(cx);
        let text: SharedString = area.text().to_owned().into();
        let marked = area.marked_range().cloned();
        let placeholder = area.placeholder().clone();
        let selection = area.selection();
        let cursor = area.cursor();
        let caret_on = !area.is_read_only()
            && area.caret_visible()
            && area.is_focused(window)
            && selection.is_empty();
        let line_height = area.line_height();
        let (scroll, reveal) = area.scroll_state();

        let face = Face::current(window);
        let wrap = Some(wrap_width(bounds.size.width));
        let lines = face
            .shape(window, &text, wrap, theme::text(), marked.as_ref())
            .unwrap_or_default();
        let shaped = Rc::new(Shaped::new(text.clone(), lines));

        let placeholder_lines = if text.is_empty() {
            face.shape(window, &placeholder, wrap, theme::placeholder(), None)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let viewport = f32::from(bounds.size.height);
        let content_height = rows_height(shaped.rows.len(), line_height);
        let mut scroll = clamp_scroll(scroll, content_height, viewport);
        let cursor_row = shaped.row_for_offset(cursor);
        if reveal {
            scroll = scroll_to_caret(
                scroll,
                rows_height(cursor_row, line_height),
                line_height,
                viewport,
                content_height,
            );
        }

        let origin = point(bounds.left(), bounds.top() - px(scroll));
        let row_top = |row: usize| origin.y + px(rows_height(row, line_height));
        let selection_quads = selection_segments(&shaped.rows, &selection, text.len())
            .into_iter()
            .map(|segment| {
                let x0 = shaped.x_for_offset(segment.from);
                let mut x1 = shaped.x_for_offset(segment.to);
                if segment.newline {
                    x1 += NEWLINE_WIDTH;
                }
                fill(
                    Bounds::new(
                        point(origin.x + px(x0), row_top(segment.row)),
                        size(px((x1 - x0).max(0.)), px(line_height)),
                    ),
                    selection_color(),
                )
                .corner_radii(Corners::all(px(2.)))
            })
            .collect();
        let caret = caret_on.then(|| {
            let x = shaped.x_for_offset(cursor);
            fill(
                Bounds::new(
                    point(origin.x + px(x), row_top(cursor_row) + px(2.)),
                    size(px(CARET_WIDTH), px(line_height - 4.)),
                ),
                theme::caret(),
            )
        });

        Prepaint {
            line_height,
            shaped: Some(shaped),
            placeholder: placeholder_lines,
            selection: selection_quads,
            caret,
            scroll,
            content_height,
            origin_y: origin.y,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.area.read(cx).focus_handle(cx);
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.area.clone()),
            cx,
        );

        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = prepaint.line_height;
        if let Some(shaped) = prepaint.shaped.as_ref() {
            for (index, line) in shaped.lines.iter().enumerate() {
                let top =
                    prepaint.origin_y + px(rows_height(shaped.first_rows[index], line_height));
                let height = px(rows_height(line.wrap_boundaries().len() + 1, line_height));
                if top + height < bounds.top() || top > bounds.bottom() {
                    continue;
                }
                // A failed paint leaves this frame without the line; the next
                // frame repaints.
                let _ = line.paint(
                    point(bounds.left(), top),
                    px(line_height),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
        for (index, line) in prepaint.placeholder.iter().enumerate() {
            let top = prepaint.origin_y + px(rows_height(index, line_height));
            let _ = line.paint(
                point(bounds.left(), top),
                px(line_height),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        if let Some(caret) = prepaint.caret.take() {
            window.paint_quad(caret);
        }

        let shaped = prepaint.shaped.take();
        let (scroll, content_height) = (prepaint.scroll, prepaint.content_height);
        self.area.update(cx, |area, _| {
            area.record_layout(shaped, bounds, scroll, content_height);
        });
    }
}
