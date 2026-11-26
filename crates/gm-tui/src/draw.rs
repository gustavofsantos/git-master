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

use gm_schema::{LineKind, SyntaxRole};
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
use crate::scrollbar::Bar;
use crate::status::{self, Item, LineChange, Mark};
use crate::threads::TYPES;
use crate::ui::{App, Click, Hit, MIN_HEIGHT, MIN_WIDTH, Overlay};

const DIM: Style = Style::new().fg(Color::DarkGray);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ON: Style = Style::new().add_modifier(Modifier::REVERSED);
/// The scroll bar's thumb (D265).
const THUMB: Style = Style::new().fg(Color::Gray);
const SCROLL_THUMB: &str = "█";
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

/// Where the screen's rows start, how far they can, and where the cursor has to go.
struct Placement {
    scroll: usize,
    /// The furthest start: the last rows fill the screen from it.
    max: usize,
    /// The row the cursor is carried to, when the mouse moved the view off it (D265).
    carry: Option<usize>,
}

/// How many terminal lines `row` takes, `width` columns wide: a line of code, of a file or a
/// diff, may wrap over several (D258 (10), D261); every other row is one.
fn row_height(app: &App, row: &Row<'_, Item>, width: usize) -> usize {
    let wrap = app.wraps();
    match &row.section.item {
        Item::FileLine { index, .. } if wrap => file_line_height(app, *index, width - 1),
        Item::Line { .. } | Item::Pair { .. } if wrap => {
            code_rows(app, row, false, width - 1, wrap).map_or(1, |lines| lines.len())
        }
        _ => 1,
    }
}

/// Places the top screen's rows in `height` lines of `width` columns. After a cursor move the
/// view follows the cursor, showing its row whole; after the mouse moved the view, the view
/// stays and the cursor is carried onto the nearest row shown whole (D265).
fn place_rows(app: &App, height: usize, width: usize) -> Placement {
    let rows = app.view().tree.rows();
    let cursor = app.view().tree.cursor_index().unwrap_or(0);
    let height_of = |row: &Row<'_, Item>| row_height(app, row, width);
    let mut max = rows.len();
    let mut used = 0;
    while max > 0 {
        let above = height_of(&rows[max - 1]);
        if used + above > height {
            break;
        }
        used += above;
        max -= 1;
    }
    let mut scroll = app.view().scroll.min(max);
    let mut carry = None;
    if app.view().scrolled {
        let mut last = scroll;
        let mut used = 0;
        for (index, row) in rows.iter().enumerate().skip(scroll) {
            used += height_of(row);
            if used > height {
                break;
            }
            last = index;
        }
        let into = cursor.clamp(scroll, last.max(scroll));
        if into != cursor && into < rows.len() {
            carry = Some(into);
        }
    } else if cursor < scroll {
        scroll = cursor;
    } else if let Some(cursor_row) = rows.get(cursor) {
        let mut lowest = cursor;
        let mut used = height_of(cursor_row);
        while lowest > 0 {
            let above = height_of(&rows[lowest - 1]);
            if used + above > height {
                break;
            }
            used += above;
            lowest -= 1;
        }
        scroll = scroll.max(lowest);
    }
    Placement { scroll, max, carry }
}

