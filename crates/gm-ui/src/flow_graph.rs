//! The layout of a workflow's state graph for the Flow tab (docs/10, Flow).
//!
//! Plain data in, plain data out: no GPUI types, so the layout is tested
//! without a window. The view draws each [`Node`] as a card at its [`Rect`],
//! each [`Edge`] along its [`Route`], and sizes its scroll area from
//! [`Geometry::width`] and [`Geometry::height`].
//!
//! The layering is deterministic:
//! - Back edges are the outcomes a depth-first search from the start state
//!   finds pointing at a state still on its path. The search follows outcomes
//!   in file order, then starts again from each unvisited state in file order.
//!   What remains is acyclic.
//! - A node's layer is the longest path to it over the forward edges. A state
//!   with no forward predecessor sits on layer 0.
//! - Each terminal (`end.done`, `end.failed`, `end.blocked`) sits one layer
//!   below the deepest node that targets it. Terminals do not share one bottom
//!   layer.
//! - A state no path from `start` reaches is laid out below the reached ones,
//!   in layers of its own. Its outcomes into the reached part are stubs.
//! - Inside a layer, nodes are ordered by the barycenter of their forward
//!   predecessors' slots, ties by file order. Layer 0 keeps file order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::workflows_page::{Edge as RowEdge, FlowRow};

/// The terminal states, as the file names them.
const TERMINALS: [&str; 3] = ["end.done", "end.failed", "end.blocked"];
/// The space around the drawing, in pixels.
const MARGIN: f32 = 24.0;
/// The distance between two back-edge lanes, and from the graph to the first.
const LANE: f32 = 16.0;
/// The length of a dangling stub below its source.
const STUB: f32 = 28.0;
/// A label starts this far right of the line it is beside, and is this tall.
/// Its text is set in the code font, at `CHAR_WIDTH` per character.
pub const LABEL_OFFSET: f32 = 6.0;
pub const LABEL_HEIGHT: f32 = 16.0;
pub const CHAR_WIDTH: f32 = 7.2;
/// The padding on both sides of a label's text.
const LABEL_PADDING: f32 = 8.0;
/// Where a label's anchor sits below its source. It is on the first vertical
/// segment of its edge, which runs from the source down into the gap below it.
const LABEL_DROP: f32 = 10.0;

/// Whether a node is a state of the workflow or one of its terminals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    State,
    End,
}

/// One node of the Flow: a top-level state, or a terminal some edge reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    /// The action's name, `wait <event>` or `for_each`. Empty for a terminal.
    pub action: String,
    /// The effect badge of the action: `read`, `propose`, `apply` or `approve`.
    pub level: Option<&'static str>,
    /// The ids of the states nested under a `for_each`, in file order. The view
    /// lists them inside the card; they are not nodes.
    pub children: Vec<String>,
    pub layer: usize,
    /// The place of the node within its layer, from the left.
    pub slot: usize,
}

/// One drawn edge: the outcomes that go from one node to the same node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The id of the state the outcomes leave from.
    pub from: String,
    /// The id of the node they reach. `None` for a dangling outcome, and for a
    /// target the graph does not hold; both are drawn as stubs.
    pub to: Option<String>,
    /// Each outcome as the graph writes it, in file order, joined with `, `. A
    /// stub also names its target, since it has no arrow to point at one.
    pub label: String,
    /// The edge goes against the layers: a retry or a loop back to an earlier state.
    pub back: bool,
    /// Every outcome on the edge is defaulted.
    pub defaulted: bool,
    pub dangling: bool,
}

/// The layered state graph of one workflow.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Graph {
    /// The top-level states in file order, then the terminals in the order the
    /// edges first reach them.
    pub nodes: Vec<Node>,
    /// The outcomes: each state's outcomes in file order, states in file order.
    pub edges: Vec<Edge>,
}

/// The sizes the layout works in, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub node_width: f32,
    pub node_height: f32,
    /// The height of a terminal, which is drawn as a small pill.
    pub terminal_height: f32,
    /// The vertical gap between two layers.
    pub layer_gap: f32,
    /// The horizontal gap between two nodes of one layer.
    pub slot_gap: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn center_x(self) -> f32 {
        self.x + self.w / 2.0
    }

    fn center_y(self) -> f32 {
        self.y + self.h / 2.0
    }
}

/// One edge as a polyline, with the anchor of its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    /// The corners from the source to the target. A stub ends at its label.
    pub points: Vec<Point>,
    pub label: Point,
}

/// The drawing of a [`Graph`]: positions in the same order as its nodes and edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub nodes: Vec<Rect>,
    pub edges: Vec<Route>,
    /// The size the scroll area needs: every card, corner and label, lanes
    /// included, and the margin.
    pub width: f32,
    pub height: f32,
}

/// An outcome between node indices, before the graph is built.
struct Outcome<'a> {
    from: usize,
    /// `None` for a dangling outcome or a target the graph does not hold.
    to: Option<usize>,
    edge: &'a RowEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Unseen,
    OnPath,
    Done,
}

