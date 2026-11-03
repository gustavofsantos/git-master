//! The app's own preferences, which are not Git's (D40), kept in Git's configuration as
//! `gitmaster.*` keys (D173). The base values are in `~/.gitconfig`, which the user can track
//! with their dotfiles, and a repository overrides them in `.git/config`. The reading and
//! writing rules are in `gm_core::settings`; this module adapts them to the window's types.
//!
//! The preferences used to be a JSON file in the user's config directory. It is moved into
//! `~/.gitconfig` the first time the app starts and renamed `settings.json.migrated`. Its
//! folder is still where the app keeps its other files, such as the avatar cache.
//! Callers run `Store::save` on a background task, as the recent list does.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use gm_core::settings::{self, DiffLayout, Settings, Source};
use gm_core::threads::config::ThreadsConfig;

use crate::diff_rows::Layout;

/// The preferences the app keeps. Each field has its default when unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppSettings {
    /// The layout of the read-only commit diff (D39). Unified by default.
    pub diff_layout: Layout,
    /// Whether author pictures are looked up on Gravatar (D110). Off until chosen,
    /// because a lookup sends a hash of the author's email to a third party.
    pub author_pictures: bool,
    /// Capture and redaction of threads (configuration.md).
    pub threads: ThreadsConfig,
}

impl From<Settings> for AppSettings {
    fn from(settings: Settings) -> Self {
        Self {
            diff_layout: match settings.diff_layout {
                DiffLayout::Unified => Layout::Unified,
                DiffLayout::Split => Layout::Split,
            },
            author_pictures: settings.author_pictures,
            threads: settings.threads,
        }
    }
}

impl From<&AppSettings> for Settings {
    fn from(app: &AppSettings) -> Self {
        Self {
            diff_layout: match app.diff_layout {
                Layout::Unified => DiffLayout::Unified,
                Layout::Split => DiffLayout::Split,
            },
            author_pictures: app.author_pictures,
            threads: app.threads.clone(),
        }
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

/// The legacy settings file, and the folder around it. The file is only read to be migrated.
/// Saves are written in the order they were asked for: a save older than the last one is
/// skipped.
pub struct Store {
    legacy: PathBuf,
    source: Source,
    /// The generation of the last save.
    written: Mutex<u64>,
}

impl Store {
    /// A store on the user's own Git configuration.
    #[must_use]
    pub fn new(legacy: PathBuf) -> Self {
        Self::with_source(legacy, Source::default())
    }

    /// A store on the Git configuration `source` names.
    #[must_use]
    pub fn with_source(legacy: PathBuf, source: Source) -> Self {
        Self {
            legacy,
            source,
            written: Mutex::new(0),
        }
    }

    /// The folder of the legacy file, where the app keeps its other files.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.legacy.parent()
    }

    /// The preferences that apply in `repo`: the repository's own `gitmaster.*` keys over the
    /// global ones. A legacy `settings.json` is moved into the global configuration first. A
    /// failure to do that is reported and leaves the file where it is.
    #[must_use]
    pub fn load(&self, repo: &Path) -> AppSettings {
        if let Err(error) = settings::migrate_legacy(&self.legacy, &self.source) {
            eprintln!("gitmaster: could not move the settings into ~/.gitconfig: {error}");
        }
        Settings::load(repo, &self.source).into()
    }

    /// Writes `settings` to the global configuration as generation `generation` (a larger
    /// number is newer). Only what differs from the file is written, and a value back at its
    /// default is removed, so `~/.gitconfig` holds only what was chosen.
    pub fn save(&self, settings: &AppSettings, generation: u64) -> io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if generation <= *written {
            return Ok(());
        }
        Settings::from(settings).save_global(&self.source)?;
        *written = generation;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gm_core::threads::config::{RedactConfig, UserPattern};

    use super::*;

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

    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("gm-settings-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        /// A store whose global Git configuration is a file in the scratch folder.
        fn store(&self) -> Store {
            Store::with_source(
                self.dir.join("gitmaster").join("settings.json"),
                Source {
                    global: Some(self.dir.join("gitconfig")),
                },
            )
        }

        fn gitconfig(&self) -> String {
            fs::read_to_string(self.dir.join("gitconfig")).unwrap_or_default()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn split() -> AppSettings {
        AppSettings {
            diff_layout: Layout::Split,
            ..AppSettings::default()
        }
    }

    fn threads_config() -> ThreadsConfig {
        ThreadsConfig {
            capture: false,
            max_result_bytes: 2048,
            redact: RedactConfig {
                builtin: true,
                disabled_builtins: vec!["jwt".into()],
                patterns: vec![UserPattern {
                    name: "acme-token".into(),
                    regex: "acme_[a-z]+".into(),
                    group: 0,
                }],
                paths: vec![".env".into(), "secrets/**".into()],
            },
        }
    }

    #[test]
    fn the_defaults_are_unified_diffs_and_no_pictures() {
        let defaults = AppSettings::default();
        assert_eq!(defaults.diff_layout, Layout::Unified);
        assert!(!defaults.author_pictures);
    }

    #[test]
    fn the_window_types_and_the_core_types_convert_both_ways() {
        let app = AppSettings {
            author_pictures: true,
            threads: threads_config(),
            ..split()
        };
        assert_eq!(AppSettings::from(Settings::from(&app)), app);
    }

    #[test]
    fn nothing_set_loads_as_the_defaults() {
        let scratch = Scratch::new("missing");
        assert_eq!(scratch.store().load(&scratch.dir), AppSettings::default());
    }

    #[test]
    fn a_save_goes_to_the_global_git_config_and_loads_back() {
        let scratch = Scratch::new("round");
        let store = scratch.store();
        let on = AppSettings {
            author_pictures: true,
            threads: threads_config(),
            ..split()
        };
        store.save(&on, 1).unwrap();
        assert!(scratch.gitconfig().contains("diffLayout = split"));
        assert_eq!(scratch.store().load(&scratch.dir), on);
        assert!(!scratch.dir.join("gitmaster").exists());
    }

    #[test]
    fn an_older_save_does_not_replace_a_newer_one() {
        let scratch = Scratch::new("generation");
        let store = scratch.store();
        store.save(&split(), 2).unwrap();
        store.save(&AppSettings::default(), 1).unwrap();
        assert_eq!(scratch.store().load(&scratch.dir), split());
    }

    #[test]
    fn a_legacy_settings_file_is_moved_into_the_git_config_on_load() {
        let scratch = Scratch::new("legacy");
        let legacy = scratch.dir.join("gitmaster").join("settings.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, r#"{"diff_layout":"split","theme":"dark"}"#).unwrap();
        assert_eq!(scratch.store().load(&scratch.dir), split());
        assert!(!legacy.exists());
        assert!(legacy.with_file_name("settings.json.migrated").exists());
        assert!(scratch.gitconfig().contains("diffLayout = split"));
    }

    #[test]
    fn a_repository_overrides_the_global_preference() {
        let scratch = Scratch::new("override");
        let repo = scratch.dir.join("repo");
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_GLOBAL", scratch.dir.join("gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        let store = scratch.store();
        store.save(&split(), 1).unwrap();
        assert_eq!(store.load(&repo).diff_layout, Layout::Split);
        git(&["config", "gitmaster.diffLayout", "unified"]);
        assert_eq!(store.load(&repo).diff_layout, Layout::Unified);
        assert_eq!(store.load(&scratch.dir).diff_layout, Layout::Split);
    }
}
