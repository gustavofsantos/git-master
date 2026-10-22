//! Built-in workflow templates, in Lua: starting points that "New from template" copies into the
//! user's workflows directory. The copy is the user's to edit; nothing here runs a workflow, and
//! nothing is written into a repository.

use std::path::{Path, PathBuf};

/// One built-in workflow: its name, what it does, and its source.
#[derive(Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

/// Every built-in template, in the order they are offered.
#[must_use]
pub fn all() -> &'static [Template] {
    todo!()
}

/// The template with this name.
#[must_use]
pub fn get(_name: &str) -> Option<&'static Template> {
    todo!()
}

/// A workflow file `create` wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    /// The workflow's `name` inside the file.
    pub name: String,
}

/// Copies `template` into `dir` under the first file name not taken: `<name>.lua`, then
/// `<name>-2.lua`, `<name>-3.lua`… When a copy carries a suffix, its `name` field gets the same
/// suffix so names stay unique. Never overwrites a file.
pub fn create(_dir: &Path, _template: &Template) -> Result<Created, String> {
    todo!()
}
