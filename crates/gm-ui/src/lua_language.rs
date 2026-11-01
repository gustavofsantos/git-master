//! The Lua language of the workflow script editor (D151, D149). gpui-component ships no Lua
//! grammar, so `tree-sitter-lua` is registered by hand with the component's `LanguageRegistry`,
//! with the query in `lua_highlights.scm`. The grammar reads Lua; the Luau-only `continue` and
//! `export` are matched by their text.

use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};

/// The name the editor asks for: `InputState::code_editor("lua")`.
pub const NAME: &str = "lua";

/// The highlight query of Lua.
const HIGHLIGHTS: &str = include_str!("lua_highlights.scm");

/// The configuration of Lua for the component. It has no injections and no locals.
#[must_use]
pub fn config() -> LanguageConfig {
    LanguageConfig::new(
        NAME,
        tree_sitter_lua::LANGUAGE.into(),
        Vec::new(),
        HIGHLIGHTS,
        "",
        "",
    )
}

/// Registers Lua with the component's global registry. Called once at startup, so an editor of
/// Lua finds the grammar. The registry is global, so this needs no window.
pub fn register() {
    LanguageRegistry::singleton().register(NAME, &config());
}

#[cfg(test)]
mod tests {
    use gpui_component::highlighter::SyntaxHighlighter;
    use gpui_component::input::Rope;
    use streaming_iterator::StreamingIterator;
    use tree_sitter::{Parser, Query, QueryCursor};

    use super::*;
    use crate::editor_theme::{COMPONENT_NAMES, highlight};
    use crate::theme::LIGHT;

    const SAMPLE: &str = r#"-- a comment
local M = {}
M.meta = { name = "x", n = 1 }
function M.execute(ctx, input)
  for _, branch in ipairs(input.inputs) do
    if branch == nil then continue end
    local t = type(branch)
    ctx.branch.create({ name = branch })
  end
  return ctx.exec({ "git", "status" }, { timeout = "30s" })
end
export type Shape = { w: number }
return M
"#;

    fn language() -> tree_sitter::Language {
        tree_sitter_lua::LANGUAGE.into()
    }

    /// Every capture of `source` as its text and the name of the capture.
    fn captures(source: &str) -> Vec<(String, String)> {
        let language = language();
        let query = Query::new(&language, HIGHLIGHTS).expect("the query compiles");
        let mut parser = Parser::new();
        parser.set_language(&language).expect("the grammar loads");
        let tree = parser.parse(source, None).expect("the sample parses");
        let bytes = source.as_bytes();
        let mut cursor = QueryCursor::new();
        let mut found = Vec::new();
        let mut matches = cursor.captures(&query, tree.root_node(), bytes);
        while let Some((matched, index)) = matches.next() {
            let capture = &matched.captures[*index];
            found.push((
                capture
                    .node
                    .utf8_text(bytes)
                    .expect("the sample is UTF-8")
                    .to_owned(),
                query.capture_names()[capture.index as usize].to_owned(),
            ));
        }
        found
    }

    #[test]
    fn the_query_compiles_against_the_grammar() {
        Query::new(&language(), HIGHLIGHTS).expect("the query compiles");
    }

    #[test]
    fn every_capture_is_a_name_the_component_paints() {
        let query = Query::new(&language(), HIGHLIGHTS).expect("the query compiles");
        let theme = highlight(&LIGHT, false);
        for name in query.capture_names() {
            assert!(
                COMPONENT_NAMES.contains(name),
                "{name} is not a highlight name of the component"
            );
            assert!(
                theme.style.syntax.style(name).is_some(),
                "{name} paints nothing"
            );
        }
    }

    #[test]
    fn the_sample_is_colored_by_its_role() {
        let found = captures(SAMPLE);
        let expected = [
            ("-- a comment", "comment"),
            ("local", "keyword"),
            ("\"x\"", "string"),
            ("1", "number"),
            ("nil", "constant"),
            ("continue", "keyword"),
            ("export", "keyword"),
            ("ctx", "variable.special"),
            ("execute", "function"),
            ("create", "function"),
            ("name", "property"),
        ];
        for (text, name) in expected {
            assert!(
                found.iter().any(|(t, n)| t == text && n == name),
                "{text} as {name} in {found:?}"
            );
        }
    }

    #[test]
    fn type_is_plain_because_it_is_also_a_lua_builtin() {
        let found = captures(SAMPLE);
        assert!(
            !found.iter().any(|(t, n)| t == "type" && n == "keyword"),
            "{found:?}"
        );
    }

    /// The path the editor takes: the component's highlighter joins the injections, locals and
    /// highlights into one query, and falls back to plain text if the lookup fails.
    #[test]
    fn the_components_highlighter_colors_the_sample_as_keywords() {
        register();
        let mut highlighter = SyntaxHighlighter::new(NAME);
        let text = Rope::from(SAMPLE);
        highlighter.update(None, &text);
        let theme = highlight(&LIGHT, false);
        let keyword = theme
            .style
            .syntax
            .style("keyword")
            .and_then(|style| style.color)
            .expect("keywords have a color");
        let at = SAMPLE.find("local").expect("the sample has a local");
        let styles = highlighter.styles(&(0..text.len()), &theme);
        let colored = styles
            .iter()
            .find(|(range, _)| range.contains(&at))
            .expect("the keyword has a style");
        assert_eq!(colored.1.color, Some(keyword), "{styles:?}");
    }

    #[test]
    fn registration_makes_lua_known_to_the_component() {
        register();
        assert!(LanguageRegistry::singleton().language(NAME).is_some());
    }
}
