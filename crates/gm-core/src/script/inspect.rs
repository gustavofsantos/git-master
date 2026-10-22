//! Checks one script without running any of its logic: compiles it in the sandbox, checks that
//! it is a module with `execute`, and reads and validates `M.meta`. The loader (which lists
//! scripts) and `gitmaster workflow check` both use this, so they report the same problems.

use super::meta::{self, Meta};
use super::port::{ScriptDiagnostic, ScriptEngine};

/// The checked `M.meta` of the script, or every problem found, each with the line it points at
/// (0 when no line is known). `chunk` is the name errors cite, such as the file name.
pub fn inspect(
    engine: &dyn ScriptEngine,
    chunk: &str,
    source: &str,
) -> Result<Meta, Vec<ScriptDiagnostic>> {
    // A script that does not compile or load has no meta worth reading.
    engine.check(chunk, source)?;
    let json = engine.read_meta(chunk, source)?;
    meta::parse(&json).map_err(|problems| {
        problems
            .into_iter()
            .map(|p| ScriptDiagnostic {
                line: meta::locate_line(source, &p.field),
                message: p.message,
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::luau::LuauEngine;

    fn module(meta: &str) -> String {
        format!("local M = {{}}\nM.meta = {meta}\nfunction M.execute(ctx, input)\nend\nreturn M\n")
    }

    #[test]
    fn a_good_script_gives_its_meta() {
        let source = module("{ name = \"w\", trigger = { on = \"manual\" } }");
        let meta = inspect(&LuauEngine::new(), "w.lua", &source).unwrap();
        assert_eq!(meta.name, "w");
    }

    #[test]
    fn a_syntax_error_points_at_its_line() {
        let source = "local M = {}\nM.meta = {\nfunction M.execute(ctx, input)\nend\nreturn M\n";
        let problems = inspect(&LuauEngine::new(), "w.lua", source).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].line >= 2, "{problems:?}");
    }

    #[test]
    fn a_missing_execute_is_reported() {
        let source =
            "local M = {}\nM.meta = { name = \"w\", trigger = { on = \"manual\" } }\nreturn M\n";
        let problems = inspect(&LuauEngine::new(), "w.lua", source).unwrap_err();
        assert!(problems[0].message.contains("execute"), "{problems:?}");
    }

    #[test]
    fn meta_problems_carry_the_line_of_the_field() {
        let source = module("{\n  name = \"w\",\n  trigger = { on = \"nope\" },\n}");
        let problems = inspect(&LuauEngine::new(), "w.lua", &source).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].line, 4, "{problems:?}");
        assert!(problems[0].message.contains("unknown trigger `nope`"));
    }

    #[test]
    fn a_script_that_loops_forever_at_the_top_level_is_stopped() {
        let source = "local M = {}\nwhile true do end\nreturn M\n";
        let problems = inspect(&LuauEngine::new(), "w.lua", source).unwrap_err();
        assert!(problems[0].message.contains("budget"), "{problems:?}");
    }
}
