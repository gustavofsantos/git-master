//! Draws the model. Colors are the terminal's own ANSI colors (D235 (7), D236 (4)), so the
//! companion follows the terminal's theme and tmux's. Every button drawn is recorded in
//! `App::hits` with its action, so a click does what the button's key does (D236 (2)).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{
    Action, App, Context, Focus, Hit, MIN_HEIGHT, MIN_WIDTH, Overlay, Row, Tab, areas,
};

const DIM: Style = Style::new().fg(Color::DarkGray);
const ON: Style = Style::new().add_modifier(Modifier::REVERSED);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    app.hits.clear();
    app.areas = areas(size, app.focus);
    let (Some(top), Some(bar)) = (app.areas.top, app.areas.bar) else {
        too_small(frame, size);
        return;
    };
    draw_top(frame, app, top);
    if let Some(area) = app.areas.sidebar {
        draw_sidebar(frame, app, area);
    }
    if let Some(area) = app.areas.main {
        draw_main(frame, app, area);
    }
    draw_bar(frame, app, bar);
    if app.overlay == Some(Overlay::Help) {
        draw_help(frame, app, size);
    }
}

fn too_small(frame: &mut Frame, size: Rect) {
    let text = format!(
        "GitMaster needs a terminal of at least {MIN_WIDTH}x{MIN_HEIGHT}; this one is {}x{}. \
         Make it larger, or press q to quit.",
        size.width, size.height
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), size);
}

/// Lays out one line of text and buttons from the left, recording each button's area.
struct Strip<'a> {
    area: Rect,
    x: u16,
    spans: Vec<Span<'a>>,
    hits: Vec<Hit>,
}

impl<'a> Strip<'a> {
    fn new(area: Rect) -> Self {
        Self {
            area,
            x: area.x,
            spans: Vec::new(),
            hits: Vec::new(),
        }
    }

    fn text(&mut self, text: impl Into<std::borrow::Cow<'a, str>>, style: Style) -> &mut Self {
        let span = Span::styled(text, style);
        self.x = self
            .x
            .saturating_add(u16::try_from(span.width()).unwrap_or(u16::MAX));
        self.spans.push(span);
        self
    }

    fn button(
        &mut self,
        text: impl Into<std::borrow::Cow<'a, str>>,
        style: Style,
        action: Action,
    ) -> &mut Self {
        let start = self.x;
        self.text(text, style);
        let right = self.area.x + self.area.width;
        if start < right {
            self.hits.push(Hit {
                area: Rect::new(start, self.area.y, self.x.min(right) - start, 1),
                action,
            });
        }
        self
    }

    /// Fills the line up to where `tail` columns are left, so what follows sits on the right.
    fn push_right(&mut self, tail: usize) -> &mut Self {
        let used = usize::from(self.x - self.area.x);
        let gap = usize::from(self.area.width).saturating_sub(used + tail);
        self.text(" ".repeat(gap), Style::new())
    }

    fn render(self, frame: &mut Frame, hits: &mut Vec<Hit>) {
        frame.render_widget(Paragraph::new(Line::from(self.spans)), self.area);
        hits.extend(self.hits);
    }
}

