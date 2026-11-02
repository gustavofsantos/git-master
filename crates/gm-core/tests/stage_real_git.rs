//! Stage, unstage and commit against real repositories in temporary directories.

use std::path::Path;
use std::process::Command;

use gm_core::GitPort;
use gm_core::command_log::CommandLog;
use gm_core::git::SubprocessGit;
use gm_schema::{DiffSide, FileDiff, HunkId, LineKind, StageTarget};

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
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
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn git(dir: &Path, args: &[&str]) -> String {
    String::from_utf8(git_bytes(dir, args)).unwrap()
}

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// The index entry of `name` as git reads it, raw bytes of the blob.
fn show_index(dir: &Path, name: &str) -> Vec<u8> {
    git_bytes(dir, &["show", &format!(":{name}")])
}

fn index_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(".git/index")).unwrap()
}

fn assert_no_lock(dir: &Path) {
    assert!(
        !dir.join(".git/index.lock").exists(),
        "index.lock left behind"
    );
}

/// A repository with one commit holding `files`. Identity is set locally so
/// `git commit` finds it.
fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    for (name, content) in files {
        write(dir.path(), name, content);
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn unstaged(dir: &Path, path: &str) -> FileDiff {
    SubprocessGit::default()
        .diff(dir, path, DiffSide::Unstaged)
        .unwrap()
}

fn staged(dir: &Path, path: &str) -> FileDiff {
    SubprocessGit::default()
        .diff(dir, path, DiffSide::Staged)
        .unwrap()
}

fn hunk_target(path: &str, id: &HunkId) -> StageTarget {
    StageTarget::Hunk {
        path: path.to_owned(),
        hunk: id.clone(),
    }
}

fn file_target(path: &str) -> StageTarget {
    StageTarget::File {
        path: path.to_owned(),
    }
}

#[test]
fn stages_and_unstages_a_whole_file() {
    let dir = repo_with(&[("a.txt", "one\n"), ("b.txt", "one\n")]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();

    port.stage(dir.path(), &file_target("a.txt")).unwrap();
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"]),
        "a.txt\n"
    );
    assert_eq!(git(dir.path(), &["diff", "--name-only"]), "");

    port.unstage(dir.path(), &file_target("a.txt")).unwrap();
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "");
    assert_eq!(git(dir.path(), &["diff", "--name-only"]), "a.txt\n");
    assert_no_lock(dir.path());
}

#[test]
fn stages_one_hunk_and_leaves_the_other() {
    let original: String = numbered("line", 20);
    let edited = original
        .replace("line 2\n", "two\n")
        .replace("line 19\n", "nineteen\n");
    let dir = repo_with(&[("f.txt", original.as_str())]);
    write(dir.path(), "f.txt", &edited);
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "f.txt");
    assert_eq!(diff.hunks.len(), 2);
    port.stage(dir.path(), &hunk_target("f.txt", &diff.hunks[0].id))
        .unwrap();

    let expected = original.replace("line 2\n", "two\n");
    assert_eq!(show_index(dir.path(), "f.txt"), expected.as_bytes());
    let rest = unstaged(dir.path(), "f.txt");
    assert_eq!(rest.hunks.len(), 1);
    assert!(rest.hunks[0].lines.iter().any(|l| l.text == "nineteen"));
    assert_no_lock(dir.path());
}

