//! An extension's document as rows (design/extensions.md): what both front ends draw. A row has
//! an indent, a kind and its pieces with their tones; sections fold by a key the front end keeps
//! per extension, slot and section title, so a section folded on one commit stays folded on the
//! next.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::hash::BuildHasher;

use gm_schema::ext::{Block, Document, Inline, LinkTarget, Tone};

/// One piece of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text { text: String, tone: Tone },
    Badge { text: String, tone: Tone },
    Code(String),
    Link { text: String, target: LinkTarget },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A section's title. `key` names it for the fold state: the titles from the top, joined.
    Section {
        key: String,
        folded: bool,
    },
    /// A field: the label is drawn in a column as wide as `label_width`.
    Field {
        label: String,
        label_width: usize,
    },
    Text,
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Levels of nesting under sections.
    pub depth: usize,
    pub kind: RowKind,
    pub pieces: Vec<Piece>,
}

/// The rows to draw. `folded` holds the keys of the sections the person folded or unfolded
/// against the document's own `folded`: a key in it flips the section's state.
#[must_use]
pub fn rows<S: BuildHasher>(document: &Document, toggled: &HashSet<String, S>) -> Vec<Row> {
    let mut out = Vec::new();
    push_blocks(&document.blocks, 0, "", toggled, &mut out);
    out
}

fn push_blocks<S: BuildHasher>(
    blocks: &[Block],
    depth: usize,
    parent: &str,
    toggled: &HashSet<String, S>,
    out: &mut Vec<Row>,
) {
    let mut i = 0;
    while i < blocks.len() {
        // Labels of consecutive fields line up.
        if let Block::Field { .. } = &blocks[i] {
            let run_end = blocks[i..]
                .iter()
                .position(|b| !matches!(b, Block::Field { .. }))
                .map_or(blocks.len(), |n| i + n);
            let width = blocks[i..run_end]
                .iter()
                .map(|b| match b {
                    Block::Field { label, .. } => label.chars().count(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0);
            for block in &blocks[i..run_end] {
                if let Block::Field { label, value } = block {
                    out.push(Row {
                        depth,
                        kind: RowKind::Field {
                            label: label.clone(),
                            label_width: width,
                        },
                        pieces: pieces(value),
                    });
                }
            }
            i = run_end;
            continue;
        }
        match &blocks[i] {
            Block::Section {
                title,
                children,
                folded,
            } => {
                let key = if parent.is_empty() {
                    title.clone()
                } else {
                    format!("{parent}/{title}")
                };
                let folded = *folded != toggled.contains(&key);
                out.push(Row {
                    depth,
                    kind: RowKind::Section {
                        key: key.clone(),
                        folded,
                    },
                    pieces: vec![Piece::Text {
                        text: title.clone(),
                        tone: Tone::Default,
                    }],
                });
                if !folded {
                    push_blocks(children, depth + 1, &key, toggled, out);
                }
            }
            Block::Text { inlines } => out.push(Row {
                depth,
                kind: RowKind::Text,
                pieces: pieces(inlines),
            }),
            Block::List { items } => out.extend(items.iter().map(|item| Row {
                depth,
                kind: RowKind::Item,
                pieces: pieces(item),
            })),
            Block::Field { .. } => {}
        }
        i += 1;
    }
}

fn pieces(inlines: &[Inline]) -> Vec<Piece> {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Span { text, tone } => Piece::Text {
                text: text.clone(),
                tone: *tone,
            },
            Inline::Badge { text, tone } => Piece::Badge {
                text: text.clone(),
                tone: *tone,
            },
            Inline::Code { text } => Piece::Code(text.clone()),
            Inline::Link { text, target } => Piece::Link {
                text: text.clone(),
                target: target.clone(),
            },
        })
        .collect()
}

/// The text of a row's pieces, without styling: badges in brackets, links underlined by `<>`
/// in plain text.
#[must_use]
pub fn plain(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text { text, .. } | Piece::Code(text) => out.push_str(text),
            Piece::Badge { text, .. } => {
                let _ = write!(out, "[{text}]");
            }
            Piece::Link { text, target } => {
                let to = match target {
                    LinkTarget::Url(url) => url.clone(),
                    LinkTarget::Commit(rev) => format!("commit {rev}"),
                    LinkTarget::File {
                        path,
                        line: Some(line),
                    } => format!("{path}:{line}"),
                    LinkTarget::File { path, line: None } => path.clone(),
                };
                let _ = write!(out, "{text} <{to}>");
            }
        }
    }
    out
}

/// The document as plain text, as `gitmaster extension render` prints it.
#[must_use]
pub fn to_text(document: &Document) -> String {
    let mut out = String::new();
    for row in rows(document, &HashSet::<String>::new()) {
        let indent = "  ".repeat(row.depth);
        let body = plain(&row.pieces);
        let line = match &row.kind {
            RowKind::Section { folded, .. } => {
                format!("{} {body}", if *folded { "▸" } else { "▾" })
            }
            RowKind::Field { label, label_width } => format!("{label:<label_width$}  {body}"),
            RowKind::Text => body,
            RowKind::Item => format!("• {body}"),
        };
        for (n, part) in line.split('\n').enumerate() {
            let pad = if n == 0 { "" } else { "  " };
            let _ = writeln!(out, "{indent}{pad}{part}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Document {
        Document::parse(&json!([
            { "type": "section", "title": "Pull request #4", "children": [
                { "type": "field", "label": "Title", "value": "Keys" },
                { "type": "field", "label": "State", "value": { "type": "badge", "text": "merged", "tone": "ok" } },
                { "type": "section", "title": "Comments", "folded": true, "children": [
                    { "type": "list", "items": ["one"] },
                ]},
                { "type": "text", "inlines": { "type": "link", "text": "Open", "target": { "url": "https://code.example.com/4" } } },
            ]},
        ]))
        .unwrap()
    }

    #[test]
    fn the_text_form_reads_like_the_companion() {
        assert_eq!(
            to_text(&doc()),
            "▾ Pull request #4\n  Title  Keys\n  State  [merged]\n  ▸ Comments\n  Open <https://code.example.com/4>\n"
        );
    }

    #[test]
    fn a_toggled_key_flips_the_fold() {
        let toggled: HashSet<String> = ["Pull request #4/Comments".to_owned()].into();
        let rows = rows(&doc(), &toggled);
        assert!(rows.iter().any(|r| r.kind == RowKind::Item));
        let toggled: HashSet<String> = ["Pull request #4".to_owned()].into();
        assert_eq!(super::rows(&doc(), &toggled).len(), 1);
    }
}
