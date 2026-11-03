//! The `.worktreeinclude` file of a repository: which ignored files a new worktree shares with
//! the main checkout (docs/reference/cli.md, "Worktrees").
//!
//! The file sits at the root of the main checkout and uses `.gitignore` syntax. A path is shared
//! when a pattern of the file matches it **and** Git ignores it, so a tracked file is never
//! touched: the worktree checks those out itself. The shared path is not copied. It becomes a
//! symbolic link to the main checkout's file, or, with `Mode::Reflink`, a copy-on-write clone that
//! takes no disk space until one side changes. A pattern that matches a directory shares the whole
//! directory with one link.
//!
//! The selection (`select`) is pure given the list of ignored paths; `ignored_entries` asks Git for
//! that list, and `apply` writes the links.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::GitError;

/// The name of the file, at the root of the main checkout.
pub const FILE_NAME: &str = ".worktreeinclude";

const BLOCK_START: &str = "# >>> gitmaster worktreeinclude";
const BLOCK_END: &str = "# <<< gitmaster worktreeinclude";

/// How a shared path reaches the new worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A symbolic link to the main checkout. Both worktrees see one file.
    Symlink,
    /// A copy-on-write clone (`cp --reflink`, `cp -c` on macOS). Falls back to a symbolic link on
    /// a file system that cannot clone.
    Reflink,
    /// Share nothing.
    None,
}

impl Mode {
    /// The mode named `name`: `symlink`, `reflink` or `none`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "symlink" => Some(Self::Symlink),
            "reflink" => Some(Self::Reflink),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// A path the file selects, relative to the main checkout with `/` separators.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
}

/// What `apply` did with one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Symlinked,
    Reflinked,
    /// A reflink was asked for and the file system refused; a symbolic link was made instead.
    SymlinkedInstead,
    /// Nothing was written, for the reason given.
    Skipped(String),
    Failed(String),
}

/// What `apply` did with `entry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub entry: Entry,
    pub action: Action,
}

/// The patterns of a `.worktreeinclude`, matched against paths under `root`.
pub struct Matcher(Gitignore);

impl Matcher {
    /// Builds the matcher from the text of the file. A pattern that does not compile is an error
    /// naming its line.
    pub fn new(root: &Path, text: &str) -> Result<Self, GitError> {
        let mut builder = GitignoreBuilder::new(root);
        for (index, line) in text.lines().enumerate() {
            builder
                .add_line(None, line)
                .map_err(|e| GitError(format!("{FILE_NAME}:{}: {e}", index + 1)))?;
        }
        builder
            .build()
            .map(Self)
            .map_err(|e| GitError(format!("{FILE_NAME}: {e}")))
    }

    /// Whether a pattern selects `path` (relative, `/`-separated), or one of its parent folders.
    fn selects(&self, path: &str, is_dir: bool) -> bool {
        self.0.matched_path_or_any_parents(path, is_dir).is_ignore()
    }
}

