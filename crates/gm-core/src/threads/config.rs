//! The `threads` key of `settings.json` (docs/reference/configuration.md, design/threads.md D127).
//! Parsing is lenient: a missing or mistyped field takes its default, so a bad value never stops
//! capture by itself. Only `load` touches the file system.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The default largest tool result that is stored, in bytes.
const DEFAULT_MAX_RESULT_BYTES: usize = 65_536;

/// A user pattern from `threads.redact.patterns`, not yet compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPattern {
    pub name: String,
    pub regex: String,
    /// The capture group that holds the secret; `0` is the whole match.
    pub group: usize,
}

/// The `threads.redact` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedactConfig {
    /// Turns the built-in patterns on or off as a group.
    pub builtin: bool,
    pub disabled_builtins: Vec<String>,
    pub patterns: Vec<UserPattern>,
    /// Globs on repository-relative paths whose content is never stored.
    pub paths: Vec<String>,
}

/// The `threads` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadsConfig {
    pub capture: bool,
    pub max_result_bytes: usize,
    pub redact: RedactConfig,
}

impl Default for RedactConfig {
    fn default() -> Self {
        Self {
            builtin: true,
            disabled_builtins: Vec::new(),
            patterns: Vec::new(),
            paths: [".env", ".env.*", "*.pem", "id_rsa*"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self {
            capture: true,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            redact: RedactConfig::default(),
        }
    }
}

impl ThreadsConfig {
    /// Reads the `threads` key of a parsed settings document. Anything that is not the
    /// expected shape takes its default, and unknown keys are ignored.
    #[must_use]
    pub fn from_settings(settings: &Value) -> Self {
        let Some(threads) = settings.get("threads").and_then(Value::as_object) else {
            return Self::default();
        };
        let defaults = Self::default();
        Self {
            capture: threads
                .get("capture")
                .and_then(Value::as_bool)
                .unwrap_or(defaults.capture),
            max_result_bytes: threads
                .get("max_result_bytes")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(defaults.max_result_bytes),
            redact: threads
                .get("redact")
                .map_or_else(RedactConfig::default, parse_redact),
        }
    }

    /// Reads the settings file at `path`. A missing file or invalid JSON reads as the defaults.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(settings) => Self::from_settings(&settings),
            Err(_) => Self::default(),
        }
    }
}

fn parse_redact(value: &Value) -> RedactConfig {
    let defaults = RedactConfig::default();
    let Some(object) = value.as_object() else {
        return defaults;
    };
    RedactConfig {
        builtin: object
            .get("builtin")
            .and_then(Value::as_bool)
            .unwrap_or(defaults.builtin),
        disabled_builtins: object
            .get("disabled_builtins")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |items| strings(items)),
        patterns: object
            .get("patterns")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |items| {
                items.iter().filter_map(parse_pattern).collect()
            }),
        paths: object
            .get("paths")
            .and_then(Value::as_array)
            .map_or(defaults.paths, |items| strings(items)),
    }
}

/// A pattern object without a string `name` and `regex` is skipped.
fn parse_pattern(value: &Value) -> Option<UserPattern> {
    let object = value.as_object()?;
    Some(UserPattern {
        name: object.get("name")?.as_str()?.to_owned(),
        regex: object.get("regex")?.as_str()?.to_owned(),
        group: object
            .get("group")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
    })
}

