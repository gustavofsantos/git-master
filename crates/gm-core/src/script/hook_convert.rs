//! Converts the `[gitmaster "prepare-commit.<name>"]` sections of the user's Git config into a
//! workflow (`WF3e`, D160). The hook path that read these sections is removed in this slice; what
//! stays here is what the conversion needs: reading the sections and checking them as the hook
//! path did, and writing one `transform.commit_message` script that runs the same commands with
//! the same JSON contract. The engine never edits Git config.
//!
//! The chain is one script: its steps are the valid, enabled user sections in config order, and
//! each step is the command the hook ran (`sh -c <command>`), so a failing step ends the chain as
//! a failing hook did. A repository section is never converted: the workflow would then apply to
//! every repository, and a repository's own command must not reach them (D17).

use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::GitPort;
use crate::port::{ConfigScope, ConfigValue};
use crate::workflow_session::write_atomically;

/// What a section runs with when it sets no `timeout`, as the hook path did.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const PREFIX: &str = "gitmaster.prepare-commit.";
/// How many copies of one conversion a folder may hold before `write` gives up.
const MAX_COPIES: u32 = 1000;

/// One section, read and checked. `problem` says why it cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// The name after `gitmaster.prepare-commit.`; it may contain dots.
    pub name: String,
    pub command: String,
    /// The glob after `branch:` in `match`, when the section has one.
    pub branch: Option<String>,
    pub timeout: Duration,
    pub enabled: bool,
    pub problem: Option<String>,
}

/// One step of the chain: what the hook ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub command: String,
    pub branch: Option<String>,
    pub timeout: Duration,
}

/// The result of a conversion: what was written, what was left out and why, and every section
/// the person can now remove. Each `skipped` and `sections` entry reads `<scope> <section>`, with
/// the section as `gitmaster.prepare-commit.<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Converted {
    pub written: Vec<String>,
    pub skipped: Vec<(String, String)>,
    pub sections: Vec<String>,
}

/// The sections of one scope, in config order. Checks each one the way the hook path did.
#[must_use]
pub fn sections(entries: &[ConfigValue]) -> Vec<Section> {
    // A section's name and its `(variable, value)` pairs, in config order.
    type Group<'a> = (String, Vec<(&'a str, Option<&'a str>)>);
    let mut grouped: Vec<Group> = Vec::new();
    for entry in entries {
        let Some(rest) = entry.key.strip_prefix(PREFIX) else {
            continue;
        };
        // The variable is the last dotted part; the name may itself contain dots.
        let Some((name, var)) = rest.rsplit_once('.') else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let value = entry.value.as_deref();
        match grouped.iter_mut().find(|(n, _)| n == name) {
            Some((_, vars)) => vars.push((var, value)),
            None => grouped.push((name.to_owned(), vec![(var, value)])),
        }
    }
    grouped
        .into_iter()
        .map(|(name, vars)| check(&name, &vars))
        .collect()
}

fn check(name: &str, vars: &[(&str, Option<&str>)]) -> Section {
    let mut section = Section {
        name: name.to_owned(),
        command: String::new(),
        branch: None,
        timeout: DEFAULT_TIMEOUT,
        enabled: true,
        problem: None,
    };
    let mut seen: Vec<&str> = Vec::new();
    let mut problem: Option<String> = None;
    for &(var, value) in vars {
        if seen.contains(&var) {
            problem.get_or_insert(format!("{var} is set more than once"));
            continue;
        }
        seen.push(var);
        let outcome = match var {
            "command" => match value {
                Some(command) if !command.trim().is_empty() => {
                    command.clone_into(&mut section.command);
                    Ok(())
                }
                _ => Err("command is empty".to_owned()),
            },
            "match" => match value {
                Some(rule) if rule.strip_prefix("branch:").is_some_and(|g| !g.is_empty()) => {
                    section.branch = rule.strip_prefix("branch:").map(str::to_owned);
                    Ok(())
                }
                _ => Err("match must be branch:<glob>".to_owned()),
            },
            "timeout" => parse_duration(value).map(|timeout| section.timeout = timeout),
            "enabled" => parse_bool(value).map(|enabled| section.enabled = enabled),
            other => Err(format!("unknown key {other}")),
        };
        if let Err(reason) = outcome {
            problem.get_or_insert(reason);
        }
    }
    if !seen.contains(&"command") {
        problem.get_or_insert("command is missing".to_owned());
    }
    section.problem = problem;
    section
}

