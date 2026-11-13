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
use gm_view::compose;
use gm_view::diff_rows::Row as DiffRow;
use gm_view::review::{self, Badge, Heading, Part, made_label, range_label, reply_head};
use gm_view::time::relative_time;

use crate::data::{Group, LineColors};
use crate::icons;
use crate::nav::keys::show_sequence;
use crate::nav::prompt::Answer;
use crate::nav::sections::Row;
use crate::reviews::Turn;
use crate::status::{self, Item};
use crate::threads::TYPES;
use crate::ui::{App, Click, Hit, MIN_HEIGHT, MIN_WIDTH, Overlay};

const DIM: Style = Style::new().fg(Color::DarkGray);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ON: Style = Style::new().add_modifier(Modifier::REVERSED);
/// Columns of a diff line's number.
const NUMBER_WIDTH: usize = 5;

fn color(color: Color) -> Style {
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
        Some(Overlay::Keys) => draw_keys(frame, app, body),
        Some(Overlay::Text { .. }) => draw_text_box(frame, app, body, now),
        Some(Overlay::Confirm { .. }) => draw_question(frame, app, body, now),
        Some(Overlay::Search { .. }) => draw_search(frame, app, size),
        None => draw_menu(frame, app, body),
    }
}

// ---- The screen's rows ----

fn draw_rows(frame: &mut Frame, app: &mut App, body: Rect) {
    let height = usize::from(body.height);
    let rows = app.status().tree.rows();
    let cursor = app.status().tree.cursor_index().unwrap_or(0);
    let selection = app.status().tree.selection();
    // Keep the cursor on screen.
    let mut scroll = app.status().scroll.min(rows.len().saturating_sub(height));
    if cursor < scroll {
        scroll = cursor;
    } else if cursor >= scroll + height {
        scroll = cursor + 1 - height;
    }
    let width = usize::from(body.width);
    let mut lines = Vec::with_capacity(height);
    let mut hits = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(scroll).take(height) {
        let y = body.y + u16::try_from(index - scroll).unwrap_or(u16::MAX);
        hits.push(Hit {
            area: Rect::new(body.x, y, body.width, 1),
            click: Click::Row(index),
        });
        let selected = selection.is_some_and(|(a, b)| (a..=b).contains(&index));
        let mark = if selected {
            Span::styled(icons::SELECTED, color(Color::Blue))
        } else {
            Span::raw(" ")
        };
        let mut line = row_line(app, row, width.saturating_sub(1));
        line.spans.insert(0, mark);
        if index == cursor {
            line = line.patch_style(ON);
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), body);
    app.hits.extend(hits);
    app.status_mut().scroll = scroll;
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
fn diff_line(app: &App, group: Group, path: &str, index: usize, width: usize) -> Line<'static> {
    let Some(file) = app.repo.files.get(&(group, path.to_owned())) else {
        return Line::default();
    };
    let Some(DiffRow::Line {
        hunk_index,
        line_index,
        ..
    }) = file.rows.get(index).copied()
    else {
        return Line::default();
    };
    let Some(line) = status::line_of(&app.repo, group, path, index) else {
        return Line::default();
    };
    code_line(line, file.colors.get(&(hunk_index, line_index)), width)
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

fn row_line(app: &App, row: &Row<'_, Item>, width: usize) -> Line<'static> {
    let repo = &app.repo;
    match &row.section.item {
        Item::Header { label, text } => Line::from(vec![
            Span::styled(format!("{label:<7}"), DIM),
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
        } => diff_line(app, *group, path, *index, width),
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

fn code_line(line: &DiffLine, colors: Option<&LineColors>, width: usize) -> Line<'static> {
    let (number, sign, sign_style, whole) = match line.kind {
        LineKind::Added => (line.new_no, "+", color(Color::Green), None),
        LineKind::Removed => (None, "-", color(Color::Red), Some(color(Color::Red))),
        LineKind::Context => (line.new_no, " ", DIM, None),
    };
    let number = number.map_or_else(
        || " ".repeat(NUMBER_WIDTH),
        |no| format!("{no:>NUMBER_WIDTH$}"),
    );
    let mut spans = vec![
        Span::styled(format!("  {number}"), sign_style),
        Span::styled(format!(" {sign} "), sign_style),
    ];
    spans.extend(code_spans(
        &line.text,
        colors,
        whole,
        width.saturating_sub(NUMBER_WIDTH + 5),
    ));
    Line::from(spans)
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

fn role_style(role: SyntaxRole) -> Style {
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

fn draw_mode_line(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut strip = Strip::new(area);
    strip
        .text(
            format!(" {} ", app.mode().label()),
            ON.add_modifier(Modifier::BOLD),
        )
        .text("  ", Style::new());
    let trail = app.stack.trail();
    let top = trail.len() - 1;
    for (depth, address) in trail.into_iter().enumerate() {
        if depth > 0 {
            strip.text(" › ", DIM);
        }
        let style = if depth == top { BOLD } else { DIM };
        strip.click(format!("gitmaster://{address}"), style, Click::Trail(depth));
    }
    let total = app.status().tree.rows().len();
    let place = app.status().tree.cursor_index().map_or(0, |at| at + 1);
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
fn draw_keys(frame: &mut Frame, app: &mut App, body: Rect) {
    let map = app.input.map(crate::nav::Mode::Normal);
    let mut lines = vec![
        Line::styled(" Every key of this screen · any key closes", DIM),
        Line::default(),
    ];
    let mut group = "";
    for spec in app.registry.all() {
        let Some(keys) = map.map(|m| m.keys_of(spec.name)).filter(|k| !k.is_empty()) else {
            continue;
        };
        if spec.group != group {
            group = spec.group;
            lines.push(Line::styled(
                format!(" {group}"),
                color(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        }
        let keys: Vec<String> = keys.iter().map(|k| show_sequence(k)).collect();
        lines.push(Line::from(vec![
            Span::styled(format!("   {:<22}", keys.join(", ")), color(Color::Yellow)),
            Span::raw(spec.description),
        ]));
    }
    frame.render_widget(Clear, body);
    frame.render_widget(Paragraph::new(lines), body);
}

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
    let inner = panel(frame, body, height, &compose::title(&composer.target));
    let mut lines: Vec<Line> = Vec::new();
    if composer.typed() {
        let mut spans = vec![Span::styled("type ", DIM)];
        for (index, (word, _)) in TYPES.iter().enumerate() {
            let style = if index == composer.kind % TYPES.len() {
                ON
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
    let save = compose::save_label(&composer.target).to_lowercase();
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
mod tests {
    use super::*;
    use gm_schema::{BranchInfo, Change, RepoStatus, StatusEntry};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn screen(app: &mut App) -> String {
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
        assert!(shown.contains("gitmaster://status"), "{shown}");
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