/// Lays out the Flow rows of one workflow. `start` is the id of its first
/// state; an id the rows do not hold falls back to the first state.
#[must_use]
pub fn layout(rows: &[FlowRow], start: &str) -> Graph {
    let (mut nodes, row_outcomes) = collect(rows);
    if nodes.is_empty() {
        return Graph::default();
    }
    let mut states = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        states.entry(node.id.clone()).or_insert(index);
    }
    let start = states.get(start).copied().unwrap_or(0);
    let mut terminals = BTreeMap::new();
    let mut outcomes: Vec<Outcome> = row_outcomes
        .into_iter()
        .map(|(from, edge)| Outcome {
            from,
            to: resolve(edge, &states, &mut terminals, &mut nodes),
            edge,
        })
        .collect();
    let out = adjacency(nodes.len(), &outcomes);
    let reached = reached(start, &out, &outcomes);
    // An outcome of a state no path reaches, into the reached part, is a stub:
    // drawn as an arrow it would pull the main flow out of its layers.
    for outcome in &mut outcomes {
        if !reached[outcome.from] && outcome.to.is_some_and(|to| reached[to]) {
            outcome.to = None;
        }
    }
    let out = adjacency(nodes.len(), &outcomes);
    let back = back_edges(start, &out, &outcomes);
    let (layer, slot) = place(&out, &outcomes, &back, &reached);
    for (index, node) in nodes.iter_mut().enumerate() {
        node.layer = layer[index];
        node.slot = slot[index];
    }
    let edges = edges_of(&nodes, &outcomes, &back);
    Graph { nodes, edges }
}

/// Whether an outcome is drawn. A defaulted outcome that goes to `end.failed` is
/// the implicit failure of its state: the cards list it, the graph leaves it out,
/// so `end.failed` is drawn only when the file writes an outcome to it.
fn drawn(edge: &RowEdge) -> bool {
    !(edge.defaulted && edge.target == "end.failed")
}

/// The top-level states, and the outcomes that belong to each of them. The
/// outcomes of a nested state stay on its `for_each`, except those that go to
/// another nested state: those are inside the loop and are not drawn.
fn collect(rows: &[FlowRow]) -> (Vec<Node>, Vec<(usize, &RowEdge)>) {
    let nested: BTreeSet<&str> = rows
        .iter()
        .filter(|row| row.depth > 0)
        .map(|row| row.id.as_str())
        .collect();
    let mut nodes: Vec<Node> = Vec::new();
    let mut outcomes = Vec::new();
    for row in rows {
        if row.depth == 0 {
            outcomes.extend(
                row.edges
                    .iter()
                    .filter(|edge| drawn(edge))
                    .map(|edge| (nodes.len(), edge)),
            );
            nodes.push(Node {
                id: row.id.clone(),
                kind: NodeKind::State,
                action: row.kind.clone(),
                level: row.level,
                children: Vec::new(),
                layer: 0,
                slot: 0,
            });
        } else if let Some(owner) = nodes.len().checked_sub(1) {
            nodes[owner].children.push(row.id.clone());
            outcomes.extend(
                row.edges
                    .iter()
                    .filter(|edge| drawn(edge))
                    .filter(|edge| edge.dangling || !nested.contains(edge.target.as_str()))
                    .map(|edge| (owner, edge)),
            );
        }
    }
    (nodes, outcomes)
}

/// The node an outcome reaches. Terminals get their node when first reached.
fn resolve(
    edge: &RowEdge,
    states: &BTreeMap<String, usize>,
    terminals: &mut BTreeMap<String, usize>,
    nodes: &mut Vec<Node>,
) -> Option<usize> {
    if edge.dangling {
        return None;
    }
    if let Some(&state) = states.get(edge.target.as_str()) {
        return Some(state);
    }
    if !TERMINALS.contains(&edge.target.as_str()) {
        return None;
    }
    let index = *terminals.entry(edge.target.clone()).or_insert_with(|| {
        nodes.push(Node {
            id: edge.target.clone(),
            kind: NodeKind::End,
            action: String::new(),
            level: None,
            children: Vec::new(),
            layer: 0,
            slot: 0,
        });
        nodes.len() - 1
    });
    Some(index)
}

/// The indices of the outcomes that leave each node, in order.
fn adjacency(count: usize, outcomes: &[Outcome]) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); count];
    for (index, outcome) in outcomes.iter().enumerate() {
        if outcome.to.is_some() {
            out[outcome.from].push(index);
        }
    }
    out
}

/// The nodes a path from `start` reaches, over the outcomes that have a target.
fn reached(start: usize, out: &[Vec<usize>], outcomes: &[Outcome]) -> Vec<bool> {
    let mut seen = vec![false; out.len()];
    let mut queue = VecDeque::from([start]);
    seen[start] = true;
    while let Some(node) = queue.pop_front() {
        for &index in &out[node] {
            let Some(to) = outcomes[index].to else {
                continue;
            };
            if !seen[to] {
                seen[to] = true;
                queue.push_back(to);
            }
        }
    }
    seen
}

/// Marks the back edges: the depth-first search starts at `start`, then at
/// each unvisited node in order, and follows outcomes in order.
fn back_edges(start: usize, out: &[Vec<usize>], outcomes: &[Outcome]) -> Vec<bool> {
    let mut marks = vec![Mark::Unseen; out.len()];
    let mut back = vec![false; outcomes.len()];
    for root in std::iter::once(start).chain(0..out.len()) {
        if marks[root] == Mark::Unseen {
            visit(root, out, outcomes, &mut marks, &mut back);
        }
    }
    back
}

fn visit(
    node: usize,
    out: &[Vec<usize>],
    outcomes: &[Outcome],
    marks: &mut [Mark],
    back: &mut [bool],
) {
    marks[node] = Mark::OnPath;
    for &index in &out[node] {
        let Some(to) = outcomes[index].to else {
            continue;
        };
        match marks[to] {
            Mark::OnPath => back[index] = true,
            Mark::Unseen => visit(to, out, outcomes, marks, back),
            Mark::Done => {}
        }
    }
    marks[node] = Mark::Done;
}