/// `30s`, `500ms` or `2m`. Plain numbers are refused so the unit is never guessed.
fn parse_duration(value: Option<&str>) -> Result<Duration, String> {
    let text = value.unwrap_or("").trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let amount: u64 = digits
        .parse()
        .map_err(|_| "timeout must look like 30s, 500ms or 2m".to_owned())?;
    if amount == 0 {
        return Err("timeout must be above zero".to_owned());
    }
    match unit {
        "ms" => Ok(Duration::from_millis(amount)),
        "s" => Ok(Duration::from_secs(amount)),
        "m" => Ok(Duration::from_secs(amount.saturating_mul(60))),
        _ => Err("timeout must look like 30s, 500ms or 2m".to_owned()),
    }
}

/// The spellings `git config --type=bool` accepts. A bare key is `true`.
fn parse_bool(value: Option<&str>) -> Result<bool, String> {
    let Some(text) = value else {
        return Ok(true);
    };
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" | "" => Ok(false),
        _ => Err("enabled must be true or false".to_owned()),
    }
}

/// The entry the answer uses for a section: `<scope> gitmaster.prepare-commit.<name>`.
#[must_use]
pub fn entry(scope: ConfigScope, name: &str) -> String {
    format!("{} gitmaster.prepare-commit.{name}", scope.as_str())
}

/// What a conversion would write, and what it leaves out. `user` and `repo` are the sections of
/// the two scopes, in config order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub skipped: Vec<(String, String)>,
    pub sections: Vec<String>,
}

#[must_use]
pub fn plan(user: &[Section], repo: &[Section]) -> Plan {
    let mut out = Plan::default();
    for section in user {
        out.sections.push(entry(ConfigScope::Global, &section.name));
    }
    for section in repo {
        out.sections.push(entry(ConfigScope::Local, &section.name));
    }
    for section in user {
        let key = entry(ConfigScope::Global, &section.name);
        if let Some(reason) = &section.problem {
            out.skipped.push((key, reason.clone()));
        } else if section.enabled {
            out.steps.push(Step {
                name: section.name.clone(),
                command: section.command.clone(),
                branch: section.branch.clone(),
                timeout: section.timeout,
            });
        }
    }
    for section in repo {
        let key = entry(ConfigScope::Local, &section.name);
        if let Some(reason) = &section.problem {
            out.skipped.push((key, reason.clone()));
        } else if section.enabled {
            out.skipped.push((
                key,
                "a repository section is not converted: a workflow applies to every repository (D17)"
                    .to_owned(),
            ));
        }
    }
    out
}

/// The sections of both scopes of the open repository, as `entry` names them, for the listing.
pub fn section_entries(git: &dyn GitPort, root: &Path) -> Result<Vec<String>, String> {
    Ok(read_plan(git, root)?.sections)
}

fn read_plan(git: &dyn GitPort, root: &Path) -> Result<Plan, String> {
    let user = sections(
        &git.prepare_commit_config(root, ConfigScope::Global)
            .map_err(|e| e.0)?,
    );
    let repo = sections(
        &git.prepare_commit_config(root, ConfigScope::Local)
            .map_err(|e| e.0)?,
    );
    Ok(plan(&user, &repo))
}

/// Converts the user's sections into one workflow in `dir`. Nothing is written when no section
/// converts. The engine never edits Git config.
pub fn convert(git: &dyn GitPort, root: &Path, dir: &Path) -> Result<Converted, String> {
    let planned = read_plan(git, root)?;
    let mut written = Vec::new();
    if !planned.steps.is_empty() {
        written.push(write(dir, &planned.steps)?.display().to_string());
    }
    Ok(Converted {
        written,
        skipped: planned.skipped,
        sections: planned.sections,
    })
}

/// The file name stem: `prepare-commit-<name>` for one step, `prepare-commit-chain` for more.
#[must_use]
pub fn file_stem(steps: &[Step]) -> String {
    match steps {
        [only] => format!("prepare-commit-{}", file_part(&only.name)),
        _ => "prepare-commit-chain".to_owned(),
    }
}

