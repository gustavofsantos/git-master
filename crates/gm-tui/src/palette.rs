//! The terminal's own colors, as it reports them, and the soft backgrounds blended from them
//! (D250): the cursor line, the selection, and added and removed lines. Pure: the query that
//! fills it lives in `terminal.rs`.

use std::sync::OnceLock;

use ratatui::style::Color;

/// A color as 8-bit red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `per_mille` thousandths of the way from `self` to `other`, capped at all the way.
    pub fn mix(self, other: Self, per_mille: u32) -> Self {
        let t = per_mille.min(1000);
        let channel = |a: u8, b: u8| {
            let value = (u32::from(a) * (1000 - t) + u32::from(b) * t + 500) / 1000;
            u8::try_from(value).unwrap_or(u8::MAX)
        };
        Self(
            channel(self.0, other.0),
            channel(self.1, other.1),
            channel(self.2, other.2),
        )
    }

    pub fn color(self) -> Color {
        Color::Rgb(self.0, self.1, self.2)
    }
}

/// What the terminal answered: its foreground, background and first 16 colors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub ansi: [Option<Rgb>; 16],
}

const RED: usize = 1;
const GREEN: usize = 2;
const BLUE: usize = 4;

impl Palette {
    /// Reads the answers to OSC 10, 11 and 4 in `reply`; anything else in it is skipped.
    pub fn parse(reply: &[u8]) -> Self {
        let mut palette = Self::default();
        let text = String::from_utf8_lossy(reply);
        for answer in text.split("\x1b]").skip(1) {
            let body = answer.split(['\x07', '\x1b']).next().unwrap_or_default();
            let mut parts = body.split(';');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("10"), Some(value), None) => palette.foreground = parse_rgb(value),
                (Some("11"), Some(value), None) => palette.background = parse_rgb(value),
                (Some("4"), Some(index), Some(value)) => {
                    if let Some(slot) = index
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| palette.ansi.get_mut(i))
                    {
                        *slot = parse_rgb(value);
                    }
                }
                _ => {}
            }
        }
        palette
    }
}

/// `rgb:R/G/B` with one to four hex digits per channel, scaled to 8 bits.
fn parse_rgb(value: &str) -> Option<Rgb> {
    let mut channels = value.strip_prefix("rgb:")?.split('/').map(|hex| {
        let digits = u32::try_from(hex.len())
            .ok()
            .filter(|n| (1..=4).contains(n))?;
        let raw = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * digits)) - 1;
        u8::try_from((raw * 255 + max / 2) / max).ok()
    });
    let rgb = Rgb(channels.next()??, channels.next()??, channels.next()??);
    channels.next().is_none().then_some(rgb)
}

/// The backgrounds blended from the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tints {
    pub background: Rgb,
    pub cursor: Rgb,
    pub selected: Rgb,
    pub added: Rgb,
    pub added_gutter: Rgb,
    pub added_cursor: Rgb,
    pub removed: Rgb,
    pub removed_gutter: Rgb,
    pub removed_cursor: Rgb,
    /// The mode chip's background.
    pub chip: Rgb,
}

/// How far each tint leans from the background, in thousandths.
const CURSOR: u32 = 120;
const SELECTED: u32 = 180;
const CHANGE: u32 = 140;

impl Tints {
    /// The tints, when the terminal reported every color they need.
    pub fn from_palette(palette: &Palette) -> Option<Self> {
        let background = palette.background?;
        let foreground = palette.foreground?;
        let red = palette.ansi[RED]?;
        let green = palette.ansi[GREEN]?;
        let blue = palette.ansi[BLUE]?;
        let added = background.mix(green, CHANGE);
        let removed = background.mix(red, CHANGE);
        Some(Self {
            background,
            cursor: background.mix(foreground, CURSOR),
            selected: background.mix(blue, SELECTED),
            added,
            added_gutter: background.mix(green, CHANGE * 2),
            added_cursor: added.mix(foreground, CURSOR),
            removed,
            removed_gutter: background.mix(red, CHANGE * 2),
            removed_cursor: removed.mix(foreground, CURSOR),
            chip: blue,
        })
    }
}

