//! The colors of gpui-component, the editor component of D147. The component reads a global
//! `Theme`; this module fills it from the `GitMaster` palette for the mode in use, so the editor
//! follows the light and dark themes like every other view. Only the component's own widgets
//! read it: the views of `GitMaster` keep their palette accessors.
//!
//! The bridge is pure: `colors` and `highlight` take a palette and the mode and build values,
//! so they are tested without a window. `apply` is the one function that touches the global.

use std::sync::Arc;

use gm_core::syntax::role_for_capture;
use gm_schema::SyntaxRole;
use gpui::App;
use gpui_component::highlighter::{
    HighlightTheme, HighlightThemeStyle, StatusColors, SyntaxColors, ThemeStyle,
};
use gpui_component::theme::{Theme, ThemeColor, ThemeMode};

use crate::theme::{self, Palette};

/// Installs the component and fills its global `Theme` for the mode in use. Call it at startup,
/// and again whenever the mode or the OS appearance changes. Later calls only refill the theme.
pub fn apply(cx: &mut App) {
    // `init` creates the global `Theme`, so its presence says the component is set up already.
    if !cx.has_global::<Theme>() {
        gpui_component::init(cx);
    }
    let dark = theme::is_dark();
    let palette = theme::palette();
    let global = Theme::global_mut(cx);
    global.mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    global.colors = colors(palette, dark);
    global.highlight_theme = Arc::new(highlight(palette, dark));
}

/// The component's colors for `palette`. Fields that `GitMaster` has no equivalent for keep the
/// component's default for the mode. The names follow the component, not `GitMaster`: its
/// `accent` is the hover and row background, its `primary` is the brand color, its `muted` is a
/// surface and its `muted_foreground` is secondary text.
#[must_use]
pub fn colors(palette: &Palette, dark: bool) -> ThemeColor {
    let base = if dark {
        ThemeColor::dark()
    } else {
        ThemeColor::light()
    };
    let c = theme::color;
    ThemeColor {
        background: c(palette.editor),
        foreground: c(palette.text),
        border: c(palette.hairline),
        input: c(palette.hairline_strong),
        ring: c(palette.accent),
        caret: c(palette.caret),
        selection: c(palette.selection),
        muted: c(palette.panel),
        muted_foreground: c(palette.muted),
        popover: c(palette.panel),
        popover_foreground: c(palette.text),
        list: c(palette.panel),
        list_active: c(palette.selected_row),
        list_active_border: c(palette.accent),
        list_hover: c(palette.selected_row),
        accent: c(palette.selected_row),
        accent_foreground: c(palette.text),
        primary: c(palette.accent),
        primary_foreground: c(palette.on_accent),
        danger: c(palette.error),
        danger_foreground: c(palette.on_accent),
        warning: c(palette.warn_fg),
        warning_foreground: c(palette.on_accent),
        // The diagnostics of the component fall back to these: red for errors, yellow for
        // warnings, blue for information, cyan for hints.
        red: c(palette.error),
        yellow: c(palette.warn_fg),
        blue: c(palette.accent),
        green: c(palette.added_fg),
        cyan: c(palette.muted),
        ..*base
    }
}

/// The component's highlight theme for `palette`: the editor surfaces and the colors of the
/// highlight names, each name painted with the role D111 gives it.
#[must_use]
pub fn highlight(palette: &Palette, dark: bool) -> HighlightTheme {
    let c = theme::color;
    HighlightTheme {
        name: if dark {
            "GitMaster dark"
        } else {
            "GitMaster light"
        }
        .to_owned(),
        appearance: if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        style: HighlightThemeStyle {
            editor_background: Some(c(palette.editor)),
            editor_foreground: Some(c(palette.text)),
            editor_active_line: Some(c(palette.cursor_row)),
            editor_line_number: Some(c(palette.gutter)),
            editor_active_line_number: Some(c(palette.text)),
            status: StatusColors::default(),
            syntax: syntax(palette),
        },
    }
}

/// The role of a highlight name of the component. D111's table (`role_for_capture`) answers for
/// our queries' names. The component's `variable.special` is the meaning of `variable.builtin`
/// (`self`, `this`), so it is looked up under that name. A name the table does not know paints
/// nothing, as a bare `@variable` does in the viewer.
fn role_of(name: &str) -> Option<SyntaxRole> {
    role_for_capture(if name == "variable.special" {
        "variable.builtin"
    } else {
        name
    })
}

