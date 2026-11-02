//! Writes the static site: one HTML file per page, the stylesheet and script, a search index,
//! and a page that shows how much of the documented product works in the app.
//!
//! Links between pages are relative, so the output works from any URL path, from a CDN bucket
//! or opened from disk.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::highlight::{Highlighter, escape};
use crate::markdown::{self, Analysis, TocItem};
use crate::model::{Docs, Page, Status};
use crate::{load, status};

const TEMPLATE: &str = include_str!("../assets/page.html");
const STYLE: &str = include_str!("../assets/style.css");
const SCRIPT: &str = include_str!("../assets/site.js");
const LOGO_DARK: &str = include_str!("../../../assets/icons/svg/gitmaster-dark.svg");
const LOGO_LIGHT: &str = include_str!("../../../assets/icons/svg/gitmaster-light.svg");
const FAVICON: &str = include_str!("../../../assets/icons/svg/gitmaster-dark-small.svg");

/// Marks a directory as one this tool wrote, so a rebuild may replace it.
const MARKER: &str = ".docsite";
const AVAILABILITY: &str = "availability.html";

pub struct Options {
    pub out: PathBuf,
    /// The URL the site is deployed at, such as `https://docs.example.com`. Only needed for
    /// canonical links and the sitemap.
    pub base_url: Option<String>,
    pub version: String,
}

/// One page of the output, whether it comes from a Markdown file or is generated.
struct SitePage {
    path: String,
    title: String,
    description: String,
    section: String,
    status: Option<Status>,
    preview: Option<String>,
    html: String,
    toc: Vec<TocItem>,
    text: String,
    headings: Vec<(String, String)>,
}

pub fn build(docs: &Docs, analyses: &[Analysis], options: &Options) -> Result<(), String> {
    prepare_out(&options.out)?;
    let highlighter = Highlighter::new();

    let mut pages: Vec<SitePage> = Vec::new();
    for (page, analysis) in docs.pages.iter().zip(analyses) {
        let rendered = markdown::render(&page.body, &highlighter);
        pages.push(SitePage {
            path: page.html_path(),
            title: page.title.clone(),
            description: page.description.clone(),
            section: docs
                .section_of(page)
                .map(|s| s.title.clone())
                .unwrap_or_default(),
            status: Some(page.status),
            preview: page.preview.clone(),
            html: rendered.html,
            toc: rendered.toc,
            text: analysis.text.clone(),
            headings: analysis
                .headings
                .iter()
                .map(|h| (h.id.clone(), h.text.clone()))
                .collect(),
        });
    }
    let linear = pages.len();
    pages.push(availability_page(docs, analyses));

    let nav = Nav::new(docs, &pages);
    for (at, page) in pages.iter().enumerate() {
        let prev = at.checked_sub(1).filter(|_| at < linear).map(|p| &pages[p]);
        let next = (at + 1 < linear).then(|| &pages[at + 1]);
        write(
            &options.out.join(&page.path),
            &layout(page, &nav, prev, next, options),
        )?;
    }

    let missing = SitePage {
        path: "404.html".to_owned(),
        title: "Page not found".to_owned(),
        description: "This page does not exist.".to_owned(),
        section: String::new(),
        status: None,
        preview: None,
        html: "<p>The page you asked for does not exist. Check the address, search the docs, or start from the <a href=\"index.html\">home page</a>.</p>\n".to_owned(),
        toc: Vec::new(),
        text: String::new(),
        headings: Vec::new(),
    };
    write(
        &options.out.join("404.html"),
        &layout(&missing, &nav, None, None, options),
    )?;

    write(&options.out.join("assets/style.css"), STYLE)?;
    write(&options.out.join("assets/site.js"), SCRIPT)?;
    write(&options.out.join("assets/logo-dark.svg"), LOGO_DARK)?;
    write(&options.out.join("assets/logo-light.svg"), LOGO_LIGHT)?;
    write(&options.out.join("assets/favicon.svg"), FAVICON)?;
    write(
        &options.out.join("assets/search-index.js"),
        &search_index(&pages)?,
    )?;
    write(&options.out.join(".nojekyll"), "")?;
    write(&options.out.join(MARKER), "written by tools/docsite\n")?;
    if let Some(base) = &options.base_url {
        write(&options.out.join("sitemap.xml"), &sitemap(base, &pages))?;
        write(
            &options.out.join("robots.txt"),
            &format!(
                "User-agent: *\nAllow: /\nSitemap: {}/sitemap.xml\n",
                base.trim_end_matches('/')
            ),
        )?;
    }
    for rel in load::assets(&docs.root).map_err(|e| e.to_string())? {
        let target = options.out.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::copy(docs.root.join(&rel), &target).map_err(|e| format!("{rel}: {e}"))?;
    }
    Ok(())
}