/// The layer and the slot of every node.
fn place(
    out: &[Vec<usize>],
    outcomes: &[Outcome],
    back: &[bool],
    reached: &[bool],
) -> (Vec<usize>, Vec<usize>) {
    let count = out.len();
    let mut indegree = vec![0usize; count];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, outcome) in outcomes.iter().enumerate() {
        if let (false, Some(to)) = (back[index], outcome.to) {
            indegree[to] += 1;
            preds[to].push(outcome.from);
        }
    }
    // Kahn's algorithm over the forward edges: a node is placed once all its
    // predecessors are, so its layer is the longest path to it.
    let mut layer = vec![0usize; count];
    let mut queue: VecDeque<usize> = (0..count).filter(|&node| indegree[node] == 0).collect();
    while let Some(node) = queue.pop_front() {
        for &index in &out[node] {
            if back[index] {
                continue;
            }
            let Some(to) = outcomes[index].to else {
                continue;
            };
            layer[to] = layer[to].max(layer[node] + 1);
            indegree[to] -= 1;
            if indegree[to] == 0 {
                queue.push_back(to);
            }
        }
    }
    // The states no path reaches go below the reached ones, in their own layers,
    // so their edges cannot move the main flow.
    let below = (0..count)
        .filter(|&node| reached[node])
        .map(|node| layer[node] + 1)
        .max()
        .unwrap_or(0);
    for node in (0..count).filter(|&node| !reached[node]) {
        layer[node] += below;
    }
    let depth = layer.iter().max().map_or(0, |deepest| deepest + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for node in 0..count {
        layers[layer[node]].push(node);
    }
    let mut slot = vec![0usize; count];
    for (depth_index, members) in layers.iter_mut().enumerate() {
        if depth_index > 0 {
            members.sort_by(|&a, &b| {
                let (sum_a, n_a) = barycenter(&preds[a], &slot);
                let (sum_b, n_b) = barycenter(&preds[b], &slot);
                (sum_a * n_b).cmp(&(sum_b * n_a)).then(a.cmp(&b))
            });
        }
        for (position, &node) in members.iter().enumerate() {
            slot[node] = position;
        }
    }
    (layer, slot)
}

/// The sum of the predecessors' slots and their count, so two averages compare
/// exactly by cross multiplication. A node with no predecessor counts as one.
fn barycenter(preds: &[usize], slot: &[usize]) -> (usize, usize) {
    (
        preds.iter().map(|&pred| slot[pred]).sum(),
        preds.len().max(1),
    )
}

/// The graph's edges. The outcomes that share a `from` and a `to` are one edge,
/// in the order of the file. A stub is keyed by its target too, so two stubs
/// from one state to different targets stay apart.
fn edges_of(nodes: &[Node], outcomes: &[Outcome], back: &[bool]) -> Vec<Edge> {
    let mut edges: Vec<Edge> = Vec::new();
    let mut groups: Vec<(Vec<String>, &str)> = Vec::new();
    let mut positions: BTreeMap<(usize, Option<usize>, &str), usize> = BTreeMap::new();
    for (index, outcome) in outcomes.iter().enumerate() {
        let stub = if outcome.to.is_none() {
            outcome.edge.target.as_str()
        } else {
            ""
        };
        let position = *positions
            .entry((outcome.from, outcome.to, stub))
            .or_insert_with(|| {
                edges.push(Edge {
                    from: nodes[outcome.from].id.clone(),
                    to: outcome.to.map(|to| nodes[to].id.clone()),
                    label: String::new(),
                    back: false,
                    defaulted: outcome.edge.defaulted,
                    dangling: outcome.edge.dangling,
                });
                groups.push((Vec::new(), stub));
                edges.len() - 1
            });
        let edge = &mut edges[position];
        edge.back |= back[index];
        edge.defaulted &= outcome.edge.defaulted;
        groups[position].0.push(outcome_label(outcome.edge));
    }
    for (edge, (parts, stub)) in edges.iter_mut().zip(groups) {
        edge.label = parts.join(", ");
        if edge.to.is_none() {
            edge.label = format!("{} → {stub}", edge.label);
        }
    }
    edges
}

/// The text an outcome carries in the graph: the outcome, then `if` and `limit`
/// when the rule has them. The arrow already shows the target, so it is left out.
fn outcome_label(edge: &RowEdge) -> String {
    let mut text = edge.outcome.clone();
    if let Some(guard) = &edge.guard {
        text = format!("{text} if {guard}");
    }
    if let Some(limit) = edge.limit {
        text = format!("{text} (limit {limit})");
    }
    text
}

/// Positions the graph in pixels. Each layer is centred on the widest one.
/// Forward edges to the next layer run down, across and down. Back edges, and
/// forward edges that skip a layer, run through the gaps between layers and out
/// to a lane beyond the widest layer, each on its own lane. A dangling outcome
/// is a short stub below its source.
#[must_use]
pub fn geometry(graph: &Graph, metrics: &Metrics) -> Geometry {
    let rects = node_rects(graph, metrics);
    let index: BTreeMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .rev()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let body_right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(0.0_f32, f32::max);
    let departures = departures(graph, &rects, &index);
    let mut lanes = 0;
    let mut routes = Vec::with_capacity(graph.edges.len());
    for (edge, &start_x) in graph.edges.iter().zip(&departures) {
        let source = rects[index[edge.from.as_str()]];
        let route = match edge.to.as_deref() {
            None => stub(source, start_x),
            // A back edge, or a forward edge that skips a layer: its middle
            // would cross the cards in between, so it takes a lane instead.
            Some(to)
                if edge.back
                    || graph.nodes[index[to]].layer
                        > graph.nodes[index[edge.from.as_str()]].layer + 1 =>
            {
                lanes += 1;
                let lane_x = body_right + LANE * px(lanes);
                if to == edge.from {
                    loop_route(source, lane_x)
                } else {
                    lane_route(source, rects[index[to]], start_x, lane_x, metrics.layer_gap)
                }
            }
            Some(to) => forward_route(source, rects[index[to]], start_x),
        };
        routes.push(route);
    }
    stagger_labels(&mut routes, &graph.edges);
    let mut right = 0.0_f32;
    let mut bottom = 0.0_f32;
    for rect in &rects {
        right = right.max(rect.right());
        bottom = bottom.max(rect.bottom());
    }
    for route in &routes {
        for point in &route.points {
            right = right.max(point.x);
            bottom = bottom.max(point.y);
        }
    }
    for (edge, route) in graph.edges.iter().zip(&routes) {
        right = right.max(route.label.x + label_width(&edge.label));
        bottom = bottom.max(route.label.y + LABEL_HEIGHT / 2.0);
    }
    Geometry {
        nodes: rects,
        edges: routes,
        width: right + MARGIN,
        height: bottom + MARGIN,
    }
}

