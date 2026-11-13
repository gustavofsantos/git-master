//! `gitmaster workflow check | dry-run | test` (docs/reference/cli.md, D102): one-shot commands
//! with no resident process and no state. They never apply an effect: `check` only compiles,
//! `dry-run` records and simulates, `test` runs against a fake `ctx`.

use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::{Map, Value, json};

use super::dry::{self, call_text, reason_of};
use super::input::{self, RepoInput};
use super::inspect::inspect;
use super::meta::Meta;
use super::port::{
    Budgets, CtxHost, HostEnd, HostReply, RunEnd, RunRequest, ScriptDiagnostic, ScriptEngine,
};
use super::surface::CtxSurface;
use crate::git::SubprocessGit;

/// What a subcommand prints and how it exits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliResult {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs `gitmaster workflow <args…>`; `args` are the words after `workflow`. `cwd` resolves
/// relative paths. Exit codes: 0 success, 1 the script has errors or a test differs, 2 usage.
pub fn run(args: &[String], engine: &dyn ScriptEngine, cwd: &Path) -> CliResult {
    let Some((word, rest)) = args.split_first() else {
        return usage(None);
    };
    let command = match word.as_str() {
        "check" => Subcommand::Check,
        "dry-run" => Subcommand::DryRun,
        "test" => Subcommand::Test,
        other => return usage(Some(&format!("unknown subcommand `{other}`"))),
    };
    let options = match Options::parse(command, rest) {
        Ok(options) => options,
        Err(message) => return usage(Some(&message)),
    };
    match command {
        Subcommand::Check => check(&options, engine, cwd),
        Subcommand::DryRun => dry_run(&options, engine, cwd),
        Subcommand::Test => run_test(&options, engine, cwd),
    }
}

const USAGE: &str = "usage: gitmaster workflow check <file> [--json]
       gitmaster workflow dry-run <file> [--input k=v]... [--repo <path>]
       gitmaster workflow test <file> [fixture]
";

/// Words of a fixture's call that end the run through the host.
const UNEXPECTED: &str = "unexpected call";

/// The `ends` values a fixture may name.
const ENDS: &[&str] = &["done", "blocked", "failed", "cancelled"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subcommand {
    Check,
    DryRun,
    Test,
}

/// The parsed words of one subcommand.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    file: String,
    fixture: Option<String>,
    json: bool,
    inputs: Vec<(String, String)>,
    repo: Option<String>,
}

impl Options {
    fn parse(command: Subcommand, args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut words = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--json" if command == Subcommand::Check => options.json = true,
                "--input" if command == Subcommand::DryRun => {
                    let value = iter.next().ok_or("--input needs k=v")?;
                    options.inputs.push(parse_input(value)?);
                }
                "--repo" if command == Subcommand::DryRun => {
                    let value = iter.next().ok_or("--repo needs a path")?;
                    options.repo = Some(value.clone());
                }
                flag if flag.starts_with("--") => return Err(format!("unknown flag `{flag}`")),
                _ => words.push(arg.clone()),
            }
        }
        let mut words = words.into_iter();
        options.file = words.next().ok_or("missing the script file")?;
        if command == Subcommand::Test {
            options.fixture = words.next();
        }
        if let Some(extra) = words.next() {
            return Err(format!("unexpected argument `{extra}`"));
        }
        Ok(options)
    }
}

/// `k=v`, split at the first `=`.
fn parse_input(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((name, value)) if !name.is_empty() => Ok((name.to_owned(), value.to_owned())),
        _ => Err(format!("--input expects k=v, got `{text}`")),
    }
}

fn usage(message: Option<&str>) -> CliResult {
    let err = match message {
        Some(message) => format!("gitmaster workflow: {message}\n{USAGE}"),
        None => USAGE.to_owned(),
    };
    CliResult {
        code: 2,
        out: String::new(),
        err,
    }
}

/// A failure with its message on the error stream.
fn failed(code: i32, message: &str) -> CliResult {
    CliResult {
        code,
        out: String::new(),
        err: format!("{}\n", message.trim_end()),
    }
}

