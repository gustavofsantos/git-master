//! Backtests the code health indicators on a real repository (D225 (8), the research doc's
//! method). Ignored by default; run it in a release build:
//!
//! ```sh
//! GM_HEALTH_REPO=/path/to/repo CARGO_INCREMENTAL=0 cargo test --release -p gm-core --test health_backtest -- --ignored --nocapture
//! ```
//!
//! T is the newest first-parent commit of HEAD whose committer time is at least `GM_BACKTEST_DAYS`
//! (default 182) before HEAD's. The indicators are computed at T, with the window of T. The fixes
//! are the commits reachable from HEAD and not from T whose subject matches `GM_BACKTEST_FIX`. A
//! file counts as fixed when a later fix touched it, under the path it has at the fix: a rename
//! after T is not followed. Each indicator splits the files that exist at T into a top group and
//! the rest, and compares the share of each group with at least one later fix.
//!
//! The counting, the lift and the verdict are pure functions, checked by the unit tests below on
//! made-up data, which run in CI.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::health::config::{self, Excluder};
use gm_core::health::store::HealthStore;
use gm_core::health::{CommitRecord, DAY_SECS, HealthConfig, churn, ingest, metrics};
use gm_schema::{FileHealth, LineChurn, Trend};
use regex::Regex;

const DEFAULT_DAYS: u32 = 182;
const DEFAULT_FIX: &str =
    r"(?i)\b(fix|fixes|fixed|fixup|bug|bugs|regression|revert|reverts|broken|crash)\b";
/// Files with fewer lines added in the window than this have no churn to rank.
const MIN_ADDED: u32 = 50;
/// Commits of a file in the window below this do not count for the ownership indicator.
const MIN_OWNER_COMMITS: u32 = 5;
/// Files in the top group below this give no verdict.
const MIN_TOP_FILES: u32 = 20;
const KEEP_LIFT: f64 = 1.5;
const WEAK_LIFT: f64 = 1.0;

/// How many files of a group, how many had a later fix, and how many fixes they had in all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Group {
    files: u32,
    fixed: u32,
    fixes: u32,
}

impl Group {
    fn add(&mut self, fixes: u32) {
        self.files += 1;
        self.fixed += u32::from(fixes > 0);
        self.fixes = self.fixes.saturating_add(fixes);
    }

    /// Percent of the files with at least one later fix; 0 for an empty group.
    fn rate(self) -> f64 {
        ratio(self.fixed, self.files) * 100.0
    }

    /// Later fixes per file; 0 for an empty group.
    fn mean(self) -> f64 {
        ratio(self.fixes, self.files)
    }
}

fn ratio(part: u32, whole: u32) -> f64 {
    if whole == 0 {
        0.0
    } else {
        f64::from(part) / f64::from(whole)
    }
}

/// Splits `paths` by `is_top` and counts the later fixes of each file.
fn tally_where<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    is_top: impl Fn(&str) -> bool,
    fixes: &HashMap<String, u32>,
) -> (Group, Group) {
    let (mut top, mut rest) = (Group::default(), Group::default());
    for path in paths {
        let count = fixes.get(path).copied().unwrap_or(0);
        if is_top(path) {
            top.add(count);
        } else {
            rest.add(count);
        }
    }
    (top, rest)
}

/// The lift of the top group: its rate over the rest's. `None` when neither group has a fix;
/// infinite when only the top group has one.
fn lift(top: Group, rest: Group) -> Option<f64> {
    let (t, r) = (top.rate(), rest.rate());
    if r > 0.0 {
        Some(t / r)
    } else if t > 0.0 {
        Some(f64::INFINITY)
    } else {
        None
    }
}

/// `keep` or `weak` by the lift; `too few` when the top group is below `MIN_TOP_FILES`, which
/// wins over the lift.
fn verdict(top: Group, lift: Option<f64>) -> &'static str {
    if top.files < MIN_TOP_FILES {
        return "too few";
    }
    match lift {
        Some(l) if l >= KEEP_LIFT => "keep",
        Some(l) if l >= WEAK_LIFT => "weak",
        _ => "no signal",
    }
}

/// The size of the top decile of `n` files, rounded up.
fn decile(n: usize) -> usize {
    n.div_ceil(10)
}

