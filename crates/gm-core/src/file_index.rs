//! The list of files of a worktree, in memory (docs/09, N1 and N2, D46). The
//! paths sit in one buffer, sorted bytewise, each named by an offset pair, so a
//! million paths cost about 40 MB and no allocation per path. The index answers
//! two questions from memory: which paths match a query, and what a folder
//! holds. Pure: `GitPort::list_files` supplies the bytes.

use gm_schema::{DirEntry, DirListing, FILE_HIT_LIMIT, FileHit, FileSearch};

use crate::path_match::{Query, top_matches};

/// The paths of a worktree, sorted and without repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileIndex {
    raw: Vec<u8>,
    /// `(start, end)` of each path in `raw`, in path order.
    entries: Vec<(u32, u32)>,
}

impl FileIndex {
    /// Builds the index from the output of `git ls-files -z -v --cached
    /// --others --exclude-standard`: records `T path` ended by NUL, where `T` is
    /// a status letter. Paths a sparse checkout leaves out (`S`, or `s` when also
    /// assumed unchanged) are not in the worktree and are dropped.
    #[must_use]
    pub fn from_ls_files(raw: Vec<u8>) -> Self {
        let mut entries = Vec::new();
        let mut start = 0usize;
        for (at, &byte) in raw.iter().enumerate() {
            if byte != 0 {
                continue;
            }
            let record = &raw[start..at];
            if record.len() > 2
                && record[1] == b' '
                && !matches!(record[0], b'S' | b's')
                && let (Ok(a), Ok(b)) = (u32::try_from(start + 2), u32::try_from(at))
            {
                entries.push((a, b));
            }
            start = at + 1;
        }
        Self::sorted(raw, entries)
    }

    /// An index of the given paths, for tests and fakes.
    #[must_use]
    pub fn from_paths<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut raw = Vec::new();
        let mut entries = Vec::new();
        for path in paths {
            let start = u32::try_from(raw.len()).unwrap_or(u32::MAX);
            raw.extend_from_slice(path.as_ref().as_bytes());
            let end = u32::try_from(raw.len()).unwrap_or(u32::MAX);
            raw.push(0);
            entries.push((start, end));
        }
        Self::sorted(raw, entries)
    }

    fn sorted(raw: Vec<u8>, mut entries: Vec<(u32, u32)>) -> Self {
        let slice = |&(a, b): &(u32, u32)| &raw[a as usize..b as usize];
        entries.sort_unstable_by(|x, y| slice(x).cmp(slice(y)));
        entries.dedup_by(|x, y| slice(x) == slice(y));
        Self { raw, entries }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Path `index`, as bytes.
    #[must_use]
    pub fn path(&self, index: usize) -> &[u8] {
        let (a, b) = self.entries[index];
        &self.raw[a as usize..b as usize]
    }

    /// Every path, in order.
    pub fn paths(&self) -> impl Iterator<Item = String> + '_ {
        (0..self.len()).map(|i| String::from_utf8_lossy(self.path(i)).into_owned())
    }

    /// The files that match `text` (a path pattern, optionally ending `:line`),
    /// best first. `None` when `stop` asked for the search to end. `indexing`
    /// says whether the list is still being read.
    pub fn search(
        &self,
        text: &str,
        indexing: bool,
        stop: impl Fn() -> bool + Sync,
    ) -> Option<FileSearch> {
        let (pattern, line) = split_line(text);
        let mut answer = FileSearch {
            query: text.to_owned(),
            hits: Vec::new(),
            matched: 0,
            files: self.len(),
            indexing,
            line,
        };
        let Some(query) = Query::new(pattern) else {
            return Some(answer);
        };
        if query.is_empty() {
            return Some(answer);
        }
        let (top, matched) =
            top_matches(&query, self.len(), FILE_HIT_LIMIT, |i| self.path(i), stop)?;
        answer.matched = matched;
        answer.hits = top
            .into_iter()
            .filter_map(|index| {
                let path = self.path(index as usize);
                let (_, ranges) = query.highlight(path)?;
                Some(FileHit {
                    path: String::from_utf8_lossy(path).into_owned(),
                    ranges: ranges.into_iter().map(|r| (r.start, r.end)).collect(),
                })
            })
            .collect();
        Some(answer)
    }

    /// What folder `dir` holds (`""` for the root): folders first, then files,
    /// each by name. A folder whose only content is one folder is joined with
    /// it (`a/b/c`), so a chain is one row. Empty for a folder that is not there.
    #[must_use]
    pub fn list_dir(&self, dir: &str, indexing: bool) -> DirListing {
        let dir = dir.trim_matches('/');
        let prefix: Vec<u8> = if dir.is_empty() {
            Vec::new()
        } else {
            format!("{dir}/").into_bytes()
        };
        let first = self
            .entries
            .partition_point(|&(a, b)| self.raw[a as usize..b as usize] < *prefix.as_slice());
        let mut entries = Vec::new();
        let mut at = first;
        while at < self.len() {
            let path = self.path(at);
            if !path.starts_with(&prefix) {
                break;
            }
            let rest = &path[prefix.len()..];
            let Some(slash) = rest.iter().position(|&b| b == b'/') else {
                entries.push(DirEntry {
                    name: String::from_utf8_lossy(rest).into_owned(),
                    is_dir: false,
                });
                at += 1;
                continue;
            };
            // Every path under this folder follows `at`, with the folder's name.
            let mut shared = prefix.len() + slash + 1;
            let under = self.entries[at..].partition_point(|&(a, b)| {
                self.raw[a as usize..b as usize].starts_with(&path[..shared])
            });
            let end = at + under;
            let mut name = rest[..slash].to_vec();
            // Join folders while the first path and the last go down the same one.
            while let Some(next) = path[shared..].iter().position(|&b| b == b'/') {
                let deeper = &path[..=shared + next];
                if !self.path(end - 1).starts_with(deeper) {
                    break;
                }
                name.push(b'/');
                name.extend_from_slice(&path[shared..shared + next]);
                shared += next + 1;
            }
            entries.push(DirEntry {
                name: String::from_utf8_lossy(&name).into_owned(),
                is_dir: true,
            });
            at = end;
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        DirListing {
            dir: dir.to_owned(),
            entries,
            files: self.len(),
            indexing,
        }
    }
}

