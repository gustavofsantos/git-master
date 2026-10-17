//! The thin strip on a panel's edge that is dragged to resize it. The
//! math and the limits live in `resize`; this draws the strip, follows the
//! pointer and calls back with the size the drag asks for.
//!
//! Adapted from rvw's `crates/rvw-gui/src/resize_handle.rs` (MIT, same author).

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Context, DragMoveEvent, ElementId, Entity, Render, Stateful, Window, div, px,
};

use crate::resize::{self, Limits};
use crate::theme;

/// The width of the strip's hit area.
const THICKNESS: f32 = 5.;

/// Which edge of its panel the strip is on. Both edges are the panel's far
/// side, so dragging away from the panel enlarges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// A panel's right edge: moving right enlarges it.
    Right,
    /// A panel's top edge: moving up enlarges it.
    Top,
}

impl Edge {
    fn vertical(self) -> bool {
        self == Self::Top
    }

    /// Whether moving the pointer to larger coordinates enlarges the panel.
    fn grows_with_pointer(self) -> bool {
        self != Self::Top
    }
}

/// The drag in flight: the strip it belongs to, the size when it started, and
/// where the pointer was at its first move. Made at render, so each drag has
/// its own.
struct Resizing {
    owner: ElementId,
    size: f32,
    origin: Rc<Cell<Option<f32>>>,
}

/// What a drag shows under the pointer: nothing.
struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The strip. `size` is the panel's size now; `set` receives the size a drag
/// asks for, already within `limits`; `reset` runs on a double click.
pub fn handle<V: 'static>(
    id: impl Into<ElementId>,
    edge: Edge,
    size: f32,
    limits: Limits,
    set: impl Fn(&mut V, f32, &mut Context<V>) + 'static,
    reset: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let value = Resizing {
        owner: id.clone(),
        size,
        origin: Rc::new(Cell::new(None)),
    };
    let owner = id.clone();
    let strip = div()
        .id(id)
        .absolute()
        .hover(|style| style.bg(theme::hairline_strong()));
    let strip = match edge {
        Edge::Right => strip
            .top_0()
            .right_0()
            .h_full()
            .w(px(THICKNESS))
            .cursor_col_resize(),
        Edge::Top => strip
            .top_0()
            .left_0()
            .w_full()
            .h(px(THICKNESS))
            .cursor_row_resize(),
    };
    strip
        .on_drag(value, |_: &Resizing, _, _, cx: &mut App| {
            let ghost: Entity<Ghost> = cx.new(|_| Ghost);
            ghost
        })
        .on_drag_move(cx.listener(
            move |this: &mut V, event: &DragMoveEvent<Resizing>, window: &mut Window, cx| {
                let drag = event.drag(cx);
                // Every strip hears every drag of this type; only the one
                // that started it may move its panel.
                if drag.owner != owner {
                    return;
                }
                let viewport = window.viewport_size();
                let (pointer, extent) = if edge.vertical() {
                    (
                        f32::from(event.event.position.y),
                        f32::from(viewport.height),
                    )
                } else {
                    (f32::from(event.event.position.x), f32::from(viewport.width))
                };
                // The origin is the pointer at the first move of this drag,
                // not at mouse down: the drag threshold would otherwise jump.
                let origin = drag.origin.get().unwrap_or(pointer);
                drag.origin.set(Some(origin));
                // Every edge grows the panel as the pointer moves away from it.
                let next = resize::dragged(
                    drag.size,
                    origin,
                    pointer,
                    edge.grows_with_pointer(),
                    limits,
                    extent,
                );
                set(this, next, cx);
                cx.notify();
            },
        ))
        .on_click(cx.listener(
            move |this: &mut V, event: &ClickEvent, _: &mut Window, cx| {
                if event.click_count() == 2 {
                    reset(this, cx);
                    cx.notify();
                }
            },
        ))
}
