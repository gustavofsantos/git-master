//! The lanes of the history graph (H5, D36, D65).
//!
//! Plain data in, plain data out: no GPUI types, so the layout is tested
//! without a window. The input is the commits in `--topo-order`, children
//! before parents, each with its parents. The output is one [`GraphRow`] per
//! commit: the column of its node and the lines the row draws.
//!
//! The layout is one pass from the top, keeping a list of *lanes*. A lane is a
//! column that waits for one commit, the next of its line of history.
//! - A commit takes the leftmost lane waiting for it; the other lanes waiting
//!   for it end at its node. A commit nothing waits for takes the leftmost free
//!   column.
//! - Its first parent stays in the node's lane. When another lane already waits
//!   for that parent, the leftmost of the two wins: the node joins a lane on
//!   its left, and a lane on its right shifts left into the node's column.
//! - Every other parent joins the lane already waiting for it, or opens a lane
//!   in the leftmost free column.
//!
//! The pass has no look-ahead, so the layout of a page extends the layout of
//! the pages before it, and the rows of a commit never change when more commits
//! are read. The lanes left open at the end of a page are the state the next
//! page starts from.
//!
//! A lane's colour is fixed where it starts. A tip with a branch label takes
//! the colour of that name (`main`, `master` and `trunk` are the trunk colour);
//! any other lane takes the colour of the oid it starts waiting for. Both are
//! a hash with a fixed definition, so a colour is the same in every session.

use gm_schema::{CommitSummary, RefKind};

use crate::theme::GRAPH_SLOTS;
use crate::worktree_list::name_hash;

/// How many columns the list draws. A row that uses more shows `+N`.
pub const MAX_COLUMNS: usize = 8;

/// The colour of a lane, before a theme turns it into a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    /// `main`, `master` and `trunk`: the quiet colour of the main line.
    Trunk,
    /// One of `GRAPH_SLOTS` accents.
    Slot(usize),
}

/// A line at the edge of a row: the column it is in and its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub column: usize,
    pub tint: Tint,
}

/// A line that crosses a row from one column at the top to another at the
/// bottom: a lane that joins the node's lane, on the node's right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    pub from: usize,
    pub to: usize,
    pub tint: Tint,
}

/// What one row draws. The row is cut in two halves at the node: lines above
/// it reach the node or go past it, lines below it leave the node or go on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The column of the node.
    pub column: usize,
    pub tint: Tint,
    /// A line reaches the node from the top edge: some lane waited for it.
    pub incoming: bool,
    /// Lanes that go straight through the row, top to bottom.
    pub through: Vec<Link>,
    /// Other lanes that wait for this commit: they end at the node, coming
    /// from the top edge at their column.
    pub converging: Vec<Link>,
    /// Lines from the node to the bottom edge, one per parent, at the column
    /// of the lane that parent is in.
    pub outgoing: Vec<Link>,
    /// Lanes that cross the row from the top edge to a column on their left.
    pub shifts: Vec<Shift>,
    /// How many columns the row uses: the rightmost one drawn, plus one.
    pub columns: usize,
}

/// A column waiting for a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Lane {
    expects: String,
    tint: Tint,
}

/// The graph of the commits read so far, and the lanes still open below them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphLayout {
    rows: Vec<GraphRow>,
    lanes: Vec<Option<Lane>>,
    columns: usize,
}

impl GraphLayout {
    /// The layout of `commits`, in one pass.
    #[must_use]
    pub fn from_commits(commits: &[CommitSummary]) -> Self {
        let mut layout = Self::default();
        layout.extend(commits);
        layout
    }

    /// Adds the next commits of the walk, which follow the ones laid out.
    pub fn extend(&mut self, commits: &[CommitSummary]) {
        for commit in commits {
            let row = self.place(commit);
            self.columns = self.columns.max(row.columns);
            self.rows.push(row);
        }
    }

