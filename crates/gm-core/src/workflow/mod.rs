//! Workflows (docs/10): a bounded state machine read from a TOML file in the
//! user's config and interpreted by the engine. WF0 is the pure core: the
//! model, the parser, the expression language, the action catalog, the
//! validator and the transition function. No I/O.

pub mod catalog;
pub mod exec;
pub mod expr;
pub mod loader;
pub mod machine;
pub mod model;
pub mod parse;
pub mod store;
pub mod validate;

/// One problem found in a workflow file, with the line it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// 1-based line in the file; 0 when the problem has no single place.
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The workflow never runs.
    Error,
    /// The workflow runs; the page shows the warning.
    Warning,
}

impl Diagnostic {
    #[must_use]
    pub fn error(line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            line,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn warning(line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            line,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "line {}: {kind}: {}", self.line, self.message)
    }
}

/// Parses and validates a file: what the loader and the editor call.
/// `Err` holds every diagnostic when any of them is an error.
pub fn load(source: &str) -> Result<(model::Definition, Vec<Diagnostic>), Vec<Diagnostic>> {
    let definition = parse::parse(source).map_err(|d| vec![d])?;
    let diagnostics = validate::validate(&definition);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics);
    }
    Ok((definition, diagnostics))
}
