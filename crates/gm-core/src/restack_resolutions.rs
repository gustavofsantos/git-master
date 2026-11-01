//! The resolutions of restack conflicts (D154): the commit a conflicting commit was resolved into,
//! for one `onto`. A restack that replays the same commit onto exactly that `onto` uses the
//! resolved commit; any other `onto` ignores the entry. Each resolved commit is kept from gc by
//! its ref under `refs/gitmaster/restack/resolved/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::GitPort;

const VERSION: u32 = 1;

/// The commit `commit` was resolved into when replayed onto `onto` (D154).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub onto: String,
    pub commit: String,
    pub resolved: String,
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    resolutions: Vec<Resolution>,
}

/// The resolutions recorded so far; none when the store does not exist yet.
pub fn load(git: &dyn GitPort, root: &Path) -> Result<Vec<Resolution>, String> {
    let path = store_path(git, root)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read the restack resolutions: {error}")),
    };
    let store: Store = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read the restack resolutions: {error}"))?;
    if store.version != VERSION {
        return Err(format!(
            "cannot read the restack resolutions: unknown version {}",
            store.version
        ));
    }
    Ok(store.resolutions)
}

/// Records `resolution`, replacing the entry with the same `onto` and `commit`. The resolved
/// commit is kept by its ref before the store is written, so an entry never names a commit
/// that gc could drop. The ref of a replaced entry is deleted unless another entry keeps it.
pub fn add(git: &dyn GitPort, root: &Path, resolution: Resolution) -> Result<(), String> {
    let list = load(git, root)?;
    let dropped = replaced(&list, &resolution);
    git.set_ref(root, &ref_name(&resolution.resolved), &resolution.resolved)
        .map_err(|e| e.0)?;
    let kept = with(list, resolution);
    save(git, root, &kept)?;
    settle_refs(git, root, &dropped, &kept)
}

/// Drops the entries that `used` names exactly, once the restack that used them has applied.
/// Their refs are deleted unless another entry still keeps the commit.
pub fn remove(git: &dyn GitPort, root: &Path, used: &[Resolution]) -> Result<(), String> {
    let list = load(git, root)?;
    let dropped: Vec<Resolution> = list.iter().filter(|r| used.contains(r)).cloned().collect();
    let kept = without(list, used);
    save(git, root, &kept)?;
    settle_refs(git, root, &dropped, &kept)
}

/// The resolved commit for a conflicting `commit` replayed onto exactly `onto`, if recorded.
pub fn lookup<'a>(list: &'a [Resolution], onto: &str, commit: &str) -> Option<&'a str> {
    list.iter()
        .find(|r| r.onto == onto && r.commit == commit)
        .map(|r| r.resolved.as_str())
}

/// The entries of `list` that `resolution` replaces (pure).
fn replaced(list: &[Resolution], resolution: &Resolution) -> Vec<Resolution> {
    list.iter()
        .filter(|r| r.onto == resolution.onto && r.commit == resolution.commit)
        .cloned()
        .collect()
}

/// `list` with the entries `resolution` replaces left out and `resolution` appended (pure).
fn with(list: Vec<Resolution>, resolution: Resolution) -> Vec<Resolution> {
    let mut kept: Vec<Resolution> = list
        .into_iter()
        .filter(|r| !(r.onto == resolution.onto && r.commit == resolution.commit))
        .collect();
    kept.push(resolution);
    kept
}

/// `list` without the entries that `used` names exactly (pure).
fn without(list: Vec<Resolution>, used: &[Resolution]) -> Vec<Resolution> {
    list.into_iter().filter(|r| !used.contains(r)).collect()
}

/// Deletes the refs of `dropped` whose commit no entry of `kept` still names.
fn settle_refs(
    git: &dyn GitPort,
    root: &Path,
    dropped: &[Resolution],
    kept: &[Resolution],
) -> Result<(), String> {
    for entry in dropped {
        if kept.iter().any(|k| k.resolved == entry.resolved) {
            continue;
        }
        git.delete_ref(root, &ref_name(&entry.resolved))
            .map_err(|e| e.0)?;
    }
    Ok(())
}

fn ref_name(resolved: &str) -> String {
    format!("refs/gitmaster/restack/resolved/{resolved}")
}

fn store_path(git: &dyn GitPort, root: &Path) -> Result<PathBuf, String> {
    Ok(git
        .common_dir(root)
        .map_err(|e| e.0)?
        .join("gitmaster")
        .join("restack-resolutions.json"))
}

/// Writes the store through a temporary file, so a crash leaves the old store or the new one.
fn save(git: &dyn GitPort, root: &Path, resolutions: &[Resolution]) -> Result<(), String> {
    let path = store_path(git, root)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot save the restack resolutions: {e}"))?;
    }
    let store = Store {
        version: VERSION,
        resolutions: resolutions.to_vec(),
    };
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|e| format!("cannot save the restack resolutions: {e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("cannot save the restack resolutions: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(onto: &str, commit: &str, resolved: &str) -> Resolution {
        Resolution {
            onto: onto.to_owned(),
            commit: commit.to_owned(),
            resolved: resolved.to_owned(),
        }
    }

    #[test]
    fn lookup_matches_both_ids_exactly() {
        let list = vec![entry("aa", "bb", "r1"), entry("aa", "cc", "r2")];
        assert_eq!(lookup(&list, "aa", "bb"), Some("r1"));
        assert_eq!(lookup(&list, "aa", "cc"), Some("r2"));
        // The same commit onto another base is not this resolution (D154).
        assert_eq!(lookup(&list, "ab", "bb"), None);
        assert_eq!(lookup(&list, "aa", "b"), None);
        assert_eq!(lookup(&[], "aa", "bb"), None);
    }

    #[test]
    fn adding_replaces_the_entry_with_the_same_onto_and_commit() {
        let list = vec![entry("aa", "bb", "r1"), entry("aa", "cc", "r2")];
        assert_eq!(
            replaced(&list, &entry("aa", "bb", "r3")),
            vec![entry("aa", "bb", "r1")]
        );
        let next = with(list, entry("aa", "bb", "r3"));
        assert_eq!(next, vec![entry("aa", "cc", "r2"), entry("aa", "bb", "r3")]);
    }

    #[test]
    fn adding_a_new_pair_keeps_the_others() {
        let list = vec![entry("aa", "bb", "r1")];
        assert!(replaced(&list, &entry("ab", "bb", "r2")).is_empty());
        let next = with(list, entry("ab", "bb", "r2"));
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn removing_drops_exactly_the_used_entries() {
        let list = vec![entry("aa", "bb", "r1"), entry("aa", "cc", "r2")];
        let next = without(list.clone(), &[entry("aa", "bb", "r1")]);
        assert_eq!(next, vec![entry("aa", "cc", "r2")]);
        // An entry with the same pair but another resolved commit is not the used one.
        assert_eq!(without(list.clone(), &[entry("aa", "bb", "r9")]), list);
    }
}
