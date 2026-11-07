//! The MCP server (D135): `gitmaster mcp`, a process of its own that speaks MCP over
//! its standard input and output and drives one engine on one repository. It opens no
//! socket and keeps no state outside the repository's `.git/gitmaster/` files, which the
//! app reads too. The tools are the ones only `GitMaster` has (review comments and
//! threads); the agent has its own for files, search and Git.

mod memory_tools;
mod protocol;
mod session;
mod tools;

pub use protocol::serve;
pub use session::Session;
pub use tools::{ToolDefinition, tools};

use std::io::{BufRead, Write};
use std::path::Path;

use gm_core::git::SubprocessGit;

/// `gitmaster mcp [--repo PATH]` (docs/reference/cli.md): serves MCP on `input` and
/// `output` until `input` ends. Returns the exit status: 0 after a normal end, 1 when the
/// repository cannot be opened or the output fails, 2 for a wrong command line.
///
/// Without `--repo` the repository is the one `cwd` is in. Claude Code does not document the
/// directory a plugin's server starts in, so when `cwd` is not in a repository the
/// `project_dir` it exports (`CLAUDE_PROJECT_DIR`) is tried next.
pub fn command(
    words: &[String],
    cwd: &Path,
    project_dir: Option<&Path>,
    input: impl BufRead,
    output: impl Write,
    errors: &mut impl Write,
) -> i32 {
    let candidates: Vec<String> = match words {
        [] => std::iter::once(cwd)
            .chain(project_dir)
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect(),
        [flag, path] if flag == "--repo" => vec![
            std::path::absolute(path)
                .unwrap_or_else(|_| path.into())
                .to_string_lossy()
                .into_owned(),
        ],
        _ => {
            let _ = writeln!(errors, "usage: gitmaster mcp [--repo <path>]");
            return 2;
        }
    };
    let mut failure = None;
    let mut opened = None;
    for repo in &candidates {
        let engine = gm_core::Engine::new(Box::new(SubprocessGit::default()));
        match Session::open(engine, repo) {
            Ok(session) => {
                opened = Some(session);
                break;
            }
            Err(reason) => {
                failure.get_or_insert_with(|| format!("cannot open {repo}: {reason}"));
            }
        }
    }
    let Some(mut session) = opened else {
        let _ = writeln!(
            errors,
            "gitmaster mcp: {}",
            failure.unwrap_or_else(|| "no repository".to_owned())
        );
        return 1;
    };
    match serve(&mut session, input, output) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(errors, "gitmaster mcp: {e}");
            1
        }
    }
}
