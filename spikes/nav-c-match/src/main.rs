//! Spike S-C (docs/09): a path matcher over a million paths.
//!
//! Usage: spike-nav-c-match <paths-file> [runs]
//! `<paths-file>` is the output of `git ls-files -z`.
//!
//! Compares three things on the same queries:
//!   baseline  the palette's `fuzzy_match` on every path (Vec<char> per call), then a sort
//!   bytes-1   a byte matcher with path bonuses, one thread, top-K heap
//!   bytes-N   the same, over N threads, merged deterministically
//! and checks that the result does not depend on the thread count.

mod baseline;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

const TOP_K: usize = 50;

/// The paths in one buffer, `starts[i]..starts[i + 1]` being path `i`.
struct Paths {
    bytes: Vec<u8>,
    starts: Vec<u32>,
}

impl Paths {
    fn from_nul_separated(raw: Vec<u8>) -> Self {
        let mut starts = vec![0u32];
        for (at, &b) in raw.iter().enumerate() {
            if b == 0 {
                starts.push(u32::try_from(at + 1).unwrap());
            }
        }
        // `raw` ends with a NUL: the last start is the end of the buffer.
        Self { bytes: raw, starts }
    }

    fn len(&self) -> usize {
        self.starts.len() - 1
    }

    fn get(&self, index: usize) -> &[u8] {
        let (a, b) = (self.starts[index] as usize, self.starts[index + 1] as usize - 1);
        &self.bytes[a..b]
    }
}

// --- the byte matcher -----------------------------------------------------------------------

#[inline]
fn lower(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

#[inline]
fn is_separator(b: u8) -> bool {
    matches!(b, b'/' | b'_' | b'-' | b'.' | b' ')
}

/// The score of `path` for the lowercase ASCII `query`, or `None` when the query is not a
/// subsequence. Higher is better. The window of the match is found forward, then tightened
/// backward, so a late short match beats an early scattered one.
fn score(query: &[u8], path: &[u8]) -> Option<i32> {
    let n = query.len();
    if n == 0 {
        return Some(0);
    }
    // Forward: the first place the whole query fits as a subsequence.
    let mut qi = 0;
    let mut end = 0;
    for (i, &b) in path.iter().enumerate() {
        if lower(b) == query[qi] {
            qi += 1;
            if qi == n {
                end = i;
                break;
            }
        }
    }
    if qi < n {
        return None;
    }
    // Backward: the tightest start for that end. Positions are kept for scoring.
    let mut at = [0usize; 32];
    let n = n.min(32);
    let mut qi = n;
    let mut i = end + 1;
    while qi > 0 {
        i -= 1;
        if lower(path[i]) == query[qi - 1] {
            qi -= 1;
            at[qi] = i;
        }
    }
    let name_start = path.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1);
    let mut total = 0i32;
    for k in 0..n {
        let p = at[k];
        let mut s = 16;
        let before = if p == 0 { b'/' } else { path[p - 1] };
        if is_separator(before) {
            s += if before == b'/' { 28 } else { 20 };
        } else if path[p].is_ascii_uppercase() && path[p - 1].is_ascii_lowercase() {
            s += 18;
        }
        if k > 0 {
            let gap = p - at[k - 1] - 1;
            if gap == 0 {
                s += 24;
            } else {
                s -= i32::try_from(gap.min(16)).unwrap();
            }
        }
        if p >= name_start {
            s += 10;
        }
        total += s;
    }
    // The file name matched whole at its start is the best result there is.
    if at[0] == name_start && at[n - 1] - at[0] + 1 == n {
        total += 60;
    }
    // Shorter paths first, slightly.
    total -= i32::try_from(path.len() / 8).unwrap();
    Some(total)
}

/// A candidate ordered by score, then by path order (the index), never by arrival.
type Entry = (i32, Reverse<u32>);

