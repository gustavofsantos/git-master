//! Draws the companion (D241): the top screen's rows, the mode line with the trail, the hint
//! line, and the overlays (the which-key menu, the text box, the question, the search line,
//! the keys). Colors are the terminal's own (D236 (4)), icons are Nerd Font (D238); with
//! `NO_COLOR` set, nothing is colored (D242 (4)). Every clickable thing is recorded in
//! `App::hits` with what a click does.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use gm_schema::{DiffLine, LineKind, SyntaxRole};
use gm_view::diff_rows::Row as DiffRow;
use gm_view::local_review::Version;
use gm_view::provenance_view::Row as ProvenanceRow;
use gm_view::review::{self, Badge, Heading, Part, made_label, range_label, reply_head};
use gm_view::thread_turns::{CallStatus, PatchKind};
use gm_view::time::relative_time;

use crate::agent_screen::AgentRow;
use crate::data::{Group, LineColors};
use crate::icons;
use crate::nav::keys::show_sequence;
use crate::nav::prompt::Answer;
use crate::nav::sections::Row;
use crate::palette::{self, Tints};
use crate::reviews::Turn;
use crate::status::{self, Item, LineChange, Mark};
use crate::threads::TYPES;
use crate::ui::{App, Click, Hit, MIN_HEIGHT, MIN_WIDTH, Overlay};

const DIM: Style = Style::new().fg(Color::DarkGray);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ON: Style = Style::new().add_modifier(Modifier::REVERSED);
/// The mark of a line a review thread covers.
const THREAD_BAR: &str = "┃";
/// The File screen's change signs (D258 (11)), (not staged, staged): a bar for an added or
/// modified line, and an edge for lines removed after or before it. The staged glyph is
/// open, so the background shows through it.
const SIGN_BAR: (&str, &str) = ("▎", "░");
const SIGN_BELOW: (&str, &str) = ("▁", "_");
const SIGN_ABOVE: (&str, &str) = ("▔", "‾");
/// Columns of a diff line's number.
const NUMBER_WIDTH: usize = 5;

pub(crate) fn color(color: Color) -> Style {
    if no_color() {
        Style::new()
    } else {
        Style::new().fg(color)
    }
}

/// `NO_COLOR` set to anything turns colors off, as the convention asks.
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_secs()).ok())
        .unwrap_or(0)
}

/// Lays out one line of text and clickable parts from the left.
struct Strip {
    area: Rect,
    x: u16,
    spans: Vec<Span<'static>>,
    hits: Vec<Hit>,
}

impl Strip {
    fn new(area: Rect) -> Self {
        Self {
            area,
            x: area.x,
            spans: Vec::new(),
            hits: Vec::new(),
        }
    }

    fn text(&mut self, text: impl Into<String>, style: Style) -> &mut Self {
        let span = Span::styled(text.into(), style);
        self.x = self
            .x
            .saturating_add(u16::try_from(span.width()).unwrap_or(u16::MAX));
        self.spans.push(span);
        self
    }

    fn click(&mut self, text: impl Into<String>, style: Style, click: Click) -> &mut Self {
        let start = self.x;
        self.text(text, style);
        let right = self.area.x + self.area.width;
        if start < right {
            self.hits.push(Hit {
                area: Rect::new(start, self.area.y, self.x.min(right) - start, 1),
                click,
            });
        }
        self
    }

    fn render(self, frame: &mut Frame, hits: &mut Vec<Hit>) {
        frame.render_widget(Paragraph::new(Line::from(self.spans)), self.area);
        hits.extend(self.hits);
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    app.hits.clear();
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        let text = format!(
            "GitMaster needs a terminal of at least {MIN_WIDTH}x{MIN_HEIGHT}; this one is {}x{}. \
             Make it larger, or press Ctrl+C to quit.",
            size.width, size.height
        );
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), size);
        return;
    }
    app.fit_layout(size.width);
    let body = Rect {
        height: size.height - 2,
        ..size
    };
    app.body_rows = usize::from(body.height);
    draw_rows(frame, app, body);
    draw_mode_line(
        frame,
        app,
        Rect {
            y: size.y + size.height - 2,
            height: 1,
            ..size
        },
    );
    draw_hint_line(
        frame,
        app,
        Rect {
            y: size.y + size.height - 1,
            height: 1,
            ..size
        },
    );
    let now = Instant::now();
    match app.overlays.last() {
        Some(Overlay::Text { .. }) => draw_text_box(frame, app, body, now),
        Some(Overlay::Confirm { .. }) => draw_question(frame, app, body, now),
        Some(Overlay::Search { .. }) => draw_search(frame, app, size),
        Some(Overlay::Files { .. }) => draw_files(frame, app, body, now),
        None => draw_menu(frame, app, body),
    }
}

// ---- The screen's rows ----

fn draw_rows(frame: &mut Frame, app: &mut App, body: Rect) {
    let height = usize::from(body.height);
    let rows = app.view().tree.rows();
    let cursor = app.view().tree.cursor_index().unwrap_or(0);
    let selection = app.view().tree.selection();
    let width = usize::from(body.width);
    // A line of code, of a file or a diff, may wrap over several terminal lines (D258 (10),
    // D261); every other row is one.
    let wrap = app.wraps();
    let row_height = |row: &Row<'_, Item>| match &row.section.item {
        Item::FileLine { index, .. } if wrap => file_line_height(app, *index, width - 1),
        Item::Line { .. } | Item::Pair { .. } if wrap => {
            code_rows(app, row, width - 1, wrap).map_or(1, |lines| lines.len())
        }
        _ => 1,
    };
    // Keep the cursor on screen: the lowest start that still shows the cursor's row whole.
    let mut scroll = app.view().scroll.min(rows.len().saturating_sub(1));
    if cursor < scroll {
        scroll = cursor;
    } else if let Some(cursor_row) = rows.get(cursor) {
        let mut lowest = cursor;
        let mut used = row_height(cursor_row);
        while lowest > 0 {
            let above = row_height(&rows[lowest - 1]);
            if used + above > height {
                break;
            }
            used += above;
            lowest -= 1;
        }
        scroll = scroll.max(lowest);
    }
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut hits = Vec::new();
    // The tinted spans of each row (D250), when the terminal gave tints.
    let mut backgrounds: Vec<(Rect, palette::Rgb)> = Vec::new();
    let covered = covered_lines(app);
    for (index, row) in rows.iter().enumerate().skip(scroll) {
        if lines.len() >= height {
            break;
        }
        let y = body.y + u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let selected = selection.is_some_and(|(a, b)| (a..=b).contains(&index));
        let mark = if selected {
            Span::styled(icons::SELECTED, color(Color::Blue))
        } else {
            Span::raw(" ")
        };
        let on = index == cursor;
        let mut drawn = match &row.section.item {
            Item::FileLine { index, mark } => {
                let gutter = Gutter {
                    on,
                    thread: covered.get(index).copied(),
                    folded: row.folded && !row.section.children.is_empty(),
                };
                file_line_rows(app, *index, *mark, gutter, width.saturating_sub(1), wrap)
            }
            _ => code_rows(app, row, width.saturating_sub(1), wrap)
                .unwrap_or_else(|| vec![row_line(app, row, width.saturating_sub(1))]),
        };
        drawn.truncate(height - lines.len());
        let tall = u16::try_from(drawn.len()).unwrap_or(1);
        let area = Rect::new(body.x, y, body.width, tall);
        hits.push(Hit {
            area,
            click: Click::Row(index),
        });
        for (at, line) in drawn.iter_mut().enumerate() {
            let first = if at == 0 {
                mark.clone()
            } else {
                Span::raw(" ")
            };
            line.spans.insert(0, first);
        }
        match palette::tints() {
            Some(tints) => {
                backgrounds.extend(row_backgrounds(tints, app, row, on, selected, area));
            }
            None if on => {
                for line in &mut drawn {
                    *line = std::mem::take(line).patch_style(ON);
                }
            }
            None => {}
        }
        lines.extend(drawn);
    }
    frame.render_widget(Paragraph::new(lines), body);
    let buffer = frame.buffer_mut();
    for (area, tint) in backgrounds {
        buffer.set_style(area.intersection(body), Style::new().bg(tint.color()));
    }
    app.hits.extend(hits);
    app.view_mut().scroll = scroll;
}

