//! The Markdown of a prompt or a reply, read into blocks with no GPUI in it. The view
//! draws the blocks (`shell_markdown.rs`); this module decides what they are.
//!
//! The reader is `pulldown-cmark` (`CommonMark` with tables, strikethrough and task
//! lists). Raw HTML and images degrade to their text. A `[REDACTED:name]` marker in
//! text becomes a `Redacted` inline, split the way `threads::spans` splits it, so the
//! marker survives the parser's own splitting of brackets. Two caps keep one message
//! from stalling the UI: a text over `MAX_BYTES` is not read as Markdown at all, and a
//! container nested deeper than `MAX_DEPTH` is read as plain text.

use std::iter::Peekable;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gm_core::syntax::{self, Language};
use gm_schema::HighlightSpan;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::threads::{self, Span};

/// A text longer than this is not read as Markdown: it is one plain paragraph.
pub const MAX_BYTES: usize = 200_000;
/// How deep lists, quotes and inline spans may nest. Deeper ones are read as plain text.
pub const MAX_DEPTH: usize = 8;
/// A code block longer than this is not coloured.
const COLOUR_BYTES: usize = 20_000;
/// How long one code block may take to colour. A block that needs more is shown plain.
const COLOUR_BUDGET: Duration = Duration::from_millis(100);

/// A block of a text, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    /// A fenced or indented code block. `spans` are its colours, set by `colour`; a
    /// block of no known language, or not coloured, has none.
    Code {
        lang: Option<String>,
        text: String,
        spans: Vec<HighlightSpan>,
    },
    Quote(Vec<Block>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Item>,
    },
    Rule,
    Table {
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// An item of a list: its blocks, and its check box when it is a task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A piece of a line of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emph(Vec<Inline>),
    Strike(Vec<Inline>),
    Link {
        text: Vec<Inline>,
        url: String,
    },
    SoftBreak,
    HardBreak,
    /// The name inside a `[REDACTED:name]` marker.
    Redacted(String),
}

/// Reads `text` as Markdown. Deterministic and bounded: a text over `MAX_BYTES` is one
/// plain paragraph, and containers nested past `MAX_DEPTH` are plain paragraphs too.
#[must_use]
pub fn parse(text: &str) -> Vec<Block> {
    if text.len() > MAX_BYTES {
        return vec![Block::Paragraph(text_inlines(text))];
    }
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut reader = Reader {
        events: Parser::new_ext(text, options).peekable(),
        block_depth: 0,
        span_depth: 0,
    };
    let mut blocks = Vec::new();
    loop {
        blocks.extend(reader.blocks());
        if reader.events.next().is_none() {
            break;
        }
    }
    blocks
}

/// Colours the code blocks of `blocks` that name a language the build has a grammar for.
pub fn colour(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Code {
                lang: Some(lang),
                text,
                spans,
            } => *spans = highlight(lang, text),
            Block::Code { .. }
            | Block::Paragraph(_)
            | Block::Heading { .. }
            | Block::Rule
            | Block::Table { .. } => {}
            Block::Quote(inner) => colour(inner),
            Block::List { items, .. } => {
                for item in items {
                    colour(&mut item.blocks);
                }
            }
        }
    }
}

/// What the next event starts, for the block and inline loops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// The end of the container being read, or of the text.
    Stop,
    Block,
    Inline,
}

struct Reader<'a> {
    events: Peekable<Parser<'a>>,
    block_depth: usize,
    span_depth: usize,
}

