//! The state graph of the Flow tab, drawn in GPUI (docs/10, Flow). The layout
//! is in `flow_graph`: every position comes from its `Geometry`, and this file
//! only turns those positions into elements. The cards and the terminal pills
//! are absolutely positioned divs; the edges are paths on one `canvas`, with a
//! small arrow head at each target; the labels are small text at their anchors.

use gpui::{
    Div, FontWeight, Hsla, IntoElement, PathBuilder, Pixels, Stateful, Window, canvas, div, point,
    prelude::*, px,
};

use crate::flow_graph::{
    Geometry, Graph, LABEL_HEIGHT, LABEL_OFFSET, Metrics, Node, NodeKind, Point, Rect, Route,
};
use crate::theme;
use crate::workflows_page::FlowRow;

/// The sizes of the cards, the terminal pills and the gaps, in pixels.
pub const METRICS: Metrics = Metrics {
    node_width: 200.0,
    node_height: 64.0,
    terminal_height: 28.0,
    layer_gap: 72.0,
    slot_gap: 40.0,
};

/// The width of a terminal pill, centred on its slot.
const TERMINAL_WIDTH: f32 = 132.0;
const EDGE_WIDTH: f32 = 1.5;
/// The arrow head at a target: its length along the edge and its half width.
const ARROW_LENGTH: f32 = 7.0;
const ARROW_HALF: f32 = 4.0;
/// The dash of a defaulted edge: the stroke, then the gap.
const DASH: [f32; 2] = [4.0, 3.0];

/// One edge ready to paint.
struct Stroke {
    points: Vec<Point>,
    color: Hsla,
    dashed: bool,
    arrow: Option<[Point; 3]>,
}

/// The state graph: the cards, the edges and their labels, in a box as large
/// as the geometry, scrolling both ways.
#[must_use]
pub fn flow_graph(graph: &Graph, geometry: &Geometry, rows: &[FlowRow]) -> Stateful<Div> {
    let mut strokes = Vec::with_capacity(graph.edges.len());
    let mut labels = Vec::with_capacity(graph.edges.len());
    for (edge, route) in graph.edges.iter().zip(&geometry.edges) {
        strokes.push(stroke(
            edge.dangling,
            edge.defaulted,
            edge.to.is_some(),
            route,
        ));
        labels.push(label(edge.dangling, edge.defaulted, &edge.label, route));
    }
    let cards = graph
        .nodes
        .iter()
        .zip(&geometry.nodes)
        .map(|(node, rect)| match node.kind {
            NodeKind::State => card(node, *rect, rows),
            NodeKind::End => terminal(node, *rect),
        });
    div()
        .id("workflow-flow-graph")
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .overflow_scroll()
        .child(
            div()
                .relative()
                .w(px(geometry.width))
                .h(px(geometry.height))
                .child(edges(strokes))
                .children(cards)
                .children(labels),
        )
}

/// The edges, painted in one canvas under the cards.
fn edges(strokes: Vec<Stroke>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _cx| {
            for stroke in &strokes {
                paint(window, bounds.origin, stroke);
            }
        },
    )
    .absolute()
    .size_full()
}

fn stroke(dangling: bool, defaulted: bool, has_target: bool, route: &Route) -> Stroke {
    let color = edge_color(dangling, defaulted);
    Stroke {
        points: route.points.clone(),
        color,
        dashed: defaulted,
        arrow: has_target.then(|| arrow_head(&route.points)).flatten(),
    }
}

/// The colour of an edge: red when it dangles, faint when it is defaulted, and
/// the text colour otherwise.
fn edge_color(dangling: bool, defaulted: bool) -> Hsla {
    if dangling {
        theme::error()
    } else if defaulted {
        theme::faint()
    } else {
        theme::muted()
    }
}

/// The three corners of the arrow head at the end of a polyline: the tip, then
/// the two corners of the base. `None` for a polyline with no segment.
fn arrow_head(points: &[Point]) -> Option<[Point; 3]> {
    let [.., from, tip] = points else {
        return None;
    };
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let length = dx.hypot(dy);
    if length == 0.0 {
        return None;
    }
    let (ux, uy) = (dx / length, dy / length);
    let base = Point {
        x: tip.x - ux * ARROW_LENGTH,
        y: tip.y - uy * ARROW_LENGTH,
    };
    let side = Point {
        x: -uy * ARROW_HALF,
        y: ux * ARROW_HALF,
    };
    Some([
        *tip,
        Point {
            x: base.x + side.x,
            y: base.y + side.y,
        },
        Point {
            x: base.x - side.x,
            y: base.y - side.y,
        },
    ])
}

