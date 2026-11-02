//! Syntax highlighting for the file viewer (N4, D92, D111): tree-sitter grammars compiled into
//! the binary, with the product's own highlight queries (`queries/<language>/`). The module is
//! pure: text in, spans out. It reads no file and touches no repository.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gm_schema::{HighlightSpan, SyntaxRole};
use tree_sitter::{
    ParseOptions, Parser, Query, QueryCursor, QueryCursorOptions, StreamingIterator,
};

/// How long one file may take to parse and query. A file that needs more is shown with no color.
pub const HIGHLIGHT_BUDGET: Duration = Duration::from_secs(2);

/// A line longer than this is not colored: the viewer cuts it, and a minified bundle is one line.
pub const HIGHLIGHT_LINE_LIMIT: usize = 2_000;

/// The languages with a grammar. Each one is behind a Cargo feature (`syntax-<name>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Tsx,
    Java,
    C,
    Cpp,
    Ruby,
    CSharp,
    Json,
    Toml,
    Yaml,
    Bash,
    Html,
    Css,
    Clojure,
}

const LANGUAGES: [Language; 18] = [
    Language::Rust,
    Language::Python,
    Language::Go,
    Language::JavaScript,
    Language::TypeScript,
    Language::Tsx,
    Language::Java,
    Language::C,
    Language::Cpp,
    Language::Ruby,
    Language::CSharp,
    Language::Json,
    Language::Toml,
    Language::Yaml,
    Language::Bash,
    Language::Html,
    Language::Css,
    Language::Clojure,
];

impl Language {
    fn index(self) -> usize {
        self as usize
    }