/// Git's ignored paths in the checkout at `root`, as `git ls-files --others --ignored` prints
/// them: a folder that is ignored as a whole is one entry ending in `/`.
pub fn ignored_entries(root: &Path) -> Result<Vec<String>, GitError> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ])
        .arg("--directory")
        .current_dir(root)
        .output()
        .map_err(|e| GitError(format!("cannot run git: {e}")))?;
    if !output.status.success() {
        return Err(GitError(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect())
}

/// The entries of `ignored` that `matcher` selects, sorted. A folder the patterns do not select as
/// a whole is searched for files they do select, because `.env` selects `secrets/.env` inside an
/// ignored `secrets/`. Reads folders under `root` only for that search.
#[must_use]
pub fn select(root: &Path, matcher: &Matcher, ignored: &[String]) -> Vec<Entry> {
    let mut chosen = BTreeSet::new();
    for item in ignored {
        match item.strip_suffix('/') {
            Some(dir) => visit_dir(root, matcher, dir, &mut chosen),
            None => {
                if matcher.selects(item, false) {
                    chosen.insert(Entry {
                        path: item.clone(),
                        is_dir: false,
                    });
                }
            }
        }
    }
    chosen.into_iter().collect()
}

/// Selects `dir` as a whole when a pattern matches it, otherwise looks at what it holds.
fn visit_dir(root: &Path, matcher: &Matcher, dir: &str, chosen: &mut BTreeSet<Entry>) {
    if matcher.selects(dir, true) {
        chosen.insert(Entry {
            path: dir.to_owned(),
            is_dir: true,
        });
        return;
    }
    let Ok(children) = fs::read_dir(root.join(dir)) else {
        return;
    };
    for child in children.flatten() {
        let name = child.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let path = format!("{dir}/{name}");
        // A link is never followed: it is shared as the link it is.
        let is_dir = child.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir {
            visit_dir(root, matcher, &path, chosen);
        } else if matcher.selects(&path, false) {
            chosen.insert(Entry {
                path,
                is_dir: false,
            });
        }
    }
}

/// Shares each entry of the checkout at `source` into the worktree at `target`. An entry whose
/// place in the worktree is taken is skipped, never replaced. With `dry_run` nothing is written
/// and the actions say what would be done. A failure on one entry does not stop the others.
#[must_use]
pub fn apply(
    source: &Path,
    target: &Path,
    entries: &[Entry],
    mode: Mode,
    dry_run: bool,
) -> Vec<Applied> {
    if mode == Mode::None {
        return Vec::new();
    }
    entries
        .iter()
        .map(|entry| {
            let action = share(source, target, entry, mode, dry_run);
            Applied {
                entry: entry.clone(),
                action,
            }
        })
        .collect()
}

fn share(source: &Path, target: &Path, entry: &Entry, mode: Mode, dry_run: bool) -> Action {
    let from = source.join(&entry.path);
    let to = target.join(&entry.path);
    if to.symlink_metadata().is_ok() {
        return Action::Skipped("already exists".into());
    }
    if from.symlink_metadata().is_err() {
        return Action::Skipped("gone from the main checkout".into());
    }
    if dry_run {
        return if mode == Mode::Reflink {
            Action::Reflinked
        } else {
            Action::Symlinked
        };
    }
    if let Some(parent) = to.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Action::Failed(error.to_string());
    }
    // A link in the main checkout is shared as a link; there is nothing to clone.
    let is_link = from
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink());
    if mode == Mode::Reflink && !is_link {
        if reflink(&from, &to, entry.is_dir).is_ok() {
            return Action::Reflinked;
        }
        // A clone that stopped halfway leaves a partial copy behind.
        remove_any(&to);
        return match symlink(&from, &to) {
            Ok(()) => Action::SymlinkedInstead,
            Err(error) => Action::Failed(error.to_string()),
        };
    }
    match symlink(&from, &to) {
        Ok(()) => Action::Symlinked,
        Err(error) => Action::Failed(error.to_string()),
    }
}

fn remove_any(path: &Path) {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() => drop(fs::remove_dir_all(path)),
        Ok(_) => drop(fs::remove_file(path)),
        Err(_) => {}
    }
}

#[cfg(unix)]
fn symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(not(unix))]
fn symlink(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are not supported on this platform",
    ))
}

/// A copy-on-write clone through the system `cp`: `--reflink=always` on Linux, `-c` on macOS. It
/// fails, rather than copying the bytes, when the file system cannot clone.
fn reflink(from: &Path, to: &Path, is_dir: bool) -> io::Result<()> {
    let mut command = Command::new("cp");
    if cfg!(target_os = "macos") {
        command.arg("-c");
        if is_dir {
            command.arg("-R");
        }
    } else {
        command.arg("--reflink=always");
        if is_dir {
            command.arg("-a");
        }
    }
    let status = command
        .arg("--")
        .arg(from)
        .arg(to)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("the file system cannot clone"))
    }
}

