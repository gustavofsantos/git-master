//! The reference files the app writes next to the user's workflows (design/workflow-scripts.md,
//! D102): `.api/API.md`, the type file for editors, and a short `SKILL.md` for agents. They are
//! generated from the `ctx` surface (which comes from the action catalog), so they cannot drift
//! from the code. A directory whose name starts with a dot is not read as a workflow source.

use std::path::Path;

use super::surface::CtxSurface;

/// The type file's name. Luau's definition-file format, for editors with a Luau language server.
pub const TYPES_FILE: &str = "ctx.d.luau";

/// The files of `.api/`, as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiFiles {
    pub api_md: String,
    pub types: String,
    pub skill_md: String,
}

/// Generates the three files for `surface`. `version` is the app version, shown in the header.
#[must_use]
pub fn generate(_surface: &CtxSurface, _version: &str) -> ApiFiles {
    todo!()
}

/// Writes `<workflows_dir>/.api/` (`API.md`, the type file, `SKILL.md` and `VERSION`) when it is
/// missing or was written by another version or by other content. Returns whether it wrote.
/// Creates the directories. Never touches anything outside `.api/`.
pub fn write_if_stale(_workflows_dir: &Path, _version: &str) -> std::io::Result<bool> {
    todo!()
}
