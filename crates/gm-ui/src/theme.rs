//! Colors and fonts. Two palettes, light and dark; the window follows the
//! system appearance unless the mode is set. Views call the accessors at render
//! time and never keep a color.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use gpui::{Hsla, SharedString, WindowAppearance, rgb};

const UI_PREFERENCES: &[&str] = &[
    "IBM Plex Sans",
    "Inter",
    "SF Pro Text",
    "Noto Sans",
    "DejaVu Sans",
];
const UI_FALLBACK: &str = "Noto Sans";
const CODE_PREFERENCES: &[&str] = &["JetBrains Mono", "SF Mono", "Menlo", "DejaVu Sans Mono"];
const CODE_FALLBACK: &str = "Noto Sans Mono";

static UI_FAMILY: OnceLock<String> = OnceLock::new();
static CODE_FAMILY: OnceLock<String> = OnceLock::new();

/// One full set of colors. Every view reads a role through an accessor below,
/// so a new role is added here for both palettes, never as a literal in a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The window background.
    pub ground: u32,
    /// Bars and headers that sit above the ground.
    pub panel: u32,
    pub hairline: u32,
    pub hairline_strong: u32,
    /// Primary text.
    pub text: u32,
    /// Secondary text: labels, counts, hints.
    pub muted: u32,
    /// Decorative text, such as the repository root in the status bar.
    pub faint: u32,
    /// Placeholder text in an empty field.
    pub placeholder: u32,
    /// The text caret.
    pub caret: u32,
    /// Background of a selected text range.
    pub selection: u32,
    /// Line numbers in the diff gutter.
    pub gutter: u32,
    /// Text drawn on a solid accent background.
    pub on_accent: u32,
    pub accent: u32,
    /// Outline of a button drawn on the diff pane.
    pub button_border: u32,
    /// Background of a selected file row.
    pub selected_row: u32,
    /// Background of the row under the diff cursor.
    pub cursor_row: u32,
    /// Background of a hunk header row in the diff pane.
    pub hunk_header_bg: u32,
    pub added_fg: u32,
    pub added_bg: u32,
    pub removed_fg: u32,
    pub removed_bg: u32,
    pub warn_fg: u32,
    pub warn_bg: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub error: u32,
}

pub const LIGHT: Palette = Palette {
    ground: 0xfa_faf8,
    panel: 0xf5_f5f3,
    hairline: 0xe6_e6e2,
    hairline_strong: 0xde_deda,
    text: 0x1b_1c1e,
    muted: 0x5e_6065,
    faint: 0x68_6b71,
    placeholder: 0x5e_6065,
    caret: 0x1b_1c1e,
    selection: 0xe0_e7fb,
    gutter: 0x5e_6065,
    on_accent: 0xf5_f5f3,
    accent: 0x2f_5bea,
    button_border: 0x2f_5bea,
    selected_row: 0xe0_e7fb,
    cursor_row: 0xe0_e7fb,
    hunk_header_bg: 0xf5_f5f3,
    added_fg: 0x1e_6b3a,
    added_bg: 0xe3_f3e8,
    removed_fg: 0xa3_3a32,
    removed_bg: 0xfb_e8e6,
    warn_fg: 0x7a_4e0b,
    warn_bg: 0xfb_f0d9,
    staged: 0x1e_6b3a,
    unstaged: 0x7a_4e0b,
    untracked: 0x2f_5bea,
    error: 0xa3_3a32,
};

pub const DARK: Palette = Palette {
    ground: 0x1b_1d21,
    panel: 0x23_262b,
    hairline: 0x2e_3238,
    hairline_strong: 0x3a_3f47,
    text: 0xd7_dae0,
    muted: 0xa3_abb6,
    faint: 0x8a_919c,
    placeholder: 0xa3_abb6,
    caret: 0xd7_dae0,
    selection: 0x3b_5b8c,
    gutter: 0xa3_abb6,
    on_accent: 0x1b_1d21,
    accent: 0x6c_8cff,
    button_border: 0x6c_8cff,
    selected_row: 0x2f_3d56,
    cursor_row: 0x28_3347,
    hunk_header_bg: 0x23_262b,
    added_fg: 0x98_c379,
    added_bg: 0x1f_3326,
    removed_fg: 0xe8_8a92,
    removed_bg: 0x3a_2326,
    warn_fg: 0xe5_c07b,
    warn_bg: 0x3a_311c,
    staged: 0x98_c379,
    unstaged: 0xe5_c07b,
    untracked: 0x61_afef,
    error: 0xe0_6c75,
};

