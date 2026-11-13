//! Draws the **Reviews** tab (D236, D240): the list of threads in the sidebar, grouped by
//! who acts next, and the selected thread in the main pane: its text as it was and after
//! the agent, what the agent changed there, its messages and its buttons. The texts, the
//! changed lines and the captions come from the app's model, `gm_view::local_review`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use gm_schema::{ReviewComment, ReviewStatus};
use gm_view::local_review::{self, Filter, Version, counts};
use gm_view::review::{self, author_label, made_label, range_label, resolve_label, shown_range};

use crate::app::{Action, App, Focus, Hit};
use crate::render::{BOLD, DIM, ON, Strip};
use crate::render_threads::{badge_color, badge_word, chip_words, draw_composer, now_secs};
use crate::reviews::Entry;

pub fn draw_review_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let chips_area = Rect {
        height: 1,
        width: area
            .width
            .saturating_sub(u16::from(app.areas.main.is_some())),
        ..area
    };
    let tally = counts(&app.comments);
    let mut chips = Strip::new(chips_area);
    for filter in Filter::ALL {
        let style = if filter == app.reviews.page.filter() {
            ON
        } else {
            DIM
        };
        chips.text(" ", Style::new()).button(
            format!(" {} {} ", filter.label(), tally.of(filter)),
            style,
            Action::ReviewFilter(filter),
        );
    }
    let (spans, buttons) = chips.into_parts();
    frame.render_widget(Paragraph::new(Line::from(spans)), chips_area);
    app.hits.extend(buttons);

    let Some(list) = app.areas.list() else {
        return;
    };
    if app.reviews.entries.is_empty() {
        let text = if app.comments.is_empty() {
            " No threads yet. Select lines in the Diff tab and press c."
        } else {
            " No thread matches this filter."
        };
        frame.render_widget(
            Paragraph::new(text).style(DIM).wrap(Wrap { trim: false }),
            list,
        );
        return;
    }
    let height = usize::from(list.height);
    let view = &mut app.reviews;
    if view.cursor < view.scroll {
        view.scroll = view.cursor.saturating_sub(1);
    } else if view.cursor >= view.scroll + height {
        view.scroll = view.cursor + 1 - height;
    }
    let focused = app.focus == Focus::Sidebar;
    let width = usize::from(list.width);
    let mut lines = Vec::new();
    for (index, entry) in view
        .entries
        .iter()
        .enumerate()
        .skip(view.scroll)
        .take(height)
    {
        let y = list.y + u16::try_from(index - view.scroll).unwrap_or(u16::MAX);
        app.hits.push(Hit {
            area: Rect::new(list.x, y, list.width, 1),
            action: Action::ReviewRow(index),
        });
        let line = match entry {
            Entry::Group(turn, count) => Line::from(vec![
                Span::styled(format!(" {}", turn.title()), BOLD),
                Span::styled(format!(" · {count}"), DIM),
            ]),
            Entry::Thread(id) => {
                let Some(comment) = app.comments.iter().find(|comment| comment.id == *id) else {
                    continue;
                };
                let mut line = thread_entry(comment, width);
                if index == view.cursor {
                    line = line.style(if focused {
                        ON
                    } else {
                        Style::new().add_modifier(Modifier::BOLD)
                    });
                }
                line
            }
        };
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), list);
}

/// One thread in the list: its status, its file and lines, and the start of its last
/// message, cut to `width`.
fn thread_entry(comment: &ReviewComment, width: usize) -> Line<'static> {
    let badge = review::badge(comment);
    let (start, end) = shown_range(comment);
    let name = comment.path.rsplit('/').next().unwrap_or(&comment.path);
    let place = if start == end {
        format!("{name}:{start}")
    } else {
        format!("{name}:{start}–{end}")
    };
    let last = comment
        .replies
        .iter()
        .max_by_key(|reply| (reply.created_at, reply.id))
        .map_or(comment.body.as_str(), |reply| reply.body.as_str());
    let icon = badge_word(badge);
    let icon = icon.split(' ').next().unwrap_or_default().to_owned();
    let head = format!("   {icon} {place}  ");
    let room = width.saturating_sub(head.chars().count());
    let summary: String = local_review::summary(last).chars().take(room).collect();
    Line::from(vec![
        Span::styled(head, Style::new().fg(badge_color(badge))),
        Span::styled(summary, DIM),
    ])
}

pub fn draw_review_thread(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(id) = app.reviews.page.selected_id() else {
        let text = if app.comments.is_empty() {
            "No threads yet."
        } else {
            "Select a thread in the list."
        };
        frame.render_widget(Paragraph::new(text).style(DIM), area);
        return;
    };
    let Some(comment) = app
        .comments
        .iter()
        .find(|comment| comment.id == id)
        .cloned()
    else {
        return;
    };
    let width = usize::from(area.width);
    let row = |at: usize| Rect {
        y: area.y + u16::try_from(at).unwrap_or(u16::MAX),
        height: 1,
        ..area
    };
    let mut hits = Vec::new();
    let mut lines = vec![thread_head(&comment)];
    lines.push(version_bar(app, &comment, row(lines.len()), &mut hits));
    if local_review::reads_change(&comment)
        && let Some((added, removed)) = app.reviews.measured()
    {
        lines.push(Line::from(vec![
            Span::styled("The agent changed ", DIM),
            Span::styled(format!("+{added}"), Style::new().fg(Color::Green)),
            Span::styled(format!(" -{removed}"), Style::new().fg(Color::Red)),
            Span::styled(" in these lines, as GitMaster measured it.", DIM),
        ]));
    }
    lines.extend(version_lines(
        app,
        &comment,
        app.reviews.page.version(),
        width,
    ));
    lines.push(Line::default());
    lines.extend(messages(&comment, width));
    lines.push(Line::default());
    lines.push(thread_buttons(&comment, row(lines.len()), &mut hits));
    // Lines past the pane are cut; the buttons drawn below it are dropped with them.
    let bottom = area.y + area.height;
    hits.retain(|hit| hit.area.y < bottom);
    frame.render_widget(Paragraph::new(lines), area);
    app.hits.extend(hits);
    if let Some(composer) = app.composer.as_ref() {
        draw_composer(frame, composer, area, bottom, &mut app.hits);
    }
}