#[test]
fn stages_selected_lines_of_a_two_line_change() {
    let dir = repo_with(&[("f.txt", "one\ntwo\nthree\n")]);
    write(dir.path(), "f.txt", "ONE\nTWO\nthree\n");
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "f.txt");
    assert_eq!(diff.hunks.len(), 1);
    let hunk = &diff.hunks[0];
    // Select the removed "one" and the added "ONE" only.
    let selected: Vec<u32> = hunk
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            (l.kind == LineKind::Removed && l.text == "one")
                || (l.kind == LineKind::Added && l.text == "ONE")
        })
        .map(|(i, _)| u32::try_from(i).unwrap())
        .collect();
    assert_eq!(selected.len(), 2);

    port.stage(
        dir.path(),
        &StageTarget::Lines {
            path: "f.txt".into(),
            hunk: hunk.id.clone(),
            lines: selected,
        },
    )
    .unwrap();

    // Added lines are placed after the removed lines of their change block,
    // so the unselected "two" comes before the staged "ONE".
    assert_eq!(show_index(dir.path(), "f.txt"), b"two\nONE\nthree\n");
    let rest = unstaged(dir.path(), "f.txt");
    assert_eq!(rest.hunks.len(), 1);
    assert_no_lock(dir.path());
}

#[test]
fn stages_a_hunk_of_an_untracked_file() {
    let dir = repo_with(&[("a.txt", "x\n")]);
    write(dir.path(), "new.txt", "alpha\nbeta\n");
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "new.txt");
    assert_eq!(diff.hunks.len(), 1);
    port.stage(dir.path(), &hunk_target("new.txt", &diff.hunks[0].id))
        .unwrap();

    assert_eq!(show_index(dir.path(), "new.txt"), b"alpha\nbeta\n");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "A  new.txt\n");
    assert_no_lock(dir.path());
}

#[test]
fn untracked_path_with_leading_dash_is_diffed_as_a_path() {
    let dir = repo_with(&[("a.txt", "x\n")]);
    write(dir.path(), "-x.txt", "dash\n");

    let diff = unstaged(dir.path(), "-x.txt");
    assert_eq!(diff.path, "-x.txt");
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.old_path, None);
}

#[test]
fn stages_a_hunk_when_the_file_has_no_newline_at_eof() {
    let dir = repo_with(&[("f.txt", "a\nb")]);
    write(dir.path(), "f.txt", "a\nB");
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "f.txt");
    port.stage(dir.path(), &hunk_target("f.txt", &diff.hunks[0].id))
        .unwrap();

    assert_eq!(show_index(dir.path(), "f.txt"), b"a\nB");
    assert_eq!(git(dir.path(), &["diff", "--name-only"]), "");
    assert_no_lock(dir.path());
}

#[test]
fn stages_a_hunk_of_a_crlf_file_keeping_the_carriage_returns() {
    let dir = repo_with(&[("f.txt", "a\r\nb\r\nc\r\n")]);
    write(dir.path(), "f.txt", "a\r\nB\r\nc\r\n");
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "f.txt");
    port.stage(dir.path(), &hunk_target("f.txt", &diff.hunks[0].id))
        .unwrap();

    assert_eq!(show_index(dir.path(), "f.txt"), b"a\r\nB\r\nc\r\n");
    assert_no_lock(dir.path());
}

#[cfg(unix)]
#[test]
fn keeps_the_executable_bit_when_staging_a_hunk() {
    use std::os::unix::fs::PermissionsExt;

    let dir = repo_with(&[("run.sh", "echo one\n")]);
    let path = dir.path().join("run.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(dir.path(), &["add", "run.sh"]);
    git(dir.path(), &["commit", "-q", "-m", "exec"]);
    write(dir.path(), "run.sh", "echo two\n");
    let port = SubprocessGit::default();

    let diff = unstaged(dir.path(), "run.sh");
    port.stage(dir.path(), &hunk_target("run.sh", &diff.hunks[0].id))
        .unwrap();

    assert!(git(dir.path(), &["ls-files", "-s", "run.sh"]).starts_with("100755 "));
    assert_eq!(show_index(dir.path(), "run.sh"), b"echo two\n");
    assert_no_lock(dir.path());
}

#[cfg(unix)]
#[test]
fn stages_a_mode_only_change_as_a_whole_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = repo_with(&[("run.sh", "echo one\n")]);
    let path = dir.path().join("run.sh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    SubprocessGit::default()
        .stage(dir.path(), &file_target("run.sh"))
        .unwrap();
    assert!(git(dir.path(), &["ls-files", "-s", "run.sh"]).starts_with("100755 "));
    assert_no_lock(dir.path());
}

#[test]
fn stages_and_unstages_a_deletion() {
    let dir = repo_with(&[("a.txt", "one\n"), ("b.txt", "two\n")]);
    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    std::fs::remove_file(dir.path().join("b.txt")).unwrap();
    let port = SubprocessGit::default();

    port.stage(dir.path(), &file_target("a.txt")).unwrap();
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-status"]),
        "D\ta.txt\n"
    );

    // A deleted file's only hunk, staged whole, removes the index entry.
    let diff = unstaged(dir.path(), "b.txt");
    port.stage(dir.path(), &hunk_target("b.txt", &diff.hunks[0].id))
        .unwrap();
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-status"]),
        "D\ta.txt\nD\tb.txt\n"
    );

    port.unstage(dir.path(), &file_target("a.txt")).unwrap();
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-status"]),
        "D\tb.txt\n"
    );
    assert_no_lock(dir.path());
}