/// The setting the user chose. `System` follows the OS appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// Reads a setting name: `system`, `light` or `dark`, in any case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    const fn code(self) -> u8 {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Light,
            2 => Self::Dark,
            _ => Self::System,
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(0);
static SYSTEM_DARK: AtomicBool = AtomicBool::new(false);

/// Sets the mode. Views must re-render to show it.
pub fn set_mode(mode: ThemeMode) {
    MODE.store(mode.code(), Ordering::Relaxed);
}

#[must_use]
pub fn mode() -> ThemeMode {
    ThemeMode::from_code(MODE.load(Ordering::Relaxed))
}

/// Records the OS appearance. Returns whether the dark/light choice changed,
/// so the caller re-renders only then.
pub fn follow(appearance: WindowAppearance) -> bool {
    let dark = matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    SYSTEM_DARK.swap(dark, Ordering::Relaxed) != dark
}

/// Whether the palette in use is the dark one.
#[must_use]
pub fn is_dark() -> bool {
    match mode() {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => SYSTEM_DARK.load(Ordering::Relaxed),
    }
}

/// The palette in use.
#[must_use]
pub fn palette() -> &'static Palette {
    if is_dark() { &DARK } else { &LIGHT }
}

macro_rules! accessors {
    ($($name:ident),* $(,)?) => {
        $(
            #[must_use]
            pub fn $name() -> Hsla {
                color(palette().$name)
            }
        )*
    };
}

accessors!(
    ground,
    panel,
    hairline,
    hairline_strong,
    text,
    muted,
    faint,
    placeholder,
    caret,
    selection,
    gutter,
    on_accent,
    accent,
    button_border,
    selected_row,
    cursor_row,
    hunk_header_bg,
    added_fg,
    added_bg,
    removed_fg,
    removed_bg,
    warn_fg,
    warn_bg,
    staged,
    unstaged,
    untracked,
    error,
);

#[must_use]
pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// The first preference that is installed, or `fallback`. Names match without
/// regard to case.
#[must_use]
pub fn pick<'a>(installed: &[String], preferences: &[&'a str], fallback: &'a str) -> &'a str {
    preferences
        .iter()
        .copied()
        .find(|wanted| {
            installed
                .iter()
                .any(|name| name.eq_ignore_ascii_case(wanted))
        })
        .unwrap_or(fallback)
}

/// Chooses the UI and code families. A second call keeps the first choice.
pub fn choose_fonts(installed: &[String]) {
    let _ = UI_FAMILY.set(pick(installed, UI_PREFERENCES, UI_FALLBACK).to_owned());
    let _ = CODE_FAMILY.set(pick(installed, CODE_PREFERENCES, CODE_FALLBACK).to_owned());
}

pub fn ui_font() -> SharedString {
    UI_FAMILY
        .get()
        .map_or(UI_FALLBACK, String::as_str)
        .to_owned()
        .into()
}