impl Reader<'_> {
    fn class(&mut self) -> Class {
        match self.events.peek() {
            None | Some(Event::End(_)) => Class::Stop,
            Some(Event::Rule) => Class::Block,
            Some(Event::Start(tag)) if starts_block(tag) => Class::Block,
            Some(_) => Class::Inline,
        }
    }

    /// Consumes the end of the container just read, when it is next.
    fn end(&mut self) {
        if matches!(self.events.peek(), Some(Event::End(_))) {
            self.events.next();
        }
    }

    /// The blocks up to the end of the container, which is not consumed.
    fn blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        loop {
            match self.class() {
                Class::Stop => break,
                Class::Inline => {
                    let inlines = self.inlines();
                    push_paragraph(&mut out, inlines);
                }
                Class::Block => self.block(&mut out),
            }
        }
        out
    }

    /// One block, which starts with the next event.
    fn block(&mut self, out: &mut Vec<Block>) {
        let Some(event) = self.events.next() else {
            return;
        };
        match event {
            Event::Rule => out.push(Block::Rule),
            Event::Start(Tag::Paragraph) => {
                let inlines = self.inlines();
                self.end();
                push_paragraph(out, inlines);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let inlines = self.inlines();
                self.end();
                out.push(Block::Heading {
                    level: level as u8,
                    inlines,
                });
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info_language(&info),
                    CodeBlockKind::Indented => None,
                };
                let text = self.raw_text();
                out.push(Block::Code {
                    lang,
                    text,
                    spans: Vec::new(),
                });
            }
            Event::Start(Tag::HtmlBlock) => {
                let text = self.raw_text();
                push_paragraph(out, text_lines(&text));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                if self.block_depth < MAX_DEPTH {
                    self.block_depth += 1;
                    let inner = self.blocks();
                    self.block_depth -= 1;
                    self.end();
                    out.push(Block::Quote(inner));
                } else {
                    let text = self.flat();
                    push_paragraph(out, text_lines(&text));
                }
            }
            Event::Start(Tag::List(start)) => {
                if self.block_depth < MAX_DEPTH {
                    self.block_depth += 1;
                    let items = self.items();
                    self.block_depth -= 1;
                    self.end();
                    out.push(Block::List {
                        ordered: start.is_some(),
                        start: start.unwrap_or(1),
                        items,
                    });
                } else {
                    let text = self.flat();
                    push_paragraph(out, text_lines(&text));
                }
            }
            Event::Start(Tag::Table(_)) => {
                let mut head = Vec::new();
                let mut rows = Vec::new();
                loop {
                    match self.events.peek() {
                        Some(Event::Start(Tag::TableHead)) => {
                            self.events.next();
                            head = self.cells();
                        }
                        Some(Event::Start(Tag::TableRow)) => {
                            self.events.next();
                            rows.push(self.cells());
                        }
                        _ => break,
                    }
                }
                self.end();
                out.push(Block::Table { head, rows });
            }
            // Any other container (a stray item, a footnote, a metadata block) keeps its
            // blocks and drops its wrapper.
            Event::Start(_) => {
                if self.block_depth < MAX_DEPTH {
                    self.block_depth += 1;
                    let inner = self.blocks();
                    self.block_depth -= 1;
                    self.end();
                    out.extend(inner);
                } else {
                    let text = self.flat();
                    push_paragraph(out, text_lines(&text));
                }
            }
            _ => {}
        }
    }

    /// The items of a list, each with its check box and its blocks.
    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while matches!(self.events.peek(), Some(Event::Start(Tag::Item))) {
            self.events.next();
            let checked = match self.events.peek() {
                Some(Event::TaskListMarker(done)) => {
                    let done = *done;
                    self.events.next();
                    Some(done)
                }
                _ => None,
            };
            let blocks = self.blocks();
            self.end();
            items.push(Item { checked, blocks });
        }
        items
    }

    /// The cells of a table's head or row, each with its inlines.
    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while matches!(self.events.peek(), Some(Event::Start(Tag::TableCell))) {
            self.events.next();
            cells.push(self.inlines());
            self.end();
        }
        self.end();
        cells
    }

    /// The inlines up to the end of the block or span, which is not consumed.
    fn inlines(&mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        while self.class() == Class::Inline {
            let Some(event) = self.events.next() else {
                break;
            };
            match event {
                Event::Text(text)
                | Event::Html(text)
                | Event::InlineHtml(text)
                | Event::InlineMath(text)
                | Event::DisplayMath(text) => push_text(&mut out, &text),
                Event::Code(code) => out.push(Inline::Code(code.to_string())),
                Event::FootnoteReference(name) => push_text(&mut out, &format!("[^{name}]")),
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::Start(Tag::Strong) => self.wrapped(&mut out, Inline::Strong),
                Event::Start(Tag::Emphasis) => self.wrapped(&mut out, Inline::Emph),
                Event::Start(Tag::Strikethrough) => self.wrapped(&mut out, Inline::Strike),
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let url = dest_url.to_string();
                    self.wrapped(&mut out, |text| Inline::Link { text, url });
                }
                // An image shows its alt text; super- and subscript show their text.
                Event::Start(_) => {
                    let text = self.flat();
                    out.extend(text_lines(&text));
                }
                _ => {}
            }
        }
        // Text is split at its markers once the parser has merged its pieces.
        out.into_iter()
            .flat_map(|inline| match inline {
                Inline::Text(text) => text_inlines(&text),
                other => vec![other],
            })
            .collect()
    }

    /// A span just opened, read into `wrap` up to its end. Past the depth cap it is text.
    fn wrapped(&mut self, out: &mut Vec<Inline>, wrap: impl FnOnce(Vec<Inline>) -> Inline) {
        if self.span_depth >= MAX_DEPTH {
            let text = self.flat();
            out.extend(text_lines(&text));
            return;
        }
        self.span_depth += 1;
        let inner = self.inlines();
        self.span_depth -= 1;
        self.end();
        out.push(wrap(inner));
    }

    /// The text of a container just opened, read flat up to its end. Past the depth cap
    /// a container has no structure left: its text is one paragraph.
    fn flat(&mut self) -> String {
        let mut text = String::new();
        let mut open = 0_usize;
        for event in self.events.by_ref() {
            match event {
                Event::Start(_) => open += 1,
                Event::End(end) => {
                    if open == 0 {
                        break;
                    }
                    open -= 1;
                    if ends_block(end) {
                        text.push('\n');
                    } else if matches!(end, TagEnd::TableCell) {
                        text.push(' ');
                    }
                }
                Event::Text(part)
                | Event::Code(part)
                | Event::Html(part)
                | Event::InlineHtml(part) => text.push_str(&part),
                Event::SoftBreak => text.push(' '),
                Event::HardBreak | Event::Rule => text.push('\n'),
                _ => {}
            }
        }
        text.trim_end_matches('\n').to_owned()
    }

    /// The text of a code block or raw HTML block, up to its end.
    fn raw_text(&mut self) -> String {
        let mut text = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::End(_) => break,
                Event::Text(part)
                | Event::Code(part)
                | Event::Html(part)
                | Event::InlineHtml(part) => text.push_str(&part),
                _ => {}
            }
        }
        text.trim_end_matches('\n').to_owned()
    }
}

