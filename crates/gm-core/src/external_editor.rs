//! Opens a file in the person's own editor (D233). The command is the Git config key
//! `gitmaster.editor`, split into words here and never given to a shell; without it, the
//! system opens the file with the application it has for it. The editor is started and left
//! alone: the app does not wait for it and reads nothing from it.

use std::path::Path;
use std::process::{Command, Stdio};

/// The Git config key that names the editor command.
pub const EDITOR_KEY: &str = "gitmaster.editor";

/// The words of a command line: split at blanks, with `'...'` and `"..."` keeping blanks and a
/// backslash keeping the next character (outside single quotes). No variable, no glob and no
/// operator is read: every word reaches the program as it is written.
///
/// # Errors
///
/// When a quote is left open or the line ends with a backslash.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some('"') | None, '\\') => {
                let next = chars
                    .next()
                    .ok_or_else(|| "the command ends with a backslash".to_owned())?;
                word.push(next);
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if std::mem::take(&mut in_word) {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if let Some(open) = quote {
        return Err(format!("the command has an open {open} quote"));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The program and its arguments that open `file`: the words of `configured` with the file as
/// the last argument, or the system's opener when nothing is configured.
///
/// # Errors
///
/// When the configured command cannot be split or has no word.
pub fn argv(configured: Option<&str>, file: &Path) -> Result<Vec<String>, String> {
    let file = file.to_string_lossy().into_owned();
    let Some(line) = configured.map(str::trim).filter(|line| !line.is_empty()) else {
        return Ok(system_opener(file));
    };
    let mut words = split_command(line).map_err(|e| format!("{EDITOR_KEY}: {e}"))?;
    if words.is_empty() {
        return Err(format!("{EDITOR_KEY} names no program"));
    }
    if let Some(rest) = words[0].strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        words[0] = Path::new(&home).join(rest).to_string_lossy().into_owned();
    }
    words.push(file);
    Ok(words)
}

/// What the system uses to open a file with its default application.
fn system_opener(file: String) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["open".to_owned(), file]
    } else if cfg!(target_os = "windows") {
        // `start` takes its first quoted argument as a window title.
        vec![
            "cmd".to_owned(),
            "/C".to_owned(),
            "start".to_owned(),
            String::new(),
            file,
        ]
    } else {
        vec!["xdg-open".to_owned(), file]
    }
}

/// Starts the editor on `file` and returns the program's name. The process runs in the folder of
/// the file, with no input and its output dropped, and `path` as its PATH when given (the login
/// shell's, as git gets it). A thread waits for it, so it leaves no zombie; nothing else follows
/// its end.
///
/// # Errors
///
/// When the command is not valid or the program cannot be started. With no configured command
/// the message says how to name one.
pub fn open(configured: Option<&str>, file: &Path, path: Option<&str>) -> Result<String, String> {
    let words = argv(configured, file)?;
    let (program, rest) = words
        .split_first()
        .ok_or_else(|| format!("{EDITOR_KEY} names no program"))?;
    let mut command = Command::new(program);
    if let Some(path) = path {
        // Set before spawning: the program is then looked up in this PATH too.
        command.env("PATH", path);
    }
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(folder) = file.parent() {
        command.current_dir(folder);
    }
    let mut child = command.spawn().map_err(|error| {
        if configured.is_some_and(|line| !line.trim().is_empty()) {
            format!("cannot start `{program}` ({EDITOR_KEY}): {error}")
        } else {
            format!(
                "cannot start `{program}`: {error}. Name your editor with `git config --global {EDITOR_KEY} <command>`"
            )
        }
    })?;
    std::thread::Builder::new()
        .name("gm-editor-wait".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|e| format!("cannot follow the editor: {e}"))?;
    Ok(program.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_command(line).unwrap()
    }

    #[test]
    fn blanks_split_words_and_quotes_keep_them() {
        assert_eq!(words("code"), ["code"]);
        assert_eq!(words("  kitty   nvim "), ["kitty", "nvim"]);
        assert_eq!(words("'my editor' --wait"), ["my editor", "--wait"]);
        assert_eq!(
            words(r#"term -e "nvim -p" x"#),
            ["term", "-e", "nvim -p", "x"]
        );
        assert_eq!(words(r"my\ editor"), ["my editor"]);
        assert_eq!(words(r#"a"b c"d"#), ["ab cd"]);
        assert_eq!(words("''"), [""]);
        assert!(words("   ").is_empty());
    }

    #[test]
    fn nothing_of_a_shell_is_read() {
        assert_eq!(
            words("nvim $HOME *.lua ; rm -rf x | y"),
            ["nvim", "$HOME", "*.lua", ";", "rm", "-rf", "x", "|", "y"]
        );
        assert_eq!(words(r"'a\b'"), [r"a\b"]);
    }

    #[test]
    fn an_open_quote_or_a_trailing_backslash_is_refused() {
        assert!(
            split_command("nvim 'x")
                .unwrap_err()
                .contains("open ' quote")
        );
        assert!(split_command("nvim \"x").is_err());
        assert!(split_command("nvim \\").unwrap_err().contains("backslash"));
    }

    #[test]
    fn the_file_is_the_last_argument_of_the_configured_command() {
        let file = Path::new("/w/my flow.lua");
        assert_eq!(
            argv(Some("kitty nvim"), file).unwrap(),
            ["kitty", "nvim", "/w/my flow.lua"]
        );
    }

    #[test]
    fn without_a_command_the_system_opens_the_file() {
        let file = Path::new("/w/a.lua");
        for unset in [None, Some(""), Some("  ")] {
            let argv = argv(unset, file).unwrap();
            assert_eq!(argv.last().map(String::as_str), Some("/w/a.lua"));
            assert_ne!(argv[0], "/w/a.lua");
        }
    }

    #[test]
    fn a_bad_command_names_the_key() {
        let error = argv(Some("nvim 'x"), Path::new("/w/a.lua")).unwrap_err();
        assert!(error.starts_with("gitmaster.editor: "), "{error}");
        assert!(argv(Some("''"), Path::new("/w/a.lua")).is_ok());
    }

    #[test]
    fn a_program_that_does_not_exist_says_so_with_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.lua");
        std::fs::write(&file, "").unwrap();
        let error = open(Some("gm-no-such-editor-program"), &file, None).unwrap_err();
        assert!(
            error.starts_with("cannot start `gm-no-such-editor-program` (gitmaster.editor): "),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_editor_starts_in_the_folder_of_the_file_with_the_file_as_argument() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.lua");
        std::fs::write(&file, "").unwrap();
        let script = dir.path().join("editor.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$1\" \"$2\" > opened.txt\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let line = format!("'{}' --flag", script.display());
        let program = open(Some(&line), &file, None).unwrap();
        assert_eq!(program, script.display().to_string());
        let written = dir.path().join("opened.txt");
        for _ in 0..200 {
            if std::fs::read_to_string(&written).is_ok_and(|text| text.lines().count() == 2) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&written).unwrap(),
            format!("--flag\n{}\n", file.display())
        );
    }
}
