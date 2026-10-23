//! The text search against real repositories in temporary directories (N3).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_core::grep::GrepRequest;
use gm_schema::{GREP_FILE_LIMIT, GREP_TOTAL_LIMIT, GrepOptions, GrepResult, GrepScope};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

fn commit_all(dir: &Path) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

fn run(dir: &Path, query: &str, options: GrepOptions, paths: &str) -> Result<GrepResult, String> {
    let git = SubprocessGit::default();
    let root = git.resolve_root(dir).unwrap();
    let request = GrepRequest::new(query, options, paths)?;
    let read = git.grep_text(&root, &request).map_err(|e| e.0)?;
    read(&AtomicBool::new(false)).map_err(|e| e.0)
}

fn find(dir: &Path, query: &str) -> GrepResult {
    run(dir, query, GrepOptions::default(), "").unwrap()
}

/// `path:line` of every listed line, in the order the answer gives them.
fn hits(result: &GrepResult) -> Vec<String> {
    result
        .files
        .iter()
        .flat_map(|f| {
            f.lines
                .iter()
                .map(move |l| format!("{}:{}", f.path, l.line))
        })
        .collect()
}

#[test]
fn matching_lines_come_with_path_line_text_and_the_highlight() {
    let dir = repo();
    write(dir.path(), "src/b.rs", "fn one() {}\n    let needle = 1;\n");
    write(dir.path(), "a.txt", "first\nsecond NEEDLE here\n");
    commit_all(dir.path());

    let result = find(dir.path(), "needle");

    assert_eq!(hits(&result), ["a.txt:2", "src/b.rs:2"]);
    let line = &result.files[0].lines[0];
    assert_eq!(line.text, "second NEEDLE here");
    assert_eq!(line.ranges, [(7, 13)]);
    let indented = &result.files[1].lines[0];
    assert_eq!(indented.text, "let needle = 1;", "indentation is dropped");
    assert_eq!(indented.ranges, [(4, 10)]);
    assert_eq!(result.listed, 2);
    assert!(!result.truncated);
    assert_eq!(result.query, "needle");
}

#[test]
fn no_match_is_an_empty_answer_not_an_error() {
    let dir = repo();
    write(dir.path(), "a.txt", "nothing here\n");
    commit_all(dir.path());

    let result = find(dir.path(), "zzz");

    assert!(result.files.is_empty());
    assert_eq!(result.listed, 0);
}

#[test]
fn case_whole_word_and_regex_options_change_what_matches() {
    let dir = repo();
    write(dir.path(), "a.txt", "foo\nFoo\nfoobar\na.c\nabc\n");
    commit_all(dir.path());
    let lines = |query: &str, case, word, regex| -> Vec<String> {
        let result = run(
            dir.path(),
            query,
            GrepOptions {
                case_sensitive: case,
                whole_word: word,
                regex,
                ..GrepOptions::default()
            },
            "",
        )
        .unwrap();
        hits(&result)
    };

    assert_eq!(
        lines("foo", false, false, false),
        ["a.txt:1", "a.txt:2", "a.txt:3"]
    );
    assert_eq!(lines("foo", true, false, false), ["a.txt:1", "a.txt:3"]);
    assert_eq!(lines("foo", false, true, false), ["a.txt:1", "a.txt:2"]);
    // A fixed string reads the dot literally; a regex does not.
    assert_eq!(lines("a.c", false, false, false), ["a.txt:4"]);
    assert_eq!(lines("a.c", false, false, true), ["a.txt:4", "a.txt:5"]);
}

#[test]
fn text_that_looks_like_an_option_is_searched_for_not_obeyed() {
    let dir = repo();
    write(
        dir.path(),
        "a.txt",
        "run with -v now\nuse --help\nthe -e flag\nplain -\n",
    );
    commit_all(dir.path());

    for (query, line) in [("-v", 1), ("--help", 2), ("-e", 3), ("-", 4)] {
        let result = find(dir.path(), query);
        assert!(
            hits(&result).contains(&format!("a.txt:{line}")),
            "{query}: {:?}",
            hits(&result)
        );
    }
    assert_eq!(hits(&find(dir.path(), "--")), ["a.txt:2"]);
}