    /// The row of the commit at `index` in the list.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<&GraphRow> {
        self.rows.get(index)
    }

    /// The most columns any row uses, capped at `MAX_COLUMNS`: the width the
    /// graph column of the list takes.
    #[must_use]
    pub fn width_columns(&self) -> usize {
        self.columns.min(MAX_COLUMNS)
    }

    /// Whether some row is wider than `MAX_COLUMNS`, so the list has to cut it.
    #[must_use]
    pub fn overflows(&self) -> bool {
        self.columns > MAX_COLUMNS
    }

    fn place(&mut self, commit: &CommitSummary) -> GraphRow {
        let waiting: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.as_ref().is_some_and(|l| l.expects == commit.oid))
            .map(|(column, _)| column)
            .collect();
        let incoming = !waiting.is_empty();
        let (column, tint) = match waiting.first() {
            Some(&column) => (column, self.lane_tint(column)),
            None => (self.free_column(), tip_tint(commit)),
        };

        let mut through: Vec<Link> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(at, _)| !waiting.contains(at))
            .filter_map(|(at, lane)| {
                lane.as_ref().map(|lane| Link {
                    column: at,
                    tint: lane.tint,
                })
            })
            .collect();
        let converging: Vec<Link> = waiting[waiting.len().min(1)..]
            .iter()
            .map(|&at| Link {
                column: at,
                tint: self.lane_tint(at),
            })
            .collect();
        for &at in &waiting {
            self.lanes[at] = None;
        }

        let (outgoing, shifts) = self.link_parents(commit, column, tint, &mut through);
        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        let widest = through
            .iter()
            .chain(&converging)
            .chain(&outgoing)
            .map(|link| link.column)
            .chain(shifts.iter().flat_map(|shift| [shift.from, shift.to]))
            .fold(column, usize::max);
        GraphRow {
            column,
            tint,
            incoming,
            through,
            converging,
            outgoing,
            shifts,
            columns: widest + 1,
        }
    }

    /// Connects the node at `column` to its parents: opens, joins or shifts
    /// lanes, and returns the lines that leave the node and the lanes that
    /// shift into its column. A shifted lane leaves `through`.
    fn link_parents(
        &mut self,
        commit: &CommitSummary,
        column: usize,
        tint: Tint,
        through: &mut Vec<Link>,
    ) -> (Vec<Link>, Vec<Shift>) {
        let mut outgoing = Vec::new();
        let mut shifts = Vec::new();
        let mut parents: Vec<&str> = Vec::new();
        for parent in &commit.parents {
            if parents.contains(&parent.as_str()) {
                continue;
            }
            parents.push(parent);
            let existing = self.waiting_for(parent);
            if parents.len() > 1 {
                if let Some(at) = existing {
                    outgoing.push(Link {
                        column: at,
                        tint: self.lane_tint(at),
                    });
                } else {
                    let at = self.free_column();
                    let tint = Tint::Slot(slot_of(parent));
                    self.set_lane(at, parent, tint);
                    outgoing.push(Link { column: at, tint });
                }
                continue;
            }
            match existing {
                Some(at) if at < column => outgoing.push(Link {
                    column: at,
                    tint: self.lane_tint(at),
                }),
                Some(at) => {
                    // The lane on the right shifts into the node's column.
                    shifts.push(Shift {
                        from: at,
                        to: column,
                        tint: self.lane_tint(at),
                    });
                    through.retain(|link| link.column != at);
                    self.lanes[at] = None;
                    self.set_lane(column, parent, tint);
                    outgoing.push(Link { column, tint });
                }
                None => {
                    self.set_lane(column, parent, tint);
                    outgoing.push(Link { column, tint });
                }
            }
        }
        (outgoing, shifts)
    }

    fn lane_tint(&self, column: usize) -> Tint {
        self.lanes[column]
            .as_ref()
            .map_or(Tint::Slot(0), |lane| lane.tint)
    }

    fn waiting_for(&self, oid: &str) -> Option<usize> {
        self.lanes
            .iter()
            .position(|lane| lane.as_ref().is_some_and(|l| l.expects == oid))
    }

    /// The leftmost column no lane holds. Past the end when all are held.
    fn free_column(&self) -> usize {
        self.lanes
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.lanes.len())
    }

    fn set_lane(&mut self, column: usize, expects: &str, tint: Tint) {
        if column >= self.lanes.len() {
            self.lanes.resize(column + 1, None);
        }
        self.lanes[column] = Some(Lane {
            expects: expects.to_owned(),
            tint,
        });
    }
}

