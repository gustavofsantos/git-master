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
use crate::icons;
use crate::render_diff::draw_diff;

pub const DIM: Style = Style::new().fg(Color::DarkGray);
pub const ON: Style = Style::new().add_modifier(Modifier::REVERSED);
pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

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
pub struct Strip<'a> {
    area: Rect,
    x: u16,
    spans: Vec<Span<'a>>,
    hits: Vec<Hit>,
}

impl<'a> Strip<'a> {
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            x: area.x,
            spans: Vec::new(),
            hits: Vec::new(),
        }
    }

    pub fn text(&mut self, text: impl Into<std::borrow::Cow<'a, str>>, style: Style) -> &mut Self {
        let span = Span::styled(text, style);
        self.x = self
            .x
            .saturating_add(u16::try_from(span.width()).unwrap_or(u16::MAX));
        self.spans.push(span);
        self
    }

    pub fn button(
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

    /// The spans and the buttons, for a line drawn with others.
    pub fn into_parts(self) -> (Vec<Span<'a>>, Vec<Hit>) {
        (self.spans, self.hits)
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
            Tab::Reviews => format!(
                " {} {} {} ",
                icons::TAB_REVIEWS,
                tab.title(),
                app.unresolved()
            ),
            Tab::Diff => format!(" {} {} ", icons::TAB_DIFF, tab.title()),
            Tab::File => format!(" {} {} ", icons::TAB_FILE, tab.title()),
        };
        let style = if tab == app.tab { ON } else { DIM };
        strip
            .text(" ", Style::new())
            .button(title, style, Action::Tab(tab));
    }
    let branch = &app.status.branch;
    let mut right = vec![(
        format!(
            "{} {}",
            icons::BRANCH,
            branch.head.as_deref().unwrap_or("(detached)")
        ),
        Style::new().fg(Color::Blue),
    )];
    if let Some(upstream) = &branch.upstream {
        right.push((format!(" {} {upstream}", icons::UPSTREAM), DIM));
        if branch.ahead > 0 {
            right.push((
                format!(" {}{}", icons::AHEAD, branch.ahead),
                Style::new().fg(Color::Green),
            ));
        }
        if branch.behind > 0 {
            right.push((
                format!(" {}{}", icons::BEHIND, branch.behind),
                Style::new().fg(Color::Red),
            ));
        }
    }
    right.push((" ".to_owned(), Style::new()));
    if let Some(notice) = app.notice.as_ref().or(app.watch_notice.as_ref()) {
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
        let empty = match (app.context, &app.range_error) {
            // The engine's message ends with advice about the app's base picker, which the
            // companion has not: its first sentence is the reason.
            (Context::Branch, Some(reason)) => format!(" {}", first_sentence(reason)),
            (Context::Branch, None) if app.range.is_none() => " Reading the branch…".to_owned(),
            (context, _) => format!(" No {} changes.", context.title().to_lowercase()),
        };
        frame.render_widget(
            Paragraph::new(empty).style(DIM).wrap(Wrap { trim: false }),
            list,
        );
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
        let button = match (row, app.context) {
            (Row::File(file), context) if Some(&file.path) == selected.as_ref() => match context {
                Context::Unstaged => Some("[stage]"),
                Context::Staged => Some("[unstage]"),
                Context::Branch => None,
            },
            _ => None,
        };
        if let Some(label) = button {
            let label_width = u16::try_from(label.len()).unwrap_or(u16::MAX);
            app.hits.push(Hit {
                area: Rect::new(
                    (list.x + list.width).saturating_sub(label_width + 1),
                    y,
                    label_width,
                    1,
                ),
                action: Action::StageFile,
            });
        }
        let mut line = sidebar_line(row, width, button);
        if index == app.cursor && focused {
            line = line.style(ON);
        } else if matches!(row, Row::File(file) if Some(&file.path) == selected.as_ref()) {
            line = line.style(BOLD);
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), list);
}

