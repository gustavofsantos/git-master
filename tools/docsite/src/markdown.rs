//! Markdown to HTML, and the facts the checks and the search index need from a page.
//!
//! `analyze` and `render` walk the same events, so the heading ids the checks verify are the
//! ids the pages carry.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};

use crate::highlight::{Highlighter, escape};

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

/// The kinds of callout: a blockquote that starts with a bold label (`> **Note.** ...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callout {
    Note,
    Warning,
    Planned,
}

impl Callout {
    fn from_label(label: &str) -> Option<Self> {
        match label {
            "Note." => Some(Self::Note),
            "Warning." => Some(Self::Warning),
            "Planned." => Some(Self::Planned),
            _ => None,
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Planned => "planned",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Warning => "Warning",
            Self::Planned => "Planned",
        }
    }
}

/// `events[at]` starts a blockquote whose first words are a bold callout label.
fn callout_at(events: &[Event<'_>], at: usize) -> Option<Callout> {
    let [
        Event::Start(Tag::BlockQuote(_)),
        Event::Start(Tag::Paragraph),
        Event::Start(Tag::Strong),
        Event::Text(label),
        Event::End(TagEnd::Strong),
    ] = events.get(at..at + 5)?
    else {
        return None;
    };
    Callout::from_label(label)
}

/// Turns heading text into an id: lowercase words joined by `-`, unique within a page.
#[derive(Default)]
pub struct Slugger {
    seen: HashMap<String, usize>,
}

impl Slugger {
    pub fn slug(&mut self, text: &str) -> String {
        let mut base = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() {
                base.extend(c.to_lowercase());
            } else if (c == ' ' || c == '-' || c == '_') && !base.ends_with('-') {
                base.push('-');
            }
        }
        let base = base.trim_matches('-');
        let base = if base.is_empty() { "section" } else { base };
        let count = self.seen.entry(base.to_owned()).or_insert(0);
        let id = if *count == 0 {
            base.to_owned()
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        id
    }
}

#[derive(Clone, Debug)]
pub struct Heading {
    pub id: String,
    pub text: String,
}

/// A `> **Planned.** ...` marker and where it sits.
#[derive(Clone, Debug)]
pub struct PlannedItem {
    pub heading_id: Option<String>,
    pub heading_text: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct LinkRef {
    pub dest: String,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub headings: Vec<Heading>,
    pub links: Vec<LinkRef>,
    pub planned: Vec<PlannedItem>,
    pub code_without_language: Vec<usize>,
    pub h1_lines: Vec<usize>,
    /// Lines of headings that skip a level, with the message.
    pub level_jumps: Vec<(usize, String)>,
    pub first_block_is_planned: bool,
    /// Words that docs/README.md forbids (history, decision numbers), with the line.
    pub style_hits: Vec<(usize, String)>,
    /// The page as plain text, for search.
    pub text: String,
}

const HISTORY_PHRASES: [&str; 4] = ["currently", "for now", "in a future", "at the moment"];

/// Whether a word looks like a decision, question or slice number (`D75`, `Q7`, `WF3`).
fn is_internal_id(word: &str) -> bool {
    let digits = |rest: &str, min: usize, max: usize| {
        (min..=max).contains(&rest.len()) && rest.chars().all(|c| c.is_ascii_digit())
    };
    word.strip_prefix("WF")
        .or_else(|| word.strip_prefix("WS"))
        .is_some_and(|rest| digits(rest, 1, 1))
        || word
            .strip_prefix('D')
            .is_some_and(|rest| digits(rest, 2, 2))
        || word
            .strip_prefix('Q')
            .is_some_and(|rest| digits(rest, 1, 2))
}

fn style_hits(text: &str, line: usize, hits: &mut Vec<(usize, String)>) {
    let lower = text.to_lowercase();
    for phrase in HISTORY_PHRASES {
        if lower.contains(phrase) {
            hits.push((
                line,
                format!("avoid \"{phrase}\": the docs describe the product as it is at launch"),
            ));
        }
    }
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if is_internal_id(word) {
            hits.push((
                line,
                format!("\"{word}\" looks like an internal decision or slice number"),
            ));
        }
    }
}

/// Reads a page: headings, links, markers, code blocks, text. Lines are relative to `body`.
pub fn analyze(body: &str) -> Analysis {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(body.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset);
    let (events, ranges): (Vec<Event<'_>>, Vec<Range<usize>>) =
        Parser::new_ext(body, options()).into_offset_iter().unzip();

    let mut out = Analysis::default();
    let mut slugger = Slugger::default();
    let mut last_level = 1;
    let mut depth = 0usize;
    let mut seen_first_block = false;
    let mut in_code = false;
    let mut i = 0;
    while i < events.len() {
        let line = line_of(ranges[i].start);
        match &events[i] {
            Event::Start(tag) => {
                if depth == 0 && !seen_first_block {
                    seen_first_block = true;
                    out.first_block_is_planned = callout_at(&events, i) == Some(Callout::Planned);
                }
                depth += 1;
                match tag {
                    Tag::Heading { level, .. } => {
                        let level = *level as usize;
                        let text = inline_text(&events, i + 1);
                        let id = slugger.slug(&text);
                        if level == 1 {
                            out.h1_lines.push(line);
                        } else if level > last_level + 1 {
                            out.level_jumps.push((
                                line,
                                format!("heading level jumps from H{last_level} to H{level}"),
                            ));
                        }
                        last_level = level;
                        out.headings.push(Heading { id, text });
                    }
                    Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                        out.links.push(LinkRef {
                            dest: dest_url.to_string(),
                            line,
                        });
                    }
                    Tag::CodeBlock(kind) => {
                        in_code = true;
                        let has_language =
                            matches!(kind, CodeBlockKind::Fenced(info) if !info.trim().is_empty());
                        if !has_language {
                            out.code_without_language.push(line);
                        }
                    }
                    Tag::BlockQuote(_) if callout_at(&events, i) == Some(Callout::Planned) => {
                        let (heading_id, heading_text) = out
                            .headings
                            .last()
                            .map_or((None, None), |h| (Some(h.id.clone()), Some(h.text.clone())));
                        out.planned.push(PlannedItem {
                            heading_id,
                            heading_text,
                            text: callout_body(&events, i + 5),
                        });
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                if matches!(tag, TagEnd::CodeBlock) {
                    in_code = false;
                }
                out.text.push(' ');
            }
            Event::Text(text) | Event::Code(text) => {
                out.text.push_str(text);
                if !in_code && matches!(&events[i], Event::Text(_)) {
                    style_hits(text, line, &mut out.style_hits);
                }
            }
            Event::SoftBreak | Event::HardBreak => out.text.push(' '),
            _ => {}
        }
        i += 1;
    }
    out.text = out.text.split_whitespace().collect::<Vec<_>>().join(" ");
    out
}

/// The plain text of the inline events from `from` up to the end of the block they are in.
fn inline_text(events: &[Event<'_>], from: usize) -> String {
    let mut text = String::new();
    for event in &events[from..] {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::End(TagEnd::Heading(_) | TagEnd::Paragraph) => break,
            _ => {}
        }
    }
    text.trim().to_owned()
}

fn callout_body(events: &[Event<'_>], from: usize) -> String {
    inline_text(events, from)
}

#[derive(Clone, Debug)]
pub struct TocItem {
    pub level: usize,
    pub id: String,
    pub text: String,
}

#[derive(Debug)]
pub struct Rendered {
    pub html: String,
    pub toc: Vec<TocItem>,
}

/// Rewrites a link to a Markdown page so that it points at the page the site writes.
pub fn rewrite_href(dest: &str) -> String {
    if is_external(dest) || dest.starts_with('#') {
        return dest.to_owned();
    }
    let (path, fragment) = dest
        .split_once('#')
        .map_or((dest, None), |(p, f)| (p, Some(f)));
    let Some(stem) = path.strip_suffix(".md") else {
        return dest.to_owned();
    };
    match fragment {
        Some(fragment) => format!("{stem}.html#{fragment}"),
        None => format!("{stem}.html"),
    }
}

pub fn is_external(dest: &str) -> bool {
    dest.contains("://") || dest.starts_with("mailto:")
}

/// Renders a page body to HTML.
pub fn render(body: &str, highlighter: &Highlighter) -> Rendered {
    let events: Vec<Event<'_>> = Parser::new_ext(body, options()).collect();
    let mut out: Vec<Event<'_>> = Vec::with_capacity(events.len());
    let mut toc = Vec::new();
    let mut slugger = Slugger::default();
    let mut quotes: Vec<Option<Callout>> = Vec::new();
    let mut trim_next_text = false;
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::Heading { level, .. }) => {
                let (event, item, end) = render_heading(&events, i, *level as usize, &mut slugger);
                out.push(event);
                toc.extend(item);
                i = end + 1;
                continue;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let (event, end) = render_code_block(&events, i, kind, highlighter);
                out.push(event);
                i = end + 1;
                continue;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let callout = callout_at(&events, i);
                quotes.push(callout);
                if let Some(callout) = callout {
                    out.push(raw(format!(
                        "<aside class=\"callout callout-{}\" role=\"note\">",
                        callout.class()
                    )));
                    out.push(events[i + 1].clone());
                    out.push(raw(format!(
                        "<strong class=\"callout-label\">{}</strong> ",
                        callout.title()
                    )));
                    trim_next_text = true;
                    i += 5;
                    continue;
                }
                out.push(events[i].clone());
            }
            Event::End(TagEnd::BlockQuote(_)) => match quotes.pop().flatten() {
                Some(_) => out.push(raw("</aside>".to_owned())),
                None => out.push(events[i].clone()),
            },
            Event::Start(Tag::Table(_)) => {
                out.push(raw("<div class=\"table-wrap\">".to_owned()));
                out.push(events[i].clone());
            }
            Event::End(TagEnd::Table) => {
                out.push(events[i].clone());
                out.push(raw("</div>".to_owned()));
            }
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                let href = rewrite_href(dest_url);
                let title = if title.is_empty() {
                    String::new()
                } else {
                    format!(" title=\"{}\"", escape(title))
                };
                let external = if is_external(dest_url) {
                    " rel=\"noopener\" class=\"external\""
                } else {
                    ""
                };
                out.push(raw(format!(
                    "<a href=\"{}\"{title}{external}>",
                    escape(&href)
                )));
            }
            Event::Text(text) if trim_next_text => {
                trim_next_text = false;
                out.push(Event::Text(CowStr::from(text.trim_start().to_owned())));
            }
            other => out.push(other.clone()),
        }
        i += 1;
    }
    let mut html = String::new();
    html::push_html(&mut html, out.into_iter());
    Rendered { html, toc }
}