/// The problems of a script, one per line on the output stream.
fn diagnostics(code: i32, file: &str, problems: &[ScriptDiagnostic]) -> CliResult {
    CliResult {
        code,
        out: diagnostic_lines(file, problems),
        err: String::new(),
    }
}

fn diagnostic_lines(file: &str, problems: &[ScriptDiagnostic]) -> String {
    let mut out = String::new();
    for problem in problems {
        let _ = if problem.line == 0 {
            writeln!(out, "{file}: {}", problem.message)
        } else {
            writeln!(out, "{file}:{}: {}", problem.line, problem.message)
        };
    }
    out
}

/// A script that was read and checked.
struct Loaded {
    source: String,
    meta: Meta,
}

/// Reads and checks the script. An unreadable file is one problem with line 0.
fn inspect_file(
    file: &str,
    engine: &dyn ScriptEngine,
    cwd: &Path,
) -> Result<Loaded, Vec<ScriptDiagnostic>> {
    let source = std::fs::read_to_string(cwd.join(file)).map_err(|error| {
        vec![ScriptDiagnostic {
            line: 0,
            message: format!("cannot read the file: {error}"),
        }]
    })?;
    let meta = inspect(engine, file, &source)?;
    Ok(Loaded { source, meta })
}

/// Runs `execute` under the default budgets, with `host` answering its calls.
fn run_with(
    engine: &dyn ScriptEngine,
    chunk: &str,
    source: &str,
    input: &Value,
    host: &mut dyn CtxHost,
) -> RunEnd {
    let surface = CtxSurface::from_catalog();
    let request = RunRequest {
        chunk,
        source,
        input,
        surface: &surface,
        budgets: Budgets::default(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    engine.run(&request, host)
}

fn check(options: &Options, engine: &dyn ScriptEngine, cwd: &Path) -> CliResult {
    let (ok, problems) = match inspect_file(&options.file, engine, cwd) {
        Ok(_) => (true, Vec::new()),
        Err(problems) => (false, problems),
    };
    let out = if options.json {
        check_json(&options.file, ok, &problems)
    } else if ok {
        format!("{}: ok\n", options.file)
    } else {
        diagnostic_lines(&options.file, &problems)
    };
    CliResult {
        code: i32::from(!ok),
        out,
        err: String::new(),
    }
}

fn check_json(file: &str, ok: bool, problems: &[ScriptDiagnostic]) -> String {
    let problems: Vec<Value> = problems
        .iter()
        .map(|p| json!({"line": p.line, "message": p.message}))
        .collect();
    format!(
        "{}\n",
        json!({"file": file, "ok": ok, "problems": problems})
    )
}

fn dry_run(options: &Options, engine: &dyn ScriptEngine, cwd: &Path) -> CliResult {
    let loaded = match inspect_file(&options.file, engine, cwd) {
        Ok(loaded) => loaded,
        Err(problems) => return diagnostics(1, &options.file, &problems),
    };
    let inputs = match input_values(&options.inputs, &loaded.meta) {
        Ok(inputs) => inputs,
        Err(message) => return failed(1, &message),
    };
    let (repo, note) = match run_repo(options.repo.as_deref(), cwd) {
        Ok(found) => found,
        Err(message) => return failed(1, &message),
    };
    let input = json!({"inputs": inputs, "trigger": {}, "repo": repo.value});
    let run = dry::walk(engine, &options.file, &loaded.source, &input);

    let mut out = format!(
        "dry run of {}: nothing is applied and no program runs\n",
        loaded.meta.name
    );
    for line in dry::lines(&run.calls) {
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out, "{}", dry::end_line(&run.end));
    CliResult {
        code: i32::from(!matches!(run.end, RunEnd::Returned(_) | RunEnd::Blocked(_))),
        out,
        err: note.map_or_default(|note| format!("{note}\n")),
    }
}

/// The values of the declared inputs, in name order. Every declared input must be given and
/// every given one must be declared.
fn input_values(given: &[(String, String)], meta: &Meta) -> Result<Map<String, Value>, String> {
    let declared: Vec<&str> = meta
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    if let Some((name, _)) = given
        .iter()
        .find(|(name, _)| !declared.contains(&name.as_str()))
    {
        return Err(format!(
            "input `{name}` is not declared. Declared inputs: {}",
            list(&declared)
        ));
    }
    let mut values = Map::new();
    for name in declared {
        let (_, value) = given
            .iter()
            .rev()
            .find(|(key, _)| key.as_str() == name)
            .ok_or_else(|| format!("input \"{name}\" is required: pass --input {name}=..."))?;
        values.insert(name.to_owned(), Value::String(value.clone()));
    }
    Ok(values)
}

fn list(names: &[&str]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// The repository a run reads: `--repo`, or the working directory. A note says when there is no
/// repository there.
fn run_repo(repo: Option<&str>, cwd: &Path) -> Result<(RepoInput, Option<String>), String> {
    let Some(given) = repo else {
        return Ok(repo_or_note(cwd));
    };
    let path = cwd.join(given);
    if !path.is_dir() {
        return Err(format!("--repo {given}: no such directory"));
    }
    Ok(repo_or_note(&path))
}

fn repo_or_note(dir: &Path) -> (RepoInput, Option<String>) {
    match git_root(dir) {
        Some(root) => (input::repo_input(&SubprocessGit::default(), &root), None),
        None => (
            RepoInput {
                value: input::no_repo(),
                diff_file: None,
            },
            Some(format!(
                "note: {} is not a git repository, so input.repo has null fields",
                dir.display()
            )),
        ),
    }
}

/// The top of the working tree that holds `dir`, asked of `git`.
fn git_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let root = text.trim_end_matches(['\n', '\r']);
    (!root.is_empty()).then(|| PathBuf::from(root))
}

fn run_test(options: &Options, engine: &dyn ScriptEngine, cwd: &Path) -> CliResult {
    let loaded = match inspect_file(&options.file, engine, cwd) {
        Ok(loaded) => loaded,
        Err(problems) => return diagnostics(1, &options.file, &problems),
    };
    let path = fixture_path(options, cwd);
    let name = path.display().to_string();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return failed(
                2,
                &format!(
                    "no fixture at {name}: create it there, or pass a fixture path as the second argument"
                ),
            );
        }
        Err(error) => return failed(2, &format!("cannot read the fixture {name}: {error}")),
    };
    let fixture = match read_fixture(engine, &name, &text) {
        Ok(fixture) => fixture,
        Err(message) => return failed(2, &message),
    };
    let total = fixture.calls.len();
    let mut host = FixtureHost::new(fixture.calls.clone());
    let end = run_with(
        engine,
        &options.file,
        &loaded.source,
        &fixture.input,
        &mut host,
    );

    let mut differences = std::mem::take(&mut host.differences);
    differences.extend(missing_calls(host.made, total));
    differences.extend(end_differences(&end, &fixture));
    if differences.is_empty() {
        return CliResult {
            code: 0,
            out: format!("{}: ok ({} calls)\n", options.file, host.made),
            err: String::new(),
        };
    }
    CliResult {
        code: 1,
        out: differences.iter().fold(String::new(), |mut out, d| {
            let _ = writeln!(out, "{d}");
            out
        }),
        err: String::new(),
    }
}

