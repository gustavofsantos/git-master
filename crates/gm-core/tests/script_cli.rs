//! `gitmaster workflow check | dry-run | test` end to end, with the Luau engine: scripts and
//! fixtures are written into temporary directories and run through `cli::run`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gm_core::script::cli::{CliResult, run};
use gm_core::script::luau::LuauEngine;
use serde_json::Value;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn cli(words: &[&str], cwd: &Path) -> CliResult {
    run(&args(words), &LuauEngine::new(), cwd)
}

/// A module with the given `meta` table body and `execute` body.
fn module(meta: &str, body: &str) -> String {
    format!(
        "local M = {{}}\nM.meta = {{\n{meta}\n}}\nfunction M.execute(ctx, input)\n{body}\nend\nreturn M\n"
    )
}

const TICKET_META: &str = r#"  name = "branch-from-ticket",
  description = "Create a branch from a ticket key",
  trigger = { on = "manual" },
  inputs = { ticket = { type = "string", prompt = "Ticket key" } },"#;

/// The branch-from-ticket example, with `touch` as the program so a real run would leave a file.
fn ticket_script(touch: &Path) -> String {
    module(
        TICKET_META,
        &format!(
            "  local r = ctx.exec({{ \"touch\", \"{}\", input.inputs.ticket }}, {{ timeout = \"30s\" }})\n  if ctx.approve(\"Create branch \" .. r.text .. \"?\") then\n    ctx.branch.create({{ name = \"feat/\" .. input.inputs.ticket, checkout = true }})\n  end",
            touch.display()
        ),
    )
}

#[test]
fn check_accepts_a_good_script() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("w.lua"), &module(TICKET_META, ""));
    let result = cli(&["check", "w.lua"], dir.path());
    assert_eq!(result.code, 0, "{result:?}");
    assert_eq!(result.out, "w.lua: ok\n");
    assert!(result.err.is_empty());
}

#[test]
fn check_points_a_syntax_error_at_its_line() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("w.lua"),
        "local M = {}\nM.meta = {\nfunction M.execute(ctx, input)\nend\nreturn M\n",
    );
    let result = cli(&["check", "w.lua"], dir.path());
    assert_eq!(result.code, 1, "{result:?}");
    let line = result.out.lines().next().unwrap();
    let number: usize = line
        .strip_prefix("w.lua:")
        .and_then(|rest| rest.split(':').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no line in {line}"));
    assert!(number >= 2, "{line}");
}

#[test]
fn check_points_bad_meta_at_the_line_of_its_field() {
    let dir = tempfile::tempdir().unwrap();
    let meta = "  name = \"w\",\n  trigger = { on = \"nope\" },";
    write(&dir.path().join("w.lua"), &module(meta, ""));
    let result = cli(&["check", "w.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert_eq!(result.out.lines().count(), 1, "{}", result.out);
    assert!(result.out.starts_with("w.lua:4: "), "{}", result.out);
    assert!(result.out.contains("unknown trigger `nope`"));
}

#[test]
fn check_json_is_one_object_with_the_problems() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("good.lua"), &module(TICKET_META, ""));
    write(
        &dir.path().join("bad.lua"),
        &module("  name = \"w\", trigger = { on = \"nope\" },", ""),
    );

    let good: Value =
        serde_json::from_str(cli(&["check", "good.lua", "--json"], dir.path()).out.trim()).unwrap();
    assert_eq!(
        good,
        serde_json::json!({"file": "good.lua", "ok": true, "problems": []})
    );

    let result = cli(&["check", "bad.lua", "--json"], dir.path());
    assert_eq!(result.code, 1);
    assert_eq!(result.out.lines().count(), 1);
    let bad: Value = serde_json::from_str(result.out.trim()).unwrap();
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["problems"][0]["line"], 3);
    assert!(
        bad["problems"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown trigger")
    );
}

#[test]
fn check_reports_an_unreadable_file_as_line_zero() {
    let dir = tempfile::tempdir().unwrap();
    let result = cli(&["check", "missing.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert!(
        result.out.starts_with("missing.lua: cannot read the file"),
        "{}",
        result.out
    );
}

#[test]
fn usage_errors_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(cli(&[], dir.path()).code, 2);
    assert_eq!(cli(&["check"], dir.path()).code, 2);
    assert_eq!(cli(&["check", "w.lua", "--bogus"], dir.path()).code, 2);
    assert_eq!(
        cli(&["test", "a.lua", "b.lua", "c.lua"], dir.path()).code,
        2
    );
}

