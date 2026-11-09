//! Review comments that a workspace holds for one of its members (D174, D182). A workspace is a
//! repository of its own, so a comment an agent or a person writes there on `member/path` is in
//! the workspace's `review.db`, not in the member clone's. A member's repository reads those
//! comments through here: the root comments on the member's files, with the member prefix
//! stripped and located in the member's own checkout. Their ids are composite, so an action on
//! one goes to the store that holds it. Nothing is copied: the workspace keeps its comments.
//!
//! A repository without a pointer file (`gitmaster/workspaces`, D180 (2)) has no source and runs
//! no Git call, so the cost for a plain repository is one read of a missing file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gm_schema::ReviewComment;

use crate::review_store::ReviewStore;
use crate::{GitPort, review_session, workspace_defs};

/// How many low bits of a composite id hold the comment's own id in its store.
const SLOT_BITS: u32 = 40;
/// The mask of the local id in a composite id.
const LOCAL_MASK: i64 = (1 << SLOT_BITS) - 1;
/// The number of workspace slots: slots run from 1 to this, and slot 0 is the repository's own.
const SLOT_COUNT: u32 = 4095;

/// One member of a workspace that this repository's clone belongs to, as the comments of the
/// workspace reach it. `member` is the folder the clone has in the workspace, so its comments
/// are the ones whose path starts with `<member>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSource {
    /// The slot of the workspace in the composite ids (1 to 4095).
    pub slot: u32,
    /// The workspace's canonical root, which holds its `review.db` and the snapshot refs.
    pub workspace_root: PathBuf,
    /// The workspace's folder name, as the comments show it.
    pub workspace_name: String,
    /// The member's folder in the workspace.
    pub member: String,
}

/// The composite id of comment or reply `local` of the store in `slot`: `slot * 2^40 + local`.
/// Slot 0 is the repository's own store and keeps its ids. Every id stays below 2^52, so it
/// is exact in JSON.
#[must_use]
pub fn encode(slot: u32, local: i64) -> i64 {
    debug_assert!(
        (0..=LOCAL_MASK).contains(&local),
        "local id {local} out of range"
    );
    if slot == 0 {
        return local;
    }
    (i64::from(slot) << SLOT_BITS) + local
}

/// The slot and the local id of a composite id. A negative id is the repository's own and is
/// returned unchanged, so that it is refused as an unknown comment as before.
#[must_use]
pub fn decode(id: i64) -> (u32, i64) {
    if id < 0 {
        return (0, id);
    }
    let slot = u32::try_from(id >> SLOT_BITS).unwrap_or(u32::MAX);
    (slot, id & LOCAL_MASK)
}

/// The slot a workspace path hashes to before any collision is settled: `1 + FNV-1a % 4095`
/// of the canonical path, so it does not change when the pointer file does.
#[must_use]
pub fn hashed_slot(workspace: &Path) -> u32 {
    let canonical = canonical(workspace);
    let text = canonical.to_string_lossy();
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    u32::try_from(hash % u64::from(SLOT_COUNT)).unwrap_or(0) + 1
}

/// The slot of each workspace of one clone (D174). A slot is `hashed_slot`; when two workspaces
/// hash to the same one, the one whose path sorts later takes the next free slot, so the result
/// is the same for the same set of paths in any order. Paths beyond the slots are left out.
#[must_use]
pub fn assign_slots(workspaces: &[PathBuf]) -> Vec<(PathBuf, u32)> {
    let mut sorted: Vec<PathBuf> = workspaces.iter().map(|path| canonical(path)).collect();
    sorted.sort();
    sorted.dedup();
    let mut taken = BTreeSet::new();
    let mut assigned = Vec::new();
    for path in sorted {
        if taken.len() >= SLOT_COUNT as usize {
            break;
        }
        let mut slot = hashed_slot(&path);
        while taken.contains(&slot) {
            slot = if slot >= SLOT_COUNT { 1 } else { slot + 1 };
        }
        taken.insert(slot);
        assigned.push((path, slot));
    }
    assigned
}

/// The workspace sources of the repository at `root`, which may be the clone's main checkout or
/// a linked worktree of it. The common directory is read from `.git` without a Git call; a clone
/// with no pointer file gives nothing, and no Git call is made. Each member of a workspace that
/// this clone is gives one source.
#[must_use]
pub fn sources(git: &dyn GitPort, root: &Path) -> Vec<WorkspaceSource> {
    let Some(common) = common_dir_of(root) else {
        return Vec::new();
    };
    let paths = workspace_defs::workspaces_of_clone(&common);
    if paths.is_empty() {
        return Vec::new();
    }
    let Some(clone_root) = common.parent() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (workspace, slot) in assign_slots(&paths) {
        let for_member =
            workspace_defs::defs_for_member(git, std::slice::from_ref(&workspace), clone_root);
        for (defs, member) in for_member {
            found.push(WorkspaceSource {
                slot,
                workspace_root: workspace.clone(),
                workspace_name: defs.name,
                member,
            });
        }
    }
    found
}

