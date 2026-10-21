//! The file list and the file viewer behind Quick Open and the Code page
//! (docs/09, N1 and N2). Three kinds of work run on threads of their own, and
//! each answers to the engine with a report: reading the list of files
//! (`IndexJob`), searching it (`SearchJob`) and reading one file (`ReadJob`).
//! A search or a read that a newer request superseded is stopped, and its late
//! answer is dropped by `OperationId`, as for the history.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gm_schema::{FILE_VIEW_LIMIT, FileBody, FileSearch, FileView, OperationId};

use crate::file_index::FileIndex;
use crate::port::FileListRead;

/// How many bytes at the start of a file are looked at for a NUL: Git's own
/// test for a binary file.
const BINARY_PROBE: usize = 8000;

/// The state of the file list of the open repository.
pub struct FilesSession {
    pub root: PathBuf,
    /// The last complete list. It keeps answering while a new one is read.
    pub index: Option<Arc<FileIndex>>,
    /// The read of the list in flight.
    pub building: Option<Building>,
    next_token: u64,
}

/// A read of the list that has not answered yet.
pub struct Building {
    pub token: u64,
    /// The command that asked for it; the `FilesIndexed` event carries it.
    pub id: OperationId,
    pub stop: Arc<AtomicBool>,
}

impl FilesSession {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            index: None,
            building: None,
            next_token: 0,
        }
    }

    /// The token of the next read of the list.
    pub fn next_token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    /// Whether the list is being read now.
    #[must_use]
    pub fn indexing(&self) -> bool {
        self.building.is_some()
    }
}

/// The read of the list of files, run on a thread of its own.
pub struct IndexJob {
    pub token: u64,
    pub read: FileListRead,
    pub stop: Arc<AtomicBool>,
}

pub struct IndexReport {
    pub token: u64,
    pub outcome: Result<FileIndex, String>,
}

impl IndexJob {
    /// Runs the read to its end, or to the stop request.
    #[must_use]
    pub fn run(self) -> IndexReport {
        let outcome = (self.read)(&self.stop).map_err(|error| error.0);
        IndexReport {
            token: self.token,
            outcome,
        }
    }
}

/// A search over a list that is already in memory.
pub struct SearchJob {
    pub id: OperationId,
    pub index: Arc<FileIndex>,
    pub query: String,
    pub indexing: bool,
    pub stop: Arc<AtomicBool>,
}

/// `outcome` is `None` when the search was stopped.
pub struct SearchReport {
    pub id: OperationId,
    pub outcome: Option<FileSearch>,
}

impl SearchJob {
    #[must_use]
    pub fn run(self) -> SearchReport {
        let stop = &self.stop;
        let outcome = self
            .index
            .search(&self.query, self.indexing, || stop.load(Ordering::SeqCst));
        SearchReport {
            id: self.id,
            outcome,
        }
    }
}

/// The read of one file for the viewer.
pub struct ReadJob {
    pub id: OperationId,
    pub root: PathBuf,
    pub path: String,
    pub stop: Arc<AtomicBool>,
}

pub struct ReadReport {
    pub id: OperationId,
    pub outcome: Result<FileView, String>,
}

impl ReadJob {
    #[must_use]
    pub fn run(self) -> ReadReport {
        let outcome = if self.stop.load(Ordering::SeqCst) {
            Err("the file read was superseded".to_owned())
        } else {
            read_file(&self.root, &self.path)
        };
        ReadReport {
            id: self.id,
            outcome,
        }
    }
}

fn view(path: &str, size: u64, body: FileBody, truncated: bool) -> FileView {
    FileView {
        path: path.to_owned(),
        size,
        body,
        truncated,
    }
}