#[test]
fn dry_run_lists_the_calls_and_runs_no_program() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));

    let result = cli(
        &["dry-run", "ticket.lua", "--input", "ticket=ABC-1"],
        dir.path(),
    );
    assert_eq!(result.code, 0, "{result:?}");
    assert!(!touched.exists(), "a dry run must not run the program");
    let lines: Vec<&str> = result.out.lines().collect();
    assert_eq!(
        lines[0],
        "dry run of branch-from-ticket: nothing is applied and no program runs"
    );
    assert!(
        lines[1].starts_with("1. ctx.exec({\"argv\":[\"touch\","),
        "{}",
        lines[1]
    );
    assert!(
        lines[1].ends_with("read, simulated: a program never runs"),
        "{}",
        lines[1]
    );
    assert_eq!(
        lines[2],
        "2. ctx.approve({\"summary\":\"Create branch ?\"})  asks, simulated: assumed approved  (estimate)"
    );
    assert!(
        lines[3].starts_with("3. ctx.branch.create({\"checkout\":true,\"name\":\"feat/ABC-1\"})"),
        "{}",
        lines[3]
    );
    assert!(lines[3].ends_with("(estimate)"), "{}", lines[3]);
    assert_eq!(lines[4], "returned: null");
}

#[test]
fn dry_run_refuses_a_missing_or_an_unknown_input() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("ticket.lua"),
        &ticket_script(&dir.path().join("PWNED")),
    );

    let missing = cli(&["dry-run", "ticket.lua"], dir.path());
    assert_eq!(missing.code, 1);
    assert!(missing.out.is_empty());
    assert!(
        missing
            .err
            .contains("input \"ticket\" is required: pass --input ticket=..."),
        "{}",
        missing.err
    );

    let unknown = cli(
        &[
            "dry-run",
            "ticket.lua",
            "--input",
            "ticket=A",
            "--input",
            "nope=1",
        ],
        dir.path(),
    );
    assert_eq!(unknown.code, 1);
    assert!(
        unknown
            .err
            .contains("input `nope` is not declared. Declared inputs: ticket"),
        "{}",
        unknown.err
    );
}

#[test]
fn dry_run_reports_a_script_error_with_its_line() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("bad.lua"),
        "local M = {}\nM.meta = { name = \"w\", trigger = { on = \"nope\" } }\nfunction M.execute(ctx, input)\nend\nreturn M\n",
    );
    let result = cli(&["dry-run", "bad.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert!(result.out.starts_with("bad.lua:2: "), "{}", result.out);
}

#[test]
fn a_dry_run_of_a_failing_script_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("f.lua"),
        &module(TICKET_META, "  ctx.fail(\"boom\")"),
    );
    let result = cli(&["dry-run", "f.lua", "--input", "ticket=ABC-1"], dir.path());
    assert_eq!(result.code, 1, "{result:?}");
    assert!(
        result
            .out
            .lines()
            .last()
            .unwrap()
            .ends_with("ended: failed: boom"),
        "{}",
        result.out
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn dry_run_in_a_real_repository_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q"]);
    git(repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write(&repo.join("a.txt"), "one\n");
    git(repo, &["add", "a.txt"]);
    git(repo, &["commit", "-q", "-m", "init"]);
    write(&repo.join("a.txt"), "two\n");
    write(&repo.join("new.txt"), "untracked\n");

    let scripts = tempfile::tempdir().unwrap();
    write(
        &scripts.path().join("make.lua"),
        &module(
            "  name = \"make\",\n  description = \"d\",\n  trigger = { on = \"manual\" },",
            "  ctx.stage.all()\n  ctx.branch.create({ name = \"feat/x\", checkout = true })\n  return input.repo.branch",
        ),
    );
    let status_before = git(repo, &["status", "--porcelain"]);
    let branches_before = git(repo, &["branch", "--list"]);

    let script = scripts.path().join("make.lua");
    let result = cli(
        &[
            "dry-run",
            script.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
        ],
        scripts.path(),
    );
    assert_eq!(result.code, 0, "{result:?}");
    assert!(
        result.out.contains(
            "2. ctx.branch.create({\"checkout\":true,\"name\":\"feat/x\"})  applies, simulated"
        ),
        "{}",
        result.out
    );
    assert!(
        result.out.contains("returned: \"main\""),
        "the repository is read for real: {}",
        result.out
    );

    assert_eq!(git(repo, &["status", "--porcelain"]), status_before);
    assert_eq!(git(repo, &["branch", "--list"]), branches_before);
    assert!(!git(repo, &["branch", "--list", "feat/x"]).contains("feat/x"));
}

#[test]
fn dry_run_outside_a_repository_notes_it_and_runs_with_null_repo_fields() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("r.lua"),
        &module(
            "  name = \"r\",\n  description = \"d\",\n  trigger = { on = \"manual\" },",
            "  return input.repo.branch",
        ),
    );
    let plain = tempfile::tempdir().unwrap();
    let result = cli(
        &["dry-run", "r.lua", "--repo", plain.path().to_str().unwrap()],
        dir.path(),
    );
    assert_eq!(result.code, 0, "{result:?}");
    assert!(
        result.err.contains("is not a git repository"),
        "{}",
        result.err
    );
    assert!(result.out.contains("returned: null"), "{}", result.out);
}