/// The paths in the top decile of `ranked`, by value (highest first, then by path).
fn top_decile(mut ranked: Vec<(&str, f64)>) -> HashSet<&str> {
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let k = decile(ranked.len());
    ranked.into_iter().take(k).map(|(path, _)| path).collect()
}

/// One commit after T, as `git log --name-only` shows it.
#[derive(Debug, PartialEq, Eq)]
struct Later {
    subject: String,
    paths: Vec<String>,
}

/// Parses `git log --no-merges -z --format=%x01%H%x00%s --name-only`: each commit starts with
/// `\x01`, then its id, its subject and its paths, each one NUL-terminated, the first one after
/// a newline.
fn parse_log(bytes: &[u8]) -> Vec<Later> {
    String::from_utf8_lossy(bytes)
        .split('\u{1}')
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            let mut fields = chunk.split('\0');
            let _sha = fields.next();
            let subject = fields.next().unwrap_or_default().to_owned();
            let paths = fields
                .map(|field| field.trim_start_matches('\n'))
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect();
            Later { subject, paths }
        })
        .collect()
}

/// The later fixes of each path: the number of fix commits that touched it, each commit once per
/// path. Returns how many commits are fixes too.
fn count_fixes(log: &[Later], fix: &Regex) -> (usize, HashMap<String, u32>) {
    let mut fixes: HashMap<String, u32> = HashMap::new();
    let mut fix_commits = 0;
    for commit in log.iter().filter(|c| fix.is_match(&c.subject)) {
        fix_commits += 1;
        let mut seen = HashSet::new();
        for path in &commit.paths {
            if seen.insert(path.as_str()) {
                *fixes.entry(path.clone()).or_default() += 1;
            }
        }
    }
    (fix_commits, fixes)
}

/// How many commits of the window changed each path.
fn commit_counts(commits: &[CommitRecord]) -> HashMap<&str, u32> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for commit in commits {
        let mut seen = HashSet::new();
        for change in &commit.changes {
            if seen.insert(change.path.as_str()) {
                *counts.entry(change.path.as_str()).or_default() += 1;
            }
        }
    }
    counts
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Runs `git` in `root` and returns its standard output.
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["--no-optional-locks", "-c", "core.quotepath=off"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    output.stdout
}

/// The newest first-parent commit of `head` whose committer time is at most `cutoff`.
fn commit_before(root: &Path, head: &str, cutoff: i64) -> String {
    let text = String::from_utf8_lossy(&git(
        root,
        &["log", "--first-parent", "--format=%H %ct", head, "--"],
    ))
    .into_owned();
    text.lines()
        .find_map(|line| {
            let (sha, time) = line.split_once(' ')?;
            (time.trim().parse::<i64>().ok()? <= cutoff).then(|| sha.to_owned())
        })
        .unwrap_or_else(|| panic!("no first-parent commit of HEAD is that old"))
}

/// The indicators and the churn of the files that exist at T, from the index of T's window.
struct Snapshot {
    /// The files at T that the configuration does not exclude, sorted.
    universe: Vec<String>,
    /// T's window, newest first.
    commits: Vec<CommitRecord>,
    /// The indicators of the files with a change in the window.
    files: HashMap<String, FileHealth>,
    /// The churn of the files at T, keyed by their path at T.
    churn: HashMap<String, LineChurn>,
}

fn snapshot(root: &Path, t: &str, since: i64, config: &HealthConfig) -> Snapshot {
    let mut store = HealthStore::open_in_memory().expect("open store");
    let stop = AtomicBool::new(false);
    ingest::ingest(root, &mut store, t, since, &stop, &mut |_| {}).expect("ingest");
    ingest::ingest_hunks(root, &mut store, t, since, &stop, &mut |_| {}).expect("ingest hunks");

    let window = ingest::window(root, t, since).expect("window");
    let commits = store.load(&window).expect("load");
    let live: HashSet<String> =
        String::from_utf8_lossy(&git(root, &["ls-tree", "-r", "-z", "--name-only", t]))
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
    let files = metrics::compute(&commits, config, Some(&live))
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();

    let first_parent = ingest::first_parent_window(root, t, since).expect("first-parent window");
    let mut hunks = store.load_hunks(&first_parent).expect("load hunks");
    hunks.reverse();
    let days: HashMap<&str, i64> = commits.iter().map(|c| (c.sha.as_str(), c.day())).collect();
    let (kept, kept_days): (Vec<_>, Vec<_>) = hunks
        .into_iter()
        .filter_map(|commit| days.get(commit.sha.as_str()).map(|&day| (commit, day)))
        .unzip();
    let churn = churn::follow(&kept, &kept_days, config);

    let excluder = Excluder::new(config);
    let mut universe: Vec<String> = live.into_iter().filter(|p| !excluder.excluded(p)).collect();
    universe.sort();
    Snapshot {
        universe,
        commits,
        files,
        churn,
    }
}