    /// The grammar, or `None` when the build left it out.
    fn grammar(self) -> Option<tree_sitter::Language> {
        match self {
            #[cfg(feature = "syntax-rust")]
            Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            #[cfg(feature = "syntax-python")]
            Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
            #[cfg(feature = "syntax-go")]
            Language::Go => Some(tree_sitter_go::LANGUAGE.into()),
            #[cfg(feature = "syntax-javascript")]
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            #[cfg(feature = "syntax-typescript")]
            Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            #[cfg(feature = "syntax-typescript")]
            Language::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            #[cfg(feature = "syntax-java")]
            Language::Java => Some(tree_sitter_java::LANGUAGE.into()),
            #[cfg(feature = "syntax-c")]
            Language::C => Some(tree_sitter_c::LANGUAGE.into()),
            #[cfg(feature = "syntax-cpp")]
            Language::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
            #[cfg(feature = "syntax-ruby")]
            Language::Ruby => Some(tree_sitter_ruby::LANGUAGE.into()),
            #[cfg(feature = "syntax-c-sharp")]
            Language::CSharp => Some(tree_sitter_c_sharp::LANGUAGE.into()),
            #[cfg(feature = "syntax-json")]
            Language::Json => Some(tree_sitter_json::LANGUAGE.into()),
            #[cfg(feature = "syntax-toml-ng")]
            Language::Toml => Some(tree_sitter_toml_ng::LANGUAGE.into()),
            #[cfg(feature = "syntax-yaml")]
            Language::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
            #[cfg(feature = "syntax-bash")]
            Language::Bash => Some(tree_sitter_bash::LANGUAGE.into()),
            #[cfg(feature = "syntax-html")]
            Language::Html => Some(tree_sitter_html::LANGUAGE.into()),
            #[cfg(feature = "syntax-css")]
            Language::Css => Some(tree_sitter_css::LANGUAGE.into()),
            #[cfg(feature = "syntax-clojure-orchard")]
            Language::Clojure => Some(tree_sitter_clojure_orchard::LANGUAGE.into()),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The highlight query, layered where the grammar extends another (TypeScript on
    /// JavaScript, C++ on C).
    fn query_source(self) -> String {
        const JAVASCRIPT: &str = include_str!("../queries/javascript/highlights.scm");
        const JSX: &str = include_str!("../queries/javascript/jsx.scm");
        const TYPESCRIPT: &str = include_str!("../queries/typescript/highlights.scm");
        const C: &str = include_str!("../queries/c/highlights.scm");
        const CPP: &str = include_str!("../queries/cpp/highlights.scm");
        match self {
            Language::Rust => include_str!("../queries/rust/highlights.scm").to_owned(),
            Language::Python => include_str!("../queries/python/highlights.scm").to_owned(),
            Language::Go => include_str!("../queries/go/highlights.scm").to_owned(),
            Language::JavaScript => format!("{JAVASCRIPT}\n{JSX}"),
            Language::TypeScript => format!("{JAVASCRIPT}\n{TYPESCRIPT}"),
            Language::Tsx => format!("{JAVASCRIPT}\n{JSX}\n{TYPESCRIPT}"),
            Language::Java => include_str!("../queries/java/highlights.scm").to_owned(),
            Language::C => C.to_owned(),
            Language::Cpp => format!("{C}\n{CPP}"),
            Language::Ruby => include_str!("../queries/ruby/highlights.scm").to_owned(),
            Language::CSharp => include_str!("../queries/csharp/highlights.scm").to_owned(),
            Language::Json => include_str!("../queries/json/highlights.scm").to_owned(),
            Language::Toml => include_str!("../queries/toml/highlights.scm").to_owned(),
            Language::Yaml => include_str!("../queries/yaml/highlights.scm").to_owned(),
            Language::Bash => include_str!("../queries/bash/highlights.scm").to_owned(),
            Language::Html => include_str!("../queries/html/highlights.scm").to_owned(),
            Language::Css => include_str!("../queries/css/highlights.scm").to_owned(),
            Language::Clojure => include_str!("../queries/clojure/highlights.scm").to_owned(),
        }
    }
}

/// The language of the file at `path`, by its name or extension, when the build has its
/// grammar. A header (`.h`) is read as C, which also reads most C++ headers.
#[must_use]
pub fn language_for_path(path: &str) -> Option<Language> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let language = match name {
        "Rakefile" | "Gemfile" | "Brewfile" | "Podfile" => Language::Ruby,
        "Cargo.lock" | "Pipfile" => Language::Toml,
        ".bashrc" | ".bash_profile" | ".bash_logout" | ".profile" | ".zshrc" | ".zprofile" => {
            Language::Bash
        }
        _ => {
            let (_, extension) = name.rsplit_once('.')?;
            match extension.to_ascii_lowercase().as_str() {
                "rs" => Language::Rust,
                "py" | "pyi" | "pyw" => Language::Python,
                "go" => Language::Go,
                "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
                "ts" | "mts" | "cts" => Language::TypeScript,
                "tsx" => Language::Tsx,
                "java" => Language::Java,
                "c" | "h" => Language::C,
                "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "ipp" => {
                    Language::Cpp
                }
                "rb" | "rake" | "gemspec" | "ru" => Language::Ruby,
                "cs" => Language::CSharp,
                "json" | "jsonc" | "geojson" => Language::Json,
                "toml" => Language::Toml,
                "yml" | "yaml" => Language::Yaml,
                "sh" | "bash" | "zsh" => Language::Bash,
                "html" | "htm" => Language::Html,
                "css" => Language::Css,
                "clj" | "cljs" | "cljc" | "cljd" | "edn" | "bb" => Language::Clojure,
                _ => return None,
            }
        }
    };
    language.grammar().map(|_| language)
}

/// The role of a highlight query capture, by its dotted name. `None` is plain text: a bare
/// `@variable` is an identifier, and coloring every one would drown the code.
#[must_use]
pub fn role_for_capture(name: &str) -> Option<SyntaxRole> {
    // A capture named `_x` only feeds a predicate of its pattern.
    if name.starts_with('_') {
        return None;
    }
    match name {
        "variable.builtin" | "variable.parameter" => return Some(SyntaxRole::Variable),
        "function.macro" => return Some(SyntaxRole::Attribute),
        // A key of a JSON object is a name, not a string to read.
        "string.special.key" => return Some(SyntaxRole::Property),
        _ => {}
    }
    let head = name.split('.').next().unwrap_or(name);
    match head {
        "comment" => Some(SyntaxRole::Comment),
        "string" | "escape" => Some(SyntaxRole::String),
        "number" => Some(SyntaxRole::Number),
        // The at-rules of CSS (`@media`, `@import`) are captured under their own names.
        "keyword" | "tag" | "charset" | "import" | "keyframes" | "media" | "supports"
        | "namespace" => Some(SyntaxRole::Keyword),
        "operator" => Some(SyntaxRole::Operator),
        "function" => Some(SyntaxRole::Function),
        "type" | "constructor" | "module" => Some(SyntaxRole::Type),
        "constant" | "boolean" => Some(SyntaxRole::Constant),
        "property" => Some(SyntaxRole::Property),
        "punctuation" | "delimiter" => Some(SyntaxRole::Punctuation),
        "attribute" | "label" => Some(SyntaxRole::Attribute),
        _ => None,
    }
}

/// A compiled highlight query and the role of each of its captures.
struct Compiled {
    query: Query,
    roles: Vec<Option<SyntaxRole>>,
}

/// The query of `language`, compiled on first use (5 to 41 ms) and kept.
fn compiled(language: Language) -> Option<&'static Compiled> {
    static CACHE: [OnceLock<Option<Compiled>>; LANGUAGES.len()] =
        [const { OnceLock::new() }; LANGUAGES.len()];
    CACHE[language.index()]
        .get_or_init(|| {
            let grammar = language.grammar()?;
            let query = Query::new(&grammar, &language.query_source()).ok()?;
            let roles = query
                .capture_names()
                .iter()
                .map(|name| role_for_capture(name))
                .collect();
            Some(Compiled { query, roles })
        })
        .as_ref()
}

