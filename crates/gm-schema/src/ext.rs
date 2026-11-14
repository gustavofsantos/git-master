//! The document an extension answers a slot with (design/extensions.md, D253, D254): a tree of
//! typed nodes that every front end draws with its own theme, folds and keys. The extension
//! never names a color or draws.
//!
//! A script returns loose Lua data (a string where an inline is expected, one node where a list
//! is expected); `Document::parse` turns it into these strict types, checks the limits and
//! cleans the text, or says exactly where it does not fit.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the protocol this build speaks (`meta.api`).
pub const API_VERSION: u64 = 1;
/// The most nodes, blocks and inlines together, in one document.
pub const MAX_NODES: usize = 200;
/// How deep sections nest.
pub const MAX_SECTION_DEPTH: usize = 3;
/// The longest string, in characters.
pub const MAX_TEXT_CHARS: usize = 2_000;
/// The largest document, as JSON.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

/// What a slot shows: blocks, top to bottom.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// A titled section that folds.
    Section {
        title: String,
        children: Vec<Block>,
        #[serde(default)]
        folded: bool,
    },
    /// A label and its value on one row.
    Field { label: String, value: Vec<Inline> },
    /// A paragraph.
    Text { inlines: Vec<Inline> },
    /// One row per item.
    List { items: Vec<Vec<Inline>> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    Span {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Badge {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Code {
        text: String,
    },
    Link {
        text: String,
        target: LinkTarget,
    },
}

/// A role, never a color: each front end maps it to its theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Default,
    Muted,
    Ok,
    Warning,
    Danger,
    Info,
}

const TONES: &[(&str, Tone)] = &[
    ("default", Tone::Default),
    ("muted", Tone::Muted),
    ("ok", Tone::Ok),
    ("warning", Tone::Warning),
    ("danger", Tone::Danger),
    ("info", Tone::Info),
];

/// What a link does. The person always clicks it; the host never follows one by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkTarget {
    /// An `https` or `http` address, opened in the browser.
    Url(String),
    /// A hash or a ref, shown in the app.
    Commit(String),
    /// A path relative to the repository's root, and a 1-based line.
    File { path: String, line: Option<u32> },
}

/// Where a document does not fit, and why. `path` reads like `[1].children[2].value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for DocError {}

impl Document {
    /// Reads what a slot function returned. `Null` is an empty document: nothing to show.
    pub fn parse(value: &Value) -> Result<Self, DocError> {
        let mut parser = Parser { nodes: 0 };
        let blocks = match value {
            Value::Null => Vec::new(),
            Value::Object(map) if map.contains_key("type") => {
                vec![parser.block(value, "[1]", 1)?]
            }
            _ => parser.blocks(value, "", 1)?,
        };
        let document = Self { blocks };
        let size = serde_json::to_vec(&document).map_or(usize::MAX, |bytes| bytes.len());
        if size > MAX_DOCUMENT_BYTES {
            return Err(DocError {
                path: String::new(),
                message: format!(
                    "the document is {size} bytes, more than the {MAX_DOCUMENT_BYTES} allowed"
                ),
            });
        }
        Ok(document)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

struct Parser {
    nodes: usize,
}

fn error(path: &str, message: impl Into<String>) -> DocError {
    DocError {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// The Lua words for a JSON value, for messages.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "nil",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a table",
    }
}

fn join(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.to_owned()
    } else {
        format!("{path}.{field}")
    }
}

/// A list, or a Lua table with no keys (an empty table converts to an empty object).
fn items<'v>(value: &'v Value, path: &str, what: &str) -> Result<Vec<&'v Value>, DocError> {
    match value {
        Value::Array(list) => Ok(list.iter().collect()),
        Value::Object(map) if map.is_empty() => Ok(Vec::new()),
        other => Err(error(
            path,
            format!("expected a list of {what}, found {}", kind(other)),
        )),
    }
}

/// Keeps text as text: no escape sequences, no control characters. A tab becomes a space;
/// a line break is kept only where `multiline` allows it, else it becomes a space.
fn clean(text: &str, multiline: bool) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' if multiline => Some('\n'),
            '\n' | '\t' | '\r' => Some(' '),
            c if c.is_control() => None,
            // Bidirectional overrides and isolates can reorder what the reader sees.
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect()
}