/// The commits reachable from HEAD and not from T, parsed (`T..HEAD`, no merges).
fn later_commits(root: &Path, t: &str, head: &str) -> Vec<Later> {
    let range = format!("{t}..{head}");
    let out = git(
        root,
        &[
            "log",
            "--no-merges",
            "-z",
            "--format=%x01%H%x00%s",
            "--name-only",
            &range,
        ],
    );
    parse_log(&out)
}

/// One indicator's line of the table.
struct Row {
    name: &'static str,
    top: Group,
    rest: Group,
}

/// The files the rest group is drawn from: all of `universe`, or with `changed_only`, those with
/// at least one commit in the window (the top group is always drawn from the same pool).
fn pool<'a>(
    universe: &'a [String],
    counts: &HashMap<&str, u32>,
    changed_only: bool,
) -> Vec<&'a str> {
    universe
        .iter()
        .map(String::as_str)
        .filter(|p| !changed_only || counts.get(p).is_some_and(|&n| n > 0))
        .collect()
}

fn rows(s: &Snapshot, fixes: &HashMap<String, u32>, changed_only: bool) -> Vec<Row> {
    let heat_k = to_u32(decile(s.universe.len()));
    let reported = |p: &str| s.files.get(p);
    let counts = commit_counts(&s.commits);
    let pool = pool(&s.universe, &counts, changed_only);
    let owner_paths: Vec<&str> = pool
        .iter()
        .copied()
        .filter(|p| counts.get(p).copied().unwrap_or(0) >= MIN_OWNER_COMMITS)
        .collect();

    let measured: Vec<(&str, f64)> = pool
        .iter()
        .filter_map(|&p| {
            let c = s.churn.get(p)?;
            (c.added >= MIN_ADDED).then(|| (p, ratio(c.rewritten, c.added)))
        })
        .collect();
    let churn_top = top_decile(measured.clone());
    let churn_paths = measured.iter().map(|(p, _)| *p);

    let all = || pool.iter().copied();
    let mut out = Vec::new();
    let mut push =
        |name: &'static str, (top, rest): (Group, Group)| out.push(Row { name, top, rest });
    push(
        "heat top 10%",
        tally_where(
            all(),
            |p| reported(p).is_some_and(|f| f.rank <= heat_k),
            fixes,
        ),
    );
    push(
        "trend rising",
        tally_where(
            all(),
            |p| reported(p).is_some_and(|f| f.trend == Trend::Rising),
            fixes,
        ),
    );
    push(
        "dormancy break",
        tally_where(
            all(),
            |p| reported(p).is_some_and(|f| f.dormancy_break.is_some()),
            fixes,
        ),
    );
    push(
        "no clear owner",
        tally_where(
            owner_paths,
            |p| {
                reported(p).is_some_and(|f| {
                    f.constraints
                        .iter()
                        .any(|c| c.starts_with("No clear owner"))
                })
            },
            fixes,
        ),
    );
    push(
        "coupled (degree >= 0.5)",
        tally_where(
            all(),
            |p| reported(p).is_some_and(|f| f.changes_together.iter().any(|q| q.degree >= 0.5)),
            fixes,
        ),
    );
    push(
        "churn top 10%",
        tally_where(churn_paths, |p| churn_top.contains(p), fixes),
    );
    push(
        "any constraint",
        tally_where(
            all(),
            |p| reported(p).is_some_and(|f| !f.constraints.is_empty()),
            fixes,
        ),
    );
    out
}

