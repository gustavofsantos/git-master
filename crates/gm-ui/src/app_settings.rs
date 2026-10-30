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

use serde_json::{Map, Value, json};

use gm_core::threads::config::{RedactConfig, ThreadsConfig, UserPattern};

use crate::diff_rows::Layout;

/// The preferences the app keeps. Each field has its default when unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppSettings {
    /// The layout of the read-only commit diff (D39). Unified by default.
    pub diff_layout: Layout,
    /// Whether author pictures are looked up on Gravatar (D110). Off until chosen,
    /// because a lookup sends a hash of the author's email to a third party.
    pub author_pictures: bool,
    /// Capture and redaction of threads, the `threads` object (configuration.md).
    pub threads: ThreadsConfig,
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
        let author_pictures = fields
            .get("author_pictures")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let threads = ThreadsConfig::from_settings(&Value::Object(fields));
        Self {
            diff_layout,
            author_pictures,
            threads,
        }
    }

    /// The file's text once this app's settings are written into `existing`.
    /// The keys this app owns are replaced; every other key of the object is
    /// kept as it is. Text that is not a JSON object (or no text) starts from an
    /// empty object, so a corrupt file is replaced by a valid one.
    #[must_use]
    pub fn merge_into(&self, existing: &str) -> String {
        let mut fields = match serde_json::from_str::<Value>(existing) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        };
        let diff_layout = match self.diff_layout {
            Layout::Unified => "unified",
            Layout::Split => "split",
        };
        fields.insert("diff_layout".to_owned(), json!(diff_layout));
        // The branch scope was a preference once; an old file loses the key.
        fields.remove("branch_scope");
        fields.insert("author_pictures".to_owned(), json!(self.author_pictures));
        let threads = threads_value(fields.get("threads"), &self.threads);
        fields.insert("threads".to_owned(), threads);
        // An object of JSON values always serializes.
        serde_json::to_string_pretty(&Value::Object(fields)).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// The `threads` object once `config` is written into `existing`. The keys and
/// the redaction fields this app reads are replaced. Items of a list that the
/// app does not read as a pattern, path or built-in name are kept after the
/// ones it writes, and so is every other key, inside `threads`, `threads.redact`
/// and each pattern object (matched by name).
fn threads_value(existing: Option<&Value>, config: &ThreadsConfig) -> Value {
    let mut threads = object_of(existing);
    threads.insert("capture".to_owned(), json!(config.capture));
    threads.insert(
        "max_result_bytes".to_owned(),
        json!(config.max_result_bytes),
    );
    let redact = redact_value(threads.get("redact"), &config.redact);
    threads.insert("redact".to_owned(), Value::Object(redact));
    Value::Object(threads)
}

fn redact_value(existing: Option<&Value>, redact: &RedactConfig) -> Map<String, Value> {
    let mut object = object_of(existing);
    object.insert("builtin".to_owned(), json!(redact.builtin));
    let disabled = kept_items(existing, "disabled_builtins", Value::is_string, |items| {
        items.extend(redact.disabled_builtins.iter().map(|name| json!(name)));
    });
    object.insert("disabled_builtins".to_owned(), Value::Array(disabled));
    let patterns = pattern_values(existing, &redact.patterns);
    object.insert("patterns".to_owned(), Value::Array(patterns));
    let paths = kept_items(existing, "paths", Value::is_string, |items| {
        items.extend(redact.paths.iter().map(|path| json!(path)));
    });
    object.insert("paths".to_owned(), Value::Array(paths));
    object
}