/// Every comment `root` lists: its own (slot 0, ids unchanged) and, for each source, the root
/// comments that the workspace holds on the member's files, with the member prefix stripped and
/// their location in `root`'s own files. A comment on another member's file, or on a file of
/// the workspace itself, is not listed.
pub(crate) fn listed(
    git: &dyn GitPort,
    root: &Path,
    own: &ReviewStore,
) -> Result<Vec<ReviewComment>, String> {
    let mut comments = review_session::listed(own, root)?;
    for source in sources(git, root) {
        let Some(store) = open_store(&source)? else {
            continue;
        };
        let shown: Vec<ReviewComment> = store
            .list()
            .map_err(|e| e.0)?
            .into_iter()
            .filter_map(|comment| shown_by(comment, &source))
            .collect();
        comments.extend(review_session::located(shown, root));
    }
    comments.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.anchor.start_line.cmp(&b.anchor.start_line))
            .then(a.id.cmp(&b.id))
    });
    Ok(comments)
}

/// Where a comment id names its comment, for an action: the root that holds the store and the
/// Git refs of the comment, the id in that store, and the member prefix the path is shown
/// without.
#[derive(Debug)]
pub(crate) struct Site {
    /// The root whose `review.db` and snapshot refs hold the comment: `root` itself for the
    /// repository's own comments, the workspace for a workspace's.
    pub(crate) store_root: PathBuf,
    /// The id of the comment or reply in that store.
    pub(crate) local: i64,
    /// `<member>/` for a workspace comment, so its path is shown relative to the member.
    prefix: Option<String>,
}

impl Site {
    /// `path`, as the repository shows it: without the member prefix of a workspace comment.
    pub(crate) fn shown(&self, path: &str) -> String {
        match &self.prefix {
            Some(prefix) => path
                .strip_prefix(prefix.as_str())
                .unwrap_or(path)
                .to_owned(),
            None => path.to_owned(),
        }
    }
}

/// The site of comment or reply `id` in the repository at `root`. A slot that is not one of its
/// sources, and a comment that is not on one of the member's files, are refused with the same
/// words, so an agent cannot tell another member's comment from an unknown one.
pub(crate) fn site(git: &dyn GitPort, root: &Path, id: i64) -> Result<Site, String> {
    let (slot, local) = decode(id);
    if slot == 0 {
        return Ok(Site {
            store_root: root.to_path_buf(),
            local: id,
            prefix: None,
        });
    }
    for source in sources(git, root).into_iter().filter(|s| s.slot == slot) {
        let Some(store) = open_store(&source)? else {
            continue;
        };
        let prefix = prefix_of(&source);
        let Some(path) = root_path(&store, local)? else {
            continue;
        };
        if path.starts_with(&prefix) {
            return Ok(Site {
                store_root: source.workspace_root,
                local,
                prefix: Some(prefix),
            });
        }
    }
    Err(format!("no review comment {id} here"))
}

/// The comment as a member's repository shows it, when the workspace holds it on one of the
/// member's files: its path relative to the member, its composite ids and the workspace name.
fn shown_by(mut comment: ReviewComment, source: &WorkspaceSource) -> Option<ReviewComment> {
    let rel = comment
        .path
        .strip_prefix(prefix_of(source).as_str())?
        .to_owned();
    comment.path = rel;
    comment.id = encode(source.slot, comment.id);
    for reply in &mut comment.replies {
        reply.id = encode(source.slot, reply.id);
    }
    comment.workspace = Some(source.workspace_name.clone());
    Some(comment)
}

/// The prefix of the member's files in the workspace: `<member>/`.
fn prefix_of(source: &WorkspaceSource) -> String {
    format!("{}/", source.member)
}

/// The path of the root comment that `id` belongs to; `None` when there is no such comment.
fn root_path(store: &ReviewStore, id: i64) -> Result<Option<String>, String> {
    let Some(root) = store.root_of(id).map_err(|e| e.0)? else {
        return Ok(None);
    };
    Ok(store
        .get(root)
        .map_err(|e| e.0)?
        .map(|comment| comment.path))
}