fn print_row(row: &Row, changed_only: bool) {
    let l = lift(row.top, row.rest);
    let lift_text = match l {
        Some(v) if v.is_infinite() => "inf".to_owned(),
        Some(v) => format!("{v:.2}"),
        None => "n/a".to_owned(),
    };
    let label = if changed_only {
        format!("  vs changed: {}", row.name)
    } else {
        row.name.to_owned()
    };
    println!(
        "{label:<38} | top {:>6}, fix rate {:>5.1}% | rest {:>6}, fix rate {:>5.1}% | lift {:>5} | mean fixes top {:.3} vs rest {:.3} | {}",
        row.top.files,
        row.top.rate(),
        row.rest.files,
        row.rest.rate(),
        lift_text,
        row.top.mean(),
        row.rest.mean(),
        verdict(row.top, l),
    );
}

#[test]
#[ignore = "backtests the indicators on a real repository named by GM_HEALTH_REPO"]
fn backtest_indicators_on_a_real_repository() {
    let Some(root) = env::var_os("GM_HEALTH_REPO").map(PathBuf::from) else {
        eprintln!("GM_HEALTH_REPO is not set; nothing backtested");
        return;
    };
    let days: u32 = env::var("GM_BACKTEST_DAYS").map_or(DEFAULT_DAYS, |v| {
        v.parse().expect("GM_BACKTEST_DAYS is a number of days")
    });
    let fix_pattern = env::var("GM_BACKTEST_FIX").unwrap_or_else(|_| DEFAULT_FIX.to_owned());
    let fix = Regex::new(&fix_pattern).expect("GM_BACKTEST_FIX is a regular expression");
    let config = config::read(&root).unwrap_or_else(|_| HealthConfig::default());

    let (head, head_time) = ingest::resolve(&root, "HEAD").expect("resolve HEAD");
    let t = commit_before(&root, &head, head_time - i64::from(days) * DAY_SECS);
    let (t, t_time) = ingest::resolve(&root, &t).expect("resolve T");
    let since = t_time - i64::from(config.window_days) * DAY_SECS;

    let snap = snapshot(&root, &t, since, &config);
    let log = later_commits(&root, &t, &head);
    let (fix_commits, fixes) = count_fixes(&log, &fix);
    let fixed_at_t = snap
        .universe
        .iter()
        .filter(|p| fixes.contains_key(p.as_str()))
        .count();
    let base = tally_where(snap.universe.iter().map(String::as_str), |_| true, &fixes).0;

    let date = |rev: &str| {
        String::from_utf8_lossy(&git(&root, &["log", "-1", "--format=%cs", rev]))
            .trim()
            .to_owned()
    };
    println!("repository: {}", root.display());
    println!(
        "T: {} ({}), HEAD: {} ({}), T is at least {days} days before HEAD",
        &t[..12],
        date(&t),
        &head[..12],
        date(&head),
    );
    println!(
        "window of T: {} commits ({} days, {} files with a change), {} churn rows",
        snap.commits.len(),
        config.window_days,
        snap.files.len(),
        snap.churn.len(),
    );
    let counts = commit_counts(&snap.commits);
    println!(
        "files at T: {} (after exclusions); {} with a change in the window",
        snap.universe.len(),
        snap.files.len()
    );
    println!(
        "vs changed: rest group drawn from the {} files with a commit in the window",
        pool(&snap.universe, &counts, true).len()
    );
    println!(
        "later commits: {} non-merge commits after T, {} fix commits ({fix_pattern}), {fixed_at_t} files at T with a later fix",
        log.len(),
        fix_commits,
    );
    println!("paths after T are taken as they are at the fix; renames after T are not followed");
    println!(
        "base rate: {} files, fix rate {:.1}%, mean fixes per file {:.3}",
        base.files,
        base.rate(),
        base.mean(),
    );
    println!();
    for (all, changed) in rows(&snap, &fixes, false)
        .into_iter()
        .zip(rows(&snap, &fixes, true))
    {
        print_row(&all, false);
        print_row(&changed, true);
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn fix() -> Regex {
        Regex::new(DEFAULT_FIX).expect("default fix pattern")
    }

    #[test]
    fn parse_log_reads_subjects_and_paths() {
        let bytes = b"\x01aaa\0Fix the crash\0\nsrc/a.rs\0src/b.rs\0\x01bbb\0Add feature\0\nsrc/a.rs\0\x01ccc\0Merge\0\x01ddd\0Bugfix\0\nsrc/c.rs\0";
        let log = parse_log(bytes);
        assert_eq!(log.len(), 4);
        assert_eq!(log[0].subject, "Fix the crash");
        assert_eq!(log[0].paths, ["src/a.rs", "src/b.rs"]);
        assert_eq!(log[2].paths, Vec::<String>::new());
        assert_eq!(log[3].paths, ["src/c.rs"]);
    }

    #[test]
    fn fixes_count_each_path_once_per_commit() {
        let log = vec![
            Later {
                subject: "Fix parser".into(),
                paths: vec!["a".into(), "a".into(), "b".into()],
            },
            Later {
                subject: "Add parser".into(),
                paths: vec!["a".into()],
            },
            Later {
                subject: "Revert: drop cache".into(),
                paths: vec!["b".into()],
            },
            Later {
                subject: "prefixed fixture".into(),
                paths: vec!["c".into()],
            },
        ];
        let (commits, fixes) = count_fixes(&log, &fix());
        assert_eq!(commits, 2);
        assert_eq!(fixes.get("a"), Some(&1));
        assert_eq!(fixes.get("b"), Some(&2));
        assert_eq!(fixes.get("c"), None);
    }

    #[test]
    fn group_rate_and_mean() {
        let mut group = Group::default();
        group.add(0);
        group.add(2);
        group.add(1);
        assert_eq!(group.files, 3);
        assert!((group.rate() - 200.0 / 3.0).abs() < 1e-9);
        assert!((group.mean() - 1.0).abs() < 1e-9);
        assert!(Group::default().rate().abs() < f64::EPSILON);
    }

    #[test]
    fn tally_splits_by_the_predicate() {
        let fixes = HashMap::from([("hot".to_owned(), 1), ("cold".to_owned(), 0)]);
        let (top, rest) = tally_where(["hot", "cold", "other"], |p| p == "hot", &fixes);
        assert_eq!((top.files, top.fixed), (1, 1));
        assert_eq!((rest.files, rest.fixed), (2, 0));
    }

    #[test]
    fn lift_and_verdict() {
        let group = |files: u32, fixed: u32| Group {
            files,
            fixed,
            fixes: fixed,
        };
        assert_eq!(lift(group(20, 10), group(20, 5)), Some(2.0));
        assert_eq!(lift(group(20, 0), group(20, 0)), None);
        assert_eq!(lift(group(20, 3), group(20, 0)), Some(f64::INFINITY));

        assert_eq!(verdict(group(20, 10), Some(2.0)), "keep");
        assert_eq!(verdict(group(20, 10), Some(1.0)), "weak");
        assert_eq!(verdict(group(20, 10), Some(0.8)), "no signal");
        assert_eq!(verdict(group(20, 10), None), "no signal");
        assert_eq!(verdict(group(19, 10), Some(3.0)), "too few");
        assert_eq!(verdict(group(5, 5), Some(f64::INFINITY)), "too few");
    }

    #[test]
    fn changed_pool_keeps_only_files_with_a_commit_in_the_window() {
        let universe: Vec<String> = ["a", "b", "c"].map(str::to_owned).into();
        let counts = HashMap::from([("a", 2), ("c", 0)]);
        assert_eq!(pool(&universe, &counts, false), ["a", "b", "c"]);
        assert_eq!(pool(&universe, &counts, true), ["a"]);

        let fixes = HashMap::from([("a".to_owned(), 1), ("b".to_owned(), 3)]);
        let changed = pool(&universe, &counts, true);
        let (top, rest) = tally_where(changed, |p| p == "a", &fixes);
        assert_eq!((top.files, rest.files), (1, 0));
    }

    #[test]
    fn decile_rounds_up() {
        assert_eq!(decile(0), 0);
        assert_eq!(decile(1), 1);
        assert_eq!(decile(10), 1);
        assert_eq!(decile(11), 2);
    }

    #[test]
    fn top_decile_ranks_by_value_then_path() {
        let ranked = vec![("c", 0.1), ("a", 0.5), ("b", 0.5), ("d", 0.0)];
        let top = top_decile(ranked);
        assert_eq!(top, HashSet::from(["a"]));
        let all_tied = vec![
            ("z", 1.0),
            ("y", 1.0),
            ("x", 1.0),
            ("w", 1.0),
            ("v", 1.0),
            ("u", 1.0),
            ("t", 1.0),
            ("s", 1.0),
            ("r", 1.0),
            ("q", 1.0),
            ("p", 1.0),
        ];
        assert_eq!(top_decile(all_tied), HashSet::from(["p", "q"]));
    }
}
