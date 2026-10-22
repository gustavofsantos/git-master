//! The Luau adapter of the `ScriptEngine` port (D104).

use serde_json::Value;

use super::port::{CtxHost, RunEnd, RunRequest, ScriptDiagnostic, ScriptEngine};

/// Luau through `mlua`. Stateless: every call builds a new VM.
#[derive(Clone, Copy, Debug, Default)]
pub struct LuauEngine;

impl LuauEngine {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ScriptEngine for LuauEngine {
    fn check(&self, _chunk: &str, _source: &str) -> Result<(), Vec<ScriptDiagnostic>> {
        todo!()
    }

    fn read_meta(&self, _chunk: &str, _source: &str) -> Result<Value, Vec<ScriptDiagnostic>> {
        todo!()
    }

    fn run(&self, _request: &RunRequest<'_>, _host: &mut dyn CtxHost) -> RunEnd {
        todo!()
    }
}
