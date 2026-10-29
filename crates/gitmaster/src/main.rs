//! `gitmaster [PATH]` opens the window on the repository at PATH, or on the
//! working directory. `gitmaster --version` prints the version. `gitmaster workflow …` runs one
//! of the one-shot workflow commands and exits; it opens no window and keeps no process.
//! `gitmaster thread …` (the hook and the redaction test) and `gitmaster harness …` (the agent
//! integration) are one-shot too (docs/reference/cli.md). To open a folder named `thread` or
//! `harness`, write `./thread` or `./harness`.

mod shell_env;

use std::time::Duration;

use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--version") {
        println!("gitmaster {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if first.as_deref() == Some("workflow") {
        let words: Vec<String> = args.collect();
        std::process::exit(workflow_command(&words));
    }
    if first.as_deref() == Some("thread") {
        let words: Vec<String> = args.collect();
        let code = gm_core::threads::cli::thread_command(
            &words,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
            &mut std::io::stderr().lock(),
        );
        std::process::exit(code);
    }
    if first.as_deref() == Some("harness") {
        let words: Vec<String> = args.collect();
        let env = gm_core::threads::cli::Env::from_process();
        let code = gm_core::threads::cli::harness_command(
            &words,
            &mut std::io::stdout().lock(),
            &mut std::io::stderr().lock(),
            &env,
        );
        std::process::exit(code);
    }
    if args.next().is_some() {
        eprintln!("usage: gitmaster [PATH]");
        std::process::exit(2);
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
    let engine = gm_core::spawn(Box::new(git));
    // Started before the window dispatches its open, so the watcher sees the repository open.
    let watch = gm_core::watch::spawn(engine.clone(), login_path);
    let start = gm_ui::StartPath {
        path: path.to_string_lossy().into_owned(),
        from_argument,
    };
    gm_ui::run(engine, command_log, watch, start);
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
    #[test]
    fn version_comes_from_the_package() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
