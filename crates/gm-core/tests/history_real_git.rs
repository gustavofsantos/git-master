//! The history read against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::git::SubprocessGit;
use gm_schema::{HISTORY_PAGE, History, RefKind};

/// Runs `git` in `dir` with a fixed identity; `when` sets both dates, so the
/// commits order by time without ties.
fn git_at(dir: &Path, when: &str, args: &[&str]) {
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
        .env("GIT_AUTHOR_DATE", when)
        .env("GIT_COMMITTER_DATE", when)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn git(dir: &Path, args: &[&str]) {
    git_at(dir, "2024-01-01T10:00:00+00:00", args);
}

fn commit(dir: &Path, when: &str, file: &str, subject: &str) {
    std::fs::write(dir.join(file), format!("{subject}\n")).unwrap();
    git(dir, &["add", file]);
    git_at(dir, when, &["commit", "-q", "-m", subject]);
}

/// The history of `dir` as the list reads it: every page of the walk, read
/// `page` commits at a time.
fn history_of(dir: &Path) -> History {
    walk(dir, HISTORY_PAGE)
}

fn walk(dir: &Path, page: usize) -> History {
    let root = SubprocessGit::default().resolve_root(dir).unwrap();
    let Some(mut stream) = SubprocessGit::default().history_stream(&root).unwrap() else {
        return History {
            commits: Vec::new(),
            unborn: true,
            more: false,
        };
    };
    let mut commits = Vec::new();
    loop {
        let read = stream.next_page(page).unwrap();
        commits.extend(read.commits);
        if read.done {
            return History {
                commits,
                unborn: false,
                more: false,
            };
        }
    }
}

/// `git rev-list --topo-order HEAD`, the order the walk must reproduce.
fn rev_list(dir: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["rev-list", "--topo-order", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Makes `commits` commits on `main` with `git fast-import`. Every 50th commit
/// is one on `side`, and every 50th after it merges `side` into `main`, so the
/// history is not a line.
fn fast_import(dir: &Path, commits: usize) {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::process::Stdio;

    let mut stream = String::new();
    let mut main: Option<usize> = None;
    let mut side: Option<usize> = None;
    for i in 0..commits {
        let mark = i + 1;
        let branch = if i % 50 == 25 { "side" } else { "main" };
        let _ = writeln!(stream, "commit refs/heads/{branch}");
        let _ = writeln!(stream, "mark :{mark}");
        let _ = writeln!(stream, "committer t <t@t> {} +0000", 1_700_000_000 + i);
        let message = format!("c{i}");
        let _ = writeln!(stream, "data {}\n{message}", message.len());
        if branch == "side" {
            if let Some(tip) = side.or(main) {
                let _ = writeln!(stream, "from :{tip}");
            }
            side = Some(mark);
        } else {
            if let Some(tip) = main {
                let _ = writeln!(stream, "from :{tip}");
            }
            if let Some(tip) = side.filter(|_| i % 50 == 49) {
                let _ = writeln!(stream, "merge :{tip}");
            }
            main = Some(mark);
        }
        stream.push('\n');
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

fn subjects(history: &History) -> Vec<&str> {
    history.commits.iter().map(|c| c.subject.as_str()).collect()
}

#[test]
fn lists_a_linear_history_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    commit(dir.path(), "2024-01-03T10:00:00+00:00", "c", "third");

    let history = history_of(dir.path());

    assert!(!history.unborn);
    assert_eq!(subjects(&history), ["third", "second", "first"]);
    let head = &history.commits[0];
    assert_eq!(head.oid.len(), 40);
    assert!(head.oid.starts_with(&head.short));
    assert_eq!(head.author_time, 1_704_276_000);
    assert_eq!(head.refs.len(), 2);
    assert_eq!(head.refs[0].name, "HEAD");
    assert_eq!(head.refs[0].kind, RefKind::Head);
    assert_eq!(head.refs[1].name, "main");
    assert_eq!(head.refs[1].kind, RefKind::Branch);
    assert!(history.commits[1].refs.is_empty());
}

#[test]
fn shows_a_merge_with_both_parents_in_the_list() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "base");
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "on feature");
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), "2024-01-03T10:00:00+00:00", "c", "on main");
    git_at(
        dir.path(),
        "2024-01-04T10:00:00+00:00",
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );

    let history = history_of(dir.path());

    // Topological order (H2): a branch's commits stay together, so "on feature"
    // comes before "on main". Git's default date order would put them the other way.
    assert_eq!(
        subjects(&history),
        ["merge feature", "on feature", "on main", "base"]
    );
    assert_eq!(history.commits[0].refs[0].kind, RefKind::Head);
}

