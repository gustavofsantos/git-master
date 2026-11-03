//! The app's preferences, kept in Git's configuration (docs/reference/configuration.md).
//!
//! Every preference is a `gitmaster.*` key, so the base values live in `~/.gitconfig`, can be
//! tracked with the rest of the user's dotfiles, and a repository overrides them in its own
//! `.git/config` the way Git's own options are overridden. Reads go through `git config`, so Git's
//! layers and `include` rules apply: system, global, repository, worktree.
//!
//! | Key | Meaning |
//! |---|---|
//! | `gitmaster.diffLayout` | `unified` (default) or `split` |
//! | `gitmaster.authorPictures` | boolean, default off |
//! | `gitmaster.threads.capture` | boolean, default on |
//! | `gitmaster.threads.maxResultBytes` | integer |
//! | `gitmaster.threads.redactBuiltin` | boolean, default on |
//! | `gitmaster.threads.redactDisabledBuiltin` | multi-valued: names of built-in patterns to turn off |
//! | `gitmaster.threads.redactPath` | multi-valued: globs whose content is never stored |
//! | `gitmaster-redact.<name>.regex`, `.group` | a user redaction pattern |
//!
//! A multi-valued key adds up across layers, as Git's do. An empty value clears what the layers
//! below it set, so a repository can remove a global entry. A pattern with an empty `regex` is
//! dropped for the same reason.
//!
//! The preferences used to live in `settings.json`. `migrate_legacy` moves them into the global
//! configuration once and sets the file aside.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::threads::config::{ThreadsConfig, UserPattern};

/// The layout of the commit diff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffLayout {
    #[default]
    Unified,
    Split,
}

/// The preferences the app keeps. Each field has its default when no layer sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub diff_layout: DiffLayout,
    /// Whether author pictures are looked up on Gravatar. Off until chosen, because a lookup
    /// sends a hash of the author's email to a third party.
    pub author_pictures: bool,
    pub threads: ThreadsConfig,
}

/// Which Git configuration the commands read and write.
#[derive(Clone, Debug, Default)]
pub struct Source {
    /// Read and write this file as the global configuration, and ignore the system one. For the
    /// tests, which must not see the user's own settings. `None` is the real configuration.
    pub global: Option<PathBuf>,
}

impl Source {
    fn git(&self, dir: Option<&Path>) -> Command {
        let mut command = Command::new("git");
        if let Some(dir) = dir.filter(|dir| dir.is_dir()) {
            command.current_dir(dir);
        }
        if let Some(global) = &self.global {
            command
                .env("GIT_CONFIG_GLOBAL", global)
                .env("GIT_CONFIG_NOSYSTEM", "1");
        }
        command
    }
}

impl Settings {
    /// The settings that apply in `dir`: Git's layers read from there, so a repository's own
    /// values override the global ones. A `dir` that is not a repository, or a Git that cannot
    /// run, reads as the global values, or the defaults.
    #[must_use]
    pub fn load(dir: &Path, source: &Source) -> Self {
        Self::from_entries(&list(source, Some(dir), &["--includes"]))
    }

    /// The settings of the global configuration alone.
    #[must_use]
    pub fn load_global(source: &Source) -> Self {
        Self::from_entries(&list(source, None, &["--global"]))
    }