/// Splits `foo.rs:120` or `foo.rs:120:5` into the pattern and the line. A
/// colon that does not end in digits is part of the pattern.
#[must_use]
pub fn split_line(text: &str) -> (&str, Option<u32>) {
    let mut pattern = text.trim();
    let mut line = None;
    for _ in 0..2 {
        match pattern.rsplit_once(':') {
            Some((head, tail))
                if !head.is_empty()
                    && !tail.is_empty()
                    && tail.bytes().all(|b| b.is_ascii_digit()) =>
            {
                line = tail.parse().ok();
                pattern = head;
            }
            _ => break,
        }
    }
    (pattern, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> FileIndex {
        FileIndex::from_paths(paths)
    }

    fn names(listing: &DirListing) -> Vec<String> {
        listing
            .entries
            .iter()
            .map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" }))
            .collect()
    }

    #[test]
    fn ls_files_records_are_read_sorted_and_without_repeats() {
        let raw = b"H b.rs\0H a.rs\0? new.txt\0M a.rs\0".to_vec();

        let files = FileIndex::from_ls_files(raw);

        assert_eq!(
            files.paths().collect::<Vec<_>>(),
            ["a.rs", "b.rs", "new.txt"]
        );
    }

    #[test]
    fn paths_a_sparse_checkout_leaves_out_are_dropped() {
        let raw = b"H in/a.rs\0S out/b.rs\0s out/c.rs\0h in/d.rs\0".to_vec();

        let files = FileIndex::from_ls_files(raw);

        assert_eq!(files.paths().collect::<Vec<_>>(), ["in/a.rs", "in/d.rs"]);
    }

    #[test]
    fn a_short_or_malformed_record_is_skipped_not_fatal() {
        let files = FileIndex::from_ls_files(b"H\0\0Hx\0H ok.rs\0".to_vec());
        assert_eq!(files.paths().collect::<Vec<_>>(), ["ok.rs"]);
        assert!(FileIndex::from_ls_files(Vec::new()).is_empty());
    }

    #[test]
    fn the_root_lists_folders_first_then_files_each_by_name() {
        let files = index(&[
            "z.txt",
            "src/main.rs",
            "Cargo.toml",
            "docs/a.md",
            "docs/b.md",
            "a.txt",
        ]);

        let listing = files.list_dir("", false);

        assert_eq!(
            names(&listing),
            ["docs/", "src/", "a.txt", "Cargo.toml", "z.txt"]
        );
        assert_eq!(listing.files, 6);
        assert_eq!(listing.dir, "");
    }

    #[test]
    fn a_folder_holding_only_a_folder_is_joined_with_it() {
        let files = index(&[
            "crates/gm-ui/src/lib.rs",
            "crates/gm-ui/src/view.rs",
            "README.md",
        ]);

        let root = files.list_dir("", false);
        assert_eq!(names(&root), ["crates/gm-ui/src/", "README.md"]);

        // The joined name is the path to ask for next.
        let inner = files.list_dir("crates/gm-ui/src", false);
        assert_eq!(names(&inner), ["lib.rs", "view.rs"]);
    }

    #[test]
    fn a_folder_with_a_file_and_a_folder_is_not_joined() {
        let files = index(&["a/b/c.rs", "a/d.rs"]);

        assert_eq!(names(&files.list_dir("", false)), ["a/"]);
        assert_eq!(names(&files.list_dir("a", false)), ["b/", "d.rs"]);
    }

    #[test]
    fn a_folder_name_that_starts_like_another_does_not_mix_with_it() {
        let files = index(&["a/x.rs", "ab/y.rs", "a.txt", "a-b/z.rs"]);

        assert_eq!(names(&files.list_dir("a", false)), ["x.rs"]);
        assert_eq!(names(&files.list_dir("ab", false)), ["y.rs"]);
        assert_eq!(
            names(&files.list_dir("", false)),
            ["a/", "a-b/", "ab/", "a.txt"]
        );
    }

    #[test]
    fn a_folder_that_is_not_there_is_empty() {
        let files = index(&["a/x.rs"]);
        assert!(files.list_dir("nope", false).entries.is_empty());
        assert!(files.list_dir("a/x.rs", false).entries.is_empty());
    }

    #[test]
    fn a_trailing_slash_in_the_folder_is_ignored() {
        let files = index(&["a/x.rs"]);
        assert_eq!(names(&files.list_dir("a/", false)), ["x.rs"]);
    }

    #[test]
    fn a_line_suffix_is_split_from_the_pattern() {
        assert_eq!(split_line("foo.rs:120"), ("foo.rs", Some(120)));
        assert_eq!(split_line("foo.rs:120:5"), ("foo.rs", Some(120)));
        assert_eq!(split_line("foo.rs"), ("foo.rs", None));
        assert_eq!(split_line("foo.rs:"), ("foo.rs:", None));
        assert_eq!(split_line(":12"), (":12", None));
        assert_eq!(split_line("a:b:12"), ("a:b", Some(12)));
        assert_eq!(split_line("  foo.rs:7 "), ("foo.rs", Some(7)));
    }

    #[test]
    fn a_search_finds_the_best_files_with_their_highlight_and_the_line() {
        let files = index(&[
            "docs/auth0/readme.md",
            "services/api/auth.rs",
            "src/main.rs",
        ]);

        let found = files.search("auth.rs:42", false, || false).unwrap();

        assert_eq!(found.line, Some(42));
        assert_eq!(found.files, 3);
        assert_eq!(found.hits[0].path, "services/api/auth.rs");
        assert_eq!(found.hits[0].ranges, [(13, 20)]);
        assert_eq!(found.query, "auth.rs:42");
    }

    #[test]
    fn an_empty_query_lists_nothing_but_still_counts_the_files() {
        let files = index(&["a.rs", "b.rs"]);

        let found = files.search("  ", true, || false).unwrap();

        assert!(found.hits.is_empty());
        assert_eq!(found.files, 2);
        assert!(found.indexing);
    }

    #[test]
    fn a_search_lists_at_most_the_limit_and_counts_every_match() {
        let paths: Vec<String> = (0..120).map(|i| format!("d/f{i:03}.rs")).collect();
        let files = FileIndex::from_paths(&paths);

        let found = files.search("f", false, || false).unwrap();

        assert_eq!(found.hits.len(), FILE_HIT_LIMIT);
        assert_eq!(found.matched, 120);
    }

    #[test]
    fn a_stopped_search_has_no_answer() {
        let files = index(&["a.rs"]);
        assert!(files.search("a", false, || true).is_none());
    }

    #[test]
    fn a_query_that_is_too_long_finds_nothing_without_failing() {
        let files = index(&["a.rs"]);
        let found = files.search(&"a".repeat(200), false, || false).unwrap();
        assert!(found.hits.is_empty());
    }
}
