//! Extensions against a real repository and the real Luau sandbox (design/extensions.md): the
//! commit focus, the read-only `ctx`, the review gate, the budgets and cancellation. The network
//! and the credential helper are fakes; `exec` runs a real program from a temporary folder.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gm_core::extension::host::{Reach, SlotRun, run_slot};
use gm_core::extension::http::{Credentials, Failure, Network, Request, Response};
use gm_core::extension::{Extension, focus, load_file};
use gm_core::script::luau::LuauEngine;
use gm_schema::ext::{Block, Inline, Tone};
use serde_json::Value;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=A", "-c", "user.email=a@x"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    dir: PathBuf,
    bin: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let dir = tmp.path().join("extensions");
    let bin = tmp.path().join("bin");
    for d in [&repo, &dir, &bin] {
        fs::create_dir_all(d).unwrap();
    }
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "add keys",
            "-m",
            "Body.",
            "-m",
            "Refs: PAY-218",
        ],
    );
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://code.example.com/acme/ledger.git",
        ],
    );
    Fixture {
        _tmp: tmp,
        repo,
        dir,
        bin,
    }
}

fn program(fx: &Fixture, name: &str, script: &str) {
    let path = fx.bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn extension(fx: &Fixture, name: &str, source: &str) -> Extension {
    let path = fx.dir.join(format!("{name}.lua"));
    fs::write(&path, source).unwrap();
    load_file(&LuauEngine::new(), &path).unwrap_or_else(|p| panic!("{p:?}"))
}

#[derive(Default)]
struct Net {
    seen: Mutex<Vec<Request>>,
}

impl Network for Net {
    fn get(&self, request: &Request) -> Result<Response, Failure> {
        self.seen.lock().unwrap().push(request.clone());
        Ok(Response {
            status: 200,
            headers: vec![],
            body: br#"[{"number":482,"merged":true}]"#.to_vec(),
            truncated: false,
        })
    }
}

struct Token;
impl Credentials for Token {
    fn lookup(&self, _host: &str) -> Option<(String, String)> {
        Some(("me".into(), "s3cret".into()))
    }
}

fn run(fx: &Fixture, ext: &Extension, effects: bool, cancel: bool, net: &Net) -> SlotRun {
    let input = focus::commit(&fx.repo, "HEAD").unwrap();
    let path = format!(
        "{}:{}",
        fx.bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let reach = Reach {
        root: fx.repo.clone(),
        effects,
        path: Some(path),
        network: net,
        credentials: &Token,
        cancel: Arc::new(AtomicBool::new(cancel)),
    };
    run_slot(&LuauEngine::new(), ext, "commit", &input, &reach)
}

const FULL: &str = r##"
local M = {}
M.meta = { name = "full", api = 1, exec = { "fake-tool" },
           http = { "code.example.com" }, credentials = { "code.example.com" } }
M.slots = {}
function M.slots.commit(ctx, commit)
  local ticket
  for _, t in ipairs(commit.trailers) do
    if t.key == "Refs" then ticket = t.value end
  end
  local tool = ctx.exec({ "fake-tool", commit.short_hash })
  local r = ctx.http.get("https://code.example.com/api/" .. commit.hash,
                         { format = "json", credential = "code.example.com" })
  ctx.log("status " .. r.status .. " " .. r.text)
  return ui.section("Ticket " .. ticket, {
    ui.field("Tool", tool.text),
    ui.field("Exit", tostring(tool.exit_code)),
    ui.field("PR", ui.badge("#" .. r.json[1].number, "ok")),
    ui.field("Remote", commit.repo.remotes[1].url),
    ui.field("Branch", commit.repo.branch),
  })
end
return M
"##;

#[test]
fn a_slot_reads_the_commit_runs_a_program_and_reaches_a_listed_host() {
    let fx = fixture();
    program(&fx, "fake-tool", "echo \"tool saw $1\"; exit 3");
    let ext = extension(&fx, "full", FULL);
    assert_eq!(ext.slots, vec!["commit"]);
    let net = Net::default();
    let run = run(&fx, &ext, true, false, &net);
    let doc = run.result.unwrap_or_else(|e| panic!("{e}"));
    let Block::Section {
        title, children, ..
    } = &doc.blocks[0]
    else {
        panic!("a section")
    };
    assert_eq!(title, "Ticket PAY-218");
    let value = |n: usize| match &children[n] {
        Block::Field { value, .. } => value.clone(),
        other => panic!("{other:?}"),
    };
    let text = |s: &str| {
        vec![Inline::Span {
            text: s.to_owned(),
            tone: Tone::Default,
        }]
    };
    let short = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&fx.repo)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(value(0), text(&format!("tool saw {}", short.trim())));
    // A non-zero exit is an answer the script reads, not a failure.
    assert_eq!(value(1), text("3"));
    assert_eq!(
        value(2),
        vec![Inline::Badge {
            text: "#482".into(),
            tone: Tone::Ok
        }]
    );
    assert_eq!(value(3), text("https://code.example.com/acme/ledger.git"));
    assert_eq!(value(4), text("main"));

    let seen = net.seen.lock().unwrap();
    assert_eq!(
        seen[0].headers,
        vec![("Authorization".into(), "Bearer s3cret".into())]
    );
    assert_eq!(run.logs.len(), 1);
    assert!(!run.logs[0].contains("s3cret"));
    assert_eq!(
        run.calls
            .iter()
            .map(|c| (c.func.as_str(), c.outcome.as_str()))
            .collect::<Vec<_>>(),
        vec![("exec", "failed"), ("http.get", "200")]
    );
}

#[test]
fn an_unreviewed_version_reaches_nothing() {
    let fx = fixture();
    program(&fx, "fake-tool", "touch ran");
    let ext = extension(&fx, "full", FULL);
    let net = Net::default();
    let run = run(&fx, &ext, false, false, &net);
    assert!(
        run.result
            .unwrap_err()
            .contains("ctx.exec runs only when this version")
    );
    assert!(!fx.repo.join("ran").exists());
    assert!(net.seen.lock().unwrap().is_empty());
}

#[test]
fn a_program_not_listed_does_not_start() {
    let fx = fixture();
    let ext = extension(
        &fx,
        "sneaky",
        r#"local M = {}
M.meta = { name = "sneaky", api = 1, exec = { "fake-tool" } }
M.slots = { commit = function(ctx) return ui.text(ctx.exec({ "sh", "-c", "touch ran" }).text) end }
return M"#,
    );
    let run = run(&fx, &ext, true, false, &Net::default());
    assert!(
        run.result
            .unwrap_err()
            .contains("\"sh\" is not in meta.exec")
    );
    assert!(!fx.repo.join("ran").exists());
}

#[test]
fn nil_shows_nothing_and_a_bad_document_names_its_place() {
    let fx = fixture();
    let none = extension(
        &fx,
        "none",
        "local M = { meta = { name = 'none', api = 1 }, slots = {} }\nfunction M.slots.commit() return nil end\nreturn M",
    );
    assert!(
        run(&fx, &none, true, false, &Net::default())
            .result
            .unwrap()
            .is_empty()
    );
    let bad = extension(
        &fx,
        "bad",
        "local M = { meta = { name = 'bad', api = 1 }, slots = {} }\nfunction M.slots.commit() return ui.section('t', { ui.field('n', 4) }) end\nreturn M",
    );
    assert_eq!(
        run(&fx, &bad, true, false, &Net::default())
            .result
            .unwrap_err(),
        "[1].children[1].value: expected text or an inline node, found a number"
    );
}

#[test]
fn the_sandbox_and_budgets_hold() {
    let fx = fixture();
    let cases = [
        ("return ui.text(tostring(os))", Ok("nil")),
        ("ui.section = nil", Err("readonly")),
        ("while true do end", Err("budget")),
        (
            "for i = 1, 60 do ctx.log('x') end",
            Err("ctx call budget exceeded"),
        ),
        (
            "ctx.block('no')",
            Err("ctx.block is not available to extensions"),
        ),
        ("ctx.fail('gave up')", Err("gave up")),
    ];
    for (body, expected) in cases {
        let source = format!(
            "local M = {{ meta = {{ name = 'x', api = 1 }}, slots = {{}} }}\nfunction M.slots.commit(ctx) {body} end\nreturn M"
        );
        let ext = extension(&fx, "x", &source);
        let result = run(&fx, &ext, true, false, &Net::default()).result;
        match (expected, result) {
            (Ok(text), Ok(doc)) => assert!(
                serde_json::to_string(&doc).unwrap().contains(text),
                "{body}"
            ),
            (Err(needle), Err(message)) => assert!(message.contains(needle), "{body}: {message}"),
            (_, other) => panic!("{body}: {other:?}"),
        }
    }
}

#[test]
fn a_cancelled_run_stops_its_program() {
    let fx = fixture();
    program(&fx, "fake-tool", "sleep 5; touch ran");
    let ext = extension(&fx, "full", FULL);
    let input = focus::commit(&fx.repo, "HEAD").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        flag.store(true, Ordering::SeqCst);
    });
    let net = Net::default();
    let path = format!(
        "{}:{}",
        fx.bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let reach = Reach {
        root: fx.repo.clone(),
        effects: true,
        path: Some(path),
        network: &net,
        credentials: &Token,
        cancel,
    };
    let started = std::time::Instant::now();
    let run = run_slot(&LuauEngine::new(), &ext, "commit", &input, &reach);
    stopper.join().unwrap();
    assert!(run.cancelled, "{:?}", run.result);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!fx.repo.join("ran").exists());
    assert!(net.seen.lock().unwrap().is_empty());
}

#[test]
fn loading_names_every_problem() {
    let fx = fixture();
    let path = fx.dir.join("bad.lua");
    fs::write(
        &path,
        "local M = {}\nM.meta = { name = 'x', api = 1 }\nM.slots = { history = function() end }\nreturn M",
    )
    .unwrap();
    let problems = load_file(&LuauEngine::new(), &path).unwrap_err();
    assert_eq!(
        problems[0].message,
        "slots.history is not a slot; valid slots: commit"
    );
    let value: Value = focus::commit(&fx.repo, "HEAD").unwrap();
    assert_eq!(value["trailers"][0]["key"], "Refs");
    assert_eq!(value["subject"], "add keys");
    assert_eq!(value["body"], "Body.\n\nRefs: PAY-218");
}
