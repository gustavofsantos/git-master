//! Reads `docs/`: `sections.toml` and every Markdown page.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::frontmatter;
use crate::model::{Diagnostics, Docs, Page, Section};

#[derive(Deserialize)]
struct SectionsFile {
    #[serde(default)]
    section: Vec<SectionEntry>,
}

#[derive(Deserialize)]
struct SectionEntry {
    dir: String,
    title: String,
    order: i64,
}

/// The files of `docs/` that are not pages and not copied to the site.
const NOT_PUBLISHED: [&str; 2] = ["README.md", "sections.toml"];

/// Reads `docs_root`. Problems that do not stop the reading go to `diags`.
pub fn load(docs_root: &Path, diags: &mut Diagnostics) -> Result<Docs, String> {
    let sections = load_sections(docs_root, diags)?;
    let mut files = Vec::new();
    walk(docs_root, docs_root, &mut files)
        .map_err(|e| format!("cannot read {}: {e}", docs_root.display()))?;
    files.sort();
    let mut pages = Vec::new();
    for rel in files.iter().filter(|rel| is_markdown(rel)) {
        if NOT_PUBLISHED.contains(&rel.as_str()) {
            continue;
        }
        let text = fs::read_to_string(docs_root.join(rel))
            .map_err(|e| format!("cannot read {rel}: {e}"))?;
        match frontmatter::split(&text) {
            Ok((front, body, body_start_line)) => pages.push(Page {
                rel: rel.clone(),
                title: front.title,
                description: front.description,
                status: front.status,
                order: front.order,
                preview: front.preview,
                body: body.to_owned(),
                body_start_line,
            }),
            Err(error) => diags.error(rel, Some(error.line), error.message),
        }
    }
    sort_pages(&mut pages, &sections);
    Ok(Docs {
        root: docs_root.to_path_buf(),
        sections,
        pages,
    })
}

fn load_sections(docs_root: &Path, diags: &mut Diagnostics) -> Result<Vec<Section>, String> {
    let path = docs_root.join("sections.toml");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file: SectionsFile =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sections: Vec<Section> = Vec::new();
    for entry in file.section {
        if sections.iter().any(|s| s.dir == entry.dir) {
            diags.error(
                "sections.toml",
                None,
                format!("section `{}` is listed twice", entry.dir),
            );
            continue;
        }
        sections.push(Section {
            dir: entry.dir,
            title: entry.title,
            order: entry.order,
        });
    }
    sections.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.title.cmp(&b.title)));
    Ok(sections)
}

/// Collects every file under `dir` as a path relative to `root`, with forward slashes.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path: PathBuf = entry?.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn is_markdown(rel: &str) -> bool {
    Path::new(rel)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Files of `docs/` that are copied to the site as they are (images and the like).
pub fn assets(docs_root: &Path) -> std::io::Result<Vec<String>> {
    let mut files = Vec::new();
    walk(docs_root, docs_root, &mut files)?;
    files.retain(|rel| !is_markdown(rel) && !NOT_PUBLISHED.contains(&rel.as_str()));
    files.sort();
    Ok(files)
}

fn sort_pages(pages: &mut [Page], sections: &[Section]) {
    let rank = |page: &Page| {
        if page.dir().is_empty() {
            return 0;
        }
        sections
            .iter()
            .position(|s| s.dir == page.dir())
            .map_or(sections.len() + 1, |at| at + 1)
    };
    pages.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(a.order.cmp(&b.order))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.rel.cmp(&b.rel))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn page(title: &str, order: i64) -> String {
        format!(
            "---\ntitle: {title}\ndescription: d\nstatus: implemented\norder: {order}\n---\n\nBody.\n"
        )
    }

    #[test]
    fn orders_the_top_page_first_then_sections_then_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "sections.toml",
            "[[section]]\ndir = \"b\"\ntitle = \"B\"\norder = 20\n[[section]]\ndir = \"a\"\ntitle = \"A\"\norder = 10\n",
        );
        write(root, "README.md", "not a page");
        write(root, "index.md", &page("Home", 0));
        write(root, "b/one.md", &page("One", 1));
        write(root, "a/two.md", &page("Two", 2));
        write(root, "a/one.md", &page("One", 1));
        write(root, "a/shot.png", "png");
        let mut diags = Diagnostics::default();
        let docs = load(root, &mut diags).unwrap();
        let order: Vec<&str> = docs.pages.iter().map(|p| p.rel.as_str()).collect();
        assert_eq!(order, ["index.md", "a/one.md", "a/two.md", "b/one.md"]);
        assert_eq!(diags.errors(), 0);
        assert_eq!(assets(root).unwrap(), ["a/shot.png"]);
    }

    #[test]
    fn reports_a_broken_page_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "sections.toml", "");
        write(root, "index.md", &page("Home", 0));
        write(root, "broken.md", "no front matter\n");
        let mut diags = Diagnostics::default();
        let docs = load(root, &mut diags).unwrap();
        assert_eq!(docs.pages.len(), 1);
        assert_eq!(diags.errors(), 1);
    }
}
