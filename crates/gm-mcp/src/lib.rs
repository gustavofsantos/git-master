//! The MCP server (D135): `gitmaster mcp`, a process of its own that speaks MCP over
//! its standard input and output and drives one engine on one repository. It opens no
//! socket and keeps no state outside the repository's `.git/gitmaster/` files, which the
//! app reads too. The tools are the ones only `GitMaster` has (review comments and
//! threads); the agent has its own for files, search and Git.

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
pub fn command(
    words: &[String],
    cwd: &Path,
    input: impl BufRead,
    output: impl Write,
    errors: &mut impl Write,
) -> i32 {
    let repo = match words {
        [] => cwd.to_string_lossy().into_owned(),
        [flag, path] if flag == "--repo" => std::path::absolute(path)
            .unwrap_or_else(|_| path.into())
            .to_string_lossy()
            .into_owned(),
        _ => {
            let _ = writeln!(errors, "usage: gitmaster mcp [--repo <path>]");
            return 2;
        }
    };
    let engine = gm_core::Engine::new(Box::new(SubprocessGit::default()));
    let mut session = match Session::open(engine, &repo) {
        Ok(session) => session,
        Err(reason) => {
            let _ = writeln!(errors, "gitmaster mcp: cannot open {repo}: {reason}");
            return 1;
        }
    };
    match serve(&mut session, input, output) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(errors, "gitmaster mcp: {e}");
            1
        }
    }
}