/// The rectangle of every node: the layers stacked, each centred on the widest.
/// A terminal is `terminal_height` tall and hangs from the top of its layer.
fn node_rects(graph: &Graph, metrics: &Metrics) -> Vec<Rect> {
    let depth = graph
        .nodes
        .iter()
        .map(|node| node.layer + 1)
        .max()
        .unwrap_or(0);
    let mut widths = vec![0usize; depth];
    for node in &graph.nodes {
        widths[node.layer] += 1;
    }
    let widest = widths.iter().copied().max().unwrap_or(0);
    let step_x = metrics.node_width + metrics.slot_gap;
    let step_y = metrics.node_height + metrics.layer_gap;
    graph
        .nodes
        .iter()
        .map(|node| {
            let inset = px(widest - widths[node.layer]) * step_x / 2.0;
            let h = match node.kind {
                NodeKind::State => metrics.node_height,
                NodeKind::End => metrics.terminal_height,
            };
            Rect {
                x: MARGIN + inset + px(node.slot) * step_x,
                y: MARGIN + px(node.layer) * step_y,
                w: metrics.node_width,
                h,
            }
        })
        .collect()
}

/// Moves a label one row down when it would meet a label already placed on its
/// row, so the labels of the edges that leave one state do not overlap. A loop
/// is drawn sideways first, and its label stays on its lane.
fn stagger_labels(routes: &mut [Route], edges: &[Edge]) {
    let mut placed: Vec<(f32, f32, f32)> = Vec::with_capacity(routes.len());
    for (route, edge) in routes.iter_mut().zip(edges) {
        let looped = route.points[0].y.total_cmp(&route.points[1].y).is_eq();
        let start = route.label.x + LABEL_OFFSET;
        let end = route.label.x + label_width(&edge.label);
        let clashes = |y: f32| {
            placed.iter().any(|&(other_y, other_start, other_end)| {
                (other_y - y).abs() < LABEL_HEIGHT && start < other_end && other_start < end
            })
        };
        if !looped && clashes(route.label.y) {
            route.label.y += LABEL_HEIGHT;
        }
        placed.push((route.label.y, start, end));
    }
}

/// The x where each edge leaves its source, in the order of the edges. The
/// edges that leave one state share its bottom side, evenly spaced in file
/// order, so each one has its own first segment and its own label.
fn departures(graph: &Graph, rects: &[Rect], index: &BTreeMap<&str, usize>) -> Vec<f32> {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &graph.edges {
        *count.entry(edge.from.as_str()).or_insert(0) += 1;
    }
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    graph
        .edges
        .iter()
        .map(|edge| {
            let from = edge.from.as_str();
            let place = seen.entry(from).or_insert(0);
            let position = *place;
            *place += 1;
            let source = rects[index[from]];
            source.x + source.w * px(position + 1) / px(count[from] + 1)
        })
        .collect()
}

fn forward_route(source: Rect, target: Rect, start_x: f32) -> Route {
    let end_x = target.center_x();
    let start_y = source.bottom();
    let end_y = target.y;
    let middle = start_y.midpoint(end_y);
    Route {
        points: vec![
            Point {
                x: start_x,
                y: start_y,
            },
            Point {
                x: start_x,
                y: middle,
            },
            Point {
                x: end_x,
                y: middle,
            },
            Point { x: end_x, y: end_y },
        ],
        label: Point {
            x: start_x,
            y: start_y + LABEL_DROP,
        },
    }
}

/// A lane route runs through the gaps between layers, which hold no cards: down
/// from the source's bottom centre into the gap below it, right to its lane
/// beyond the widest layer, to the gap above the target, left to the target's
/// top centre and down into it. Used by back edges and by forward edges that
/// skip a layer.
fn lane_route(source: Rect, target: Rect, start_x: f32, lane_x: f32, layer_gap: f32) -> Route {
    let end_x = target.center_x();
    let below = source.bottom() + layer_gap / 2.0;
    let above = target.y - layer_gap / 2.0;
    Route {
        points: vec![
            Point {
                x: start_x,
                y: source.bottom(),
            },
            Point {
                x: start_x,
                y: below,
            },
            Point {
                x: lane_x,
                y: below,
            },
            Point {
                x: lane_x,
                y: above,
            },
            Point { x: end_x, y: above },
            Point {
                x: end_x,
                y: target.y,
            },
        ],
        label: Point {
            x: start_x,
            y: source.bottom() + LABEL_DROP,
        },
    }
}

