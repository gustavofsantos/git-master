//! The kitty graphics protocol, with Unicode placeholders (D268). An image is sent to the
//! terminal once, under an id, with a virtual placement of so many columns and rows; the screen
//! then draws it through ordinary cells, each the placeholder character with two diacritics for
//! its row and column and the image id in its foreground color. The cells move, scroll and clear
//! like text, so the drawing needs nothing special, and tmux carries them as it carries text:
//! only the image itself goes through tmux's passthrough. Pure: what writes these to the
//! terminal lives in `lib.rs`.

use std::fmt::Write as _;

use base64::Engine as _;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// The character of an image cell.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The diacritics that number rows and columns, in order: kitty's `rowcolumn-diacritics.txt`.
/// An image is at most this many cells wide and tall.
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186,
    0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// The most cells an image spans either way.
pub const MAX_CELLS: u16 = 297;

/// Base64 characters per chunk of a transmission, as the protocol allows.
const CHUNK: usize = 4096;

/// How images reach the terminal: wrapped for tmux's passthrough or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Support {
    pub tmux: bool,
}

/// What `gitmaster.images` asks for (D268).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wanted {
    /// Images where the terminal is known to draw them.
    #[default]
    Auto,
    /// Images, whatever the terminal says it is.
    Kitty,
    /// Never images: a line of text names them.
    Off,
}

/// Whether images are drawn, from the environment `var` reads: kitty and Ghostty draw them
/// with placeholders, and tmux passes them on (D268). The variables kitty and Ghostty set
/// survive into tmux, whose own `TERM` hides the terminal.
pub fn detect(wanted: Wanted, var: impl Fn(&str) -> Option<String>) -> Option<Support> {
    let tmux = var("TMUX").is_some_and(|v| !v.is_empty());
    let known = || {
        var("KITTY_WINDOW_ID").is_some()
            || var("GHOSTTY_RESOURCES_DIR").is_some()
            || var("TERM").is_some_and(|t| t == "xterm-kitty" || t == "xterm-ghostty")
            || var("TERM_PROGRAM").is_some_and(|t| t == "ghostty" || t == "kitty")
    };
    match wanted {
        Wanted::Off => None,
        Wanted::Kitty => Some(Support { tmux }),
        Wanted::Auto => known().then_some(Support { tmux }),
    }
}

/// One command of the protocol, wrapped for tmux when it runs inside it: tmux passes on what
/// follows `ESC P tmux;` with each escape doubled.
fn command(support: Support, keys: &str, payload: &str) -> String {
    let plain = format!("\x1b_G{keys};{payload}\x1b\\");
    if support.tmux {
        format!("\x1bPtmux;{}\x1b\\", plain.replace('\x1b', "\x1b\x1b"))
    } else {
        plain
    }
}

/// Sends the PNG `png` under `id` and places it virtually over `cols` by `rows` cells, for
/// placeholders to show (`a=T,U=1`). Quiet: the terminal answers nothing (`q=2`).
pub fn transmit(support: Support, id: u32, png: &[u8], cols: u16, rows: u16) -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&str> = if data.is_empty() {
        vec![""]
    } else {
        data.as_bytes()
            .chunks(CHUNK)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
            .collect()
    };
    let last = chunks.len() - 1;
    let mut out = String::new();
    for (at, chunk) in chunks.into_iter().enumerate() {
        let more = u8::from(at < last);
        let keys = if at == 0 {
            format!("a=T,U=1,f=100,t=d,i={id},c={cols},r={rows},q=2,m={more}")
        } else {
            format!("q=2,m={more}")
        };
        out.push_str(&command(support, &keys, chunk));
    }
    out
}

/// Places image `id` again, over `cols` by `rows` cells: the one virtual placement replaces
/// the one before (`p=1`).
pub fn place(support: Support, id: u32, cols: u16, rows: u16) -> String {
    command(
        support,
        &format!("a=p,U=1,i={id},p=1,c={cols},r={rows},q=2"),
        "",
    )
}

/// Deletes image `id` and frees its data in the terminal.
pub fn delete(support: Support, id: u32) -> String {
    command(support, &format!("a=d,d=I,i={id},q=2"), "")
}

/// Row `row` of image `id`, `cols` cells wide, as spans of placeholder cells: the first
/// names its row and column, the others follow from it, and the id is the foreground color.
pub fn row(id: u32, row: u16, cols: u16) -> Vec<Span<'static>> {
    let mut text = String::new();
    let mark = |n: u16| {
        char::from_u32(DIACRITICS[usize::from(n.min(MAX_CELLS - 1))]).unwrap_or(PLACEHOLDER)
    };
    for col in 0..cols {
        text.push(PLACEHOLDER);
        text.push(mark(row));
        text.push(mark(col));
    }
    let [_, r, g, b] = id.to_be_bytes();
    vec![Span::styled(text, Style::new().fg(Color::Rgb(r, g, b)))]
}

/// The cells an image of `pixels` takes, at most `room` (columns, rows), with cells of
/// `cell` pixels: its own size when that fits, else as large as fits, keeping its shape.
pub fn fit(pixels: (u32, u32), cell: (u16, u16), room: (u16, u16)) -> (u16, u16) {
    let (width, height) = (u64::from(pixels.0.max(1)), u64::from(pixels.1.max(1)));
    let (cell_w, cell_h) = (u64::from(cell.0.max(1)), u64::from(cell.1.max(1)));
    let room_w = u64::from(room.0.clamp(1, MAX_CELLS));
    let room_h = u64::from(room.1.clamp(1, MAX_CELLS));
    // Its own size in cells, then shrunk to fit either way.
    let mut cols = width.div_ceil(cell_w).min(room_w);
    let mut rows = (cols * cell_w * height).div_ceil(width * cell_h).max(1);
    if rows > room_h {
        rows = room_h;
        cols = (rows * cell_h * width / (height * cell_w)).max(1);
    }
    (
        u16::try_from(cols.max(1)).unwrap_or(1),
        u16::try_from(rows.max(1)).unwrap_or(1),
    )
}

