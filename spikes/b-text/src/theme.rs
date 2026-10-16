//! Spike-local stand-in for rvw's `theme` module: only the tokens the text
//! area reads, one light palette, one font. Not a product theme.

use gpui::{Rgba, SharedString, rgb, rgba};

pub const BASE_FONT_SIZE: f32 = 13.5;

/// rvw picks the first installed family from a list and falls back to
/// "Noto Sans". Here the fallback is used directly. If the family is missing,
/// shaping can yield no lines; the trace in `text_area.rs` shows that case.
pub fn ui_font() -> SharedString {
    "Noto Sans".into()
}

pub fn paper() -> Rgba {
    rgb(0x00fa_faf7)
}
pub fn ink() -> Rgba {
    rgb(0x0016_1614)
}
pub fn faint() -> Rgba {
    rgb(0x008a_8882)
}
pub fn needs_you() -> Rgba {
    rgb(0x00f5_4e00)
}
pub fn text_selection() -> Rgba {
    rgba(0x006c_bc40)
}