/// The fixture named on the command line, or `.tests/<stem>.lua` next to the script.
fn fixture_path(options: &Options, cwd: &Path) -> PathBuf {
    if let Some(path) = &options.fixture {
        return cwd.join(path);
    }
    let script = cwd.join(&options.file);
    let dir = script.parent().unwrap_or(cwd);
    let mut name = script
        .file_stem()
        .map_or_else(|| std::ffi::OsString::from("script"), ToOwned::to_owned);
    name.push(".lua");
    dir.join(".tests").join(name)
}

fn read_fixture(engine: &dyn ScriptEngine, name: &str, text: &str) -> Result<Fixture, String> {
    let data = engine
        .read_data(name, text)
        .map_err(|problems| diagnostic_lines(name, &problems))?;
    Fixture::parse(&data).map_err(|message| format!("{name}: {message}"))
}

/// A fixture: the input of the run, the calls the script must make, and how it must end.
#[derive(Debug)]
struct Fixture {
    input: Value,
    calls: Vec<Entry>,
    ends: String,
    reason: Option<String>,
    returns: Option<Value>,
    returns_nothing: bool,
}

/// One call the script must make, in order.
#[derive(Clone, Debug)]
struct Entry {
    func: String,
    args: Option<Value>,
    reply: Value,
    outcome: Option<String>,
    message: String,
}

