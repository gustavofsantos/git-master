//! `gitmaster [PATH]` opens the window on the repository at PATH, or on the
//! working directory. `gitmaster --version` prints the version. `gitmaster workflow …` runs one
//! of the one-shot workflow commands and exits; it opens no window and keeps no process.
//! `gitmaster mcp` is the MCP server over stdio (D135). `gitmaster thread …` (the hook and the
//! redaction test), `gitmaster harness list`, `gitmaster worktree …` (list, find and create
//! worktrees from the shell) and `gitmaster workspace …` (create, list and remove workspaces, each
//! a folder of worktrees of your clones) are one-shot too (docs/reference/cli.md). To open a folder
//! named `thread`, `harness`, `worktree`, `workspace` or `mcp`, write `./thread`, `./harness`,
//! `./worktree`, `./workspace` or `./mcp`.

mod shell_env;

use std::time::Duration;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;

fn main() {
    // The preview features (D169): each `GITMASTER_ENABLE_<ID>` variable, else the build's default.
    let vars = std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect::<Vec<_>>();
    let (features, warnings) = gm_schema::features::resolve(cfg!(feature = "preview"), vars);
    for warning in &warnings {
        eprintln!("gitmaster: {warning}");
    }
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--version") {
        println!("gitmaster {}", version());
        let enabled: Vec<&str> = features.enabled().map(gm_schema::Feature::id).collect();
        if !enabled.is_empty() {
            println!("preview features: {}", enabled.join(", "));
        }
        return;
    }
    if first.as_deref() == Some("workflow") {
        let words: Vec<String> = args.collect();
        std::process::exit(workflow_command(&words));
    }
    if first.as_deref() == Some("mcp") {
        let words: Vec<String> = args.collect();
        let cwd = std::env::current_dir().unwrap_or_default();
        let code = gm_mcp::command(
            &words,
            &cwd,
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .map(std::path::PathBuf::from)
                .as_deref(),
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            &mut std::io::stderr().lock(),
        );
        std::process::exit(code);
    }
    if let Some(name) = first.as_deref() {
        let words: Vec<String> = args.by_ref().collect();
        if let Some(code) = engine_command(name, &words) {
            std::process::exit(code);
        }
        // Not a subcommand: the first word is the path, and `words` holds what follows it.
        if !words.is_empty() {
            eprintln!("usage: gitmaster [PATH]");
            std::process::exit(2);
        }
    }
    let from_argument = first.is_some();
    let path = match first {
        Some(path) => std::path::absolute(path),
        None => std::env::current_dir(),
    };
    let path = match path {
        Ok(path) => path,
        Err(error) => {
            eprintln!("gitmaster: {error}");
            std::process::exit(1);
        }
    };
    let command_log = CommandLog::default();
    let git = SubprocessGit {
        log: Some(command_log.clone()),
        ..SubprocessGit::default()
    };
    if let Ok(shell) = std::env::var("SHELL") {
        // The window does not wait for the shell; a failure only means git
        // keeps the PATH this process was started with.
        let slot = git.login_path.clone();
        std::thread::spawn(move || {
            if let Ok(path) = shell_env::login_path(&shell, Duration::from_secs(10)) {
                let _ = slot.set(path);
            }
        });
    }
    let login_path = git.login_path.clone();
    let engine = gm_core::spawn_app(Box::new(git));
    // Started before the window dispatches its open, so the watcher sees the repository open.
    let watch = gm_core::watch::spawn(engine.clone(), login_path);
    let start = gm_ui::StartPath {
        path: path.to_string_lossy().into_owned(),
        from_argument,
    };
    gm_ui::run(engine, command_log, watch, start, features);
}

/// Runs the one-shot subcommand `name` that lives in `gm-core` (`thread`, `harness`, `worktree`,
/// `workspace`)
/// and returns its exit status, or `None` when `name` is not one of them.
fn engine_command(name: &str, words: &[String]) -> Option<i32> {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    match name {
        "thread" => Some(gm_core::threads::cli::thread_command(
            words,
            &mut std::io::stdin().lock(),
            &mut out,
            &mut err,
        )),
        "harness" => Some(gm_core::threads::cli::harness_command(
            words, &mut out, &mut err,
        )),
        "worktree" => Some(gm_core::worktree_cli::command(
            words,
            &mut std::io::stdin().lock(),
            &mut out,
            &mut err,
        )),
        "workspace" => Some(gm_core::workspace_cli::command(
            words,
            &mut std::io::stdin().lock(),
            &mut out,
            &mut err,
        )),
        _ => None,
    }
}

/// The version `--version` prints: the release tag the binary was built for, or the package version.
fn version() -> &'static str {
    display_version(option_env!("GITMASTER_VERSION"), env!("CARGO_PKG_VERSION"))
}

/// `tag` without its leading `v`, or `fallback` when there is no tag.
fn display_version<'a>(tag: Option<&'a str>, fallback: &'a str) -> &'a str {
    match tag.map(|tag| tag.strip_prefix('v').unwrap_or(tag)) {
        Some(version) if !version.is_empty() => version,
        _ => fallback,
    }
}

/// `gitmaster workflow check | dry-run | test` (docs/reference/cli.md). Never applies an effect.
fn workflow_command(words: &[String]) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("gitmaster: {error}");
            return 1;
        }
    };
    let result = gm_core::script::cli::run(words, &gm_core::script::luau::LuauEngine::new(), &cwd);
    print!("{}", result.out);
    eprint!("{}", result.err);
    result.code
}

#[cfg(test)]
mod tests {
    use super::display_version;

    #[test]
    fn version_comes_from_the_package() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }

    #[test]
    fn the_leading_v_of_a_tag_is_dropped() {
        assert_eq!(
            display_version(Some("v0.1.0-beta.1"), "0.0.0"),
            "0.1.0-beta.1"
        );
        assert_eq!(display_version(Some("0.2.0"), "0.0.0"), "0.2.0");
    }

    #[test]
    fn without_a_tag_the_package_version_is_used() {
        assert_eq!(display_version(None, "0.3.0"), "0.3.0");
        assert_eq!(display_version(Some("v"), "0.3.0"), "0.3.0");
        assert_eq!(display_version(Some(""), "0.3.0"), "0.3.0");
    }
}