/// The style of a highlight name with `palette`'s color for its role, or `None` for a plain name.
fn style_of(name: &str, palette: &Palette) -> Option<ThemeStyle> {
    role_of(name).and_then(|role| style(palette.syntax[role as usize]))
}

/// A style that sets only the color. `ThemeStyle` has no public constructor, so it is read from
/// the hex form the component itself accepts. `None` only if that read fails, and the test
/// `every_role_resolves_to_the_viewer_color` catches it.
fn style(hex: u32) -> Option<ThemeStyle> {
    serde_json::from_value(serde_json::json!({ "color": format!("#{hex:06x}") })).ok()
}

/// Every field of the component's syntax colors, named after the highlight name it answers to.
fn syntax(palette: &Palette) -> SyntaxColors {
    let paint = |name: &str| style_of(name, palette);
    SyntaxColors {
        attribute: paint("attribute"),
        boolean: paint("boolean"),
        comment: paint("comment"),
        comment_doc: paint("comment.doc"),
        constant: paint("constant"),
        constructor: paint("constructor"),
        embedded: paint("embedded"),
        emphasis: paint("emphasis"),
        emphasis_strong: paint("emphasis.strong"),
        enum_: paint("enum"),
        function: paint("function"),
        hint: paint("hint"),
        keyword: paint("keyword"),
        label: paint("label"),
        link_text: paint("link_text"),
        link_uri: paint("link_uri"),
        number: paint("number"),
        operator: paint("operator"),
        predictive: paint("predictive"),
        preproc: paint("preproc"),
        primary: paint("primary"),
        property: paint("property"),
        punctuation: paint("punctuation"),
        punctuation_bracket: paint("punctuation.bracket"),
        punctuation_delimiter: paint("punctuation.delimiter"),
        punctuation_list_marker: paint("punctuation.list_marker"),
        punctuation_special: paint("punctuation.special"),
        string: paint("string"),
        string_escape: paint("string.escape"),
        string_regex: paint("string.regex"),
        string_special: paint("string.special"),
        string_special_symbol: paint("string.special.symbol"),
        tag: paint("tag"),
        tag_doctype: paint("tag.doctype"),
        text_literal: paint("text.literal"),
        title: paint("title"),
        type_: paint("type"),
        variable: paint("variable"),
        variable_special: paint("variable.special"),
        variant: paint("variant"),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{HighlightStyle, Hsla};

    use super::*;
    use crate::theme::{DARK, LIGHT};

    /// The highlight names of gpui-component 0.5.1, copied from `HIGHLIGHT_NAMES` in its
    /// `src/highlighter/registry.rs`. That array is `pub(super)`, so it cannot be read from here.
    /// The crate is pinned to `=0.5.1` (D147); a bump must update this list.
    const CRATE_NAMES: [&str; 40] = [
        "attribute",
        "boolean",
        "comment",
        "comment.doc",
        "constant",
        "constructor",
        "embedded",
        "emphasis",
        "emphasis.strong",
        "enum",
        "function",
        "hint",
        "keyword",
        "label",
        "link_text",
        "link_uri",
        "number",
        "operator",
        "predictive",
        "preproc",
        "primary",
        "property",
        "punctuation",
        "punctuation.bracket",
        "punctuation.delimiter",
        "punctuation.list_marker",
        "punctuation.special",
        "string",
        "string.escape",
        "string.regex",
        "string.special",
        "string.special.symbol",
        "tag",
        "tag.doctype",
        "text.literal",
        "title",
        "type",
        "variable",
        "variable.special",
        "variant",
    ];

    /// The names that paint no color, so they stay plain text. Each is a decision: a name not
    /// in D111's table, or a bare variable, which the viewer also leaves plain.
    const PLAIN: [&str; 14] = [
        "embedded",
        "emphasis",
        "emphasis.strong",
        "enum",
        "hint",
        "link_text",
        "link_uri",
        "predictive",
        "preproc",
        "primary",
        "text.literal",
        "title",
        "variable",
        "variant",
    ];

    /// The name that paints each of the twelve roles.
    const ROLE_NAMES: [(SyntaxRole, &str); 12] = [
        (SyntaxRole::Comment, "comment"),
        (SyntaxRole::String, "string"),
        (SyntaxRole::Number, "number"),
        (SyntaxRole::Keyword, "keyword"),
        (SyntaxRole::Operator, "operator"),
        (SyntaxRole::Function, "function"),
        (SyntaxRole::Type, "type"),
        (SyntaxRole::Constant, "constant"),
        (SyntaxRole::Variable, "variable.special"),
        (SyntaxRole::Property, "property"),
        (SyntaxRole::Punctuation, "punctuation"),
        (SyntaxRole::Attribute, "attribute"),
    ];

    /// Both modes, with the palette and the mode flag each one is built from.
    fn modes() -> [(&'static Palette, bool); 2] {
        [(&LIGHT, false), (&DARK, true)]
    }

    fn color_of(theme: &HighlightTheme, name: &str) -> Option<Hsla> {
        theme.style.syntax.style(name).and_then(|s| s.color)
    }

    #[test]
    fn every_role_resolves_to_the_viewer_color_in_both_modes() {
        for (palette, dark) in modes() {
            let theme = highlight(palette, dark);
            for (role, name) in ROLE_NAMES {
                assert_eq!(role_of(name), Some(role), "{name} paints {role:?}");
                // `theme::syntax` is `theme::color` of the palette's entry for the role.
                assert_eq!(
                    color_of(&theme, name),
                    Some(theme::color(palette.syntax[role as usize])),
                    "{name} in {}",
                    if dark { "dark" } else { "light" },
                );
            }
        }
    }

    #[test]
    fn every_name_the_component_defines_is_mapped_or_plain() {
        for (palette, dark) in modes() {
            let theme = highlight(palette, dark);
            for name in CRATE_NAMES {
                let role = role_of(name);
                assert_eq!(
                    role.is_none(),
                    PLAIN.contains(&name),
                    "{name} must be mapped to a role, or listed as plain in this test",
                );
                // A plain name has no style, so it takes the editor's text color.
                assert_eq!(color_of(&theme, name).is_some(), role.is_some(), "{name}");
                if let Some(role) = role {
                    assert_eq!(
                        color_of(&theme, name),
                        Some(theme::color(palette.syntax[role as usize])),
                        "{name}",
                    );
                }
            }
        }
    }

    #[test]
    fn the_listed_plain_names_are_all_names_of_the_component() {
        for name in PLAIN {
            assert!(
                CRATE_NAMES.contains(&name),
                "{name} is not a component name"
            );
        }
    }

    #[test]
    fn the_editor_surfaces_are_the_palette_editor_tokens() {
        for (palette, dark) in modes() {
            let highlight = highlight(palette, dark);
            assert_eq!(
                highlight.style.editor_background,
                Some(theme::color(palette.editor))
            );
            assert_eq!(
                highlight.style.editor_foreground,
                Some(theme::color(palette.text))
            );
            assert_eq!(highlight.appearance.is_dark(), dark);

            let colors = colors(palette, dark);
            assert_eq!(colors.background, theme::color(palette.editor));
            assert_eq!(colors.foreground, theme::color(palette.text));
            assert_eq!(colors.caret, theme::color(palette.caret));
            assert_eq!(colors.selection, theme::color(palette.selection));
            assert_eq!(colors.muted_foreground, theme::color(palette.muted));
        }
    }

    #[test]
    fn the_diagnostic_colors_come_from_the_error_and_warning_tokens() {
        for (palette, dark) in modes() {
            let colors = colors(palette, dark);
            assert_eq!(colors.danger, theme::color(palette.error));
            assert_eq!(colors.warning, theme::color(palette.warn_fg));
            assert_eq!(colors.red, theme::color(palette.error));
            assert_eq!(colors.yellow, theme::color(palette.warn_fg));
        }
    }

    #[test]
    fn the_style_keeps_only_the_color() {
        let style = style(0x12_3456).expect("a hex color reads as a style");
        let highlight: HighlightStyle = style.into();
        assert_eq!(highlight.color, Some(theme::color(0x12_3456)));
        assert_eq!(highlight.font_weight, None);
        assert_eq!(highlight.font_style, None);
    }
}