/// A heading with its permalink. Also returns its table-of-contents entry (levels 2 and 3) and
/// the index of its closing event.
fn render_heading<'a>(
    events: &[Event<'a>],
    at: usize,
    level: usize,
    slugger: &mut Slugger,
) -> (Event<'a>, Option<TocItem>, usize) {
    let end = events[at..]
        .iter()
        .position(|e| matches!(e, Event::End(TagEnd::Heading(_))))
        .map_or(events.len(), |found| at + found);
    let text = inline_text(events, at + 1);
    let id = slugger.slug(&text);
    let mut inner = String::new();
    html::push_html(&mut inner, events[at + 1..end].iter().cloned());
    let id_attr = escape(&id);
    let event = raw(format!(
        "<h{level} id=\"{id_attr}\">{inner}<a class=\"anchor\" href=\"#{id_attr}\" aria-label=\"Link to this section\">#</a></h{level}>"
    ));
    let item = (level == 2 || level == 3).then_some(TocItem { level, id, text });
    (event, item, end)
}

/// A fenced code block, highlighted. Also returns the index of its closing event.
fn render_code_block<'a>(
    events: &[Event<'a>],
    at: usize,
    kind: &CodeBlockKind<'_>,
    highlighter: &Highlighter,
) -> (Event<'a>, usize) {
    let end = events[at..]
        .iter()
        .position(|e| matches!(e, Event::End(TagEnd::CodeBlock)))
        .map_or(events.len(), |found| at + found);
    let lang = match kind {
        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
        CodeBlockKind::Indented => String::new(),
    };
    let code: String = events[at + 1..end]
        .iter()
        .filter_map(|e| {
            if let Event::Text(t) = e {
                Some(t.as_ref())
            } else {
                None
            }
        })
        .collect();
    let shown = if lang.is_empty() { "text" } else { &lang };
    let event = raw(format!(
        "<div class=\"codeblock\" data-lang=\"{}\"><pre><code class=\"language-{}\">{}</code></pre></div>",
        escape(shown),
        escape(shown),
        highlighter.highlight(&lang, &code).trim_end_matches('\n'),
    ));
    (event, end)
}

