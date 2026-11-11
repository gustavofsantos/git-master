//! Measures code health on a real repository (slice 6h). Ignored by default; run it in a
//! release build against any repository:
//!
//! ```sh
//! GM_HEALTH_REPO=/path/to/repo cargo test --release -p gm-core --test health_measure -- --ignored --nocapture
//! ```
//!
//! It indexes the window of HEAD into a temporary `health.db` (the repository is only read),
//! then times a second, incremental pass, the load of the window and the indicators.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gm_core::health::{DAY_SECS, HealthConfig, config, ingest, metrics, store::HealthStore};

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
}