fn sidebar_line(row: &Row, width: usize, button: Option<&'static str>) -> Line<'static> {
    match row {
        Row::Dir { dir, files, folded } => {
            let name = if dir.is_empty() { "./" } else { dir.as_str() };
            let text = if *folded {
                format!(" {} {name} ({files})", icons::FOLDER)
            } else {
                format!(" {} {name}", icons::FOLDER_OPEN)
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
                    format!(" -{deleted}"),
                    Style::new().fg(Color::Red),
                ));
            }
            if file.threads > 0 {
                tail.push(Span::styled(
                    format!(" {}{}", icons::THREADS, file.threads),
                    Style::new().fg(Color::Yellow),
                ));
            }
            if let Some(label) = button {
                let color = if label == "[stage]" {
                    Color::Green
                } else {
                    Color::Yellow
                };
                tail.push(Span::raw(" "));
                tail.push(Span::styled(label, Style::new().fg(color)));
            }
            tail.push(Span::raw(" "));
            let tail_width: usize = tail.iter().map(Span::width).sum();
            let letter = Span::styled(
                format!("   {} ", file.letter),
                Style::new().fg(letter_color(file.letter)),
            );
            let icon = Span::raw(format!("{} ", icons::file(&file.name)));
            let room = width.saturating_sub(letter.width() + icon.width() + tail_width);
            let name = fit_end(&file.name, room);
            let gap = room.saturating_sub(Span::raw(name.as_str()).width());
            let mut spans = vec![letter, icon, Span::raw(name), Span::raw(" ".repeat(gap))];
            spans.extend(tail);
            Line::from(spans)
        }
    }
}