/// The id of the `n`th image of this process: high bits from the process id, so two
/// companions in panes of one terminal do not take each other's images. Never zero, and
/// within 24 bits, as a foreground color carries it.
pub fn image_id(process: u32, n: u32) -> u32 {
    ((process % 0xFFF + 1) << 12) | (n % 0xFFF + 1)
}

/// The size of `bytes` as people read it: `512 B`, `12 KB`, `3.4 MB`.
pub fn size_label(bytes: u64) -> String {
    let mut out = String::new();
    match bytes {
        0..1024 => {
            let _ = write!(out, "{bytes} B");
        }
        1024..1_048_576 => {
            let _ = write!(out, "{} KB", bytes.div_ceil(1024));
        }
        _ => {
            let tenths = (bytes * 10 + 524_288) / 1_048_576;
            let _ = write!(out, "{}.{} MB", tenths / 10, tenths % 10);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Support = Support { tmux: false };
    const TMUX: Support = Support { tmux: true };

    #[test]
    fn kitty_and_ghostty_are_known_also_inside_tmux_and_the_setting_wins() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            detect(Wanted::Auto, env(&[("TERM", "xterm-kitty")])),
            Some(PLAIN)
        );
        assert_eq!(
            detect(
                Wanted::Auto,
                env(&[
                    ("TERM", "tmux-256color"),
                    ("TMUX", "/tmp/t,1,0"),
                    ("GHOSTTY_RESOURCES_DIR", "/x")
                ])
            ),
            Some(TMUX)
        );
        assert_eq!(
            detect(Wanted::Auto, env(&[("TERM", "xterm-256color")])),
            None
        );
        assert_eq!(detect(Wanted::Kitty, env(&[])), Some(PLAIN));
        assert_eq!(detect(Wanted::Off, env(&[("TERM", "xterm-kitty")])), None);
    }

    #[test]
    fn a_transmission_is_chunked_and_wrapped_for_tmux() {
        let png = vec![7u8; 5000];
        let sent = transmit(PLAIN, 42, &png, 10, 5);
        assert!(sent.starts_with("\x1b_Ga=T,U=1,f=100,t=d,i=42,c=10,r=5,q=2,m=1;"));
        // 5000 bytes are 6668 base64 characters: two chunks, the last with m=0.
        assert_eq!(sent.matches("\x1b_G").count(), 2);
        assert!(sent.contains("\x1b_Gq=2,m=0;"));
        let wrapped = transmit(TMUX, 42, &[1, 2, 3], 1, 1);
        assert_eq!(
            wrapped,
            "\x1bPtmux;\x1b\x1b_Ga=T,U=1,f=100,t=d,i=42,c=1,r=1,q=2,m=0;AQID\x1b\x1b\\\x1b\\"
        );
        assert_eq!(
            place(PLAIN, 42, 3, 2),
            "\x1b_Ga=p,U=1,i=42,p=1,c=3,r=2,q=2;\x1b\\"
        );
        assert_eq!(delete(PLAIN, 42), "\x1b_Ga=d,d=I,i=42,q=2;\x1b\\");
    }

    #[test]
    fn a_row_of_cells_names_its_row_and_columns_and_carries_the_id_in_its_color() {
        let spans = row(0x01_02_03, 2, 2);
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(1, 2, 3)));
        let chars: Vec<char> = spans[0].content.chars().collect();
        assert_eq!(
            chars,
            [
                PLACEHOLDER,
                '\u{030E}',
                '\u{0305}',
                PLACEHOLDER,
                '\u{030E}',
                '\u{030D}'
            ]
        );
        // Each cell is one column: the diacritics take none.
        assert_eq!(row(1, 0, 5)[0].width(), 5);
        assert_eq!(row(1, 200, 296)[0].width(), 296);
    }

    #[test]
    fn an_image_keeps_its_shape_and_never_grows_past_its_size() {
        // 8x16 cells: a 80x32 image is 10 columns by 2 rows, as it is.
        assert_eq!(fit((80, 32), (8, 16), (100, 50)), (10, 2));
        // A wide one is cut to the columns there are, rows follow.
        assert_eq!(fit((1600, 800), (8, 16), (100, 50)), (100, 25));
        // A tall one is cut to the rows there are, columns follow.
        assert_eq!(fit((800, 1600), (8, 16), (100, 20)), (20, 20));
        assert_eq!(fit((0, 0), (0, 0), (0, 0)), (1, 1));
    }

    #[test]
    fn ids_are_never_zero_and_fit_a_color() {
        for (process, n) in [(0, 0), (u32::MAX, u32::MAX), (4095, 4095), (1, 7)] {
            let id = image_id(process, n);
            assert!(id > 0 && id < 1 << 24, "{id}");
        }
        assert_ne!(image_id(1, 0), image_id(2, 0));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(12 * 1024), "12 KB");
        assert_eq!(size_label(3_565_158), "3.4 MB");
    }
}
