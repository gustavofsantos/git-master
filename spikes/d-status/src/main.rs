//! Spike D harness: time `git status` via subprocess and `git2::Repository::statuses`.
//!
//! Usage (CSV on stdout, progress on stderr):
//!   spike-d-status proc  <repo> <label> <runs> [plain]   subprocess status; app command unless `plain`
//!   spike-d-status git2  <repo> <label> <runs> <mode>    libgit2; mode = tracked | untracked_flat | untracked_recurse
//!   spike-d-status fsmon <repo> <label> <runs>           daemon stopped, then started; first call + warm runs
//!   spike-d-status lock  <repo> <iterations>             app status vs concurrent `git add && git commit`
//!   spike-d-status count <repo>                          entry counts from both backends

use std::env;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Exactly what gm-core SubprocessGit runs (crates/gm-core/src/git.rs).
const APP_STATUS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--branch",
    "--untracked-files=normal",
];

fn subprocess_status(repo: &str, plain: bool) -> (f64, usize) {
    let mut c = Command::new("git");
    if !plain {
        c.arg("--no-optional-locks");
    }
    c.args(APP_STATUS).current_dir(repo).env("LC_ALL", "C").stdout(Stdio::piped()).stderr(Stdio::piped());
    let t = Instant::now();
    let out = c.output().expect("cannot run git");
    let secs = t.elapsed().as_secs_f64();
    assert!(out.status.success(), "git status failed: {}", String::from_utf8_lossy(&out.stderr));
    // Records are NUL-terminated; count them as a sanity figure.
    (secs, out.stdout.iter().filter(|&&b| b == 0).count())
}

fn git2_status(repo: &str, mode: &str) -> (f64, usize) {
    let r = git2::Repository::open(repo).expect("open");
    let mut opts = git2::StatusOptions::new();
    match mode {
        "tracked" => {
            opts.include_untracked(false);
        }
        "untracked_flat" => {
            opts.include_untracked(true).recurse_untracked_dirs(false);
        }
        "untracked_recurse" => {
            opts.include_untracked(true).recurse_untracked_dirs(true);
        }
        other => panic!("unknown git2 mode {other}"),
    }
    let t = Instant::now();
    let statuses = r.statuses(Some(&mut opts)).expect("statuses");
    let n = statuses.len();
    let secs = t.elapsed().as_secs_f64();
    (secs, n)
}

fn stats(mut xs: Vec<f64>) -> (f64, f64) {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    let median = if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 };
    // nearest-rank p95
    let rank = ((0.95 * n as f64).ceil() as usize).max(1);
    (median, xs[rank - 1])
}

fn report(kind: &str, repo: &str, label: &str, first: f64, warm: &[f64], entries: usize) {
    let (med, p95) = stats(warm.to_vec());
    println!(
        "{kind},{label},{repo},first_s={first:.3},runs={},median_s={med:.3},p95_s={p95:.3},entries={entries}",
        warm.len()
    );
    eprintln!("  {kind} {label}: first {first:.3}s, median {med:.3}s, p95 {p95:.3}s ({} warm runs)", warm.len());
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    match cmd {
        "proc" => {
            let (repo, label, runs) = (&args[2], &args[3], args[4].parse::<usize>().unwrap());
            let plain = args.get(5).map(String::as_str) == Some("plain");
            let kind = if plain { "proc_plain" } else { "proc_app" };
            let (first, entries) = subprocess_status(repo, plain);
            let warm: Vec<f64> = (0..runs).map(|_| subprocess_status(repo, plain).0).collect();
            report(kind, repo, label, first, &warm, entries);
        }
        "git2" => {
            let (repo, label, runs, mode) = (&args[2], &args[3], args[4].parse::<usize>().unwrap(), args[5].as_str());
            let (first, entries) = git2_status(repo, mode);
            let warm: Vec<f64> = (0..runs).map(|_| git2_status(repo, mode).0).collect();
            report(&format!("git2_{mode}"), repo, label, first, &warm, entries);
        }
        "fsmon" => {
            let (repo, label, runs) = (&args[2], &args[3], args[4].parse::<usize>().unwrap());
            let _ = Command::new("git").args(["fsmonitor--daemon", "stop"]).current_dir(repo).output();
            let started = Command::new("git")
                .args(["fsmonitor--daemon", "start"])
                .current_dir(repo)
                .output()
                .expect("start daemon");
            assert!(started.status.success(), "daemon start: {}", String::from_utf8_lossy(&started.stderr));
            // First call right after the daemon starts: its initial crawl is included.
            let (first, entries) = subprocess_status(repo, false);
            let warm: Vec<f64> = (0..runs).map(|_| subprocess_status(repo, false).0).collect();
            report("proc_app_fsmon", repo, label, first, &warm, entries);
        }
        "lock" => {
            let (repo, iters) = (&args[2], args[3].parse::<usize>().unwrap());
            lock_test(repo, iters);
        }
        "count" => {
            let repo = &args[2];
            let (_, plain_n) = subprocess_status(repo, true);
            let (_, g_flat) = git2_status(repo, "untracked_flat");
            let (_, g_rec) = git2_status(repo, "untracked_recurse");
            println!("count,{repo},proc_plain_records={plain_n},git2_flat_entries={g_flat},git2_recurse_entries={g_rec}");
        }
        _ => {
            eprintln!("see module docs for usage");
            std::process::exit(2);
        }
    }
}

