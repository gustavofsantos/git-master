//! The pointer each member's repository keeps to the workspaces it belongs to (D180 (2)):
//! `gitmaster/workspaces` in the Git common directory, one absolute workspace path per line.
//! `gitmaster workspace add` writes it and `remove` takes the line out; the working tree is never
//! written. The History commit lookup reads it to find the workspace threads linked to a commit.

use std::io;
use std::path::{Path, PathBuf};

/// The file of the pointers, inside the `gitmaster` folder of the common directory.
const FILE: &str = "workspaces";
const FOLDER: &str = "gitmaster";

/// The pointer file of the repository whose common directory is `common_dir`.
#[must_use]
pub fn file_of(common_dir: &Path) -> PathBuf {
    common_dir.join(FOLDER).join(FILE)
}

/// `text` with `line` appended, unless a line of `text` already is `line`. Other lines are kept.
#[must_use]
pub fn with_line(text: &str, line: &str) -> String {
    if text.lines().any(|kept| kept == line) {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// `text` without the lines equal to `line`. An empty result means the file has no pointer left.
#[must_use]
pub fn without_line(text: &str, line: &str) -> String {
    text.lines()
        .filter(|kept| *kept != line && !kept.is_empty())
        .fold(String::new(), |mut out, kept| {
            out.push_str(kept);
            out.push('\n');
            out
        })
}

/// The workspace paths listed in the pointer file of `common_dir`, in file order. Empty when
/// there is no file, and a file that cannot be read counts as empty too.
#[must_use]
pub fn read(common_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file_of(common_dir))
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Lists the workspace folder `workspace` (absolute) in the pointer file of `common_dir`. A line
/// already there is kept once.
pub fn add(common_dir: &Path, workspace: &Path) -> io::Result<()> {
    let line = workspace.to_string_lossy().into_owned();
    let file = file_of(common_dir);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    std::fs::create_dir_all(common_dir.join(FOLDER))?;
    std::fs::write(file, with_line(&text, &line))
}

/// Takes `workspace` out of the pointer file of `common_dir`, and deletes the file when no line is
/// left. A missing file is not an error.
pub fn remove(common_dir: &Path, workspace: &Path) -> io::Result<()> {
    let line = workspace.to_string_lossy().into_owned();
    let file = file_of(common_dir);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(());
    };
    let kept = without_line(&text, &line);
    if kept.is_empty() {
        std::fs::remove_file(file)
    } else {
        std::fs::write(file, kept)
    }
}

#[cfg(test)]
mod tests {
    use super::{with_line, without_line};

    #[test]
    fn a_line_is_appended_once_and_the_others_are_kept() {
        let text = with_line("", "/ws/a");
        assert_eq!(text, "/ws/a\n");
        let text = with_line(&text, "/ws/b");
        assert_eq!(text, "/ws/a\n/ws/b\n");
        assert_eq!(with_line(&text, "/ws/a"), text, "no duplicate line");
    }

    #[test]
    fn a_missing_final_newline_is_completed_before_the_new_line() {
        assert_eq!(with_line("/ws/a", "/ws/b"), "/ws/a\n/ws/b\n");
    }

    #[test]
    fn only_the_named_line_goes_and_an_empty_result_says_so() {
        let text = "/ws/a\n/ws/b\n";
        assert_eq!(without_line(text, "/ws/a"), "/ws/b\n");
        assert_eq!(without_line("/ws/b\n", "/ws/b"), "");
        assert_eq!(without_line(text, "/ws/zzz"), text);
    }
}