/// The string items of an array; other items are skipped.
fn strings(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// The settings file of the app (docs/reference/configuration.md). `None` when no home
/// directory is known.
#[must_use]
pub fn settings_path() -> Option<PathBuf> {
    settings_path_from(|name| std::env::var_os(name).map(PathBuf::from))
}

/// `settings_path` with the environment passed in, so the lookup is testable without
/// changing the process environment.
fn settings_path_from(env: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let home = || env("HOME").filter(|p| !p.as_os_str().is_empty());
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support/gitmaster/settings.json"));
    }
    // Linux and other Unix systems: the XDG location when it is set to an absolute path.
    let xdg = env("XDG_CONFIG_HOME").filter(|p| p.is_absolute() && !p.as_os_str().is_empty());
    match xdg {
        Some(dir) => Some(dir.join("gitmaster/settings.json")),
        None => Some(home()?.join(".config/gitmaster/settings.json")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_match_the_documentation() {
        let config = ThreadsConfig::default();
        assert!(config.capture);
        assert_eq!(config.max_result_bytes, 65_536);
        assert!(config.redact.builtin);
        assert!(config.redact.disabled_builtins.is_empty());
        assert!(config.redact.patterns.is_empty());
        assert_eq!(config.redact.paths, [".env", ".env.*", "*.pem", "id_rsa*"]);
    }

    #[test]
    fn missing_key_reads_as_defaults() {
        assert_eq!(
            ThreadsConfig::from_settings(&json!({})),
            ThreadsConfig::default()
        );
        assert_eq!(
            ThreadsConfig::from_settings(&json!(null)),
            ThreadsConfig::default()
        );
        assert_eq!(
            ThreadsConfig::from_settings(&json!([1, 2])),
            ThreadsConfig::default()
        );
        assert_eq!(
            ThreadsConfig::from_settings(&json!({"threads": "on"})),
            ThreadsConfig::default()
        );
    }

    #[test]
    fn full_document_is_read() {
        let config = ThreadsConfig::from_settings(&json!({
            "threads": {
                "capture": false,
                "max_result_bytes": 1024,
                "redact": {
                    "builtin": false,
                    "disabled_builtins": ["jwt"],
                    "patterns": [{"name": "acme-token", "regex": "acme_[a-z]+", "group": 1}],
                    "paths": ["secrets/**"]
                }
            }
        }));
        assert!(!config.capture);
        assert_eq!(config.max_result_bytes, 1024);
        assert!(!config.redact.builtin);
        assert_eq!(config.redact.disabled_builtins, ["jwt"]);
        assert_eq!(
            config.redact.patterns,
            [UserPattern {
                name: "acme-token".into(),
                regex: "acme_[a-z]+".into(),
                group: 1,
            }]
        );
        assert_eq!(config.redact.paths, ["secrets/**"]);
    }

    #[test]
    fn mistyped_fields_take_their_defaults() {
        let config = ThreadsConfig::from_settings(&json!({
            "threads": {
                "capture": "yes",
                "max_result_bytes": -1,
                "redact": {
                    "builtin": 0,
                    "disabled_builtins": "jwt",
                    "patterns": "none",
                    "paths": 7
                }
            }
        }));
        assert_eq!(config, ThreadsConfig::default());
    }

    #[test]
    fn unknown_keys_are_ignored_and_bad_items_skipped() {
        let config = ThreadsConfig::from_settings(&json!({
            "theme": "dark",
            "threads": {
                "extra": true,
                "redact": {
                    "disabled_builtins": ["jwt", 3, null, "github-token"],
                    "patterns": [
                        {"name": "a", "regex": "b", "group": 2},
                        {"name": 5, "regex": "c"},
                        {"regex": "no name"},
                        {"name": "no-regex"},
                        "text",
                        {"name": "neg", "regex": "d", "group": -1}
                    ],
                    "paths": [".env", false]
                }
            }
        }));
        assert_eq!(config.redact.disabled_builtins, ["jwt", "github-token"]);
        assert_eq!(
            config.redact.patterns,
            [
                UserPattern {
                    name: "a".into(),
                    regex: "b".into(),
                    group: 2,
                },
                UserPattern {
                    name: "neg".into(),
                    regex: "d".into(),
                    group: 0,
                },
            ]
        );
        assert_eq!(config.redact.paths, [".env"]);
    }

    #[test]
    fn empty_path_list_is_kept() {
        let config = ThreadsConfig::from_settings(&json!({
            "threads": {"redact": {"paths": []}}
        }));
        assert!(config.redact.paths.is_empty());
    }

    #[test]
    fn load_reads_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"threads": {"capture": false, "max_result_bytes": 10}}"#,
        )
        .expect("write settings");
        let config = ThreadsConfig::load(&path);
        assert!(!config.capture);
        assert_eq!(config.max_result_bytes, 10);
    }

    #[test]
    fn load_of_missing_or_invalid_file_reads_as_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            ThreadsConfig::load(&dir.path().join("absent.json")),
            ThreadsConfig::default()
        );
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{ not json").expect("write settings");
        assert_eq!(ThreadsConfig::load(&broken), ThreadsConfig::default());
    }

    #[test]
    fn settings_path_uses_xdg_when_absolute() {
        let env = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some(PathBuf::from("/cfg")),
            "HOME" => Some(PathBuf::from("/home/u")),
            _ => None,
        };
        if cfg!(target_os = "macos") {
            return;
        }
        assert_eq!(
            settings_path_from(env),
            Some(PathBuf::from("/cfg/gitmaster/settings.json"))
        );
    }

    #[test]
    fn settings_path_falls_back_to_home() {
        let home_only = |name: &str| match name {
            "HOME" => Some(PathBuf::from("/home/u")),
            _ => None,
        };
        let relative_xdg = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some(PathBuf::from("relative")),
            "HOME" => Some(PathBuf::from("/home/u")),
            _ => None,
        };
        let empty_xdg = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some(PathBuf::new()),
            "HOME" => Some(PathBuf::from("/home/u")),
            _ => None,
        };
        let expected = if cfg!(target_os = "macos") {
            PathBuf::from("/home/u/Library/Application Support/gitmaster/settings.json")
        } else {
            PathBuf::from("/home/u/.config/gitmaster/settings.json")
        };
        assert_eq!(settings_path_from(home_only), Some(expected.clone()));
        assert_eq!(settings_path_from(relative_xdg), Some(expected.clone()));
        assert_eq!(settings_path_from(empty_xdg), Some(expected));
    }

    #[test]
    fn settings_path_needs_a_home() {
        assert_eq!(settings_path_from(|_| None), None);
    }
}
