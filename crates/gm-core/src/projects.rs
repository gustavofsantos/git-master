//! The list of projects a shell command can work on (docs/reference/cli.md, "Several
//! repositories").
//!
//! The file has the format of the author's `~/.projects`: one directory per line, a leading `~`
//! is the home directory, blank lines and `#` comments are ignored, and a glob such as
//! `~/Workplace/*` stands for the directories it matches. `parse` and `expand` are pure apart from
//! the directory listing a glob needs.

use std::path::{Path, PathBuf};

/// The entries of the file in `text`: comments and blank lines removed, `~` expanded. A line with
/// a glob is kept as it is; `expand` resolves it.
#[must_use]
pub fn parse(text: &str, home: Option<&Path>) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                return None;
            }
            Some(match (line.strip_prefix('~'), home) {
                (Some(""), Some(home)) => home.to_string_lossy().into_owned(),
                (Some(rest), Some(home)) if rest.starts_with('/') => {
                    format!("{}{rest}", home.to_string_lossy())
                }
                _ => line.to_owned(),
            })
        })
        .collect()
}

/// The directories the entries name, in order and each once. A glob gives the directories it
/// matches, sorted. A path that is not a directory is left out.
#[must_use]
pub fn expand(entries: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let matches: Vec<PathBuf> = if entry.contains(['*', '?', '[']) {
            glob::glob(entry)
                .map(|paths| paths.flatten().collect())
                .unwrap_or_default()
        } else {
            vec![PathBuf::from(entry)]
        };
        for path in matches.into_iter().filter(|path| path.is_dir()) {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

/// The projects listed in the file at `file`. A file that cannot be read lists nothing.
#[must_use]
pub fn read(file: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    std::fs::read_to_string(file)
        .map(|text| expand(&parse(&text, home)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_blank_lines_and_the_tilde_are_handled() {
        let home = Path::new("/home/u");
        let text = "# work\n\n~/app  # main\n  ~  \n/abs/path\n~x/not-home\n";
        assert_eq!(
            parse(text, Some(home)),
            ["/home/u/app", "/home/u", "/abs/path", "~x/not-home"]
        );
        assert_eq!(parse("~/a", None), ["~/a"]);
    }

    #[test]
    fn globs_expand_to_directories_and_each_one_appears_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for name in ["b", "a"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        std::fs::write(root.join("file"), "x").unwrap();
        let entries = vec![
            format!("{}/*", root.display()),
            format!("{}/a", root.display()),
            format!("{}/missing", root.display()),
        ];
        assert_eq!(expand(&entries), [root.join("a"), root.join("b")]);
    }

    #[test]
    fn an_unreadable_file_lists_nothing() {
        assert!(read(Path::new("/nonexistent/projects"), None).is_empty());
    }
}