#[cfg(test)]
impl GraphLayout {
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// How many lanes still wait for a commit below the last row: the lines
    /// that run off the bottom of the list.
    fn open_lanes(&self) -> usize {
        self.lanes.iter().flatten().count()
    }
}

/// The colour of a lane that starts at `commit`: the branch it is the tip of,
/// else the commit itself.
fn tip_tint(commit: &CommitSummary) -> Tint {
    commit
        .refs
        .iter()
        .find(|label| label.kind == RefKind::Branch)
        .map_or_else(
            || Tint::Slot(slot_of(&commit.oid)),
            |label| named_tint(&label.name),
        )
}

/// The colour of a branch name. The remote form (`origin/main`) is the trunk
/// too, because only its last segment is looked at.
#[must_use]
pub fn named_tint(name: &str) -> Tint {
    let last = name.rsplit('/').next().unwrap_or(name);
    if matches!(last, "main" | "master" | "trunk") {
        Tint::Trunk
    } else {
        Tint::Slot(slot_of(name))
    }
}

fn slot_of(key: &str) -> usize {
    usize::try_from(name_hash(key) % GRAPH_SLOTS as u64).unwrap_or(0)
}

/// The width of one column of the graph, in pixels.
pub const COLUMN_WIDTH: f32 = 14.0;
/// The room for the `+N` label, when some row is wider than `MAX_COLUMNS`.
pub const OVERFLOW_WIDTH: f32 = 26.0;

/// One line of a row, in pixels from the row's top left corner. A line whose
/// ends are in the same column is straight; any other is an S-curve with
/// vertical ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub tint: Tint,
}

/// What the graph column of one row paints.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub lines: Vec<Line>,
    /// The centre of the node, or `None` when its column is cut off.
    pub node: Option<(f32, f32)>,
}