/// The store of a workspace, when it has one yet. It is read here without creating it.
fn open_store(source: &WorkspaceSource) -> Result<Option<ReviewStore>, String> {
    let Some(common) = common_dir_of(&source.workspace_root) else {
        return Ok(None);
    };
    let file = common.join("gitmaster").join("review.db");
    if !file.is_file() {
        return Ok(None);
    }
    ReviewStore::open(&file).map(Some).map_err(|e| e.0)
}

/// The common Git directory of the checkout at `root`, read from its `.git` without a Git call:
/// the folder of a main checkout, or the `commondir` that a linked worktree's `gitdir` names.
/// `None` when `.git` is not there or cannot be read.
fn common_dir_of(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if dot.is_dir() {
        return std::fs::canonicalize(dot).ok();
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    let gitdir = root.join(target);
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(relative) => gitdir.join(relative.trim()),
        Err(_) => gitdir,
    };
    std::fs::canonicalize(common).ok()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_store_keeps_its_ids_and_a_workspace_slot_is_in_the_high_bits() {
        assert_eq!(encode(0, 7), 7);
        assert_eq!(decode(7), (0, 7));
        let id = encode(3, 7);
        assert_eq!(id, 3 * (1 << 40) + 7);
        assert_eq!(decode(id), (3, 7));
    }

    #[test]
    fn the_boundaries_round_trip_and_stay_json_safe() {
        for (slot, local) in [
            (1, 0),
            (1, LOCAL_MASK),
            (SLOT_COUNT, 0),
            (SLOT_COUNT, LOCAL_MASK),
        ] {
            let id = encode(slot, local);
            assert_eq!(decode(id), (slot, local), "slot {slot} local {local}");
            assert!(id < (1 << 52), "{id} is not exact in JSON");
        }
        assert_eq!(decode(-5), (0, -5));
        assert_eq!(
            decode(i64::MAX).0,
            u32::try_from(i64::MAX >> SLOT_BITS).unwrap()
        );
    }

    #[test]
    fn a_slot_is_in_range_and_depends_only_on_the_path() {
        for n in 0..200 {
            let path = PathBuf::from(format!("/nowhere/ws-{n}"));
            let slot = hashed_slot(&path);
            assert!((1..=SLOT_COUNT).contains(&slot), "{slot}");
            assert_eq!(slot, hashed_slot(&path));
        }
    }

    #[test]
    fn two_workspaces_that_collide_keep_distinct_slots_and_the_later_one_moves() {
        // Find two paths that hash to the same slot; the search is deterministic.
        let mut first_seen = std::collections::BTreeMap::new();
        let mut pair = None;
        for n in 0..10_000 {
            let path = PathBuf::from(format!("/nowhere/collide-{n}"));
            let slot = hashed_slot(&path);
            if let Some(earlier) = first_seen.insert(slot, path.clone()) {
                pair = Some((earlier, path));
                break;
            }
        }
        let (a, b) = pair.expect("a collision among 10000 paths");
        let (early, late) = if a < b { (a, b) } else { (b, a) };
        let slot = hashed_slot(&early);

        for order in [
            vec![early.clone(), late.clone()],
            vec![late.clone(), early.clone()],
        ] {
            let assigned = assign_slots(&order);
            let slot_of = |path: &Path| {
                assigned
                    .iter()
                    .find(|(p, _)| p == &canonical(path))
                    .map(|(_, s)| *s)
            };
            assert_eq!(
                slot_of(&early),
                Some(slot),
                "the earlier path keeps its slot"
            );
            let moved = slot_of(&late).expect("the later path has a slot");
            assert_ne!(moved, slot);
            assert_eq!(moved, if slot >= SLOT_COUNT { 1 } else { slot + 1 });
        }
    }

    #[test]
    fn the_common_directory_of_a_linked_worktree_is_found_without_git() {
        let dir = tempfile::tempdir().unwrap();
        let clone = dir.path().join("clone");
        let common = clone.join(".git");
        let gitdir = common.join("worktrees").join("api");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        let worktree = dir.path().join("ws").join("api");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        std::fs::create_dir_all(clone.join("sub")).unwrap();
        assert_eq!(
            common_dir_of(&worktree),
            Some(std::fs::canonicalize(&common).unwrap())
        );
        assert_eq!(
            common_dir_of(&clone),
            Some(std::fs::canonicalize(&common).unwrap())
        );
        assert_eq!(common_dir_of(&clone.join("sub")), None);
    }

    #[test]
    fn a_clone_without_a_pointer_file_has_no_source() {
        let dir = tempfile::tempdir().unwrap();
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        let git = crate::git::SubprocessGit::default();
        assert!(sources(&git, &clone).is_empty());
    }
}