fn raw<'a>(html: String) -> Event<'a> {
    Event::Html(CowStr::from(html))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_text(md: &str) -> String {
        render(md, &Highlighter::new()).html
    }

    #[test]
    fn headings_get_unique_ids_and_a_toc() {
        let rendered = render(
            "## Alpha beta\n\n### Sub\n\n## Alpha beta\n",
            &Highlighter::new(),
        );
        assert!(
            rendered
                .html
                .contains("<h2 id=\"alpha-beta\">Alpha beta<a class=\"anchor\"")
        );
        assert!(rendered.html.contains("id=\"alpha-beta-1\""));
        assert_eq!(
            rendered.toc.iter().map(|t| t.level).collect::<Vec<_>>(),
            [2, 3, 2]
        );
    }

    #[test]
    fn analysis_and_render_agree_on_ids() {
        let md = "## One\n\n## One\n\n## `code` here!\n";
        let ids: Vec<String> = analyze(md).headings.into_iter().map(|h| h.id).collect();
        let rendered = render(md, &Highlighter::new());
        for id in &ids {
            assert!(rendered.html.contains(&format!("id=\"{id}\"")), "{id}");
        }
        assert_eq!(ids, ["one", "one-1", "code-here"]);
    }

    #[test]
    fn markdown_links_become_html_links() {
        assert_eq!(rewrite_href("stacks.md"), "stacks.html");
        assert_eq!(
            rewrite_href("../guide/stacks.md#restack"),
            "../guide/stacks.html#restack"
        );
        assert_eq!(rewrite_href("#local"), "#local");
        assert_eq!(
            rewrite_href("https://example.com/a.md"),
            "https://example.com/a.md"
        );
        let html = render_text("[x](a.md) and [y](https://example.com)");
        assert!(html.contains("<a href=\"a.html\">x</a>"), "{html}");
        assert!(
            html.contains("rel=\"noopener\" class=\"external\""),
            "{html}"
        );
    }

    #[test]
    fn callouts_become_asides_and_are_found() {
        let md =
            "> **Planned.** Not available yet.\n\nText.\n\n> **Note.** Careful.\n\n> plain quote\n";
        let html = render_text(md);
        assert!(html.contains("<aside class=\"callout callout-planned\" role=\"note\">\n<p><strong class=\"callout-label\">Planned</strong> Not available yet."), "{html}");
        assert!(html.contains("callout-note"), "{html}");
        assert!(html.contains("<blockquote>"), "{html}");
        let analysis = analyze(md);
        assert!(analysis.first_block_is_planned);
        assert_eq!(analysis.planned.len(), 1);
        assert_eq!(analysis.planned[0].text, "Not available yet.");
    }

    #[test]
    fn planned_items_remember_their_heading() {
        let analysis = analyze("## Restack\n\n> **Planned.** Missing.\n");
        assert_eq!(analysis.planned[0].heading_id.as_deref(), Some("restack"));
    }

    #[test]
    fn code_blocks_are_wrapped_and_flagged_without_a_language() {
        let md = "```lua\nlocal x = 1\n```\n\n```\nplain\n```\n";
        let html = render_text(md);
        assert!(
            html.contains("<div class=\"codeblock\" data-lang=\"lua\">"),
            "{html}"
        );
        assert!(html.contains("data-lang=\"text\""), "{html}");
        assert_eq!(analyze(md).code_without_language, [5]);
    }

    #[test]
    fn tables_are_wrapped_for_scrolling() {
        let html = render_text("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(
            html.starts_with("<div class=\"table-wrap\"><table>"),
            "{html}"
        );
        assert!(
            html.trim_end().ends_with("</table>\n</div>") || html.trim_end().ends_with("</div>"),
            "{html}"
        );
    }

    #[test]
    fn finds_h1_heading_jumps_links_and_style_problems() {
        let md = "# Title\n\n### Skipped\n\nSee [a](b.md) and ![i](p.png). It currently works, as in D75.\n";
        let analysis = analyze(md);
        assert_eq!(analysis.h1_lines, [1]);
        assert_eq!(analysis.level_jumps.len(), 1);
        assert_eq!(
            analysis
                .links
                .iter()
                .map(|l| l.dest.as_str())
                .collect::<Vec<_>>(),
            ["b.md", "p.png"]
        );
        assert_eq!(analysis.style_hits.len(), 2);
        assert!(
            analysis.text.starts_with("Title Skipped See a and"),
            "{}",
            analysis.text
        );
        assert!(
            analysis.text.contains("It currently works, as in D75."),
            "{}",
            analysis.text
        );
    }
}
