//! The Review page's continuous diff (UI redesign, slice 4): every changed file's diff, read
//! once per status, and the **Viewed** mark of each file (docs/guide/working-copy.md,
//! "Reviewed marker"). A mark records the fingerprint of the file's change when it was set,
//! so it clears by itself once the change differs. Pure: the window sends the reads and draws
//! what this holds. Marks live with the window; nothing is written anywhere.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use gm_schema::{DiffSide, FileDiff, OperationId, StatusEntry};

/// The most files the continuous diff reads; the rest are listed by name only.
pub(crate) const FILE_LIMIT: usize = 200;

/// The diffs of one file: its unstaged and its staged side, as far as they are read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sides {
    pub(crate) unstaged: Option<FileDiff>,
    pub(crate) staged: Option<FileDiff>,
}

impl Sides {
    /// Lines added and removed over both sides.
    #[must_use]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for diff in [&self.unstaged, &self.staged].into_iter().flatten() {
            for hunk in &diff.hunks {
                for line in &hunk.lines {
                    match line.kind {
                        gm_schema::LineKind::Added => added += 1,
                        gm_schema::LineKind::Removed => removed += 1,
                        gm_schema::LineKind::Context => {}
                    }
                }
            }
        }
        (added, removed)
    }

    /// A fingerprint of the change: the lines of both sides with their kinds. Two reads of
    /// the same change give the same value.
    #[must_use]
    pub(crate) fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for diff in [&self.staged, &self.unstaged] {
            match diff {
                None => 0u8.hash(&mut hasher),
                Some(diff) => {
                    1u8.hash(&mut hasher);
                    diff.binary.hash(&mut hasher);
                    for hunk in &diff.hunks {
                        for line in &hunk.lines {
                            format!("{:?}", line.kind).hash(&mut hasher);
                            line.text.hash(&mut hasher);
                        }
                    }
                }
            }
        }
        hasher.finish()
    }
}

/// The reads in flight, the diffs read and the Viewed marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReviewFiles {
    pending: HashMap<OperationId, (String, DiffSide)>,
    diffs: BTreeMap<String, Sides>,
    /// The fingerprint of each file's change when it was marked Viewed.
    viewed: HashMap<String, u64>,
}

impl ReviewFiles {
    /// The reads a fresh status needs: both sides of each changed file that has them, up to
    /// `FILE_LIMIT` files. Diffs of files no longer changed are dropped; the others stay
    /// shown until their new read arrives.
    #[must_use]
    pub(crate) fn plan(&mut self, entries: &[StatusEntry]) -> Vec<(String, DiffSide)> {
        self.diffs
            .retain(|path, _| entries.iter().any(|entry| &entry.path == path));
        entries
            .iter()
            .filter(|entry| !entry.conflicted && entry.submodule.is_none())
            .take(FILE_LIMIT)
            .flat_map(|entry| {
                let mut sides = Vec::with_capacity(2);
                if entry.staged.is_some() {
                    sides.push((entry.path.clone(), DiffSide::Staged));
                }
                if entry.unstaged.is_some() || entry.untracked {
                    sides.push((entry.path.clone(), DiffSide::Unstaged));
                }
                sides
            })
            .collect()
    }

    /// Read `id` was sent for `path`'s `side`.
    pub(crate) fn request(&mut self, id: OperationId, path: String, side: DiffSide) {
        self.pending.insert(id, (path, side));
    }

    /// Whether read `id` is one of this store's.
    #[must_use]
    pub(crate) fn owns(&self, id: OperationId) -> bool {
        self.pending.contains_key(&id)
    }

    /// Read `id` answered with `diff`. Returns whether it was this store's.
    pub(crate) fn loaded(&mut self, id: OperationId, diff: FileDiff) -> bool {
        let Some((path, side)) = self.pending.remove(&id) else {
            return false;
        };
        let sides = self.diffs.entry(path).or_default();
        match side {
            DiffSide::Unstaged => sides.unstaged = Some(diff),
            DiffSide::Staged => sides.staged = Some(diff),
        }
        true
    }

    /// Read `id` ended without a diff (it failed). Returns whether it was this store's.
    pub(crate) fn settled(&mut self, id: OperationId) -> bool {
        self.pending.remove(&id).is_some()
    }

