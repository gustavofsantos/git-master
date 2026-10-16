//! `gitmaster [PATH]` opens the window on the repository at PATH, or on the
//! working directory. `gitmaster --version` prints the version.

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
    if args.next().is_some() {
        eprintln!("usage: gitmaster [PATH]");
        std::process::exit(2);
    }
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
    let engine = gm_core::spawn(Box::new(git));
    gm_ui::run(engine, command_log, path.to_string_lossy().into_owned());
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_comes_from_the_package() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
