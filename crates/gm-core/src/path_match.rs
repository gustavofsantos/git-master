//! The path matcher behind Quick Open (docs/09, N1, D46, spike S-C). Pure: no
//! I/O, so the ranking and the highlight are tested on their own.
//!
//! A query matches a path when its letters appear in the path in order,
//! ignoring case. A path scores higher for a letter at the start of a segment
//! (after `/`, `_`, `-`, `.`, a space or a lowercase-to-uppercase step), for
//! letters next to each other, for letters in the file name, and for a file name
//! that starts with the whole query. Shorter paths win ties on score; equal
//! scores keep the order of the list, so the same query over the same list
//! always gives the same answer, whatever the number of threads.
//!
//! An ASCII query runs over bytes. Any other query runs over characters, which
//! is slower and rarer. A query longer than `MAX_QUERY` bytes matches nothing.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;

/// The longest query that matches anything, in bytes.
pub const MAX_QUERY: usize = 64;

/// How many paths one thread takes at least; below this the search stays on one.
const MIN_PER_THREAD: usize = 20_000;

/// A query prepared for matching: lowercase, in the form its path loop wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// Matches everything, with no highlight.
    Empty,
    /// Lowercase ASCII letters.
    Ascii(Vec<u8>),
    /// Lowercase characters, for a query with a non-ASCII letter.
    Unicode(Vec<char>),
}

impl Query {
    /// `None` when the query can match nothing (it is too long).
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Some(Self { kind: Kind::Empty });
        }
        if text.len() > MAX_QUERY {
            return None;
        }
        let kind = if text.is_ascii() {
            Kind::Ascii(text.bytes().map(|b| b.to_ascii_lowercase()).collect())
        } else {
            Kind::Unicode(text.chars().flat_map(char::to_lowercase).collect())
        };
        Some(Self { kind })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind == Kind::Empty
    }

    /// The score of `path`, or `None` when the query is not a subsequence of it.
    #[must_use]
    pub fn score(&self, path: &[u8]) -> Option<i32> {
        let mut at = [0usize; MAX_QUERY];
        let n = self.positions(path, &mut at)?;
        Some(score_positions(path, &at[..n]))
    }

    /// The score of `path` and the byte ranges of it that matched, adjacent
    /// letters merged. An empty query matches with no ranges.
    #[must_use]
    pub fn highlight(&self, path: &[u8]) -> Option<(i32, Vec<Range<usize>>)> {
        let mut at = [0usize; MAX_QUERY];
        let n = self.positions(path, &mut at)?;
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for &position in &at[..n] {
            let width = char_width(path, position);
            match ranges.last_mut() {
                Some(last) if last.end == position => last.end = position + width,
                _ => ranges.push(position..position + width),
            }
        }
        Some((score_positions(path, &at[..n]), ranges))
    }

    /// Fills `at` with the byte offset of each query letter in `path`, in the
    /// tightest window: the first place the whole query fits, then pulled back
    /// from its end. Returns how many letters there are.
    fn positions(&self, path: &[u8], at: &mut [usize; MAX_QUERY]) -> Option<usize> {
        match &self.kind {
            Kind::Empty => Some(0),
            Kind::Ascii(query) => ascii_positions(query, path, at),
            Kind::Unicode(query) => unicode_positions(query, path, at),
        }
    }
}

/// The byte width of the character that starts at `position`.
fn char_width(path: &[u8], position: usize) -> usize {
    match path.get(position) {
        Some(&b) if b >= 0xF0 => 4,
        Some(&b) if b >= 0xE0 => 3,
        Some(&b) if b >= 0xC0 => 2,
        _ => 1,
    }
}

fn ascii_positions(query: &[u8], path: &[u8], at: &mut [usize; MAX_QUERY]) -> Option<usize> {
    let n = query.len();
    let mut matched = 0;
    let mut end = 0;
    for (i, &b) in path.iter().enumerate() {
        if b.to_ascii_lowercase() == query[matched] {
            matched += 1;
            if matched == n {
                end = i;
                break;
            }
        }
    }
    if matched < n {
        return None;
    }
    let mut remaining = n;
    let mut i = end + 1;
    while remaining > 0 {
        i -= 1;
        if path[i].to_ascii_lowercase() == query[remaining - 1] {
            remaining -= 1;
            at[remaining] = i;
        }
    }
    Some(n)
}

