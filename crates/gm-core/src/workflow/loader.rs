//! Finds and loads the workflow files of the user (docs/10, D61, D73): the
//! user's workflows directory and the directories in `gitmaster.workflowPath`.
//! Nothing is ever read from a repository's working tree.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::Diagnostic;
use super::model::Definition;

/// Files larger than this are refused.
pub const MAX_FILE: u64 = 256 * 1024;

/// One file and what loading it gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// The workflow's `name`, or the file stem when the file does not parse.
    pub name: String,
    /// sha256 of the file's bytes, lowercase hex: the key of approvals.
    pub hash: String,
    pub source: String,
    pub result: Result<(Definition, Vec<Diagnostic>), Vec<Diagnostic>>,
}

/// A directory that was not read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub entries: Vec<Entry>,
    pub ignored: Vec<Ignored>,
}

const IGNORED_IN_REPO: &str =
    "inside the repository's working tree; workflows are only read from your own config (D73)";

/// `$XDG_CONFIG_HOME/gitmaster/workflows` (else `~/.config/...`) on Linux,
/// `~/Library/Application Support/gitmaster/workflows` on macOS. A relative
/// `XDG_CONFIG_HOME` is ignored; no home, no directory.
pub fn user_dir(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let absolute = |value: Option<std::ffi::OsString>| {
        value
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
    };
    let base = if macos {
        absolute(home)?.join("Library/Application Support")
    } else {
        match absolute(xdg_config_home) {
            Some(base) => base,
            None => absolute(home)?.join(".config"),
        }
    };
    Some(base.join("gitmaster").join("workflows"))
}

/// `user_dir` with this process's environment.
pub fn default_user_dir() -> Option<PathBuf> {
    user_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

/// Loads every `*.toml` directly inside each directory, directories in the
/// given order and files by name. A directory inside `repo_root` is ignored
/// (D73); a missing directory is skipped without a note.
pub fn load_dirs(dirs: &[PathBuf], repo_root: Option<&Path>) -> Loaded {
    let repo_root = repo_root.map(canonical);
    let mut loaded = Loaded::default();
    let mut seen = HashSet::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let real = canonical(dir);
        if !seen.insert(real.clone()) {
            continue;
        }
        if !real.is_dir() {
            loaded.ignored.push(Ignored {
                path: dir.clone(),
                reason: "not a directory".to_owned(),
            });
            continue;
        }
        if repo_root
            .as_ref()
            .is_some_and(|root| real.starts_with(root))
        {
            loaded.ignored.push(Ignored {
                path: dir.clone(),
                reason: IGNORED_IN_REPO.to_owned(),
            });
            continue;
        }
        match workflow_files(dir) {
            Ok(files) => loaded
                .entries
                .extend(files.iter().map(|file| read_entry(file))),
            Err(e) => loaded.ignored.push(Ignored {
                path: dir.clone(),
                reason: format!("cannot read the directory: {e}"),
            }),
        }
    }
    mark_duplicates(&mut loaded.entries);
    loaded
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The `*.toml` files directly inside `dir`, sorted by file name. Dot-files
/// are skipped; a symlink to a file is followed.
fn workflow_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for item in std::fs::read_dir(dir)?.flatten() {
        let path = item.path();
        let hidden = item.file_name().to_string_lossy().starts_with('.');
        if !hidden && path.extension().is_some_and(|ext| ext == "toml") && path.is_file() {
            files.push(path);
        }
    }
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(files)
}

fn read_entry(path: &Path) -> Entry {
    let stem = stem(path);
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_FILE) {
        return refused(path, stem, String::new(), "file is larger than 256 KiB");
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return refused(
                path,
                stem,
                String::new(),
                format!("cannot read the file: {e}"),
            );
        }
    };
    let hash = sha256_hex(&bytes);
    let Ok(source) = String::from_utf8(bytes) else {
        return refused(path, stem, hash, "file is not valid UTF-8");
    };
    let result = super::load(&source);
    let name = match &result {
        Ok((definition, _)) => definition.name.clone(),
        Err(_) => stem,
    };
    Entry {
        path: path.to_path_buf(),
        name,
        hash,
        source,
        result,
    }
}