/// Whether a tag is a block: it starts a line of its own, and `inlines` stops before it.
fn starts_block(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::MetadataBlock(_)
    )
}

/// Whether the end of a block tag closes a line of its own.
fn ends_block(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::BlockQuote(_)
            | TagEnd::List(_)
    )
}

fn push_paragraph(out: &mut Vec<Block>, inlines: Vec<Inline>) {
    if !inlines.is_empty() {
        out.push(Block::Paragraph(inlines));
    }
}

/// Adds `text` to the inlines, merged with a text just before it.
fn push_text(out: &mut Vec<Inline>, text: &str) {
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Inline::Text(text.to_owned()));
    }
}

/// `text` as inlines, with its redaction markers split out as `Redacted`.
fn text_inlines(text: &str) -> Vec<Inline> {
    threads::spans(text)
        .into_iter()
        .map(|span| match span {
            Span::Text(text) => Inline::Text(text),
            Span::Redacted(name) => Inline::Redacted(name),
        })
        .collect()
}

/// `text` as inlines with a hard break at each line end.
fn text_lines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push(Inline::HardBreak);
        }
        out.extend(text_inlines(line));
    }
    out
}

/// The language a fenced block names: its first word, without a `,` or a `{`.
fn info_language(info: &str) -> Option<String> {
    info.split(|c: char| c.is_whitespace() || c == ',' || c == '{')
        .find(|word| !word.is_empty())
        .map(str::to_owned)
}

fn highlight(lang: &str, text: &str) -> Vec<HighlightSpan> {
    let Some(language) = language(lang) else {
        return Vec::new();
    };
    if text.len() > COLOUR_BYTES {
        return Vec::new();
    }
    let stop = AtomicBool::new(false);
    syntax::highlight_within(language, text, &stop, COLOUR_BUDGET).unwrap_or_default()
}