fn code_of(role: Option<SyntaxRole>) -> u8 {
    role.map_or(0, |role| role as u8 + 1)
}

fn role_of(code: u8) -> Option<SyntaxRole> {
    const ROLES: [SyntaxRole; 12] = [
        SyntaxRole::Comment,
        SyntaxRole::String,
        SyntaxRole::Number,
        SyntaxRole::Keyword,
        SyntaxRole::Operator,
        SyntaxRole::Function,
        SyntaxRole::Type,
        SyntaxRole::Constant,
        SyntaxRole::Variable,
        SyntaxRole::Property,
        SyntaxRole::Punctuation,
        SyntaxRole::Attribute,
    ];
    ROLES.get(usize::from(code).checked_sub(1)?).copied()
}

/// The colors of `text` as `language` reads it, within `HIGHLIGHT_BUDGET`. `None` when `stop`
/// is set, the time ran out, or the build has no grammar for the language.
///
/// The spans are in order, do not overlap, never empty, and fall on character boundaries of
/// `text`. Where captures overlap, the smaller range wins, and of equal ranges the later
/// pattern of the query; that is how a specific pattern refines a general one. Text no capture
/// names has no span.
#[must_use]
pub fn highlight(language: Language, text: &str, stop: &AtomicBool) -> Option<Vec<HighlightSpan>> {
    highlight_within(language, text, stop, HIGHLIGHT_BUDGET)
}

/// `highlight` with a time limit of its own.
#[must_use]
pub fn highlight_within(
    language: Language,
    text: &str,
    stop: &AtomicBool,
    budget: Duration,
) -> Option<Vec<HighlightSpan>> {
    let compiled = compiled(language)?;
    let deadline = Instant::now() + budget;
    let should_stop = || stop.load(Ordering::SeqCst) || Instant::now() >= deadline;

    let mut parser = Parser::new();
    parser.set_language(&language.grammar()?).ok()?;
    let bytes = text.as_bytes();
    // tree-sitter 0.25 takes a progress callback that answers `true` to cancel.
    let mut on_parse = |_: &tree_sitter::ParseState| should_stop();
    let tree = parser.parse_with_options(
        &mut |offset, _| &bytes[offset.min(bytes.len())..],
        None,
        Some(ParseOptions::new().progress_callback(&mut on_parse)),
    )?;

    let mut on_query = |_: &tree_sitter::QueryCursorState| should_stop();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures_with_options(
        &compiled.query,
        tree.root_node(),
        bytes,
        QueryCursorOptions::new().progress_callback(&mut on_query),
    );
    // (start, end, pattern, code)
    let mut found: Vec<(usize, usize, usize, u8)> = Vec::new();
    while let Some((matched, index)) = captures.next() {
        let capture = matched.captures[*index];
        let node = capture.node;
        let (start, end) = (node.start_byte(), node.end_byte().min(bytes.len()));
        // A capture with no role (a bare `@variable`) is plain text: it paints nothing, so a
        // generic pattern can sit anywhere in a query without hiding a specific one.
        if let (true, Some(role)) = (start < end, compiled.roles[capture.index as usize]) {
            found.push((start, end, matched.pattern_index, code_of(Some(role))));
        }
    }
    drop(captures);
    if should_stop() {
        return None;
    }

    // Paint the larger ranges first, so a smaller one overwrites the part it refines.
    found.sort_by_key(|&(start, end, pattern, _)| (std::cmp::Reverse(end - start), pattern, start));
    let mut paint = vec![0_u8; bytes.len()];
    for (start, end, _, code) in found {
        paint[start..end].fill(code);
    }
    clear_long_lines(bytes, &mut paint);

    let mut spans = Vec::new();
    let mut at = 0;
    while at < paint.len() {
        let code = paint[at];
        let mut end = at + 1;
        while end < paint.len() && paint[end] == code {
            end += 1;
        }
        if let Some(role) = role_of(code) {
            spans.push(HighlightSpan {
                start: at,
                end,
                role,
            });
        }
        at = end;
    }
    Some(spans)
}