/// An entry whose file was not parsed: no source, one error at line 0.
fn refused(path: &Path, name: String, hash: String, message: impl Into<String>) -> Entry {
    Entry {
        path: path.to_path_buf(),
        name,
        hash,
        source: String::new(),
        result: Err(vec![Diagnostic::error(0, message)]),
    }
}

/// The later of two loaded files with the same workflow name is refused: a
/// run must never pick between two definitions.
fn mark_duplicates(entries: &mut [Entry]) {
    let mut first: HashMap<String, PathBuf> = HashMap::new();
    for entry in entries.iter_mut() {
        if entry.result.is_err() {
            continue;
        }
        if let Some(earlier) = first.get(&entry.name) {
            let message = format!(
                "another workflow is also named `{}`: {}",
                entry.name,
                earlier.display()
            );
            entry.result = Err(vec![Diagnostic::error(0, message)]);
            entry.name = stem(&entry.path);
        } else {
            first.insert(entry.name.clone(), entry.path.clone());
        }
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

/// Lowercase hex sha256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/workflows/suggest-commit-message.toml");

    /// The fixture under another name, so several files can coexist.
    fn named(name: &str) -> String {
        FIXTURE.replacen(
            "name = \"suggest-commit-message\"",
            &format!("name = \"{name}\""),
            1,
        )
    }

    fn write(dir: &Path, file: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn names(loaded: &Loaded) -> Vec<&str> {
        loaded.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn the_fixture_loads() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "suggest.toml", FIXTURE);
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        assert_eq!(loaded.entries.len(), 1);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "suggest-commit-message");
        assert_eq!(entry.source, FIXTURE);
        assert_eq!(entry.hash, sha256_hex(FIXTURE.as_bytes()));
        assert!(entry.result.is_ok());
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn a_broken_file_is_an_error_with_a_line() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "broken.toml",
            "schema = 1\nname = \"broken\"\n[[state\nid = \"x\"\n",
        );
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(
            entry.name, "broken",
            "a file that does not parse is named by its stem"
        );
        let diagnostics = entry.result.as_ref().unwrap_err();
        assert!(diagnostics[0].line > 0, "{diagnostics:?}");
    }

    #[test]
    fn entries_are_sorted_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "c.toml", named("c"));
        write(dir.path(), "a.toml", named("a"));
        write(dir.path(), "b.toml", named("b"));
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["a", "b", "c"]);
    }

    #[test]
    fn only_toml_files_that_are_not_hidden_are_read() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes.md", named("md"));
        write(dir.path(), "README", named("plain"));
        write(dir.path(), ".hidden.toml", named("hidden"));
        write(dir.path(), "backup.toml.bak", named("bak"));
        std::fs::create_dir(dir.path().join("nested.toml")).unwrap();
        write(dir.path(), "ok.toml", named("ok"));
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["ok"]);
    }

    #[test]
    fn a_missing_directory_is_skipped_silently() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_dirs(&[dir.path().join("nope")], None);
        assert_eq!(loaded, Loaded::default());
    }

    #[test]
    fn a_directory_inside_the_repository_is_ignored_and_a_sibling_is_read() {
        let repo = tempfile::tempdir().unwrap();
        let inside = repo.path().join("workflows");
        std::fs::create_dir(&inside).unwrap();
        write(&inside, "in.toml", named("in"));
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "out.toml", named("out"));

        let loaded = load_dirs(
            &[inside.clone(), outside.path().to_path_buf()],
            Some(repo.path()),
        );
        assert_eq!(
            loaded.ignored,
            [Ignored {
                path: inside,
                reason: IGNORED_IN_REPO.to_owned(),
            }]
        );
        assert_eq!(names(&loaded), ["out"]);

        let root = load_dirs(&[repo.path().to_path_buf()], Some(repo.path()));
        assert_eq!(
            root.ignored.len(),
            1,
            "the repository root itself is inside"
        );
        assert!(root.entries.is_empty());
    }

    #[test]
    fn a_file_given_as_a_directory_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "plain", "x");
        let loaded = load_dirs(std::slice::from_ref(&file), None);
        assert_eq!(
            loaded.ignored,
            [Ignored {
                path: file,
                reason: "not a directory".to_owned(),
            }]
        );
    }

    #[test]
    fn a_file_over_the_limit_is_refused_without_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let too_big = vec![b'#'; usize::try_from(MAX_FILE).unwrap() + 1];
        write(dir.path(), "big.toml", too_big);
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "big");
        assert!(entry.source.is_empty());
        assert_eq!(
            entry.result.as_ref().unwrap_err(),
            &[Diagnostic::error(0, "file is larger than 256 KiB")]
        );
    }

    #[test]
    fn a_file_that_is_not_utf8_is_refused_with_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bin.toml", [0xff, 0xfe, 0x00]);
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert!(entry.source.is_empty());
        assert_eq!(entry.hash.len(), 64);
        assert_eq!(
            entry.result.as_ref().unwrap_err(),
            &[Diagnostic::error(0, "file is not valid UTF-8")]
        );
    }

    #[test]
    fn the_later_of_two_files_with_one_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let first = write(dir.path(), "a.toml", named("dup"));
        write(dir.path(), "b.toml", named("dup"));
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        assert!(loaded.entries[0].result.is_ok());
        let later = &loaded.entries[1];
        assert_eq!(later.source, named("dup"), "the source is kept");
        assert_eq!(later.name, "b", "a refused entry is named by its stem");
        assert_eq!(
            later.result.as_ref().unwrap_err(),
            &[Diagnostic::error(
                0,
                format!("another workflow is also named `dup`: {}", first.display()),
            )]
        );
    }

    #[test]
    fn a_directory_listed_twice_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.toml", named("one"));
        let path = dir.path().to_path_buf();
        let loaded = load_dirs(&[path.clone(), path], None);
        assert_eq!(names(&loaded), ["one"]);
        assert!(loaded.entries[0].result.is_ok());
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn the_hash_is_sha256_of_the_bytes_and_changes_with_one_byte() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "abc.toml", "abc");
        let loaded = load_dirs(&[dir.path().to_path_buf()], None);
        assert_eq!(
            loaded.entries[0].hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let path = write(dir.path(), "suggest.toml", FIXTURE);
        let hash_of = |loaded: &Loaded| {
            loaded
                .entries
                .iter()
                .find(|e| e.path == path)
                .unwrap()
                .hash
                .clone()
        };
        let original = hash_of(&load_dirs(&[dir.path().to_path_buf()], None));
        assert_eq!(original, sha256_hex(FIXTURE.as_bytes()));

        write(
            dir.path(),
            "suggest.toml",
            FIXTURE.replacen("Suggest", "suggest", 1),
        );
        assert_ne!(
            original,
            hash_of(&load_dirs(&[dir.path().to_path_buf()], None))
        );
    }

    #[test]
    fn user_dir_follows_xdg_config_home_then_home() {
        assert_eq!(
            user_dir(Some("/x".into()), Some("/h".into()), false),
            Some(PathBuf::from("/x/gitmaster/workflows"))
        );
        assert_eq!(
            user_dir(Some("rel".into()), Some("/h".into()), false),
            Some(PathBuf::from("/h/.config/gitmaster/workflows"))
        );
        assert_eq!(
            user_dir(None, Some("/h".into()), false),
            Some(PathBuf::from("/h/.config/gitmaster/workflows"))
        );
        assert_eq!(user_dir(None, None, false), None);
        assert_eq!(user_dir(Some("rel".into()), None, false), None);
    }

    #[test]
    fn user_dir_on_macos_uses_application_support() {
        assert_eq!(
            user_dir(None, Some("/h".into()), true),
            Some(PathBuf::from(
                "/h/Library/Application Support/gitmaster/workflows"
            ))
        );
        assert_eq!(
            user_dir(Some("/x".into()), Some("/h".into()), true),
            Some(PathBuf::from(
                "/h/Library/Application Support/gitmaster/workflows"
            ))
        );
        assert_eq!(user_dir(Some("/x".into()), None, true), None);
    }
}