#[test]
fn dry_run_refuses_a_repo_that_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("r.lua"),
        &module(
            "  name = \"r\",\n  description = \"d\",\n  trigger = { on = \"manual\" },",
            "",
        ),
    );
    let result = cli(&["dry-run", "r.lua", "--repo", "nope"], dir.path());
    assert_eq!(result.code, 1);
    assert!(
        result.err.contains("--repo nope: no such directory"),
        "{}",
        result.err
    );
}

/// The fixture of the branch-from-ticket example, written for the script at `touch`.
fn ticket_fixture(touch: &Path, ticket: &str) -> String {
    format!(
        r#"return {{
  input = {{ inputs = {{ ticket = "{ticket}" }} }},
  calls = {{
    {{ fn = "exec", args = {{ argv = {{ "touch", "{touch}", "{ticket}" }}, timeout = "30s" }},
      reply = {{ text = "feat/{ticket}", exit_code = 0 }} }},
    {{ fn = "approve", args = {{ summary = "Create branch feat/{ticket}?" }}, reply = true }},
    {{ fn = "branch.create", args = {{ name = "feat/{ticket}", checkout = true }} }},
  }},
}}
"#,
        touch = touch.display()
    )
}

#[test]
fn test_passes_when_the_calls_match_the_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));
    write(
        &dir.path().join(".tests/ticket.lua"),
        &ticket_fixture(&touched, "ABC-1"),
    );

    let result = cli(&["test", "ticket.lua"], dir.path());
    assert_eq!(result.code, 0, "{result:?}");
    assert_eq!(result.out, "ticket.lua: ok (3 calls)\n");
    assert!(!touched.exists(), "a test must not run the program");
}

#[test]
fn test_takes_a_fixture_named_on_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));
    write(
        &dir.path().join("fixtures/one.lua"),
        &ticket_fixture(&touched, "ABC-1"),
    );
    let result = cli(&["test", "ticket.lua", "fixtures/one.lua"], dir.path());
    assert_eq!(result.code, 0, "{result:?}");
}

#[test]
fn test_reports_a_wrong_call_order() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));
    let fixture = r#"return {
  input = { inputs = { ticket = "ABC-1" } },
  calls = {
    { fn = "exec", reply = { text = "x" } },
    { fn = "branch.create" },
    { fn = "approve", reply = true },
  },
}"#;
    write(&dir.path().join(".tests/ticket.lua"), fixture);
    let result = cli(&["test", "ticket.lua"], dir.path());
    assert_eq!(result.code, 1, "{result:?}");
    assert!(
        result
            .out
            .contains("call 2: expected ctx.branch.create, got ctx.approve"),
        "{}",
        result.out
    );
}

#[test]
fn test_reports_args_that_differ() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));
    let fixture =
        ticket_fixture(&touched, "ABC-1").replace("Create branch feat/ABC-1?", "Create it?");
    write(&dir.path().join(".tests/ticket.lua"), &fixture);
    let result = cli(&["test", "ticket.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert!(
        result
            .out
            .contains("call 2: args differ: expected {\"summary\":\"Create it?\"}"),
        "{}",
        result.out
    );
}