fn top_k_range(paths: &Paths, query: &[u8], from: usize, to: usize, k: usize) -> Vec<Entry> {
    // A min-heap on (score, earlier index wins): the root is the worst kept entry.
    let mut heap: BinaryHeap<Reverse<Entry>> = BinaryHeap::with_capacity(k + 1);
    for index in from..to {
        let Some(s) = score(query, paths.get(index)) else {
            continue;
        };
        let entry = (s, Reverse(u32::try_from(index).unwrap()));
        if heap.len() < k {
            heap.push(Reverse(entry));
        } else if entry > heap.peek().unwrap().0 {
            heap.pop();
            heap.push(Reverse(entry));
        }
    }
    heap.into_iter().map(|Reverse(e)| e).collect()
}

fn top_k(paths: &Paths, query: &[u8], threads: usize, k: usize) -> Vec<u32> {
    let n = paths.len();
    let chunk = n.div_ceil(threads.max(1));
    let mut all: Vec<Entry> = if threads <= 1 {
        top_k_range(paths, query, 0, n, k)
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let (from, to) = (t * chunk, ((t + 1) * chunk).min(n));
                    scope.spawn(move || top_k_range(paths, query, from, to.max(from), k))
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
        })
    };
    all.sort_by(|a, b| b.cmp(a));
    all.truncate(k);
    all.into_iter().map(|(_, Reverse(i))| i).collect()
}

// --- the baseline ----------------------------------------------------------------------------

fn baseline_top_k(paths: &Paths, query: &str, k: usize) -> Vec<u32> {
    let mut hits: Vec<(i64, u32)> = Vec::new();
    for index in 0..paths.len() {
        let text = std::str::from_utf8(paths.get(index)).unwrap();
        if let Some(m) = baseline::fuzzy_match(query, text) {
            hits.push((m.score, u32::try_from(index).unwrap()));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.truncate(k);
    hits.into_iter().map(|(_, i)| i).collect()
}

// --- the harness -----------------------------------------------------------------------------

fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("paths file (git ls-files -z)");
    let runs: usize = args.next().map_or(7, |r| r.parse().unwrap());
    let threads = std::thread::available_parallelism().map_or(1, usize::from);

    let t = Instant::now();
    let paths = Paths::from_nul_separated(std::fs::read(&file).unwrap());
    println!(
        "loaded {} paths, {} KiB buffer + {} KiB offsets in {:.0} ms; threads={threads}",
        paths.len(),
        paths.bytes.len() / 1024,
        paths.starts.len() * 4 / 1024,
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Queries typed the way people type: a file name, a fragment of the path, short and
    // noisy ones, and one that matches nothing.
    let queries = [
        "file_0500",
        "corsrc",
        "docsmd",
        "ui/sr",
        "auth",
        "rs",
        "x",
        "mod12",
        "crates/ui/src/file",
        "zzzzqq",
    ];
    println!("query,matches,baseline_ms,bytes1_ms,bytesN_ms,same_top_as_1thread,same_top_baseline");
    for q in queries {
        let qb = q.as_bytes();
        let one = top_k(&paths, qb, 1, TOP_K);
        let many = top_k(&paths, qb, threads, TOP_K);
        let four = top_k(&paths, qb, 4, TOP_K);
        let seven = top_k(&paths, qb, 7, TOP_K);
        assert!(one == many && one == four && one == seven, "result depends on thread count for {q:?}");
        let matches = (0..paths.len()).filter(|&i| score(qb, paths.get(i)).is_some()).count();

        // The baseline is slow at a million paths: fewer runs.
        let base_runs = if paths.len() > 200_000 { 1 } else { runs.min(3) };
        let mut base = Vec::new();
        let base_ms = median_ms(base_runs, || base = baseline_top_k(&paths, q, TOP_K));
        let b1 = median_ms(runs, || {
            top_k(&paths, qb, 1, TOP_K);
        });
        let bn = median_ms(runs, || {
            top_k(&paths, qb, threads, TOP_K);
        });
        // How much of the baseline's top 10 the new matcher also puts in its top 10.
        let overlap = base.iter().take(10).filter(|i| one.iter().take(10).any(|j| j == *i)).count();
        println!("{q},{matches},{base_ms:.1},{b1:.1},{bn:.1},true,{overlap}/10");
        if std::env::var_os("SHOW").is_some() {
            for (label, list) in [("new", &one), ("old", &base)] {
                for i in list.iter().take(4) {
                    println!("    {label}: {}", String::from_utf8_lossy(paths.get(*i as usize)));
                }
            }
        }
    }
}