/// Leaves a line longer than `HIGHLIGHT_LINE_LIMIT` uncolored.
fn clear_long_lines(bytes: &[u8], paint: &mut [u8]) {
    let mut start = 0;
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        if line.len() > HIGHLIGHT_LINE_LIMIT {
            paint[start..start + line.len()].fill(0);
        }
        start += line.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn run(language: Language, text: &str) -> Vec<HighlightSpan> {
        highlight(language, text, &AtomicBool::new(false)).expect("a grammar is compiled in")
    }

    /// The text of the first span with `role`.
    fn first<'a>(text: &'a str, spans: &[HighlightSpan], role: SyntaxRole) -> Option<&'a str> {
        spans
            .iter()
            .find(|span| span.role == role)
            .map(|span| &text[span.start..span.end])
    }

    fn texts<'a>(text: &'a str, spans: &[HighlightSpan], role: SyntaxRole) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|span| span.role == role)
            .map(|span| &text[span.start..span.end])
            .collect()
    }

    #[test]
    fn language_comes_from_the_extension() {
        assert_eq!(language_for_path("src/main.rs"), Some(Language::Rust));
        assert_eq!(language_for_path("a/b/tool.PY"), Some(Language::Python));
        assert_eq!(language_for_path("x.go"), Some(Language::Go));
        assert_eq!(language_for_path("x.mjs"), Some(Language::JavaScript));
        assert_eq!(language_for_path("x.jsx"), Some(Language::JavaScript));
        assert_eq!(language_for_path("x.ts"), Some(Language::TypeScript));
        assert_eq!(language_for_path("x.tsx"), Some(Language::Tsx));
        assert_eq!(language_for_path("README.md"), None);
        assert_eq!(language_for_path("Makefile"), None);
        assert_eq!(language_for_path(".rs"), Some(Language::Rust));
        assert_eq!(language_for_path("dir.rs/file"), None);
    }

    #[test]
    fn captures_map_to_roles_by_their_dotted_name() {
        assert_eq!(
            role_for_capture("comment.documentation"),
            Some(SyntaxRole::Comment)
        );
        assert_eq!(role_for_capture("string.special"), Some(SyntaxRole::String));
        assert_eq!(role_for_capture("escape"), Some(SyntaxRole::String));
        assert_eq!(
            role_for_capture("function.method"),
            Some(SyntaxRole::Function)
        );
        assert_eq!(
            role_for_capture("function.macro"),
            Some(SyntaxRole::Attribute)
        );
        assert_eq!(role_for_capture("constructor"), Some(SyntaxRole::Type));
        assert_eq!(role_for_capture("type.builtin"), Some(SyntaxRole::Type));
        assert_eq!(
            role_for_capture("constant.builtin"),
            Some(SyntaxRole::Constant)
        );
        assert_eq!(
            role_for_capture("variable.parameter"),
            Some(SyntaxRole::Variable)
        );
        assert_eq!(
            role_for_capture("variable.builtin"),
            Some(SyntaxRole::Variable)
        );
        assert_eq!(
            role_for_capture("punctuation.bracket"),
            Some(SyntaxRole::Punctuation)
        );
        assert_eq!(role_for_capture("tag"), Some(SyntaxRole::Keyword));
        // A bare variable is plain text, and so is what the viewer cannot name.
        assert_eq!(role_for_capture("variable"), None);
        assert_eq!(role_for_capture("embedded"), None);
        assert_eq!(role_for_capture("nonsense"), None);
    }

    #[test]
    fn every_capture_of_every_query_has_a_known_name() {
        // A capture the mapping does not know is invisible in the viewer; list them here so a new
        // one in a query file is a decision, not an accident.
        let silent = ["variable", "embedded"];
        for language in LANGUAGES {
            let Some(compiled) = compiled(language) else {
                panic!("{language:?}: the query does not compile");
            };
            for (name, role) in compiled.query.capture_names().iter().zip(&compiled.roles) {
                assert!(
                    role.is_some() || name.starts_with('_') || silent.contains(name),
                    "{language:?}: capture @{name} has no role"
                );
            }
        }
    }

    #[test]
    fn rust_is_colored_by_role() {
        let text = "// note\nfn add(a: u32) -> u32 {\n    let s = \"hi\";\n    a + 1\n}\n";
        let spans = run(Language::Rust, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        assert_eq!(first(text, &spans, SyntaxRole::Keyword), Some("fn"));
        assert_eq!(first(text, &spans, SyntaxRole::Function), Some("add"));
        assert!(texts(text, &spans, SyntaxRole::Type).contains(&"u32"));
        assert_eq!(first(text, &spans, SyntaxRole::String), Some("\"hi\""));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("1"));
    }

    #[test]
    fn python_is_colored_by_role() {
        let text = "# note\ndef greet(name):\n    return f\"hi {name}\" + str(3)\n";
        let spans = run(Language::Python, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("# note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"def"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"greet"));
        assert!(texts(text, &spans, SyntaxRole::Number).contains(&"3"));
    }

    #[test]
    fn go_is_colored_by_role() {
        let text = "package main\n\n// note\nfunc Add(a int) int {\n\treturn a + 1\n}\n";
        let spans = run(Language::Go, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"func"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"Add"));
        assert!(texts(text, &spans, SyntaxRole::Type).contains(&"int"));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("1"));
    }

    #[test]
    fn javascript_is_colored_by_role() {
        let text = "// note\nfunction add(a) { return a + 'x' + 2; }\n";
        let spans = run(Language::JavaScript, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"function"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"add"));
        assert_eq!(first(text, &spans, SyntaxRole::String), Some("'x'"));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("2"));
    }

    #[test]
    fn typescript_adds_its_own_keywords_and_types_to_javascript() {
        let text =
            "interface Point { x: number }\nexport function f(p: Point): string { return 'a'; }\n";
        let spans = run(Language::TypeScript, text);
        let keywords = texts(text, &spans, SyntaxRole::Keyword);
        assert!(keywords.contains(&"interface"));
        assert!(keywords.contains(&"export"));
        assert!(keywords.contains(&"function"));
        let types = texts(text, &spans, SyntaxRole::Type);
        assert!(types.contains(&"Point"));
        assert!(types.contains(&"number"));
        assert_eq!(first(text, &spans, SyntaxRole::String), Some("'a'"));
    }

    #[test]
    fn tsx_colors_jsx_tags_and_attributes() {
        let text = "const a = <div className=\"x\">hi</div>;\n";
        let spans = run(Language::Tsx, text);
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"div"));
        assert!(texts(text, &spans, SyntaxRole::Attribute).contains(&"className"));
    }

    #[test]
    fn language_comes_from_the_name_or_extension_for_the_wider_set() {
        let cases = [
            ("A.java", Language::Java),
            ("x.c", Language::C),
            ("x.h", Language::C),
            ("x.cpp", Language::Cpp),
            ("x.hpp", Language::Cpp),
            ("x.cc", Language::Cpp),
            ("x.rb", Language::Ruby),
            ("Gemfile", Language::Ruby),
            ("Rakefile", Language::Ruby),
            ("X.cs", Language::CSharp),
            ("package.json", Language::Json),
            ("Cargo.toml", Language::Toml),
            ("Cargo.lock", Language::Toml),
            ("ci.yml", Language::Yaml),
            ("ci.yaml", Language::Yaml),
            ("run.sh", Language::Bash),
            (".bashrc", Language::Bash),
            ("index.html", Language::Html),
            ("site.css", Language::Css),
            ("core.clj", Language::Clojure),
            ("app.cljs", Language::Clojure),
            ("shared.cljc", Language::Clojure),
            ("deps.edn", Language::Clojure),
        ];
        for (path, language) in cases {
            assert_eq!(language_for_path(path), Some(language), "{path}");
        }
        assert_eq!(language_for_path("x.scss"), None);
        assert_eq!(language_for_path("x.md"), None);
        assert_eq!(language_for_path("Makefile"), None);
    }

    #[test]
    fn java_is_colored_by_role() {
        let text = "// note\npublic class A {\n  int f(String s) { return 42; }\n}\n";
        let spans = run(Language::Java, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"class"));
        assert!(texts(text, &spans, SyntaxRole::Type).contains(&"String"));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("42"));
    }

    #[test]
    fn c_is_colored_by_role() {
        let text = "/* note */\nstatic int add(int a) { return a + 1; }\nconst char *s = \"hi\";\n";
        let spans = run(Language::C, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("/* note */"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"return"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"add"));
        assert!(texts(text, &spans, SyntaxRole::Type).contains(&"int"));
        assert_eq!(first(text, &spans, SyntaxRole::String), Some("\"hi\""));
    }

    #[test]
    fn cpp_adds_its_own_keywords_to_c() {
        let text = "// note\nnamespace n { class A { public: virtual int f() const; }; }\n";
        let spans = run(Language::Cpp, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        let keywords = texts(text, &spans, SyntaxRole::Keyword);
        assert!(keywords.contains(&"namespace"));
        assert!(keywords.contains(&"class"));
        assert!(keywords.contains(&"virtual"));
    }

    #[test]
    fn ruby_is_colored_by_role() {
        let text = "# note\ndef greet(name)\n  puts \"hi #{name}\" + 3.to_s\nend\n";
        let spans = run(Language::Ruby, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("# note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"def"));
        assert!(texts(text, &spans, SyntaxRole::Number).contains(&"3"));
    }

    #[test]
    fn csharp_is_colored_by_role() {
        let text = "// note\nclass A { public int F(string s) { return 7; } }\n";
        let spans = run(Language::CSharp, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("// note"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"class"));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("7"));
    }

    #[test]
    fn json_keys_are_properties_and_values_keep_their_own_roles() {
        let text = "{\"name\": \"gm\", \"n\": 12, \"ok\": true, \"x\": null}\n";
        let spans = run(Language::Json, text);
        let keys = texts(text, &spans, SyntaxRole::Property);
        assert!(
            keys.contains(&"\"name\"") && keys.contains(&"\"n\""),
            "{keys:?}"
        );
        assert!(texts(text, &spans, SyntaxRole::String).contains(&"\"gm\""));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("12"));
        assert!(texts(text, &spans, SyntaxRole::Constant).contains(&"true"));
    }

    #[test]
    fn toml_is_colored_by_role() {
        let text = "# note\n[package]\nname = \"gm\"\nversion = 3\nok = true\n";
        let spans = run(Language::Toml, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("# note"));
        assert!(texts(text, &spans, SyntaxRole::Property).contains(&"name"));
        assert!(texts(text, &spans, SyntaxRole::String).contains(&"\"gm\""));
        assert_eq!(first(text, &spans, SyntaxRole::Number), Some("3"));
        assert!(texts(text, &spans, SyntaxRole::Constant).contains(&"true"));
    }

    #[test]
    fn yaml_is_colored_by_role() {
        let text = "# note\nname: gm\ncount: 3\non: true\nlist:\n  - \"a\"\n";
        let spans = run(Language::Yaml, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("# note"));
        assert!(texts(text, &spans, SyntaxRole::Property).contains(&"name"));
        assert!(texts(text, &spans, SyntaxRole::Number).contains(&"3"));
        assert!(texts(text, &spans, SyntaxRole::String).contains(&"\"a\""));
    }

    #[test]
    fn bash_is_colored_by_role() {
        let text = "# note\nif [ -f x ]; then\n  echo \"hi $USER\"\nfi\n";
        let spans = run(Language::Bash, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("# note"));
        let keywords = texts(text, &spans, SyntaxRole::Keyword);
        assert!(keywords.contains(&"if") && keywords.contains(&"fi"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"echo"));
    }

    #[test]
    fn html_colors_tags_attributes_and_comments() {
        let text = "<!-- note -->\n<div class=\"a\">hi</div>\n";
        let spans = run(Language::Html, text);
        assert_eq!(
            first(text, &spans, SyntaxRole::Comment),
            Some("<!-- note -->")
        );
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"div"));
        assert!(texts(text, &spans, SyntaxRole::Attribute).contains(&"class"));
        assert!(texts(text, &spans, SyntaxRole::String).contains(&"\"a\""));
    }

    #[test]
    fn css_is_colored_by_role() {
        let text = "/* note */\n@media (min-width: 10px) { a.b { color: #fff; margin: 4px; } }\n";
        let spans = run(Language::Css, text);
        assert_eq!(first(text, &spans, SyntaxRole::Comment), Some("/* note */"));
        assert!(texts(text, &spans, SyntaxRole::Keyword).contains(&"@media"));
        assert!(texts(text, &spans, SyntaxRole::Property).contains(&"color"));
        assert!(
            texts(text, &spans, SyntaxRole::Number)
                .iter()
                .any(|n| n.starts_with('4'))
        );
    }

    #[test]
    fn clojure_is_colored_by_role() {
        let text = ";; note\n(ns app.core (:require [clojure.string :as str]))\n\n(defn greet\n  [name]\n  (str \"hi \" name 42 true nil))\n\n(def ^:private limit 10)\n#_(ignored)\n";
        let spans = run(Language::Clojure, text);
        // This grammar's comment node ends after the line break.
        assert_eq!(
            first(text, &spans, SyntaxRole::Comment).map(str::trim_end),
            Some(";; note")
        );
        // Only the `#_` marker: the form inside keeps its own colors, as the smaller range wins.
        assert!(texts(text, &spans, SyntaxRole::Comment).contains(&"#_"));
        let keywords = texts(text, &spans, SyntaxRole::Keyword);
        for word in ["ns", "defn", "def"] {
            assert!(keywords.contains(&word), "{word}: {keywords:?}");
        }
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"greet"));
        assert!(texts(text, &spans, SyntaxRole::Function).contains(&"str"));
        assert!(texts(text, &spans, SyntaxRole::Type).contains(&"app.core"));
        assert!(texts(text, &spans, SyntaxRole::Constant).contains(&"limit"));
        assert!(texts(text, &spans, SyntaxRole::Constant).contains(&":require"));
        assert!(texts(text, &spans, SyntaxRole::Constant).contains(&"nil"));
        assert!(texts(text, &spans, SyntaxRole::String).contains(&"\"hi \""));
        assert!(texts(text, &spans, SyntaxRole::Number).contains(&"42"));
        assert!(texts(text, &spans, SyntaxRole::Attribute).contains(&"^:private"));
    }

    #[test]
    fn a_plain_identifier_has_no_color() {
        let text = "fn main() { let value = other; }\n";
        let spans = run(Language::Rust, text);
        assert!(!texts(text, &spans, SyntaxRole::Variable).contains(&"value"));
        let covered: Vec<_> = spans.iter().map(|s| &text[s.start..s.end]).collect();
        assert!(!covered.contains(&"other"));
    }

    #[test]
    fn multibyte_text_keeps_spans_on_character_boundaries() {
        let text = "// olá, ação ✓\nfn café() { let s = \"日本語\"; }\n";
        let spans = run(Language::Rust, text);
        assert_eq!(
            first(text, &spans, SyntaxRole::Comment),
            Some("// olá, ação ✓")
        );
        assert_eq!(first(text, &spans, SyntaxRole::String), Some("\"日本語\""));
        for span in &spans {
            assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
        }
    }

    #[test]
    fn a_syntax_error_still_colors_the_rest() {
        let text = "fn broken( {{{\n// still a comment\nlet x = 1;\n";
        let spans = run(Language::Rust, text);
        assert_eq!(
            first(text, &spans, SyntaxRole::Comment),
            Some("// still a comment")
        );
    }

    #[test]
    fn a_very_long_line_is_left_plain() {
        let long = format!("let s = \"{}\";", "x".repeat(HIGHLIGHT_LINE_LIMIT + 10));
        let text = format!("// kept\n{long}\n// kept too\n");
        let spans = run(Language::Rust, &text);
        let comments = texts(&text, &spans, SyntaxRole::Comment);
        assert_eq!(comments, ["// kept", "// kept too"]);
        assert!(texts(&text, &spans, SyntaxRole::String).is_empty());
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(run(Language::Rust, "").is_empty());
    }

    #[test]
    fn a_stop_request_ends_the_work_with_no_spans() {
        let stop = AtomicBool::new(true);
        assert_eq!(highlight(Language::Rust, "fn main() {}\n", &stop), None);
    }

    #[test]
    fn a_file_that_takes_too_long_shows_with_no_color() {
        let text = "fn f() { let x = 1; }\n".repeat(20_000);
        let stop = AtomicBool::new(false);
        assert_eq!(
            highlight_within(Language::Rust, &text, &stop, Duration::ZERO),
            None
        );
    }

    #[test]
    fn a_large_file_is_colored_to_the_end() {
        // The product budget is checked by the release timing test below. Here the file must be
        // colored in full, so the clock gets a test-only limit that a loaded debug build cannot hit.
        let text = "fn f(a: u32) -> u32 { a + 1 } // c\n".repeat(3_000);
        let spans = highlight_within(
            Language::Rust,
            &text,
            &AtomicBool::new(false),
            Duration::from_secs(600),
        )
        .expect("colored within the test limit");
        assert_eq!(texts(&text, &spans, SyntaxRole::Comment).len(), 3_000);
    }

    /// The cost on real code, for a release build: `cargo test --release -p gm-core --lib
    /// syntax::tests::timing -- --ignored --nocapture`. Debug builds are several times slower.
    #[test]
    #[ignore = "a measurement for release builds"]
    fn timing_of_a_file_at_the_viewer_limit() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut text = String::new();
        'fill: loop {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "rs") {
                    text.push_str(&std::fs::read_to_string(path).unwrap());
                    if text.len() >= gm_schema::FILE_VIEW_LIMIT {
                        break 'fill;
                    }
                }
            }
        }
        text.truncate(text.floor_char_boundary(gm_schema::FILE_VIEW_LIMIT));
        let stop = AtomicBool::new(false);
        let warm = Instant::now();
        let _ = highlight(Language::Rust, "fn a() {}\n", &stop);
        eprintln!("first use (query compile): {:?}", warm.elapsed());
        let started = Instant::now();
        let spans = highlight(Language::Rust, &text, &stop).expect("within the budget");
        eprintln!(
            "{} bytes, {} lines, {} spans: {:?}",
            text.len(),
            text.lines().count(),
            spans.len(),
            started.elapsed()
        );
        assert!(started.elapsed() < HIGHLIGHT_BUDGET);
    }

    fn sample_language() -> impl Strategy<Value = Language> {
        prop::sample::select(LANGUAGES.to_vec())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn spans_are_ordered_apart_and_on_character_boundaries(
            language in sample_language(),
            text in "[ -~\n\té日ñ]{0,400}",
        ) {
            let spans = highlight(language, &text, &AtomicBool::new(false)).unwrap();
            let mut end = 0;
            for span in &spans {
                prop_assert!(span.start < span.end);
                prop_assert!(span.start >= end, "overlap or disorder at {span:?}");
                prop_assert!(span.end <= text.len());
                prop_assert!(text.is_char_boundary(span.start));
                prop_assert!(text.is_char_boundary(span.end));
                end = span.end;
            }
        }

        #[test]
        fn the_same_text_gives_the_same_spans(
            language in sample_language(),
            text in "[ -~\n]{0,300}",
        ) {
            let stop = AtomicBool::new(false);
            prop_assert_eq!(
                highlight(language, &text, &stop),
                highlight(language, &text, &stop)
            );
        }
    }
}