/// Whether the terminal says it shows 24-bit color.
pub fn truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor" | "24bit"))
}

static TINTS: OnceLock<Option<Tints>> = OnceLock::new();

/// Keeps the tints for the rest of the run; only the first call counts.
pub fn set(tints: Option<Tints>) {
    let _ = TINTS.set(tints);
}

/// Whether the terminal was already asked.
pub fn asked() -> bool {
    TINTS.get().is_some()
}

/// The tints, or `None` when the terminal did not report them or cannot show them: the
/// companion then draws without backgrounds.
pub fn tints() -> Option<&'static Tints> {
    TINTS.get().and_then(Option::as_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &[u8] = b"\x1b]10;rgb:d2d2/cfcf/c5c5\x1b\\\x1b]11;rgb:1a1a/1919/1818\x07\
\x1b]4;1;rgb:ffff/8b8b/9090\x1b\\\x1b]4;2;rgb:64/c1/75\x1b\\\x1b]4;4;rgb:6/b/f\x1b\\\x1b[?1;2c";

    #[test]
    fn reads_the_answers_whatever_their_terminator_and_digits() {
        let palette = Palette::parse(REPLY);
        assert_eq!(palette.foreground, Some(Rgb(0xd2, 0xcf, 0xc5)));
        assert_eq!(palette.background, Some(Rgb(0x1a, 0x19, 0x18)));
        assert_eq!(palette.ansi[1], Some(Rgb(0xff, 0x8b, 0x90)));
        assert_eq!(palette.ansi[2], Some(Rgb(0x64, 0xc1, 0x75)));
        assert_eq!(palette.ansi[4], Some(Rgb(0x66, 0xbb, 0xff)));
        assert_eq!(palette.ansi[0], None);
    }

    #[test]
    fn skips_what_it_cannot_read() {
        let palette = Palette::parse(
            b"\x1b]11;rgb:zz/00/00\x1b\\\x1b]4;99;rgb:1/1/1\x1b\\\x1b]10;#ffffff\x07",
        );
        assert_eq!(palette, Palette::default());
        assert_eq!(parse_rgb("rgb:12345/0/0"), None);
        assert_eq!(parse_rgb("rgb:1/2"), None);
        assert_eq!(parse_rgb("rgb:1/2/3/4"), None);
    }

    #[test]
    fn mixes_between_two_colors() {
        let black = Rgb(0, 0, 0);
        let white = Rgb(255, 255, 255);
        assert_eq!(black.mix(white, 0), black);
        assert_eq!(black.mix(white, 1000), white);
        assert_eq!(black.mix(white, 500), Rgb(128, 128, 128));
        assert_eq!(white.mix(black, 2000), black);
    }

    #[test]
    fn tints_lean_from_the_background_toward_their_color() {
        let tints = Tints::from_palette(&Palette::parse(REPLY)).expect("every color answered");
        assert_eq!(tints.background, Rgb(0x1a, 0x19, 0x18));
        // Green leads in the added tint, red in the removed one, and the gutter is stronger.
        assert!(tints.added.1 > tints.added.0 && tints.added_gutter.1 > tints.added.1);
        assert!(tints.removed.0 > tints.removed.1 && tints.removed_gutter.0 > tints.removed.0);
        // The cursor is lighter than the background on a dark theme.
        assert!(tints.cursor.0 > tints.background.0);
        assert_eq!(tints.chip, Rgb(0x66, 0xbb, 0xff));
    }

    #[test]
    fn no_tints_without_every_color_they_need() {
        let mut palette = Palette::parse(REPLY);
        palette.ansi[2] = None;
        assert_eq!(Tints::from_palette(&palette), None);
        assert_eq!(Tints::from_palette(&Palette::default()), None);
    }

    #[test]
    fn only_24_bit_terminals_get_tints() {
        assert!(truecolor(Some("truecolor")));
        assert!(truecolor(Some("24bit")));
        assert!(!truecolor(Some("256color")));
        assert!(!truecolor(None));
    }
}
