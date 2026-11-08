//! The recently opened repositories, newest first. The list lives in the app's
//! data directory, never inside a repository, so a repository's working tree
//! is never written to. The list logic is pure; the file access takes its
//! paths as arguments and never blocks the UI: callers run `Store::save` on a
//! background task.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// How many recent repositories are kept.
pub const CAP: usize = 10;

/// The recent repositories, newest first, with no path twice and at most `CAP`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recents {
    paths: Vec<PathBuf>,
}

impl Recents {
    /// Builds a list from paths given newest first. Repeats keep their first
    /// place and anything past the cap is dropped, so a hand-edited file comes
    /// back normalized.
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut kept: Vec<PathBuf> = Vec::new();
        for path in paths {
            if kept.len() == CAP {
                break;
            }
            if !kept.contains(&path) {
                kept.push(path);
            }
        }
        Self { paths: kept }
    }

    /// Moves `path` to the front, adding it if it is new, and drops the oldest
    /// entry past the cap.
    pub fn push(&mut self, path: PathBuf) {
        self.paths.retain(|known| known != &path);
        self.paths.insert(0, path);
        self.paths.truncate(CAP);
    }

    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// The file's text. A file that is not a JSON array of strings reads as
    /// an empty list.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match serde_json::from_str::<Vec<String>>(text) {
            Ok(paths) => Self::from_paths(
                paths
                    .into_iter()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
            ),
            Err(_) => Self::default(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        let paths: Vec<String> = self
            .paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        // A list of strings always serializes.
        serde_json::to_string_pretty(&paths).unwrap_or_else(|_| "[]".to_owned())
    }
}

/// The file the list is kept in, under the given environment values. Pure, so
/// both platforms' layouts can be tested on any host. A relative or empty
/// `XDG_DATA_HOME` is ignored, as the XDG spec asks. Without a home directory
/// there is no file.
#[must_use]
pub fn data_file(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    gm_core::paths::data_dir_under(xdg_data_home, home).map(|dir| dir.join("recent.json"))
}

/// The file for this process, from the real environment.
#[must_use]
pub fn default_file() -> Option<PathBuf> {
    data_file(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// The list's file. Saves are written in the order they were asked for: a
/// save that is older than the one already on disk is skipped.
pub struct Store {
    path: PathBuf,
    /// The generation of the file on disk.
    written: Mutex<u64>,
}

impl Store {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            written: Mutex::new(0),
        }
    }

    /// The saved list, or an empty one when the file is missing or unreadable.
    #[must_use]
    pub fn load(&self) -> Recents {
        fs::read_to_string(&self.path)
            .map(|text| Recents::parse(&text))
            .unwrap_or_default()
    }

    /// Writes `recents` as generation `generation` (a larger number is newer).
    /// The file is replaced whole, through a temporary file, so a crash
    /// leaves the old list or the new one, never half of each.
    pub fn save(&self, recents: &Recents, generation: u64) -> io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if generation <= *written {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = temp_path(&self.path);
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(recents.to_json().as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temp, &self.path)?;
        *written = generation;
        Ok(())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/repos/{name}"))
    }

    #[test]
    fn push_puts_the_newest_first() {
        let mut recents = Recents::default();
        recents.push(p("a"));
        recents.push(p("b"));
        assert_eq!(recents.paths(), [p("b"), p("a")]);
    }

    #[test]
    fn pushing_a_known_path_moves_it_to_the_front() {
        let mut recents = Recents::default();
        recents.push(p("a"));
        recents.push(p("b"));
        recents.push(p("a"));
        assert_eq!(recents.paths(), [p("a"), p("b")]);
    }

    #[test]
    fn the_list_keeps_only_the_ten_newest() {
        let mut recents = Recents::default();
        for n in 0..12 {
            recents.push(p(&n.to_string()));
        }
        assert_eq!(recents.paths().len(), CAP);
        assert_eq!(recents.paths()[0], p("11"));
        assert_eq!(recents.paths()[CAP - 1], p("2"));
    }

    #[test]
    fn from_paths_dedupes_and_caps_what_it_is_given() {
        let given = ["a", "b", "a", "c"]
            .map(p)
            .into_iter()
            .chain((0..20).map(|n| p(&format!("x{n}"))));
        let recents = Recents::from_paths(given);
        assert_eq!(recents.paths().len(), CAP);
        assert_eq!(recents.paths()[..3], [p("a"), p("b"), p("c")]);
        let unique: std::collections::HashSet<_> = recents.paths().iter().collect();
        assert_eq!(unique.len(), CAP);
    }

    #[test]
    fn a_json_list_round_trips() {
        let mut recents = Recents::default();
        recents.push(p("a"));
        recents.push(p("b"));
        assert_eq!(Recents::parse(&recents.to_json()), recents);
    }

    #[test]
    fn a_corrupt_file_reads_as_an_empty_list() {
        assert_eq!(Recents::parse("{not json"), Recents::default());
        assert_eq!(Recents::parse("{\"paths\": 3}"), Recents::default());
        assert_eq!(Recents::parse(""), Recents::default());
    }

    #[test]
    fn empty_entries_in_a_file_are_dropped() {
        assert_eq!(Recents::parse(r#"["", "/repos/a"]"#).paths(), [p("a")]);
    }

    #[test]
    fn data_file_follows_xdg_then_home() {
        let xdg = Some(OsString::from("/data"));
        assert_eq!(
            data_file(xdg, Some("/home/u".into())),
            Some(PathBuf::from("/data/gitmaster/recent.json"))
        );
        assert_eq!(
            data_file(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/share/gitmaster/recent.json"))
        );
    }

    #[test]
    fn a_relative_or_empty_xdg_value_is_ignored() {
        for bad in ["relative/dir", ""] {
            assert_eq!(
                data_file(Some(bad.into()), Some("/home/u".into())),
                Some(PathBuf::from("/home/u/.local/share/gitmaster/recent.json"))
            );
        }
    }

    #[test]
    fn the_same_layout_on_a_mac_home() {
        assert_eq!(
            data_file(None, Some("/Users/u".into())),
            Some(PathBuf::from("/Users/u/.local/share/gitmaster/recent.json"))
        );
        assert_eq!(
            data_file(Some("/data".into()), Some("/Users/u".into())),
            Some(PathBuf::from("/data/gitmaster/recent.json"))
        );
    }

    #[test]
    fn no_home_means_no_file() {
        assert_eq!(data_file(None, None), None);
    }

    #[test]
    fn a_missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("recent.json"));
        assert_eq!(store.load(), Recents::default());
    }

    #[test]
    fn a_corrupt_file_on_disk_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("recent.json"), "[[[").unwrap();
        assert_eq!(
            Store::new(dir.path().join("recent.json")).load(),
            Recents::default()
        );
    }

    #[test]
    fn save_creates_the_directory_and_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nested").join("recent.json"));
        let mut recents = Recents::default();
        recents.push(p("a"));
        store.save(&recents, 1).unwrap();
        assert_eq!(store.load(), recents);
        assert!(!temp_path(&store.path).exists());
    }

    #[test]
    fn an_older_save_does_not_replace_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("recent.json"));
        let mut newer = Recents::default();
        newer.push(p("new"));
        let mut older = Recents::default();
        older.push(p("old"));
        store.save(&newer, 2).unwrap();
        store.save(&older, 1).unwrap();
        assert_eq!(store.load(), newer);
    }
}
