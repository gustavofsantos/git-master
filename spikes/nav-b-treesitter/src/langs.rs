//! The grammars the spike can load, each behind a Cargo feature, with the
//! queries the grammar crate ships.

use tree_sitter::Language;

pub struct Lang {
    pub name: &'static str,
    pub language: Language,
    /// The tags query as published by the grammar crate.
    pub tags: &'static str,
    pub highlights: &'static str,
    /// File extensions (without the dot) this grammar parses.
    pub exts: &'static [&'static str],
}

pub fn all() -> Vec<Lang> {
    #[allow(unused_mut)]
    let mut v: Vec<Lang> = Vec::new();
    #[cfg(feature = "rust")]
    v.push(Lang {
        name: "rust",
        language: tree_sitter_rust::LANGUAGE.into(),
        tags: tree_sitter_rust::TAGS_QUERY,
        highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
        exts: &["rs"],
    });
    #[cfg(feature = "python")]
    v.push(Lang {
        name: "python",
        language: tree_sitter_python::LANGUAGE.into(),
        tags: tree_sitter_python::TAGS_QUERY,
        highlights: tree_sitter_python::HIGHLIGHTS_QUERY,
        exts: &["py"],
    });
    #[cfg(feature = "go")]
    v.push(Lang {
        name: "go",
        language: tree_sitter_go::LANGUAGE.into(),
        tags: tree_sitter_go::TAGS_QUERY,
        highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
        exts: &["go"],
    });
    #[cfg(feature = "typescript")]
    {
        v.push(Lang {
            name: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tags: tree_sitter_typescript::TAGS_QUERY,
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
            exts: &["ts", "mts", "cts"],
        });
        v.push(Lang {
            name: "tsx",
            language: tree_sitter_typescript::LANGUAGE_TSX.into(),
            tags: tree_sitter_typescript::TAGS_QUERY,
            highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
            exts: &["tsx"],
        });
    }
    #[cfg(feature = "javascript")]
    v.push(Lang {
        name: "javascript",
        language: tree_sitter_javascript::LANGUAGE.into(),
        tags: tree_sitter_javascript::TAGS_QUERY,
        highlights: tree_sitter_javascript::HIGHLIGHT_QUERY,
        exts: &["js", "mjs", "cjs", "jsx"],
    });
    #[cfg(feature = "java")]
    v.push(Lang {
        name: "java",
        language: tree_sitter_java::LANGUAGE.into(),
        tags: tree_sitter_java::TAGS_QUERY,
        highlights: tree_sitter_java::HIGHLIGHTS_QUERY,
        exts: &["java"],
    });
    #[cfg(feature = "c")]
    v.push(Lang {
        name: "c",
        language: tree_sitter_c::LANGUAGE.into(),
        tags: tree_sitter_c::TAGS_QUERY,
        highlights: tree_sitter_c::HIGHLIGHT_QUERY,
        exts: &["c", "h"],
    });
    #[cfg(feature = "cpp")]
    v.push(Lang {
        name: "cpp",
        language: tree_sitter_cpp::LANGUAGE.into(),
        tags: tree_sitter_cpp::TAGS_QUERY,
        highlights: tree_sitter_cpp::HIGHLIGHT_QUERY,
        exts: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
    });
    #[cfg(feature = "ruby")]
    v.push(Lang {
        name: "ruby",
        language: tree_sitter_ruby::LANGUAGE.into(),
        tags: tree_sitter_ruby::TAGS_QUERY,
        highlights: tree_sitter_ruby::HIGHLIGHTS_QUERY,
        exts: &["rb"],
    });
    #[cfg(feature = "csharp")]
    v.push(Lang {
        name: "csharp",
        language: tree_sitter_c_sharp::LANGUAGE.into(),
        tags: tree_sitter_c_sharp::TAGS_QUERY,
        highlights: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        exts: &["cs"],
    });
    v
}