fn draw_top(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut strip = Strip::new(area);
    strip.text(" gitmaster ", BOLD);
    for tab in Tab::ALL {
        let title = match tab {
            Tab::Reviews => format!(" {} {} ", tab.title(), app.unresolved()),
            _ => format!(" {} ", tab.title()),
        };
        let style = if tab == app.tab { ON } else { DIM };
        strip
            .text(" ", Style::new())
            .button(title, style, Action::Tab(tab));
    }
    let branch = &app.status.branch;
    let mut right = vec![(
        format!("⎇ {}", branch.head.as_deref().unwrap_or("(detached)")),
        Style::new().fg(Color::Blue),
    )];
    if let Some(upstream) = &branch.upstream {
        right.push((format!(" → {upstream}"), DIM));
        if branch.ahead > 0 {
            right.push((format!(" ↑{}", branch.ahead), Style::new().fg(Color::Green)));
        }
        if branch.behind > 0 {
            right.push((format!(" ↓{}", branch.behind), Style::new().fg(Color::Red)));
        }
    }
    right.push((" ".to_owned(), Style::new()));
    if let Some(notice) = &app.notice {
        right.insert(0, (format!("{notice}  "), Style::new().fg(Color::Yellow)));
    }
    let tail: usize = right
        .iter()
        .map(|(text, _)| Span::raw(text.as_str()).width())
        .sum();
    strip.push_right(tail);
    for (text, style) in right {
        strip.text(text, style);
    }
    strip.render(frame, &mut app.hits);
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let split = app.areas.main.is_some();
    if split {
        let border = Block::new().borders(Borders::RIGHT).border_style(DIM);
        frame.render_widget(border, area);
    }
    let chips_area = Rect {
        height: 1,
        width: area.width.saturating_sub(u16::from(split)),
        ..area
    };
    let mut chips = Strip::new(chips_area);
    for context in Context::ALL {
        let title = format!(" {} {} ", context.title(), app.file_count(context));
        let style = if context == app.context { ON } else { DIM };
        chips
            .text(" ", Style::new())
            .button(title, style, Action::Context(context));
    }
    chips.render(frame, &mut app.hits);

    let Some(list) = app.areas.list() else {
        return;
    };
    if app.rows.is_empty() {
        let empty = format!(" No {} changes.", app.context.title().to_lowercase());
        frame.render_widget(Paragraph::new(empty).style(DIM), list);
        return;
    }
    let height = usize::from(list.height);
    app.keep_cursor_visible(height);
    let selected = app.selected().map(|file| file.path.clone());
    let focused = app.focus == Focus::Sidebar;
    let width = usize::from(list.width);
    let mut lines = Vec::with_capacity(height);
    for (index, row) in app.rows.iter().enumerate().skip(app.scroll).take(height) {
        let y = list.y + u16::try_from(index - app.scroll).unwrap_or(u16::MAX);
        app.hits.push(Hit {
            area: Rect::new(list.x, y, list.width, 1),
            action: Action::Row(index),
        });
        let mut line = sidebar_line(row, width);
        if index == app.cursor && focused {
            line = line.style(ON);
        } else if matches!(row, Row::File(file) if Some(&file.path) == selected.as_ref()) {
            line = line.style(BOLD);
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), list);
}

fn sidebar_line(row: &Row, width: usize) -> Line<'static> {
    match row {
        Row::Dir { dir, files, folded } => {
            let name = if dir.is_empty() { "./" } else { dir.as_str() };
            let text = if *folded {
                format!(" ▸ {name} ({files})")
            } else {
                format!(" ▾ {name}")
            };
            Line::styled(fit_end(&text, width), DIM)
        }
        Row::File(file) => {
            let mut tail = Vec::new();
            if let Some((added, deleted)) = file.counts {
                tail.push(Span::styled(
                    format!(" +{added}"),
                    Style::new().fg(Color::Green),
                ));
                tail.push(Span::styled(
                    format!(" −{deleted}"),
                    Style::new().fg(Color::Red),
                ));
            }
            if file.threads > 0 {
                tail.push(Span::styled(
                    format!(" ◫{}", file.threads),
                    Style::new().fg(Color::Yellow),
                ));
            }
            tail.push(Span::raw(" "));
            let tail_width: usize = tail.iter().map(Span::width).sum();
            let letter = Span::styled(
                format!("   {} ", file.letter),
                Style::new().fg(letter_color(file.letter)),
            );
            let room = width.saturating_sub(letter.width() + tail_width);
            let name = fit_end(&file.name, room);
            let gap = room.saturating_sub(Span::raw(name.as_str()).width());
            let mut spans = vec![letter, Span::raw(name), Span::raw(" ".repeat(gap))];
            spans.extend(tail);
            Line::from(spans)
        }
    }
}

