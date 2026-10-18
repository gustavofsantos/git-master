//! The user's PATH, for git started from a window that was not started by a
//! shell. A window opened from the Finder or the Dock has the system's minimal
//! PATH, and a commit hook that needs a program the shell's startup files add
//! (Homebrew's, say) fails there. Asking the login shell once closes the gap.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Printed around the environment so whatever the startup files print is
/// ignored.
const MARK: &str = "__GITMASTER_SHELL_ENV__";

/// Reads PATH from a login, interactive `shell`. Fails with a message when
/// the shell does not answer within `wait` or prints no PATH; the caller then
/// keeps the process's own. The process environment is never written.
pub fn login_path(shell: &str, wait: Duration) -> Result<String, String> {
    let output = run(shell, wait)?;
    parse(&output)
        .into_iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "the shell printed no PATH".to_owned())
}

/// Runs `shell` as a login, interactive shell that prints its environment,
/// and returns what it printed. The shell is killed after `wait`.
fn run(shell: &str, wait: Duration) -> Result<String, String> {
    let script = format!("printf %s {MARK}; env; printf %s {MARK}");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {shell}: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= wait => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{shell} took longer than {} s", wait.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The variables between the two marks of `output`, one `NAME=value` per
/// line. A line that is not `NAME=...` continues the previous value, which is
/// how `env` prints a value holding a newline. Empty when the marks are
/// missing.
#[must_use]
pub fn parse(output: &str) -> Vec<(String, String)> {
    let Some(start) = output.find(MARK) else {
        return Vec::new();
    };
    let rest = &output[start + MARK.len()..];
    let Some(end) = rest.find(MARK) else {
        return Vec::new();
    };
    let mut env: Vec<(String, String)> = Vec::new();
    for line in rest[..end].lines() {
        match line.split_once('=') {
            Some((key, value)) if is_name(key) => env.push((key.to_owned(), value.to_owned())),
            _ => {
                if let Some((_, value)) = env.last_mut() {
                    value.push('\n');
                    value.push_str(line);
                }
            }
        }
    }
    env
}

fn is_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_what_sits_between_the_marks() {
        let out = format!(
            "motd noise\n{MARK}PATH=/opt/homebrew/bin:/usr/bin\nHOME=/Users/g\nMULTI=a\nb c\n  d\nEMPTY=\n{MARK}prompt"
        );
        assert_eq!(
            parse(&out),
            [
                ("PATH".to_owned(), "/opt/homebrew/bin:/usr/bin".to_owned()),
                ("HOME".to_owned(), "/Users/g".to_owned()),
                ("MULTI".to_owned(), "a\nb c\n  d".to_owned()),
                ("EMPTY".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn without_both_marks_there_is_no_environment() {
        assert!(parse("PATH=/usr/bin\n").is_empty());
        assert!(parse(&format!("{MARK}PATH=/usr/bin\n")).is_empty());
    }
}