/// The pattern objects of `patterns`, each with the extra keys of the object of the
/// same name already in the file. Items that are not patterns are kept at the end.
fn pattern_values(existing: Option<&Value>, patterns: &[UserPattern]) -> Vec<Value> {
    let old = existing
        .and_then(|value| value.get("patterns"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut values: Vec<Value> = patterns
        .iter()
        .map(|pattern| {
            let mut object = old
                .iter()
                .filter_map(Value::as_object)
                .find(|object| object.get("name").and_then(Value::as_str) == Some(&pattern.name))
                .cloned()
                .unwrap_or_default();
            object.insert("name".to_owned(), json!(pattern.name));
            object.insert("regex".to_owned(), json!(pattern.regex));
            object.insert("group".to_owned(), json!(pattern.group));
            Value::Object(object)
        })
        .collect();
    values.extend(old.iter().filter(|item| !is_pattern(item)).cloned());
    values
}

/// A pattern object as the config reader takes it: a string name and regex.
fn is_pattern(value: &Value) -> bool {
    value.get("name").is_some_and(Value::is_string)
        && value.get("regex").is_some_and(Value::is_string)
}

/// The items of the array `key` of `existing` that `is_ours` does not claim, then
/// the items `ours` adds. Used for lists whose claimed items the app rewrites.
fn kept_items(
    existing: Option<&Value>,
    key: &str,
    is_ours: fn(&Value) -> bool,
    ours: impl FnOnce(&mut Vec<Value>),
) -> Vec<Value> {
    let mut items = Vec::new();
    ours(&mut items);
    if let Some(old) = existing
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
    {
        items.extend(old.iter().filter(|item| !is_ours(item)).cloned());
    }
    items
}

/// The object of `value`, or an empty one when it is missing or not an object.
fn object_of(value: Option<&Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(object)) => object.clone(),
        _ => Map::new(),
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

    /// The folder of the settings file, where the app keeps its other files.
    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.path.parent()
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
    pub fn save(&self, settings: &AppSettings, generation: u64) -> io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if generation <= *written {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Read and write under the lock, so two saves cannot merge the same base.
        let existing = fs::read_to_string(&self.path).unwrap_or_default();
        let text = settings.merge_into(&existing);
        let temp = temp_path(&self.path);
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
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
            author_pictures: false,
            threads: ThreadsConfig::default(),
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

    fn written(settings: &AppSettings, existing: &str) -> Value {
        serde_json::from_str(&settings.merge_into(existing)).expect("valid JSON")
    }

    #[test]
    fn author_pictures_are_off_until_chosen_and_round_trip() {
        assert!(!AppSettings::default().author_pictures);
        let on = AppSettings {
            author_pictures: true,
            ..split()
        };
        assert_eq!(AppSettings::parse(&on.merge_into("")), on);
        assert!(!AppSettings::parse("{\"author_pictures\": \"yes\"}").author_pictures);
    }

    #[test]
    fn the_defaults_are_unified() {
        assert_eq!(AppSettings::default().diff_layout, Layout::Unified);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let unified = AppSettings::default();
        assert_eq!(AppSettings::parse(&unified.merge_into("")), unified);
        assert_eq!(AppSettings::parse(&split().merge_into("")), split());
    }

    #[test]
    fn the_retired_branch_scope_key_is_dropped_on_save() {
        let text = split().merge_into(r#"{"branch_scope":"all","keep":1}"#);
        assert!(!text.contains("branch_scope"));
        assert!(text.contains("keep"));
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
    fn a_save_keeps_the_keys_it_does_not_own() {
        let path = scratch("unknown-key");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"theme":"dark","window":{"width":900}}"#).unwrap();
        Store::new(path.clone()).save(&split(), 1).unwrap();
        let text: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(text["theme"], "dark");
        assert_eq!(text["window"]["width"], 900);
        assert_eq!(text["diff_layout"], "split");
        assert_eq!(Store::new(path).load(), split());
    }

    #[test]
    fn a_save_over_a_corrupt_file_writes_a_valid_one() {
        let path = scratch("save-corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        Store::new(path.clone()).save(&split(), 1).unwrap();
        assert_eq!(Store::new(path).load(), split());
    }

    #[test]
    fn save_creates_the_directory_and_loads_back_without_a_temp_file() {
        let path = scratch("round");
        let store = Store::new(path.clone());
        store.save(&split(), 1).unwrap();
        assert_eq!(Store::new(path.clone()).load(), split());
        assert!(!temp_path(&path).exists());
    }

    #[test]
    fn an_older_save_does_not_replace_a_newer_one() {
        let path = scratch("generation");
        let store = Store::new(path.clone());
        store.save(&split(), 2).unwrap();
        store.save(&AppSettings::default(), 1).unwrap();
        assert_eq!(Store::new(path).load(), split());
    }

    #[test]
    fn the_threads_object_round_trips_and_defaults_when_missing() {
        let on = AppSettings {
            threads: threads_config(),
            ..split()
        };
        assert_eq!(
            AppSettings::parse(&on.merge_into("")).threads,
            threads_config()
        );
        assert_eq!(AppSettings::parse("{}").threads, ThreadsConfig::default());
    }

    #[test]
    fn a_save_keeps_unknown_keys_inside_threads_and_redact_and_pattern_objects() {
        let existing = r#"{"threads":{"future":1,"capture":true,"redact":{"note":"x","builtin":true,"paths":[],"patterns":[{"name":"acme-token","regex":"old","group":0,"comment":"keep me"}]}}}"#;
        let settings = AppSettings {
            threads: threads_config(),
            ..split()
        };
        let text = written(&settings, existing);
        assert_eq!(text["threads"]["future"], 1);
        assert_eq!(text["threads"]["redact"]["note"], "x");
        assert_eq!(text["threads"]["capture"], false);
        assert_eq!(
            text["threads"]["redact"]["patterns"][0]["regex"],
            "acme_[a-z]+"
        );
        assert_eq!(
            text["threads"]["redact"]["patterns"][0]["comment"],
            "keep me"
        );
        assert_eq!(
            AppSettings::parse(&settings.merge_into(existing)).threads,
            threads_config()
        );
    }

    #[test]
    fn items_the_reader_skips_are_kept_after_the_written_ones() {
        let existing = r#"{"threads":{"redact":{"disabled_builtins":[3,"jwt"],"paths":[".env",false],"patterns":["text",{"name":"no-regex"}]}}}"#;
        let settings = AppSettings {
            threads: ThreadsConfig::default(),
            ..split()
        };
        let text = written(&settings, existing);
        assert_eq!(text["threads"]["redact"]["disabled_builtins"], json!([3]));
        assert_eq!(
            text["threads"]["redact"]["paths"],
            json!([".env", ".env.*", "*.pem", "id_rsa*", false])
        );
        assert_eq!(
            text["threads"]["redact"]["patterns"],
            json!(["text", {"name": "no-regex"}])
        );
        assert_eq!(
            AppSettings::parse(&settings.merge_into(existing)).threads,
            ThreadsConfig::default()
        );
    }

    #[test]
    fn a_removed_pattern_is_dropped_and_a_mistyped_threads_key_is_replaced() {
        let settings = AppSettings {
            threads: ThreadsConfig::default(),
            ..split()
        };
        let existing =
            r#"{"threads":{"redact":{"patterns":[{"name":"gone","regex":"x","extra":1}]}}}"#;
        assert_eq!(
            written(&settings, existing)["threads"]["redact"]["patterns"],
            json!([])
        );
        let text = written(&settings, r#"{"threads":"on","theme":"dark"}"#);
        assert_eq!(text["threads"]["capture"], true);
        assert_eq!(text["theme"], "dark");
    }

    #[test]
    fn a_save_to_disk_keeps_the_threads_keys_it_does_not_own() {
        let path = scratch("threads-keys");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"threads":{"future":true},"window":{"width":900}}"#,
        )
        .unwrap();
        let settings = AppSettings {
            threads: threads_config(),
            ..split()
        };
        Store::new(path.clone()).save(&settings, 1).unwrap();
        let text: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(text["threads"]["future"], true);
        assert_eq!(text["window"]["width"], 900);
        assert_eq!(Store::new(path).load().threads, threads_config());
    }
}