fn letter_color(letter: char) -> Color {
    match letter {
        'U' => Color::Magenta,
        '?' => Color::Cyan,
        'D' => Color::Red,
        'A' => Color::Green,
        _ => Color::Yellow,
    }
}

/// `text` in at most `width` columns: whole when it fits, else its end after `…`, so the
/// name stays readable.
fn fit_end(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = text.chars().skip(count - (width - 1)).collect();
    format!("…{tail}")
}

fn draw_main(frame: &mut Frame, app: &App, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(1),
        ..area
    };
    let lines = match app.tab {
        Tab::Diff => diff_placeholder(app),
        Tab::File => vec![Line::styled("The File tab is not available yet.", DIM)],
        Tab::Reviews => vec![Line::styled("The Reviews tab is not available yet.", DIM)],
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn diff_placeholder(app: &App) -> Vec<Line<'static>> {
    let Some(file) = app.selected() else {
        return vec![Line::styled(
            format!("No {} changes.", app.context.title().to_lowercase()),
            DIM,
        )];
    };
    let mut header = vec![Span::styled(file.path.clone(), BOLD)];
    if let Some((added, deleted)) = file.counts {
        header.push(Span::styled(
            format!("  +{added}"),
            Style::new().fg(Color::Green),
        ));
        header.push(Span::styled(
            format!(" −{deleted}"),
            Style::new().fg(Color::Red),
        ));
    }
    header.push(Span::styled(
        format!("  · {}", app.context.title().to_lowercase()),
        DIM,
    ));
    vec![
        Line::from(header),
        Line::default(),
        Line::styled("The diff is not shown yet.", DIM),
    ]
}

/// The buttons of the bottom bar, each with its key, for where the person is.
fn bar_buttons(app: &App) -> Vec<(&'static str, String, Action)> {
    let focus = match app.focus {
        Focus::Sidebar => "main pane",
        Focus::Main => "files",
    };
    let mut buttons = vec![
        (
            "t",
            app.context.other().title().to_owned(),
            Action::OtherContext,
        ),
        ("Tab", focus.to_owned(), Action::ToggleFocus),
    ];
    for tab in Tab::ALL {
        if tab != app.tab {
            let key = match tab {
                Tab::Diff => "1",
                Tab::File => "2",
                Tab::Reviews => "3",
            };
            buttons.push((key, tab.title().to_owned(), Action::Tab(tab)));
        }
    }
    buttons.push(("?", "keys".to_owned(), Action::Help));
    buttons.push(("q", "quit".to_owned(), Action::Quit));
    buttons
}

fn draw_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut strip = Strip::new(area);
    for (key, label, action) in bar_buttons(app) {
        strip.text(" ", Style::new());
        let start = strip.x;
        strip.text(key, BOLD).text(format!(" {label}"), DIM);
        let width = strip.x.saturating_sub(start);
        strip.hits.push(Hit {
            area: Rect::new(start, area.y, width, 1),
            action,
        });
        strip.text(" ", Style::new());
    }
    strip.render(frame, &mut app.hits);
}

/// Every key, as `?` shows them.
const KEYS: &[(&str, &str)] = &[
    ("1 2 3", "Diff, File or Reviews tab"),
    ("Tab", "sidebar or main pane"),
    ("t", "Unstaged or Staged"),
    ("j k ↓ ↑", "move in the sidebar"),
    ("Ctrl+D Ctrl+U", "half a screen down or up"),
    ("g G", "first or last row"),
    ("Enter Space", "fold or unfold a folder"),
    ("l h", "unfold or fold a folder"),
    ("?", "these keys"),
    ("q", "quit"),
];