const TOP_FIELDS: &[&str] = &[
    "input",
    "calls",
    "returns",
    "returns_nothing",
    "ends",
    "reason",
];
const ENTRY_FIELDS: &[&str] = &["fn", "args", "reply", "outcome", "message"];
const INPUT_FIELDS: &[&str] = &["inputs", "trigger", "repo"];

impl Fixture {
    fn parse(data: &Value) -> Result<Self, String> {
        let object = data.as_object().ok_or("the fixture must return a table")?;
        check_keys(object, TOP_FIELDS, "the fixture")?;
        let ends = match object.get("ends") {
            None => "done".to_owned(),
            Some(Value::String(ends)) if ENDS.contains(&ends.as_str()) => ends.clone(),
            Some(other) => {
                return Err(format!(
                    "ends must be one of: {} (got {other})",
                    ENDS.join(", ")
                ));
            }
        };
        Ok(Self {
            input: input_of(object.get("input"))?,
            calls: calls_of(object.get("calls"))?,
            ends,
            reason: text_field(object, "reason")?,
            returns: object.get("returns").cloned(),
            returns_nothing: flag(object, "returns_nothing")?,
        })
    }
}

fn check_keys(object: &Map<String, Value>, valid: &[&str], what: &str) -> Result<(), String> {
    match object.keys().find(|key| !valid.contains(&key.as_str())) {
        Some(key) => Err(format!(
            "unknown field `{key}` in {what}. Valid fields: {}",
            valid.join(", ")
        )),
        None => Ok(()),
    }
}

fn text_field(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{key} must be text")),
    }
}

fn flag(object: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match object.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

/// The `input` of the run; each of `inputs`, `trigger` and `repo` defaults to an empty table.
fn input_of(value: Option<&Value>) -> Result<Value, String> {
    let mut input = json!({"inputs": {}, "trigger": {}, "repo": {}});
    let Some(value) = value else {
        return Ok(input);
    };
    let object = value.as_object().ok_or("input must be a table")?;
    check_keys(object, INPUT_FIELDS, "input")?;
    for key in INPUT_FIELDS {
        if let Some(part) = object.get(*key) {
            if !part.is_object() {
                return Err(format!("input.{key} must be a table"));
            }
            input[*key] = part.clone();
        }
    }
    Ok(input)
}

fn calls_of(value: Option<&Value>) -> Result<Vec<Entry>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Object(map)) if map.is_empty() => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| Entry::parse(index + 1, item))
            .collect(),
        Some(_) => Err("calls must be a list of tables, such as { { fn = \"approve\" } }".into()),
    }
}

impl Entry {
    fn parse(number: usize, value: &Value) -> Result<Self, String> {
        let what = format!("call {number}");
        let object = value
            .as_object()
            .ok_or_else(|| format!("{what} must be a table"))?;
        check_keys(object, ENTRY_FIELDS, &what)?;
        let func = match object.get("fn") {
            Some(Value::String(name)) => name.strip_prefix("ctx.").unwrap_or(name).to_owned(),
            _ => {
                return Err(format!(
                    "{what}: fn must name a ctx function, such as \"approve\""
                ));
            }
        };
        let args = match object.get("args") {
            None => None,
            Some(args @ Value::Object(_)) => Some(args.clone()),
            Some(_) => return Err(format!("{what}: args must be a table")),
        };
        let wrap = |error: String| format!("{what}: {error}");
        Ok(Self {
            func,
            args,
            reply: object.get("reply").cloned().unwrap_or(Value::Null),
            outcome: text_field(object, "outcome").map_err(wrap)?,
            message: text_field(object, "message")
                .map_err(wrap)?
                .unwrap_or_default(),
        })
    }

