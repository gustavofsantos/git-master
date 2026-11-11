//! Measures code health on a real repository (slice 6h, D228). Ignored by default; run it in a
//! release build against any repository:
//!
//! ```sh
//! GM_HEALTH_REPO=/path/to/repo cargo test --release -p gm-core --test health_measure -- --ignored --nocapture
//! ```
//!
//! It indexes the window of HEAD into a temporary `health.db` (the repository is only read),
//! then times a second, incremental pass, the load of the window and the indicators. Then it
//! indexes the hunks of the first-parent window, times a second pass, the replay of the lines
//! (`churn::follow`) and prints the churn of the files that exist at HEAD.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gm_core::health::{
    DAY_SECS, HealthConfig, churn, complexity, config, ingest, metrics, store::HealthStore,
};
use gm_schema::{ChurnSummary, Complexity, FileHealth};

#[test]
#[ignore = "measures a real repository named by GM_HEALTH_REPO"]
fn measure_health_on_a_real_repository() {
    let Some(root) = std::env::var_os("GM_HEALTH_REPO").map(PathBuf::from) else {
        eprintln!("GM_HEALTH_REPO is not set; nothing measured");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let mut store = HealthStore::open(&dir.path().join("health.db")).expect("open store");
    let config = config::read(&root).unwrap_or_else(|_| HealthConfig::default());
    let (sha, time) = ingest::resolve(&root, "HEAD").expect("resolve HEAD");
    let since = time - i64::from(config.window_days) * DAY_SECS;
    let stop = AtomicBool::new(false);

    let started = Instant::now();
    let first = ingest::ingest(&root, &mut store, &sha, since, &stop, &mut |_| {}).expect("ingest");
    let cold = started.elapsed();

    let started = Instant::now();
    ingest::ingest(&root, &mut store, &sha, since, &stop, &mut |_| {}).expect("second ingest");
    let warm = started.elapsed();

    let started = Instant::now();
    let window = ingest::window(&root, &sha, since).expect("window");
    let commits = store.load(&window).expect("load");
    let load = started.elapsed();

    let started = Instant::now();
    let files = metrics::compute(
        &commits,
        &config,
        None::<&std::collections::HashSet<String>>,
    );
    let compute = started.elapsed();

    let changes: usize = commits.iter().map(|c| c.changes.len()).sum();
    eprintln!(
        "window: {} commits, {changes} changes, {} files",
        first.total,
        files.len()
    );
    eprintln!("cold index: {cold:.2?}; incremental pass: {warm:.2?}");
    eprintln!("window + load: {load:.2?}; indicators: {compute:.2?}");
    for file in files.iter().take(10) {
        eprintln!(
            "  #{:<3} {:>8.2} {:?} {}",
            file.rank, file.heat, file.trend, file.path
        );
    }

    // Churn (D228): the hunks of the first-parent window, replayed.
    let started = Instant::now();
    let hunks_progress =
        ingest::ingest_hunks(&root, &mut store, &sha, since, &stop, &mut |_| {}).expect("hunks");
    let hunks_cold = started.elapsed();

    let started = Instant::now();
    ingest::ingest_hunks(&root, &mut store, &sha, since, &stop, &mut |_| {}).expect("hunks again");
    let hunks_warm = started.elapsed();

    let fp = ingest::first_parent_window(&root, &sha, since).expect("first-parent window");
    let mut hunks = store.load_hunks(&fp).expect("load hunks");
    hunks.reverse();
    let days: HashMap<&str, i64> = commits.iter().map(|c| (c.sha.as_str(), c.day())).collect();
    let mut kept = Vec::with_capacity(hunks.len());
    let mut kept_days = Vec::with_capacity(hunks.len());
    for commit in hunks {
        if let Some(&day) = days.get(commit.sha.as_str()) {
            kept_days.push(day);
            kept.push(commit);
        }
    }
    let hunk_count: usize = kept.iter().map(|c| c.hunks.len()).sum();

    let started = Instant::now();
    let followed = churn::follow(&kept, &kept_days, &config);
    let follow = started.elapsed();

    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-tree", "-r", "-z", "--name-only", &sha])
        .output()
        .expect("git ls-tree");
    let live: HashSet<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    let at_head = followed
        .iter()
        .filter(|(path, _)| live.contains(path.as_str()));
    let summary = ChurnSummary::of(at_head.map(|(_, churn)| churn));

    eprintln!(
        "hunks: {} first-parent commits, {hunk_count} hunks, {} files followed",
        fp.len(),
        followed.len()
    );
    eprintln!(
        "hunks cold index: {hunks_cold:.2?}; incremental pass: {hunks_warm:.2?} ({} commits in the window)",
        hunks_progress.total
    );
    eprintln!("churn replay: {follow:.2?}");
    eprintln!("churn of the files at HEAD: {summary:?}");

    measure_complexity(&root, &sha, &files, &stop);
}

/// The complexity of the hottest text files at `sha`, read as the index job reads them (D229).
fn measure_complexity(root: &std::path::Path, sha: &str, files: &[FileHealth], stop: &AtomicBool) {
    let tree = ingest::tree(root, sha).expect("tree");
    let largest = u64::try_from(complexity::MAX_BYTES).unwrap_or(u64::MAX);
    let hot: Vec<&str> = files
        .iter()
        .take(complexity::MEASURED_FILES)
        .map(|file| file.path.as_str())
        .collect();
    let ids: Vec<Option<String>> = hot
        .iter()
        .map(|path| {
            tree.blobs
                .get(*path)
                .filter(|blob| blob.bytes <= largest)
                .map(|blob| blob.id.clone())
        })
        .collect();
    let started = Instant::now();
    let measured = complexity::read(root, &ids, stop).expect("complexity");
    let read = started.elapsed();
    let count = measured.iter().flatten().count();
    eprintln!(
        "complexity read: {read:.2?} for {} hot files ({count} measured)",
        ids.len()
    );
    let mut deepest: Vec<(&str, &Complexity)> = hot
        .iter()
        .zip(measured.iter())
        .filter_map(|(path, value)| value.as_ref().map(|value| (*path, value)))
        .collect();
    deepest.sort_by(|a, b| {
        b.1.indent_total
            .cmp(&a.1.indent_total)
            .then_with(|| a.0.cmp(b.0))
    });
    for (path, value) in deepest.iter().take(5) {
        eprintln!(
            "  {path}: total {} max {} mean {} lines {} percentile {}",
            value.indent_total, value.indent_max, value.indent_mean, value.lines, value.percentile
        );
    }
}