/// The grammar of a fence's language name, by the names people write and the extensions
/// `gm_core::syntax` knows.
fn language(lang: &str) -> Option<Language> {
    let lower = lang.to_ascii_lowercase();
    let extension = match lower.as_str() {
        "rust" => "rs",
        "python" => "py",
        "javascript" | "node" => "js",
        "typescript" => "ts",
        "golang" => "go",
        "shell" | "zsh" => "sh",
        "csharp" => "cs",
        "c++" => "cpp",
        "ruby" => "rb",
        "yaml" => "yml",
        "clojure" => "clj",
        other => other,
    };
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    syntax::language_for_path(&format!("snippet.{extension}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The text an inline list reads as, with a marker as it was written.
    fn inline_text(inlines: &[Inline]) -> String {
        let mut text = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(part) | Inline::Code(part) => text.push_str(part),
                Inline::Strong(inner) | Inline::Emph(inner) | Inline::Strike(inner) => {
                    text.push_str(&inline_text(inner));
                }
                Inline::Link { text: inner, .. } => text.push_str(&inline_text(inner)),
                Inline::SoftBreak => text.push(' '),
                Inline::HardBreak => text.push('\n'),
                Inline::Redacted(name) => {
                    text.push_str("[REDACTED:");
                    text.push_str(name);
                    text.push(']');
                }
            }
        }
        text
    }

    /// The text of every block, in order.
    fn block_text(blocks: &[Block]) -> String {
        let mut text = String::new();
        for block in blocks {
            match block {
                Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                    text.push_str(&inline_text(inlines));
                }
                Block::Code { text: code, .. } => text.push_str(code),
                Block::Quote(inner) => text.push_str(&block_text(inner)),
                Block::List { items, .. } => {
                    for item in items {
                        text.push_str(&block_text(&item.blocks));
                    }
                }
                Block::Rule => {}
                Block::Table { head, rows } => {
                    for cell in head.iter().chain(rows.iter().flatten()) {
                        text.push_str(&inline_text(cell));
                    }
                }
            }
        }
        text
    }

    /// The deepest nesting of quotes and lists in `blocks`.
    fn container_depth(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                Block::Quote(inner) => 1 + container_depth(inner),
                Block::List { items, .. } => {
                    1 + items
                        .iter()
                        .map(|item| container_depth(&item.blocks))
                        .max()
                        .unwrap_or(0)
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// The deepest nesting of inline spans in `inlines`.
    fn span_depth(inlines: &[Inline]) -> usize {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Strong(inner) | Inline::Emph(inner) | Inline::Strike(inner) => {
                    1 + span_depth(inner)
                }
                Inline::Link { text, .. } => 1 + span_depth(text),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![Inline::Text(text.to_owned())])
    }

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_owned())
    }

    #[test]
    fn plain_text_is_one_paragraph() {
        assert_eq!(parse("hello world"), vec![para("hello world")]);
    }

    #[test]
    fn empty_text_has_no_blocks() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn soft_and_hard_breaks() {
        assert_eq!(
            parse("a\nb"),
            vec![Block::Paragraph(vec![
                text("a"),
                Inline::SoftBreak,
                text("b")
            ])]
        );
        assert_eq!(
            parse("a  \nb"),
            vec![Block::Paragraph(vec![
                text("a"),
                Inline::HardBreak,
                text("b")
            ])]
        );
    }

    #[test]
    fn emphasis_strike_and_inline_code() {
        assert_eq!(
            parse("**b** *i* ~~s~~ `c`"),
            vec![Block::Paragraph(vec![
                Inline::Strong(vec![text("b")]),
                text(" "),
                Inline::Emph(vec![text("i")]),
                text(" "),
                Inline::Strike(vec![text("s")]),
                text(" "),
                Inline::Code("c".to_owned()),
            ])]
        );
    }

    #[test]
    fn link_keeps_its_text_and_url() {
        assert_eq!(
            parse("[docs](https://example.org/x)"),
            vec![Block::Paragraph(vec![Inline::Link {
                text: vec![text("docs")],
                url: "https://example.org/x".to_owned(),
            }])]
        );
    }

    #[test]
    fn image_degrades_to_its_alt_text() {
        assert_eq!(
            parse("![a cat](cat.png)"),
            vec![Block::Paragraph(vec![text("a cat")])]
        );
    }

    #[test]
    fn raw_inline_html_degrades_to_text() {
        let blocks = parse("a <b>x</b>");
        assert_eq!(block_text(&blocks), "a <b>x</b>");
    }

    #[test]
    fn heading_has_its_level() {
        assert_eq!(
            parse("## Title"),
            vec![Block::Heading {
                level: 2,
                inlines: vec![text("Title")],
            }]
        );
    }

    #[test]
    fn fenced_code_keeps_its_language() {
        assert_eq!(
            parse("```rust\nfn x() {}\n```"),
            vec![Block::Code {
                lang: Some("rust".to_owned()),
                text: "fn x() {}".to_owned(),
                spans: Vec::new(),
            }]
        );
    }

    #[test]
    fn indented_code_has_no_language() {
        assert_eq!(
            parse("    let x = 1;"),
            vec![Block::Code {
                lang: None,
                text: "let x = 1;".to_owned(),
                spans: Vec::new(),
            }]
        );
    }

    #[test]
    fn fence_language_stops_at_a_comma_or_brace() {
        assert_eq!(info_language("rust,ignore"), Some("rust".to_owned()));
        assert_eq!(info_language("  py {x}"), Some("py".to_owned()));
        assert_eq!(info_language(""), None);
    }

    #[test]
    fn quote_holds_its_blocks() {
        assert_eq!(
            parse("> a\n> b"),
            vec![Block::Quote(vec![Block::Paragraph(vec![
                text("a"),
                Inline::SoftBreak,
                text("b"),
            ])])]
        );
    }

    #[test]
    fn nested_lists_nest_inside_their_item() {
        let blocks = parse("- a\n  - b\n- c");
        let Block::List { ordered, items, .. } = &blocks[0] else {
            panic!("a list: {blocks:?}");
        };
        assert!(!ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].blocks,
            vec![
                para("a"),
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![Item {
                        checked: None,
                        blocks: vec![para("b")],
                    }],
                },
            ]
        );
        assert_eq!(items[1].blocks, vec![para("c")]);
    }

    #[test]
    fn ordered_list_keeps_its_start() {
        let blocks = parse("3. x\n4. y");
        assert!(matches!(
            &blocks[0],
            Block::List {
                ordered: true,
                start: 3,
                items,
            } if items.len() == 2
        ));
    }

    #[test]
    fn task_items_carry_their_check_box() {
        let blocks = parse("- [x] done\n- [ ] todo\n- plain");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("a list: {blocks:?}");
        };
        let checks: Vec<Option<bool>> = items.iter().map(|item| item.checked).collect();
        assert_eq!(checks, vec![Some(true), Some(false), None]);
        assert_eq!(items[0].blocks, vec![para("done")]);
    }

    #[test]
    fn table_has_head_and_rows() {
        assert_eq!(
            parse("| a | b |\n|---|---|\n| 1 | 2 |"),
            vec![Block::Table {
                head: vec![vec![text("a")], vec![text("b")]],
                rows: vec![vec![vec![text("1")], vec![text("2")]]],
            }]
        );
    }

    #[test]
    fn thematic_break_is_a_rule() {
        assert_eq!(
            parse("a\n\n---\n\nb"),
            vec![para("a"), Block::Rule, para("b")]
        );
    }

    #[test]
    fn redaction_marker_is_a_redacted_inline() {
        assert_eq!(
            parse("token [REDACTED:github-token] in use"),
            vec![Block::Paragraph(vec![
                text("token "),
                Inline::Redacted("github-token".to_owned()),
                text(" in use"),
            ])]
        );
    }

    #[test]
    fn redaction_marker_inside_bold_is_kept() {
        assert_eq!(
            parse("**a [REDACTED:k] b**"),
            vec![Block::Paragraph(vec![Inline::Strong(vec![
                text("a "),
                Inline::Redacted("k".to_owned()),
                text(" b"),
            ])])]
        );
    }

    #[test]
    fn redaction_marker_split_by_the_parser_is_one_marker() {
        // Unresolved brackets reach the reader as three text events.
        assert_eq!(
            parse("[REDACTED:key]"),
            vec![Block::Paragraph(vec![Inline::Redacted("key".to_owned())])]
        );
    }

    #[test]
    fn code_span_keeps_a_marker_as_written() {
        assert_eq!(
            parse("`[REDACTED:k]`"),
            vec![Block::Paragraph(vec![Inline::Code(
                "[REDACTED:k]".to_owned()
            )])]
        );
    }

    #[test]
    fn unbalanced_emphasis_is_text() {
        assert_eq!(block_text(&parse("*a **b")), "*a **b");
        assert_eq!(block_text(&parse("_x__y")), "_x__y");
    }

    #[test]
    fn deeply_nested_quotes_stop_at_the_cap() {
        let text = format!("{}deep", "> ".repeat(12));
        let blocks = parse(&text);
        assert!(container_depth(&blocks) <= MAX_DEPTH);
        assert!(block_text(&blocks).contains("deep"));
    }

    #[test]
    fn deeply_nested_lists_stop_at_the_cap() {
        let mut text = String::new();
        for depth in 0..12 {
            text.push_str(&"  ".repeat(depth));
            text.push_str("- level");
            text.push_str(&depth.to_string());
            text.push('\n');
        }
        let blocks = parse(&text);
        assert!(container_depth(&blocks) <= MAX_DEPTH);
        assert!(block_text(&blocks).contains("level11"));
    }

    #[test]
    fn deeply_nested_emphasis_stops_at_the_cap() {
        let text = format!("{}x{}", "*".repeat(40), "*".repeat(40));
        let blocks = parse(&text);
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("a paragraph: {blocks:?}");
        };
        assert!(span_depth(inlines) <= MAX_DEPTH);
        assert!(inline_text(inlines).contains('x'));
    }

    #[test]
    fn a_megabyte_of_quote_marks_is_one_plain_paragraph() {
        let text = ">".repeat(1_000_000);
        let blocks = parse(&text);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], Block::Paragraph(inlines) if inline_text(inlines) == text));
    }

    #[test]
    fn nul_and_control_characters_are_kept() {
        let text = "a\0b\u{7}c\u{1b}[0m";
        assert_eq!(block_text(&parse(text)), text);
    }

    #[test]
    fn text_over_the_cap_is_one_plain_paragraph() {
        let text = format!("# not a heading [REDACTED:k]\n{}", "x".repeat(MAX_BYTES));
        let blocks = parse(&text);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], Block::Paragraph(_)));
        assert_eq!(block_text(&blocks), text);
    }

    #[test]
    fn code_is_coloured_only_for_a_known_language() {
        let mut blocks = parse("```rust\nfn main() {}\n```\n\n```nope\nx\n```");
        colour(&mut blocks);
        let spans: Vec<usize> = blocks
            .iter()
            .map(|block| match block {
                Block::Code { spans, .. } => spans.len(),
                _ => 0,
            })
            .collect();
        assert!(spans[0] > 0);
        assert_eq!(spans[1], 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn never_panics_and_is_deterministic(text in "\\PC{0,400}") {
            prop_assert_eq!(parse(&text), parse(&text));
        }

        #[test]
        fn markdown_alphabet_never_panics(
            text in "[*_`~\\[\\]()#>|\\-0-9a-zA-Z \n\t:!]{0,300}"
        ) {
            let blocks = parse(&text);
            prop_assert_eq!(parse(&text), blocks);
        }

        #[test]
        fn plain_fallback_keeps_the_text(extra in 1_usize..64, seed in any::<usize>()) {
            // Built by index, not by a regex: a 200 KB string is slow to generate otherwise.
            const CHARS: &[u8] = b"ab *_[]()#>\n\t\0\x07";
            let length = MAX_BYTES + extra;
            let text: String = (0..length)
                .map(|index| char::from(CHARS[(index.wrapping_mul(7).wrapping_add(seed)) % CHARS.len()]))
                .collect();
            let blocks = parse(&text);
            prop_assert_eq!(blocks.len(), 1);
            prop_assert_eq!(block_text(&blocks), text);
        }
    }
}