#[test]
fn a_stale_hunk_id_is_rejected_and_leaves_the_index_alone() {
    let dir = repo_with(&[("f.txt", "one\n")]);
    write(dir.path(), "f.txt", "two\n");
    let port = SubprocessGit::default();
    let before = index_bytes(dir.path());

    let unknown = HunkId("h_0000000000000000".to_owned());
    let error = port
        .stage(dir.path(), &hunk_target("f.txt", &unknown))
        .unwrap_err();
    assert!(error.0.starts_with("hunk not found or changed"), "{error}");
    assert_eq!(index_bytes(dir.path()), before);

    let id = unstaged(dir.path(), "f.txt").hunks[0].id.clone();
    write(dir.path(), "f.txt", "three\n");
    let error = port
        .stage(dir.path(), &hunk_target("f.txt", &id))
        .unwrap_err();
    assert!(error.0.starts_with("hunk not found or changed"), "{error}");
    assert_eq!(index_bytes(dir.path()), before);
    assert_no_lock(dir.path());
}

#[test]
fn unstaging_a_hunk_restores_the_head_side() {
    let original: String = numbered("line", 20);
    let edited = original.replace("line 10\n", "ten\n");
    let dir = repo_with(&[("f.txt", original.as_str())]);
    write(dir.path(), "f.txt", &edited);
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("f.txt")).unwrap();

    let diff = staged(dir.path(), "f.txt");
    assert_eq!(diff.hunks.len(), 1);
    port.unstage(dir.path(), &hunk_target("f.txt", &diff.hunks[0].id))
        .unwrap();

    assert_eq!(show_index(dir.path(), "f.txt"), original.as_bytes());
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "");
    assert_no_lock(dir.path());
}

/// Files and edits shared by the invariant test. Every edit is unstaged.
fn apply_scenario(dir: &Path) {
    let long: String = numbered("row", 30);
    write(
        dir,
        "f1.txt",
        &long.replace("row 2\n", "two\n").replace("row 29\n", "x\n"),
    );
    // The size changes too: a same-size rewrite within one timestamp tick is
    // invisible to git's stat check, and that would test git, not the adapter.
    write(dir, "g.txt", &"a\nb\nc\n".replace("b\n", "BB\n"));
    write(dir, "new.txt", "fresh\nlines\n");
}

fn scenario_repo() -> tempfile::TempDir {
    let long: String = numbered("row", 30);
    let dir = repo_with(&[
        ("f1.txt", long.as_str()),
        ("g.txt", "a\nb\nc\n"),
        ("h.txt", "gone\n"),
    ]);
    apply_scenario(dir.path());
    std::fs::remove_file(dir.path().join("h.txt")).unwrap();
    dir
}

