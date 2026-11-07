//! The repository identity: `<git common dir>/gitmaster/repo-id`, 32 lowercase hex digits written by
//! `GitMaster` the first time a repository joins a context (design/threads.md, "Repository identity";
//! D191 (4) (b), D193 (2), (4)). The file is in `.git`, so the working tree stays untouched. A clone
//! that is copied keeps the id, which `identity` tells apart from a clone that was moved.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::threads::memories::new_hex;

/// The folder under the git common directory, shared with `threads.db`.
const DIR: &str = "gitmaster";
/// The file that holds the id.
const FILE: &str = "repo-id";
/// Hex digits of an id.
const DIGITS: usize = 32;

/// Whether the repository's id file holds an id: 32 lowercase hex digits.
fn is_id(text: &str) -> bool {
    text.len() == DIGITS
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The id the file under `common_dir` holds, or `None` when it is missing or malformed.
#[must_use]
pub fn read(common_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path_of(common_dir)).ok()?;
    let id = text.trim();
    is_id(id).then(|| id.to_owned())
}

/// The id of the repository: the one the file holds, or a new one written first when there is none
/// (missing or malformed). Two processes that write at once both return the id the file holds
/// afterwards.
pub fn ensure(common_dir: &Path) -> std::io::Result<String> {
    if let Some(id) = read(common_dir) {
        return Ok(id);
    }
    let id = write_new(common_dir)?;
    Ok(read(common_dir).unwrap_or(id))
}

/// Writes a new id over any existing one and returns it. The clone that keeps the old id is the
/// other one (D193 (4)).
pub fn renew(common_dir: &Path) -> std::io::Result<String> {
    write_new(common_dir)
}

/// Writes a new id to a temporary file next to the real one, mode `0600` on Unix, then renames it
/// over the real one, so a reader sees the old id or the new one and never a part of either.
fn write_new(common_dir: &Path) -> std::io::Result<String> {
    let dir = common_dir.join(DIR);
    std::fs::create_dir_all(&dir)?;
    let id = new_hex(DIGITS);
    let temp = dir.join(format!("{FILE}.{}.tmp", new_hex(16)));
    if let Err(error) = write_then_rename(&temp, &dir.join(FILE), &id) {
        // The temporary file is removed when the write fails; nothing else is left behind.
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(id)
}

fn write_then_rename(temp: &Path, target: &Path, id: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(temp)?;
    file.write_all(id.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, target)
}

/// What the repository's recorded path and id say about the clone at the current path (D193 (4)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    /// The clone is where the contexts last saw it.
    Same,
    /// The recorded path no longer holds this id: the clone was moved, and the path is updated.
    Moved,
    /// The recorded path still holds this id, so two clones share it: the contexts are not used here.
    Clash,
}

/// Compares the path a repository was last seen at with the current one. `id_at_recorded` is the
/// id the file at the recorded path holds (`None` when it is missing or malformed).
#[must_use]
pub fn identity(
    recorded_path: &Path,
    current_path: &Path,
    id_at_recorded: Option<&str>,
    id: &str,
) -> Identity {
    if recorded_path == current_path {
        Identity::Same
    } else if id_at_recorded == Some(id) {
        Identity::Clash
    } else {
        Identity::Moved
    }
}

/// The file's path under a common directory.
fn path_of(common_dir: &Path) -> PathBuf {
    common_dir.join(DIR).join(FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn common() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn ensure_writes_32_hex_digits_privately_and_returns_the_same_id_again() {
        let dir = common();
        let first = ensure(dir.path()).expect("ensure");
        assert_eq!(first.len(), 32);
        assert!(is_id(&first), "lowercase hex: {first}");
        assert_eq!(ensure(dir.path()).expect("again"), first);
        assert_eq!(read(dir.path()).as_deref(), Some(first.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path_of(dir.path()))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(DIR))
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from(FILE)],
            "no temp file left"
        );
    }

    #[test]
    fn renew_changes_the_id_and_the_file_holds_the_new_one() {
        let dir = common();
        let first = ensure(dir.path()).expect("ensure");
        let renewed = renew(dir.path()).expect("renew");
        assert_ne!(renewed, first);
        assert_eq!(read(dir.path()).as_deref(), Some(renewed.as_str()));
    }

    #[test]
    fn a_malformed_or_missing_file_reads_as_none() {
        let dir = common();
        assert_eq!(read(dir.path()), None, "missing");
        std::fs::create_dir_all(dir.path().join(DIR)).expect("dir");
        std::fs::write(path_of(dir.path()), "not-an-id\n").expect("write");
        assert_eq!(read(dir.path()), None, "not hex");
        std::fs::write(path_of(dir.path()), "ABCDEF0123456789ABCDEF0123456789").expect("write");
        assert_eq!(read(dir.path()), None, "upper case");
        std::fs::write(path_of(dir.path()), "0123456789abcdef0123456789abcde").expect("write");
        assert_eq!(read(dir.path()), None, "31 digits");
        assert_eq!(
            ensure(dir.path()).expect("ensure replaces it").len(),
            32,
            "a malformed file is replaced"
        );
    }

    #[test]
    fn identity_is_same_for_the_same_path_moved_when_the_id_is_gone_and_clash_when_it_is_held() {
        let here = Path::new("/work/a/.git");
        let there = Path::new("/work/b/.git");
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(identity(here, here, Some(id), id), Identity::Same);
        assert_eq!(identity(here, there, None, id), Identity::Moved);
        assert_eq!(identity(here, there, Some("other"), id), Identity::Moved);
        assert_eq!(identity(here, there, Some(id), id), Identity::Clash);
    }
}