/// The head of a thread: status, number, file, lines, label and chips.
fn thread_head(comment: &ReviewComment) -> Line<'static> {
    let badge = review::badge(comment);
    let (start, end) = shown_range(comment);
    let mut head = vec![
        Span::styled(
            badge_word(badge),
            Style::new()
                .fg(badge_color(badge))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · #{} · ", comment.id), DIM),
        Span::styled(comment.path.clone(), BOLD),
        Span::styled(format!(" · {}", range_label(start, end)), DIM),
    ];
    for word in chip_words(&review::chips(comment)) {
        head.push(Span::styled(
            format!(" · {word}"),
            Style::new().fg(Color::Cyan),
        ));
    }
    Line::from(head)
}

/// The Before and After buttons with the caption of the text shown, as the app's page.
fn version_bar(
    app: &App,
    comment: &ReviewComment,
    area: Rect,
    hits: &mut Vec<Hit>,
) -> Line<'static> {
    let version = app.reviews.page.version();
    let mut strip = Strip::new(area);
    for choice in [Version::Before, Version::After] {
        let style = if choice == version { ON } else { DIM };
        strip
            .button(
                format!(" {} ", choice.label()),
                style,
                Action::ReviewVersion(choice),
            )
            .text(" ", Style::new());
    }
    strip.text(format!(" {}", local_review::caption(version, comment)), DIM);
    let (spans, buttons) = strip.into_parts();
    hits.extend(buttons);
    Line::from(spans)
}

/// The comment, then every reply, oldest first, each under its author and time.
fn messages(comment: &ReviewComment, width: usize) -> Vec<Line<'static>> {
    let wrap = |body: &str| {
        review::wrap(body, width.saturating_sub(2).max(10))
            .into_iter()
            .map(|text| Line::raw(format!("  {text}")))
            .collect::<Vec<_>>()
    };
    let root_author = comment.author.as_deref().unwrap_or("human");
    let mut lines = vec![Line::styled(
        review::reply_head(root_author, comment.created_at, now_secs()),
        BOLD,
    )];
    lines.extend(wrap(&comment.body));
    for reply in review::thread(&comment.replies, true).shown {
        let style = if review::is_agent_reply(reply) {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            BOLD
        };
        lines.push(Line::styled(
            format!(
                "{} · {}",
                author_label(&reply.author),
                made_label(now_secs(), reply.created_at)
            ),
            style,
        ));
        lines.extend(wrap(&reply.body));
    }
    lines
}

/// What can be done with the thread: reply, resolve or reopen, delete, see it in the diff.
fn thread_buttons(comment: &ReviewComment, area: Rect, hits: &mut Vec<Hit>) -> Line<'static> {
    let id = comment.id;
    let resolved = comment.status == ReviewStatus::Resolved;
    let mut strip = Strip::new(area);
    if review::can_reply(comment.status) {
        strip
            .button("[reply]", Style::new().fg(Color::Blue), Action::Reply(id))
            .text(" ", Style::new());
    }
    let label = format!(
        "[{}]",
        resolve_label(resolved, review::is_note(comment)).to_lowercase()
    );
    let action = if resolved {
        Action::Reopen(id)
    } else {
        Action::Resolve(id)
    };
    strip
        .button(label, Style::new().fg(Color::Green), action)
        .text(" ", Style::new())
        .button("[delete]", Style::new().fg(Color::Red), Action::Delete(id))
        .text(" ", Style::new())
        .button(
            "[go to diff]",
            Style::new().fg(Color::Blue),
            Action::GoToDiff(id),
        );
    let (spans, buttons) = strip.into_parts();
    hits.extend(buttons);
    Line::from(spans)
}

/// The text of `version`: each line with its number in its file, the lines that changed
/// between the two texts marked, or what the app says when there is none.
fn version_lines(
    app: &App,
    comment: &ReviewComment,
    version: Version,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(text) = app.reviews.page.lines(version) else {
        return vec![Line::styled("Reading…", DIM)];
    };
    if text.is_empty() {
        return vec![Line::styled(
            local_review::empty_text(version, comment),
            DIM,
        )];
    }
    let marks = app.reviews.page.marks(version);
    let first = app.reviews.page.first_line(comment, version);
    let (sign, color) = match version {
        Version::Before => ("-", Color::Red),
        Version::After => ("+", Color::Green),
    };
    text.iter()
        .enumerate()
        .map(|(at, code)| {
            let number = first + u32::try_from(at).unwrap_or(u32::MAX);
            let marked = marks.get(at).copied().unwrap_or(false);
            let (mark, style) = if marked {
                (sign, Style::new().fg(color))
            } else {
                (" ", DIM)
            };
            let code: String = code
                .replace('\t', "    ")
                .chars()
                .take(width.saturating_sub(9))
                .collect();
            Line::from(vec![
                Span::styled(format!("{number:>5} {mark} "), style),
                Span::styled(
                    code,
                    if marked {
                        Style::new().fg(color)
                    } else {
                        Style::new()
                    },
                ),
            ])
        })
        .collect()
}