fn unicode_positions(query: &[char], path: &[u8], at: &mut [usize; MAX_QUERY]) -> Option<usize> {
    let text = String::from_utf8_lossy(path);
    let letters: Vec<(usize, char)> = text
        .char_indices()
        .map(|(i, c)| (i, c.to_lowercase().next().unwrap_or(c)))
        .collect();
    let n = query.len().min(MAX_QUERY);
    let mut matched = 0;
    let mut end = 0;
    for (index, &(_, c)) in letters.iter().enumerate() {
        if c == query[matched] {
            matched += 1;
            if matched == n {
                end = index;
                break;
            }
        }
    }
    if matched < n {
        return None;
    }
    let mut remaining = n;
    let mut index = end + 1;
    while remaining > 0 {
        index -= 1;
        if letters[index].1 == query[remaining - 1] {
            remaining -= 1;
            at[remaining] = letters[index].0;
        }
    }
    Some(n)
}

fn is_separator(b: u8) -> bool {
    matches!(b, b'/' | b'_' | b'-' | b'.' | b' ')
}

/// The score of a match whose letters sit at `at`.
fn score_positions(path: &[u8], at: &[usize]) -> i32 {
    let Some(&first) = at.first() else {
        return 0;
    };
    let name_start = path.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1);
    let mut total = 0i32;
    for (k, &p) in at.iter().enumerate() {
        let mut s = 16;
        let before = if p == 0 { b'/' } else { path[p - 1] };
        if is_separator(before) {
            s += if before == b'/' { 28 } else { 20 };
        } else if path[p].is_ascii_uppercase() && before.is_ascii_lowercase() {
            s += 18;
        }
        if k > 0 {
            let gap = p - at[k - 1] - 1;
            if gap == 0 {
                s += 24;
            } else {
                s -= i32::try_from(gap.min(16)).unwrap_or(16);
            }
        }
        if p >= name_start {
            s += 10;
        }
        total += s;
    }
    // The whole query at the start of the file name is the best a path can do.
    let last = at[at.len() - 1];
    if first == name_start && last - first + 1 == at.len() {
        total += 60;
    }
    total - i32::try_from(path.len() / 8).unwrap_or(i32::MAX)
}

/// A candidate: its score, then the earlier index wins.
type Entry = (i32, Reverse<u32>);

/// The best `limit` matches among `count` paths, best first, with the number of
/// paths that matched. `path(i)` gives path `i`. `stop` is asked between paths:
/// when it returns true the search ends and the answer is `None`.
pub fn top_matches<'a, P, S>(
    query: &Query,
    count: usize,
    limit: usize,
    path: P,
    stop: S,
) -> Option<(Vec<u32>, usize)>
where
    P: Fn(usize) -> &'a [u8] + Sync,
    S: Fn() -> bool + Sync,
{
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(count / MIN_PER_THREAD + 1)
        .max(1);
    top_matches_on(query, count, limit, threads, &path, &stop)
}

/// `top_matches` over a given number of threads: the answer does not depend on it.
pub fn top_matches_on<'a, P, S>(
    query: &Query,
    count: usize,
    limit: usize,
    threads: usize,
    path: &P,
    stop: &S,
) -> Option<(Vec<u32>, usize)>
where
    P: Fn(usize) -> &'a [u8] + Sync,
    S: Fn() -> bool + Sync,
{
    let chunk = count.div_ceil(threads.max(1)).max(1);
    let parts: Vec<Option<(Vec<Entry>, usize)>> = if threads <= 1 {
        vec![scan(query, 0, count, limit, path, stop)]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let from = (t * chunk).min(count);
                    let to = ((t + 1) * chunk).min(count);
                    scope.spawn(move || scan(query, from, to, limit, path, stop))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or(None))
                .collect()
        })
    };
    let mut all = Vec::new();
    let mut matched = 0;
    for part in parts {
        let (entries, found) = part?;
        all.extend(entries);
        matched += found;
    }
    all.sort_by(|a, b| b.cmp(a));
    all.truncate(limit);
    Some((all.into_iter().map(|(_, Reverse(i))| i).collect(), matched))
}

/// How often the scan asks whether it should stop, in paths.
const STOP_EVERY: usize = 4096;