impl Parser {
    fn count(&mut self, path: &str) -> Result<(), DocError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(error(
                path,
                format!("the document has more than {MAX_NODES} nodes"),
            ));
        }
        Ok(())
    }

    fn text(value: &Value, path: &str, multiline: bool) -> Result<String, DocError> {
        let text = match value {
            Value::String(text) => text.as_str(),
            other => {
                return Err(error(
                    path,
                    format!("expected a string, found {}", kind(other)),
                ));
            }
        };
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(error(
                path,
                format!("the string is longer than {MAX_TEXT_CHARS} characters"),
            ));
        }
        Ok(clean(text, multiline))
    }

    fn field<'v>(value: &'v Value, path: &str, name: &str) -> Result<&'v Value, DocError> {
        value
            .get(name)
            .ok_or_else(|| error(&join(path, name), "is missing"))
    }

    fn tone(value: &Value, path: &str) -> Result<Tone, DocError> {
        match value.get("tone") {
            None | Some(Value::Null) => Ok(Tone::Default),
            Some(Value::String(name)) => TONES
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, tone)| *tone)
                .ok_or_else(|| {
                    let names: Vec<&str> = TONES.iter().map(|(n, _)| *n).collect();
                    error(
                        &join(path, "tone"),
                        format!("unknown tone {name:?}; valid tones: {}", names.join(", ")),
                    )
                }),
            Some(other) => Err(error(
                &join(path, "tone"),
                format!("expected a string, found {}", kind(other)),
            )),
        }
    }

    fn blocks(&mut self, value: &Value, path: &str, depth: usize) -> Result<Vec<Block>, DocError> {
        items(value, path, "nodes")?
            .into_iter()
            .enumerate()
            .map(|(i, item)| self.block(item, &format!("{path}[{}]", i + 1), depth))
            .collect()
    }

    fn block(&mut self, value: &Value, path: &str, depth: usize) -> Result<Block, DocError> {
        self.count(path)?;
        let Value::Object(_) = value else {
            return Err(error(
                path,
                format!("expected a node (a table with type), found {}", kind(value)),
            ));
        };
        let node_type = match value.get("type") {
            Some(Value::String(t)) => t.as_str(),
            _ => return Err(error(&join(path, "type"), "is missing")),
        };
        match node_type {
            "section" => {
                if depth > MAX_SECTION_DEPTH {
                    return Err(error(
                        path,
                        format!("sections nest at most {MAX_SECTION_DEPTH} levels"),
                    ));
                }
                let title = Self::text(
                    Self::field(value, path, "title")?,
                    &join(path, "title"),
                    false,
                )?;
                let children = match value.get("children") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(children) => self.blocks(children, &join(path, "children"), depth + 1)?,
                };
                let folded = match value.get("folded") {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(b)) => *b,
                    Some(other) => {
                        return Err(error(
                            &join(path, "folded"),
                            format!("expected a boolean, found {}", kind(other)),
                        ));
                    }
                };
                Ok(Block::Section {
                    title,
                    children,
                    folded,
                })
            }
            "field" => {
                let label = Self::text(
                    Self::field(value, path, "label")?,
                    &join(path, "label"),
                    false,
                )?;
                let value_path = join(path, "value");
                let value = self.inlines(Self::field(value, path, "value")?, &value_path)?;
                Ok(Block::Field { label, value })
            }
            "text" => {
                let inlines_path = join(path, "inlines");
                let inlines = self.inlines(Self::field(value, path, "inlines")?, &inlines_path)?;
                Ok(Block::Text { inlines })
            }
            "list" => {
                let items_path = join(path, "items");
                let list = items(Self::field(value, path, "items")?, &items_path, "items")?;
                let items = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| self.inlines(item, &format!("{items_path}[{}]", i + 1)))
                    .collect::<Result<_, _>>()?;
                Ok(Block::List { items })
            }
            "span" | "badge" | "code" | "link" => Err(error(
                path,
                format!("{node_type} is an inline node; put it in a field, a text or a list"),
            )),
            other => Err(error(
                &join(path, "type"),
                format!("unknown node type {other:?}; block nodes: section, field, text, list"),
            )),
        }
    }

    /// A string, one inline node, or a list of them.
    fn inlines(&mut self, value: &Value, path: &str) -> Result<Vec<Inline>, DocError> {
        match value {
            Value::String(_) => Ok(vec![self.inline(value, path)?]),
            Value::Object(map) if map.contains_key("type") => Ok(vec![self.inline(value, path)?]),
            Value::Array(_) | Value::Object(_) => items(value, path, "inline nodes")?
                .into_iter()
                .enumerate()
                .map(|(i, item)| self.inline(item, &format!("{path}[{}]", i + 1)))
                .collect(),
            other => Err(error(
                path,
                format!("expected text or an inline node, found {}", kind(other)),
            )),
        }
    }

    fn inline(&mut self, value: &Value, path: &str) -> Result<Inline, DocError> {
        self.count(path)?;
        if let Value::String(_) = value {
            return Ok(Inline::Span {
                text: Self::text(value, path, true)?,
                tone: Tone::Default,
            });
        }
        let Value::Object(_) = value else {
            return Err(error(
                path,
                format!("expected text or an inline node, found {}", kind(value)),
            ));
        };
        let node_type = match value.get("type") {
            Some(Value::String(t)) => t.as_str(),
            _ => return Err(error(&join(path, "type"), "is missing")),
        };
        let text = |multiline| {
            Self::text(
                Self::field(value, path, "text")?,
                &join(path, "text"),
                multiline,
            )
        };
        match node_type {
            "span" => Ok(Inline::Span {
                text: text(true)?,
                tone: Self::tone(value, path)?,
            }),
            "badge" => Ok(Inline::Badge {
                text: text(false)?,
                tone: Self::tone(value, path)?,
            }),
            "code" => Ok(Inline::Code { text: text(false)? }),
            "link" => Ok(Inline::Link {
                text: text(false)?,
                target: Self::target(Self::field(value, path, "target")?, &join(path, "target"))?,
            }),
            "section" | "field" | "text" | "list" => Err(error(
                path,
                format!("{node_type} is a block node and cannot stand in a line"),
            )),
            other => Err(error(
                &join(path, "type"),
                format!("unknown node type {other:?}; inline nodes: span, badge, code, link"),
            )),
        }
    }

    fn target(value: &Value, path: &str) -> Result<LinkTarget, DocError> {
        let Value::Object(map) = value else {
            return Err(error(
                path,
                format!("expected a table, found {}", kind(value)),
            ));
        };
        if let Some(url) = map.get("url") {
            let url = Self::text(url, &join(path, "url"), false)?;
            let lower = url.to_ascii_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://")) {
                return Err(error(
                    &join(path, "url"),
                    "only https and http addresses are allowed",
                ));
            }
            return Ok(LinkTarget::Url(url));
        }
        if let Some(commit) = map.get("commit") {
            let commit = Self::text(commit, &join(path, "commit"), false)?;
            if commit.is_empty()
                || commit.len() > 256
                || commit.contains(' ')
                || commit.starts_with('-')
            {
                return Err(error(&join(path, "commit"), "expected a hash or a ref"));
            }
            return Ok(LinkTarget::Commit(commit));
        }
        if let Some(file) = map.get("file") {
            let file_path = join(path, "file");
            let file = Self::text(file, &file_path, false)?;
            if file.is_empty()
                || file.starts_with('/')
                || file.starts_with('\\')
                || file.split(['/', '\\']).any(|part| part == "..")
            {
                return Err(error(
                    &file_path,
                    "expected a path relative to the repository's root, without ..",
                ));
            }
            let line = match map.get("line") {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) => match n.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    Some(0) | None => {
                        return Err(error(&join(path, "line"), "expected a line number from 1"));
                    }
                    Some(line) => Some(line),
                },
                Some(other) => {
                    return Err(error(
                        &join(path, "line"),
                        format!("expected a number, found {}", kind(other)),
                    ));
                }
            };
            return Ok(LinkTarget::File { path: file, line });
        }
        Err(error(path, "expected url, commit or file"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: &Value) -> Result<Document, String> {
        Document::parse(value).map_err(|e| e.to_string())
    }

    #[test]
    fn nil_is_an_empty_document() {
        assert!(parse(&Value::Null).unwrap().is_empty());
        assert!(parse(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn one_node_or_a_list_of_nodes() {
        let section = json!({ "type": "section", "title": "PR #4", "children": [
            { "type": "field", "label": "Title", "value": "Keys" },
            { "type": "field", "label": "State", "value": { "type": "badge", "text": "merged", "tone": "ok" } },
            { "type": "list", "items": [ "one", [ "two ", { "type": "code", "text": "4be21c7" } ] ] },
            { "type": "text", "inlines": { "type": "link", "text": "open", "target": { "url": "https://code.example.com/4" } } },
        ]});
        let one = parse(&section).unwrap();
        let many = parse(&json!([section])).unwrap();
        assert_eq!(one, many);
        let Block::Section {
            title,
            children,
            folded,
        } = &one.blocks[0]
        else {
            panic!("a section");
        };
        assert_eq!(title, "PR #4");
        assert!(!folded);
        assert_eq!(
            children[1],
            Block::Field {
                label: "State".into(),
                value: vec![Inline::Badge {
                    text: "merged".into(),
                    tone: Tone::Ok
                }],
            }
        );
        assert_eq!(
            children[2],
            Block::List {
                items: vec![
                    vec![Inline::Span {
                        text: "one".into(),
                        tone: Tone::Default
                    }],
                    vec![
                        Inline::Span {
                            text: "two ".into(),
                            tone: Tone::Default
                        },
                        Inline::Code {
                            text: "4be21c7".into()
                        },
                    ],
                ],
            }
        );
    }

    #[test]
    fn the_message_says_where_and_what() {
        let value = json!([{ "type": "section", "title": "x", "children": [
            { "type": "field", "label": "a", "value": "b" },
            { "type": "field", "label": "a", "value": 4 },
        ]}]);
        assert_eq!(
            parse(&value).unwrap_err(),
            "[1].children[2].value: expected text or an inline node, found a number"
        );
        assert_eq!(
            parse(&json!({ "type": "badge", "text": "x" })).unwrap_err(),
            "[1]: badge is an inline node; put it in a field, a text or a list"
        );
        assert_eq!(
            parse(&json!({ "type": "field", "label": "x", "value": { "type": "span", "text": "y", "tone": "red" } })).unwrap_err(),
            "[1].value.tone: unknown tone \"red\"; valid tones: default, muted, ok, warning, danger, info"
        );
        assert_eq!(
            parse(&json!({ "type": "field", "label": "x" })).unwrap_err(),
            "[1].value: is missing"
        );
        assert_eq!(
            parse(&json!("text")).unwrap_err(),
            "expected a list of nodes, found a string"
        );
    }

    #[test]
    fn links_go_only_where_they_are_allowed() {
        let link = |target: Value| json!({ "type": "text", "inlines": { "type": "link", "text": "l", "target": target } });
        assert!(parse(&link(json!({ "url": "https://x.example" }))).is_ok());
        assert!(parse(&link(json!({ "commit": "HEAD~1" }))).is_ok());
        assert!(parse(&link(json!({ "file": "src/lib.rs", "line": 4 }))).is_ok());
        for bad in [
            json!({ "url": "javascript:alert(1)" }),
            json!({ "url": "file:///etc/passwd" }),
            json!({ "commit": "--output=x" }),
            json!({ "file": "../secret" }),
            json!({ "file": "/etc/passwd" }),
            json!({ "file": "a.rs", "line": 0 }),
            json!({}),
        ] {
            assert!(parse(&link(bad.clone())).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn text_cannot_paint_the_terminal() {
        let doc = parse(
            &json!({ "type": "section", "title": "a\u{1b}[31mred\tx\ny\u{202e}", "children": [
                { "type": "text", "inlines": "line\none\u{7}" },
            ]}),
        )
        .unwrap();
        let Block::Section {
            title, children, ..
        } = &doc.blocks[0]
        else {
            panic!("a section");
        };
        assert_eq!(title, "a[31mred x y");
        assert_eq!(
            children[0],
            Block::Text {
                inlines: vec![Inline::Span {
                    text: "line\none".into(),
                    tone: Tone::Default
                }]
            }
        );
    }

    #[test]
    fn limits_hold() {
        let many: Vec<Value> = (0..=MAX_NODES)
            .map(|_| json!({ "type": "text", "inlines": [] }))
            .collect();
        assert!(
            parse(&Value::Array(many))
                .unwrap_err()
                .contains("more than 200 nodes")
        );

        let mut deep = json!({ "type": "section", "title": "4" });
        for level in (1..=3).rev() {
            deep = json!({ "type": "section", "title": level.to_string(), "children": [deep] });
        }
        assert_eq!(
            parse(&deep).unwrap_err(),
            "[1].children[1].children[1].children[1]: sections nest at most 3 levels"
        );

        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        assert!(
            parse(&json!({ "type": "text", "inlines": long }))
                .unwrap_err()
                .contains("longer than 2000")
        );

        let big: Vec<Value> = (0..40)
            .map(|_| json!({ "type": "text", "inlines": "y".repeat(MAX_TEXT_CHARS) }))
            .collect();
        assert!(
            parse(&Value::Array(big))
                .unwrap_err()
                .contains("bytes, more than")
        );
    }

    #[test]
    fn the_strict_form_round_trips() {
        let doc = parse(&json!({ "type": "field", "label": "a", "value": { "type": "link", "text": "f", "target": { "file": "a.rs" } } })).unwrap();
        let text = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&text).unwrap();
        assert_eq!(doc, back);
    }
}