/// The terminal lines of a diff line row, Unified or Split, wrapped or cut; `None` for any
/// other row.
fn code_rows(
    app: &App,
    row: &Row<'_, Item>,
    width: usize,
    wrap: bool,
) -> Option<Vec<Line<'static>>> {
    match &row.section.item {
        Item::Line {
            group,
            path,
            row: index,
        } => Some(diff_lines(app, *group, path, *index, width, wrap)),
        Item::Pair {
            group,
            path,
            old,
            new,
            ..
        } => Some(pair_lines(app, *group, path, (*old, *new), width, wrap)),
        _ => None,
    }
}

/// Whether a row is an added (`true`) or removed (`false`) line, with the columns of its
/// gutter (the selection mark, number and sign).
fn row_change(app: &App, row: &Row<'_, Item>) -> Option<(bool, u16)> {
    match &row.section.item {
        Item::Line {
            group,
            path,
            row: index,
        } => match status::line_of(&app.repo, *group, path, *index)?.kind {
            LineKind::Added => Some((true, 11)),
            LineKind::Removed => Some((false, 11)),
            LineKind::Context => None,
        },
        Item::Text {
            version,
            marked: true,
            ..
        } => Some((matches!(version, Version::After), 13)),
        Item::Agent(AgentRow::Patch {
            kind: PatchKind::Added,
            ..
        }) => Some((true, 0)),
        Item::Agent(AgentRow::Patch {
            kind: PatchKind::Removed,
            ..
        }) => Some((false, 0)),
        _ => None,
    }
}

/// A row's background and its gutter's (D250): the line's change, then the cursor on it, or
/// the selection.
type RowTint = (Option<palette::Rgb>, Option<(u16, palette::Rgb)>);

fn row_tint(tints: &Tints, change: Option<(bool, u16)>, on: bool, selected: bool) -> RowTint {
    match change {
        Some((added, gutter)) => {
            let (base, strong, cursor) = if added {
                (tints.added, tints.added_gutter, tints.added_cursor)
            } else {
                (tints.removed, tints.removed_gutter, tints.removed_cursor)
            };
            let whole = if on { cursor } else { base };
            (Some(whole), (gutter > 0).then_some((gutter, strong)))
        }
        None if on => (Some(tints.cursor), None),
        None if selected => (Some(tints.selected), None),
        None => (None, None),
    }
}

/// The tinted spans of a row in `area`: the whole row, or in Split each side on its own
/// (D252 (5)).
fn row_backgrounds(
    tints: &Tints,
    app: &App,
    row: &Row<'_, Item>,
    on: bool,
    selected: bool,
    area: Rect,
) -> Vec<(Rect, palette::Rgb)> {
    let spans = |area: Rect, (whole, gutter): RowTint| {
        let mut out = Vec::new();
        let split = gutter.map_or(0, |(columns, _)| columns.min(area.width));
        if let Some((_, strong)) = gutter {
            out.push((
                Rect {
                    width: split,
                    ..area
                },
                strong,
            ));
        }
        if let Some(whole) = whole {
            let rest = Rect {
                x: area.x + split,
                width: area.width - split,
                ..area
            };
            out.push((rest, whole));
        }
        out
    };
    let Item::Pair {
        group,
        path,
        old,
        new,
        ..
    } = &row.section.item
    else {
        return spans(area, row_tint(tints, row_change(app, row), on, selected));
    };
    let (left, right) = pair_sides(area.width);
    let side = |index: Option<usize>, x: u16, width: u16, gutter: u16| {
        let kind = index
            .and_then(|i| status::line_of(&app.repo, *group, path, i))
            .map(|line| line.kind);
        let change = match kind {
            Some(LineKind::Added) => Some((true, gutter)),
            Some(LineKind::Removed) => Some((false, gutter)),
            _ => None,
        };
        let area = Rect {
            x: area.x + x,
            width,
            ..area
        };
        spans(area, row_tint(tints, change, on, selected))
    };
    // The mark and the space before the left side take its tint.
    let mut out = side(*old, 0, 2 + left, 2 + SIDE_GUTTER);
    out.extend(side(*new, 3 + left, right, SIDE_GUTTER));
    out
}