    /// The files read, by path.
    pub(crate) fn files(&self) -> impl Iterator<Item = (&str, &Sides)> {
        self.diffs
            .iter()
            .map(|(path, sides)| (path.as_str(), sides))
    }

    /// Whether `path` is marked Viewed and its change is the one that was viewed.
    #[must_use]
    pub(crate) fn is_viewed(&self, path: &str) -> bool {
        match (self.viewed.get(path), self.diffs.get(path)) {
            (Some(mark), Some(sides)) => *mark == sides.fingerprint(),
            _ => false,
        }
    }

    /// Marks `path` Viewed, or clears its mark when it is viewed. A file not read yet cannot
    /// be marked.
    pub(crate) fn toggle_viewed(&mut self, path: &str) {
        if self.is_viewed(path) {
            self.viewed.remove(path);
        } else if let Some(sides) = self.diffs.get(path) {
            self.viewed.insert(path.to_owned(), sides.fingerprint());
        }
    }

    /// How many of the files read are Viewed, and how many were read.
    #[must_use]
    pub(crate) fn viewed_count(&self) -> (usize, usize) {
        let viewed = self
            .diffs
            .keys()
            .filter(|path| self.is_viewed(path))
            .count();
        (viewed, self.diffs.len())
    }

    /// Another repository opened: nothing of the old one is kept.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use gm_schema::{Change, DiffLine, Hunk, HunkId, LineKind};

    use super::*;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    fn diff(path: &str, added: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            old_path: None,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId("h".into()),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                header: String::new(),
                lines: vec![
                    DiffLine {
                        kind: LineKind::Removed,
                        text: "old".into(),
                        old_no: Some(1),
                        new_no: None,
                        no_newline: false,
                    },
                    DiffLine {
                        kind: LineKind::Added,
                        text: added.into(),
                        old_no: None,
                        new_no: Some(1),
                        no_newline: false,
                    },
                ],
            }],
        }
    }

    const M: Option<Change> = Some(Change::Modified);

    #[test]
    fn a_status_plans_one_read_per_side_and_drops_files_no_longer_changed() {
        let mut files = ReviewFiles::default();
        let plan = files.plan(&[entry("a", M, M), entry("b", None, M)]);
        assert_eq!(
            plan,
            vec![
                ("a".to_owned(), DiffSide::Staged),
                ("a".to_owned(), DiffSide::Unstaged),
                ("b".to_owned(), DiffSide::Unstaged),
            ]
        );
        files.request(OperationId(1), "b".into(), DiffSide::Unstaged);
        assert!(files.loaded(OperationId(1), diff("b", "new")));
        assert!(!files.loaded(OperationId(1), diff("b", "new")));
        assert_eq!(files.files().count(), 1);
        let _ = files.plan(&[entry("a", M, None)]);
        assert_eq!(files.files().count(), 0);
    }

    #[test]
    fn a_viewed_mark_clears_when_the_change_changes() {
        let mut files = ReviewFiles::default();
        files.request(OperationId(1), "b".into(), DiffSide::Unstaged);
        files.loaded(OperationId(1), diff("b", "new"));
        files.toggle_viewed("b");
        assert!(files.is_viewed("b"));
        assert_eq!(files.viewed_count(), (1, 1));
        // The same change read again stays viewed.
        files.request(OperationId(2), "b".into(), DiffSide::Unstaged);
        files.loaded(OperationId(2), diff("b", "new"));
        assert!(files.is_viewed("b"));
        // Another change clears it.
        files.request(OperationId(3), "b".into(), DiffSide::Unstaged);
        files.loaded(OperationId(3), diff("b", "newer"));
        assert!(!files.is_viewed("b"));
        files.toggle_viewed("b");
        assert!(files.is_viewed("b"));
        files.toggle_viewed("b");
        assert!(!files.is_viewed("b"));
        assert!(!files.is_viewed("unknown"));
    }

    #[test]
    fn counts_add_both_sides() {
        let sides = Sides {
            unstaged: Some(diff("a", "x")),
            staged: Some(diff("a", "y")),
        };
        assert_eq!(sides.counts(), (2, 2));
    }
}
