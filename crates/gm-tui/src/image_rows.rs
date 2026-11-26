//! How images are laid out among a screen's rows (D268): a caption, then the image's cells
//! when the terminal draws images. On the File screen the image takes the screen's height; in
//! a diff, the before and the after stand side by side, each in half the width.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::images::{ImageKey, Picture};
use crate::kitty;
use crate::status::Item;
use crate::ui::App;

/// Columns of the gutter the image sits after, as code does.
const GUTTER: usize = 9;
/// The most rows an image of a diff takes, so the files after it stay in reach.
const DIFF_ROWS: u16 = 16;
/// The separator between the before and the after.
const SEPARATOR: &str = " │ ";

const DIM: Style = Style::new().fg(Color::DarkGray);

/// An image laid out: which, and the cells it spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub key: ImageKey,
    pub cols: u16,
    pub rows: u16,
}

/// The lines of an image row `width` columns wide, and the images they show; `None` for
/// any other row.
pub fn lines(app: &App, item: &Item, width: usize) -> Option<(Vec<Line<'static>>, Vec<Placed>)> {
    match item {
        Item::Image(key) => {
            let room_cols = width.saturating_sub(GUTTER);
            let room_rows = u16::try_from(app.body_rows.saturating_sub(2)).unwrap_or(u16::MAX);
            let placed = cells(app, key, room_cols, room_rows);
            let mut out = vec![Line::from(vec![
                Span::raw(" ".repeat(GUTTER)),
                Span::styled(cut(&caption(app, Some(key), "File"), room_cols), DIM),
            ])];
            if let Some(placed) = &placed {
                for row in 0..placed.rows {
                    let mut spans = vec![Span::raw(" ".repeat(GUTTER))];
                    spans.extend(kitty::row(id(app, key), row, placed.cols));
                    out.push(Line::from(spans));
                }
            }
            Some((out, placed.into_iter().collect()))
        }
        Item::ImagePair { before, after } => {
            let half = width.saturating_sub(GUTTER + SEPARATOR.chars().count()) / 2;
            let room_rows =
                DIFF_ROWS.min(u16::try_from(app.body_rows.saturating_sub(2)).unwrap_or(1));
            let side = |key: &Option<ImageKey>| {
                key.as_ref()
                    .and_then(|key| cells(app, key, half, room_rows))
            };
            let (left, right) = (side(before), side(after));
            let height = [&left, &right]
                .into_iter()
                .flatten()
                .map(|placed| placed.rows)
                .max()
                .unwrap_or(0);
            let mut out = vec![pair_line(
                half,
                Span::styled(cut(&caption(app, before.as_ref(), "Before"), half), DIM),
                Span::styled(cut(&caption(app, after.as_ref(), "After"), half), DIM),
            )];
            for row in 0..height {
                let cell = |placed: &Option<Placed>| match placed {
                    Some(placed) if row < placed.rows => {
                        kitty::row(id(app, &placed.key), row, placed.cols)
                            .pop()
                            .unwrap_or_default()
                    }
                    _ => Span::raw(""),
                };
                out.push(pair_line(half, cell(&left), cell(&right)));
            }
            Some((out, [left, right].into_iter().flatten().collect()))
        }
        _ => None,
    }
}

/// A line of a pair: the gutter, the left side filled to `half`, the separator, the right.
fn pair_line(half: usize, left: Span<'static>, right: Span<'static>) -> Line<'static> {
    let used = left.width();
    Line::from(vec![
        Span::raw(" ".repeat(GUTTER)),
        left,
        Span::raw(" ".repeat(half.saturating_sub(used))),
        Span::styled(SEPARATOR, DIM),
        right,
    ])
}

/// The cells image `key` takes within `cols` by `rows`, when it is drawn: decoded, and the
/// terminal draws images.
fn cells(app: &App, key: &ImageKey, cols: usize, rows: u16) -> Option<Placed> {
    app.graphics?;
    let Some(Picture::Ready(ready)) = app.repo.images.get(key) else {
        return None;
    };
    let room = (u16::try_from(cols).unwrap_or(u16::MAX), rows);
    let (cols, rows) = kitty::fit(ready.pixels, app.cell_px, room);
    Some(Placed {
        key: key.clone(),
        cols,
        rows,
    })
}

fn id(app: &App, key: &ImageKey) -> u32 {
    match app.repo.images.get(key) {
        Some(Picture::Ready(ready)) => ready.id,
        _ => 0,
    }
}

/// What a caption says of image `key`, after `label`: its format, size in pixels and in bytes,
/// or why it is not shown.
pub fn caption(app: &App, key: Option<&ImageKey>, label: &str) -> String {
    let Some(key) = key else {
        return format!("{label} · reading…");
    };
    let what = match app.repo.images.get(key) {
        None | Some(Picture::Reading | Picture::Decoding { .. }) => "reading…".to_owned(),
        Some(Picture::Missing) if label == "Before" => "not there: the image is new".to_owned(),
        Some(Picture::Missing) if label == "After" => "deleted".to_owned(),
        Some(Picture::Missing) => "there is no such file".to_owned(),
        Some(Picture::TooLarge(size)) => format!(
            "{}, larger than {}: not shown",
            kitty::size_label(*size),
            kitty::size_label(gm_schema::IMAGE_LIMIT)
        ),
        Some(Picture::Failed(reason)) => reason.trim_end_matches('.').to_owned(),
        Some(Picture::Ready(ready)) => format!(
            "{} {}×{} · {}{}",
            ready.format,
            ready.pixels.0,
            ready.pixels.1,
            kitty::size_label(ready.bytes),
            // Said once, on the File screen: a diff's halves have no room for it.
            if app.graphics.is_none() && label == "File" {
                " · images show in kitty and Ghostty"
            } else {
                ""
            }
        ),
    };
    format!("{label} · {what}")
}

/// `text` cut to `width` characters.
fn cut(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}