/// `git rev-list --parents --topo-order HEAD`: each line is a commit and its
/// parents, the answer the walk's parents must match (H5).
fn rev_list_parents(dir: &Path) -> Vec<Vec<String>> {
    let out = Command::new("git")
        .args(["rev-list", "--parents", "--topo-order", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split(' ').map(str::to_owned).collect())
        .collect()
}

#[test]
fn parents_are_those_of_rev_list_with_the_first_parent_first() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "base");
    for (day, branch) in [(2, "one"), (3, "two"), (4, "three")] {
        git(dir.path(), &["checkout", "-q", "-b", branch, "main"]);
        commit(
            dir.path(),
            &format!("2024-01-0{day}T10:00:00+00:00"),
            branch,
            branch,
        );
    }
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), "2024-01-05T10:00:00+00:00", "m", "on main");
    // An octopus merge: the first parent is main, then the three branches.
    git_at(
        dir.path(),
        "2024-01-06T10:00:00+00:00",
        &[
            "merge", "-q", "--no-ff", "-m", "octopus", "one", "two", "three",
        ],
    );

    let history = history_of(dir.path());

    let walked: Vec<Vec<String>> = history
        .commits
        .iter()
        .map(|c| std::iter::once(c.oid.clone()).chain(c.parents.clone()).collect())
        .collect();
    assert_eq!(walked, rev_list_parents(dir.path()));
    assert_eq!(history.commits[0].subject, "octopus");
    assert_eq!(history.commits[0].parents.len(), 4);
    let first_parent = &history.commits[0].parents[0];
    let first = history.commits.iter().find(|c| &c.oid == first_parent);
    assert_eq!(first.unwrap().subject, "on main");
    let root = history.commits.last().unwrap();
    assert_eq!(root.subject, "base");
    assert!(root.parents.is_empty());
}

#[test]
fn a_root_commit_is_listed_last() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "only");

    let history = history_of(dir.path());

    assert_eq!(subjects(&history), ["only"]);
    assert!(!history.unborn);
}

#[test]
fn an_unborn_head_is_reported_with_no_commits() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);

    let history = history_of(dir.path());

    assert!(history.unborn);
    assert!(history.commits.is_empty());
}

#[test]
fn a_detached_head_is_labelled_head_on_its_commit() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    git(dir.path(), &["checkout", "-q", "--detach", "HEAD~1"]);

    let history = history_of(dir.path());

    // HEAD is on "first", so the list is that commit alone, labelled HEAD only.
    assert_eq!(subjects(&history), ["first"]);
    let head = &history.commits[0];
    assert_eq!(head.refs.len(), 1);
    assert_eq!(head.refs[0].name, "HEAD");
    assert_eq!(head.refs[0].kind, RefKind::Head);
}

#[test]
fn a_unicode_subject_comes_through_whole() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(
        dir.path(),
        "2024-01-01T10:00:00+00:00",
        "a",
        "Ajusta arredondamento de moeda: çãé 日本語 🚀 and a very long tail of words that goes on",
    );

    let history = history_of(dir.path());

    assert_eq!(
        history.commits[0].subject,
        "Ajusta arredondamento de moeda: çãé 日本語 🚀 and a very long tail of words that goes on"
    );
}

#[test]
fn a_tag_is_a_label_of_its_commit() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "release");
    git(dir.path(), &["tag", "v1.0"]);

    let history = history_of(dir.path());

    let tag = history.commits[0]
        .refs
        .iter()
        .find(|r| r.kind == RefKind::Tag)
        .unwrap();
    assert_eq!(tag.name, "v1.0");
}

#[test]
fn reading_the_history_records_nothing_and_writes_no_commit_graph() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    let log = gm_core::command_log::CommandLog::default();
    let git_port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    let root = git_port.resolve_root(dir.path()).unwrap();

    let mut stream = git_port.history_stream(&root).unwrap().unwrap();
    stream.next_page(HISTORY_PAGE).unwrap();

    assert!(log.entries().is_empty());
    assert!(!dir.path().join(".git/objects/info/commit-graph").exists());
}