/// Keeps symbolic links to shared folders out of `git status` in every worktree.
///
/// A pattern such as `node_modules/` matches a folder but not a link to one, so the link would
/// show as untracked. The folders are listed, anchored and without a trailing slash, in a block of
/// `<common dir>/info/exclude`, which all worktrees read. The block is rewritten as the union of
/// what it held and `dirs`; the rest of the file is kept. Nothing is written when it is up to date.
pub fn ensure_excludes(common_dir: &Path, dirs: &[String]) -> io::Result<()> {
    if dirs.is_empty() {
        return Ok(());
    }
    let file = common_dir.join("info").join("exclude");
    let text = fs::read_to_string(&file).unwrap_or_default();
    let (before, block, after) = split_block(&text);
    let mut lines: BTreeSet<String> = block.into_iter().collect();
    for dir in dirs {
        lines.insert(format!("/{}", dir.trim_end_matches('/')));
    }
    let mut next = before;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(BLOCK_START);
    next.push('\n');
    for line in &lines {
        next.push_str(line);
        next.push('\n');
    }
    next.push_str(BLOCK_END);
    next.push('\n');
    next.push_str(&after);
    if next == text {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(file, next)
}

/// The text before the managed block, the lines inside it, and the text after it.
fn split_block(text: &str) -> (String, Vec<String>, String) {
    let Some(start) = text.find(BLOCK_START) else {
        return (text.to_owned(), Vec::new(), String::new());
    };
    let Some(end_at) = text[start..].find(BLOCK_END) else {
        // A block that was never closed is dropped with everything after its start.
        return (text[..start].to_owned(), Vec::new(), String::new());
    };
    let end = start + end_at + BLOCK_END.len();
    let inside = text[start + BLOCK_START.len()..start + end_at]
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    let after = text[end..].strip_prefix('\n').unwrap_or(&text[end..]);
    (text[..start].to_owned(), inside, after.to_owned())
}

/// The folder entries among `applied` that were linked rather than cloned, as the paths
/// `ensure_excludes` takes.
#[must_use]
pub fn linked_dirs(applied: &[Applied]) -> Vec<String> {
    applied
        .iter()
        .filter(|item| {
            item.entry.is_dir && matches!(item.action, Action::Symlinked | Action::SymlinkedInstead)
        })
        .map(|item| item.entry.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(text: &str) -> Matcher {
        Matcher::new(Path::new("/repo"), text).unwrap()
    }

    fn paths(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn mode_names_parse() {
        assert_eq!(Mode::parse("symlink"), Some(Mode::Symlink));
        assert_eq!(Mode::parse("reflink"), Some(Mode::Reflink));
        assert_eq!(Mode::parse("none"), Some(Mode::None));
        assert_eq!(Mode::parse("copy"), None);
    }

    #[test]
    fn a_pattern_selects_only_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        let ignored = vec![".env".to_owned(), "build.log".to_owned()];
        let chosen = select(dir.path(), &matcher(".env\n"), &ignored);
        assert_eq!(paths(&chosen), [".env"]);
    }

    #[test]
    fn comments_blank_lines_and_negation_follow_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let ignored = vec![
            ".env".to_owned(),
            ".env.local".to_owned(),
            "x.key".to_owned(),
        ];
        let text = "# secrets\n\n.env*\n!.env.local\n";
        let chosen = select(dir.path(), &matcher(text), &ignored);
        assert_eq!(paths(&chosen), [".env"]);
    }

    #[test]
    fn a_folder_pattern_selects_the_folder_as_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let ignored = vec!["node_modules/".to_owned()];
        let chosen = select(dir.path(), &matcher("node_modules/\n"), &ignored);
        assert_eq!(
            chosen,
            [Entry {
                path: "node_modules".into(),
                is_dir: true
            }]
        );
    }

    #[test]
    fn a_file_pattern_is_found_inside_an_ignored_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("secrets/deep")).unwrap();
        fs::write(dir.path().join("secrets/.env"), "k=v").unwrap();
        fs::write(dir.path().join("secrets/other"), "x").unwrap();
        fs::write(dir.path().join("secrets/deep/.env"), "k=v").unwrap();
        let ignored = vec!["secrets/".to_owned()];
        let chosen = select(dir.path(), &matcher(".env\n"), &ignored);
        assert_eq!(paths(&chosen), ["secrets/.env", "secrets/deep/.env"]);
    }

    #[test]
    fn a_pattern_under_a_folder_matches_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("config/local.json"), "{}").unwrap();
        let ignored = vec!["config/local.json".to_owned()];
        let chosen = select(dir.path(), &matcher("/config/*.json\n"), &ignored);
        assert_eq!(paths(&chosen), ["config/local.json"]);
    }

    #[test]
    fn a_bad_pattern_names_its_line() {
        let error = Matcher::new(Path::new("/repo"), "ok\n[z-a]\n")
            .err()
            .unwrap();
        assert!(error.0.contains(":2:"), "{}", error.0);
    }

    #[test]
    fn the_exclude_block_is_added_merged_and_left_alone_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let info = dir.path().join("info");
        fs::create_dir_all(&info).unwrap();
        fs::write(info.join("exclude"), "# mine\n*.swp\n").unwrap();
        ensure_excludes(dir.path(), &["node_modules".into()]).unwrap();
        ensure_excludes(dir.path(), &["a/b".into(), "node_modules".into()]).unwrap();
        let text = fs::read_to_string(info.join("exclude")).unwrap();
        assert_eq!(
            text,
            format!("# mine\n*.swp\n{BLOCK_START}\n/a/b\n/node_modules\n{BLOCK_END}\n")
        );
        ensure_excludes(dir.path(), &["node_modules".into()]).unwrap();
        assert_eq!(fs::read_to_string(info.join("exclude")).unwrap(), text);
    }

    #[test]
    fn text_after_the_block_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let info = dir.path().join("info");
        fs::create_dir_all(&info).unwrap();
        fs::write(
            info.join("exclude"),
            format!("{BLOCK_START}\n/old\n{BLOCK_END}\ntail\n"),
        )
        .unwrap();
        ensure_excludes(dir.path(), &["new".into()]).unwrap();
        let text = fs::read_to_string(info.join("exclude")).unwrap();
        assert_eq!(
            text,
            format!("{BLOCK_START}\n/new\n/old\n{BLOCK_END}\ntail\n")
        );
    }
}