#[test]
fn staging_every_hunk_one_by_one_matches_add_all() {
    let staged_repo = scenario_repo();
    let port = SubprocessGit::default();
    let paths = ["f1.txt", "g.txt", "h.txt", "new.txt"];

    for _ in 0..100 {
        let next = paths.iter().find_map(|path| {
            unstaged(staged_repo.path(), path)
                .hunks
                .first()
                .map(|hunk| ((*path).to_owned(), hunk.id.clone()))
        });
        let Some((path, id)) = next else { break };
        port.stage(staged_repo.path(), &hunk_target(&path, &id))
            .unwrap();
    }

    assert_eq!(git(staged_repo.path(), &["diff", "--name-only"]), "");
    assert_eq!(
        git(
            staged_repo.path(),
            &["ls-files", "--others", "--exclude-standard"]
        ),
        ""
    );
    let reference = scenario_repo();
    git(reference.path(), &["add", "-A"]);
    assert_eq!(
        git(staged_repo.path(), &["write-tree"]),
        git(reference.path(), &["write-tree"])
    );
    assert_no_lock(staged_repo.path());
}

#[test]
fn commits_on_an_unborn_branch() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    write(dir.path(), "a.txt", "hello\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    let id = port.commit(dir.path(), "first commit").unwrap();

    assert_eq!(id.len(), 40);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), id);
    assert_eq!(
        git(dir.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%B"]).trim_end(),
        "first commit"
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert_no_lock(dir.path());
}

#[test]
fn commits_on_top_of_head_with_a_multi_line_message() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    let parent = git(dir.path(), &["rev-parse", "HEAD"]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    let id = port.commit(dir.path(), "change a\n\nbody line").unwrap();

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), id);
    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD^"]).trim(),
        parent.trim()
    );
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%B"]).trim_end(),
        "change a\n\nbody line"
    );
    let commit = String::from_utf8(git_bytes(dir.path(), &["cat-file", "commit", "HEAD"])).unwrap();
    assert!(commit.ends_with("\n\nchange a\n\nbody line\n"), "{commit}");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert_no_lock(dir.path());
}

#[test]
fn a_message_with_a_trailing_newline_is_not_doubled() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    port.commit(dir.path(), "subject\n").unwrap();

    let commit = String::from_utf8(git_bytes(dir.path(), &["cat-file", "commit", "HEAD"])).unwrap();
    assert!(commit.ends_with("\n\nsubject\n"), "{commit}");
}

#[test]
fn refuses_to_commit_when_nothing_is_staged() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();

    let error = port.commit(dir.path(), "nothing").unwrap_err();

    assert_eq!(error.0, "nothing staged to commit");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_no_lock(dir.path());
}

#[test]
fn refuses_to_commit_an_empty_index_on_an_unborn_branch() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let error = SubprocessGit::default()
        .commit(dir.path(), "nothing")
        .unwrap_err();
    assert_eq!(error.0, "nothing staged to commit");
}

/// Writes an executable script from a child process. Writing it here and running it at once
/// races with other test threads' forks, which inherit the open file and make the exec fail
/// with "Text file busy" (ETXTBSY).
#[cfg(unix)]
fn write_script(path: &Path, text: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sh");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

#[cfg(unix)]
fn install_hook(dir: &Path, name: &str, script: &str) {
    let path = dir.join(".git/hooks").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_script(&path, script);
}

#[cfg(unix)]
#[test]
fn a_failing_pre_commit_hook_blocks_the_commit_and_its_report_is_the_error() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    install_hook(
        dir.path(),
        "pre-commit",
        "#!/bin/sh\necho 'lint failed: a.txt:1' >&2\nexit 1\n",
    );
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    let error = port.commit(dir.path(), "msg").unwrap_err();

    assert!(error.0.contains("lint failed: a.txt:1"), "{error}");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_no_lock(dir.path());
}