fn scan<'a, P, S>(
    query: &Query,
    from: usize,
    to: usize,
    limit: usize,
    path: &P,
    stop: &S,
) -> Option<(Vec<Entry>, usize)>
where
    P: Fn(usize) -> &'a [u8],
    S: Fn() -> bool,
{
    // A min-heap: the root is the worst entry kept so far.
    let mut heap: BinaryHeap<Reverse<Entry>> = BinaryHeap::with_capacity(limit + 1);
    let mut matched = 0;
    for index in from..to {
        if index % STOP_EVERY == 0 && stop() {
            return None;
        }
        let Some(score) = query.score(path(index)) else {
            continue;
        };
        matched += 1;
        let entry = (score, Reverse(u32::try_from(index).unwrap_or(u32::MAX)));
        if heap.len() < limit {
            heap.push(Reverse(entry));
        } else if heap.peek().is_some_and(|worst| entry > worst.0) {
            heap.pop();
            heap.push(Reverse(entry));
        }
    }
    Some((heap.into_iter().map(|Reverse(e)| e).collect(), matched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn q(text: &str) -> Query {
        Query::new(text).expect("a usable query")
    }

    fn score(query: &str, path: &str) -> Option<i32> {
        q(query).score(path.as_bytes())
    }

    fn best(query: &str, paths: &[&str]) -> Vec<String> {
        let query = q(query);
        let (hits, _) = top_matches_on(
            &query,
            paths.len(),
            10,
            1,
            &|i| paths[i].as_bytes(),
            &|| false,
        )
        .unwrap();
        hits.into_iter()
            .map(|i| paths[i as usize].to_owned())
            .collect()
    }

    #[test]
    fn letters_must_appear_in_order_and_case_is_ignored() {
        assert!(score("dp", "crates/gm-ui/src/diff_pane.rs").is_some());
        assert!(score("DP", "crates/gm-ui/src/diff_pane.rs").is_some());
        assert!(score("pd", "diff_pane.rs").is_none(), "order matters");
        assert!(score("xq", "diff_pane.rs").is_none());
        assert!(score("pdd", "diff_pane.rs").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything_with_no_highlight() {
        let query = q("   ");
        assert!(query.is_empty());
        assert_eq!(query.highlight(b"a/b.rs"), Some((0, Vec::new())));
    }

    #[test]
    fn a_query_that_is_too_long_matches_nothing() {
        assert!(Query::new(&"a".repeat(MAX_QUERY + 1)).is_none());
        assert!(Query::new(&"a".repeat(MAX_QUERY)).is_some());
    }

    #[test]
    fn the_file_name_beats_a_directory_with_the_same_letters() {
        let ranked = best(
            "auth",
            &["docs/auth0/bench/readme.md", "services/api/auth.rs"],
        );
        assert_eq!(ranked[0], "services/api/auth.rs");
    }

    #[test]
    fn a_name_that_starts_with_the_query_beats_one_that_contains_it() {
        let ranked = best(
            "diff",
            &[
                "src/undiffed.rs",
                "src/diff_pane.rs",
                "src/a/b/diff_rows.rs",
            ],
        );
        assert_eq!(ranked[0], "src/diff_pane.rs");
        assert_eq!(ranked[1], "src/a/b/diff_rows.rs");
    }

    #[test]
    fn letters_at_the_start_of_words_beat_letters_in_the_middle() {
        let ranked = best("dp", &["src/adopt.rs", "src/diff_pane.rs"]);
        assert_eq!(ranked[0], "src/diff_pane.rs");
    }

    #[test]
    fn camel_case_humps_count_as_word_starts() {
        let ranked = best("hl", &["src/shelf.rs", "src/HistoryList.rs"]);
        assert_eq!(ranked[0], "src/HistoryList.rs");
    }

    #[test]
    fn a_shorter_path_wins_when_nothing_else_differs() {
        let ranked = best("main.rs", &["a/b/c/d/e/f/main.rs", "src/main.rs"]);
        assert_eq!(ranked[0], "src/main.rs");
    }

    #[test]
    fn equal_scores_keep_the_order_of_the_list() {
        let paths = ["a/x.rs", "b/x.rs", "c/x.rs"];
        assert_eq!(best("x", &paths), paths);
    }

    #[test]
    fn the_highlight_marks_the_tightest_window_and_merges_adjacent_letters() {
        let (_, ranges) = q("pane").highlight(b"src/diff_pane.rs").unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 9..13);
        // "ds" is found at the d of diff and the s of src's neighbour in the window.
        let (_, ranges) = q("rs").highlight(b"src/diff_pane.rs").unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 14..16);
    }

    #[test]
    fn highlight_ranges_are_whole_characters_for_a_unicode_query() {
        let path = "docs/ação/relatório.md";
        let (_, ranges) = q("ção").highlight(path.as_bytes()).unwrap();
        for range in &ranges {
            assert!(path.is_char_boundary(range.start) && path.is_char_boundary(range.end));
        }
        assert_eq!(&path[ranges[0].clone()], "ção");
    }

    #[test]
    fn a_unicode_query_ignores_case_too() {
        assert!(score("AÇÃO", "docs/ação.md").is_some());
        assert!(score("ação", "docs/outra.md").is_none());
    }

    #[test]
    fn the_count_of_matches_is_the_whole_list_not_just_the_top() {
        let query = q("x");
        let paths: Vec<String> = (0..500).map(|i| format!("d/x{i}.rs")).collect();
        let (hits, matched) =
            top_matches_on(&query, paths.len(), 5, 1, &|i| paths[i].as_bytes(), &|| {
                false
            })
            .unwrap();
        assert_eq!(hits.len(), 5);
        assert_eq!(matched, 500);
    }

    #[test]
    fn a_stop_request_ends_the_search_with_no_answer() {
        let query = q("x");
        let paths: Vec<String> = (0..10_000).map(|i| format!("d/x{i}.rs")).collect();
        let answer = top_matches_on(&query, paths.len(), 5, 1, &|i| paths[i].as_bytes(), &|| {
            true
        });
        assert!(answer.is_none());
    }

    fn random_paths() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec("[a-zA-Z0-9_./-]{1,30}", 1..120)
    }

    proptest! {
        #[test]
        fn a_path_matches_exactly_when_the_query_is_a_subsequence(
            path in "[a-zA-Z0-9_./-]{1,40}",
            query in "[a-z0-9_.]{1,6}",
        ) {
            let lower = path.to_ascii_lowercase();
            let mut letters = lower.chars();
            let expected = query.chars().all(|c| letters.any(|l| l == c));
            prop_assert_eq!(score(&query, &path).is_some(), expected);
        }

        #[test]
        fn the_highlight_names_the_query_letters_in_order(
            path in "[a-zA-Z0-9_./-]{1,40}",
            query in "[a-z0-9_.]{1,6}",
        ) {
            if let Some((_, ranges)) = q(&query).highlight(path.as_bytes()) {
                let marked: String = ranges
                    .iter()
                    .map(|r| path[r.clone()].to_ascii_lowercase())
                    .collect();
                prop_assert_eq!(marked, query);
            }
        }

        #[test]
        fn the_answer_does_not_depend_on_the_number_of_threads(
            paths in random_paths(),
            query in "[a-z0-9_.]{1,4}",
        ) {
            let query = q(&query);
            let path = |i: usize| paths[i].as_bytes();
            let one = top_matches_on(&query, paths.len(), 7, 1, &path, &|| false);
            for threads in [2, 3, 4, 8] {
                prop_assert_eq!(&one, &top_matches_on(&query, paths.len(), 7, threads, &path, &|| false));
            }
        }

        #[test]
        fn the_top_is_the_head_of_a_full_sort_by_score_then_order(
            paths in random_paths(),
            query in "[a-z0-9_.]{1,3}",
        ) {
            let query = q(&query);
            let (top, matched) =
                top_matches_on(&query, paths.len(), 5, 3, &|i| paths[i].as_bytes(), &|| false).unwrap();
            let mut all: Vec<(i32, Reverse<u32>)> = paths
                .iter()
                .enumerate()
                .filter_map(|(i, p)| query.score(p.as_bytes()).map(|s| (s, Reverse(u32::try_from(i).unwrap()))))
                .collect();
            all.sort_by(|a, b| b.cmp(a));
            prop_assert_eq!(matched, all.len());
            let expected: Vec<u32> = all.into_iter().take(5).map(|(_, Reverse(i))| i).collect();
            prop_assert_eq!(top, expected);
        }
    }
}
