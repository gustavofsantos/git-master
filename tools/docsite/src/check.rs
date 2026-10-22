//! The authoring rules of `docs/README.md`, checked. Errors fail the build; warnings are advice.

use std::collections::HashSet;
use std::path::Path;

use crate::markdown::{self, Analysis};
use crate::model::{Diagnostics, Docs, Page, Status};

/// Checks every page and returns the analysis of each, in the order of `docs.pages`.
pub fn check(docs: &Docs, diags: &mut Diagnostics) -> Vec<Analysis> {
    let analyses: Vec<Analysis> = docs
        .pages
        .iter()
        .map(|page| markdown::analyze(&page.body))
        .collect();
    let mut checker = Checker {
        docs,
        analyses: &analyses,
        diags,
    };
    for (page, analysis) in docs.pages.iter().zip(&analyses) {
        checker.page(page, analysis);
    }
    checker.sections();
    analyses
}

struct Checker<'a> {
    docs: &'a Docs,
    analyses: &'a [Analysis],
    diags: &'a mut Diagnostics,
}

impl Checker<'_> {
    fn page(&mut self, page: &Page, analysis: &Analysis) {
        let line = |relative: usize| Some(page.body_start_line + relative - 1);
        let file = page.rel.as_str();

        if !page.dir().is_empty() && self.docs.section_of(page).is_none() {
            self.diags.error(
                file,
                None,
                format!("directory `{}` is not listed in sections.toml", page.dir()),
            );
        }
        if page.dir().is_empty() && page.rel != "index.md" {
            self.diags.error(
                file,
                None,
                "only index.md may sit at the top of docs/; put other pages in a section directory",
            );
        }

        let planned = analysis.planned.len();
        match page.status {
            Status::Implemented if planned > 0 => self.diags.error(
                file,
                None,
                format!("status is implemented but the page has {planned} Planned marker(s)"),
            ),
            Status::Partial if planned == 0 => {
                self.diags.error(
                    file,
                    None,
                    "status is partial but the page has no `> **Planned.**` marker",
                );
            }
            Status::Planned if planned == 0 => {
                self.diags.error(
                    file,
                    None,
                    "status is planned but the page has no `> **Planned.**` marker",
                );
            }
            Status::Planned if !analysis.first_block_is_planned => self.diags.error(
                file,
                None,
                "a planned page starts with a `> **Planned.**` marker right after the front matter",
            ),
            _ => {}
        }

        for &at in &analysis.h1_lines {
            self.diags.error(
                file,
                line(at),
                "the body has no H1; the title comes from the front matter",
            );
        }
        for (at, message) in &analysis.level_jumps {
            self.diags.error(file, line(*at), message.clone());
        }
        for &at in &analysis.code_without_language {
            self.diags.error(
                file,
                line(at),
                "code block without a language tag (use `text` for plain output)",
            );
        }
        for (at, message) in &analysis.style_hits {
            self.diags.warn(file, line(*at), message.clone());
        }
        for link in &analysis.links {
            if let Some(message) = self.link_problem(page, analysis, &link.dest) {
                self.diags.error(file, line(link.line), message);
            }
        }
    }

    /// Why a link does not resolve, or `None` when it does.
    fn link_problem(&self, page: &Page, analysis: &Analysis, dest: &str) -> Option<String> {
        if markdown::is_external(dest) {
            return None;
        }
        if dest.starts_with('/') {
            return Some(format!(
                "`{dest}` is an absolute path; use a relative link so the site works under any URL"
            ));
        }
        let (path, fragment) = dest
            .split_once('#')
            .map_or((dest, None), |(p, f)| (p, Some(f)));
        if path.is_empty() {
            return fragment.and_then(|f| missing_anchor(analysis, f, &page.rel));
        }
        let Some(target) = resolve(page.dir(), path) else {
            return Some(format!("`{dest}` points outside docs/"));
        };
        if !Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            return if self.docs.root.join(&target).is_file() {
                None
            } else {
                Some(format!(
                    "`{dest}` does not exist (internal links point at .md pages or files under docs/)"
                ))
            };
        }
        let Some(index) = self.docs.pages.iter().position(|p| p.rel == target) else {
            return Some(format!(
                "`{dest}` does not match a page (looked for docs/{target})"
            ));
        };
        fragment.and_then(|f| missing_anchor(&self.analyses[index], f, &target))
    }

    fn sections(&mut self) {
        for section in &self.docs.sections {
            if !self.docs.pages.iter().any(|page| page.dir() == section.dir) {
                self.diags.warn(
                    "sections.toml",
                    None,
                    format!("section `{}` has no pages", section.dir),
                );
            }
        }
        let mut seen = HashSet::new();
        for page in &self.docs.pages {
            if !seen.insert((page.dir().to_owned(), page.order)) {
                self.diags.warn(
                    &page.rel,
                    None,
                    format!(
                        "`order: {}` is used by another page in the same section",
                        page.order
                    ),
                );
            }
        }
    }
}