    /// What the host answers to this call, given the entry.
    fn answer(&self, func: &str, args: &Value) -> HostReply {
        if func == "block" {
            return HostReply::End(HostEnd::Blocked(reason_of(args)));
        }
        if func == "fail" {
            return HostReply::End(HostEnd::Failed(reason_of(args)));
        }
        match self.outcome.as_deref() {
            Some(outcome) if outcome != "done" => HostReply::Outcome {
                outcome: outcome.to_owned(),
                value: self.reply.clone(),
                message: self.message.clone(),
            },
            _ => HostReply::Value(self.reply.clone()),
        }
    }
}

/// The host of a test: answers the n-th call from the n-th entry and records every difference.
struct FixtureHost {
    entries: Vec<Entry>,
    /// Calls the script made, including the ones with no entry.
    made: usize,
    differences: Vec<String>,
}

impl FixtureHost {
    fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            made: 0,
            differences: Vec::new(),
        }
    }
}

impl CtxHost for FixtureHost {
    fn call(&mut self, func: &str, args: Value) -> HostReply {
        self.made += 1;
        let number = self.made;
        let Some(entry) = self.entries.get(number - 1).cloned() else {
            self.differences.push(format!(
                "call {number}: unexpected {}, the fixture has {} calls",
                call_text(func, &args),
                self.entries.len()
            ));
            return HostReply::Error(UNEXPECTED.to_owned());
        };
        if entry.func != func {
            self.differences.push(format!(
                "call {number}: expected ctx.{}, got ctx.{func}",
                entry.func
            ));
            return HostReply::Error(UNEXPECTED.to_owned());
        }
        if let Some(expected) = &entry.args
            && *expected != args
        {
            self.differences.push(format!(
                "call {number}: args differ: expected {expected}, got {args}"
            ));
        }
        entry.answer(func, &args)
    }
}

fn missing_calls(made: usize, total: usize) -> Option<String> {
    (made < total).then(|| format!("missing calls: the script stopped after {made} of {total}"))
}