/// Reads `path`, relative to `root`, for the viewer. Nothing outside the
/// worktree is read: a path must be made of plain names, and the folder that
/// holds the file must resolve inside the root (a symbolic link in the way would
/// lead out). A symbolic link is shown, not followed. At most `FILE_VIEW_LIMIT`
/// bytes are read, and a larger file is cut at a line end.
pub fn read_file(root: &Path, path: &str) -> Result<FileView, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("not a path inside the worktree: {path:?}"));
    }
    let full = root.join(relative);
    let cannot = |error: std::io::Error| format!("cannot read {path}: {error}");
    if let (Some(parent), Ok(base)) = (full.parent(), root.canonicalize()) {
        match parent.canonicalize() {
            Ok(real) if real.starts_with(&base) => {}
            Ok(_) => return Err(format!("{path} is outside the worktree")),
            // A missing folder is a missing file.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(view(path, 0, FileBody::Missing, false));
            }
            Err(error) => return Err(cannot(error)),
        }
    }
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(view(path, 0, FileBody::Missing, false));
        }
        Err(error) => return Err(cannot(error)),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full).map_err(cannot)?;
        let body = FileBody::Symlink {
            target: target.to_string_lossy().into_owned(),
        };
        return Ok(view(path, 0, body, false));
    }
    if meta.is_dir() {
        return Ok(view(path, 0, FileBody::Directory, false));
    }
    if !meta.is_file() {
        // A pipe or a device could block the read for ever.
        return Err(format!("{path} is not a regular file"));
    }
    let size = meta.len();
    let mut bytes = Vec::new();
    std::fs::File::open(&full)
        .and_then(|file| {
            file.take(FILE_VIEW_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(cannot)?;
    if bytes.iter().take(BINARY_PROBE).any(|&b| b == 0) {
        return Ok(view(path, size, FileBody::Binary, false));
    }
    let truncated = bytes.len() > FILE_VIEW_LIMIT;
    if truncated {
        bytes.truncate(FILE_VIEW_LIMIT);
        // Cut at the end of a line, so a line is never shown half and a
        // multi-byte character is never cut.
        if let Some(end) = bytes.iter().rposition(|&b| b == b'\n') {
            bytes.truncate(end + 1);
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(view(path, size, FileBody::Text { text }, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(file: &FileView) -> &str {
        match &file.body {
            FileBody::Text { text } => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_text_file_is_read_whole_with_its_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();

        let file = read_file(dir.path(), "src/a.rs").unwrap();

        assert_eq!(text(&file), "fn a() {}\n");
        assert_eq!(file.size, 10);
        assert!(!file.truncated);
        assert_eq!(file.path, "src/a.rs");
    }

    #[test]
    fn an_empty_file_is_empty_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("e"), "").unwrap();
        let file = read_file(dir.path(), "e").unwrap();
        assert_eq!(text(&file), "");
        assert_eq!(file.size, 0);
    }

    #[test]
    fn a_file_without_a_final_newline_keeps_its_last_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("n"), "a\nb").unwrap();
        assert_eq!(text(&read_file(dir.path(), "n").unwrap()), "a\nb");
    }

    #[test]
    fn a_file_with_a_nul_byte_is_binary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.bin"), b"PNG\0\x01\x02").unwrap();
        let file = read_file(dir.path(), "b.bin").unwrap();
        assert_eq!(file.body, FileBody::Binary);
        assert_eq!(file.size, 6);
    }

    #[test]
    fn text_that_is_not_utf8_is_shown_with_replacement_characters() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("l1"), b"caf\xe9 ok\n").unwrap();
        assert_eq!(
            text(&read_file(dir.path(), "l1").unwrap()),
            "caf\u{fffd} ok\n"
        );
    }

    #[test]
    fn a_large_file_is_cut_at_a_line_end_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let line = "0123456789abcdef\n";
        let body = line.repeat(FILE_VIEW_LIMIT / line.len() + 100);
        std::fs::write(dir.path().join("big"), &body).unwrap();

        let file = read_file(dir.path(), "big").unwrap();

        assert!(file.truncated);
        assert_eq!(file.size, body.len() as u64);
        let shown = text(&file);
        assert!(shown.len() <= FILE_VIEW_LIMIT);
        assert!(shown.ends_with('\n'));
        assert!(shown.len() > FILE_VIEW_LIMIT - line.len());
    }

    #[test]
    fn a_missing_file_or_folder_is_reported_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_file(dir.path(), "nope.rs").unwrap().body,
            FileBody::Missing
        );
        assert_eq!(
            read_file(dir.path(), "no/such/dir/x.rs").unwrap().body,
            FileBody::Missing
        );
    }

    #[test]
    fn a_folder_is_reported_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        assert_eq!(
            read_file(dir.path(), "d").unwrap().body,
            FileBody::Directory
        );
    }

    #[test]
    fn paths_that_leave_the_worktree_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["", "/etc/passwd", "../x", "a/../../x", "./a", "a//b/../c"] {
            assert!(read_file(dir.path(), path).is_err(), "{path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_shown_and_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();

        let file = read_file(dir.path(), "link").unwrap();

        assert_eq!(
            file.body,
            FileBody::Symlink {
                target: outside.path().join("secret").to_string_lossy().into_owned()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_to_a_folder_outside_does_not_lead_out() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();

        let result = read_file(dir.path(), "out/secret");

        assert!(result.is_err(), "{result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        if status.is_ok_and(|s| s.success()) {
            assert!(read_file(dir.path(), "pipe").is_err());
        }
    }

    #[test]
    fn a_read_stopped_before_it_starts_fails_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "x").unwrap();
        let job = ReadJob {
            id: OperationId(1),
            root: dir.path().to_path_buf(),
            path: "a".into(),
            stop: Arc::new(AtomicBool::new(true)),
        };
        assert!(job.run().outcome.is_err());
    }
}