/// `name` with each character that a file name should not hold replaced by `-`.
fn file_part(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Writes the script for `steps` into `dir` under the first name not taken: `<stem>.lua`, then
/// `<stem>-2.lua`, `<stem>-3.lua`... It never overwrites a file. Returns the path written.
pub fn write(dir: &Path, steps: &[Step]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let stem = file_stem(steps);
    for copy in 1..=MAX_COPIES {
        let name = if copy == 1 {
            stem.clone()
        } else {
            format!("{stem}-{copy}")
        };
        let path = dir.join(format!("{name}.lua"));
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                write_atomically(&path, &script_source(&name, steps))?;
                return Ok(path);
            }
            Ok(_) => {}
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        }
    }
    Err(format!(
        "there are already {MAX_COPIES} conversions named `{stem}` in {}",
        dir.display()
    ))
}

/// The Lua source of the workflow `name` with `steps`. The steps are literal data in a table, so
/// the script reads as what it runs.
#[must_use]
pub fn script_source(name: &str, steps: &[Step]) -> String {
    let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
    let mut out = String::new();
    out.push_str(
        "-- Converted from the prepare-commit sections of your Git config (GitMaster WF3e, D160).\n\
         -- Each step runs the command its section ran, with the JSON the hook read on stdin and\n\
         -- the answer the hook wrote on stdout, and passes its message to the next step.\n\
         local M = {}\n\n",
    );
    out.push_str("M.meta = {\n");
    let _ = writeln!(out, "  name = {},", lua_string(name));
    let _ = writeln!(
        out,
        "  description = {},",
        lua_string(&format!(
            "Suggest a commit message: converted from {}",
            names.join(", ")
        ))
    );
    out.push_str("  trigger = { on = \"transform.commit_message\" },\n}\n\n");
    out.push_str("local STEPS = {\n");
    for step in steps {
        let branch = match &step.branch {
            Some(glob) => lua_string(&branch_pattern(glob)),
            None => "nil".to_owned(),
        };
        let _ = writeln!(
            out,
            "  {{ name = {}, argv = {{ \"sh\", \"-c\", {} }}, timeout = {}, branch = {} }},",
            lua_string(&format!("gitmaster.prepare-commit.{}", step.name)),
            lua_string(&step.command),
            lua_string(&timeout_text(step.timeout)),
            branch,
        );
    }
    out.push_str("}\n");
    out.push_str(BODY);
    out
}

