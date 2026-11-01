//! Measurement for Q34 (D144): the shortest run for `line_match`, and how `post_oid` is hashed.
//! Ignored by default; it reads a copy of a real `threads.db` and a real repository, and never
//! writes to either:
//!
//! ```text
//! CARGO_INCREMENTAL=0 GM_MEASURE_DB=<copy of threads.db> GM_MEASURE_REPO=<repo> \
//!   cargo test --release -p gm-core --test provenance_measure -- --ignored --nocapture
//! ```
//!
//! The repository must be a clean checkout of the commit to measure, so blame sees no
//! uncommitted lines. The pipeline is the one `ReadProvenance` runs (blame, owners, the blob of
//! each commit, the stored edits, `judge`), written against the public rules, with the shortest
//! run as a parameter. Each range is timed end to end for each N.

use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_core::threads::provenance::{
    COMMIT_LIMIT, CONTEXT, EDIT_LIMIT, EditFact, Verdict, judge_with, owners,
};
use gm_core::threads::store::ThreadStore;
use gm_schema::Evidence;

/// The shortest runs measured (Q34).
const CANDIDATES: [usize; 6] = [2, 3, 4, 5, 6, 8];
/// Lines in one window.
const WINDOW: u32 = 20;
/// Windows sampled per file, evenly spread over it.
const WINDOWS_PER_FILE: u32 = 8;
/// The longest range a query may ask for (`RANGE_LIMIT`).
const LONGEST: u32 = 1000;

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("set {name}")))
}

/// Copies the database and its WAL and shared-memory files into a directory of its own, and
/// opens the copy read-only through the store's own path layout.
fn copied_store(source: &Path, dir: &Path) -> ThreadStore {
    let target = dir.join("gitmaster");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::copy(source, target.join("threads.db")).expect("copy threads.db");
    for suffix in ["-wal", "-shm"] {
        let mut from = source.as_os_str().to_owned();
        from.push(suffix);
        let from = PathBuf::from(from);
        if from.exists() {
            std::fs::copy(&from, target.join(format!("threads.db{suffix}"))).unwrap();
        }
    }
    ThreadStore::open_read_only(dir).expect("open the copy")
}

/// Every path a stored edit names, with its edit count, most edited first.
fn edited_paths(dir: &Path) -> Vec<(String, u32)> {
    let connection = rusqlite::Connection::open_with_flags(
        dir.join("gitmaster").join("threads.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the copy for reading");
    let mut statement = connection
        .prepare("SELECT path, COUNT(*) FROM edit GROUP BY path ORDER BY COUNT(*) DESC, path")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[derive(Default)]
struct Outcome {
    /// Some edit line-matched the range.
    line_match: bool,
    /// Some commit of the range had more than one decisive edit, and one of them line-matched.
    ambiguous: bool,
    /// Edits that line-matched, over all commits of the range.
    line_match_edits: u32,
    /// Line-match edits whose commit has a blob match from another edit (proxy b).
    fp_other_blob: u32,
    /// Line-match edits whose recorded post image differs from the commit's blob (proxy a).
    fp_post_differs: u32,
}

/// One range through the pipeline with the shortest run `min_run`. Returns the outcome and the
/// time the whole read took.
fn measure(
    git: &SubprocessGit,
    store: &ThreadStore,
    repo: &Path,
    path: &str,
    from: u32,
    to: u32,
    min_run: usize,
) -> (Outcome, Duration, Duration) {
    let started = Instant::now();
    let read = git.blame_lines(repo, path, from, to).expect("blame starts");
    let lines = read(&AtomicBool::new(false)).expect("blame");
    let blame = started.elapsed();
    let mut outcome = Outcome::default();
    for owner in owners(&lines).iter().take(COMMIT_LIMIT) {
        let blob = git
            .blob_at_commit(repo, &owner.commit, &owner.path)
            .expect("blob at commit");
        let rows = store
            .path_edits(
                &owner.path,
                owner.author_at,
                blob.as_deref(),
                u32::try_from(EDIT_LIMIT + 1).unwrap(),
            )
            .expect("path edits");
        let judged: Vec<(EditFact, Verdict)> = rows
            .into_iter()
            .take(EDIT_LIMIT)
            .map(|row| {
                let fact = EditFact::new(row);
                let verdict = judge_with(owner, blob.as_deref(), &fact, min_run);
                (fact, verdict)
            })
            .filter(|(_, verdict)| !verdict.evidence.is_empty() || verdict.unverifiable.is_some())
            .collect();
        for (fact, _) in &judged {
            // The engine reads the entries around every judged edit; time the same reads.
            store
                .entries_around(&fact.thread_uid, fact.seq, CONTEXT)
                .expect("entries around");
        }
        let decisive: Vec<&(EditFact, Verdict)> =
            judged.iter().filter(|(_, v)| v.decisive()).collect();
        let blob_edits: Vec<i64> = judged
            .iter()
            .filter(|(f, _)| blob.is_some() && f.post_oid == blob)
            .map(|(f, _)| f.edit_id)
            .collect();
        let line_matches: Vec<&EditFact> = judged
            .iter()
            .filter(|(_, v)| v.evidence.contains(&Evidence::LineMatch))
            .map(|(fact, _)| fact)
            .collect();
        if line_matches.is_empty() {
            continue;
        }
        outcome.line_match = true;
        if decisive.len() > 1 {
            outcome.ambiguous = true;
        }
        for fact in line_matches {
            outcome.line_match_edits += 1;
            if !blob_edits.is_empty() && !blob_edits.contains(&fact.edit_id) {
                outcome.fp_other_blob += 1;
            }
            if let (Some(post), Some(b)) = (&fact.post_oid, &blob)
                && post != b
            {
                outcome.fp_post_differs += 1;
            }
        }
    }
    (outcome, blame, started.elapsed())
}

/// The nearest-rank percentile of `sorted` (ascending), `percent` from 0 to 100.
fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (sorted.len() * percent)
        .div_ceil(100)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Default)]
