//! The app's own preferences, which are not Git's (D40). The diff layout is the
//! first. The file lives in the user's config directory, never inside a
//! repository (Principle 5). A missing, unreadable or corrupt file reads as the
//! defaults. The file access takes its paths as arguments; callers run
//! `Store::save` on a background task, as the recent list does.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};

use crate::diff_rows::Layout;

/// The preferences the app keeps. Each field has its default when unset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppSettings {
    /// The layout of the read-only commit diff (D39). Unified by default.
    pub diff_layout: Layout,
}

impl AppSettings {
    /// The settings in `text`. Anything that is not a JSON object reads as the
    /// defaults, and a field with an unknown value reads as its default.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let diff_layout = match fields.get("diff_layout").and_then(Value::as_str) {
            Some("split") => Layout::Split,
            _ => Layout::Unified,
        };
        Self { diff_layout }
    }

    #[must_use]
    pub fn to_json(self) -> String {
        let diff_layout = match self.diff_layout {
            Layout::Unified => "unified",
            Layout::Split => "split",
        };
        // An object of strings always serializes.
        serde_json::to_string_pretty(&json!({ "diff_layout": diff_layout }))
            .unwrap_or_else(|_| "{}".to_owned())
    }
}

/// The file the settings are kept in, under the given environment values.
/// Pure, so both platforms' layouts can be tested on any host. On Linux it is
/// `$XDG_CONFIG_HOME/gitmaster/settings.json` (D38), or `~/.config` when that is
/// unset, empty or relative. On macOS it is under `Library/Application Support`,
/// as the other app files are. Without a home directory there is no file.
#[must_use]
pub fn settings_file(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let absolute = |value: Option<OsString>| {
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
    Some(base.join("gitmaster").join("settings.json"))
}

/// The file for this process, from the real environment.
#[must_use]
pub fn default_file() -> Option<PathBuf> {
    settings_file(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

/// The settings file. Saves are written in the order they were asked for: a
/// save older than the one on disk is skipped.
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

    /// The saved settings, or the defaults when the file is missing or
    /// unreadable.
    #[must_use]
    pub fn load(&self) -> AppSettings {
        fs::read_to_string(&self.path)
            .map(|text| AppSettings::parse(&text))
            .unwrap_or_default()
    }

    /// Writes `settings` as generation `generation` (a larger number is newer).
    /// The file is replaced whole, through a temporary file, so a crash leaves
    /// the old settings or the new ones, never half of each.
    pub fn save(&self, settings: AppSettings, generation: u64) -> io::Result<()> {
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
            file.write_all(settings.to_json().as_bytes())?;
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

    fn split() -> AppSettings {
        AppSettings {
            diff_layout: Layout::Split,
        }
    }

    #[test]
    fn the_defaults_are_unified() {
        assert_eq!(AppSettings::default().diff_layout, Layout::Unified);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let unified = AppSettings::default();
        assert_eq!(AppSettings::parse(&unified.to_json()), unified);
        assert_eq!(AppSettings::parse(&split().to_json()), split());
    }

    #[test]
    fn a_corrupt_or_non_object_file_reads_as_the_defaults() {
        for text in ["", "{", "[\"split\"]", "42", "null"] {
            assert_eq!(AppSettings::parse(text), AppSettings::default(), "{text}");
        }
    }

    #[test]
    fn an_unknown_layout_value_reads_as_unified() {
        assert_eq!(
            AppSettings::parse(r#"{"diff_layout":"sideways"}"#),
            AppSettings::default()
        );
        assert_eq!(
            AppSettings::parse(r#"{"diff_layout":7}"#),
            AppSettings::default()
        );
        assert_eq!(AppSettings::parse("{}"), AppSettings::default());
    }

    #[test]
    fn settings_file_follows_xdg_config_then_home() {
        let path = |xdg: Option<&str>, home: Option<&str>| {
            settings_file(xdg.map(OsString::from), home.map(OsString::from), false)
        };
        assert_eq!(
            path(Some("/xdg"), Some("/home/u")),
            Some(PathBuf::from("/xdg/gitmaster/settings.json"))
        );
        assert_eq!(
            path(None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/gitmaster/settings.json"))
        );
    }

    #[test]
    fn a_relative_or_empty_xdg_config_value_is_ignored() {
        let path = |xdg: &str| {
            settings_file(
                Some(OsString::from(xdg)),
                Some(OsString::from("/home/u")),
                false,
            )
        };
        assert_eq!(
            path("relative"),
            Some(PathBuf::from("/home/u/.config/gitmaster/settings.json"))
        );
        assert_eq!(
            path(""),
            Some(PathBuf::from("/home/u/.config/gitmaster/settings.json"))
        );
    }

    #[test]
    fn macos_uses_application_support() {
        assert_eq!(
            settings_file(None, Some(OsString::from("/Users/u")), true),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/gitmaster/settings.json"
            ))
        );
    }

    #[test]
    fn no_home_means_no_file() {
        assert_eq!(settings_file(None, None, false), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gm-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("gitmaster").join("settings.json")
    }

    #[test]
    fn a_missing_file_loads_as_the_defaults() {
        let store = Store::new(scratch("missing"));
        assert_eq!(store.load(), AppSettings::default());
    }

    #[test]
    fn a_corrupt_file_on_disk_loads_as_the_defaults() {
        let path = scratch("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Store::new(path).load(), AppSettings::default());
    }

    #[test]
    fn save_creates_the_directory_and_loads_back_without_a_temp_file() {
        let path = scratch("round");
        let store = Store::new(path.clone());
        store.save(split(), 1).unwrap();
        assert_eq!(Store::new(path.clone()).load(), split());
        assert!(!temp_path(&path).exists());
    }

    #[test]
    fn an_older_save_does_not_replace_a_newer_one() {
        let path = scratch("generation");
        let store = Store::new(path.clone());
        store.save(split(), 2).unwrap();
        store.save(AppSettings::default(), 1).unwrap();
        assert_eq!(Store::new(path).load(), split());
    }
}
