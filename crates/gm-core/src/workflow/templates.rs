//! Built-in workflow templates: starting points that `CreateWorkflowFromTemplate`
//! copies into the user's workflows directory. The copy is the user's to edit;
//! nothing in this module runs a workflow.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// One built-in workflow: its name, what it does, and the file it is copied from.
#[derive(Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

static TEMPLATES: &[Template] = &[
    // The name is the workflow's own `name`, so a copy is named like it.
    Template {
        name: "new-workflow",
        description: "An empty workflow to build in the Flow tab",
        source: include_str!("templates/blank.toml"),
    },
    Template {
        name: "suggest-commit-message",
        description: "Suggest a commit message from the staged diff",
        source: include_str!("templates/suggest-commit-message.toml"),
    },
    Template {
        name: "branch-from-ticket",
        description: "Create and check out a branch named from a ticket key",
        source: include_str!("templates/branch-from-ticket.toml"),
    },
    Template {
        name: "commit-staged",
        description: "Commit the staged changes with the message you give",
        source: include_str!("templates/commit-staged.toml"),
    },
];

/// Every built-in template, in the order they are offered.
#[must_use]
pub fn all() -> &'static [Template] {
    TEMPLATES
}

/// A workflow file `create` wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    /// The workflow's `name` inside the file.
    pub name: String,
}

/// How many copies of one template a directory may hold before `create` gives up.
const MAX_COPIES: u32 = 1000;

/// Copies `template` into `dir` under the first file name not taken: `<name>.toml`,
/// then `<name>-2.toml`, `<name>-3.toml`… When a copy carries a suffix, its `name`
/// line gets the same suffix so names stay unique. Never overwrites a file.
pub fn create(dir: &Path, template: &Template) -> Result<Created, String> {
    for copy in 1..=MAX_COPIES {
        let name = if copy == 1 {
            template.name.to_owned()
        } else {
            format!("{}-{copy}", template.name)
        };
        let text = if copy == 1 {
            template.source.to_owned()
        } else {
            renamed(template.source, template.name, &name)
        };
        let path = dir.join(format!("{name}.toml"));
        if place(dir, &path, copy, &text)? {
            return Ok(Created { path, name });
        }
    }
    Err(format!(
        "there are already {MAX_COPIES} copies of `{}` in {}",
        template.name,
        dir.display()
    ))
}

/// Writes `text` to a temporary file beside `path`, then hard-links it into place.
/// The link fails when `path` exists, so nothing is ever replaced. `Ok(false)`
/// means the name was taken.
fn place(dir: &Path, path: &Path, copy: u32, text: &str) -> Result<bool, String> {
    let temp = dir.join(format!(".gm-template-{}-{copy}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    let linked = std::fs::hard_link(&temp, path);
    // The temporary name is gone whatever the link said.
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!("could not write {}: {e}", path.display())),
    }
}

/// `source` with its top-level `name = "<from>"` line set to `name = "<to>"`.
fn renamed(source: &str, from: &str, to: &str) -> String {
    let line = format!("name = \"{from}\"\n");
    match source.find(&line) {
        Some(at) => format!(
            "{}name = \"{to}\"\n{}",
            &source[..at],
            &source[at + line.len()..]
        ),
        None => source.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{Severity, load};

    #[test]
    fn every_template_loads_with_no_errors() {
        for template in all() {
            let loaded = load(template.source);
            assert!(
                loaded.is_ok(),
                "template `{}` does not load: {:?}",
                template.name,
                loaded.err()
            );
        }
    }

    #[test]
    fn every_template_loads_without_warnings() {
        for template in all() {
            let (_, diagnostics) = load(template.source).unwrap();
            assert!(
                diagnostics.iter().all(|d| d.severity != Severity::Warning),
                "template `{}` has warnings: {diagnostics:?}",
                template.name
            );
        }
    }

    #[test]
    fn template_names_are_unique() {
        let mut names: Vec<&str> = all().iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all().len());
    }

    #[test]
    fn each_template_names_its_own_workflow_and_runs_on_demand() {
        for template in all() {
            let (definition, _) = load(template.source).unwrap();
            assert_eq!(definition.name, template.name);
            assert_eq!(definition.trigger.on, "manual");
        }
    }

    #[test]
    fn renaming_changes_only_the_name_line() {
        let source = all()
            .iter()
            .find(|t| t.name == "suggest-commit-message")
            .expect("template exists");
        let text = renamed(source.source, source.name, "suggest-commit-message-2");
        let (definition, _) = load(&text).unwrap();
        assert_eq!(definition.name, "suggest-commit-message-2");
        assert_eq!(
            text.replacen("suggest-commit-message-2", "suggest-commit-message", 1),
            source.source
        );
    }

    #[test]
    fn a_missing_name_line_leaves_the_text_as_it_is() {
        assert_eq!(renamed("schema = 1\n", "x", "y"), "schema = 1\n");
    }
}