fn draw_rows(frame: &mut Frame, app: &mut App, body: Rect) {
    // The last column is the scroll bar's (D265).
    let track = Rect {
        x: body.x + body.width.saturating_sub(1),
        width: 1,
        ..body
    };
    let body = Rect {
        width: body.width.saturating_sub(1),
        ..body
    };
    let height = usize::from(body.height);
    let width = usize::from(body.width);
    let wrap = app.wraps();
    let place = place_rows(app, height, width);
    if let Some(into) = place.carry {
        app.view_mut().tree.move_to(into);
    }
    let Placement { scroll, max, .. } = place;
    let rows = app.view().tree.rows();
    let cursor = app.view().tree.cursor_index().unwrap_or(0);
    let selection = app.view().tree.selection();
    let rows_len = rows.len();
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut hits = Vec::new();
    // The tinted spans of each row (D250), when the terminal gave tints.
    let mut backgrounds: Vec<(Rect, palette::Rgb)> = Vec::new();
    let covered = match (app.stack.top(), app.repo.file.as_ref()) {
        (crate::ui::Screen::File { .. }, Some(file)) => thread_lines(app, &file.path),
        _ => std::collections::HashMap::new(),
    };
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
                    thread: u32::try_from(index + 1)
                        .ok()
                        .and_then(|line| covered.get(&line).copied()),
                    folded: row.folded && !row.section.children.is_empty(),
                };
                file_line_rows(app, *index, *mark, gutter, width.saturating_sub(1), wrap)
            }
            _ => code_rows(app, row, on, width.saturating_sub(1), wrap)
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
    let shown = hits.len();
    app.hits.extend(hits);
    let view = app.view_mut();
    view.scroll = scroll;
    view.scrolled = false;
    let bar = Bar {
        height,
        total: rows_len,
        shown,
        scroll,
        max,
    };
    draw_bar(frame, app, track, bar);
}

/// The scroll bar (D265), when there is something to scroll: the thumb, as long as the share
/// of the rows on screen, on an empty track. It takes clicks over the rows.
fn draw_bar(frame: &mut Frame, app: &mut App, track: Rect, bar: Bar) {
    app.bar = None;
    if !bar.needed() {
        return;
    }
    app.bar = Some((track, bar));
    app.hits.push(Hit {
        area: track,
        click: Click::Bar,
    });
    let (top, len) = (bar.thumb_top(), bar.thumb_len());
    let lines: Vec<Line> = (0..bar.height)
        .map(|at| {
            if (top..top + len).contains(&at) {
                Line::styled(SCROLL_THUMB, THUMB)
            } else {
                Line::raw(" ")
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), track);
}

/// The terminal lines of a diff line row, Unified or Split, wrapped or cut; `None` for any
/// other row.
fn code_rows(
    app: &App,
    row: &Row<'_, Item>,
    on: bool,
    width: usize,
    wrap: bool,
) -> Option<Vec<Line<'static>>> {
    match &row.section.item {
        Item::Line {
            group,
            path,
            row: index,
        } => Some(diff_lines(app, *group, path, *index, on, width, wrap)),
        Item::Pair {
            group,
            path,
            old,
            new,
            ..
        } => Some(pair_lines(app, *group, path, (*old, *new), on, width, wrap)),
        _ => None,
    }
}

/// What a tinted row shows: an added, a modified or a removed line (D250, D263).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Added,
    Modified,
    Removed,
}

/// The tone of row `index` of a file's diff: by its run, as its gutter sign (D262).
fn line_tone(app: &App, group: Group, path: &str, index: usize) -> Option<Tone> {
    let file = app.repo.files.get(&(group, path.to_owned()))?;
    let DiffRow::Line {
        hunk_index,
        line_index,
        ..
    } = file.rows.get(index).copied()?
    else {
        return None;
    };
    let hunk = file.diff.as_ref()?.hunks.get(hunk_index)?;
    match crate::diff_screen::run_change(hunk, line_index)? {
        LineChange::Added => Some(Tone::Added),
        LineChange::Modified => Some(Tone::Modified),
        LineChange::Removed | LineChange::RemovedBelow | LineChange::RemovedAbove => {
            Some(Tone::Removed)
        }
    }
}