/// The first sentence of `text`, with its full stop.
fn first_sentence(text: &str) -> String {
    match text.find(". ") {
        Some(end) => text[..=end].to_owned(),
        None => text.to_owned(),
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

fn draw_main(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(1),
        ..area
    };
    let placeholder = match app.tab {
        Tab::Diff => return draw_diff(frame, app, inner),
        Tab::File => "The File tab is not available yet.",
        Tab::Reviews => "The Reviews tab is not available yet.",
    };
    frame.render_widget(Paragraph::new(Line::styled(placeholder, DIM)), inner);
}

/// The buttons of the bottom bar, each with its key, for where the person is.
fn bar_buttons(app: &App) -> Vec<(&'static str, String, Action)> {
    if app.composer.is_some() {
        return vec![
            ("Enter", "save".to_owned(), Action::SaveComment),
            ("Esc", "cancel".to_owned(), Action::CancelComment),
        ];
    }
    let focus = match app.focus {
        Focus::Sidebar => "main pane",
        Focus::Main => "files",
    };
    let mut buttons = vec![
        (
            "t",
            app.context.next().title().to_owned(),
            Action::OtherContext,
        ),
        ("Tab", focus.to_owned(), Action::ToggleFocus),
    ];
    let selecting = app.diff.as_ref().is_some_and(|view| view.anchor.is_some());
    if app.focus == Focus::Main && app.tab == Tab::Diff && selecting {
        match app.context {
            Context::Unstaged => {
                buttons.push(("s", "stage lines".to_owned(), Action::StageSelection));
            }
            Context::Staged => {
                buttons.push(("u", "unstage lines".to_owned(), Action::StageSelection));
            }
            Context::Branch => {}
        }
        buttons.push(("c", "comment".to_owned(), Action::Comment));
        buttons.push(("Esc", "clear".to_owned(), Action::ClearSelection));
    } else if app.focus == Focus::Main && app.tab == Tab::Diff && app.diff.is_some() {
        if let Some(id) = app.thread_at_cursor() {
            let resolved = app
                .comment(id)
                .is_some_and(|comment| comment.status == gm_schema::ReviewStatus::Resolved);
            buttons.push(("r", "reply".to_owned(), Action::Reply(id)));
            if resolved {
                buttons.push(("o", "reopen".to_owned(), Action::Reopen(id)));
            } else {
                buttons.push(("R", "resolve".to_owned(), Action::Resolve(id)));
            }
            buttons.push(("D", "delete".to_owned(), Action::Delete(id)));
        }
        buttons.push(("c", "comment".to_owned(), Action::Comment));
        match app.context {
            Context::Unstaged => {
                buttons.push(("s", "stage hunk".to_owned(), Action::StageCursorHunk));
            }
            Context::Staged => {
                buttons.push(("u", "unstage hunk".to_owned(), Action::StageCursorHunk));
            }
            Context::Branch => {}
        }
        buttons.push(("J", "next hunk".to_owned(), Action::JumpHunk(true)));
        buttons.push(("K", "previous hunk".to_owned(), Action::JumpHunk(false)));
    }
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
    ("j k Down Up", "move in the sidebar or the diff"),
    ("Ctrl+D Ctrl+U", "half a screen down or up"),
    ("g G", "first or last row"),
    ("Enter", "fold a folder, or show a file's diff"),
    ("Space", "fold or unfold a folder"),
    ("l h", "unfold or fold a folder"),
    ("J K", "next or previous hunk, in the diff"),
    ("V", "start or end a line selection, in the diff"),
    ("o", "go to the other end of the selection"),
    ("Esc", "clear the selection"),
    (
        "s u",
        "stage or unstage the file, the selection or the hunk",
    ),
    ("c", "comment on the selection or the line"),
    ("} {", "next or previous thread"),
    ("Enter", "unfold or fold the thread at the cursor"),
    ("r R o D", "reply, resolve, reopen, delete the thread"),
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
            lines[0].starts_with(&format!(
                " gitmaster   {} Diff   {} File   {} Reviews 0",
                icons::TAB_DIFF,
                icons::TAB_FILE,
                icons::TAB_REVIEWS
            )),
            "{lines:?}"
        );
        assert!(
            lines[0].ends_with(&format!(
                "{} feature {} origin/feature {}2",
                icons::BRANCH,
                icons::UPSTREAM,
                icons::AHEAD
            )),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("  Unstaged 1   Staged 1"), "{lines:?}");
        assert!(
            lines[2].starts_with(&format!(" {} src", icons::FOLDER_OPEN)),
            "{lines:?}"
        );
        assert!(
            lines[3].starts_with(&format!("   M {} lib.rs", icons::file("lib.rs"))),
            "{lines:?}"
        );
        assert!(lines[1].contains("│ src/lib.rs · unstaged"), "{lines:?}");
        assert!(lines[3].contains("│  Reading the diff…"), "{lines:?}");
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

    fn diff_event() -> gm_schema::Event {
        use gm_schema::{DiffLine, DiffSide, Event, FileDiff, Hunk, HunkId, LineKind, OperationId};
        let line = |kind, text: &str, new_no| DiffLine {
            kind,
            old_no: None,
            new_no,
            text: text.to_owned(),
            no_newline: false,
        };
        Event::DiffLoaded {
            id: OperationId(5),
            side: DiffSide::Unstaged,
            diff: FileDiff {
                path: "src/lib.rs".to_owned(),
                old_path: None,
                binary: false,
                hunks: vec![Hunk {
                    id: HunkId("h1".to_owned()),
                    old_start: 3,
                    old_lines: 2,
                    new_start: 3,
                    new_lines: 2,
                    header: "fn main() {".to_owned(),
                    lines: vec![
                        line(LineKind::Context, "    let a = 1;", Some(3)),
                        line(LineKind::Removed, "    old();", None),
                        line(LineKind::Added, "\tnew();", Some(4)),
                    ],
                }],
            },
        }
    }

    fn drag(app: &mut App, column: u16, row: u16) {
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn a_drag_selects_lines_and_stage_lines_stages_their_changes() {
        use gm_schema::{CommandKind, HunkId, StageTarget};
        let mut app = app();
        app.apply(&diff_event());
        app.take_commands();
        screen(&mut app, 120, 30);
        click(&mut app, 60, 3); // the context line
        drag(&mut app, 60, 4);
        drag(&mut app, 60, 5); // the added line
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines[1].ends_with("│ src/lib.rs · 3–4 · 3 lines  [comment] [stage lines] [clear]"),
            "{lines:?}"
        );
        assert!(
            lines[3].contains(&format!("│ {}   3", icons::SELECTED)),
            "{lines:?}"
        );
        assert!(
            lines[5].contains(&format!("│ {}   4 +", icons::SELECTED)),
            "{lines:?}"
        );
        let column = u16::try_from(
            lines[1]
                .find("[stage lines]")
                .map_or(0, |at| lines[1][..at].chars().count()),
        )
        .unwrap();
        click(&mut app, column + 2, 1);
        assert_eq!(
            app.take_commands(),
            [CommandKind::Stage {
                target: StageTarget::Lines {
                    path: "src/lib.rs".to_owned(),
                    hunk: HunkId("h1".to_owned()),
                    lines: vec![1, 2],
                },
            }]
        );
        assert!(app.diff.as_ref().unwrap().anchor.is_none());
    }

    #[test]
    fn a_comment_is_written_with_its_type_and_drawn_under_its_line_with_its_buttons() {
        use gm_schema::{
            CommandKind, CommentLabel, Event, OperationId, OperationState, ReviewLocation,
            ReviewStatus,
        };
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut app = app();
        app.apply(&diff_event());
        app.take_commands();
        app.focus = Focus::Main;
        // The cursor starts on the first line (3); select down to the added line (4).
        app.on_key(key(KeyCode::Char('V')));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char('c')));
        assert!(app.composer.is_some(), "{:?}", app.notice);
        for letter in "Use ? here.".chars() {
            app.on_key(key(KeyCode::Char(letter)));
        }
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Comment on lines 3–4")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("type  change   question   nit")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("Use ? here.")),
            "{lines:?}"
        );
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.take_commands(),
            [CommandKind::AddReviewComment {
                path: "src/lib.rs".to_owned(),
                start_line: 3,
                end_line: 4,
                body: "Use ? here.".to_owned(),
                label: Some(CommentLabel::Issue),
            }]
        );
        // The engine takes it, lists it, and the write ends: the box closes.
        app.sent(
            OperationId(8),
            &CommandKind::ListReviewComments {
                author: gm_schema::ReviewAuthorFilter::Any,
            },
        );
        let mut listed = crate::app::tests::comment(4, "src/lib.rs", ReviewStatus::Open);
        listed.body = "Use ? here.".to_owned();
        listed.location = Some(ReviewLocation {
            start_line: 3,
            end_line: 4,
        });
        listed.label = Some(CommentLabel::Issue);
        if let Some(composer) = app.composer.as_mut() {
            composer.pending = Some(OperationId(9));
        }
        app.apply(&Event::ReviewCommentsListed {
            id: OperationId(9),
            comments: vec![listed],
        });
        app.apply(&Event::OperationChanged {
            id: OperationId(9),
            actor: gm_schema::Actor::Human,
            state: OperationState::Done,
            detail: None,
        });
        assert!(app.composer.is_none());
        let lines = screen(&mut app, 120, 30);
        let head = lines
            .iter()
            .position(|line| line.contains(&format!("│ {} Open · lines 3–4", icons::OPEN)))
            .expect("the card's head");
        assert!(lines[head - 1].contains("4 +"), "{lines:?}");
        assert!(
            lines[head].contains("issue  [reply] [resolve] [delete]"),
            "{lines:?}"
        );
        assert!(lines[head + 1].ends_with("│ Use ? here."), "{lines:?}");
        assert!(lines[3].contains("┃   3"), "{lines:?}");
        let column = u16::try_from(
            lines[head]
                .find("[resolve]")
                .map_or(0, |at| lines[head][..at].chars().count()),
        )
        .unwrap();
        click(&mut app, column + 2, u16::try_from(head).unwrap());
        assert_eq!(
            app.take_commands(),
            [CommandKind::ResolveReviewComment { id: 4 }]
        );
    }

    #[test]
    fn the_selected_files_stage_button_stages_the_file() {
        use gm_schema::{CommandKind, StageTarget};
        let mut app = app();
        app.take_commands();
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines[3].starts_with(&format!("   M {} lib.rs", icons::file("lib.rs"))),
            "{lines:?}"
        );
        assert!(lines[3].contains("[stage] │"), "{lines:?}");
        let column = u16::try_from(lines[3].find("[stage]").unwrap()).unwrap();
        click(&mut app, column + 1, 3);
        assert_eq!(
            app.take_commands(),
            [CommandKind::Stage {
                target: StageTarget::File {
                    path: "src/lib.rs".to_owned(),
                },
            }]
        );
    }

    #[test]
    fn the_diff_shows_hunk_bars_and_lines_and_its_stage_button_stages_the_hunk() {
        use gm_schema::{CommandKind, HunkId, StageTarget};
        let mut app = app();
        app.take_commands();
        app.apply(&diff_event());
        let lines = screen(&mut app, 120, 30);
        assert!(
            lines[1].ends_with("│ src/lib.rs  +1 -1 · hunk 1/1 · unstaged"),
            "{lines:?}"
        );
        assert!(lines[2].ends_with("│ @@ fn main() {  [stage]"), "{lines:?}");
        assert!(lines[3].ends_with("│     3       let a = 1;"), "{lines:?}");
        assert!(lines[4].ends_with("│       -     old();"), "{lines:?}");
        assert!(lines[5].ends_with("│     4 +     new();"), "{lines:?}");
        let column = u16::try_from(lines[2].chars().count() - 3).unwrap();
        click(&mut app, column, 2);
        assert_eq!(
            app.take_commands(),
            [CommandKind::Stage {
                target: StageTarget::Hunk {
                    path: "src/lib.rs".to_owned(),
                    hunk: HunkId("h1".to_owned()),
                },
            }]
        );
    }

    #[test]
    fn a_narrow_terminal_shows_one_pane() {
        let mut app = app();
        let lines = screen(&mut app, 90, 30);
        assert!(
            lines[3].starts_with(&format!("   M {} lib.rs", icons::file("lib.rs"))),
            "{lines:?}"
        );
        assert!(!lines[1].contains("unstaged"), "{lines:?}");
        app.focus = Focus::Main;
        let lines = screen(&mut app, 90, 30);
        assert!(lines[1].starts_with(" src/lib.rs · unstaged"), "{lines:?}");
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