#[test]
fn an_invalid_regex_fails_with_gits_message() {
    let dir = repo();
    write(dir.path(), "a.txt", "x\n");
    commit_all(dir.path());

    let error = run(
        dir.path(),
        "(",
        GrepOptions {
            regex: true,
            ..GrepOptions::default()
        },
        "",
    )
    .unwrap_err();

    assert!(error.contains("Unmatched"), "{error}");
}

#[test]
fn binary_files_are_not_searched_and_ignored_ones_neither() {
    let dir = repo();
    write(dir.path(), "text.txt", "needle\n");
    std::fs::write(dir.path().join("blob.bin"), b"needle\0binary").unwrap();
    write(dir.path(), ".gitignore", "ignored.txt\n");
    write(dir.path(), "ignored.txt", "needle\n");
    commit_all(dir.path());

    assert_eq!(hits(&find(dir.path(), "needle")), ["text.txt:1"]);
}

#[test]
fn untracked_files_are_searched_and_a_file_gone_from_disk_is_not() {
    let dir = repo();
    write(dir.path(), "tracked.txt", "needle\n");
    write(dir.path(), "gone.txt", "needle\n");
    commit_all(dir.path());
    write(dir.path(), "fresh.txt", "needle\n");
    std::fs::remove_file(dir.path().join("gone.txt")).unwrap();

    assert_eq!(
        hits(&find(dir.path(), "needle")),
        ["fresh.txt:1", "tracked.txt:1"]
    );
}

#[test]
fn untracked_files_are_left_out_when_the_option_is_off() {
    let dir = repo();
    write(dir.path(), "tracked.txt", "needle\n");
    commit_all(dir.path());
    write(dir.path(), "fresh.txt", "needle\n");
    let tracked_only = GrepOptions {
        scope: GrepScope::TrackedOnly,
        ..GrepOptions::default()
    };

    let result = run(dir.path(), "needle", tracked_only, "").unwrap();

    assert_eq!(hits(&result), ["tracked.txt:1"]);
}

#[test]
fn names_with_spaces_unicode_quotes_and_a_leading_dash_come_through() {
    let dir = repo();
    let names = [
        "a b.txt",
        "ação/relatório.md",
        "q\"uote.txt",
        "-dash.txt",
        "tab\there.txt",
    ];
    for name in names {
        write(dir.path(), name, "needle\n");
    }
    commit_all(dir.path());

    let result = find(dir.path(), "needle");

    let mut expected: Vec<String> = names.iter().map(|n| format!("{n}:1")).collect();
    expected.sort();
    assert_eq!(hits(&result), expected);
}

#[test]
fn pathspecs_include_and_exclude() {
    let dir = repo();
    write(dir.path(), "crates/a.rs", "needle\n");
    write(dir.path(), "docs/b.md", "needle\n");
    write(dir.path(), "tests/c.rs", "needle\n");
    commit_all(dir.path());
    let with =
        |paths: &str| hits(&run(dir.path(), "needle", GrepOptions::default(), paths).unwrap());

    assert_eq!(with("crates"), ["crates/a.rs:1"]);
    assert_eq!(with("crates docs"), ["crates/a.rs:1", "docs/b.md:1"]);
    assert_eq!(with("!docs"), ["crates/a.rs:1", "tests/c.rs:1"]);
    assert_eq!(with("*.rs !tests"), ["crates/a.rs:1"]);
}

#[test]
fn a_pathspec_git_cannot_read_fails_with_gits_message() {
    let dir = repo();
    write(dir.path(), "a.txt", "needle\n");
    commit_all(dir.path());

    let error = run(
        dir.path(),
        "needle",
        GrepOptions::default(),
        ":(nosuchmagic)x",
    )
    .unwrap_err();

    assert!(
        error.to_lowercase().contains("pathspec") || error.contains("magic"),
        "{error}"
    );
}