fn draw_help(frame: &mut Frame, app: &mut App, size: Rect) {
    let height = u16::try_from(KEYS.len() + 4)
        .unwrap_or(u16::MAX)
        .min(size.height);
    let width = 52.min(size.width);
    let area = Rect::new(
        size.x + (size.width - width) / 2,
        size.y + (size.height - height) / 2,
        width,
        height,
    );
    let mut lines: Vec<Line> = KEYS
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!(" {key:<15}"), BOLD),
                Span::raw(*what),
            ])
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled(" Any key or click closes this.", DIM));
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(" Keys ").border_style(DIM);
    frame.render_widget(Paragraph::new(lines).block(block), area);
    app.hits.push(Hit {
        area: size,
        action: Action::CloseHelp,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{BranchInfo, Change, RepoStatus, StatusEntry};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn app() -> App {
        let entry = |path: &str, staged, unstaged| StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        };
        App::new(RepoStatus {
            root: "/repo".to_owned(),
            branch: BranchInfo {
                head: Some("feature".to_owned()),
                upstream: Some("origin/feature".to_owned()),
                ahead: 2,
                ..BranchInfo::default()
            },
            entries: vec![
                entry("src/lib.rs", None, Some(Change::Modified)),
                entry("README.md", Some(Change::Added), None),
            ],
        })
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn the_frame_has_tabs_branch_chips_tree_and_buttons() {
        let mut app = app();
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines[0].starts_with(" gitmaster   Diff   File   Reviews 0"),
            "{lines:?}"
        );
        assert!(
            lines[0].ends_with("⎇ feature → origin/feature ↑2"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("  Unstaged 1   Staged 1"), "{lines:?}");
        assert!(lines[2].starts_with(" ▾ src"), "{lines:?}");
        assert!(lines[3].starts_with("   M lib.rs"), "{lines:?}");
        assert!(lines[1].contains("│ src/lib.rs  · unstaged"), "{lines:?}");
        assert!(
            lines[29].starts_with(" t Staged  Tab main pane  2 File  3 Reviews  ? keys  q quit"),
            "{lines:?}"
        );
    }

    #[test]
    fn clicks_on_the_chips_tabs_and_bar_act() {
        let mut app = app();
        let lines = screen(&mut app, 120, 30);
        let column = |line: &str, text: &str| {
            u16::try_from(line[..line.find(text).unwrap()].chars().count()).unwrap()
        };
        click(&mut app, column(&lines[1], "Staged"), 1);
        assert_eq!(app.context, Context::Staged);
        click(&mut app, column(&lines[0], "Reviews"), 0);
        assert_eq!(app.tab, Tab::Reviews);
        let lines = screen(&mut app, 120, 30);
        click(&mut app, column(&lines[29], "? keys"), 29);
        assert_eq!(app.overlay, Some(Overlay::Help));
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines.iter().any(|line| line.contains(" Keys ")),
            "{lines:?}"
        );
        click(&mut app, 5, 5);
        assert_eq!(app.overlay, None);
        screen(&mut app, 120, 30);
        click(&mut app, column(&lines[29], "q quit"), 29);
        assert!(app.quit);
    }

    #[test]
    fn a_narrow_terminal_shows_one_pane() {
        let mut app = app();
        let lines = screen(&mut app, 90, 30);
        assert!(lines[3].starts_with("   M lib.rs"), "{lines:?}");
        assert!(!lines[1].contains("unstaged"), "{lines:?}");
        app.focus = Focus::Main;
        let lines = screen(&mut app, 90, 30);
        assert!(lines[1].starts_with(" src/lib.rs  · unstaged"), "{lines:?}");
    }

    #[test]
    fn a_small_terminal_shows_only_a_message() {
        let mut app = app();
        let lines = screen(&mut app, 60, 20);
        assert!(lines[0].starts_with("GitMaster needs a terminal of at least 80x24"));
        assert!(lines.join(" ").contains("this one is 60x20"));
    }

    #[test]
    fn a_long_name_keeps_its_end() {
        assert_eq!(fit_end("src/very/long/name.rs", 10), "…g/name.rs");
        assert_eq!(fit_end("short.rs", 10), "short.rs");
    }
}