    /// The settings the entries of `git config --list` give, in order: a later entry overrides
    /// an earlier one. Keys are as Git prints them, with the section and the variable in lower
    /// case.
    #[must_use]
    pub fn from_entries(entries: &[Entry]) -> Self {
        let mut settings = Self::default();
        let mut disabled: Option<Vec<String>> = None;
        let mut paths: Option<Vec<String>> = None;
        let mut patterns: Vec<(String, String, usize)> = Vec::new();
        for entry in entries {
            let value = entry.value.as_deref();
            match entry.key.as_str() {
                "gitmaster.difflayout" => {
                    settings.diff_layout = match value {
                        Some(v) if v.eq_ignore_ascii_case("split") => DiffLayout::Split,
                        _ => DiffLayout::Unified,
                    };
                }
                "gitmaster.authorpictures" => {
                    settings.author_pictures = boolean(value, settings.author_pictures);
                }
                "gitmaster.threads.capture" => {
                    settings.threads.capture = boolean(value, settings.threads.capture);
                }
                "gitmaster.threads.maxresultbytes" => {
                    if let Some(bytes) = value.and_then(|v| v.trim().parse().ok()) {
                        settings.threads.max_result_bytes = bytes;
                    }
                }
                "gitmaster.threads.redactbuiltin" => {
                    settings.threads.redact.builtin =
                        boolean(value, settings.threads.redact.builtin);
                }
                "gitmaster.threads.redactdisabledbuiltin" => {
                    push(&mut disabled, value.unwrap_or_default());
                }
                "gitmaster.threads.redactpath" => push(&mut paths, value.unwrap_or_default()),
                key => {
                    let Some((name, variable)) = key
                        .strip_prefix("gitmaster-redact.")
                        .and_then(|rest| rest.rsplit_once('.'))
                    else {
                        continue;
                    };
                    let slot = patterns
                        .iter()
                        .position(|(n, _, _)| n == name)
                        .unwrap_or_else(|| {
                            patterns.push((name.to_owned(), String::new(), 0));
                            patterns.len() - 1
                        });
                    match variable {
                        "regex" => value.unwrap_or_default().clone_into(&mut patterns[slot].1),
                        "group" => {
                            if let Some(group) = value.and_then(|v| v.trim().parse().ok()) {
                                patterns[slot].2 = group;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(disabled) = disabled {
            settings.threads.redact.disabled_builtins = disabled;
        }
        if let Some(paths) = paths {
            settings.threads.redact.paths = paths;
        }
        settings.threads.redact.patterns = patterns
            .into_iter()
            .filter(|(_, regex, _)| !regex.is_empty())
            .map(|(name, regex, group)| UserPattern { name, regex, group })
            .collect();
        settings
    }

    /// The settings of a legacy `settings.json` document. Anything of the wrong shape takes its
    /// default.
    #[must_use]
    pub fn from_legacy_json(text: &str) -> Self {
        let Ok(document @ Value::Object(_)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        Self {
            diff_layout: match document.get("diff_layout").and_then(Value::as_str) {
                Some("split") => DiffLayout::Split,
                _ => DiffLayout::Unified,
            },
            author_pictures: document
                .get("author_pictures")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            threads: ThreadsConfig::from_settings(&document),
        }
    }

    /// Writes into the global configuration whatever differs from what it holds now, and
    /// removes the keys that are back at their default, so `~/.gitconfig` stays small. A
    /// repository's own overrides are not touched.
    pub fn save_global(&self, source: &Source) -> io::Result<()> {
        let current = Self::load_global(source);
        let defaults = Self::default();
        let (new, old, def) = (&self.threads, &current.threads, &defaults.threads);

        if self.diff_layout != current.diff_layout {
            let value = (self.diff_layout != defaults.diff_layout).then_some("split");
            set(source, "gitmaster.diffLayout", value)?;
        }
        if self.author_pictures != current.author_pictures {
            let value = (self.author_pictures != defaults.author_pictures).then(|| flag(true));
            set(source, "gitmaster.authorPictures", value)?;
        }
        if new.capture != old.capture {
            let value = (new.capture != def.capture).then(|| flag(new.capture));
            set(source, "gitmaster.threads.capture", value)?;
        }
        if new.max_result_bytes != old.max_result_bytes {
            let value = (new.max_result_bytes != def.max_result_bytes)
                .then(|| new.max_result_bytes.to_string());
            set(source, "gitmaster.threads.maxResultBytes", value.as_deref())?;
        }
        if new.redact.builtin != old.redact.builtin {
            let value =
                (new.redact.builtin != def.redact.builtin).then(|| flag(new.redact.builtin));
            set(source, "gitmaster.threads.redactBuiltin", value)?;
        }
        if new.redact.disabled_builtins != old.redact.disabled_builtins {
            set_list(
                source,
                "gitmaster.threads.redactDisabledBuiltin",
                &new.redact.disabled_builtins,
                &def.redact.disabled_builtins,
            )?;
        }
        if new.redact.paths != old.redact.paths {
            set_list(
                source,
                "gitmaster.threads.redactPath",
                &new.redact.paths,
                &def.redact.paths,
            )?;
        }
        if new.redact.patterns != old.redact.patterns {
            set_patterns(source, &new.redact.patterns)?;
        }
        Ok(())
    }
}

/// One line of `git config --list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// `None` for a key written without `=`, which Git reads as true.
    pub value: Option<String>,
}

/// Git's configuration as entries, in the order Git applies them. Empty when Git fails: a
/// missing file, a directory that is not a repository and a missing `git` all read as "nothing
/// set".
fn list(source: &Source, dir: Option<&Path>, scope: &[&str]) -> Vec<Entry> {
    let output = source
        .git(dir)
        .arg("config")
        .args(scope)
        .args(["--null", "--list"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_list(&output.stdout)
}

/// The entries of the `--null --list` output: `key\nvalue\0`, or `key\0` without a value.
fn parse_list(raw: &[u8]) -> Vec<Entry> {
    raw.split(|&byte| byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            let text = String::from_utf8_lossy(chunk);
            match text.split_once('\n') {
                Some((key, value)) => Entry {
                    key: key.to_owned(),
                    value: Some(value.to_owned()),
                },
                None => Entry {
                    key: text.into_owned(),
                    value: None,
                },
            }
        })
        .collect()
}

/// Git's boolean: `true`, `yes`, `on`, a non-zero number or no value; `false`, `no`, `off`, `0`.
/// Anything else keeps `current`.
fn boolean(value: Option<&str>, current: bool) -> bool {
    let Some(value) = value else {
        return true;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "" => true,
        "false" | "no" | "off" | "0" => false,
        other => other.parse::<i64>().map_or(current, |n| n != 0),
    }
}

/// Adds `value` to a multi-valued key; an empty value clears what came before.
fn push(slot: &mut Option<Vec<String>>, value: &str) {
    if value.is_empty() {
        *slot = Some(Vec::new());
    } else {
        slot.get_or_insert_with(Vec::new).push(value.to_owned());
    }
}

fn flag(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

/// Sets `key` in the global configuration, or removes it when `value` is `None`.
fn set(source: &Source, key: &str, value: Option<&str>) -> io::Result<()> {
    let mut command = source.git(None);
    command.args(["config", "--global"]);
    match value {
        Some(value) => command.args(["--replace-all", key, value]),
        None => command.args(["--unset-all", key]),
    };
    run(&mut command, &[0, 5])
}

/// Rewrites a multi-valued key. The default list is not stored. An empty list where the default
/// is not empty is stored as one empty value, which is how the layers read "none".
fn set_list(source: &Source, key: &str, values: &[String], default: &[String]) -> io::Result<()> {
    set(source, key, None)?;
    if values == default {
        return Ok(());
    }
    let shown: &[String] = if values.is_empty() {
        &[String::new()]
    } else {
        values
    };
    for value in shown {
        let mut command = source.git(None);
        command.args(["config", "--global", "--add", key, value]);
        run(&mut command, &[0])?;
    }
    Ok(())
}

/// Replaces every `[gitmaster-redact "<name>"]` section of the global configuration.
fn set_patterns(source: &Source, patterns: &[UserPattern]) -> io::Result<()> {
    let names: BTreeSet<String> = list(source, None, &["--global"])
        .into_iter()
        .filter_map(|entry| {
            let rest = entry.key.strip_prefix("gitmaster-redact.")?;
            rest.rsplit_once('.').map(|(name, _)| name.to_owned())
        })
        .collect();
    for name in names {
        let mut command = source.git(None);
        command.args(["config", "--global", "--remove-section"]);
        command.arg(format!("gitmaster-redact.{name}"));
        // Git answers 128 for a section that is already gone.
        run(&mut command, &[0, 128])?;
    }
    for pattern in patterns {
        let prefix = format!("gitmaster-redact.{}", pattern.name);
        let mut regex = source.git(None);
        regex.args([
            "config",
            "--global",
            &format!("{prefix}.regex"),
            &pattern.regex,
        ]);
        run(&mut regex, &[0])?;
        if pattern.group != 0 {
            let mut group = source.git(None);
            group.args([
                "config",
                "--global",
                &format!("{prefix}.group"),
                &pattern.group.to_string(),
            ]);
            run(&mut group, &[0])?;
        }
    }
    Ok(())
}

fn run(command: &mut Command, accepted: &[i32]) -> io::Result<()> {
    let output = command.output()?;
    if output
        .status
        .code()
        .is_some_and(|code| accepted.contains(&code))
    {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "git config: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// What `migrate_legacy` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Migration {
    /// There was no `settings.json`.
    NothingToDo,
    /// The values were written to the global configuration and the file was set aside.
    Moved { file: PathBuf },
}

/// Moves the preferences of the legacy `settings.json` at `path` into the global configuration,
/// then renames the file to `settings.json.migrated`, so it is no longer read. A key the global
/// configuration already sets is left as it is, and a value equal to the default is not written.
/// Safe to run again, and by two processes at once: the second finds no file.
pub fn migrate_legacy(path: &Path, source: &Source) -> io::Result<Migration> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Migration::NothingToDo);
    };
    let legacy = Settings::from_legacy_json(&text);
    let global = list(source, None, &["--global"]);
    let has = |key: &str| global.iter().any(|entry| entry.key == key);
    let mut merged = Settings::from_entries(&global);
    if !has("gitmaster.difflayout") {
        merged.diff_layout = legacy.diff_layout;
    }
    if !has("gitmaster.authorpictures") {
        merged.author_pictures = legacy.author_pictures;
    }
    if !has("gitmaster.threads.capture") {
        merged.threads.capture = legacy.threads.capture;
    }
    if !has("gitmaster.threads.maxresultbytes") {
        merged.threads.max_result_bytes = legacy.threads.max_result_bytes;
    }
    if !has("gitmaster.threads.redactbuiltin") {
        merged.threads.redact.builtin = legacy.threads.redact.builtin;
    }
    if !has("gitmaster.threads.redactdisabledbuiltin") {
        merged.threads.redact.disabled_builtins = legacy.threads.redact.disabled_builtins;
    }
    if !has("gitmaster.threads.redactpath") {
        merged.threads.redact.paths = legacy.threads.redact.paths;
    }
    if !global
        .iter()
        .any(|entry| entry.key.starts_with("gitmaster-redact."))
    {
        merged.threads.redact.patterns = legacy.threads.redact.patterns;
    }
    merged.save_global(source)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".migrated");
    let file = path.with_file_name(name);
    std::fs::rename(path, &file)?;
    Ok(Migration::Moved { file })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<Entry> {
        text.lines()
            .map(|line| match line.split_once('=') {
                Some((key, value)) => Entry {
                    key: key.to_owned(),
                    value: Some(value.to_owned()),
                },
                None => Entry {
                    key: line.to_owned(),
                    value: None,
                },
            })
            .collect()
    }

    #[test]
    fn nothing_set_reads_as_the_defaults() {
        assert_eq!(Settings::from_entries(&[]), Settings::default());
        assert_eq!(
            Settings::from_entries(&entries("user.name=x\ncore.editor=vi")),
            Settings::default()
        );
    }

    #[test]
    fn scalars_are_read_and_the_last_layer_wins() {
        let s = Settings::from_entries(&entries(
            "gitmaster.difflayout=split\ngitmaster.authorpictures=yes\n\
             gitmaster.threads.capture=false\ngitmaster.threads.maxresultbytes=1024\n\
             gitmaster.threads.redactbuiltin=off\ngitmaster.difflayout=unified",
        ));
        assert_eq!(s.diff_layout, DiffLayout::Unified);
        assert!(s.author_pictures);
        assert!(!s.threads.capture);
        assert_eq!(s.threads.max_result_bytes, 1024);
        assert!(!s.threads.redact.builtin);
    }

    #[test]
    fn a_key_without_a_value_is_true_and_a_bad_value_keeps_the_default() {
        let s = Settings::from_entries(&entries(
            "gitmaster.authorpictures\ngitmaster.threads.maxresultbytes=lots\n\
             gitmaster.threads.capture=maybe",
        ));
        assert!(s.author_pictures);
        assert_eq!(
            s.threads.max_result_bytes,
            ThreadsConfig::default().max_result_bytes
        );
        assert!(s.threads.capture);
    }

    #[test]
    fn lists_add_up_and_an_empty_value_clears_them() {
        let s = Settings::from_entries(&entries(
            "gitmaster.threads.redactpath=a\ngitmaster.threads.redactpath=b\n\
             gitmaster.threads.redactdisabledbuiltin=jwt",
        ));
        assert_eq!(s.threads.redact.paths, ["a", "b"]);
        assert_eq!(s.threads.redact.disabled_builtins, ["jwt"]);
        let cleared = Settings::from_entries(&entries(
            "gitmaster.threads.redactpath=a\ngitmaster.threads.redactpath=\n\
             gitmaster.threads.redactpath=c",
        ));
        assert_eq!(cleared.threads.redact.paths, ["c"]);
        let none = Settings::from_entries(&entries("gitmaster.threads.redactpath="));
        assert!(none.threads.redact.paths.is_empty());
    }

    #[test]
    fn patterns_are_collected_by_name_and_an_empty_regex_drops_one() {
        let s = Settings::from_entries(&entries(
            "gitmaster-redact.acme.regex=acme_[a-z]+\ngitmaster-redact.acme.group=1\n\
             gitmaster-redact.db.pw.regex=(PW=)(.+)\ngitmaster-redact.db.pw.group=2\n\
             gitmaster-redact.old.regex=x\ngitmaster-redact.old.regex=",
        ));
        assert_eq!(
            s.threads.redact.patterns,
            [
                UserPattern {
                    name: "acme".into(),
                    regex: "acme_[a-z]+".into(),
                    group: 1
                },
                UserPattern {
                    name: "db.pw".into(),
                    regex: "(PW=)(.+)".into(),
                    group: 2
                },
            ]
        );
    }

    #[test]
    fn the_null_list_output_is_split_into_entries() {
        let parsed = parse_list(b"a.b\nc\nd\0e.f\0g.h\n\0");
        assert_eq!(
            parsed,
            [
                Entry {
                    key: "a.b".into(),
                    value: Some("c\nd".into())
                },
                Entry {
                    key: "e.f".into(),
                    value: None
                },
                Entry {
                    key: "g.h".into(),
                    value: Some(String::new())
                },
            ]
        );
    }

    #[test]
    fn a_legacy_document_is_read_with_defaults_for_the_wrong_shape() {
        let s = Settings::from_legacy_json(
            r#"{"diff_layout":"split","author_pictures":true,"threads":{"capture":false}}"#,
        );
        assert_eq!(s.diff_layout, DiffLayout::Split);
        assert!(s.author_pictures);
        assert!(!s.threads.capture);
        assert_eq!(Settings::from_legacy_json("[1]"), Settings::default());
        assert_eq!(Settings::from_legacy_json("{ no"), Settings::default());
    }
}