#[cfg(unix)]
#[test]
fn a_commit_msg_hook_can_rewrite_the_message() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    install_hook(
        dir.path(),
        "commit-msg",
        "#!/bin/sh\nprintf 'hooked: ' | cat - \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"\n",
    );
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    port.commit(dir.path(), "subject").unwrap();

    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%s"]).trim_end(),
        "hooked: subject"
    );
}

#[test]
fn refuses_an_empty_message() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    assert!(port.commit(dir.path(), "  \n").is_err());
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"]),
        "a.txt\n"
    );
}

#[test]
fn every_operation_refuses_while_index_lock_exists() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    write(dir.path(), "a.txt", "two\n");
    let port = SubprocessGit::default();
    let lock = dir.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let before = index_bytes(dir.path());

    assert!(port.stage(dir.path(), &file_target("a.txt")).is_err());
    assert!(port.unstage(dir.path(), &file_target("a.txt")).is_err());
    let error = port.commit(dir.path(), "msg").unwrap_err();
    assert!(error.0.contains("locked"), "{error}");

    assert_eq!(index_bytes(dir.path()), before);
    assert!(
        lock.exists(),
        "the lock belongs to another process and stays"
    );
    std::fs::remove_file(&lock).unwrap();
}

/// `count` lines of the form "<prefix> <n>\n".
fn numbered(prefix: &str, count: u32) -> String {
    (1..=count).fold(String::new(), |text, i| format!("{text}{prefix} {i}\n"))
}

fn logged_port() -> (SubprocessGit, CommandLog) {
    let log = CommandLog::default();
    let port = SubprocessGit {
        log: Some(log.clone()),
        ..SubprocessGit::default()
    };
    (port, log)
}

#[test]
fn writes_are_logged_and_reads_are_not() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    write(dir.path(), "a.txt", "two\n");
    let (port, log) = logged_port();

    port.status(dir.path()).unwrap();
    unstaged(dir.path(), "a.txt");
    assert!(log.entries().is_empty());

    port.stage(dir.path(), &file_target("a.txt")).unwrap();
    port.commit(dir.path(), "two").unwrap();

    let lines: Vec<String> = log.entries().iter().map(|r| r.args.join(" ")).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("--literal-pathspecs add"), "{lines:?}");
    assert_eq!(lines[1], "commit -F -");
    assert!(
        log.entries()
            .iter()
            .all(gm_core::command_log::CommandRecord::succeeded)
    );
}

#[cfg(unix)]
#[test]
fn a_failed_commit_is_logged_with_the_hooks_report() {
    let dir = repo_with(&[("a.txt", "one\n")]);
    install_hook(
        dir.path(),
        "pre-commit",
        "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n",
    );
    write(dir.path(), "a.txt", "two\n");
    let (port, log) = logged_port();
    port.stage(dir.path(), &file_target("a.txt")).unwrap();

    port.commit(dir.path(), "msg").unwrap_err();

    let entries = log.entries();
    let last = entries.last().unwrap();
    assert!(!last.succeeded());
    assert_eq!(last.code, Some(1));
    assert!(last.stderr.contains("lint failed"), "{}", last.stderr);
    assert!(!last.args.contains(&"--no-optional-locks".to_owned()));
}

#[test]
fn a_line_stage_logs_the_blob_write_and_the_index_update() {
    let dir = repo_with(&[("a.txt", &numbered("row", 10))]);
    write(
        dir.path(),
        "a.txt",
        &numbered("row", 10).replace("row 2\n", "two\n"),
    );
    let (port, log) = logged_port();
    let diff = unstaged(dir.path(), "a.txt");
    let target = StageTarget::Hunk {
        path: "a.txt".into(),
        hunk: diff.hunks[0].id.clone(),
    };

    port.stage(dir.path(), &target).unwrap();

    let verbs: Vec<String> = log.entries().iter().map(|r| r.args[0].clone()).collect();
    assert_eq!(verbs, ["hash-object", "update-index"]);
}