pub fn code_font() -> SharedString {
    CODE_FAMILY
        .get()
        .map_or(CODE_FALLBACK, String::as_str)
        .to_owned()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// WCAG 2.x relative luminance of an 0xRRGGBB color.
    fn luminance(hex: u32) -> f64 {
        let channel = |shift: u32| {
            let value = f64::from((hex >> shift) & 0xff) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (hi, lo) = {
            let (x, y) = (luminance(a), luminance(b));
            if x > y { (x, y) } else { (y, x) }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The text-on-background pairs the views draw, as (name, text, background).
    fn text_pairs(p: &Palette) -> Vec<(&'static str, u32, u32)> {
        vec![
            ("text on ground", p.text, p.ground),
            ("text on panel", p.text, p.panel),
            ("text on selection", p.text, p.selection),
            ("text on selected row", p.text, p.selected_row),
            ("text on cursor row", p.text, p.cursor_row),
            ("text on added line", p.text, p.added_bg),
            ("text on removed line", p.text, p.removed_bg),
            ("muted on ground", p.muted, p.ground),
            ("muted on panel", p.muted, p.panel),
            ("muted on hunk header", p.muted, p.hunk_header_bg),
            ("muted on selected row", p.muted, p.selected_row),
            ("muted on cursor row", p.muted, p.cursor_row),
            ("muted on added line", p.muted, p.added_bg),
            ("muted on removed line", p.muted, p.removed_bg),
            ("muted on warning", p.muted, p.warn_bg),
            ("placeholder on ground", p.placeholder, p.ground),
            ("gutter on ground", p.gutter, p.ground),
            ("gutter on cursor row", p.gutter, p.cursor_row),
            ("gutter on added line", p.gutter, p.added_bg),
            ("gutter on removed line", p.gutter, p.removed_bg),
            ("faint on panel", p.faint, p.panel),
            ("added text on added line", p.added_fg, p.added_bg),
            ("added text on cursor row", p.added_fg, p.cursor_row),
            ("removed text on removed line", p.removed_fg, p.removed_bg),
            ("removed text on cursor row", p.removed_fg, p.cursor_row),
            ("removed text on panel", p.removed_fg, p.panel),
            ("error on panel", p.error, p.panel),
            ("warning text on warning", p.warn_fg, p.warn_bg),
            ("warning text on ground", p.warn_fg, p.ground),
            ("text on accent", p.on_accent, p.accent),
        ]
    }

    #[test]
    fn both_palettes_keep_every_text_pair_at_least_4_5_to_1() {
        for (name, palette) in [("light", LIGHT), ("dark", DARK)] {
            for (pair, fg, bg) in text_pairs(&palette) {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{name}: {pair} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn non_text_marks_keep_3_to_1() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            for (mark, bg) in [
                (p.button_border, p.panel),
                (p.button_border, p.cursor_row),
                (p.accent, p.panel),
                (p.caret, p.ground),
            ] {
                let ratio = contrast(mark, bg);
                assert!(
                    ratio >= 3.0,
                    "{name}: {mark:06x} on {bg:06x} is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn the_dark_diff_ground_is_dark_and_tints_are_the_dark_backgrounds() {
        assert!(luminance(DARK.ground) < 0.05);
        assert!(luminance(DARK.panel) < 0.05);
        assert!(luminance(DARK.added_bg) < 0.05);
        assert!(luminance(DARK.removed_bg) < 0.05);
        assert_ne!(DARK.added_bg, LIGHT.added_bg);
        assert_ne!(DARK.removed_bg, LIGHT.removed_bg);
    }

    #[test]
    fn mode_names_parse_in_any_case() {
        assert_eq!(ThemeMode::from_name("Dark"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_name("light"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_name("system"), Some(ThemeMode::System));
        assert_eq!(ThemeMode::from_name("blue"), None);
    }

    #[test]
    fn mode_codes_round_trip() {
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(ThemeMode::from_code(mode.code()), mode);
        }
    }

    #[test]
    fn the_first_installed_preference_wins() {
        let installed = names(&["DejaVu Sans", "Inter"]);
        assert_eq!(pick(&installed, UI_PREFERENCES, UI_FALLBACK), "Inter");
    }

    #[test]
    fn names_match_without_regard_to_case() {
        let installed = names(&["jetbrains mono"]);
        assert_eq!(
            pick(&installed, CODE_PREFERENCES, CODE_FALLBACK),
            "JetBrains Mono"
        );
    }

    #[test]
    fn nothing_installed_gives_the_fallback() {
        assert_eq!(pick(&[], UI_PREFERENCES, UI_FALLBACK), UI_FALLBACK);
    }
}