/// The part of the script that does not depend on the steps.
const BODY: &str = r#"
local ESCAPES = { ['"'] = '\\"', ['\\'] = '\\\\', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t' }

-- A JSON string holding `text`.
local function json(text)
  return '"' .. string.gsub(text, '[%c"\\]', function(c)
    return ESCAPES[c] or string.format("\\u%04x", string.byte(c))
  end) .. '"'
end

-- A JSON string, or null for a value that is missing.
local function optional(text)
  if text == nil then
    return "null"
  end
  return json(text)
end

-- The staged diff as the hook received it: the file's path when the diff was cut, else the text.
local function staged(input)
  local path = input.repo.staged_diff_path
  if path ~= nil then
    return ',"staged_diff_path":' .. json(path)
  end
  local diff = input.repo.staged_diff
  if diff ~= nil then
    return ',"staged_diff":' .. json(diff)
  end
  return ""
end

function M.execute(ctx, input)
  local message = input.trigger.current
  for index, step in ipairs(STEPS) do
    local branch = input.repo.branch
    local applies = step.branch == nil or (branch ~= nil and string.find(branch, step.branch) ~= nil)
    if applies then
      local id = "prep-" .. string.format("%d", math.floor(ctx.random() * 1000000000)) .. "-" .. index
      local repo = '{"root":' .. json(input.repo.root) .. ',"branch":' .. optional(input.repo.branch)
        .. ',"head":' .. optional(input.repo.head) .. '}'
      local payload = '{"schema_version":1,"event":"prepare-commit","invocation_id":' .. json(id)
        .. ',"repo":' .. repo .. ',"input":{"message":' .. json(message) .. staged(input) .. '}}'
      -- A failing command raises here, which ends the run as a failed hook did.
      local r = ctx.exec(step.argv, { stdin = payload, format = "json", timeout = step.timeout })
      local answer = r.json
      if answer.schema_version ~= nil and answer.schema_version ~= 1 then
        ctx.fail(step.name .. " printed invalid output")
      end
      if answer.invocation_id ~= id then
        ctx.fail(step.name .. " answered another invocation")
      end
      local text = answer.output ~= nil and answer.output.message or nil
      if type(text) ~= "string" then
        ctx.fail(step.name .. " printed invalid output")
      end
      message = text
    end
  end
  return message
end

return M
"#;

/// The Lua string literal for `text`: quotes and backslashes escaped, control characters as
/// decimal escapes.
#[must_use]
pub fn lua_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\{:03}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A Lua pattern that matches the branch names `glob` matches: `*` is any run of characters
/// (including `/`), `?` one character. Anchored at both ends.
#[must_use]
pub fn branch_pattern(glob: &str) -> String {
    let mut out = String::from("^");
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c if "^$()%.[]*+-?".contains(c) => {
                out.push('%');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

/// The timeout as `exec` reads it (`<n>s`), rounded up to whole seconds and at least one.
fn timeout_text(timeout: Duration) -> String {
    let millis = timeout.as_millis();
    let seconds = millis.div_ceil(1000).max(1);
    format!("{seconds}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: &str, value: Option<&str>) -> ConfigValue {
        ConfigValue {
            key: format!("{PREFIX}{key}"),
            value: value.map(str::to_owned),
        }
    }

    fn one(entries: &[ConfigValue]) -> Section {
        let mut all = sections(entries);
        assert_eq!(all.len(), 1);
        all.remove(0)
    }

    #[test]
    fn a_section_is_read_with_its_defaults_and_its_keys() {
        let section = one(&[kv("llm.command", Some("suggest --fast"))]);
        assert_eq!(section.name, "llm");
        assert_eq!(section.command, "suggest --fast");
        assert_eq!(section.timeout, Duration::from_secs(30));
        assert!(section.enabled);
        assert_eq!(section.branch, None);
        assert_eq!(section.problem, None);

        let section = one(&[
            kv("team.v2.command", Some("x")),
            kv("team.v2.match", Some("branch:feat/*")),
            kv("team.v2.timeout", Some("500ms")),
            kv("team.v2.enabled", None),
        ]);
        assert_eq!(section.name, "team.v2");
        assert_eq!(section.branch.as_deref(), Some("feat/*"));
        assert_eq!(section.timeout, Duration::from_millis(500));
        assert!(section.enabled, "a bare enabled key is true");
    }

    #[test]
    fn problems_are_reported_per_section() {
        let cases: [(&[ConfigValue], &str); 7] = [
            (&[kv("a.timeout", Some("1s"))], "command is missing"),
            (&[kv("a.command", Some("  "))], "command is empty"),
            (
                &[kv("a.command", Some("x")), kv("a.timeout", Some("30"))],
                "timeout must look like 30s, 500ms or 2m",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.match", Some("main"))],
                "match must be branch:<glob>",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.enabled", Some("maybe"))],
                "enabled must be true or false",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.colour", Some("red"))],
                "unknown key colour",
            ),
            (
                &[kv("a.command", Some("x")), kv("a.command", Some("y"))],
                "command is set more than once",
            ),
        ];
        for (entries, reason) in cases {
            assert_eq!(one(entries).problem.as_deref(), Some(reason));
        }
    }

    #[test]
    fn other_keys_and_bare_prefixes_are_not_sections() {
        let entries = [
            ConfigValue {
                key: "gitmaster.prepare-commit".into(),
                value: Some("x".into()),
            },
            ConfigValue {
                key: "gitmaster.worktree.root".into(),
                value: Some("x".into()),
            },
            ConfigValue {
                key: "user.name".into(),
                value: Some("x".into()),
            },
        ];
        assert!(sections(&entries).is_empty());
    }

    fn section(name: &str, command: &str) -> Section {
        Section {
            name: name.into(),
            command: command.into(),
            branch: None,
            timeout: Duration::from_secs(30),
            enabled: true,
            problem: None,
        }
    }

    #[test]
    fn the_plan_keeps_valid_enabled_user_sections_in_config_order() {
        let mut off = section("off", "x");
        off.enabled = false;
        let mut broken = section("broken", "");
        broken.problem = Some("command is empty".into());
        let user = [section("second", "b"), off, broken, section("first", "a")];
        let plan = plan(&user, &[]);
        let names: Vec<&str> = plan.steps.iter().map(|step| step.name.as_str()).collect();
        assert_eq!(names, ["second", "first"]);
        assert_eq!(
            plan.skipped,
            [(
                "global gitmaster.prepare-commit.broken".to_owned(),
                "command is empty".to_owned()
            )]
        );
        assert_eq!(
            plan.sections,
            [
                "global gitmaster.prepare-commit.second",
                "global gitmaster.prepare-commit.off",
                "global gitmaster.prepare-commit.broken",
                "global gitmaster.prepare-commit.first",
            ]
        );
    }

    #[test]
    fn a_repository_section_is_listed_and_never_converted() {
        let repo = [section("team", "t")];
        let plan = plan(&[], &repo);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.sections, ["local gitmaster.prepare-commit.team"]);
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].1.contains("D17"));
    }

    #[test]
    fn a_file_stem_names_one_step_or_the_chain() {
        let step = |name: &str| Step {
            name: name.into(),
            command: "x".into(),
            branch: None,
            timeout: Duration::from_secs(1),
        };
        assert_eq!(file_stem(&[step("llm")]), "prepare-commit-llm");
        assert_eq!(file_stem(&[step("a b/c")]), "prepare-commit-a-b-c");
        assert_eq!(file_stem(&[step("a"), step("b")]), "prepare-commit-chain");
    }

    #[test]
    fn a_glob_becomes_an_anchored_lua_pattern() {
        assert_eq!(branch_pattern("feat/*"), "^feat/.*$");
        assert_eq!(branch_pattern("main"), "^main$");
        assert_eq!(branch_pattern("a?c.x"), "^a.c%.x$");
        assert_eq!(branch_pattern("v[1]+"), "^v%[1%]%+$");
    }

    #[test]
    fn a_lua_string_escapes_what_needs_it() {
        assert_eq!(lua_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(lua_string("x\ny\tz"), "\"x\\ny\\tz\"");
        assert_eq!(lua_string("\u{1}"), "\"\\001\"");
    }

    #[test]
    fn a_timeout_rounds_up_to_whole_seconds() {
        assert_eq!(timeout_text(Duration::from_millis(500)), "1s");
        assert_eq!(timeout_text(Duration::from_millis(1500)), "2s");
        assert_eq!(timeout_text(Duration::from_secs(120)), "120s");
        assert_eq!(timeout_text(Duration::ZERO), "1s");
    }

    #[test]
    fn the_script_carries_the_trigger_and_each_command_as_argv() {
        let steps = [
            Step {
                name: "llm".into(),
                command: "suggest \"fast\"".into(),
                branch: Some("feat/*".into()),
                timeout: Duration::from_secs(30),
            },
            Step {
                name: "lint".into(),
                command: "lint".into(),
                branch: None,
                timeout: Duration::from_millis(250),
            },
        ];
        let source = script_source("prepare-commit-chain", &steps);
        assert!(source.contains("name = \"prepare-commit-chain\""));
        assert!(source.contains("trigger = { on = \"transform.commit_message\" }"));
        assert!(source.contains(
            "{ name = \"gitmaster.prepare-commit.llm\", argv = { \"sh\", \"-c\", \"suggest \\\"fast\\\"\" }, timeout = \"30s\", branch = \"^feat/.*$\" },"
        ));
        assert!(source.contains(
            "{ name = \"gitmaster.prepare-commit.lint\", argv = { \"sh\", \"-c\", \"lint\" }, timeout = \"1s\", branch = nil },"
        ));
        assert!(source.contains(
            "ctx.exec(step.argv, { stdin = payload, format = \"json\", timeout = step.timeout })"
        ));
    }

    #[test]
    fn write_picks_a_free_name_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let steps = [Step {
            name: "llm".into(),
            command: "x".into(),
            branch: None,
            timeout: Duration::from_secs(30),
        }];
        let first = write(dir.path(), &steps).unwrap();
        let second = write(dir.path(), &steps).unwrap();
        let third = write(dir.path(), &steps).unwrap();
        assert_eq!(first.file_name().unwrap(), "prepare-commit-llm.lua");
        assert_eq!(second.file_name().unwrap(), "prepare-commit-llm-2.lua");
        assert_eq!(third.file_name().unwrap(), "prepare-commit-llm-3.lua");
        assert!(
            std::fs::read_to_string(&second)
                .unwrap()
                .contains("name = \"prepare-commit-llm-2\"")
        );
    }
}