#[test]
fn a_file_lists_a_hundred_lines_and_counts_the_rest() {
    let dir = repo();
    write(dir.path(), "many.txt", &"needle\n".repeat(130));
    commit_all(dir.path());

    let result = find(dir.path(), "needle");

    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].lines.len(), GREP_FILE_LIMIT);
    assert_eq!(result.files[0].more, 30);
    assert!(!result.truncated);
}

#[test]
fn the_search_stops_at_the_total_limit_and_says_so() {
    let dir = repo();
    // 30 files of 40 lines: 1200 lines, over the limit of 1000.
    for n in 0..30 {
        write(dir.path(), &format!("f{n:02}.txt"), &"needle\n".repeat(40));
    }
    commit_all(dir.path());

    let result = find(dir.path(), "needle");

    assert_eq!(result.listed, GREP_TOTAL_LIMIT);
    assert!(result.truncated);
    let again = find(dir.path(), "needle");
    assert_eq!(result, again, "the same search gives the same answer");
    // The files listed are the first ones in path order: Git ran on one thread.
    assert_eq!(result.files[0].path, "f00.txt");
}

#[test]
fn the_same_search_gives_the_same_answer_every_time() {
    let dir = repo();
    for n in 0..40 {
        write(
            dir.path(),
            &format!("d{}/f{n:02}.txt", n % 5),
            "x needle y\nneedle\n",
        );
    }
    commit_all(dir.path());

    let first = find(dir.path(), "needle");
    for _ in 0..3 {
        assert_eq!(find(dir.path(), "needle"), first);
    }
    let paths: Vec<&str> = first.files.iter().map(|f| f.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
}

#[test]
fn a_long_line_is_cut_around_the_match() {
    let dir = repo();
    let line = format!("{} needle {}", "a".repeat(600), "b".repeat(600));
    write(dir.path(), "long.txt", &format!("{line}\n"));
    commit_all(dir.path());

    let result = find(dir.path(), "needle");

    let shown = &result.files[0].lines[0];
    assert!(shown.cut_start && shown.cut_end);
    assert!(shown.text.len() <= gm_schema::GREP_LINE_LIMIT);
    let (a, b) = shown.ranges[0];
    assert_eq!(&shown.text[a..b], "needle");
}

#[test]
fn a_stopped_search_fails_without_an_answer() {
    let dir = repo();
    write(dir.path(), "a.txt", "needle\n");
    commit_all(dir.path());
    let git = SubprocessGit::default();
    let root = git.resolve_root(dir.path()).unwrap();
    let request = GrepRequest::new("needle", GrepOptions::default(), "").unwrap();
    let read = git.grep_text(&root, &request).unwrap();

    assert!(read(&AtomicBool::new(true)).is_err());
}

#[test]
fn searching_writes_nothing_into_the_repository() {
    let dir = repo();
    write(dir.path(), "a.txt", "needle\n");
    commit_all(dir.path());
    let index = dir.path().join(".git/index");
    let before = std::fs::metadata(&index).unwrap().modified().unwrap();

    find(dir.path(), "needle");

    assert_eq!(
        before,
        std::fs::metadata(&index).unwrap().modified().unwrap()
    );
    assert!(!dir.path().join(".git/index.lock").exists());
}

/// Tests that stand in for Git with a script that prints records and then hangs,
/// to see that the search ends without waiting for Git.
#[cfg(unix)]
mod with_a_script_for_git {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    /// A directory with a `git` script first on the path.
    fn fake_git(script: &str) -> (tempfile::TempDir, SubprocessGit) {
        let bin = tempfile::tempdir().unwrap();
        let path = bin.path().join("git");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let git = SubprocessGit::default();
        git.login_path
            .set(format!("{}:/usr/bin:/bin", bin.path().display()))
            .unwrap();
        (bin, git)
    }

    fn request() -> GrepRequest {
        GrepRequest::new("x", GrepOptions::default(), "").unwrap()
    }

    #[test]
    fn git_is_killed_when_the_result_is_full_not_waited_for() {
        // 55 files of 20 lines, then Git "keeps working" for a minute. `exec` makes the sleep the
        // process that is killed: a child of the shell would outlive it and hold the pipe open.
        let (_bin, git) = fake_git(
            "i=0; while [ $i -lt 1100 ]; do \
             printf 'f%03d.rs\\000%d\\0001\\000x\\n' $((i / 20)) $((i % 20 + 1)); i=$((i + 1)); done; \
             exec sleep 60",
        );
        let dir = tempfile::tempdir().unwrap();
        let read = git.grep_text(dir.path(), &request()).unwrap();

        let started = Instant::now();
        let result = read(&AtomicBool::new(false)).unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(result.listed, GREP_TOTAL_LIMIT);
        assert!(result.truncated);
    }

    #[test]
    fn a_stop_request_kills_git_and_ends_the_search() {
        let (_bin, git) = fake_git("printf 'a.rs\\0001\\0001\\000x\\n'; exec sleep 60");
        let dir = tempfile::tempdir().unwrap();
        let read = git.grep_text(dir.path(), &request()).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&stop);
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let started = Instant::now();
        let outcome = read(&stop);
        stopper.join().unwrap();

        assert!(outcome.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_exit_code_of_two_or_more_fails_with_the_first_line_git_wrote() {
        let (_bin, git) = fake_git("echo 'warning: noise' >&2; echo 'fatal: boom' >&2; exit 128");
        let dir = tempfile::tempdir().unwrap();
        let read = git.grep_text(dir.path(), &request()).unwrap();

        let error = read(&AtomicBool::new(false)).unwrap_err();

        assert_eq!(error.0, "warning: noise");
    }

    #[test]
    fn exit_one_with_no_output_is_no_match() {
        let (_bin, git) = fake_git("exit 1");
        let dir = tempfile::tempdir().unwrap();
        let read = git.grep_text(dir.path(), &request()).unwrap();

        let result = read(&AtomicBool::new(false)).unwrap();

        assert!(result.files.is_empty());
    }
}

/// The S-A benchmark through the adapter the app uses (design/code-navigation.md, N3).
///
/// `GM_BENCH_REPO=<repo with a worktree> cargo test -p gm-core --test grep_real_git
/// -- --ignored --nocapture grep_timings`. The repository comes from the `gen.sh <files> <dir> checkout`
/// script of the nav-a-lists spike (git history, 7932c61): `common_token` is in one
/// file of ten, `rare_token` in one of 20 000, `needle_unique` in one.
#[test]
#[ignore = "needs GM_BENCH_REPO, a large generated repository"]
fn grep_timings() {
    let repo = std::env::var("GM_BENCH_REPO").expect("GM_BENCH_REPO is not set");
    let dir = Path::new(&repo);
    let runs = 3;
    for (scope_label, scope) in [
        ("tracked and new", GrepScope::TrackedAndNew),
        ("tracked only", GrepScope::TrackedOnly),
    ] {
        for (label, token) in [
            ("common (stops at the limit)", "common_token"),
            ("rare", "rare_token"),
            ("unique", "needle_unique"),
            ("none", "token_that_is_not_there"),
        ] {
            let options = GrepOptions {
                scope,
                ..GrepOptions::default()
            };
            let mut times = Vec::new();
            let mut listed = 0;
            for _ in 0..runs {
                let started = std::time::Instant::now();
                let result = run(dir, token, options, "").unwrap();
                times.push(started.elapsed().as_secs_f64());
                listed = result.listed;
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{scope_label:<16} {label:<28} {:>7.3} s (median of {runs}), {listed} lines",
                times[runs / 2]
            );
        }
    }
}