/// The differences in how the run ended and what it returned.
fn end_differences(end: &RunEnd, fixture: &Fixture) -> Vec<String> {
    let (got, reason) = match end {
        RunEnd::Returned(_) => ("done", None),
        RunEnd::Blocked(reason) => ("blocked", Some(reason.clone())),
        RunEnd::Failed(failure) => ("failed", Some(failure.to_string())),
        RunEnd::Cancelled => ("cancelled", None),
    };
    if got != fixture.ends {
        let mut line = format!("ended: expected {}, got {got}", fixture.ends);
        if let Some(reason) = &reason {
            let _ = write!(line, ": {reason}");
        }
        return vec![line];
    }
    let mut out = Vec::new();
    if let Some(expected) = &fixture.reason
        && reason.as_deref() != Some(expected.as_str())
    {
        out.push(format!(
            "ended: reason differs: expected {expected:?}, got {:?}",
            reason.unwrap_or_default()
        ));
    }
    if let RunEnd::Returned(value) = end {
        let expected = if fixture.returns_nothing {
            Some(Value::Null)
        } else {
            fixture.returns.clone()
        };
        if let Some(expected) = expected
            && *value != expected
        {
            let want = if fixture.returns_nothing {
                "nothing".to_owned()
            } else {
                expected.to_string()
            };
            out.push(format!("returned: expected {want}, got {value}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::script::luau::LuauEngine;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn no_subcommand_is_a_usage_error() {
        let result = run(&[], &LuauEngine::new(), Path::new("."));
        assert_eq!(result.code, 2);
        assert!(result.err.starts_with("usage: "), "{}", result.err);
        assert!(result.out.is_empty());
    }

    #[test]
    fn an_unknown_subcommand_is_a_usage_error() {
        let result = run(&words(&["nope"]), &LuauEngine::new(), Path::new("."));
        assert_eq!(result.code, 2);
        assert!(result.err.contains("unknown subcommand `nope`"));
    }

    #[test]
    fn check_takes_one_file_and_json() {
        let parsed = Options::parse(Subcommand::Check, &words(&["w.lua", "--json"])).unwrap();
        assert_eq!(
            parsed,
            Options {
                file: "w.lua".into(),
                json: true,
                ..Options::default()
            }
        );
    }

    #[test]
    fn dry_run_takes_inputs_and_a_repo() {
        let parsed = Options::parse(
            Subcommand::DryRun,
            &words(&[
                "w.lua",
                "--input",
                "ticket=ABC-1",
                "--input",
                "x=y",
                "--repo",
                "/r",
            ]),
        )
        .unwrap();
        assert_eq!(
            parsed.inputs,
            vec![
                ("ticket".to_owned(), "ABC-1".to_owned()),
                ("x".to_owned(), "y".to_owned())
            ]
        );
        assert_eq!(parsed.repo.as_deref(), Some("/r"));
    }

    #[test]
    fn an_input_value_is_split_at_its_first_equals() {
        assert_eq!(
            parse_input("a=b=c").unwrap(),
            ("a".to_owned(), "b=c".to_owned())
        );
        assert!(parse_input("ticket").is_err());
        assert!(parse_input("=x").is_err());
    }

    #[test]
    fn test_takes_a_fixture_as_its_second_word() {
        let parsed = Options::parse(Subcommand::Test, &words(&["w.lua", "f.lua"])).unwrap();
        assert_eq!(parsed.fixture.as_deref(), Some("f.lua"));
    }

    #[test]
    fn each_flag_belongs_to_one_subcommand() {
        assert!(Options::parse(Subcommand::DryRun, &words(&["w.lua", "--json"])).is_err());
        assert!(Options::parse(Subcommand::Check, &words(&["w.lua", "--repo", "x"])).is_err());
        assert!(Options::parse(Subcommand::Check, &words(&["w.lua", "--input", "a=b"])).is_err());
        assert!(Options::parse(Subcommand::Check, &words(&["w.lua", "--nope"])).is_err());
    }

    #[test]
    fn a_missing_file_or_an_extra_word_is_an_error() {
        assert!(Options::parse(Subcommand::Check, &words(&[])).is_err());
        assert!(Options::parse(Subcommand::Check, &words(&["a.lua", "b.lua"])).is_err());
        assert!(Options::parse(Subcommand::DryRun, &words(&["a.lua", "--input"])).is_err());
    }

    fn entry(func: &str, args: Option<Value>) -> Entry {
        Entry {
            func: func.into(),
            args,
            reply: Value::Null,
            outcome: None,
            message: String::new(),
        }
    }

    #[test]
    fn a_fixture_host_reports_a_call_out_of_order() {
        let mut host = FixtureHost::new(vec![entry("approve", None), entry("branch.create", None)]);
        host.call("approve", json!({}));
        host.call("branch.create", json!({}));
        assert!(host.differences.is_empty());

        let mut host = FixtureHost::new(vec![entry("approve", None), entry("branch.create", None)]);
        host.call("branch.create", json!({}));
        assert_eq!(
            host.differences,
            vec!["call 1: expected ctx.approve, got ctx.branch.create".to_owned()]
        );
    }

    #[test]
    fn a_fixture_host_reports_args_that_differ_and_still_answers() {
        let mut host = FixtureHost::new(vec![entry("approve", Some(json!({"summary": "a"})))]);
        let reply = host.call("approve", json!({"summary": "b"}));
        assert_eq!(reply, HostReply::Value(Value::Null));
        assert_eq!(
            host.differences,
            vec![
                r#"call 1: args differ: expected {"summary":"a"}, got {"summary":"b"}"#.to_owned()
            ]
        );
    }

    #[test]
    fn a_call_beyond_the_fixture_is_an_error_for_the_script() {
        let mut host = FixtureHost::new(Vec::new());
        assert_eq!(
            host.call("approve", json!({})),
            HostReply::Error(UNEXPECTED.into())
        );
        assert_eq!(host.differences.len(), 1);
    }

    #[test]
    fn a_fixture_entry_with_a_non_done_outcome_raises_it() {
        let mut item = entry("branch.create", None);
        item.outcome = Some("failed".into());
        item.message = "exists".into();
        assert_eq!(
            item.answer("branch.create", &json!({})),
            HostReply::Outcome {
                outcome: "failed".into(),
                value: Value::Null,
                message: "exists".into()
            }
        );
    }

    fn fixture(ends: &str) -> Fixture {
        Fixture {
            input: json!({}),
            calls: Vec::new(),
            ends: ends.to_owned(),
            reason: None,
            returns: None,
            returns_nothing: false,
        }
    }

    #[test]
    fn the_missing_calls_are_counted() {
        assert_eq!(
            missing_calls(2, 4).unwrap(),
            "missing calls: the script stopped after 2 of 4"
        );
        assert!(missing_calls(4, 4).is_none());
    }

    #[test]
    fn a_wrong_end_names_what_ended_the_run() {
        let end = RunEnd::Failed(super::super::port::Failure::Reason("boom".into()));
        assert_eq!(
            end_differences(&end, &fixture("done")),
            vec!["ended: expected done, got failed: boom".to_owned()]
        );
        assert!(end_differences(&RunEnd::Returned(Value::Null), &fixture("done")).is_empty());
    }

    #[test]
    fn a_wrong_return_value_is_reported() {
        let mut expect = fixture("done");
        expect.returns = Some(json!("XYZ"));
        assert_eq!(
            end_differences(&RunEnd::Returned(json!("ABC")), &expect),
            vec![r#"returned: expected "XYZ", got "ABC""#.to_owned()]
        );
        let mut nothing = fixture("done");
        nothing.returns_nothing = true;
        assert_eq!(
            end_differences(&RunEnd::Returned(json!("x")), &nothing),
            vec!["returned: expected nothing, got \"x\"".to_owned()]
        );
    }

    #[test]
    fn a_fixture_is_read_with_its_defaults() {
        let parsed =
            Fixture::parse(&json!({"calls": [{"fn": "ctx.approve", "args": {"summary": "s"}}]}))
                .unwrap();
        assert_eq!(parsed.ends, "done");
        assert_eq!(
            parsed.input,
            json!({"inputs": {}, "trigger": {}, "repo": {}})
        );
        assert_eq!(parsed.calls[0].func, "approve");
        assert_eq!(parsed.calls[0].reply, Value::Null);
    }

    #[test]
    fn a_fixture_with_a_bad_field_is_refused_clearly() {
        let unknown =
            Fixture::parse(&json!({"calls": [{"fn": "approve", "bogus": 1}]})).unwrap_err();
        assert!(unknown.contains("unknown field `bogus`"), "{unknown}");
        let ends = Fixture::parse(&json!({"ends": "nope"})).unwrap_err();
        assert!(ends.contains("ends must be one of"), "{ends}");
        let no_fn = Fixture::parse(&json!({"calls": [{"args": {}}]})).unwrap_err();
        assert!(no_fn.contains("call 1: fn must name"), "{no_fn}");
        assert!(Fixture::parse(&json!([1, 2])).is_err());
    }

    #[test]
    fn the_default_fixture_is_next_to_the_script() {
        let options = Options {
            file: "flows/w.lua".into(),
            ..Options::default()
        };
        assert_eq!(
            fixture_path(&options, Path::new("/x")),
            Path::new("/x/flows/.tests/w.lua")
        );
    }
}