/// A column count as a float. The graph never has anywhere near 65 thousand.
fn as_f32(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

/// The horizontal centre of `column`.
#[must_use]
pub fn column_x(column: usize) -> f32 {
    as_f32(column) * COLUMN_WIDTH + COLUMN_WIDTH / 2.0
}

/// The width of the graph column for a layout: its columns, and room for the
/// `+N` label when a row is cut off.
#[must_use]
pub fn graph_width(layout: &GraphLayout) -> f32 {
    let overflow = if layout.overflows() {
        OVERFLOW_WIDTH
    } else {
        0.0
    };
    as_f32(layout.width_columns()) * COLUMN_WIDTH + overflow
}

/// The lines and the node of `row`, for a row `height` pixels tall. A line
/// with an end in a column past `MAX_COLUMNS` is left out.
#[must_use]
pub fn drawing(row: &GraphRow, height: f32) -> Drawing {
    let middle = height / 2.0;
    let mut lines = Vec::new();
    let mut add = |from: (usize, f32), to: (usize, f32), tint: Tint| {
        if from.0 < MAX_COLUMNS && to.0 < MAX_COLUMNS {
            lines.push(Line {
                from: (column_x(from.0), from.1),
                to: (column_x(to.0), to.1),
                tint,
            });
        }
    };
    for link in &row.through {
        add((link.column, 0.0), (link.column, height), link.tint);
    }
    if row.incoming {
        add((row.column, 0.0), (row.column, middle), row.tint);
    }
    for link in &row.converging {
        add((link.column, 0.0), (row.column, middle), link.tint);
    }
    for link in &row.outgoing {
        add((row.column, middle), (link.column, height), link.tint);
    }
    for shift in &row.shifts {
        add((shift.from, 0.0), (shift.to, height), shift.tint);
    }
    Drawing {
        lines,
        node: (row.column < MAX_COLUMNS).then(|| (column_x(row.column), middle)),
    }
}

/// What the graph column shows for columns past `MAX_COLUMNS`: `+N`, or
/// nothing when the row fits.
#[must_use]
pub fn overflow_label(row: &GraphRow) -> Option<String> {
    (row.columns > MAX_COLUMNS).then(|| format!("+{}", row.columns - MAX_COLUMNS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::RefLabel;
    use proptest::prelude::*;

    fn commit(oid: &str, parents: &[&str]) -> CommitSummary {
        CommitSummary {
            oid: oid.into(),
            parents: parents.iter().map(|&p| p.into()).collect(),
            short: oid.into(),
            subject: oid.into(),
            refs: Vec::new(),
            author_time: 0,
        }
    }

    fn labelled(mut commit: CommitSummary, branch: &str) -> CommitSummary {
        commit.refs.push(RefLabel {
            name: branch.into(),
            kind: RefKind::Branch,
        });
        commit
    }

    fn columns(layout: &GraphLayout) -> Vec<usize> {
        (0..layout.len())
            .map(|i| layout.row(i).unwrap().column)
            .collect()
    }

    #[test]
    fn a_linear_history_is_one_column() {
        let layout = GraphLayout::from_commits(&[
            commit("c", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);

        assert_eq!(columns(&layout), [0, 0, 0]);
        let top = layout.row(0).unwrap();
        assert!(!top.incoming);
        assert_eq!(top.outgoing.len(), 1);
        assert!(layout.row(1).unwrap().incoming);
        let root = layout.row(2).unwrap();
        assert!(root.incoming);
        assert!(root.outgoing.is_empty());
        assert_eq!(layout.open_lanes(), 0);
    }

    #[test]
    fn a_fork_and_a_merge_open_and_close_a_second_lane() {
        // The order of `git log --topo-order`: the merge, the merged branch,
        // the main line, the base.
        let layout = GraphLayout::from_commits(&[
            commit("m", &["main", "feat"]),
            commit("feat", &["base"]),
            commit("main", &["base"]),
            commit("base", &[]),
        ]);

        let merge = layout.row(0).unwrap();
        assert_eq!(merge.column, 0);
        assert_eq!(
            merge.outgoing.iter().map(|l| l.column).collect::<Vec<_>>(),
            [0, 1]
        );
        let feat = layout.row(1).unwrap();
        assert_eq!(feat.column, 1);
        assert_eq!(
            feat.through.iter().map(|l| l.column).collect::<Vec<_>>(),
            [0]
        );
        // "main" joins the lane already waiting for the base, on its right:
        // the leftmost wins, so that lane shifts left into column 0.
        let main = layout.row(2).unwrap();
        assert_eq!(main.column, 0);
        assert_eq!(
            main.shifts,
            [Shift {
                from: 1,
                to: 0,
                tint: feat.tint
            }]
        );
        assert!(main.through.is_empty());
        let base = layout.row(3).unwrap();
        assert_eq!(base.column, 0);
        assert!(base.converging.is_empty());
        assert_eq!(layout.open_lanes(), 0);
    }

    #[test]
    fn a_node_joins_a_lane_on_its_left() {
        // "b" is the first parent of both "x" (column 0) and "y" (column 1):
        // the lane of "y" ends in the lane of "x".
        let layout = GraphLayout::from_commits(&[
            commit("x", &["b"]),
            commit("y", &["b"]),
            commit("b", &[]),
        ]);

        assert_eq!(columns(&layout), [0, 1, 0]);
        let y = layout.row(1).unwrap();
        assert_eq!(
            y.outgoing,
            [Link {
                column: 0,
                tint: layout.row(0).unwrap().tint
            }]
        );
        assert_eq!(layout.open_lanes(), 0);
    }

    #[test]
    fn two_lanes_waiting_for_one_commit_end_at_its_node() {
        let layout = GraphLayout::from_commits(&[
            commit("m", &["a", "b"]),
            commit("a", &["c"]),
            commit("b", &["c"]),
            commit("c", &[]),
        ]);

        // "a" keeps column 0 for "c"; "b" in column 1 joins it on its left.
        assert_eq!(columns(&layout), [0, 0, 1, 0]);
        assert_eq!(layout.row(3).unwrap().converging, []);
        assert_eq!(layout.row(2).unwrap().outgoing[0].column, 0);
    }

    #[test]
    fn an_octopus_merge_opens_a_lane_per_extra_parent() {
        let layout = GraphLayout::from_commits(&[commit("o", &["a", "b", "c", "d"])]);

        let row = layout.row(0).unwrap();
        assert_eq!(
            row.outgoing.iter().map(|l| l.column).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(row.columns, 4);
        assert_eq!(layout.open_lanes(), 4);
    }

    #[test]
    fn two_roots_are_two_separate_lines() {
        let layout = GraphLayout::from_commits(&[
            commit("m", &["a", "b"]),
            commit("a", &[]),
            commit("b", &[]),
        ]);

        assert_eq!(columns(&layout), [0, 0, 1]);
        assert_eq!(layout.open_lanes(), 0);
    }

    #[test]
    fn a_parent_beyond_the_page_leaves_its_lane_open() {
        let layout = GraphLayout::from_commits(&[commit("c", &["b"]), commit("x", &["y"])]);

        assert_eq!(layout.open_lanes(), 2);
        assert_eq!(layout.row(1).unwrap().column, 1);
    }

    #[test]
    fn a_duplicate_parent_is_drawn_once() {
        let layout = GraphLayout::from_commits(&[commit("c", &["a", "a"]), commit("a", &[])]);

        assert_eq!(layout.row(0).unwrap().outgoing.len(), 1);
        assert_eq!(layout.open_lanes(), 0);
    }

    #[test]
    fn an_empty_list_has_no_rows_and_no_width() {
        let layout = GraphLayout::from_commits(&[]);

        assert!(layout.is_empty());
        assert_eq!(layout.width_columns(), 0);
        assert!(layout.row(0).is_none());
    }

    #[test]
    fn the_width_is_capped_and_a_wider_row_says_how_much_is_cut() {
        let parents: Vec<String> = (0..12).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = parents.iter().map(String::as_str).collect();
        let layout = GraphLayout::from_commits(&[commit("o", &refs)]);

        assert_eq!(layout.width_columns(), MAX_COLUMNS);
        assert_eq!(overflow_label(layout.row(0).unwrap()), Some("+4".into()));
        let narrow = GraphLayout::from_commits(&[commit("a", &[])]);
        assert_eq!(overflow_label(narrow.row(0).unwrap()), None);
    }

    #[test]
    fn a_lane_keeps_its_colour_from_where_it_starts() {
        let layout = GraphLayout::from_commits(&[
            labelled(commit("c", &["b"]), "feature/x"),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);

        let tint = layout.row(0).unwrap().tint;
        assert_eq!(tint, named_tint("feature/x"));
        assert_eq!(layout.row(1).unwrap().tint, tint);
        assert_eq!(layout.row(2).unwrap().tint, tint);
    }

    #[test]
    fn main_master_and_trunk_are_the_trunk_colour_also_as_remotes() {
        for name in ["main", "master", "trunk", "origin/main", "upstream/master"] {
            assert_eq!(named_tint(name), Tint::Trunk, "{name}");
        }
        assert_ne!(named_tint("domain"), Tint::Trunk);
        assert_ne!(named_tint("feature/main-menu"), Tint::Trunk);
    }

    #[test]
    fn a_named_colour_is_a_fixed_slot() {
        // FNV-1a is fixed by its definition, so these never change between
        // builds or sessions.
        assert_eq!(named_tint("feature/x"), named_tint("feature/x"));
        let Tint::Slot(slot) = named_tint("feature/x") else {
            panic!("not a trunk name");
        };
        assert!(slot < GRAPH_SLOTS);
        assert_eq!(
            slot,
            usize::try_from(name_hash("feature/x") % GRAPH_SLOTS as u64).unwrap()
        );
    }

    #[test]
    fn a_lane_with_no_branch_takes_the_colour_of_its_oid() {
        let layout = GraphLayout::from_commits(&[
            commit("m", &["a", "side"]),
            commit("a", &[]),
            commit("side", &[]),
        ]);

        assert_eq!(
            layout.row(0).unwrap().outgoing[1].tint,
            Tint::Slot(slot_of("side"))
        );
        assert_eq!(layout.row(2).unwrap().tint, Tint::Slot(slot_of("side")));
    }

    #[test]
    fn a_through_lane_is_a_vertical_line_the_height_of_the_row() {
        let layout = GraphLayout::from_commits(&[
            commit("m", &["a", "b"]),
            commit("b", &["c"]),
            commit("a", &["c"]),
            commit("c", &[]),
        ]);

        let drawn = drawing(layout.row(1).unwrap(), 48.0);

        assert!(
            drawn
                .lines
                .iter()
                .any(|line| line.from == (column_x(0), 0.0) && line.to == (column_x(0), 48.0))
        );
        assert_eq!(drawn.node, Some((column_x(1), 24.0)));
    }

    #[test]
    fn a_fork_leaves_the_node_and_reaches_the_bottom_edge_in_its_own_column() {
        let layout = GraphLayout::from_commits(&[commit("m", &["a", "b"])]);

        let drawn = drawing(layout.row(0).unwrap(), 48.0);

        assert_eq!(drawn.lines.len(), 2);
        assert_eq!(drawn.lines[0].from, (column_x(0), 24.0));
        assert_eq!(drawn.lines[0].to, (column_x(0), 48.0));
        assert_eq!(drawn.lines[1].to, (column_x(1), 48.0));
    }

    #[test]
    fn a_line_with_an_end_past_the_visible_columns_is_left_out() {
        let parents: Vec<String> = (0..12).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = parents.iter().map(String::as_str).collect();
        let layout = GraphLayout::from_commits(&[commit("o", &refs)]);

        let drawn = drawing(layout.row(0).unwrap(), 48.0);

        assert_eq!(drawn.lines.len(), MAX_COLUMNS);
        assert!(layout.overflows());
        assert_eq!(
            graph_width(&layout),
            as_f32(MAX_COLUMNS) * COLUMN_WIDTH + OVERFLOW_WIDTH
        );
    }

    #[test]
    fn a_node_past_the_visible_columns_is_not_drawn() {
        let parents: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = parents.iter().map(String::as_str).collect();
        let mut commits = vec![commit("o", &refs)];
        commits.push(commit("p9", &[]));
        let layout = GraphLayout::from_commits(&commits);

        assert!(layout.row(1).unwrap().column >= MAX_COLUMNS);
        assert_eq!(drawing(layout.row(1).unwrap(), 48.0).node, None);
    }

    /// A random history in walk order: parents always come later in the list.
    fn history(shape: &[(u8, [u8; 3])]) -> Vec<CommitSummary> {
        let count = shape.len();
        let oid = |index: usize| format!("{index:040x}");
        shape
            .iter()
            .enumerate()
            .map(|(index, &(kind, picks))| {
                let mut parents: Vec<String> = Vec::new();
                if index + 1 < count {
                    let wanted = match kind % 8 {
                        0 => 0,
                        1..=5 => 1,
                        6 => 2,
                        _ => 3,
                    };
                    let room = count - index - 1;
                    for pick in picks.iter().take(wanted) {
                        let parent = oid(index + 1 + usize::from(*pick) % room);
                        if !parents.contains(&parent) {
                            parents.push(parent);
                        }
                    }
                    if parents.is_empty() {
                        parents.push(oid(index + 1));
                    }
                }
                CommitSummary {
                    oid: oid(index),
                    parents,
                    short: String::new(),
                    subject: String::new(),
                    refs: Vec::new(),
                    author_time: 0,
                }
            })
            .collect()
    }

    /// Follows the line of the edge `from_row` → `to_row`, row by row, and
    /// fails when it is not drawn all the way.
    fn edge_is_drawn(layout: &GraphLayout, from_row: usize, link: Link, to_row: usize) -> bool {
        let mut column = link.column;
        for index in from_row + 1..to_row {
            let row = layout.row(index).unwrap();
            if row.through.iter().any(|l| l.column == column) {
                continue;
            }
            match row.shifts.iter().find(|shift| shift.from == column) {
                Some(shift) => column = shift.to,
                None => return false,
            }
        }
        let end = layout.row(to_row).unwrap();
        end.incoming && (end.column == column || end.converging.iter().any(|l| l.column == column))
    }

    fn parent_row(commits: &[CommitSummary], oid: &str) -> usize {
        commits.iter().position(|c| c.oid == oid).unwrap()
    }

    /// The commits of `dir`'s HEAD, read the way the history page reads them.
    fn read_history(dir: &std::path::Path) -> Vec<CommitSummary> {
        use gm_core::GitPort;
        use gm_core::git::SubprocessGit;
        let git = SubprocessGit::default();
        let root = git.resolve_root(dir).unwrap();
        let mut stream = git
            .history_stream(&root)
            .unwrap()
            .expect("HEAD has commits");
        stream.next_page(usize::MAX).unwrap().commits
    }

    fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn commit_file(dir: &std::path::Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        run_git(dir, &["add", file]);
        run_git(dir, &["commit", "-q", "-m", file]);
    }

    #[test]
    fn the_graph_of_a_real_repository_with_a_merge_draws_every_edge() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        commit_file(dir.path(), "base");
        run_git(dir.path(), &["checkout", "-q", "-b", "feature"]);
        commit_file(dir.path(), "feature-1");
        commit_file(dir.path(), "feature-2");
        run_git(dir.path(), &["checkout", "-q", "main"]);
        commit_file(dir.path(), "main-1");
        run_git(
            dir.path(),
            &["merge", "-q", "--no-ff", "-m", "merge", "feature"],
        );
        run_git(dir.path(), &["checkout", "-q", "-b", "side", "main~1"]);
        commit_file(dir.path(), "side-1");
        run_git(dir.path(), &["checkout", "-q", "main"]);
        run_git(
            dir.path(),
            &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
        );

        let commits = read_history(dir.path());
        let layout = GraphLayout::from_commits(&commits);

        assert_eq!(layout.len(), commits.len());
        assert_eq!(layout.open_lanes(), 0);
        for (index, commit) in commits.iter().enumerate() {
            let row = layout.row(index).unwrap();
            assert_eq!(row.outgoing.len(), commit.parents.len());
            for (link, parent) in row.outgoing.iter().zip(&commit.parents) {
                assert!(
                    edge_is_drawn(&layout, index, *link, parent_row(&commits, parent)),
                    "{} -> {parent}",
                    commit.subject
                );
            }
        }
        // HEAD is on `main`: the first row starts the trunk lane, and the
        // root ends it, so the whole main line is one colour.
        assert_eq!(layout.row(0).unwrap().tint, Tint::Trunk);
        assert_eq!(layout.row(0).unwrap().column, 0);
        let root = layout.row(commits.len() - 1).unwrap();
        assert_eq!(root.column, 0);
        assert!(root.outgoing.is_empty());
        assert!(layout.width_columns() >= 2);
    }

    #[test]
    fn laying_out_a_long_history_takes_a_blink() {
        // 20 thousand commits with a merge every seventh: a loose bound, run
        // in a debug build, that a release build beats by an order of magnitude.
        let count = 20_000;
        let oid = |index: usize| format!("{index:040x}");
        let commits: Vec<CommitSummary> = (0..count)
            .map(|index| {
                let mut parents = Vec::new();
                if index + 1 < count {
                    parents.push(oid(index + 1));
                }
                if index % 7 == 0 && index + 5 < count {
                    parents.push(oid(index + 5));
                }
                CommitSummary {
                    oid: oid(index),
                    parents,
                    short: String::new(),
                    subject: String::new(),
                    refs: Vec::new(),
                    author_time: 0,
                }
            })
            .collect();

        let started = std::time::Instant::now();
        let layout = GraphLayout::from_commits(&commits);
        let elapsed = started.elapsed();

        eprintln!("laid out {count} commits in {elapsed:?}");
        assert_eq!(layout.len(), count);
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    proptest! {
        #[test]
        fn every_commit_has_a_row_and_every_parent_edge_is_drawn_once_to_the_end(
            shape in prop::collection::vec((any::<u8>(), any::<[u8; 3]>()), 1..60),
        ) {
            let commits = history(&shape);
            let layout = GraphLayout::from_commits(&commits);

            prop_assert_eq!(layout.len(), commits.len());
            for (index, commit) in commits.iter().enumerate() {
                let row = layout.row(index).unwrap();
                prop_assert_eq!(row.outgoing.len(), commit.parents.len());
                for (link, parent) in row.outgoing.iter().zip(&commit.parents) {
                    let to = parent_row(&commits, parent);
                    prop_assert!(
                        edge_is_drawn(&layout, index, *link, to),
                        "edge {} -> {} is not drawn", index, to
                    );
                }
            }
            prop_assert_eq!(layout.open_lanes(), 0);
        }

        #[test]
        fn a_lane_that_goes_through_a_row_is_alone_in_its_column(
            shape in prop::collection::vec((any::<u8>(), any::<[u8; 3]>()), 1..60),
        ) {
            let commits = history(&shape);
            let layout = GraphLayout::from_commits(&commits);

            for index in 0..layout.len() {
                let row = layout.row(index).unwrap();
                let mut top: Vec<usize> = row.through.iter().map(|l| l.column).collect();
                top.extend(row.converging.iter().map(|l| l.column));
                top.extend(row.shifts.iter().map(|s| s.from));
                top.extend(row.incoming.then_some(row.column));
                let seen = top.len();
                top.sort_unstable();
                top.dedup();
                prop_assert_eq!(seen, top.len(), "row {} repeats a column at the top", index);
                prop_assert!(
                    row.through.iter().all(|l| l.column != row.column),
                    "row {} runs a lane through its node", index
                );
            }
        }

        #[test]
        fn the_first_parent_stays_in_the_node_lane_unless_a_lane_on_its_left_has_it(
            shape in prop::collection::vec((any::<u8>(), any::<[u8; 3]>()), 1..60),
        ) {
            let commits = history(&shape);
            let layout = GraphLayout::from_commits(&commits);

            for index in 0..layout.len() {
                let row = layout.row(index).unwrap();
                if let Some(first) = row.outgoing.first() {
                    prop_assert!(first.column <= row.column, "row {}", index);
                }
            }
        }

        #[test]
        fn the_same_input_gives_the_same_layout_and_pages_extend_it(
            shape in prop::collection::vec((any::<u8>(), any::<[u8; 3]>()), 1..60),
            cut in 0usize..60,
        ) {
            let commits = history(&shape);
            let whole = GraphLayout::from_commits(&commits);
            prop_assert_eq!(&whole, &GraphLayout::from_commits(&commits));

            let cut = cut.min(commits.len());
            let mut paged = GraphLayout::from_commits(&commits[..cut]);
            paged.extend(&commits[cut..]);
            prop_assert_eq!(whole, paged);
        }
    }
}
