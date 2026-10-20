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
/// The horizontal distance between parallel edges of one pair of nodes.
const PARALLEL: f32 = 12.0;

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

/// One outcome of a state, drawn between two nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The id of the state the outcome leaves from.
    pub from: String,
    /// The id of the node it reaches. `None` for a dangling outcome, and for a
    /// target the graph does not hold; both are drawn as stubs.
    pub to: Option<String>,
    /// The outcome, then `if` and `limit` when the rule has them, as
    /// [`RowEdge::label`] writes it.
    pub label: String,
    /// The edge goes against the layers: a retry or a loop back to an earlier state.
    pub back: bool,
    pub defaulted: bool,
    pub dangling: bool,
    /// The place of this edge among the edges with the same `from` and `to`.
    pub parallel_index: usize,
    /// How many edges share that `from` and `to`, this one included.
    pub parallel_count: usize,
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
    /// The size the scroll area needs, margins included.
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
    let outcomes: Vec<Outcome> = row_outcomes
        .into_iter()
        .map(|(from, edge)| Outcome {
            from,
            to: resolve(edge, &states, &mut terminals, &mut nodes),
            edge,
        })
        .collect();
    let out = adjacency(nodes.len(), &outcomes);
    let back = back_edges(start, &out, &outcomes);
    let (layer, slot) = place(&out, &outcomes, &back);
    for (index, node) in nodes.iter_mut().enumerate() {
        node.layer = layer[index];
        node.slot = slot[index];
    }
    let edges = edges_of(&nodes, &outcomes, &back);
    Graph { nodes, edges }
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
            outcomes.extend(row.edges.iter().map(|edge| (nodes.len(), edge)));
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
fn place(out: &[Vec<usize>], outcomes: &[Outcome], back: &[bool]) -> (Vec<usize>, Vec<usize>) {
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

/// The graph's edges, with the back flags and the parallel positions.
fn edges_of(nodes: &[Node], outcomes: &[Outcome], back: &[bool]) -> Vec<Edge> {
    let mut totals: BTreeMap<(usize, Option<usize>), usize> = BTreeMap::new();
    for outcome in outcomes {
        *totals.entry((outcome.from, outcome.to)).or_insert(0) += 1;
    }
    let mut seen: BTreeMap<(usize, Option<usize>), usize> = BTreeMap::new();
    outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            let key = (outcome.from, outcome.to);
            let counter = seen.entry(key).or_insert(0);
            let parallel_index = *counter;
            *counter += 1;
            Edge {
                from: nodes[outcome.from].id.clone(),
                to: outcome.to.map(|to| nodes[to].id.clone()),
                label: outcome.edge.label(),
                back: back[index],
                defaulted: outcome.edge.defaulted,
                dangling: outcome.edge.dangling,
                parallel_index,
                parallel_count: totals[&key],
            }
        })
        .collect()
}

/// Positions the graph in pixels. Each layer is centred on the widest one.
/// Forward edges to the next layer run down, across and down. Back edges, and
/// forward edges that skip a layer, run through the gaps between layers and out
/// to a lane beyond the widest layer, each on its own lane. A dangling outcome
/// is a short stub below its source. Parallel edges of one pair are spread
/// horizontally.
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
    let mut lanes = 0;
    let mut routes = Vec::with_capacity(graph.edges.len());
    for edge in &graph.edges {
        let source = rects[index[edge.from.as_str()]];
        let spread = spread(edge.parallel_index, edge.parallel_count);
        let route = match edge.to.as_deref() {
            None => stub(source, spread),
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
                    lane_route(source, rects[index[to]], lane_x, spread, metrics.layer_gap)
                }
            }
            Some(to) => forward_route(source, rects[index[to]], spread),
        };
        routes.push(route);
    }
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

fn forward_route(source: Rect, target: Rect, spread: f32) -> Route {
    let start_x = source.center_x() + spread;
    let end_x = target.center_x() + spread;
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
            x: start_x.midpoint(end_x),
            y: middle,
        },
    }
}