/// A back edge from a state to itself: out and back on the right of the card.
fn loop_route(rect: Rect, lane_x: f32) -> Route {
    let top = rect.y + rect.h * 0.35;
    let bottom = rect.y + rect.h * 0.65;
    Route {
        points: vec![
            Point {
                x: rect.right(),
                y: top,
            },
            Point { x: lane_x, y: top },
            Point {
                x: lane_x,
                y: bottom,
            },
            Point {
                x: rect.right(),
                y: bottom,
            },
        ],
        label: Point {
            x: lane_x,
            y: rect.center_y(),
        },
    }
}

fn stub(source: Rect, x: f32) -> Route {
    Route {
        points: vec![
            Point {
                x,
                y: source.bottom(),
            },
            Point {
                x,
                y: source.bottom() + STUB,
            },
        ],
        label: Point {
            x,
            y: source.bottom() + LABEL_DROP,
        },
    }
}

/// The width a label takes, from its anchor's offset to its right side.
#[must_use]
pub fn label_width(text: &str) -> f32 {
    let chars = f32::from(u16::try_from(text.chars().count()).unwrap_or(u16::MAX));
    LABEL_OFFSET + chars * CHAR_WIDTH + LABEL_PADDING
}

/// A count or a slot as a float. Layers and slots are far below `u16::MAX`;
/// a larger one saturates.
fn px(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows_page::{analyse, flow_rows};
    use std::fs;
    use std::path::Path;

    /// The sizes the view draws with, so the tests check the drawn layout.
    const METRICS: Metrics = Metrics {
        node_width: 200.0,
        node_height: 64.0,
        terminal_height: 28.0,
        layer_gap: 72.0,
        slot_gap: 40.0,
    };

    fn workflow(states: &str) -> String {
        format!(
            "schema = 1\nname = \"graph\"\ndescription = \"Test graph\"\n\n[trigger]\non = \"manual\"\n\n{states}"
        )
    }

    fn rows(source: &str) -> Vec<FlowRow> {
        let (definition, diagnostics) = analyse(source);
        let definition = definition.expect("the fixture parses");
        flow_rows(&definition, &diagnostics).0
    }

    fn node<'a>(graph: &'a Graph, id: &str) -> &'a Node {
        graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("no node {id}"))
    }

    fn layer_of(graph: &Graph, id: &str) -> usize {
        node(graph, id).layer
    }

    fn edges_between<'a>(graph: &'a Graph, from: &str, to: &str) -> Vec<&'a Edge> {
        graph
            .edges
            .iter()
            .filter(|edge| edge.from == from && edge.to.as_deref() == Some(to))
            .collect()
    }

    const LINEAR: &str = r#"
[[state]]
id = "fetch"
action = "exec"
with = { argv = ["git", "fetch"] }

[[state]]
id = "rebase"
action = "exec"
with = { argv = ["git", "rebase"] }

[[state]]
id = "push"
action = "exec"
with = { argv = ["git", "push"] }
"#;

    const BRANCH: &str = r#"
[[state]]
id = "decide"
action = "approve"
with = { summary = "Create the branch?" }
on.approved = "left"
on.rejected = "right"

[[state]]
id = "left"
action = "exec"
with = { argv = ["true"] }
on.done = "end.done"

[[state]]
id = "right"
action = "exec"
with = { argv = ["true"] }
on.done = "end.done"
"#;

    const RETRY: &str = r#"
[[state]]
id = "prepare"
action = "exec"
with = { argv = ["true"] }
on.done = "push"

[[state]]
id = "push"
action = "exec"
with = { argv = ["git", "push"] }
on.done = "end.done"
on.failed = { to = "prepare", limit = 3 }
"#;

    const NESTED: &str = r#"
[[state]]
id = "roots"
action = "stack.stale"

[[state]]
id = "each"
for_each = "${states.roots.output.roots}"
as = "root"
states = [
  { id = "plan", action = "restack.plan", with = { root = "${root}" } },
  { id = "apply", action = "restack.apply", with = { plan_id = "${states.plan.output.plan_id}" }, on.done = "end.done", on.stale = { to = "plan", limit = 3 }, on.conflict = "end.failed" },
]

