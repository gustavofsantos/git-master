//! Finds and loads the workflow scripts of the user (design/workflow-scripts.md, D61, D73, D98):
//! `*.lua` files in the user's workflows directory and in the directories of
//! `gitmaster.workflowPath`. Nothing is read from a repository's working tree. `M.meta` is read
//! in the sandbox without `ctx`, so a script is listed without running any of its logic.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::inspect::inspect;
use super::meta::Meta;
use super::port::{ScriptDiagnostic, ScriptEngine};

/// Files larger than this are refused.
pub const MAX_FILE: u64 = 256 * 1024;

/// One script file and what loading it gave.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEntry {
    pub path: PathBuf,
    /// The file name, such as `branch-from-ticket.lua`.
    pub file_name: String,
    /// The workflow's `name`, or the file stem when the file does not load.
    pub name: String,
    /// sha256 of the file's bytes, lowercase hex: the key of reviews (D101).
    pub hash: String,
    /// Empty for a file that was refused before it was read (too large, not UTF-8).
    pub source: String,
    /// The checked `M.meta`, or every problem found, each with the line it points at.
    pub meta: Result<Meta, Vec<ScriptDiagnostic>>,
    /// Whether the script ships with the app (`WF4b`, D164(2)): no file, `path` is `builtin:<name>`.
    pub builtin: bool,
}

/// A directory or file that was not read as a workflow, and why. The page shows these as warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignored {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub entries: Vec<ScriptEntry>,
    pub ignored: Vec<Ignored>,
}

const IGNORED_IN_REPO: &str =
    "inside the repository's working tree; workflows are only read from your own config (D73)";

const TOML_IGNORED: &str = ".toml workflows are no longer supported; rewrite it as a .lua script";

/// `$XDG_CONFIG_HOME/gitmaster/workflows` (else `~/.config/...`) on Linux,
/// `~/Library/Application Support/gitmaster/workflows` on macOS. A relative `XDG_CONFIG_HOME` is
/// ignored; no home, no directory.
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

/// Loads every `*.lua` directly inside each directory, directories in the given order and files
/// by name. See the module doc and the tests for the rules.
pub fn load_dirs(engine: &dyn ScriptEngine, dirs: &[PathBuf], repo_root: Option<&Path>) -> Loaded {
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
        match script_files(dir) {
            Ok(listing) => {
                loaded
                    .entries
                    .extend(listing.scripts.iter().map(|file| load_file(engine, file)));
                loaded
                    .ignored
                    .extend(listing.tomls.into_iter().map(|path| Ignored {
                        path,
                        reason: TOML_IGNORED.to_owned(),
                    }));
            }
            Err(e) => loaded.ignored.push(Ignored {
                path: dir.clone(),
                reason: format!("cannot read the directory: {e}"),
            }),
        }
    }
    mark_duplicates(&mut loaded.entries);
    loaded
}

/// Loads one file by path, with the same checks, for `check`, `dry-run` and `test`.
pub fn load_file(engine: &dyn ScriptEngine, path: &Path) -> ScriptEntry {
    let file_name = file_name_of(path);
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_FILE) {
        return refused(
            path,
            &file_name,
            String::new(),
            "file is larger than 256 KiB",
        );
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return refused(
                path,
                &file_name,
                String::new(),
                format!("cannot read the file: {e}"),
            );
        }
    };
    let hash = sha256_hex(&bytes);
    let Ok(source) = String::from_utf8(bytes) else {
        return refused(path, &file_name, hash, "file is not valid UTF-8");
    };
    let meta = inspect(engine, &file_name, &source);
    let name = match &meta {
        Ok(meta) => meta.name.clone(),
        Err(_) => stem(path),
    };
    ScriptEntry {
        path: path.to_path_buf(),
        file_name,
        name,
        hash,
        source,
        meta,
        builtin: false,
    }
}

/// sha256 of `bytes`, lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The files directly inside a directory that matter here, each sorted by file name.
#[derive(Default)]
struct Listing {
    scripts: Vec<PathBuf>,
    tomls: Vec<PathBuf>,
}

/// The `*.lua` files and the `*.toml` files directly inside `dir`. Dot-files are skipped and
/// directories are never read; a symlink to a file is followed.
fn script_files(dir: &Path) -> std::io::Result<Listing> {
    let mut listing = Listing::default();
    for item in std::fs::read_dir(dir)?.flatten() {
        let path = item.path();
        let hidden = item.file_name().to_string_lossy().starts_with('.');
        if hidden || !path.is_file() {
            continue;
        }
        match path.extension() {
            Some(ext) if ext == "lua" => listing.scripts.push(path),
            Some(ext) if ext == "toml" => listing.tomls.push(path),
            _ => {}
        }
    }
    listing
        .scripts
        .sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    listing
        .tomls
        .sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(listing)
}