/// A lane route runs through the gaps between layers, which hold no cards: down
/// from the source's bottom centre into the gap below it, right to its lane
/// beyond the widest layer, to the gap above the target, left to the target's
/// top centre and down into it. Used by back edges and by forward edges that
/// skip a layer.
fn lane_route(source: Rect, target: Rect, lane_x: f32, spread: f32, layer_gap: f32) -> Route {
    let start_x = source.center_x() + spread;
    let end_x = target.center_x() + spread;
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
            x: lane_x,
            y: below.midpoint(above),
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

fn stub(source: Rect, spread: f32) -> Route {
    let x = source.center_x() + spread;
    let end = source.bottom() + STUB;
    let end_point = Point { x, y: end };
    Route {
        points: vec![
            Point {
                x,
                y: source.bottom(),
            },
            end_point,
        ],
        label: end_point,
    }
}

/// The horizontal offset of the edge at `index` among `count` parallel edges,
/// centred on zero.
fn spread(index: usize, count: usize) -> f32 {
    (px(index) - (px(count) - 1.0) / 2.0) * PARALLEL
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

    const METRICS: Metrics = Metrics {
        node_width: 200.0,
        node_height: 96.0,
        terminal_height: 28.0,
        layer_gap: 48.0,
        slot_gap: 32.0,
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
        assert_eq!(layer_of(&graph, "end.failed"), 3);
        assert_eq!(graph.nodes.len(), 5);
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
        // The deepest node reaching end.failed is a branch target, on layer 1.
        assert_eq!(layer_of(&graph, "end.failed"), 2);
        let approved = edges_between(&graph, "decide", "left");
        assert_eq!(approved[0].label, "approved → left");
    }

    #[test]
    fn a_layer_is_ordered_by_the_barycenter_of_its_predecessors() {
        // `b` follows the first state and `a` follows the second, so `b` goes
        // first even though `a` comes first in the file.
        let source = workflow(
            r#"
[[state]]
id = "start"
action = "exec"
with = { argv = ["true"] }
on.done = "b"

[[state]]
id = "other"
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
        assert_eq!(layer_of(&graph, "a"), 1);
        assert_eq!(layer_of(&graph, "b"), 1);
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
        assert_eq!(retry[0].label, "failed → prepare (limit 3)");
        let forward = edges_between(&graph, "prepare", "push");
        assert!(!forward[0].back);

        let geometry = geometry(&graph, &METRICS);
        let push = geometry.nodes[graph.nodes.iter().position(|n| n.id == "push").unwrap()];
        let route = &geometry.edges[graph.edges.iter().position(|e| e.back).unwrap()];
        let lane = route.points[2].x;
        assert_eq!(route.points[0].x, push.center_x());
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
        assert_eq!(route.label, route.points[1]);
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
    fn parallel_edges_of_one_pair_are_kept_apart_and_spread() {
        let source = workflow(
            r#"
[[state]]
id = "decide"
action = "approve"
with = { summary = "Go?" }
on.approved = [
  { if = "${trigger.root} == 'main'", to = "end.done" },
  { to = "end.done" },
]
on.rejected = "end.failed"
"#,
        );
        let graph = layout(&rows(&source), "decide");
        let pair = edges_between(&graph, "decide", "end.done");
        assert_eq!(pair.len(), 2);
        assert_eq!(
            pair.iter()
                .map(|e| (e.parallel_index, e.parallel_count))
                .collect::<Vec<_>>(),
            [(0, 2), (1, 2)]
        );
        let geometry = geometry(&graph, &METRICS);
        let first = graph
            .edges
            .iter()
            .position(|e| std::ptr::eq(e, pair[0]))
            .unwrap();
        let second = graph
            .edges
            .iter()
            .position(|e| std::ptr::eq(e, pair[1]))
            .unwrap();
        assert!(geometry.edges[first].points[0].x < geometry.edges[second].points[0].x);
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