#[test]
fn test_reports_missing_calls() {
    let dir = tempfile::tempdir().unwrap();
    let touched = dir.path().join("PWNED");
    write(&dir.path().join("ticket.lua"), &ticket_script(&touched));
    let fixture = ticket_fixture(&touched, "ABC-1").replace(
        "  },\n}",
        "    { fn = \"commit.create\", args = { message = \"m\" } },\n  },\n}",
    );
    write(&dir.path().join(".tests/ticket.lua"), &fixture);
    let result = cli(&["test", "ticket.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert!(
        result
            .out
            .contains("missing calls: the script stopped after 3 of 4"),
        "{}",
        result.out
    );
}

#[test]
fn test_reports_a_wrong_return_value() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("r.lua"),
        &module(
            "  name = \"r\",\n  description = \"d\",\n  trigger = { on = \"manual\" },\n  inputs = { ticket = { type = \"string\", prompt = \"t\" } },",
            "  return input.inputs.ticket",
        ),
    );
    write(
        &dir.path().join(".tests/r.lua"),
        "return { input = { inputs = { ticket = \"ABC-1\" } }, returns = \"XYZ\" }",
    );
    let result = cli(&["test", "r.lua"], dir.path());
    assert_eq!(result.code, 1, "{result:?}");
    assert_eq!(result.out, "returned: expected \"XYZ\", got \"ABC-1\"\n");

    write(
        &dir.path().join(".tests/r.lua"),
        "return { input = { inputs = { ticket = \"ABC-1\" } }, returns = \"ABC-1\" }",
    );
    assert_eq!(cli(&["test", "r.lua"], dir.path()).code, 0);
}

#[test]
fn test_accepts_a_script_that_blocks_when_the_fixture_says_so() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("b.lua"),
        &module(
            "  name = \"b\",\n  description = \"d\",\n  trigger = { on = \"manual\" },",
            "  ctx.block(\"not today\")",
        ),
    );
    write(
        &dir.path().join(".tests/b.lua"),
        "return { calls = { { fn = \"block\", args = { reason = \"not today\" } } }, ends = \"blocked\", reason = \"not today\" }",
    );
    let result = cli(&["test", "b.lua"], dir.path());
    assert_eq!(result.code, 0, "{result:?}");
    assert_eq!(result.out, "b.lua: ok (1 calls)\n");

    write(
        &dir.path().join(".tests/b.lua"),
        "return { calls = { { fn = \"block\", args = { reason = \"not today\" } } } }",
    );
    let wrong = cli(&["test", "b.lua"], dir.path());
    assert_eq!(wrong.code, 1);
    assert_eq!(wrong.out, "ended: expected done, got blocked: not today\n");
}

#[test]
fn test_refuses_a_missing_fixture_with_the_path_it_looked_at() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("w.lua"), &module(TICKET_META, ""));
    let result = cli(&["test", "w.lua"], dir.path());
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert!(result.err.contains(".tests/w.lua"), "{}", result.err);
    assert!(result.err.contains("second argument"), "{}", result.err);
}

#[test]
fn test_refuses_a_fixture_that_is_not_a_table() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("w.lua"), &module(TICKET_META, ""));
    write(&dir.path().join(".tests/w.lua"), "return 42");
    let result = cli(&["test", "w.lua"], dir.path());
    assert_eq!(result.code, 2, "{result:?}");
    assert!(result.err.contains("must return a table"), "{}", result.err);
}

#[test]
fn test_refuses_a_script_that_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("w.lua"),
        "local M = {}\nM.meta = {\nreturn M\n",
    );
    write(&dir.path().join(".tests/w.lua"), "return {}");
    let result = cli(&["test", "w.lua"], dir.path());
    assert_eq!(result.code, 1);
    assert!(result.out.starts_with("w.lua:"), "{}", result.out);
}

#[test]
fn a_fixture_path_is_kept_as_a_path_in_the_message() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("w.lua"), &module(TICKET_META, ""));
    let result = cli(&["test", "w.lua", "missing/fixture.lua"], dir.path());
    assert_eq!(result.code, 2);
    let path: PathBuf = dir.path().join("missing/fixture.lua");
    assert!(
        result.err.contains(&path.display().to_string()),
        "{}",
        result.err
    );
}
