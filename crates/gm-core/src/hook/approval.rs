//! Approvals of repository hooks (D17). A file in the app's data directory,
//! outside any repository, lists `(repository root, name, sha256 of command)`
//! triples. A hook runs only while its command still hashes to an approved
//! value, in the repository it was approved for: the same section and text in
//! another repository is a different hook.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "hook-approvals.json";
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Approval {
    repo: String,
    name: String,
    sha256: String,
}

fn same(approval: &Approval, repo: &str, name: &str, sha256: &str) -> bool {
    approval.repo == repo && approval.name == name && approval.sha256 == sha256
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    approved: Vec<Approval>,
}

/// The approvals file. `unavailable` is a store with no file: nothing is
/// approved and approving fails, which is how a missing data directory reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalStore {
    path: Option<PathBuf>,
}

impl ApprovalStore {
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    #[must_use]
    pub fn unavailable() -> Self {
        Self { path: None }
    }

    /// The file under the real data directory, or unavailable without one.
    #[must_use]
    pub fn default_location() -> Self {
        Self {
            path: default_file(),
        }
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Reads the file on every call: approvals are few, and another window may
    /// have changed them.
    #[must_use]
    pub fn is_approved(&self, repo: &str, name: &str, sha256: &str) -> bool {
        self.load()
            .approved
            .iter()
            .any(|a| same(a, repo, name, sha256))
    }

    /// Records the approval. Writing goes through a temporary file, so a crash
    /// leaves the old file whole.
    pub fn approve(&self, repo: &str, name: &str, sha256: &str) -> Result<(), String> {
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| "no data directory: approvals cannot be saved".to_owned())?;
        let mut stored = self.load();
        if !stored.approved.iter().any(|a| same(a, repo, name, sha256)) {
            stored.approved.push(Approval {
                repo: repo.to_owned(),
                name: name.to_owned(),
                sha256: sha256.to_owned(),
            });
        }
        stored.schema_version = VERSION;
        write(path, &stored).map_err(|e| format!("cannot save the approval: {e}"))
    }

    /// A missing or unreadable file reads as nothing approved.
    fn load(&self) -> Stored {
        self.path
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

fn write(path: &Path, stored: &Stored) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(stored).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

/// The file for this process, from the real environment.
#[must_use]
pub fn default_file() -> Option<PathBuf> {
    data_file(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

/// The approvals file under the given environment values. Same rules as the
/// recent-projects list in `gm-ui`: a relative `XDG_DATA_HOME` is ignored, and
/// without a home directory there is no file.
#[must_use]
pub fn data_file(
    xdg_data_home: Option<std::ffi::OsString>,
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
        match absolute(xdg_data_home) {
            Some(base) => base,
            None => absolute(home)?.join(".local/share"),
        }
    };
    Some(base.join("gitmaster").join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn data_file_follows_xdg_then_home() {
        assert_eq!(
            data_file(Some("/x".into()), Some("/h".into()), false),
            Some(PathBuf::from("/x/gitmaster/hook-approvals.json"))
        );
        assert_eq!(
            data_file(Some("rel".into()), Some("/h".into()), false),
            Some(PathBuf::from(
                "/h/.local/share/gitmaster/hook-approvals.json"
            ))
        );
        assert_eq!(
            data_file(None, Some(OsString::from("/h")), true),
            Some(PathBuf::from(
                "/h/Library/Application Support/gitmaster/hook-approvals.json"
            ))
        );
        assert_eq!(data_file(None, None, false), None);
    }

    #[test]
    fn an_approval_survives_a_new_store_on_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/approvals.json");
        ApprovalStore::at(path.clone())
            .approve("/r", "llm", "abc")
            .unwrap();
        let reopened = ApprovalStore::at(path);
        assert!(reopened.is_approved("/r", "llm", "abc"));
        assert!(!reopened.is_approved("/r", "llm", "abd"));
        assert!(!reopened.is_approved("/r", "other", "abc"));
        assert!(
            !reopened.is_approved("/elsewhere", "llm", "abc"),
            "the same hook in another repository needs its own approval"
        );
    }

    #[test]
    fn approving_twice_keeps_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        let store = ApprovalStore::at(path.clone());
        store.approve("/r", "llm", "abc").unwrap();
        store.approve("/r", "llm", "abc").unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(text.matches("abc").count(), 1);
    }

    #[test]
    fn a_missing_or_corrupt_file_approves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.json");
        let store = ApprovalStore::at(path.clone());
        assert!(!store.is_approved("/r", "llm", "abc"));
        std::fs::write(&path, "{not json").unwrap();
        assert!(!store.is_approved("/r", "llm", "abc"));
    }

    #[test]
    fn an_unavailable_store_refuses_to_approve() {
        let store = ApprovalStore::unavailable();
        assert!(!store.is_approved("/r", "llm", "abc"));
        assert!(store.approve("/r", "llm", "abc").is_err());
    }
}
