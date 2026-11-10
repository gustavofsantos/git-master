//! The engine's side of review comments (B4, D96, D97): the store of the open
//! repository, the reads of the working tree that comments are anchored in and
//! checked against, and where each comment is now. The snapshot writes are the
//! port's; the commands are in `engine`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use gm_schema::ReviewComment;

use crate::GitPort;
use crate::review;
use crate::review_store::ReviewStore;

/// The largest file a comment may be made on.
const FILE_LIMIT: u64 = 4 * 1024 * 1024;

/// The store of one open repository: created on the first review command, and
/// replaced when another repository opens.
pub(crate) struct ReviewSession {
    pub(crate) root: PathBuf,
    pub(crate) store: ReviewStore,
}

impl ReviewSession {
    /// Opens `.git/gitmaster/review.db` in the common Git directory of `root`,
    /// which linked worktrees share.
    pub(crate) fn open(git: &dyn GitPort, root: &Path) -> Result<Self, String> {
        let common = git.common_dir(root).map_err(|e| e.0)?;
        let store =
            ReviewStore::open(&common.join("gitmaster").join("review.db")).map_err(|e| e.0)?;
        Ok(Self {
            root: root.to_path_buf(),
            store,
        })
    }
}

/// How many repositories' stores the engine keeps open at once (D96).
pub(crate) const REVIEW_SESSION_LIMIT: usize = 4;

/// The store of the checkout at `root` in `slot`, opened on first use. The oldest session is
/// dropped when the slot is full.
pub(crate) fn review_store<'a>(
    slot: &'a mut Vec<ReviewSession>,
    git: &dyn GitPort,
    root: &Path,
) -> Result<&'a mut ReviewStore, String> {
    if let Some(index) = slot.iter().position(|session| session.root == root) {
        return Ok(&mut slot[index].store);
    }
    if slot.len() >= REVIEW_SESSION_LIMIT {
        slot.remove(0);
    }
    slot.push(ReviewSession::open(git, root)?);
    let last = slot.len() - 1;
    Ok(&mut slot[last].store)
}

/// Reads `path`, relative to `root`, as the text a comment is anchored in. The
/// path must be made of plain names and its folder must resolve inside the
/// root. Refused: a missing file, a symbolic link, a folder, a file over
/// `FILE_LIMIT`, and a file that is not UTF-8.
pub(crate) fn read_text(root: &Path, path: &str) -> Result<String, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("not a path inside the worktree: {path:?}"));
    }
    let full = root.join(relative);
    let missing = || format!("there is no file {path} in the worktree");
    if let (Some(parent), Ok(base)) = (full.parent(), root.canonicalize()) {
        let real = parent.canonicalize().map_err(|_| missing())?;
        if !real.starts_with(&base) {
            return Err(format!("{path} is outside the worktree"));
        }
    }
    let meta = std::fs::symlink_metadata(&full).map_err(|_| missing())?;
    if !meta.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if meta.len() > FILE_LIMIT {
        return Err(format!("{path} is larger than 4 MiB"));
    }
    let bytes = std::fs::read(&full).map_err(|e| format!("cannot read {path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8 text"))
}

/// Every comment of `store`, with its location in the file now and whether it waits for an
/// answer (D200). Each distinct file is read once; a file that cannot be read makes its
/// comments outdated.
pub(crate) fn listed(store: &ReviewStore, root: &Path) -> Result<Vec<ReviewComment>, String> {
    let comments = store.list().map_err(|e| e.0)?;
    Ok(located(comments, root))
}

/// `comments`, each with its location in the file `root` holds at its path now, and whether it
/// waits for an answer (D200). The paths are relative to `root`, so a comment that a workspace
/// holds for a member is located in the member's own checkout. Each distinct file is read once.
pub(crate) fn located(comments: Vec<ReviewComment>, root: &Path) -> Vec<ReviewComment> {
    let mut texts: BTreeMap<String, Option<String>> = BTreeMap::new();
    comments
        .into_iter()
        .map(|mut comment| {
            let text = texts
                .entry(comment.path.clone())
                .or_insert_with(|| read_text(root, &comment.path).ok());
            let location = text
                .as_deref()
                .and_then(|text| review::locate(text, &comment.anchor));
            comment.outdated = location.is_none();
            comment.location = location;
            comment.needs_answer = review::needs_answer(&comment);
            comment
        })
        .collect()
}