[[state]]
id = "after"
action = "exec"
with = { argv = ["true"] }
"#;

    #[test]
    fn a_linear_workflow_stacks_its_states_above_the_terminals() {
        let graph = layout(&rows(&workflow(LINEAR)), "fetch");
        assert_eq!(layer_of(&graph, "fetch"), 0);
        assert_eq!(layer_of(&graph, "rebase"), 1);
        assert_eq!(layer_of(&graph, "push"), 2);
        assert_eq!(layer_of(&graph, "end.done"), 3);
        // The implicit failure of each state is not drawn, so no end.failed.
        assert!(graph.nodes.iter().all(|node| node.id != "end.failed"));
        assert_eq!(graph.nodes.len(), 4);
        assert!(graph.edges.iter().all(|edge| !edge.back && !edge.dangling));
        assert_eq!(edges_between(&graph, "fetch", "rebase").len(), 1);
        assert!(edges_between(&graph, "fetch", "rebase")[0].defaulted);
    }

    #[test]
    fn a_branch_puts_its_two_targets_side_by_side_in_file_order() {
        let graph = layout(&rows(&workflow(BRANCH)), "decide");
        assert_eq!(layer_of(&graph, "decide"), 0);
        assert_eq!(layer_of(&graph, "left"), 1);
        assert_eq!(layer_of(&graph, "right"), 1);
        assert_eq!(node(&graph, "left").slot, 0);
        assert_eq!(node(&graph, "right").slot, 1);
        assert_eq!(layer_of(&graph, "end.done"), 2);
        assert!(graph.nodes.iter().all(|node| node.id != "end.failed"));
        let approved = edges_between(&graph, "decide", "left");
        assert_eq!(approved[0].label, "approved");
    }

    #[test]
    fn a_defaulted_failure_to_end_failed_is_not_drawn_but_a_written_one_is() {
        // The `failed` of `push` is the implicit one; the `failed` of `build`
        // is written, so only it draws an edge to end.failed.
        let source = workflow(
            r#"
[[state]]
id = "build"
action = "exec"
with = { argv = ["make"] }
on.failed = "end.failed"

[[state]]
id = "push"
action = "exec"
with = { argv = ["git", "push"] }
"#,
        );
        let graph = layout(&rows(&source), "build");
        let written = edges_between(&graph, "build", "end.failed");
        assert_eq!(written.len(), 1);
        assert!(!written[0].defaulted);
        assert!(
            graph
                .edges
                .iter()
                .all(|edge| edge.from != "push" || edge.to.as_deref() != Some("end.failed"))
        );
        assert_eq!(edges_between(&graph, "push", "end.done").len(), 1);
    }

    #[test]
    fn a_layer_is_ordered_by_the_barycenter_of_its_predecessors() {
        // `P` and `Q` share layer 1 in file order. `b` follows `P` and `a`
        // follows `Q`, so `b` goes first even though `a` comes first in the file.
        let source = workflow(
            r#"
[[state]]
id = "start"
action = "exec"
with = { argv = ["true"] }
on.done = [
  { if = "${trigger.root} == 'x'", to = "P" },
  { to = "Q" },
]

[[state]]
id = "P"
action = "exec"
with = { argv = ["true"] }
on.done = "b"

[[state]]
id = "Q"
action = "exec"
with = { argv = ["true"] }
on.done = "a"

[[state]]
id = "a"
action = "exec"
with = { argv = ["true"] }
on.done = "end.done"

[[state]]
id = "b"
action = "exec"
with = { argv = ["true"] }
on.done = "end.done"
"#,
        );
        let graph = layout(&rows(&source), "start");
        assert_eq!(layer_of(&graph, "a"), 2);
        assert_eq!(layer_of(&graph, "b"), 2);
        assert_eq!(node(&graph, "b").slot, 0);
        assert_eq!(node(&graph, "a").slot, 1);
    }

    #[test]
    fn a_retry_loop_is_a_back_edge_routed_on_the_right() {
        let graph = layout(&rows(&workflow(RETRY)), "prepare");
        assert_eq!(layer_of(&graph, "prepare"), 0);
        assert_eq!(layer_of(&graph, "push"), 1);
        let retry = edges_between(&graph, "push", "prepare");
        assert_eq!(retry.len(), 1);
        assert!(retry[0].back);
        assert!(!retry[0].defaulted);
        assert_eq!(retry[0].label, "failed (limit 3)");
        let forward = edges_between(&graph, "prepare", "push");
        assert!(!forward[0].back);

        let geometry = geometry(&graph, &METRICS);
        let push = geometry.nodes[graph.nodes.iter().position(|n| n.id == "push").unwrap()];
        let route = &geometry.edges[graph.edges.iter().position(|e| e.back).unwrap()];
        let lane = route.points[2].x;
        // `push` has two outcomes, so the retry leaves its bottom side, not its centre.
        assert!(route.points[0].x > push.x && route.points[0].x < push.right());
        assert_eq!(route.points[0].y, push.bottom());
        assert!(lane > push.right());
        assert!(geometry.nodes.iter().all(|rect| rect.right() < lane));
    }

    #[test]
    fn back_edges_take_one_lane_each_on_the_right() {
        let source = workflow(
            r#"
[[state]]
id = "a"
action = "exec"
with = { argv = ["true"] }
on.done = "b"

[[state]]
id = "b"
action = "exec"
with = { argv = ["true"] }
on.done = "c"
on.failed = { to = "a", limit = 2 }

[[state]]
id = "c"
action = "exec"
with = { argv = ["true"] }
on.done = "end.done"
on.failed = { to = "a", limit = 2 }
"#,
        );
        let graph = layout(&rows(&source), "a");
        let back: Vec<usize> = graph
            .edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.back)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(back.len(), 2);
        let geometry = geometry(&graph, &METRICS);
        let first = geometry.edges[back[0]].points[2].x;
        let second = geometry.edges[back[1]].points[2].x;
        assert!(first < second, "the first back edge is the nearer lane");
        let body_right = geometry
            .nodes
            .iter()
            .map(|rect| rect.right())
            .fold(0.0_f32, f32::max);
        assert!(first > body_right);
        assert!(geometry.width > second);
    }

    #[test]
    fn terminals_exist_only_when_an_edge_reaches_them() {
        let source = workflow(
            r#"
[[state]]
id = "check"
action = "exec"
with = { argv = ["true"] }
on.done = [
  { if = "${trigger.root} == 'main'", to = "end.blocked", reason = "protected" },
  { to = "end.done" },
]
on.failed = "end.done"
on.timeout = "end.done"
"#,
        );
        let graph = layout(&rows(&source), "check");
        let ids: Vec<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, ["check", "end.blocked", "end.done"]);
        assert!(graph.nodes.iter().skip(1).all(|n| n.kind == NodeKind::End));
    }

    #[test]
    fn states_no_path_from_the_start_reaches_sit_below_the_main_flow() {
        let main = r#"
[[state]]
id = "fetch"
action = "exec"
with = { argv = ["git", "fetch"] }
on.done = "push"

[[state]]
id = "push"
action = "exec"
with = { argv = ["git", "push"] }
on.done = "end.done"
"#;
        let with_strays = format!(
            "{main}{}",
            r#"
[[state]]
id = "stray"
action = "exec"
with = { argv = ["true"] }
on.done = "cleanup"

[[state]]
id = "cleanup"
action = "exec"
with = { argv = ["true"] }
on.done = "push"
"#
        );
        let alone = layout(&rows(&workflow(main)), "fetch");
        let graph = layout(&rows(&workflow(&with_strays)), "fetch");
        // The main flow lays out as if the stray states were not there.
        for id in ["fetch", "push", "end.done"] {
            assert_eq!(node(&graph, id).layer, node(&alone, id).layer, "{id}");
            assert_eq!(node(&graph, id).slot, node(&alone, id).slot, "{id}");
        }
        let flow_depth = layer_of(&graph, "end.done");
        assert!(layer_of(&graph, "stray") > flow_depth);
        assert_eq!(
            layer_of(&graph, "cleanup"),
            layer_of(&graph, "stray") + 1,
            "the stray states keep their own order"
        );
        // Their edge into the main flow is a stub, and the edge between them is drawn.
        let into_main = edges_between(&graph, "cleanup", "push");
        assert!(into_main.is_empty());
        assert!(graph.edges.iter().any(|edge| edge.from == "cleanup"
            && edge.to.is_none()
            && edge.label == "done → push"));
        assert_eq!(edges_between(&graph, "stray", "cleanup").len(), 1);
    }

    #[test]
    fn an_unhandled_outcome_is_a_dangling_stub_with_no_target_node() {
        let source = workflow(
            r#"
[[state]]
id = "decide"
action = "approve"
with = { summary = "Go?" }
on.approved = "end.done"
"#,
        );
        let graph = layout(&rows(&source), "decide");
        assert!(graph.nodes.iter().all(|node| node.id != "unhandled"));
        let stub = graph
            .edges
            .iter()
            .find(|edge| edge.dangling)
            .expect("the rejected outcome dangles");
        assert_eq!(stub.to, None);
        assert_eq!(stub.label, "rejected → unhandled");
        assert!(!stub.back);

        let geometry = geometry(&graph, &METRICS);
        let index = graph.edges.iter().position(|edge| edge.dangling).unwrap();
        let route = &geometry.edges[index];
        let source_rect = geometry.nodes[0];
        assert_eq!(route.points.len(), 2);
        assert_eq!(route.points[0].y, source_rect.bottom());
        assert!(route.points[1].y > route.points[0].y);
        assert_eq!(route.label.x, route.points[0].x);
        assert!(route.label.y > route.points[0].y && route.label.y < route.points[1].y);
    }

    /// The graphs of the example workflows, laid out as the Flow lays them out.
    fn example_graphs() -> Vec<(String, Graph)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../gm-core/tests/workflows");
        let mut paths: Vec<_> = fs::read_dir(&dir)
            .expect("the workflow examples")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let name = path.display().to_string();
                let source = fs::read_to_string(&path).expect("readable example");
                let (definition, diagnostics) = analyse(&source);
                let definition = definition.unwrap_or_else(|| panic!("{name} does not parse"));
                let (rows, _) = flow_rows(&definition, &diagnostics);
                let start = rows[0].id.clone();
                (name, layout(&rows, &start))
            })
            .collect()
    }

    #[test]
    fn a_label_sits_beside_the_first_segment_just_below_its_source() {
        for (name, graph) in example_graphs() {
            let geometry = geometry(&graph, &METRICS);
            for (edge, route) in graph.edges.iter().zip(&geometry.edges) {
                let (first, second) = (route.points[0], route.points[1]);
                // A loop leaves its card sideways, and its label sits on its lane.
                if first.y.total_cmp(&second.y).is_eq() {
                    continue;
                }
                assert_eq!(route.label.x, first.x, "{name}: {} label", edge.from);
                assert!(
                    route.label.y > first.y && route.label.y < second.y,
                    "{name}: the label of {} is off its first segment",
                    edge.from
                );
            }
        }
    }

    #[test]
    fn the_scroll_area_holds_every_card_route_and_label_of_the_examples() {
        for (name, graph) in example_graphs() {
            let geometry = geometry(&graph, &METRICS);
            for rect in &geometry.nodes {
                assert!(
                    rect.right() <= geometry.width,
                    "{name}: a card leaves the area"
                );
                assert!(
                    rect.bottom() <= geometry.height,
                    "{name}: a card leaves the area"
                );
            }
            for (edge, route) in graph.edges.iter().zip(&geometry.edges) {
                for point in &route.points {
                    assert!(
                        point.x <= geometry.width && point.y <= geometry.height,
                        "{name}: {} route",
                        edge.from
                    );
                }
                let label_right = route.label.x + label_width(&edge.label);
                assert!(
                    label_right <= geometry.width,
                    "{name}: the label of {} leaves the area",
                    edge.from
                );
            }
        }
    }

    #[test]
    fn no_two_labels_of_the_examples_overlap_on_one_row() {
        for (name, graph) in example_graphs() {
            let geometry = geometry(&graph, &METRICS);
            let labels: Vec<(Point, &str)> = geometry
                .edges
                .iter()
                .zip(&graph.edges)
                .map(|(route, edge)| (route.label, edge.label.as_str()))
                .collect();
            for (index, &(first, first_text)) in labels.iter().enumerate() {
                for &(second, second_text) in &labels[index + 1..] {
                    if (first.y - second.y).abs() >= LABEL_HEIGHT {
                        continue;
                    }
                    let (left, left_text, right) = if first.x <= second.x {
                        (first, first_text, second)
                    } else {
                        (second, second_text, first)
                    };
                    assert!(
                        right.x + LABEL_OFFSET >= left.x + label_width(left_text),
                        "{name}: the labels {left_text:?} and {:?} overlap",
                        if first.x <= second.x {
                            second_text
                        } else {
                            first_text
                        },
                    );
                }
            }
        }
    }

    #[test]
    fn a_nested_for_each_is_attached_to_its_parent_and_not_drawn() {
        let graph = layout(&rows(&workflow(NESTED)), "roots");
        let each = node(&graph, "each");
        assert_eq!(each.children, ["plan", "apply"]);
        assert!(
            graph
                .nodes
                .iter()
                .all(|n| n.id != "plan" && n.id != "apply")
        );
        assert_eq!(layer_of(&graph, "each"), 1);
        assert_eq!(layer_of(&graph, "after"), 2);
        // The loop's own outcomes leave the parent card; the inner ones do not.
        let inner = graph
            .edges
            .iter()
            .filter(|edge| {
                edge.to.as_deref() == Some("plan") || edge.to.as_deref() == Some("apply")
            })
            .count();
        assert_eq!(inner, 0);
        assert_eq!(edges_between(&graph, "each", "end.done").len(), 1);
        assert!(
            graph
                .edges
                .iter()
                .all(|edge| edge.from != "plan" && edge.from != "apply")
        );
    }

    #[test]
    fn outcomes_that_reach_one_target_are_one_edge_with_each_outcome() {
        let source = workflow(
            r#"
[[state]]
id = "check"
action = "exec"
with = { argv = ["true"] }
on.failed = "end.failed"
on.timeout = "end.failed"
on.done = [
  { if = "${trigger.root} == 'main'", to = "end.failed" },
  { to = "end.done" },
]
"#,
        );
        let graph = layout(&rows(&source), "check");
        let to_failed = edges_between(&graph, "check", "end.failed");
        assert_eq!(to_failed.len(), 1);
        // Outcomes join in the order the state declares them: done, failed, timeout.
        assert_eq!(
            to_failed[0].label,
            "done if ${trigger.root} == 'main', failed, timeout"
        );
        assert!(!to_failed[0].defaulted);
        assert_eq!(edges_between(&graph, "check", "end.done").len(), 1);
    }

    #[test]
    fn the_same_input_gives_the_same_graph_and_geometry() {
        for source in [
            workflow(LINEAR),
            workflow(BRANCH),
            workflow(RETRY),
            workflow(NESTED),
        ] {
            let flow = rows(&source);
            let first = layout(&flow, "roots");
            let second = layout(&flow, "roots");
            assert_eq!(first, second);
            assert_eq!(geometry(&first, &METRICS), geometry(&second, &METRICS));
        }
    }

    /// Whether an axis-aligned segment passes through the interior of `rect`.
    /// Touching an edge or a corner does not count.
    fn crosses_interior(rect: Rect, a: Point, b: Point) -> bool {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        x0 < rect.right() && x1 > rect.x && y0 < rect.bottom() && y1 > rect.y
    }

    #[test]
    fn no_route_segment_crosses_the_interior_of_a_node() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../gm-core/tests/workflows");
        let mut paths: Vec<_> = fs::read_dir(&dir)
            .expect("the workflow examples")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path.display().to_string();
            let source = fs::read_to_string(&path).expect("readable example");
            let (definition, diagnostics) = analyse(&source);
            let definition = definition.unwrap_or_else(|| panic!("{name} does not parse"));
            let (rows, _) = flow_rows(&definition, &diagnostics);
            let start = rows[0].id.clone();
            let graph = layout(&rows, &start);
            let geometry = geometry(&graph, &METRICS);
            for (edge, route) in graph.edges.iter().zip(&geometry.edges) {
                for pair in route.points.windows(2) {
                    for (node, rect) in graph.nodes.iter().zip(&geometry.nodes) {
                        assert!(
                            !crosses_interior(*rect, pair[0], pair[1]),
                            "{name}: the route {} → {:?} crosses node {}",
                            edge.from,
                            edge.to,
                            node.id
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_example_lays_out_with_forward_edges_going_down_and_no_overlap() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../gm-core/tests/workflows");
        let mut paths: Vec<_> = fs::read_dir(&dir)
            .expect("the workflow examples")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no examples found in {}", dir.display());
        for path in paths {
            let name = path.display().to_string();
            let source = fs::read_to_string(&path).expect("readable example");
            let (definition, diagnostics) = analyse(&source);
            let Some(definition) = definition else {
                panic!("{name} does not parse");
            };
            let (rows, _) = flow_rows(&definition, &diagnostics);
            let start = rows[0].id.clone();
            let graph = layout(&rows, &start);
            let geometry = geometry(&graph, &METRICS);

            for (edge, route) in graph.edges.iter().zip(&geometry.edges) {
                if edge.back || edge.to.is_none() {
                    continue;
                }
                let to = layer_of(&graph, edge.to.as_deref().unwrap());
                let from = layer_of(&graph, &edge.from);
                assert!(
                    from < to,
                    "{name}: {} → {} goes against the layers",
                    edge.from,
                    to
                );
                let first = route.points[0];
                let last = route.points[route.points.len() - 1];
                assert!(first.y < last.y, "{name}: {} runs upward", edge.from);
            }
            for (a, first) in geometry.nodes.iter().enumerate() {
                for second in &geometry.nodes[a + 1..] {
                    let apart = first.right() <= second.x
                        || second.right() <= first.x
                        || first.bottom() <= second.y
                        || second.bottom() <= first.y;
                    assert!(apart, "{name}: two nodes overlap");
                }
            }
            for (index, node) in graph.nodes.iter().enumerate() {
                let clash = graph.nodes.iter().enumerate().any(|(other, rest)| {
                    other != index && rest.layer == node.layer && rest.slot == node.slot
                });
                assert!(!clash, "{name}: two nodes share a slot");
            }
        }
    }
}
