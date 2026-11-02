//! Which workflows run in one repository (D164(1), `WF4b`). The switches are kept in
//! `.git/gitmaster/workflows.json` of that repository, never in the working tree. A user workflow
//! runs unless its name is in `disabled`; a built-in one runs only when its name is in
//! `enabled_builtin`. A missing file holds the defaults. The file is written through a temporary
//! file, so a crash leaves the old one or the new one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::loader::ScriptEntry;
use crate::GitPort;

const VERSION: u32 = 1;

/// The switches of one repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    /// User workflows switched off here, by name.
    pub disabled: BTreeSet<String>,
    /// Built-in workflows switched on here, by name.
    pub enabled_builtin: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    enabled_builtin: Vec<String>,
}

impl Switches {
    /// Whether the workflow `name` runs here. `builtin` says which of the two defaults applies.
    #[must_use]
    pub fn is_enabled(&self, name: &str, builtin: bool) -> bool {
        if builtin {
            self.enabled_builtin.contains(name)
        } else {
            !self.disabled.contains(name)
        }
    }

    /// Sets whether the workflow `name` runs here.
    pub fn set(&mut self, name: &str, builtin: bool, enabled: bool) {
        let set = if builtin {
            &mut self.enabled_builtin
        } else {
            &mut self.disabled
        };
        // A user workflow is recorded when it is off, a built-in one when it is on: the rest
        // are the defaults and need no entry.
        if enabled == builtin {
            set.insert(name.to_owned());
        } else {
            set.remove(name);
        }
    }

    /// The file text of these switches.
    #[must_use]
    pub fn to_text(&self) -> String {
        let store = Store {
            version: VERSION,
            disabled: self.disabled.iter().cloned().collect(),
            enabled_builtin: self.enabled_builtin.iter().cloned().collect(),
        };
        // A list of strings always serializes.
        serde_json::to_string_pretty(&store).unwrap_or_default()
    }

    /// The switches of a file text. An unknown version is refused, so an older app never reads
    /// switches it does not understand.
    pub fn from_text(text: &str) -> Result<Self, String> {
        let store: Store = serde_json::from_str(text)
            .map_err(|error| format!("cannot read the workflow switches: {error}"))?;
        if store.version != VERSION {
            return Err(format!(
                "cannot read the workflow switches: unknown version {}",
                store.version
            ));
        }
        Ok(Self {
            disabled: store.disabled.into_iter().collect(),
            enabled_builtin: store.enabled_builtin.into_iter().collect(),
        })
    }
}

/// `.git/gitmaster/workflows.json` of the repository at `root`, in its common Git directory.
pub fn path(git: &dyn GitPort, root: &Path) -> Result<PathBuf, String> {
    Ok(git
        .common_dir(root)
        .map_err(|e| e.0)?
        .join("gitmaster")
        .join("workflows.json"))
}

/// The switches of the repository at `root`; the defaults when the file does not exist.
pub fn load(git: &dyn GitPort, root: &Path) -> Result<Switches, String> {
    let file = path(git, root)?;
    match std::fs::read_to_string(&file) {
        Ok(text) => Switches::from_text(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Switches::default()),
        Err(error) => Err(format!("cannot read the workflow switches: {error}")),
    }
}

/// Writes `switches` for the repository at `root`.
pub fn save(git: &dyn GitPort, root: &Path, switches: &Switches) -> Result<(), String> {
    let file = path(git, root)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot save the workflow switches: {e}"))?;
    }
    let temp = file.with_extension("json.tmp");
    std::fs::write(&temp, switches.to_text())
        .map_err(|e| format!("cannot save the workflow switches: {e}"))?;
    std::fs::rename(&temp, &file).map_err(|e| format!("cannot save the workflow switches: {e}"))
}

/// Keeps the entries that run in the repository whose switches these are: the one filter that
/// triggers, phases and transforms apply (D164(1)). A manual run does not use it.
pub fn retain_enabled(entries: &mut Vec<ScriptEntry>, switches: &Switches) {
    entries.retain(|entry| switches.is_enabled(&entry.name, entry.builtin));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::loader::ScriptEntry;

    fn entry(name: &str, builtin: bool) -> ScriptEntry {
        ScriptEntry {
            path: PathBuf::from(name),
            file_name: format!("{name}.lua"),
            name: name.to_owned(),
            hash: String::new(),
            source: String::new(),
            meta: Err(Vec::new()),
            builtin,
        }
    }

    #[test]
    fn the_defaults_are_on_for_user_workflows_and_off_for_built_ins() {
        let switches = Switches::default();
        assert!(switches.is_enabled("mine", false));
        assert!(!switches.is_enabled("restack-children-on-parent-move", true));
    }

    #[test]
    fn setting_a_default_leaves_no_entry() {
        let mut switches = Switches::default();
        switches.set("mine", false, true);
        switches.set("restack-stale-after-fetch", true, false);
        assert_eq!(switches, Switches::default());
        switches.set("mine", false, false);
        switches.set("restack-stale-after-fetch", true, true);
        assert!(!switches.is_enabled("mine", false));
        assert!(switches.is_enabled("restack-stale-after-fetch", true));
    }

    #[test]
    fn the_file_round_trips_and_is_sorted() {
        let mut switches = Switches::default();
        switches.set("zeta", false, false);
        switches.set("alpha", false, false);
        switches.set("restack-stale-after-fetch", true, true);
        let text = switches.to_text();
        assert!(text.find("alpha").unwrap() < text.find("zeta").unwrap());
        assert_eq!(Switches::from_text(&text), Ok(switches));
    }

    #[test]
    fn the_file_format_is_the_documented_one() {
        let text = r#"{ "version": 1, "disabled": ["mine"], "enabled_builtin": ["b"] }"#;
        let switches = Switches::from_text(text).unwrap();
        assert!(!switches.is_enabled("mine", false));
        assert!(switches.is_enabled("b", true));
    }

    #[test]
    fn a_file_without_lists_holds_the_defaults() {
        assert_eq!(
            Switches::from_text(r#"{ "version": 1 }"#),
            Ok(Switches::default())
        );
    }

    #[test]
    fn an_unknown_version_and_broken_json_are_refused() {
        assert!(Switches::from_text(r#"{ "version": 2, "disabled": [] }"#).is_err());
        assert!(Switches::from_text("{ not json").is_err());
    }

    #[test]
    fn the_filter_drops_what_is_switched_off_here_and_keeps_the_rest() {
        let mut switches = Switches::default();
        switches.set("off", false, false);
        switches.set("on", true, true);
        let mut entries = vec![
            entry("mine", false),
            entry("off", false),
            entry("builtin-off", true),
            entry("on", true),
        ];
        retain_enabled(&mut entries, &switches);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["mine", "on"]);
    }
}
