//! Colors and fonts. Dark only for now; fonts are chosen once at startup from
//! what the system has installed.

use std::sync::OnceLock;

use gpui::{Hsla, SharedString, rgb};

const UI_PREFERENCES: &[&str] = &["Inter", "SF Pro Text", "Noto Sans", "DejaVu Sans"];
const UI_FALLBACK: &str = "Noto Sans";
const CODE_PREFERENCES: &[&str] = &["JetBrains Mono", "SF Mono", "Menlo", "DejaVu Sans Mono"];
const CODE_FALLBACK: &str = "Noto Sans Mono";

static UI_FAMILY: OnceLock<String> = OnceLock::new();
static CODE_FAMILY: OnceLock<String> = OnceLock::new();

pub const BACKGROUND: u32 = 0x1b_1d21;
pub const PANEL: u32 = 0x23_262b;
pub const TEXT: u32 = 0xd7_dae0;
pub const MUTED: u32 = 0x8a_919c;
pub const SELECTED: u32 = 0x2f_3d56;
pub const ERROR: u32 = 0xe0_6c75;
pub const STAGED: u32 = 0x98_c379;
pub const UNSTAGED: u32 = 0xe5_c07b;
pub const UNTRACKED: u32 = 0x61_afef;

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