struct Totals {
    ranges: u32,
    line_match: u32,
    ambiguous: u32,
    line_match_edits: u32,
    fp_other_blob: u32,
    fp_post_differs: u32,
    times: Vec<Duration>,
    blames: Vec<Duration>,
}

/// The ranges of one mode: whole files up to `LONGEST` lines, or evenly spread windows.
fn ranges(repo: &Path, paths: &[(String, u32)], windows: bool) -> (Vec<(String, u32, u32)>, u32) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for (path, _) in paths {
        let Ok(text) = std::fs::read(repo.join(path)) else {
            skipped += 1;
            continue;
        };
        let newlines = text
            .iter()
            .fold(0_usize, |count, byte| count + usize::from(*byte == b'\n'));
        let unterminated = usize::from(text.last().is_some_and(|byte| *byte != b'\n'));
        let lines = u32::try_from(newlines + unterminated).unwrap_or(u32::MAX);
        if lines == 0 {
            continue;
        }
        if !windows {
            if lines <= LONGEST {
                out.push((path.clone(), 1, lines));
            } else {
                skipped += 1;
            }
            continue;
        }
        if lines < WINDOW {
            continue;
        }
        let count = (lines / WINDOW).clamp(1, WINDOWS_PER_FILE);
        for index in 0..count {
            let span = lines - WINDOW;
            let start = 1 + if count > 1 {
                index * span / (count - 1)
            } else {
                0
            };
            out.push((path.clone(), start, start + WINDOW - 1));
        }
    }
    (out, skipped)
}