/// Empties a previous build, refusing to touch a directory this tool did not create.
fn prepare_out(out: &Path) -> Result<(), String> {
    if out.exists() {
        let empty = fs::read_dir(out)
            .map_err(|e| format!("{}: {e}", out.display()))?
            .next()
            .is_none();
        if !empty && !out.join(MARKER).exists() {
            return Err(format!(
                "{} exists and was not written by docsite; choose another --out or empty it",
                out.display()
            ));
        }
        fs::remove_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// The prefix that leads from a page back to the site root: nothing, `../`, `../../`, and so on.
fn root_of(path: &str) -> String {
    "../".repeat(path.matches('/').count())
}

/// Replaces `{{name}}` in `template`. Values are not scanned again, so page content may hold `{{`.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 4096);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let key = &after[..end];
            if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
                out.push_str(value);
            } else {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
            rest = &after[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn layout(
    page: &SitePage,
    nav: &Nav,
    prev: Option<&SitePage>,
    next: Option<&SitePage>,
    options: &Options,
) -> String {
    let root = root_of(&page.path);
    let document_title = if page.path == "index.html" {
        "GitMaster Documentation".to_owned()
    } else {
        format!("{} · GitMaster Docs", escape(&page.title))
    };
    let canonical = options.base_url.as_ref().map_or_else(String::new, |base| {
        format!(
            "<link rel=\"canonical\" href=\"{}/{}\">\n",
            escape(base.trim_end_matches('/')),
            escape(&page.path)
        )
    });
    let crumbs = if page.section.is_empty() {
        String::new()
    } else {
        format!("      <p class=\"crumbs\">{}</p>\n", escape(&page.section))
    };
    let mut badge = match page.status.filter(|_| page.path != "index.html") {
        Some(Status::Partial) => badge_html(
            Status::Partial,
            "Part of this page is not available in the app yet",
        ),
        Some(Status::Planned) => badge_html(Status::Planned, "Not available in the app yet"),
        _ => String::new(),
    };
    let preview = page.preview.as_deref().map(preview_note);
    if preview.is_some() {
        badge.push_str(
            " <span class=\"badge preview\" title=\"Preview feature, hidden in the release build\">Preview</span>",
        );
    }
    fill(
        TEMPLATE,
        &[
            ("document_title", &document_title),
            ("description", &escape(&page.description)),
            ("root", &root),
            ("canonical", &canonical),
            ("nav", &nav.html(&page.path, &root)),
            ("crumbs", &crumbs),
            ("title", &escape(&page.title)),
            ("badge", &badge),
            ("preview", preview.as_deref().unwrap_or_default()),
            ("content", &page.html),
            ("pager", &pager_html(&root, prev, next)),
            ("toc", &toc_html(&page.toc)),
            ("version", &escape(&options.version)),
        ],
    )
}

fn badge_html(status: Status, title: &str) -> String {
    format!(
        " <span class=\"badge badge-{}\" title=\"{}\">{}</span>",
        status.as_str(),
        escape(title),
        status.label()
    )
}

/// The note under the title of a preview page: how to turn the feature on.
fn preview_note(feature: &str) -> String {
    format!(
        "<p class=\"preview-note\">This is a preview feature. It is hidden in the release build; start GitMaster with <code>GITMASTER_PREVIEW={}</code> to turn it on.</p>\n",
        escape(feature)
    )
}

fn pager_html(root: &str, prev: Option<&SitePage>, next: Option<&SitePage>) -> String {
    if prev.is_none() && next.is_none() {
        return String::new();
    }
    let mut out = String::from("    <nav class=\"pager\" aria-label=\"Previous and next page\">\n");
    if let Some(page) = prev {
        let _ = writeln!(
            out,
            "      <a class=\"pager-prev\" href=\"{root}{}\"><span class=\"pager-label\">Previous</span>{}</a>",
            page.path,
            escape(&page.title)
        );
    }
    if let Some(page) = next {
        let _ = writeln!(
            out,
            "      <a class=\"pager-next\" href=\"{root}{}\"><span class=\"pager-label\">Next</span>{}</a>",
            page.path,
            escape(&page.title)
        );
    }
    out.push_str("    </nav>\n");
    out
}

fn toc_html(toc: &[TocItem]) -> String {
    if toc.is_empty() {
        return String::new();
    }
    let mut out = String::from("    <p class=\"toc-title\">On this page</p>\n    <ul>\n");
    for item in toc {
        let _ = writeln!(
            out,
            "      <li class=\"toc-l{}\"><a href=\"#{}\">{}</a></li>",
            item.level,
            escape(&item.id),
            escape(&item.text)
        );
    }
    out.push_str("    </ul>");
    out
}

struct NavItem {
    path: String,
    title: String,
    status: Option<Status>,
    preview: bool,
}

struct NavGroup {
    title: String,
    items: Vec<NavItem>,
}

/// The sidebar: the same on every page, with the current page marked.
struct Nav {
    groups: Vec<NavGroup>,
}

impl Nav {
    fn new(docs: &Docs, pages: &[SitePage]) -> Self {
        let item = |page: &SitePage| NavItem {
            path: page.path.clone(),
            title: page.title.clone(),
            status: page.status,
            preview: page.preview.is_some(),
        };
        let mut groups = vec![NavGroup {
            title: String::new(),
            items: Vec::new(),
        }];
        for (doc, page) in docs.pages.iter().zip(pages) {
            if doc.dir().is_empty() {
                groups[0].items.push(item(page));
                continue;
            }
            let title = docs
                .section_of(doc)
                .map_or_else(|| doc.dir().to_owned(), |s| s.title.clone());
            if let Some(group) = groups.iter_mut().find(|g| g.title == title) {
                group.items.push(item(page));
            } else {
                groups.push(NavGroup {
                    title,
                    items: vec![item(page)],
                });
            }
        }
        if let Some(page) = pages.iter().find(|p| p.path == AVAILABILITY) {
            groups.push(NavGroup {
                title: "Status".to_owned(),
                items: vec![item(page)],
            });
        }
        Self { groups }
    }

    fn html(&self, current: &str, root: &str) -> String {
        let mut out = String::new();
        for group in &self.groups {
            if !group.title.is_empty() {
                let _ = writeln!(
                    out,
                    "    <p class=\"nav-heading\">{}</p>",
                    escape(&group.title)
                );
            }
            out.push_str("    <ul>\n");
            for item in &group.items {
                let aria = if item.path == current {
                    " aria-current=\"page\""
                } else {
                    ""
                };
                let mut marker = match item.status {
                    Some(status @ (Status::Partial | Status::Planned)) => format!(
                        "<span class=\"dot dot-{}\" aria-hidden=\"true\"></span><span class=\"sr-only\">({})</span>",
                        status.as_str(),
                        status.label()
                    ),
                    _ => String::new(),
                };
                if item.preview {
                    marker.push_str(
                        "<span class=\"dot dot-preview\" aria-hidden=\"true\"></span><span class=\"sr-only\">(Preview)</span>",
                    );
                }
                let _ = writeln!(
                    out,
                    "      <li><a class=\"nav-link\" href=\"{root}{}\"{aria}><span>{}</span>{marker}</a></li>",
                    item.path,
                    escape(&item.title)
                );
            }
            out.push_str("    </ul>\n");
        }
        out
    }
}

/// The page that shows which parts of the documented product work in the app today.
fn availability_page(docs: &Docs, analyses: &[Analysis]) -> SitePage {
    let totals = status::totals(docs, analyses);
    let mut html = String::new();
    html.push_str("<p>These pages describe GitMaster as it is at launch. This page shows which parts work in the app today, and which are still being built. A page marked <strong>Planned</strong> or <strong>Partly available</strong> says so at the top, and each missing part carries a <strong>Planned</strong> note.</p>\n");
    let _ = writeln!(
        html,
        "<div class=\"summary\"><div><strong>{}</strong><span>pages fully available</span></div><div><strong>{}</strong><span>partly available</span></div><div><strong>{}</strong><span>planned</span></div><div><strong>{}</strong><span>parts still planned</span></div></div>",
        totals.implemented, totals.partial, totals.planned, totals.markers
    );
    let mut toc = Vec::new();
    let mut headings = Vec::new();
    let mut text =
        String::from("Availability: which parts work in the app today and which are planned. ");
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (at, page) in docs.pages.iter().enumerate() {
        let title = docs
            .section_of(page)
            .map_or_else(|| "Overview".to_owned(), |s| s.title.clone());
        match groups.iter_mut().find(|(t, _)| *t == title) {
            Some((_, list)) => list.push(at),
            None => groups.push((title, vec![at])),
        }
    }
    let mut slugger = markdown::Slugger::default();
    for (title, list) in &groups {
        let id = slugger.slug(title);
        let _ = writeln!(html, "<h2 id=\"{id}\">{}</h2>", escape(title));
        toc.push(TocItem {
            level: 2,
            id: id.clone(),
            text: title.clone(),
        });
        headings.push((id, title.clone()));
        html.push_str("<div class=\"table-wrap\"><table>\n<thead><tr><th>Page</th><th>Status</th><th>Not available yet</th></tr></thead>\n<tbody>\n");
        for &at in list {
            let (entry, found) = (&docs.pages[at], &analyses[at]);
            text.push_str(&entry.title);
            text.push(' ');
            for planned in &found.planned {
                text.push_str(&planned.text);
                text.push(' ');
            }
            let _ = writeln!(
                html,
                "<tr><td><a href=\"{}\">{}</a></td><td><span class=\"badge badge-{}\" style=\"margin-left:0\">{}</span></td><td>{}</td></tr>",
                entry.html_path(),
                escape(&entry.title),
                entry.status.as_str(),
                entry.status.label(),
                missing_list(entry, found)
            );
        }
        html.push_str("</tbody></table></div>\n");
    }
    let previews: Vec<&Page> = docs.pages.iter().filter(|p| p.preview.is_some()).collect();
    if !previews.is_empty() {
        let id = slugger.slug("Preview features");
        let _ = writeln!(html, "<h2 id=\"{id}\">Preview features</h2>");
        html.push_str("<p>Preview features are hidden in the release build. Start GitMaster with the variable shown to turn one on.</p>\n<ul>\n");
        for page in previews {
            let feature = page.preview.as_deref().unwrap_or_default();
            let _ = writeln!(
                html,
                "  <li><a href=\"{}\">{}</a>: <code>GITMASTER_PREVIEW={}</code></li>",
                page.html_path(),
                escape(&page.title),
                escape(feature)
            );
            text.push_str(&page.title);
            text.push(' ');
        }
        html.push_str("</ul>\n");
        toc.push(TocItem {
            level: 2,
            id: id.clone(),
            text: "Preview features".to_owned(),
        });
        headings.push((id, "Preview features".to_owned()));
    }
    SitePage {
        path: AVAILABILITY.to_owned(),
        title: "Availability".to_owned(),
        description: "Which parts of the documented product work in the app today, and which are still planned.".to_owned(),
        section: String::new(),
        status: None,
        preview: None,
        html,
        toc,
        text,
        headings,
    }
}

fn missing_list(page: &Page, analysis: &Analysis) -> String {
    if analysis.planned.is_empty() {
        return "—".to_owned();
    }
    let item = |planned: &markdown::PlannedItem| match (&planned.heading_id, &planned.heading_text)
    {
        (Some(id), Some(text)) => format!(
            "<li><a href=\"{}#{}\">{}</a>: {}</li>",
            page.html_path(),
            escape(id),
            escape(text),
            escape(&planned.text)
        ),
        _ => format!("<li>{}</li>", escape(&planned.text)),
    };
    let shown = 3;
    let mut out = String::from("<ul class=\"missing\">");
    for planned in analysis.planned.iter().take(shown) {
        out.push_str(&item(planned));
    }
    out.push_str("</ul>");
    if analysis.planned.len() > shown {
        let rest = analysis.planned.len() - shown;
        let _ = write!(
            out,
            "<details class=\"missing-more\"><summary>{rest} more</summary><ul class=\"missing\">"
        );
        for planned in analysis.planned.iter().skip(shown) {
            out.push_str(&item(planned));
        }
        out.push_str("</ul></details>");
    }
    out
}

#[derive(Serialize)]
struct IndexHeading<'a> {
    id: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct IndexEntry<'a> {
    url: &'a str,
    title: &'a str,
    section: &'a str,
    description: &'a str,
    headings: Vec<IndexHeading<'a>>,
    text: &'a str,
}

/// The search index as a script, so it also loads when the site is opened from disk.
fn search_index(pages: &[SitePage]) -> Result<String, String> {
    let entries: Vec<IndexEntry<'_>> = pages
        .iter()
        .map(|page| IndexEntry {
            url: &page.path,
            title: &page.title,
            section: &page.section,
            description: &page.description,
            headings: page
                .headings
                .iter()
                .map(|(id, text)| IndexHeading { id, text })
                .collect(),
            text: &page.text,
        })
        .collect();
    let json = serde_json::to_string(&entries).map_err(|e| e.to_string())?;
    Ok(format!("window.GM_SEARCH={json};\n"))
}

fn sitemap(base: &str, pages: &[SitePage]) -> String {
    let base = base.trim_end_matches('/');
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for page in pages {
        let _ = writeln!(
            out,
            "  <url><loc>{}/{}</loc></url>",
            escape(base),
            escape(&page.path)
        );
    }
    out.push_str("</urlset>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_prefix_follows_the_depth() {
        assert_eq!(root_of("index.html"), "");
        assert_eq!(root_of("guide/a.html"), "../");
        assert_eq!(root_of("a/b/c.html"), "../../");
    }

    #[test]
    fn fill_replaces_names_and_does_not_rescan_values() {
        let filled = fill("<{{a}}|{{b}}|{{missing}}>", &[("a", "{{b}}"), ("b", "2")]);
        assert_eq!(filled, "<{{b}}|2|{{missing}}>");
        assert_eq!(fill("no marks", &[]), "no marks");
        assert_eq!(fill("open {{ never closed", &[]), "open {{ never closed");
    }
}
