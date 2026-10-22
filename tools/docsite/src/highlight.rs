//! Syntax highlighting for fenced code blocks. Tokens become `<span class="hl-...">`; the colors
//! live in the site's stylesheet, so the page follows the light and dark themes.

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub struct Highlighter {
    syntaxes: SyntaxSet,
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
        }
    }

    /// The inner HTML of a code block. A language with no grammar comes out as escaped plain text.
    pub fn highlight(&self, lang: &str, code: &str) -> String {
        let syntax = match lang {
            "" | "text" | "txt" | "plain" => None,
            other => self.syntaxes.find_syntax_by_token(other),
        };
        let Some(syntax) = syntax else {
            return escape(code);
        };
        let mut generator = ClassedHTMLGenerator::new_with_class_style(
            syntax,
            &self.syntaxes,
            ClassStyle::SpacedPrefixed { prefix: "hl-" },
        );
        for line in LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                return escape(code);
            }
        }
        generator.finalize()
    }
}

/// Escapes text for HTML element content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape("a < b && \"c\""),
            "a &lt; b &amp;&amp; &quot;c&quot;"
        );
    }

    #[test]
    fn plain_text_is_escaped_not_highlighted() {
        let html = Highlighter::new().highlight("text", "<tag>");
        assert_eq!(html, "&lt;tag&gt;");
        assert_eq!(
            Highlighter::new().highlight("no-such-language", "a & b"),
            "a &amp; b"
        );
    }

    #[test]
    fn lua_gets_token_classes() {
        let html = Highlighter::new().highlight("lua", "local x = 1 -- note\n");
        assert!(html.contains("hl-"), "{html}");
        assert!(html.contains("local"), "{html}");
        assert!(!html.contains('<') || html.contains("<span"), "{html}");
    }
}