#[test]
fn pages_of_a_5000_commit_history_are_rev_list_with_no_gap_or_repeat() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fast_import(dir.path(), 5000);
    let expected = rev_list(dir.path());
    assert_eq!(expected.len(), 5000);

    let history = history_of(dir.path());

    let got: Vec<String> = history.commits.iter().map(|c| c.oid.clone()).collect();
    assert_eq!(got, expected, "the pages must be the walk, in order");
    let unique: std::collections::BTreeSet<_> = got.iter().collect();
    assert_eq!(unique.len(), 5000, "no commit may repeat");
    // Reading never writes the commit-graph (D32).
    assert!(!dir.path().join(".git/objects/info/commit-graph").exists());
    assert!(!dir.path().join(".git/objects/info/commit-graphs").exists());
}

#[test]
fn a_page_after_a_cursor_resumes_the_walk_past_it() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fast_import(dir.path(), 5000);
    let expected = rev_list(dir.path());
    let root = SubprocessGit::default().resolve_root(dir.path()).unwrap();
    let mut stream = SubprocessGit::default()
        .history_stream(&root)
        .unwrap()
        .unwrap();

    stream.skip_past(&expected[1234]).unwrap();
    let page = stream.next_page(HISTORY_PAGE).unwrap();

    let got: Vec<String> = page.commits.iter().map(|c| c.oid.clone()).collect();
    assert_eq!(got, expected[1235..1435]);
    assert!(!page.done);
}

#[test]
fn stopping_a_walk_makes_its_next_read_fail() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fast_import(dir.path(), 5000);
    let root = SubprocessGit::default().resolve_root(dir.path()).unwrap();
    let mut stream = SubprocessGit::default()
        .history_stream(&root)
        .unwrap()
        .unwrap();
    let stop = stream.canceller();

    stop();

    assert!(stream.next_page(HISTORY_PAGE).is_err());
}

#[test]
fn an_external_commit_is_the_new_top_of_the_walk() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    let before = walk(dir.path(), HISTORY_PAGE);

    // Another tool commits while the list is open.
    commit(
        dir.path(),
        "2024-01-03T10:00:00+00:00",
        "c",
        "made elsewhere",
    );
    let after = walk(dir.path(), HISTORY_PAGE);

    assert_eq!(after.commits[0].subject, "made elsewhere");
    assert_eq!(after.commits[1].oid, before.commits[0].oid);
    assert_eq!(after.commits.len(), before.commits.len() + 1);
}

#[test]
fn a_second_top_load_after_an_outside_commit_shows_it_through_the_engine() {
    use gm_core::Engine;
    use gm_schema::{Actor, Command, CommandKind, Event, OperationId};

    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "2024-01-01T10:00:00+00:00", "a", "first");
    let mut engine = Engine::new(Box::new(SubprocessGit::default()));
    let command = |kind| Command {
        actor: Actor::Human,
        kind,
    };
    let root = dir.path().to_string_lossy().into_owned();
    engine.handle(
        OperationId(1),
        &command(CommandKind::OpenRepo { path: root }),
    );
    let run = |engine: &mut Engine| -> Vec<Event> {
        engine
            .take_history_jobs()
            .into_iter()
            .flat_map(|job| engine.history_report(job.run()))
            .collect()
    };
    engine.handle(OperationId(2), &command(CommandKind::LoadHistory));
    run(&mut engine);

    commit(dir.path(), "2024-01-02T10:00:00+00:00", "b", "second");
    let started = engine.handle(OperationId(3), &command(CommandKind::LoadHistory));
    let events = run(&mut engine);

    assert!(
        !started.iter().any(|e| matches!(
            e,
            Event::OperationChanged {
                state: gm_schema::OperationState::Failed,
                ..
            }
        )),
        "{started:?}"
    );
    let loaded = events.iter().find_map(|e| match e {
        Event::HistoryLoaded { history, .. } => Some(history.clone()),
        _ => None,
    });
    assert_eq!(
        subjects(&loaded.expect("the second load answers")),
        ["second", "first"]
    );
}