/// Paints one edge: its polyline, its dash, and its arrow head.
fn paint(window: &mut Window, origin: gpui::Point<Pixels>, stroke: &Stroke) {
    let at = |p: &Point| point(origin.x + px(p.x), origin.y + px(p.y));
    let mut line = PathBuilder::stroke(px(EDGE_WIDTH));
    if stroke.dashed {
        line = line.dash_array(&[px(DASH[0]), px(DASH[1])]);
    }
    for (index, p) in stroke.points.iter().enumerate() {
        if index == 0 {
            line.move_to(at(p));
        } else {
            line.line_to(at(p));
        }
    }
    if let Ok(path) = line.build() {
        window.paint_path(path, stroke.color);
    }
    if let Some([tip, left, right]) = stroke.arrow {
        let mut head = PathBuilder::fill();
        head.move_to(at(&tip));
        head.line_to(at(&left));
        head.line_to(at(&right));
        head.close();
        if let Ok(path) = head.build() {
            window.paint_path(path, stroke.color);
        }
    }
}

/// A state's card: its id in bold, its effect badge, the number of its
/// diagnostics in red, and its action.
fn card(node: &Node, rect: Rect, rows: &[FlowRow]) -> Div {
    let problems = diagnostic_count(node, rows);
    div()
        .absolute()
        .left(px(rect.x))
        .top(px(rect.y))
        .w(px(rect.w))
        .h(px(rect.h))
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme::hairline())
        .bg(theme::panel())
        .flex()
        .flex_col()
        .gap_1()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w(px(0.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .font_family(theme::code_font())
                        .font_weight(FontWeight::BOLD)
                        .child(node.id.clone()),
                )
                .children(node.level.map(level_badge))
                .children((problems > 0).then(|| problem_marker(problems))),
        )
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_xs()
                .text_color(theme::muted())
                .child(node.action.clone()),
        )
}

/// A terminal: a small pill centred on its slot. A failed one is red and a
/// blocked one amber.
fn terminal(node: &Node, rect: Rect) -> Div {
    let width = rect.w.min(TERMINAL_WIDTH);
    let color = match node.id.as_str() {
        "end.failed" => theme::error(),
        "end.blocked" => theme::warn_fg(),
        _ => theme::muted(),
    };
    div()
        .absolute()
        .left(px(rect.x + (rect.w - width) / 2.0))
        .top(px(rect.y))
        .w(px(width))
        .h(px(rect.h))
        .rounded_full()
        .border_1()
        .border_color(theme::hairline())
        .bg(theme::panel())
        .flex()
        .items_center()
        .justify_center()
        .font_family(theme::code_font())
        .text_xs()
        .text_color(color)
        .child(node.id.clone())
}

/// The effect badge of an action, styled as the badge on a Flow card.
fn level_badge(level: &'static str) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .bg(theme::selection())
        .text_color(theme::text())
        .child(level)
}

/// The count of a card's diagnostics, in red.
fn problem_marker(count: usize) -> Div {
    div()
        .flex_none()
        .px_1()
        .rounded_full()
        .border_1()
        .border_color(theme::error())
        .text_xs()
        .text_color(theme::error())
        .child(count.to_string())
}

/// The diagnostics on a state and on the states nested under it.
fn diagnostic_count(node: &Node, rows: &[FlowRow]) -> usize {
    rows.iter()
        .filter(|row| row.id == node.id || node.children.contains(&row.id))
        .map(|row| row.diagnostics.len())
        .sum()
}

/// An edge's label, as small text starting right of its anchor.
fn label(dangling: bool, defaulted: bool, text: &str, route: &Route) -> Div {
    let color = if dangling {
        theme::error()
    } else if defaulted {
        theme::faint()
    } else {
        theme::text()
    };
    div()
        .absolute()
        .left(px(route.label.x + LABEL_OFFSET))
        .top(px(route.label.y - LABEL_HEIGHT / 2.0))
        .px_1()
        .rounded_sm()
        .bg(theme::ground())
        .font_family(theme::code_font())
        .text_xs()
        .text_color(color)
        .whitespace_nowrap()
        .child(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn an_arrow_head_points_down_at_the_end_of_a_downward_segment() {
        let head = arrow_head(&[at(10.0, 0.0), at(10.0, 40.0)]).expect("a segment has a head");
        let [tip, left, right] = head;
        assert_eq!(tip, at(10.0, 40.0));
        // The base sits ARROW_LENGTH above the tip, its corners ARROW_HALF to each side.
        assert!((left.y - (40.0 - ARROW_LENGTH)).abs() < 1e-4);
        assert!((right.y - left.y).abs() < 1e-4);
        assert!((right.x - left.x - 2.0 * ARROW_HALF).abs() < 1e-4);
    }

    #[test]
    fn an_arrow_head_uses_only_the_last_segment() {
        let head = arrow_head(&[at(0.0, 0.0), at(0.0, 30.0), at(50.0, 30.0)])
            .expect("a segment has a head");
        assert_eq!(head[0], at(50.0, 30.0));
    }

    #[test]
    fn a_polyline_without_a_segment_has_no_arrow_head() {
        assert!(arrow_head(&[]).is_none());
        assert!(arrow_head(&[at(1.0, 1.0)]).is_none());
        assert!(arrow_head(&[at(1.0, 1.0), at(1.0, 1.0)]).is_none());
    }
}