/// One phase: a writer thread runs `git add` + `git commit` in a loop while the main thread
/// runs `iters` status commands (app = with --no-optional-locks, or plain). Reports status
/// failures, lock messages and commit failures for that phase only.
fn lock_phase(repo: &str, iters: usize, app: bool) -> String {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    let stop = Arc::new(AtomicBool::new(false));
    let commit_fail = Arc::new(AtomicUsize::new(0));
    let commit_ok = Arc::new(AtomicUsize::new(0));
    let writer = {
        let (repo, stop, fail, ok) = (repo.to_string(), stop.clone(), commit_fail.clone(), commit_ok.clone());
        std::thread::spawn(move || {
            let mut j = 0usize;
            while !stop.load(Ordering::SeqCst) {
                let path = format!("lock-commit-{app}-{j}.txt");
                std::fs::write(std::path::Path::new(&repo).join(&path), format!("c{j}\n")).unwrap();
                let add = Command::new("git").args(["add", "--", &path]).current_dir(&repo).output().unwrap();
                let commit = Command::new("git")
                    .args(["-c", "user.name=spike", "-c", "user.email=spike@example.invalid", "commit", "-q", "-m", "spike"])
                    .current_dir(&repo)
                    .output()
                    .unwrap();
                if add.status.success() && commit.status.success() {
                    ok.fetch_add(1, Ordering::SeqCst);
                } else {
                    fail.fetch_add(1, Ordering::SeqCst);
                }
                j += 1;
            }
        })
    };
    let (mut fails, mut lock_msgs, mut max_s) = (0usize, 0usize, 0f64);
    for _ in 0..iters {
        let mut c = Command::new("git");
        if app {
            c.arg("--no-optional-locks");
        }
        c.args(APP_STATUS).current_dir(repo).env("LC_ALL", "C");
        let t = Instant::now();
        let out = c.output().unwrap();
        max_s = max_s.max(t.elapsed().as_secs_f64());
        if !out.status.success() {
            fails += 1;
        }
        if String::from_utf8_lossy(&out.stderr).contains("index.lock") {
            lock_msgs += 1;
        }
    }
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    let name = if app { "app_no_optional_locks" } else { "plain_status" };
    format!(
        "{name}: status_failed={fails} status_index.lock_msgs={lock_msgs} status_max_s={max_s:.3} commits_ok={} commits_failed={}",
        commit_ok.load(Ordering::SeqCst),
        commit_fail.load(Ordering::SeqCst)
    )
}

fn lock_test(repo: &str, iters: usize) {
    for app in [true, false] {
        println!("lock,{repo},{}", lock_phase(repo, iters, app));
    }
}
