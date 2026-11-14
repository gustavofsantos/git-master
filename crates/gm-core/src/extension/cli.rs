//! `gitmaster extension check | render` (docs/reference/cli.md): one-shot commands with no
//! resident process. `render` runs a slot for real; it is read-only, like every extension, and
//! reaches a program or the network only for a reviewed version or with `--allow-exec`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gm_schema::ext::Document;
use serde_json::json;

use super::host::{Reach, run_slot};
use super::http::{GitCredentials, UreqNetwork};
use super::{SLOTS, focus, load_file};
use crate::script::cli::CliResult;
use crate::script::port::{ScriptDiagnostic, ScriptEngine};

const USAGE: &str = "usage: gitmaster extension check <file> [--json]
       gitmaster extension render <file> --slot <slot> [--commit <rev>] [--repo <path>] [--json] [--allow-exec]
";

/// What `render` needs from outside the engine: whether a version is reviewed, and how the
/// companion draws a document as text.
pub struct Outside<'a> {
    pub is_reviewed: &'a dyn Fn(&str) -> bool,
    pub draw: &'a dyn Fn(&Document) -> String,
}

#[derive(Debug, Default)]
struct Options {
    file: String,
    json: bool,
    slot: Option<String>,
    commit: Option<String>,
    repo: Option<String>,
    allow_exec: bool,
}

fn usage(message: Option<&str>) -> CliResult {
    let mut err = String::new();
    if let Some(message) = message {
        let _ = writeln!(err, "gitmaster extension: {message}");
    }
    err.push_str(USAGE);
    CliResult {
        code: 2,
        out: String::new(),
        err,
    }
}

fn parse(render: bool, words: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let mut value = |name: &str| {
            words
                .next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match word.as_str() {
            "--json" => options.json = true,
            "--slot" if render => options.slot = Some(value("--slot")?),
            "--commit" if render => options.commit = Some(value("--commit")?),
            "--repo" if render => options.repo = Some(value("--repo")?),
            "--allow-exec" if render => options.allow_exec = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option `{flag}`")),
            file if options.file.is_empty() => file.clone_into(&mut options.file),
            extra => return Err(format!("unexpected `{extra}`")),
        }
    }
    if options.file.is_empty() {
        return Err("name the extension file".to_owned());
    }
    if render && options.slot.is_none() {
        return Err(format!("name the slot with --slot ({})", SLOTS.join(", ")));
    }
    Ok(options)
}

/// Runs `gitmaster extension <args…>`. Exit codes: 0 success, 1 the extension has a problem or
/// its run failed, 2 usage.
pub fn run(
    args: &[String],
    engine: &dyn ScriptEngine,
    cwd: &Path,
    outside: &Outside<'_>,
) -> CliResult {
    let Some((word, rest)) = args.split_first() else {
        return usage(None);
    };
    let render = match word.as_str() {
        "check" => false,
        "render" => true,
        other => return usage(Some(&format!("unknown subcommand `{other}`"))),
    };
    let options = match parse(render, rest) {
        Ok(options) => options,
        Err(message) => return usage(Some(&message)),
    };
    let path = cwd.join(&options.file);
    let loaded = load_file(engine, &path);
    if !render {
        return check_result(&options, loaded.map(|_| ()));
    }
    let extension = match loaded {
        Ok(extension) => extension,
        Err(problems) => return check_result(&options, Err(problems)),
    };
    let slot = options.slot.clone().unwrap_or_default();
    if !SLOTS.contains(&slot.as_str()) {
        return usage(Some(&format!(
            "unknown slot `{slot}`; slots: {}",
            SLOTS.join(", ")
        )));
    }
    if !extension.slots.contains(&slot) {
        return failure(&format!(
            "{} does not claim the {slot} slot",
            extension.file_name
        ));
    }
    let root = match repo_root(
        &options
            .repo
            .as_ref()
            .map_or_else(|| cwd.to_path_buf(), |r| cwd.join(r)),
    ) {
        Ok(root) => root,
        Err(message) => return failure(&message),
    };
    let input = match focus::commit(&root, options.commit.as_deref().unwrap_or("HEAD")) {
        Ok(input) => input,
        Err(message) => return failure(&message),
    };
    let reach = Reach {
        root,
        effects: options.allow_exec || (outside.is_reviewed)(&extension.hash),
        path: None,
        network: &UreqNetwork,
        credentials: &GitCredentials,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let run = run_slot(engine, &extension, &slot, &input, &reach);
    let mut out = String::new();
    let mut err = String::new();
    for line in &run.logs {
        let _ = writeln!(err, "log: {line}");
    }
    let code = match &run.result {
        Ok(document) if options.json => {
            out = serde_json::to_string_pretty(document).unwrap_or_default();
            out.push('\n');
            0
        }
        Ok(document) if document.is_empty() => {
            out.push_str("(nothing to show)\n");
            0
        }
        Ok(document) => {
            out = (outside.draw)(document);
            0
        }
        Err(message) => {
            let _ = writeln!(err, "{}: {slot}: {message}", extension.meta.name);
            1
        }
    };
    CliResult { code, out, err }
}

fn failure(message: &str) -> CliResult {
    CliResult {
        code: 1,
        out: String::new(),
        err: format!("gitmaster extension: {message}\n"),
    }
}

fn check_result(options: &Options, result: Result<(), Vec<ScriptDiagnostic>>) -> CliResult {
    let file = &options.file;
    let problems = result.err().unwrap_or_default();
    let out = if options.json {
        let problems: Vec<_> = problems
            .iter()
            .map(|p| json!({ "line": p.line, "message": p.message }))
            .collect();
        format!(
            "{}\n",
            json!({ "file": file, "ok": problems.is_empty(), "problems": problems })
        )
    } else if problems.is_empty() {
        format!("{file}: ok\n")
    } else {
        problems
            .iter()
            .map(|p| {
                if p.line == 0 {
                    format!("{file}: {}\n", p.message)
                } else {
                    format!("{file}:{}: {}\n", p.line, p.message)
                }
            })
            .collect()
    };
    CliResult {
        code: i32::from(!problems.is_empty()),
        out,
        err: String::new(),
    }
}

fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!("{} is not inside a Git repository", dir.display()));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}