/// Whether a row is a changed line, with its tone and the columns of a gutter tinted
/// stronger (0 for one tone over the whole row: a diff line's gutter and code, D263).
fn row_change(app: &App, row: &Row<'_, Item>) -> Option<(Tone, u16)> {
    let tone = |added: bool| if added { Tone::Added } else { Tone::Removed };
    match &row.section.item {
        Item::Line {
            group,
            path,
            row: index,
        } => line_tone(app, *group, path, *index).map(|t| (t, 0)),
        Item::Text {
            version,
            marked: true,
            ..
        } => Some((tone(matches!(version, Version::After)), 13)),
        Item::Agent(AgentRow::Patch {
            kind: PatchKind::Added,
            ..
        }) => Some((Tone::Added, 0)),
        Item::Agent(AgentRow::Patch {
            kind: PatchKind::Removed,
            ..
        }) => Some((Tone::Removed, 0)),
        _ => None,
    }
}

/// A row's background and its gutter's (D250): the line's change, then the cursor on it, or
/// the selection.
type RowTint = (Option<palette::Rgb>, Option<(u16, palette::Rgb)>);

fn row_tint(tints: &Tints, change: Option<(Tone, u16)>, on: bool, selected: bool) -> RowTint {
    match change {
        Some((tone, gutter)) => {
            let (base, strong, cursor) = match tone {
                Tone::Added => (tints.added, tints.added_gutter, tints.added_cursor),
                Tone::Modified => (tints.modified, tints.modified, tints.modified_cursor),
                Tone::Removed => (tints.removed, tints.removed_gutter, tints.removed_cursor),
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
pub(crate) fn row_backgrounds(
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
    // Each side takes its line's tone over its gutter and its code (D263).
    let side = |index: Option<usize>, x: u16, width: u16| {
        let change = index
            .and_then(|i| line_tone(app, *group, path, i))
            .map(|tone| (tone, 0));
        let area = Rect {
            x: area.x + x,
            width,
            ..area
        };
        spans(area, row_tint(tints, change, on, selected))
    };
    // The mark and the space before the left side take its tint.
    let mut out = side(*old, 0, 2 + left);
    out.extend(side(*new, 3 + left, right));
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
    on: bool,
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let fit = Fit {
        width,
        wrap,
        pad: false,
    };
    diff_cell(app, group, path, index, false, on, fit).map_or_else(
        || vec![Line::default()],
        |rows| rows.into_iter().map(Line::from).collect(),
    )
}

/// One line of a diff in the shared gutter (D262), as rows of spans: its change, its number
/// (the old one on Split's old side), the threads on it and its code. `pad` fills each row to
/// `width`, for a Split side. `None` when the row is not a line of the file's diff.
fn diff_cell(
    app: &App,
    group: Group,
    path: &str,
    index: usize,
    old_side: bool,
    on: bool,
    fit: Fit,
) -> Option<Vec<Vec<Span<'static>>>> {
    let file = app.repo.files.get(&(group, path.to_owned()))?;
    let DiffRow::Line {
        hunk_index,
        line_index,
        ..
    } = file.rows.get(index).copied()?
    else {
        return None;
    };
    let line = status::line_of(&app.repo, group, path, index)?;
    let hunk = file.diff.as_ref()?.hunks.get(hunk_index)?;
    let mark =
        crate::diff_screen::run_change(hunk, line_index).map_or(Mark::Same, |kind| Mark::Changed {
            kind,
            staged: group == Group::Staged,
        });
    let number = if old_side { line.old_no } else { line.new_no };
    // Threads sit on the new side's lines.
    let thread = if old_side || line.kind == LineKind::Removed {
        None
    } else {
        line.new_no
            .and_then(|n| thread_lines(app, path).get(&n).copied())
    };
    // Without tints a removed line is red, so it reads as removed (D251 (3)).
    let whole =
        (line.kind == LineKind::Removed && palette::tints().is_none()).then(|| color(Color::Red));
    let gutter = Gutter {
        on,
        thread,
        folded: false,
    };
    let colors = file.colors.get(&(hunk_index, line_index));
    Some(gutter_rows(
        mark, number, gutter, &line.text, colors, whole, fit,
    ))
}

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
    on: bool,
    width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    let (left, right) = pair_sides(u16::try_from(width + 1).unwrap_or(u16::MAX));
    let (left, right) = (usize::from(left), usize::from(right));
    let side = |row: Option<usize>, old_side: bool, width: usize| {
        let fit = Fit {
            width,
            wrap,
            pad: true,
        };
        row.and_then(|row| diff_cell(app, group, path, row, old_side, on, fit))
            .unwrap_or_else(|| vec![vec![Span::raw(" ".repeat(width))]])
    };
    let old_rows = side(old, true, left);
    let new_rows = side(new, false, right);
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

/// How a line of code fits its columns: `width` of them, wrapped or cut, and with `pad` each
/// row filled to `width` (a Split side).
#[derive(Clone, Copy)]
struct Fit {
    width: usize,
    wrap: bool,
    pad: bool,
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
/// The threads of `path`, by line number (from 1): the strongest state of the threads
/// covering each line, an open thread over a handled one over a resolved one.
fn thread_lines(app: &App, path: &str) -> std::collections::HashMap<u32, Badge> {
    let mut covered = std::collections::HashMap::new();
    let rank = |badge: Badge| match badge {
        Badge::Open => 3,
        Badge::Handled => 2,
        Badge::Outdated => 1,
        Badge::Resolved => 0,
    };
    for comment in app.repo.comments.iter().filter(|c| c.path == path) {
        let Some(location) = comment.location else {
            continue;
        };
        let state = review::badge(comment);
        for line in location.start_line..=location.end_line {
            let slot = covered.entry(line).or_insert(state);
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
    let chars = app
        .repo
        .file
        .as_ref()
        .and_then(|file| file.lines.get(index))
        .map_or_else(Vec::new, |text| {
            text.replace('\t', "    ").chars().collect::<Vec<_>>()
        });
    wrap_rows(&chars, room).len()
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
    let number = u32::try_from(index + 1).ok();
    let fit = Fit {
        width,
        wrap,
        pad: false,
    };
    gutter_rows(
        mark,
        number,
        gutter,
        text,
        file.colors.get(&index),
        None,
        fit,
    )
    .into_iter()
    .map(Line::from)
    .collect()
}

/// A line of code in the gutter the File screen and the diffs share (D262): the change sign,
/// the number (bold under the cursor, in a thread's color on its lines), the thread column
/// and a space, then the code, cut at `width` or wrapped with the signs carried down. `whole`
/// paints all the code in one style; `pad` fills each row to `width`.
fn gutter_rows(
    mark: Mark,
    number: Option<u32>,
    gutter: Gutter,
    text: &str,
    colors: Option<&LineColors>,
    whole: Option<Style>,
    Fit { width, wrap, pad }: Fit,
) -> Vec<Vec<Span<'static>>> {
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
    let code = code_spans(text, colors, whole, if wrap { usize::MAX } else { room });
    let pieces = if wrap {
        let chars: Vec<char> = code.iter().flat_map(|span| span.content.chars()).collect();
        split_spans(code, &wrap_rows(&chars, room))
    } else {
        vec![code]
    };
    let count = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(at, piece)| {
            let change = change_sign(mark, at == 0, at + 1 == count);
            let shown = match number {
                Some(n) if at == 0 => format!("{n:>NUMBER_WIDTH$} "),
                _ => " ".repeat(NUMBER_WIDTH + 1),
            };
            let used: usize = piece.iter().map(|span| span.content.chars().count()).sum();
            let mut spans = vec![
                change,
                Span::styled(shown, number_style),
                thread.clone(),
                Span::raw(" "),
            ];
            spans.extend(piece);
            if pad {
                spans.push(Span::raw(
                    " ".repeat(width.saturating_sub(FILE_GUTTER + used)),
                ));
            }
            spans
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
        LineChange::Removed => (SIGN_BAR, Color::Red),
        LineChange::RemovedBelow if last => (SIGN_BELOW, Color::Red),
        LineChange::RemovedAbove if first => (SIGN_ABOVE, Color::Red),
        LineChange::RemovedBelow | LineChange::RemovedAbove => return Span::raw(" "),
    };
    let (solid, open) = glyphs;
    Span::styled(if staged { open } else { solid }, color(tint))
}

/// The rows a line of `chars` wraps to in `room` columns, as `(start, end)` ranges of
/// `chars`. A row ends at the end of a word and the spaces after it are not drawn, so words
/// stay whole (D264); a word longer than `room` fills the row and is cut. A row is never
/// wider than `room`, and there is always one row, empty for an empty line.
pub(crate) fn wrap_rows(chars: &[char], room: usize) -> Vec<(usize, usize)> {
    let room = room.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    while chars.len() - start > room {
        // The length of the word at or after `at`, past any spaces.
        let word_at = |at: usize| {
            chars[at..]
                .iter()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| !c.is_whitespace())
                .count()
        };
        let window = start + 1..=start + room;
        // Best, the row ends with a word, and the word after fits a row of its own (else
        // breaking before it would only waste this row). Next, the row is only the spaces
        // before a word that fits (an indentation). Last, the row is cut.
        let end = window
            .clone()
            .rev()
            .find(|&at| {
                !chars[at - 1].is_whitespace() && chars[at].is_whitespace() && word_at(at) <= room
            })
            .or_else(|| {
                window.rev().find(|&at| {
                    chars[at - 1].is_whitespace()
                        && !chars[at].is_whitespace()
                        && word_at(at) <= room
                })
            })
            .unwrap_or(start + room);
        rows.push((start, end));
        start = end;
        if chars[end].is_whitespace() {
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
            if start == chars.len() {
                return rows;
            }
        }
    }
    rows.push((start, chars.len()));
    rows
}

/// Cuts `spans` into the `rows` of [`wrap_rows`], keeping each piece's style; the characters
/// between rows (the spaces at a break) are left out. No span is empty.
fn split_spans(spans: Vec<Span<'static>>, rows: &[(usize, usize)]) -> Vec<Vec<Span<'static>>> {
    let mut out: Vec<Vec<Span<'static>>> = vec![Vec::new(); rows.len().max(1)];
    let mut row = 0;
    let mut at = 0;
    for span in spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            while rows.get(row).is_some_and(|&(_, end)| at >= end) {
                if !piece.is_empty() {
                    out[row].push(Span::styled(std::mem::take(&mut piece), span.style));
                }
                row += 1;
            }
            if rows.get(row).is_some_and(|&(start, _)| at >= start) {
                piece.push(c);
            }
            at += 1;
        }
        if let Some(line) = out.get_mut(row).filter(|_| !piece.is_empty()) {
            line.push(Span::styled(piece, span.style));
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

/// A line of the branch's description: under the head's text, past the labels' column, in
/// italic (D266).
fn description_line(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(" ".repeat(8)),
        Span::styled(text.to_owned(), Style::new().add_modifier(Modifier::ITALIC)),
    ])
}

fn row_line(app: &App, row: &Row<'_, Item>, width: usize) -> Line<'static> {
    let repo = &app.repo;
    match &row.section.item {
        Item::Header { label, text } => Line::from(vec![
            Span::styled(format!("{label:<8}"), DIM),
            Span::raw(text.clone()),
        ]),
        Item::Description(text) => description_line(text),
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
        } => diff_lines(app, *group, path, *index, false, width, false).swap_remove(0),
        Item::Pair {
            group,
            path,
            old,
            new,
            ..
        } => pair_lines(app, *group, path, (*old, *new), false, width, false).swap_remove(0),
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

    fn wrapped(text: &str, room: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        wrap_rows(&chars, room)
            .into_iter()
            .map(|(start, end)| chars[start..end].iter().collect())
            .collect()
    }

    #[test]
    fn wrap_breaks_between_words_and_drops_the_spaces_at_the_break() {
        assert_eq!(wrapped("one two three", 7), ["one two", "three"]);
        assert_eq!(
            wrapped("let value = compute(a, b);", 12),
            ["let value =", "compute(a,", "b);"]
        );
        assert_eq!(wrapped("abc   ", 3), ["abc"]);
        assert_eq!(wrapped("    indented words", 12), ["    indented", "words"]);
        assert_eq!(wrapped("", 5), [""]);
        assert_eq!(wrapped("fits", 5), ["fits"]);
        assert_eq!(wrapped("  ab", 3), ["  ", "ab"]);
    }

    #[test]
    fn wrap_cuts_a_word_longer_than_a_row_without_wasting_the_row_before_it() {
        assert_eq!(wrapped("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrapped("ab cdefghijk", 4), ["ab c", "defg", "hijk"]);
        assert_eq!(wrapped("ab cdef", 4), ["ab", "cdef"]);
    }

    #[test]
    fn split_spans_follows_the_rows_and_keeps_styles() {
        let spans = vec![
            Span::styled("one ", BOLD),
            Span::raw("two"),
            Span::styled(" three", DIM),
        ];
        let rows = wrap_rows(&"one two three".chars().collect::<Vec<_>>(), 7);
        let out = split_spans(spans, &rows);
        assert_eq!(
            out,
            [
                vec![Span::styled("one ", BOLD), Span::raw("two")],
                vec![Span::styled("three", DIM)],
            ]
        );
    }

    proptest::proptest! {
        #[test]
        fn wrapped_rows_fit_and_keep_every_word(text in "[a-z ]{0,80}", room in 1usize..20) {
            let rows = wrapped(&text, room);
            proptest::prop_assert!(!rows.is_empty());
            for row in &rows {
                proptest::prop_assert!(row.chars().count() <= room);
            }
            let joined: String = rows.concat();
            proptest::prop_assert_eq!(
                joined.split_whitespace().collect::<String>(),
                text.split_whitespace().collect::<String>()
            );
            // A word that fits a row is never split.
            for word in text.split_whitespace().filter(|w| w.len() <= room) {
                proptest::prop_assert!(
                    rows.iter().any(|row| row.split_whitespace().any(|w| w == word)),
                    "{} split in {:?}", word, rows
                );
            }
        }
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
    fn the_branch_description_shows_under_its_head_in_italic_only_when_it_has_one() {
        let mut app = app();
        let lines =
            |app: &mut App| -> Vec<String> { screen(app).lines().map(str::to_owned).collect() };
        // None: the line under Head is the next header, no room is kept.
        let read = |app: &mut App, branch: &str, text: Option<&str>| {
            app.apply(&gm_schema::Event::BranchDescriptionRead {
                id: gm_schema::OperationId(9),
                branch: branch.to_owned(),
                text: text.map(str::to_owned),
            });
        };
        read(&mut app, "feature", None);
        let shown = lines(&mut app);
        assert!(
            shown[0].contains("Head:") && shown[1].contains("Base:"),
            "{shown:?}"
        );
        read(&mut app, "feature", Some("   \n"));
        assert!(
            lines(&mut app)[1].contains("Base:"),
            "a blank description is none"
        );
        // Another branch's description is not this one's.
        read(&mut app, "main", Some("Not this branch"));
        assert!(!lines(&mut app).concat().contains("Not this branch"));
        read(&mut app, "feature", Some("Ledger postings\nin two lines\n"));
        let shown = lines(&mut app);
        assert!(
            shown[1].starts_with("         Ledger postings"),
            "{shown:?}"
        );
        assert!(shown[2].starts_with("         in two lines"), "{shown:?}");
        assert!(shown[3].contains("Base:"), "{shown:?}");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let cell = &terminal.backend().buffer()[(9, 1)];
        assert_eq!(cell.symbol(), "L");
        assert!(cell.modifier.contains(Modifier::ITALIC));
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