#[test]
#[ignore = "Q34 measurement: needs GM_MEASURE_DB and GM_MEASURE_REPO, run in release"]
fn shortest_line_run_on_real_threads() {
    let source = env_path("GM_MEASURE_DB");
    let repo = env_path("GM_MEASURE_REPO");
    let dir = tempfile::tempdir().unwrap();
    let store = copied_store(&source, dir.path());
    // GM_MEASURE_SUFFIX limits the sample to paths with that ending, such as `.rs`.
    let suffix = std::env::var("GM_MEASURE_SUFFIX").unwrap_or_default();
    let paths: Vec<(String, u32)> = edited_paths(dir.path())
        .into_iter()
        .filter(|(path, _)| path.ends_with(&suffix))
        .collect();
    let git = SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let head = Process::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();
    println!(
        "repo {} at {}",
        repo.display(),
        String::from_utf8_lossy(&head.stdout).trim()
    );
    println!("edited paths: {} (suffix {suffix:?})", paths.len());

    for (mode, windows) in [("whole", false), ("window20", true)] {
        let (list, skipped) = ranges(&repo, &paths, windows);
        println!(
            "\n== {mode}: {} ranges, {skipped} files skipped (longer than {LONGEST} lines or unreadable)",
            list.len()
        );
        let mut totals: Vec<Totals> = CANDIDATES.iter().map(|_| Totals::default()).collect();
        for (path, from, to) in &list {
            for (slot, min_run) in CANDIDATES.iter().enumerate() {
                let (outcome, blame, total) =
                    measure(&git, &store, &repo, path, *from, *to, *min_run);
                let t = &mut totals[slot];
                t.ranges += 1;
                t.line_match += u32::from(outcome.line_match);
                t.ambiguous += u32::from(outcome.ambiguous);
                t.line_match_edits += outcome.line_match_edits;
                t.fp_other_blob += outcome.fp_other_blob;
                t.fp_post_differs += outcome.fp_post_differs;
                t.times.push(total);
                t.blames.push(blame);
            }
        }
        for (slot, min_run) in CANDIDATES.iter().enumerate() {
            let t = &mut totals[slot];
            t.times.sort();
            t.blames.sort();
            let share = if t.line_match == 0 {
                0.0
            } else {
                100.0 * f64::from(t.ambiguous) / f64::from(t.line_match)
            };
            let fp_other = if t.line_match_edits == 0 {
                0.0
            } else {
                100.0 * f64::from(t.fp_other_blob) / f64::from(t.line_match_edits)
            };
            let fp_post = if t.line_match_edits == 0 {
                0.0
            } else {
                100.0 * f64::from(t.fp_post_differs) / f64::from(t.line_match_edits)
            };
            println!(
                "N={min_run} ranges={} with_line_match={} ({:.1}%) ambiguous={} ({share:.1}% of with_line_match) line_match_edits={} fp_other_blob={} ({fp_other:.1}%) fp_post_differs={} ({fp_post:.1}%)",
                t.ranges,
                t.line_match,
                100.0 * f64::from(t.line_match) / f64::from(t.ranges.max(1)),
                t.ambiguous,
                t.line_match_edits,
                t.fp_other_blob,
                t.fp_post_differs,
            );
            println!(
                "    total per range ms: p50={:.1} p95={:.1} p99={:.1} max={:.1} | blame only p95={:.1}",
                ms(percentile(&t.times, 50)),
                ms(percentile(&t.times, 95)),
                ms(percentile(&t.times, 99)),
                ms(*t.times.last().unwrap_or(&Duration::ZERO)),
                ms(percentile(&t.blames, 95)),
            );
        }
    }
}

