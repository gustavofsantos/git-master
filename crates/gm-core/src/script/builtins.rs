//! The built-in stack workflows (D164(2), `WF4b`): Lua scripts compiled into the binary and listed
//! under **Built-in** on the Workflows page. They are off in every repository until they are
//! switched on there (`switches`), and they are never written to disk. Their text is their hash:
//! a new app version with a changed script is a new hash, so its grant of automatic approval
//! (D101) asks again. **Copy to Edit** writes an unreviewed copy into the user's folder.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::inspect::inspect;
use super::loader::{Loaded, ScriptEntry, sha256_hex};
use super::port::ScriptEngine;

/// The prefix of a built-in's `path`: there is no file behind it.
pub const PATH_PREFIX: &str = "builtin:";

/// One built-in workflow: its name in `M.meta`, the title the page shows, and its source.
#[derive(Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub title: &'static str,
    pub source: &'static str,
}

static BUILTINS: &[Builtin] = &[
    Builtin {
        name: "restack-children-on-parent-move",
        title: "Restack children when parent moves",
        source: include_str!("builtins/restack-children-on-parent-move.lua"),
    },
    Builtin {
        name: "restack-stale-after-fetch",
        title: "Restack stale stacks after fetch",
        source: include_str!("builtins/restack-stale-after-fetch.lua"),
    },
    Builtin {
        name: "reparent-to-grandparent-on-merge",
        title: "Reparent to grandparent when parent is merged",
        source: include_str!("builtins/reparent-to-grandparent-on-merge.lua"),
    },
];

/// Every built-in, in the order the page lists them.
#[must_use]
pub fn all() -> &'static [Builtin] {
    BUILTINS
}

/// The built-in named `name`.
#[must_use]
pub fn get(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|builtin| builtin.name == name)
}

/// Whether `hash` is the hash of a built-in's text as this build ships it. Such a version counts
/// as reviewed: the app ships it (D164(2)). Its grant of automatic approval is a separate row, so
/// a new build asks for it again.
#[must_use]
pub fn is_shipped(hash: &str) -> bool {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    HASHES
        .get_or_init(|| {
            BUILTINS
                .iter()
                .map(|builtin| sha256_hex(builtin.source.as_bytes()))
                .collect()
        })
        .iter()
        .any(|shipped| shipped == hash)
}

/// The built-in as a loaded script: its checked `M.meta` and its hash, read like a file's.
#[must_use]
pub fn entry(engine: &dyn ScriptEngine, builtin: &Builtin) -> ScriptEntry {
    let file_name = format!("{}.lua", builtin.name);
    ScriptEntry {
        path: PathBuf::from(format!("{PATH_PREFIX}{}", builtin.name)),
        meta: inspect(engine, &file_name, builtin.source),
        file_name,
        name: builtin.name.to_owned(),
        hash: sha256_hex(builtin.source.as_bytes()),
        source: builtin.source.to_owned(),
        builtin: true,
    }
}

/// Adds the built-ins to `loaded`, after the user's scripts. A built-in whose name a user script
/// already takes is not added: the user's file wins, and the built-in stays hidden.
pub fn add_to(engine: &dyn ScriptEngine, loaded: &mut Loaded) {
    for builtin in BUILTINS {
        if loaded
            .entries
            .iter()
            .any(|entry| entry.name == builtin.name)
        {
            continue;
        }
        loaded.entries.push(entry(engine, builtin));
    }
}

/// The name of a copy of the built-in `name` (D164(4)): `<name>-custom`, which the copy writes
/// under a file name not taken yet.
#[must_use]
pub fn copy_base(name: &str) -> String {
    format!("{name}-custom")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::luau::LuauEngine;

    #[test]
    fn every_built_in_loads_with_its_own_name_and_no_problems() {
        let engine = LuauEngine::new();
        for builtin in all() {
            let entry = entry(&engine, builtin);
            let meta = entry
                .meta
                .as_ref()
                .unwrap_or_else(|problems| panic!("{} does not load: {problems:?}", builtin.name));
            assert_eq!(meta.name, builtin.name);
            assert!(entry.builtin);
            assert_eq!(
                entry.path,
                PathBuf::from(format!("builtin:{}", builtin.name))
            );
            assert!(is_shipped(&entry.hash));
        }
    }

    #[test]
    fn the_built_ins_are_the_three_of_the_guide_with_their_triggers() {
        let engine = LuauEngine::new();
        let triggers: Vec<(String, String)> = all()
            .iter()
            .map(|builtin| {
                let entry = entry(&engine, builtin);
                let meta = entry.meta.expect("loads");
                (meta.name, meta.trigger.on)
            })
            .collect();
        assert_eq!(
            triggers,
            vec![
                (
                    "restack-children-on-parent-move".to_owned(),
                    "stack.parent_moved".to_owned()
                ),
                (
                    "restack-stale-after-fetch".to_owned(),
                    "fetch.done".to_owned()
                ),
                (
                    "reparent-to-grandparent-on-merge".to_owned(),
                    "stack.parent_merged".to_owned()
                ),
            ]
        );
        assert_eq!(
            get("restack-stale-after-fetch").map(|b| b.title),
            Some("Restack stale stacks after fetch")
        );
    }

    #[test]
    fn a_user_script_with_the_same_name_hides_the_built_in() {
        let engine = LuauEngine::new();
        let mut loaded = Loaded::default();
        let mut mine = entry(&engine, get("restack-stale-after-fetch").unwrap());
        mine.builtin = false;
        mine.path = PathBuf::from("/flows/restack-stale-after-fetch.lua");
        loaded.entries.push(mine);
        add_to(&engine, &mut loaded);
        let builtins: Vec<&str> = loaded
            .entries
            .iter()
            .filter(|entry| entry.builtin)
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            builtins,
            vec![
                "restack-children-on-parent-move",
                "reparent-to-grandparent-on-merge"
            ]
        );
    }

    #[test]
    fn a_changed_text_is_not_a_shipped_version() {
        assert!(!is_shipped(&sha256_hex(b"something else")));
    }

    #[test]
    fn the_copy_base_adds_custom_to_the_name() {
        assert_eq!(
            copy_base("restack-children-on-parent-move"),
            "restack-children-on-parent-move-custom"
        );
    }
}
