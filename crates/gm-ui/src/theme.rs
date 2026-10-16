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

/// The light palette's values under their old names; other modules still
/// refer to them, so they are the light palette and `Palette::LIGHT` uses them.
pub const BACKGROUND: u32 = 0xfa_faf8;
pub const PANEL: u32 = 0xf5_f5f3;
pub const TEXT: u32 = 0x1b_1c1e;
pub const MUTED: u32 = 0x5e_6065;
pub const SELECTED: u32 = 0xe0_e7fb;
pub const ERROR: u32 = 0xa3_3a32;
pub const STAGED: u32 = 0x1e_6b3a;
pub const UNSTAGED: u32 = 0x7a_4e0b;
pub const UNTRACKED: u32 = 0x2f_5bea;

/// One full set of colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub ground: u32,
    pub panel: u32,
    pub hairline: u32,
    pub hairline_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub selected_row: u32,
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
    ground: BACKGROUND,
    panel: PANEL,
    hairline: 0xe6_e6e2,
    hairline_strong: 0xde_deda,
    text: TEXT,
    muted: MUTED,
    faint: 0x8a_8c91,
    accent: UNTRACKED,
    selected_row: SELECTED,
    added_fg: STAGED,
    added_bg: 0xe3_f3e8,
    removed_fg: ERROR,
    removed_bg: 0xfb_e8e6,
    warn_fg: UNSTAGED,
    warn_bg: 0xfb_f0d9,
    staged: STAGED,
    unstaged: UNSTAGED,
    untracked: UNTRACKED,
    error: ERROR,
};

pub const DARK: Palette = Palette {
    ground: 0x1b_1d21,
    panel: 0x23_262b,
    hairline: 0x2e_3238,
    hairline_strong: 0x3a_3f47,
    text: 0xd7_dae0,
    muted: 0x8a_919c,
    faint: 0x6b_7280,
    accent: 0x6c_8cff,
    selected_row: 0x2f_3d56,
    added_fg: 0x98_c379,
    added_bg: 0x1f_3326,
    removed_fg: 0xe0_6c75,
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
    accent,
    selected_row,
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

    #[test]
    fn both_palettes_keep_text_and_muted_readable() {
        for palette in [LIGHT, DARK] {
            assert!(contrast(palette.text, palette.ground) >= 4.5);
            assert!(contrast(palette.muted, palette.ground) >= 3.0);
        }
    }

    #[test]
    fn the_legacy_names_are_the_light_palette() {
        assert_eq!(LIGHT.ground, BACKGROUND);
        assert_eq!(LIGHT.text, TEXT);
        assert_eq!(LIGHT.error, ERROR);
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