fn fold_icon(row: &Row<'_, Item>) -> &'static str {
    if row.section.children.is_empty() {
        " "
    } else if row.folded {
        icons::FOLDED
    } else {
        icons::UNFOLDED
    }
}

fn file_line(row: &Row<'_, Item>) -> Line<'static> {
    let Item::File {
        group,
        path,
        letter,
        counts,
        threads,
    } = &row.section.item
    else {
        return Line::default();
    };
    let mut spans = vec![
        Span::raw(format!("  {} ", fold_icon(row))),
        Span::styled(format!("{letter} "), color(letter_color(*letter, *group))),
        Span::raw(format!("{} {path}", icons::file(path))),
    ];
    if let Some((added, deleted)) = counts {
        spans.push(Span::styled(format!("  +{added}"), color(Color::Green)));
        spans.push(Span::styled(format!(" -{deleted}"), color(Color::Red)));
    }
    if *threads > 0 {
        spans.push(Span::styled(
            format!("  {}{threads}", icons::THREADS),
            color(Color::Yellow),
        ));
    }
    Line::from(spans)
}

/// A line of a file's diff with the colors of its code.
fn diff_lines(
    app: &App,
    group: Group,
    path: &str,
    index: usize,
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let none = || vec![Line::default()];
    let Some(file) = app.repo.files.get(&(group, path.to_owned())) else {
        return none();
    };
    let Some(DiffRow::Line {
        hunk_index,
        line_index,
        ..
    }) = file.rows.get(index).copied()
    else {
        return none();
    };
    let Some(line) = status::line_of(&app.repo, group, path, index) else {
        return none();
    };
    code_lines(
        line,
        file.colors.get(&(hunk_index, line_index)),
        width,
        wrap,
    )
}

/// The columns of a side's gutter in Split: the number, the sign and a space each side.
const SIDE_GUTTER: u16 = 8;

/// The widths of the two sides of a Split row `width` columns wide. The row is the
/// selection mark, a space, the left side, a separator and the right side.
fn pair_sides(width: u16) -> (u16, u16) {
    let room = width.saturating_sub(3);
    let left = room / 2;
    (left, room - left)
}

/// A row of a file's diff in Split: the old line on the left, the new on the right, each
/// with its number, sign and colors (D252).
/// With `wrap`, each side wraps within its half and the row is as tall as its taller side.
fn pair_lines(
    app: &App,
    group: Group,
    path: &str,
    (old, new): (Option<usize>, Option<usize>),
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let (left, right) = pair_sides(u16::try_from(width + 1).unwrap_or(u16::MAX));
    let (left, right) = (usize::from(left), usize::from(right));
    let old_rows = cell_rows(app, group, path, old, true, left, wrap);
    let new_rows = cell_rows(app, group, path, new, false, right, wrap);
    let count = old_rows.len().max(new_rows.len());
    let blank = |width: usize| vec![Span::raw(" ".repeat(width))];
    (0..count)
        .map(|at| {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(old_rows.get(at).cloned().unwrap_or_else(|| blank(left)));
            spans.push(Span::styled("│", DIM));
            spans.extend(new_rows.get(at).cloned().unwrap_or_else(|| blank(right)));
            Line::from(spans)
        })
        .collect()
}

/// One side of a Split row, each row of it exactly `width` columns: blank when the side has
/// no line; with `wrap`, its code continues on the next rows under the code.
fn cell_rows(
    app: &App,
    group: Group,
    path: &str,
    row: Option<usize>,
    old_side: bool,
    width: usize,
    wrap: bool,
) -> Vec<Vec<Span<'static>>> {
    let file = app.repo.files.get(&(group, path.to_owned()));
    let line = row.and_then(|row| status::line_of(&app.repo, group, path, row));
    let (Some(file), Some(row), Some(line)) = (file, row, line) else {
        return vec![vec![Span::raw(" ".repeat(width))]];
    };
    let colors = match file.rows.get(row) {
        Some(DiffRow::Line {
            hunk_index,
            line_index,
            ..
        }) => file.colors.get(&(*hunk_index, *line_index)),
        _ => None,
    };
    let (sign, sign_style, whole) = match line.kind {
        LineKind::Added => ("+", color(Color::Green), None),
        // Without tints a removed line is red, as in Unified (D251 (3)).
        LineKind::Removed => (
            "-",
            color(Color::Red),
            palette::tints().is_none().then(|| color(Color::Red)),
        ),
        LineKind::Context => (" ", DIM, None),
    };
    let number = if old_side { line.old_no } else { line.new_no };
    let number = number.map_or_else(
        || " ".repeat(NUMBER_WIDTH),
        |no| format!("{no:>NUMBER_WIDTH$}"),
    );
    let gutter = usize::from(SIDE_GUTTER);
    let room = width.saturating_sub(gutter).max(1);
    let code = code_spans(
        &line.text,
        colors,
        whole,
        if wrap { usize::MAX } else { room },
    );
    let pieces = if wrap {
        split_spans(code, room)
    } else {
        vec![code]
    };
    pieces
        .into_iter()
        .enumerate()
        .map(|(at, piece)| {
            let head = if at == 0 {
                format!("{number} {sign} ")
            } else {
                format!("{} {sign} ", " ".repeat(NUMBER_WIDTH))
            };
            let used: usize = piece.iter().map(|span| span.content.chars().count()).sum();
            let mut spans = vec![Span::styled(head, sign_style)];
            spans.extend(piece);
            spans.push(Span::raw(" ".repeat(width.saturating_sub(gutter + used))));
            spans
        })
        .collect()
}

/// A row of the Commits and Stashes sections.
fn history_line(app: &App, row: &Row<'_, Item>) -> Line<'static> {
    let repo = &app.repo;
    match &row.section.item {
        Item::Commits { count, more } => Line::from(vec![
            Span::styled(
                format!("{} Commits", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count}{})", if *more { "+" } else { "" }), DIM),
        ]),
        Item::Commit { index } => repo
            .commits
            .as_ref()
            .and_then(|history| history.commits.get(*index))
            .map_or_default(|commit| {
                Line::from(vec![
                    Span::styled(format!("    {} ", commit.short), color(Color::Yellow)),
                    Span::raw(commit.subject.clone()),
                    Span::styled(
                        format!("  {}", relative_time(now_secs(), commit.author_time)),
                        DIM,
                    ),
                ])
            }),
        Item::Stashes { count } => Line::from(vec![
            Span::styled(
                format!("{} Stashes", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::Stash { index } => repo.stashes.get(*index).map_or_default(|stash| {
            Line::from(vec![
                Span::styled(
                    format!("    stash@{{{}}} ", stash.index),
                    color(Color::Yellow),
                ),
                Span::raw(stash.message.clone()),
                Span::styled(format!("  {}", relative_time(now_secs(), stash.time)), DIM),
            ])
        }),
        _ => Line::default(),
    }
}

/// A row of the Thread and Keys screens.
fn screen_line(row: &Row<'_, Item>) -> Line<'static> {
    match &row.section.item {
        Item::Version { version, caption } => Line::from(vec![
            Span::styled(
                format!("{} {}", fold_icon(row), version.label()),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {caption}"), DIM),
        ]),
        Item::Text {
            version,
            number,
            text,
            marked,
            colors,
        } => {
            let (sign, tint) = match (marked, version) {
                (false, _) => (' ', None),
                (true, Version::Before) => ('-', Some(Color::Red)),
                (true, Version::After) => ('+', Some(Color::Green)),
            };
            let style = tint.map_or(Style::new(), color);
            // With tints the background marks a changed line and its code keeps its colors;
            // without them, the whole line takes the mark's color (D251 (3)).
            let whole = tint.filter(|_| palette::tints().is_none()).map(color);
            let mut spans = vec![
                Span::styled(format!("    {number:>5} "), DIM),
                Span::styled(format!("{sign} "), style),
            ];
            spans.extend(code_spans(text, Some(colors), whole, usize::MAX));
            Line::from(spans)
        }
        Item::Messages { count } => Line::from(vec![
            Span::styled(
                format!("{} Messages", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::KeyGroup { title, count } => Line::from(vec![
            Span::styled(
                format!("{} {title}", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::Binding { keys, description } => Line::from(vec![
            Span::styled(format!("    {keys:<24}"), color(Color::Yellow)),
            Span::raw(*description),
        ]),
        _ => Line::default(),
    }
}

/// What the File screen's gutter shows beside a line's number (D258 (7)).
#[derive(Clone, Copy)]
struct Gutter {
    /// The cursor is on the line: its number stands out, as in an editor.
    on: bool,
    /// The state of the strongest thread that covers the line.
    thread: Option<Badge>,
    /// The threads under the line are folded.
    folded: bool,
}

const NO_GUTTER: Gutter = Gutter {
    on: false,
    thread: None,
    folded: false,
};

/// The threads of the File screen's file, by line index (from 0): the strongest state of
/// the threads covering each line, an open thread over a handled one over a resolved one.
fn covered_lines(app: &App) -> std::collections::HashMap<usize, Badge> {
    let mut covered = std::collections::HashMap::new();
    if !matches!(app.stack.top(), crate::ui::Screen::File { .. }) {
        return covered;
    }
    let Some(file) = app.repo.file.as_ref() else {
        return covered;
    };
    let rank = |badge: Badge| match badge {
        Badge::Open => 3,
        Badge::Handled => 2,
        Badge::Outdated => 1,
        Badge::Resolved => 0,
    };
    for comment in app.repo.comments.iter().filter(|c| c.path == file.path) {
        let Some(location) = comment.location else {
            continue;
        };
        let state = review::badge(comment);
        for line in location.start_line..=location.end_line {
            let Some(index) = (line as usize).checked_sub(1) else {
                continue;
            };
            let slot = covered.entry(index).or_insert(state);
            if rank(state) > rank(*slot) {
                *slot = state;
            }
        }
    }
    covered
}

/// Columns of the File screen's gutter: the change bar, the number, the thread mark.
const FILE_GUTTER: usize = 9;

/// How many terminal lines File line `index` takes when long lines wrap in `width` columns.
fn file_line_height(app: &App, index: usize, width: usize) -> usize {
    let room = width.saturating_sub(FILE_GUTTER).max(1);
    let columns = app
        .repo
        .file
        .as_ref()
        .and_then(|file| file.lines.get(index))
        .map_or(0, |text| {
            text.chars()
                .map(|c| if c == '\t' { 4 } else { 1 })
                .sum::<usize>()
        });
    columns.div_ceil(room).max(1)
}

/// A line of the File screen (D258 (9)): the change bar, its number, the thread mark and
/// its colored code; cut at `width`, or wrapped over several lines with the bars carried on.
fn file_line_rows(
    app: &App,
    index: usize,
    mark: Mark,
    gutter: Gutter,
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let Some(file) = app.repo.file.as_ref() else {
        return vec![Line::default()];
    };
    let text = file.lines.get(index).map_or("", String::as_str);
    let number_style = match (gutter.on, gutter.thread) {
        (true, _) => BOLD,
        (false, Some(state)) => color(badge(state).2),
        (false, None) => DIM,
    };
    // A thread's lines carry a bar in its color; the line its cards hang from shows a
    // chevron while they are folded.
    let thread = match gutter.thread {
        Some(state) if gutter.folded => Span::styled(icons::FOLDED, color(badge(state).2)),
        Some(state) => Span::styled(THREAD_BAR, color(badge(state).2)),
        None => Span::raw(" "),
    };
    let room = width.saturating_sub(FILE_GUTTER).max(1);
    let code = code_spans(
        text,
        file.colors.get(&index),
        None,
        if wrap { usize::MAX } else { room },
    );
    let pieces = if wrap {
        split_spans(code, room)
    } else {
        vec![code]
    };
    let count = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(at, piece)| {
            let change = change_sign(mark, at == 0, at + 1 == count);
            let number = if at == 0 {
                Span::styled(format!("{:>5} ", index + 1), number_style)
            } else {
                Span::raw("      ")
            };
            let mut spans = vec![change.clone(), number, thread.clone(), Span::raw(" ")];
            spans.extend(piece);
            Line::from(spans)
        })
        .collect()
}

/// The change column of one row of a File line (D258 (11)): the color says what changed (green
/// added, blue modified, red removed) and the glyph whether it is staged: solid while it is
/// not, open once it is, so the background shows through. A bar runs down every row of a
/// wrapped line; a removal sits on the edge it happened at.
fn change_sign(mark: Mark, first: bool, last: bool) -> Span<'static> {
    let Mark::Changed { kind, staged } = mark else {
        return Span::raw(" ");
    };
    let (glyphs, tint) = match kind {
        LineChange::Added => (SIGN_BAR, Color::Green),
        LineChange::Modified => (SIGN_BAR, Color::Blue),
        LineChange::RemovedBelow if last => (SIGN_BELOW, Color::Red),
        LineChange::RemovedAbove if first => (SIGN_ABOVE, Color::Red),
        LineChange::RemovedBelow | LineChange::RemovedAbove => return Span::raw(" "),
    };
    let (solid, open) = glyphs;
    Span::styled(if staged { open } else { solid }, color(tint))
}

/// Cuts `spans` into lines of at most `room` characters each, keeping each piece's style.
/// No span is empty; an empty input gives one empty line.
fn split_spans(spans: Vec<Span<'static>>, room: usize) -> Vec<Vec<Span<'static>>> {
    let mut out: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut left = room;
    for span in spans {
        let style = span.style;
        let mut chars = span.content.chars().peekable();
        while chars.peek().is_some() {
            if left == 0 {
                out.push(Vec::new());
                left = room;
            }
            let piece: String = chars.by_ref().take(left).collect();
            left -= piece.chars().count();
            if let Some(line) = out.last_mut() {
                line.push(Span::styled(piece, style));
            }
        }
    }
    out
}

/// A row of the Blame and Origin screens.
fn blame_row(app: &App, row: &Row<'_, Item>, width: usize) -> Line<'static> {
    let lines = app.repo.blame.as_ref();
    match &row.section.item {
        Item::BlameRun { first, last } => {
            let Some(line) = lines.and_then(|(_, l)| l.get(*first)) else {
                return Line::default();
            };
            let count = last - first + 1;
            let mut spans = vec![Span::raw(format!("{} ", fold_icon(row)))];
            match &line.commit {
                Some(commit) => {
                    spans.push(Span::styled(
                        format!("{} ", &commit[..commit.len().min(7)]),
                        color(Color::Yellow),
                    ));
                    spans.push(Span::styled(line.subject.clone(), BOLD));
                    spans.push(Span::styled(
                        format!(
                            "  {} · {}",
                            line.author,
                            relative_time(now_secs(), line.author_at / 1000)
                        ),
                        DIM,
                    ));
                }
                None => spans.push(Span::styled("Not committed", color(Color::Green))),
            }
            spans.push(Span::styled(
                format!(" ({count} {})", if count == 1 { "line" } else { "lines" }),
                DIM,
            ));
            Line::from(spans)
        }
        Item::BlameLine { index } => {
            let Some(line) = lines.and_then(|(_, l)| l.get(*index)) else {
                return Line::default();
            };
            let colors = app.repo.blame_colors.get(index);
            let mut spans = vec![Span::styled(format!("    {:>6}  ", line.line), DIM)];
            spans.extend(code_spans(
                &line.text,
                colors,
                None,
                width.saturating_sub(12),
            ));
            Line::from(spans)
        }
        Item::Origin(origin) => origin_line(origin),
        _ => Line::default(),
    }
}

/// A row of the Origin screen, as the app's panel words it.
fn origin_line(row: &ProvenanceRow) -> Line<'static> {
    match row {
        ProvenanceRow::Note(text) => Line::styled(format!("  {text}"), DIM),
        ProvenanceRow::Failure(text) => Line::styled(format!("  {text}"), color(Color::Red)),
        ProvenanceRow::Cut(text) => {
            Line::styled(format!("  {text}"), DIM.add_modifier(Modifier::ITALIC))
        }
        ProvenanceRow::Commit { oid, text } => {
            let short = &oid[..oid.len().min(7)];
            Line::from(vec![
                Span::styled(format!("  {short}"), color(Color::Yellow)),
                Span::raw(text.strip_prefix(short).unwrap_or(text).to_owned()),
            ])
        }
        ProvenanceRow::Thread {
            title,
            harness,
            evidence,
            ambiguous,
            ..
        } => {
            let mut spans = vec![
                Span::styled(format!("    {} ", icons::AGENT), color(Color::Magenta)),
                Span::styled(
                    title.clone(),
                    color(Color::Magenta).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" · {harness}"), DIM),
            ];
            if !evidence.is_empty() {
                spans.push(Span::styled(
                    format!(" · {}", evidence.join(", ")),
                    color(Color::Cyan),
                ));
            }
            if *ambiguous {
                spans.push(Span::styled(" · ambiguous", color(Color::Yellow)));
            }
            Line::from(spans)
        }
        ProvenanceRow::Label { name, origin } => {
            Line::styled(format!("      label {name} · {origin}"), DIM)
        }
        ProvenanceRow::Link {
            relation, title, ..
        } => Line::styled(format!("      {relation} {title}"), DIM),
        ProvenanceRow::Edit {
            path,
            at,
            evidence,
            unverifiable,
            ..
        } => {
            let mut parts = vec![format!("      edit {path}"), at.clone()];
            if !evidence.is_empty() {
                parts.push(evidence.join(", "));
            }
            parts.extend(unverifiable.map(str::to_owned));
            let text = parts.join(" · ");
            Line::styled(text, color(Color::Cyan))
        }
        ProvenanceRow::Entry { speaker, text } => Line::from(vec![
            Span::styled(format!("        {speaker}: "), BOLD),
            Span::styled(text.clone(), DIM),
        ]),
    }
}

/// A row of the Agent thread screen.
fn agent_line(row: &Row<'_, Item>, agent: &AgentRow) -> Line<'static> {
    let indent = "  ".repeat(row.depth + 1);
    match agent {
        AgentRow::Heading { title, count } => Line::from(vec![
            Span::styled(
                format!("{} {title}", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        AgentRow::Text { text, quiet } => Line::styled(
            format!("{indent}{text}"),
            if *quiet { DIM } else { Style::new() },
        ),
        AgentRow::Call {
            tool,
            summary,
            status,
            duration,
        } => {
            let (mark, style) = match status {
                CallStatus::Done => (icons::CALL_DONE, color(Color::Green)),
                CallStatus::Failed => (icons::CALL_FAILED, color(Color::Red)),
                CallStatus::Pending => (icons::CALL_PENDING, DIM),
            };
            let mut spans = vec![
                Span::raw(format!("{indent}{} ", fold_icon(row))),
                Span::styled(format!("{mark} "), style),
                Span::styled(format!("{tool} "), BOLD),
                Span::raw(summary.clone()),
            ];
            if let Some(duration) = duration {
                spans.push(Span::styled(format!("  {duration}"), DIM));
            }
            Line::from(spans)
        }
        AgentRow::File {
            path,
            added,
            removed,
        } => Line::from(vec![
            Span::raw(format!(
                "{indent}{} {} {path}",
                fold_icon(row),
                icons::file(path)
            )),
            Span::styled(format!("  +{added}"), color(Color::Green)),
            Span::styled(format!(" -{removed}"), color(Color::Red)),
        ]),
        AgentRow::Patch { kind, text, colors } => {
            let style = match kind {
                PatchKind::Added => color(Color::Green),
                PatchKind::Removed => color(Color::Red),
                PatchKind::Hunk => color(Color::Blue),
                PatchKind::Context => Style::new(),
            };
            if colors.is_empty() {
                return Line::styled(format!("{indent}{}", text.replace('\t', "    ")), style);
            }
            // The sign in its color; the code in its syntax colors, or whole in the sign's
            // color when there are no tints to mark the line (D251 (3)).
            let (sign, code) = text.split_at(text.chars().next().map_or(0, char::len_utf8));
            let tinted = matches!(kind, PatchKind::Added | PatchKind::Removed);
            let whole = (tinted && palette::tints().is_none()).then_some(style);
            let mut spans = vec![Span::styled(format!("{indent}{sign}"), style)];
            spans.extend(code_spans(code, Some(colors), whole, usize::MAX));
            Line::from(spans)
        }
        AgentRow::Turn { number, prompt } => Line::from(vec![
            Span::raw(format!("{indent}{} ", fold_icon(row))),
            Span::styled(format!("Turn {number} "), BOLD),
            Span::styled(prompt.clone(), DIM),
        ]),
    }
}

fn row_line(app: &App, row: &Row<'_, Item>, width: usize) -> Line<'static> {
    let repo = &app.repo;
    match &row.section.item {
        Item::Header { label, text } => Line::from(vec![
            Span::styled(format!("{label:<8}"), DIM),
            Span::raw(text.clone()),
        ]),
        Item::Blank => Line::default(),
        Item::Group { group, count } => Line::from(vec![
            Span::styled(
                format!("{} {}", fold_icon(row), group.title()),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::File { .. } => file_line(row),
        Item::Note(text) => Line::styled(format!("{}{text}", " ".repeat(row.depth * 2 + 2)), DIM),
        Item::Hunk { group, path, hunk } => {
            let header = repo
                .files
                .get(&(*group, path.clone()))
                .and_then(|f| f.diff.as_ref())
                .and_then(|d| d.hunks.get(*hunk))
                .map_or_default(|h| {
                    if h.header.trim().is_empty() {
                        format!("line {}", h.new_start)
                    } else {
                        h.header.trim().to_owned()
                    }
                });
            Line::from(vec![
                Span::raw(format!("    {} ", fold_icon(row))),
                Span::styled("@@ ", color(Color::Blue)),
                Span::styled(header, BOLD),
            ])
        }
        Item::Line {
            group,
            path,
            row: index,
        } => diff_lines(app, *group, path, *index, width, false).swap_remove(0),
        Item::Pair {
            group,
            path,
            old,
            new,
            ..
        } => pair_lines(app, *group, path, (*old, *new), width, false).swap_remove(0),
        Item::Card { id, part } => card_line(app, *id, part),
        Item::Heading(Heading::Outdated { count, open }) => Line::styled(
            format!(
                "      {} {count} outdated {}",
                if *open {
                    icons::UNFOLDED
                } else {
                    icons::FOLDED
                },
                if *count == 1 { "thread" } else { "threads" }
            ),
            color(Color::Magenta),
        ),
        Item::Heading(Heading::Outside) => {
            Line::styled("      Threads on lines this diff does not show:", DIM)
        }
        Item::Threads { count } => Line::from(vec![
            Span::styled(
                format!("{} Review threads", fold_icon(row)),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::Turn { turn, count } => Line::from(vec![
            Span::raw(format!("  {} ", fold_icon(row))),
            Span::styled(
                turn.title(),
                if *turn == Turn::You {
                    color(Color::Cyan)
                } else {
                    BOLD
                },
            ),
            Span::styled(format!(" ({count})"), DIM),
        ]),
        Item::Thread { id } => thread_line(app, *id, width),
        Item::Commits { .. } | Item::Commit { .. } | Item::Stashes { .. } | Item::Stash { .. } => {
            history_line(app, row)
        }
        Item::FileLine { index, mark } => {
            file_line_rows(app, *index, *mark, NO_GUTTER, width, false).swap_remove(0)
        }
        Item::BlameRun { .. } | Item::BlameLine { .. } | Item::Origin(_) => {
            blame_row(app, row, width)
        }
        Item::Agent(agent) => agent_line(row, agent),
        Item::Version { .. }
        | Item::Text { .. }
        | Item::Messages { .. }
        | Item::KeyGroup { .. }
        | Item::Binding { .. } => screen_line(row),
    }
}

fn letter_color(letter: char, group: Group) -> Color {
    match (letter, group) {
        ('U', _) => Color::Magenta,
        ('?', _) => Color::Cyan,
        ('D', _) => Color::Red,
        (_, Group::Staged) | ('A', _) => Color::Green,
        _ => Color::Yellow,
    }
}

/// A Unified diff line: its number, its sign and its code, cut at `width`, or with `wrap`
/// continued on the next rows under the code with the sign carried along.
fn code_lines(
    line: &DiffLine,
    colors: Option<&LineColors>,
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let (number, sign, sign_style, whole) = match line.kind {
        LineKind::Added => (line.new_no, "+", color(Color::Green), None),
        LineKind::Removed => {
            // With tints the red background marks the line, so its code keeps its syntax
            // colors (D251 (3)); without them, the whole line is red.
            let whole = palette::tints().is_none().then(|| color(Color::Red));
            (None, "-", color(Color::Red), whole)
        }
        LineKind::Context => (line.new_no, " ", DIM, None),
    };
    let number = number.map_or_else(
        || " ".repeat(NUMBER_WIDTH),
        |no| format!("{no:>NUMBER_WIDTH$}"),
    );
    let room = width.saturating_sub(NUMBER_WIDTH + 5).max(1);
    let code = code_spans(
        &line.text,
        colors,
        whole,
        if wrap { usize::MAX } else { room },
    );
    let pieces = if wrap {
        split_spans(code, room)
    } else {
        vec![code]
    };
    pieces
        .into_iter()
        .enumerate()
        .map(|(at, piece)| {
            let shown = if at == 0 {
                number.clone()
            } else {
                " ".repeat(NUMBER_WIDTH)
            };
            let mut spans = vec![
                Span::styled(format!("  {shown}"), sign_style),
                Span::styled(format!(" {sign} "), sign_style),
            ];
            spans.extend(piece);
            Line::from(spans)
        })
        .collect()
}

/// The spans of a line's code: its colored ranges in their role's color, the rest plain;
/// tabs as spaces; cut at `room` columns. `whole` colors the whole line instead.
fn code_spans(
    text: &str,
    colors: Option<&LineColors>,
    whole: Option<Style>,
    room: usize,
) -> Vec<Span<'static>> {
    let mut pieces: Vec<(&str, Style)> = Vec::new();
    match (whole, colors) {
        (Some(style), _) => pieces.push((text, style)),
        (None, None) => pieces.push((text, Style::new())),
        (None, Some(colors)) => {
            let mut at = 0;
            for &(start, end, role) in colors {
                let valid = start >= at
                    && end <= text.len()
                    && start < end
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end);
                if !valid {
                    continue;
                }
                if start > at {
                    pieces.push((&text[at..start], Style::new()));
                }
                pieces.push((&text[start..end], role_style(role)));
                at = end;
            }
            pieces.push((&text[at..], Style::new()));
        }
    }
    let mut left = room;
    let mut spans = Vec::new();
    for (piece, style) in pieces {
        if left == 0 {
            break;
        }
        let expanded: String = piece.replace('\t', "    ").chars().take(left).collect();
        left -= expanded.chars().count();
        if !expanded.is_empty() {
            spans.push(Span::styled(expanded, style));
        }
    }
    spans
}

pub(crate) fn role_style(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Comment => color(Color::DarkGray).add_modifier(Modifier::ITALIC),
        SyntaxRole::String => color(Color::Green),
        SyntaxRole::Number | SyntaxRole::Constant | SyntaxRole::Attribute => color(Color::Yellow),
        SyntaxRole::Keyword => color(Color::Magenta),
        SyntaxRole::Function => color(Color::Blue),
        SyntaxRole::Type => color(Color::Cyan),
        SyntaxRole::Variable
        | SyntaxRole::Property
        | SyntaxRole::Operator
        | SyntaxRole::Punctuation => Style::new(),
    }
}

fn badge(badge: Badge) -> (&'static str, &'static str, Color) {
    match badge {
        Badge::Open => (icons::OPEN, "Open", Color::Yellow),
        Badge::Handled => (icons::HANDLED, "Handled", Color::Cyan),
        Badge::Resolved => (icons::RESOLVED, "Resolved", Color::Green),
        Badge::Outdated => (icons::OUTDATED, "Outdated", Color::Magenta),
    }
}

/// One row of a thread drawn under its lines.
fn card_line(app: &App, id: i64, part: &Part) -> Line<'static> {
    let state = app.repo.comment(id).map_or(Badge::Open, review::badge);
    let edge = Span::styled("         │ ", color(badge(state).2));
    let rest: Vec<Span<'static>> = match part {
        Part::Head {
            badge: b,
            lines,
            created_at,
            chips,
            ..
        } => {
            let (icon, word, tint) = badge(*b);
            let mut spans = vec![
                Span::styled(
                    format!("{icon} {word}"),
                    color(tint).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        " · #{id} · {lines} · {}",
                        made_label(now_secs(), *created_at)
                    ),
                    DIM,
                ),
            ];
            if chips.agent {
                spans.push(Span::styled(
                    format!(" · {} {}", icons::AGENT, review::AGENT_WRITTEN),
                    color(Color::Cyan),
                ));
            }
            if let Some(label) = chips.label {
                spans.push(Span::styled(
                    format!(" · {}", label.as_str()),
                    color(Color::Cyan),
                ));
            }
            spans
        }
        Part::Text(text) => vec![Span::raw(text.clone())],
        Part::Folded(summary) => vec![Span::styled(format!("{} {summary}", icons::FOLDED), DIM)],
        Part::Reply {
            agent,
            author,
            created_at,
        } => {
            let mut spans = vec![Span::styled(
                reply_head(author, *created_at, now_secs()),
                BOLD,
            )];
            if *agent {
                spans.push(Span::styled(
                    format!(" · {} {}", icons::AGENT, review::AGENT_WRITTEN),
                    color(Color::Cyan),
                ));
            }
            spans
        }
        Part::Toggle(toggle) => vec![Span::styled(toggle.label(), DIM)],
    };
    let mut spans = vec![edge];
    spans.extend(rest);
    Line::from(spans)
}

/// A thread in the Review threads section: its status, file and lines, and its last word.
fn thread_line(app: &App, id: i64, width: usize) -> Line<'static> {
    let Some(comment) = app.repo.comment(id) else {
        return Line::default();
    };
    let (icon, _, tint) = badge(review::badge(comment));
    let (start, end) = review::shown_range(comment);
    let name = comment.path.rsplit('/').next().unwrap_or(&comment.path);
    let head = format!("    {icon} #{id} {name} {}  ", range_label(start, end));
    let last = comment
        .replies
        .iter()
        .max_by_key(|reply| (reply.created_at, reply.id))
        .map_or(comment.body.as_str(), |reply| reply.body.as_str());
    let summary: String = gm_view::local_review::summary(last)
        .chars()
        .take(width.saturating_sub(head.chars().count()))
        .collect();
    Line::from(vec![
        Span::styled(head, color(tint)),
        Span::styled(summary, DIM),
    ])
}

// ---- The bottom lines ----

/// A chip that stands out: the terminal's background on its blue with tints (D250 (4)),
/// reversed without.
fn chip_style() -> Style {
    match palette::tints() {
        Some(tints) => Style::new()
            .fg(tints.background.color())
            .bg(tints.chip.color())
            .add_modifier(Modifier::BOLD),
        None => ON.add_modifier(Modifier::BOLD),
    }
}

fn draw_mode_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut strip = Strip::new(area);
    strip
        .text(format!(" {} ", app.mode().label()), chip_style())
        .text("  ", Style::new());
    let trail = app.stack.trail();
    let top = trail.len() - 1;
    for (depth, address) in trail.into_iter().enumerate() {
        if depth > 0 {
            strip.text(" › ", DIM);
        }
        let style = if depth == top { BOLD } else { DIM };
        strip.click(address, style, Click::Trail(depth));
    }
    let total = app.view().tree.rows().len();
    let place = app.view().tree.cursor_index().map_or(0, |at| at + 1);
    let position = format!("{place}/{total} ");
    let used = usize::from(strip.x - area.x);
    let gap = usize::from(area.width).saturating_sub(used + position.len());
    strip
        .text(" ".repeat(gap), Style::new())
        .text(position, DIM);
    strip.render(frame, &mut app.hits);
}

fn draw_hint_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut strip = Strip::new(area);
    if let Some(notice) = app.notice.as_ref().or(app.watch_notice.as_ref()) {
        strip.text(format!(" {notice}"), color(Color::Yellow));
    } else {
        strip.text(" Space menu · ? keys · / search · q back", DIM);
    }
    strip.render(frame, &mut app.hits);
}

// ---- Overlays ----

/// The panel at the bottom of the body, `height` rows high, cleared and framed.
fn panel(frame: &mut Frame, body: Rect, height: u16, title: &str) -> Rect {
    let height = height.min(body.height);
    let area = Rect {
        y: body.y + body.height - height,
        height,
        ..body
    };
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(ratatui::widgets::Borders::TOP)
        .title(format!(" {title} "))
        .border_style(DIM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// A line of the menu: its keys, its title and what a click on it does.
type MenuEntry = (String, String, Click);

/// The which-key menu of the pending prefix: what may follow, grouped, only what applies.
fn draw_menu(frame: &mut Frame, app: &mut App, body: Rect) {
    let mode = app.mode();
    let Some((prefix, node)) = app.input.pending(mode) else {
        return;
    };
    let prefix = prefix.to_vec();
    let mut groups: Vec<(String, Vec<MenuEntry>)> = Vec::new();
    for (key, child) in &node.children {
        let (group, title) = match child.action {
            Some(name) => {
                if !app.applies(name) {
                    continue;
                }
                let spec = app.registry.get(name);
                (
                    spec.map_or("Other", |s| s.group).to_owned(),
                    spec.map_or(name, |s| s.title).to_owned(),
                )
            }
            None => (
                "More".to_owned(),
                format!("{}…", child.label.clone().unwrap_or_default()),
            ),
        };
        let entry = (key.to_string(), title, Click::Key(*key));
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((group, vec![entry])),
        }
    }
    let tallest = groups.iter().map(|(_, e)| e.len()).max().unwrap_or(0);
    let height = u16::try_from(tallest + 2).unwrap_or(u16::MAX);
    let title = format!(
        "{} · {}   Esc close",
        show_sequence(&prefix),
        app.target_title()
    );
    let inner = panel(frame, body, height, &title);
    let columns = u16::try_from(groups.len().max(1)).unwrap_or(1);
    let column_width = (inner.width / columns).max(16);
    for (at, (group, entries)) in groups.into_iter().enumerate() {
        let x = inner.x + column_width * u16::try_from(at).unwrap_or(0);
        if x >= inner.x + inner.width {
            break;
        }
        let column = Rect::new(x, inner.y, column_width.min(inner.x + inner.width - x), 1);
        let mut head = Strip::new(column);
        head.text(group, color(Color::Magenta).add_modifier(Modifier::BOLD));
        head.render(frame, &mut app.hits);
        for (row, (key, title, click)) in entries.into_iter().enumerate() {
            let area = Rect {
                y: inner.y + 1 + u16::try_from(row).unwrap_or(0),
                ..column
            };
            if area.y >= inner.y + inner.height {
                break;
            }
            let mut strip = Strip::new(area);
            strip.click(
                format!("{key:<3}"),
                color(Color::Yellow).add_modifier(Modifier::BOLD),
                click.clone(),
            );
            strip.click(title, Style::new(), click);
            strip.render(frame, &mut app.hits);
        }
    }
}

/// Every key of the screen, by group, with what it does.
/// Keys of a prompt, dimmed until its guard accepts them (D242 (5)).
fn guarded(open: bool) -> Style {
    if open {
        color(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        DIM
    }
}

fn draw_text_box(frame: &mut Frame, app: &mut App, body: Rect, now: Instant) {
    let Some(Overlay::Text { composer, guard }) = app.overlays.last() else {
        return;
    };
    let width = usize::from(body.width.saturating_sub(2)).max(10);
    let mut text: Vec<Line> = review::wrap(&composer.text, width)
        .into_iter()
        .map(Line::raw)
        .collect();
    match text.last_mut() {
        Some(last) => last.push_span(Span::styled(" ", ON)),
        None => text.push(Line::from(vec![
            Span::styled(" ", ON),
            Span::styled(" Write here.", DIM),
        ])),
    }
    let shown = text.len().min(6);
    let extra = usize::from(composer.typed()) + usize::from(composer.error.is_some());
    let height = u16::try_from(shown + extra + 3).unwrap_or(u16::MAX);
    let inner = panel(frame, body, height, &composer.title());
    let mut lines: Vec<Line> = Vec::new();
    if composer.typed() {
        let mut spans = vec![Span::styled("type ", DIM)];
        for (index, (word, _)) in TYPES.iter().enumerate() {
            let style = if index == composer.kind % TYPES.len() {
                chip_style()
            } else {
                DIM
            };
            spans.push(Span::styled(format!(" {word} "), style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(" Tab", DIM));
        lines.push(Line::from(spans));
    }
    let skip = text.len() - shown;
    lines.extend(text.into_iter().skip(skip));
    if let Some(error) = &composer.error {
        lines.push(Line::styled(error.clone(), color(Color::Red)));
    }
    let open = guard.accepts(Answer::Confirm, now);
    let cancel = guard.accepts(Answer::Cancel, now);
    let save = composer.save_label();
    lines.push(Line::from(vec![
        Span::styled("Ctrl+S ", guarded(open)),
        Span::styled(format!("{save}   "), DIM),
        Span::styled("Esc ", guarded(cancel)),
        Span::styled("cancel   Enter new line", DIM),
    ]));
    frame.render_widget(Paragraph::new(lines), inner);
    app.hits.retain(|hit| !inner.intersects(hit.area));
}

fn draw_question(frame: &mut Frame, app: &mut App, body: Rect, now: Instant) {
    let Some(Overlay::Confirm {
        question, guard, ..
    }) = app.overlays.last()
    else {
        return;
    };
    let question = question.clone();
    let yes = guard.accepts(Answer::Confirm, now);
    let no = guard.accepts(Answer::Cancel, now);
    let inner = panel(frame, body, 3, "Confirm");
    let mut strip = Strip::new(Rect { height: 1, ..inner });
    strip
        .text(format!("{question}  "), BOLD)
        .click("y yes", guarded(yes), Click::Answer(Answer::Confirm))
        .text("   ", Style::new())
        .click("n no", guarded(no), Click::Answer(Answer::Cancel));
    app.hits.retain(|hit| !inner.intersects(hit.area));
    strip.render(frame, &mut app.hits);
}

/// The most files the picker lists at once.
const PICKER_ROWS: usize = 12;

/// The file picker (D258): the query with how many files it matched, the files best first with
/// the matched letters bold, the one under the cursor reversed, and its keys.
fn draw_files(frame: &mut Frame, app: &mut App, body: Rect, now: Instant) {
    let Some(Overlay::Files { picker, guard }) = app.overlays.last() else {
        return;
    };
    let rows = usize::from(body.height)
        .saturating_sub(4)
        .clamp(1, PICKER_ROWS);
    let height = u16::try_from(rows + 3).unwrap_or(u16::MAX);
    let cancel = guard.accepts(Answer::Cancel, now);
    let mut lines: Vec<Line> = Vec::new();
    let count = match (&picker.search, &picker.error) {
        (_, Some(error)) => Span::styled(format!("  {error}"), color(Color::Red)),
        (Some(search), None) => Span::styled(
            format!(
                "  {}/{}{}",
                search.matched,
                search.files,
                if search.indexing {
                    " · indexing…"
                } else {
                    ""
                }
            ),
            DIM,
        ),
        (None, None) => Span::styled("  searching…", DIM),
    };
    lines.push(Line::from(vec![
        Span::styled("› ", BOLD),
        Span::raw(picker.query.clone()),
        Span::styled(" ", ON),
        count,
    ]));
    let hits = picker.hits();
    let first = (picker.selected + 1).saturating_sub(rows);
    let mut clicks: Vec<(usize, usize)> = Vec::new();
    for (index, hit) in hits.iter().enumerate().skip(first).take(rows) {
        clicks.push((lines.len(), index));
        lines.push(hit_line(&hit.path, &hit.ranges, index == picker.selected));
    }
    if picker.query.trim().is_empty() {
        lines.push(Line::styled(
            "Type part of a path; :12 after it opens at line 12.",
            DIM,
        ));
    } else if hits.is_empty() && picker.search.is_some() {
        lines.push(Line::styled("No file matches.", DIM));
    }
    while lines.len() < rows + 1 {
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(vec![
        Span::styled("Enter ", color(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("open   ", DIM),
        Span::styled("↑ ↓ ", BOLD),
        Span::styled("move   ", DIM),
        Span::styled("Esc ", guarded(cancel)),
        Span::styled("close", DIM),
    ]));
    let inner = panel(frame, body, height, "Open a file");
    app.hits.retain(|hit| !inner.intersects(hit.area));
    for (row, index) in clicks {
        let y = inner.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y < inner.y + inner.height {
            app.hits.push(Hit {
                area: Rect::new(inner.x, y, inner.width, 1),
                click: Click::Pick(index),
            });
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// A file the picker lists: its icon and path, the byte `ranges` the query matched in bold.
fn hit_line(path: &str, ranges: &[(usize, usize)], selected: bool) -> Line<'static> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut spans = vec![Span::raw(format!("{} ", icons::file(name)))];
    let matched = color(Color::Yellow).add_modifier(Modifier::BOLD);
    let mut at = 0;
    for &(start, end) in ranges {
        let (Some(before), Some(part)) = (path.get(at..start), path.get(start..end)) else {
            continue;
        };
        spans.push(Span::raw(before.to_owned()));
        spans.push(Span::styled(part.to_owned(), matched));
        at = end;
    }
    spans.push(Span::raw(path.get(at..).unwrap_or_default().to_owned()));
    let line = Line::from(spans);
    if selected { line.patch_style(ON) } else { line }
}

/// The search line, at the bottom, as in less and vim.
fn draw_search(frame: &mut Frame, app: &mut App, size: Rect) {
    let Some(Overlay::Search { text, .. }) = app.overlays.last() else {
        return;
    };
    let area = Rect {
        y: size.y + size.height - 1,
        height: 1,
        ..size
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!("/{text}")),
            Span::styled(" ", ON),
        ])),
        area,
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gm_schema::{BranchInfo, Change, RepoStatus, StatusEntry};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    pub(crate) fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        App::new(RepoStatus {
            root: "/repo".to_owned(),
            branch: BranchInfo {
                head: Some("feature".to_owned()),
                oid: Some("a41f9c2aaaa".to_owned()),
                ..BranchInfo::default()
            },
            entries: vec![StatusEntry {
                path: "src/lib.rs".to_owned(),
                orig_path: None,
                staged: None,
                unstaged: Some(Change::Modified),
                untracked: false,
                conflicted: false,
                submodule: None,
            }],
        })
    }

    #[test]
    fn status_shows_its_sections_the_mode_line_and_the_hints() {
        let shown = screen(&mut app());
        assert!(shown.contains("Head:"), "{shown}");
        assert!(shown.contains("feature"), "{shown}");
        assert!(shown.contains("Unstaged changes"), "{shown}");
        assert!(shown.contains("src/lib.rs"), "{shown}");
        assert!(shown.contains("NORMAL"), "{shown}");
        assert!(shown.contains(" status "), "{shown}");
        assert!(!shown.contains("gitmaster://"), "{shown}");
        assert!(shown.contains("Space menu"), "{shown}");
    }

    #[test]
    fn space_opens_the_menu_with_what_applies_here() {
        let mut app = app();
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        for _ in 0..3 {
            app.on_key(down);
        }
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let shown = screen(&mut app);
        assert!(!shown.contains("clean"), "files are listed: {shown}");
        assert!(
            shown.contains("Diff…") && !shown.contains("Diff……"),
            "{shown}"
        );
        assert!(shown.contains("s  stage"), "{shown}");
        assert!(
            !shown.contains("u  unstage"),
            "an unstaged file cannot be unstaged: {shown}"
        );
    }
}