/// How `post_oid` relates to `git hash-object`: raw (what the engine stores) against
/// `--path` (which applies clean filters, CRLF and LFS), over the edits of the copy.
#[test]
#[ignore = "Q34 measurement: needs GM_MEASURE_DB and GM_MEASURE_REPO, run in release"]
fn post_oid_against_hash_object_path() {
    let source = env_path("GM_MEASURE_DB");
    let repo = env_path("GM_MEASURE_REPO");
    let dir = tempfile::tempdir().unwrap();
    copied_store(&source, dir.path());
    let connection = rusqlite::Connection::open_with_flags(
        dir.path().join("gitmaster").join("threads.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = connection
        .prepare("SELECT path, post_oid FROM edit WHERE post_oid IS NOT NULL")
        .unwrap();
    let edits: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    let (mut raw_equal, mut path_equal, mut neither, mut missing) = (0, 0, 0, 0);
    let (mut attributed, mut attributed_mismatch) = (0, 0);
    let mut attribute_paths: Vec<String> = Vec::new();
    for (path, post) in &edits {
        let file = repo.join(path);
        let Ok(bytes) = std::fs::read(&file) else {
            missing += 1;
            continue;
        };
        let raw = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)
            .unwrap()
            .to_string();
        let with_path = hash_object_path(&repo, path);
        let crlf = bytes.windows(2).any(|pair| pair == b"\r\n");
        let attr = attribute_of(&repo, path);
        if crlf || attr {
            attributed += 1;
            attribute_paths.push(path.clone());
            if raw != with_path {
                attributed_mismatch += 1;
            }
        }
        if *post == raw {
            raw_equal += 1;
        } else if *post == with_path {
            path_equal += 1;
        } else {
            neither += 1;
        }
    }
    println!(
        "post_oid edits with a file in the checkout: {} (missing: {missing})",
        edits.len() - missing
    );
    println!(
        "  equal to raw hash of the current file: {raw_equal}\n  equal only to hash-object --path: {path_equal}\n  equal to neither (file changed after the edit, or the post image was not the file): {neither}"
    );
    println!(
        "  files with CRLF or a git attribute: {attributed} (raw and --path differ on {attributed_mismatch}); {attribute_paths:?}"
    );
}

fn hash_object_path(repo: &Path, path: &str) -> String {
    let output = Process::new("git")
        .args(["hash-object", "--path", path, path])
        .current_dir(repo)
        .stdin(Stdio::null())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn attribute_of(repo: &Path, path: &str) -> bool {
    let output = Process::new("git")
        .args(["check-attr", "--all", "--", path])
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| !line.ends_with("unspecified"))
}

/// Times `ReadProvenance` through the engine at the shipped N: the call, the job's blame on this
/// thread, and the report. The store is the one the engine opens, so the repository must hold
/// a copy of the database at `.git/gitmaster/threads.db`. Run it on a clone, never on the
/// repository the threads were written in.
#[test]
#[ignore = "Q34 measurement: needs GM_MEASURE_DB and a clone whose .git/gitmaster/threads.db is a copy"]
fn engine_read_latency_at_the_shipped_n() {
    let source = env_path("GM_MEASURE_DB");
    let repo = env_path("GM_MEASURE_REPO");
    let dir = tempfile::tempdir().unwrap();
    copied_store(&source, dir.path());
    let suffix = std::env::var("GM_MEASURE_SUFFIX").unwrap_or_default();
    let paths: Vec<(String, u32)> = edited_paths(dir.path())
        .into_iter()
        .filter(|(path, _)| path.ends_with(&suffix))
        .collect();
    let store_file = repo.join(".git").join("gitmaster").join("threads.db");
    assert!(
        store_file.exists(),
        "the clone needs its store at {}",
        store_file.display()
    );
    let git = SubprocessGit {
        global_config: Some("/dev/null".into()),
        ..SubprocessGit::default()
    };
    let mut engine = gm_core::Engine::new(Box::new(git));
    let opened = engine.handle(
        gm_schema::OperationId(1),
        &gm_schema::Command {
            actor: gm_schema::Actor::Human,
            kind: gm_schema::CommandKind::OpenRepo {
                path: repo.to_string_lossy().into_owned(),
            },
        },
    );
    assert!(opened.iter().any(|event| matches!(
        event,
        gm_schema::Event::OperationChanged {
            state: gm_schema::OperationState::Done,
            ..
        }
    )));
    for (mode, windows) in [("whole", false), ("window20", true)] {
        let (list, _) = ranges(&repo, &paths, windows);
        let mut times = Vec::new();
        let mut answered = 0_u32;
        for (index, (path, from, to)) in list.iter().enumerate() {
            let id = gm_schema::OperationId(2 + index as u64);
            let started = Instant::now();
            let _ = engine.handle(
                id,
                &gm_schema::Command {
                    actor: gm_schema::Actor::Human,
                    kind: gm_schema::CommandKind::ReadProvenance {
                        path: path.clone(),
                        from_line: *from,
                        to_line: *to,
                    },
                },
            );
            for job in engine.take_provenance_jobs() {
                let events = engine.provenance_report(job.run());
                answered += u32::from(
                    events
                        .iter()
                        .any(|event| matches!(event, gm_schema::Event::ThreadProvenance { .. })),
                );
            }
            times.push(started.elapsed());
        }
        times.sort();
        println!(
            "engine {mode}: {} ranges, {answered} answered, p50={:.1} ms p95={:.1} ms p99={:.1} ms max={:.1} ms",
            list.len(),
            ms(percentile(&times, 50)),
            ms(percentile(&times, 95)),
            ms(percentile(&times, 99)),
            ms(*times.last().unwrap_or(&Duration::ZERO)),
        );
    }
}