/// An entry whose file was not read as a script: no source, one problem at line 0. The hash is
/// empty when the bytes were never read.
fn refused(path: &Path, file_name: &str, hash: String, message: impl Into<String>) -> ScriptEntry {
    ScriptEntry {
        path: path.to_path_buf(),
        file_name: file_name.to_owned(),
        name: stem(path),
        hash,
        source: String::new(),
        meta: Err(vec![ScriptDiagnostic {
            line: 0,
            message: message.into(),
        }]),
        builtin: false,
    }
}

/// The later of two loaded files with the same workflow name is refused: a run must never pick
/// between two definitions.
fn mark_duplicates(entries: &mut [ScriptEntry]) {
    let mut first: HashMap<String, PathBuf> = HashMap::new();
    for entry in entries.iter_mut() {
        if entry.meta.is_err() {
            continue;
        }
        if let Some(earlier) = first.get(&entry.name) {
            let message = format!(
                "another workflow named \"{}\" is loaded from {}",
                entry.name,
                earlier.display()
            );
            entry.meta = Err(vec![ScriptDiagnostic { line: 0, message }]);
            entry.name = stem(&entry.path);
        } else {
            first.insert(entry.name.clone(), entry.path.clone());
        }
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::luau::LuauEngine;

    const FIXTURE: &str = r#"local M = {}
M.meta = {
  name = "suggest-commit-message",
  description = "Suggests a commit message from the staged diff.",
  trigger = { on = "manual" },
}
function M.execute(ctx, input)
end
return M
"#;

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

    fn load(dirs: &[PathBuf], repo_root: Option<&Path>) -> Loaded {
        load_dirs(&LuauEngine::new(), dirs, repo_root)
    }

    #[test]
    fn the_fixture_loads() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "suggest.lua", FIXTURE);
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(loaded.entries.len(), 1);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "suggest-commit-message");
        assert_eq!(entry.file_name, "suggest.lua");
        assert_eq!(entry.source, FIXTURE);
        assert_eq!(entry.hash, sha256_hex(FIXTURE.as_bytes()));
        assert!(entry.meta.is_ok(), "{:?}", entry.meta);
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn a_broken_file_is_an_error_with_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let source = "local M = {}\n\
            M.meta = { name = \"broken\", trigger = { on = \"manual\" } }\n\
            local x = = 1\n\
            function M.execute(ctx, input)\n\
            end\n\
            return M\n";
        write(dir.path(), "broken.lua", source);
        let loaded = load(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(
            entry.name, "broken",
            "a file that does not load is named by its stem"
        );
        assert_eq!(entry.source, source, "the source is kept");
        assert_eq!(entry.hash, sha256_hex(source.as_bytes()));
        let diagnostics = entry.meta.as_ref().unwrap_err();
        assert_eq!(diagnostics[0].line, 3, "{diagnostics:?}");
    }

    #[test]
    fn a_script_with_bad_meta_gives_the_problems_with_their_lines() {
        let dir = tempfile::tempdir().unwrap();
        let source = "local M = {}\nM.meta = {\n  name = \"w\",\n  trigger = { on = \"nope\" },\n}\n\
            function M.execute(ctx, input)\nend\nreturn M\n";
        write(dir.path(), "bad-meta.lua", source);
        let loaded = load(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "bad-meta");
        assert_eq!(entry.source, source);
        let diagnostics = entry.meta.as_ref().unwrap_err();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].line, 4);
        assert!(diagnostics[0].message.contains("unknown trigger `nope`"));
    }

    #[test]
    fn a_script_that_returns_no_table_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "no-table.lua", "return 1\n");
        let loaded = load(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "no-table");
        assert!(entry.meta.is_err(), "{:?}", entry.meta);
    }

    #[test]
    fn entries_are_sorted_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "c.lua", named("c"));
        write(dir.path(), "a.lua", named("a"));
        write(dir.path(), "b.lua", named("b"));
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["a", "b", "c"]);
    }

    #[test]
    fn only_lua_files_that_are_not_hidden_are_read() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes.md", named("md"));
        write(dir.path(), "README", named("plain"));
        write(dir.path(), ".hidden.lua", named("hidden"));
        write(dir.path(), "backup.lua.bak", named("bak"));
        std::fs::create_dir(dir.path().join("nested.lua")).unwrap();
        write(dir.path(), "ok.lua", named("ok"));
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["ok"]);
        assert!(loaded.ignored.is_empty(), "{:?}", loaded.ignored);
    }

    #[test]
    fn a_toml_file_is_ignored_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let old = write(dir.path(), "old.toml", "schema = 1\n");
        write(dir.path(), ".hidden.toml", "schema = 1\n");
        write(dir.path(), "new.lua", named("new"));
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["new"]);
        assert_eq!(
            loaded.ignored,
            [Ignored {
                path: old,
                reason: ".toml workflows are no longer supported; rewrite it as a .lua script"
                    .to_owned(),
            }]
        );
    }

    #[test]
    fn dot_directories_are_never_read() {
        let dir = tempfile::tempdir().unwrap();
        for hidden in [".api", ".tests"] {
            let inner = dir.path().join(hidden);
            std::fs::create_dir(&inner).unwrap();
            write(&inner, "fixture.lua", named("inside"));
        }
        write(dir.path(), "ok.lua", named("ok"));
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(names(&loaded), ["ok"]);
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn a_missing_directory_is_skipped_silently() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&[dir.path().join("nope")], None);
        assert_eq!(loaded, Loaded::default());
    }

    #[test]
    fn a_directory_inside_the_repository_is_ignored_and_a_sibling_is_read() {
        let repo = tempfile::tempdir().unwrap();
        let inside = repo.path().join("workflows");
        std::fs::create_dir(&inside).unwrap();
        write(&inside, "in.lua", named("in"));
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "out.lua", named("out"));

        let loaded = load(
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

        let root = load(&[repo.path().to_path_buf()], Some(repo.path()));
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
        let loaded = load(std::slice::from_ref(&file), None);
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
        write(dir.path(), "big.lua", too_big);
        let loaded = load(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert_eq!(entry.name, "big");
        assert!(entry.source.is_empty());
        assert_eq!(
            entry.meta.as_ref().unwrap_err(),
            &[ScriptDiagnostic {
                line: 0,
                message: "file is larger than 256 KiB".to_owned(),
            }]
        );
    }

    #[test]
    fn a_file_that_is_not_utf8_is_refused_with_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bin.lua", [0xff, 0xfe, 0x00]);
        let loaded = load(&[dir.path().to_path_buf()], None);
        let entry = &loaded.entries[0];
        assert!(entry.source.is_empty());
        assert_eq!(entry.hash.len(), 64);
        assert_eq!(
            entry.meta.as_ref().unwrap_err(),
            &[ScriptDiagnostic {
                line: 0,
                message: "file is not valid UTF-8".to_owned(),
            }]
        );
    }

    #[test]
    fn the_later_of_two_files_with_one_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let first = write(dir.path(), "a.lua", named("dup"));
        write(dir.path(), "b.lua", named("dup"));
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert!(loaded.entries[0].meta.is_ok());
        let later = &loaded.entries[1];
        assert_eq!(later.source, named("dup"), "the source is kept");
        assert_eq!(later.name, "b", "a refused entry is named by its stem");
        assert_eq!(
            later.meta.as_ref().unwrap_err(),
            &[ScriptDiagnostic {
                line: 0,
                message: format!(
                    "another workflow named \"dup\" is loaded from {}",
                    first.display()
                ),
            }]
        );
    }

    #[test]
    fn a_directory_listed_twice_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.lua", named("one"));
        let path = dir.path().to_path_buf();
        let loaded = load(&[path.clone(), path], None);
        assert_eq!(names(&loaded), ["one"]);
        assert!(loaded.entries[0].meta.is_ok());
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn the_hash_is_sha256_of_the_bytes_and_changes_with_one_byte() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "abc.lua", "abc");
        let loaded = load(&[dir.path().to_path_buf()], None);
        assert_eq!(
            loaded.entries[0].hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let path = write(dir.path(), "suggest.lua", FIXTURE);
        let hash_of = |loaded: &Loaded| {
            loaded
                .entries
                .iter()
                .find(|e| e.path == path)
                .unwrap()
                .hash
                .clone()
        };
        let original = hash_of(&load(&[dir.path().to_path_buf()], None));
        assert_eq!(original, sha256_hex(FIXTURE.as_bytes()));

        write(dir.path(), "copy.lua", FIXTURE.as_bytes());
        let copy = load_file(&LuauEngine::new(), &dir.path().join("copy.lua"));
        assert_eq!(copy.hash, original, "same bytes, same hash");

        write(
            dir.path(),
            "suggest.lua",
            FIXTURE.replacen("Suggests", "suggests", 1),
        );
        assert_ne!(original, hash_of(&load(&[dir.path().to_path_buf()], None)));
    }

    #[test]
    fn load_file_on_a_missing_path_gives_an_error_entry() {
        let dir = tempfile::tempdir().unwrap();
        let entry = load_file(&LuauEngine::new(), &dir.path().join("gone.lua"));
        assert_eq!(entry.file_name, "gone.lua");
        assert_eq!(entry.name, "gone");
        assert!(entry.source.is_empty());
        assert!(entry.hash.is_empty());
        let diagnostics = entry.meta.as_ref().unwrap_err();
        assert_eq!(diagnostics[0].line, 0);
        assert!(
            diagnostics[0].message.starts_with("cannot read the file: "),
            "{diagnostics:?}"
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
