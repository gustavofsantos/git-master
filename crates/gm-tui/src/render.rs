//! Draws the model. Colors are the terminal's own ANSI colors (D235 (7)), so the companion
//! follows the terminal's theme and tmux's.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, Focus, Group, ListRow, MIN_HEIGHT, MIN_WIDTH, areas};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();
    app.areas = areas(size, app.focus);
    let (Some(status), files, diff) = (app.areas.status, app.areas.files, app.areas.diff) else {
        too_small(frame, size);
        return;
    };
    if let Some(area) = files {
        draw_files(frame, app, area);
    }
    if let Some(area) = diff {
        draw_diff(frame, app, area);
    }
    draw_status(frame, app, status);
}

fn too_small(frame: &mut Frame, size: Rect) {
    let text = format!(
        "GitMaster needs a terminal of at least {MIN_WIDTH}x{MIN_HEIGHT}; this one is {}x{}. \
         Make it larger, or press q to quit.",
        size.width, size.height
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), size);
}

fn draw_files(frame: &mut Frame, app: &mut App, area: Rect) {
    // The right border separates the list from the diff when both show.
    let split = app.areas.diff.is_some();
    let block = if split {
        Block::new()
            .borders(Borders::RIGHT)
            .border_style(Style::new().fg(Color::DarkGray))
    } else {
        Block::new()
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.rows.is_empty() {
        let empty = Paragraph::new("No changes.").style(Style::new().fg(Color::DarkGray));
        frame.render_widget(empty, inner);
        return;
    }
    let height = usize::from(inner.height);
    app.keep_cursor_visible(height);
    let focused = app.focus == Focus::Files;
    let width = usize::from(inner.width);
    let lines: Vec<Line> = app
        .rows
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(height)
        .map(|(index, row)| match row {
            ListRow::Header(group, count) => Line::from(vec![
                Span::styled(group.title(), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({count})"), Style::new().fg(Color::DarkGray)),
            ]),
            ListRow::File(file) => {
                let letter = Span::styled(
                    format!(" {} ", file.letter),
                    Style::new().fg(letter_color(file.letter, file.group)),
                );
                let path = Span::raw(fit_path(&file.path, width.saturating_sub(3)));
                let mut line = Line::from(vec![letter, path]);
                if index == app.cursor {
                    line = line.style(if focused {
                        Style::new().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    });
                }
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn letter_color(letter: char, group: Group) -> Color {
    match (letter, group) {
        ('U', _) => Color::Magenta,
        ('?', _) => Color::Cyan,
        ('D', _) => Color::Red,
        (_, Group::Staged) => Color::Green,
        (_, Group::Unstaged) => Color::Yellow,
    }
}

/// `path` in at most `width` columns: whole when it fits, else its end after `…`, so the
/// file name stays readable.
fn fit_path(path: &str, width: usize) -> String {
    let count = path.chars().count();
    if count <= width {
        return path.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = path.chars().skip(count - (width - 1)).collect();
    format!("…{tail}")
}

fn draw_diff(frame: &mut Frame, app: &App, area: Rect) {
    let text = match app.selected() {
        Some(file) => vec![
            Line::from(vec![
                Span::styled(
                    file.path.as_str(),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", file.group.title().to_lowercase()),
                    Style::new().fg(Color::DarkGray),
                ),
            ]),
            Line::default(),
            Line::styled(
                "The diff is not shown yet.",
                Style::new().fg(Color::DarkGray),
            ),
        ],
        None => vec![Line::styled(
            "Nothing to show: the working tree has no changes.",
            Style::new().fg(Color::DarkGray),
        )],
    };
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(1),
        ..area
    };
    frame.render_widget(Paragraph::new(text), inner);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let branch = &app.status.branch;
    let mut spans = vec![Span::styled(
        format!(" {} ", branch.head.as_deref().unwrap_or("(detached)")),
        Style::new().add_modifier(Modifier::REVERSED),
    )];
    if let Some(upstream) = &branch.upstream {
        spans.push(Span::raw(format!(" → {upstream}")));
        if branch.ahead > 0 {
            spans.push(Span::styled(
                format!(" ↑{}", branch.ahead),
                Style::new().fg(Color::Green),
            ));
        }
        if branch.behind > 0 {
            spans.push(Span::styled(
                format!(" ↓{}", branch.behind),
                Style::new().fg(Color::Red),
            ));
        }
    }
    let count = app.file_count();
    spans.push(Span::raw(format!(
        "  {count} {}",
        if count == 1 { "change" } else { "changes" }
    )));
    if let Some(notice) = &app.notice {
        spans.push(Span::styled(
            format!("  {notice}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    let hints = match app.focus {
        Focus::Files => "j/k move  Tab diff  q quit ",
        Focus::Diff => "Tab files  q quit ",
    };
    let used: usize = spans.iter().map(Span::width).sum();
    let gap = usize::from(area.width).saturating_sub(used + hints.len());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(hints, Style::new().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Focus;
    use gm_schema::{BranchInfo, Change, RepoStatus, StatusEntry};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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

    #[test]
    fn a_wide_terminal_shows_the_list_the_diff_and_the_status_bar() {
        let mut app = app();
        let lines = screen(&mut app, 120, 30);
        assert!(lines[0].starts_with("Unstaged (1)"), "{lines:?}");
        assert!(lines[1].starts_with(" M src/lib.rs"), "{lines:?}");
        assert!(lines[2].starts_with("Staged (1)"), "{lines:?}");
        assert!(lines[3].starts_with(" A README.md"), "{lines:?}");
        assert!(lines[0].contains("src/lib.rs  unstaged"), "{lines:?}");
        let status = &lines[29];
        assert!(
            status.starts_with(" feature  → origin/feature ↑2  2 changes"),
            "{status}"
        );
        assert!(status.ends_with("q quit"), "{status}");
    }

    #[test]
    fn a_narrow_terminal_shows_one_pane() {
        let mut app = app();
        let lines = screen(&mut app, 90, 30);
        assert!(lines[1].starts_with(" M src/lib.rs"), "{lines:?}");
        assert!(!lines[0].contains("unstaged"), "{lines:?}");
        app.focus = Focus::Diff;
        let lines = screen(&mut app, 90, 30);
        assert!(lines[0].starts_with(" src/lib.rs  unstaged"), "{lines:?}");
    }

    #[test]
    fn a_small_terminal_shows_only_a_message() {
        let mut app = app();
        let lines = screen(&mut app, 60, 20);
        assert!(lines[0].starts_with("GitMaster needs a terminal of at least 80x24"));
        assert!(lines.join(" ").contains("this one is 60x20"));
    }

    #[test]
    fn a_long_path_keeps_its_end() {
        assert_eq!(fit_path("src/very/long/name.rs", 10), "…g/name.rs");
        assert_eq!(fit_path("short.rs", 10), "short.rs");
    }
}