fn missing_anchor(analysis: &Analysis, fragment: &str, page: &str) -> Option<String> {
    if analysis.headings.iter().any(|h| h.id == fragment) {
        None
    } else {
        Some(format!("`#{fragment}` is not a heading of {page}"))
    }
}

/// Resolves `link` (relative to the directory `from_dir` under docs/) to a path under docs/.
fn resolve(from_dir: &str, link: &str) -> Option<String> {
    let mut parts: Vec<&str> = from_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in link.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Section;
    use std::path::PathBuf;

    fn page(rel: &str, status: Status, body: &str) -> Page {
        Page {
            rel: rel.to_owned(),
            title: "T".to_owned(),
            description: "d".to_owned(),
            status,
            order: 1,
            body: body.to_owned(),
            body_start_line: 7,
        }
    }

    fn docs(pages: Vec<Page>) -> Docs {
        let sections = vec![Section {
            dir: "guide".to_owned(),
            title: "Guide".to_owned(),
            order: 1,
        }];
        Docs {
            root: PathBuf::from("/nonexistent"),
            sections,
            pages,
        }
    }

    fn run(pages: Vec<Page>) -> Vec<String> {
        let mut diags = Diagnostics::default();
        check(&docs(pages), &mut diags);
        diags.0.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_clean_site_has_no_findings() {
        let findings = run(vec![
            page(
                "index.md",
                Status::Implemented,
                "Hello. See [guide](guide/a.md#top).\n",
            ),
            page(
                "guide/a.md",
                Status::Implemented,
                "## Top\n\nBack to [home](../index.md).\n",
            ),
        ]);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn broken_links_and_anchors_are_errors_with_the_file_line() {
        let findings = run(vec![
            page(
                "index.md",
                Status::Implemented,
                "Intro.\n\n[gone](guide/missing.md) [bad](guide/a.md#nope) [abs](/x.md) [self](#nope)\n",
            ),
            page("guide/a.md", Status::Implemented, "## Top\n"),
        ]);
        assert_eq!(findings.len(), 4, "{findings:?}");
        assert!(
            findings[0].starts_with("index.md:9: error:"),
            "{findings:?}"
        );
    }

    #[test]
    fn status_must_agree_with_the_planned_markers() {
        let marker = "> **Planned.** Missing.\n";
        let findings = run(vec![
            page("index.md", Status::Implemented, marker),
            page("guide/a.md", Status::Partial, "## A\n"),
            page("guide/b.md", Status::Planned, &format!("Lead.\n\n{marker}")),
            page("guide/c.md", Status::Planned, marker),
        ]);
        let findings: Vec<_> = findings
            .into_iter()
            .filter(|f| f.contains(": error:"))
            .collect();
        assert_eq!(findings.len(), 3, "{findings:?}");
        assert!(findings[0].contains("implemented but the page has 1"));
        assert!(findings[1].contains("status is partial"));
        assert!(findings[2].contains("starts with"));
    }

    #[test]
    fn authoring_rules_are_enforced() {
        let findings = run(vec![
            page(
                "index.md",
                Status::Implemented,
                "# Title\n\n### Skipped\n\n```\nplain\n```\n",
            ),
            page("other/x.md", Status::Implemented, "Text.\n"),
            page("top.md", Status::Implemented, "Text.\n"),
        ]);
        let joined = findings.join("\n");
        assert!(joined.contains("no H1"), "{joined}");
        assert!(joined.contains("jumps from H1 to H3"), "{joined}");
        assert!(joined.contains("without a language"), "{joined}");
        assert!(joined.contains("`other` is not listed"), "{joined}");
        assert!(joined.contains("only index.md"), "{joined}");
    }

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(
            resolve("guide", "../reference/x.md").as_deref(),
            Some("reference/x.md")
        );
        assert_eq!(resolve("", "guide/a.md").as_deref(), Some("guide/a.md"));
        assert_eq!(resolve("guide", "../../x.md"), None);
    }
}
