//! What the generator knows about the documentation: sections, pages and the problems found.

use std::fmt;
use std::path::PathBuf;

/// How much of a page works in the app today (`docs/README.md`, "Status markers").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Implemented,
    Partial,
    Planned,
}

impl Status {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "implemented" => Some(Self::Implemented),
            "partial" => Some(Self::Partial),
            "planned" => Some(Self::Planned),
            _ => None,
        }
    }

    /// The value written in the front matter.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Partial => "partial",
            Self::Planned => "planned",
        }
    }

    /// The words the site shows a reader.
    pub fn label(self) -> &'static str {
        match self {
            Self::Implemented => "Available",
            Self::Partial => "Partly available",
            Self::Planned => "Planned",
        }
    }
}

/// One directory of `docs/`, as listed in `sections.toml`.
#[derive(Clone, Debug)]
pub struct Section {
    pub dir: String,
    pub title: String,
    pub order: i64,
}

/// One Markdown page of `docs/`.
#[derive(Clone, Debug)]
pub struct Page {
    /// Path under `docs/` with forward slashes, such as `guide/branches.md`.
    pub rel: String,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub order: i64,
    /// The Markdown after the front matter.
    pub body: String,
    /// Line of the file where `body` starts (1-based), for diagnostics.
    pub body_start_line: usize,
}

impl Page {
    /// The directory of the page under `docs/`, empty for a page at the top.
    pub fn dir(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    /// The output path under the site root, such as `guide/branches.html`.
    pub fn html_path(&self) -> String {
        match self.rel.strip_suffix(".md") {
            Some(stem) => format!("{stem}.html"),
            None => self.rel.clone(),
        }
    }
}

/// Everything read from `docs/`.
#[derive(Debug)]
pub struct Docs {
    pub root: PathBuf,
    pub sections: Vec<Section>,
    /// Sorted by section order, then page order, then title; the top-level page first.
    pub pages: Vec<Page>,
}

impl Docs {
    pub fn section_of(&self, page: &Page) -> Option<&Section> {
        self.sections
            .iter()
            .find(|section| section.dir == page.dir())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem in one file of `docs/`.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.line {
            Some(line) => write!(f, "{}:{line}: {kind}: {}", self.file, self.message),
            None => write!(f, "{}: {kind}: {}", self.file, self.message),
        }
    }
}

#[derive(Debug, Default)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    pub fn error(&mut self, file: &str, line: Option<usize>, message: impl Into<String>) {
        self.push(Severity::Error, file, line, message.into());
    }

    pub fn warn(&mut self, file: &str, line: Option<usize>, message: impl Into<String>) {
        self.push(Severity::Warning, file, line, message.into());
    }

    fn push(&mut self, severity: Severity, file: &str, line: Option<usize>, message: String) {
        self.0.push(Diagnostic {
            severity,
            file: file.to_owned(),
            line,
            message,
        });
    }

    pub fn errors(&self) -> usize {
        self.0
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.0
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
}
