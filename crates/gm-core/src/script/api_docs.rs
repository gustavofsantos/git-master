//! The reference files the app writes next to the user's workflows (design/workflow-scripts.md,
//! D102): `.api/API.md`, the type file for editors, and a short `SKILL.md` for agents. They are
//! generated from the `ctx` surface (which comes from the action catalog), so they cannot drift
//! from the code. A directory whose name starts with a dot is not read as a workflow source.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::meta::{self, Concurrency};
use super::port::Budgets;
use super::surface::{ArgShape, CtxFn, CtxSurface, HELPERS};
use crate::script::catalog::{self, FieldKind, FieldSpec, Level};

/// The type file's name. Luau's definition-file format, for editors with a Luau language server.
pub const TYPES_FILE: &str = "ctx.d.luau";

const API_DIR: &str = ".api";
const API_FILE: &str = "API.md";
const SKILL_FILE: &str = "SKILL.md";
const VERSION_FILE: &str = "VERSION";

/// The files of `.api/`, as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiFiles {
    pub api_md: String,
    pub types: String,
    pub skill_md: String,
}

/// Generates the three files for `surface`. `version` is the app version, shown in the header.
#[must_use]
pub fn generate(surface: &CtxSurface, version: &str) -> ApiFiles {
    ApiFiles {
        api_md: api_md(surface, version),
        types: types(surface),
        skill_md: skill_md(),
    }
}

/// Writes `<workflows_dir>/.api/` (`API.md`, the type file, `SKILL.md` and `VERSION`) when it is
/// missing or was written by another version or by other content. Returns whether it wrote.
/// Creates the directories. Never touches anything outside `.api/`.
///
/// # Errors
///
/// Fails when the folder cannot be created or a file cannot be written.
pub fn write_if_stale(workflows_dir: &Path, version: &str) -> io::Result<bool> {
    let files = generate(&CtxSurface::from_catalog(), version);
    let dir = workflows_dir.join(API_DIR);
    let stamp = format!("{version}\n{}\n", digest(&files));
    fs::create_dir_all(&dir)?;
    let wanted = [
        (API_FILE, files.api_md.as_bytes()),
        (TYPES_FILE, files.types.as_bytes()),
        (SKILL_FILE, files.skill_md.as_bytes()),
        (VERSION_FILE, stamp.as_bytes()),
    ];
    let mut wrote = false;
    for (name, bytes) in wanted {
        // A file that is missing, unreadable or different is written again.
        if fs::read(dir.join(name)).is_ok_and(|current| current == bytes) {
            continue;
        }
        write_atomic(&dir, name, bytes)?;
        wrote = true;
    }
    Ok(wrote)
}

/// Writes through a temporary name in the same folder, then renames it over the file.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temporary = dir.join(format!("{name}.tmp"));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, dir.join(name))
}

/// sha256 of the three generated files, lowercase hex. It goes into `VERSION` with the version.
fn digest(files: &ApiFiles) -> String {
    let mut hasher = Sha256::new();
    hasher.update(files.api_md.as_bytes());
    hasher.update(files.types.as_bytes());
    hasher.update(files.skill_md.as_bytes());
    let mut hex = String::new();
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

// ---------------------------------------------------------------------------------------------
// The examples. They are the complete files of docs/workflows/writing-workflows.md. The tests
// check that each compiles and has valid metadata, and that the fixture matches the first one.
// ---------------------------------------------------------------------------------------------

const MODULE_SKELETON: &str = r#"local M = {}

M.meta = {
  name = "my-workflow",
  description = "One line that says what the workflow does",
  trigger = { on = "manual" },
}

function M.execute(ctx, input)
  -- the work goes here
end

return M
"#;

const BRANCH_FROM_TICKET: &str = r#"local M = {}

M.meta = {
  name = "branch-from-ticket",
  description = "Create a branch from a ticket key",
  trigger = { on = "manual" },
  concurrency = "queue",
  inputs = { ticket = { type = "string", prompt = "Ticket key" } },
}

function M.execute(ctx, input)
  local r = ctx.exec({ "ticket-branch-name", input.inputs.ticket }, { timeout = "30s" })
  if ctx.approve("Create branch " .. r.text .. "?") then
    ctx.branch.create({ name = r.text, checkout = true })
  end
end

return M
"#;

const BLOCK_RESTACK_ON_PROTECTED: &str = r#"local M = {}

M.meta = { name = "block-restack-on-protected", trigger = { on = "before.restack" } }

function M.execute(ctx, input)
  local root = input.trigger.root
  if root == "main" or root == "release" then
    ctx.block("Restack on " .. root .. " is blocked")
  end
end

return M
"#;

const COMMIT_MESSAGE_WITH_LLM: &str = r#"local M = {}

M.meta = { name = "commit-message-with-llm", trigger = { on = "transform.commit_message" } }

function M.execute(ctx, input)
  for _ = 1, 3 do
    local r = ctx.exec({ "llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only." },
                       { stdin = input.repo.staged_diff, timeout = "60s" })
    if #r.text > 0 then return r.text end
  end
end

return M
"#;

const RESTACK_CHILDREN_ON_PARENT_MOVE: &str = r#"local M = {}

M.meta = { name = "restack-children-on-parent-move", trigger = { on = "stack.parent_moved" }, concurrency = "replace" }

function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = input.trigger.branch })
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  if r.outcome == "conflict" then
    local w = ctx.wait("restack.resumed", { timeout = "24h" })
    if w.outcome ~= "resumed" then return ctx.fail("conflict not resolved") end
  elseif r.outcome ~= "done" then
    return ctx.fail("restack " .. r.outcome)
  end
  ctx.notify("Restacked " .. input.trigger.branch .. " onto " .. input.trigger.parent)
end

return M
"#;

/// The fixture of the branch-from-ticket example, in the format of `gitmaster workflow test`.
const BRANCH_FROM_TICKET_FIXTURE: &str = r#"return {
  input = { inputs = { ticket = "ABC-12" } },
  calls = {
    { fn = "exec", args = { argv = { "ticket-branch-name", "ABC-12" }, timeout = "30s" },
      reply = { text = "feat/abc-12", exit_code = 0, stderr = "", truncated = false } },
    { fn = "approve", args = { summary = "Create branch feat/abc-12?" }, reply = true },
    { fn = "branch.create", args = { name = "feat/abc-12", checkout = true } },
  },
  returns_nothing = true,
}
"#;

/// Types of the results and the input, written before the classes of `ctx`.
const TYPES_HEAD: &str = r"export type ExecResult = {
  text: string,
  exit_code: number,
  stderr: string,
  truncated: boolean,
  json: any?,
}

export type TryResult = {
  outcome: string,
  [string]: any,
}

export type Input = {
  inputs: { [string]: string },
  trigger: { [string]: any },
  repo: {
    root: string,
    branch: string?,
    head: string?,
    staged_diff: string?,
    staged_diff_path: string?,
  },
}

";

/// The modifiers of a shortcut (`meta::check_shortcut` keeps the list private; a test checks it).
const MODIFIER_NAMES: &[&str] = &["secondary", "ctrl", "alt", "shift", "cmd"];

/// The named keys of a shortcut (a test checks them against `meta::check_shortcut`).
const KEY_NAMES: &[&str] = &[
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

// ---------------------------------------------------------------------------------------------
// API.md
// ---------------------------------------------------------------------------------------------

fn api_md(surface: &CtxSurface, version: &str) -> String {
    let mut out = String::new();
    heading(&mut out, 1, "GitMaster workflow API");
    para(
        &mut out,
        &format!(
            "Version {version}. This file is generated from the action catalog. The app rewrites it when its version changes, so do not edit it. The file `ctx.d.luau` in this folder holds the same signatures as Luau types, for an editor with a Luau language server."
        ),
    );
    module_section(&mut out);
    meta_section(&mut out);
    input_section(&mut out);
    phase_section(&mut out);
    level_section(&mut out);
    calls_section(&mut out, surface);
    helpers_section(&mut out);
    outcomes_section(&mut out, surface);
    sandbox_section(&mut out);
    test_section(&mut out);
    examples_section(&mut out);
    out
}

fn module_section(out: &mut String) {
    heading(out, 2, "The module");
    para(
        out,
        "A workflow file is a Lua module. It returns a table with `meta` and `execute`.",
    );
    code(out, "lua", MODULE_SKELETON);
    para(
        out,
        "- `M.meta` is read when GitMaster loads the file, in a sandbox without `ctx`. No logic runs then.\n- `M.execute(ctx, input)` runs the workflow. `ctx` is the only way the script acts. `input` holds data only.",
    );
}

fn meta_section(out: &mut String) {
    heading(out, 2, "M.meta");
    para(
        out,
        "GitMaster reads `M.meta` when it loads the file, without running `execute`. An unknown field is an error, and the message lists the valid fields.",
    );
    let rows: Vec<Vec<String>> = meta::FIELDS
        .iter()
        .map(|field| {
            vec![
                format!("`{field}`"),
                meta_type(field),
                meta_help(field).to_owned(),
            ]
        })
        .collect();
    table(out, &["Field", "Type", "Meaning"], &rows);

    heading(out, 3, "trigger");
    let rows: Vec<Vec<String>> = meta::TRIGGER_FIELDS
        .iter()
        .map(|field| vec![format!("`{field}`"), trigger_help(field).to_owned()])
        .collect();
    table(out, &["Field", "Meaning"], &rows);
    let triggers: Vec<String> = catalog::triggers()
        .iter()
        .map(|on| format!("`{on}`"))
        .collect();
    para(out, &format!("Valid triggers: {}.", triggers.join(", ")));
    para(
        out,
        "`<operation>` is one of `commit`, `restack`, `push`, `branch_create` and `conflict_resolve`. `transform.<field>` takes `commit_message`, `branch_name` or `conflict_hunk`.",
    );

    heading(out, 3, "concurrency");
    let rows = vec![
        row(&[
            &format!("`{}`", Concurrency::Queue.as_str()),
            "One run waits behind the active run, and further triggers merge into that waiting run. The default.",
        ]),
        row(&[
            &format!("`{}`", Concurrency::Replace.as_str()),
            "The active run is cancelled if it has not reached an applying call. Otherwise the new run waits in the queue.",
        ]),
        row(&[
            &format!("`{}`", Concurrency::Skip.as_str()),
            "The trigger is discarded.",
        ]),
    ];
    table(
        out,
        &["Value", "When the trigger fires during a run"],
        &rows,
    );

    heading(out, 3, "Shortcuts");
    let modifiers: Vec<String> = MODIFIER_NAMES.iter().map(|m| format!("`{m}`")).collect();
    let keys: Vec<String> = KEY_NAMES.iter().map(|k| format!("`{k}`")).collect();
    para(
        out,
        &format!(
            "`trigger.key` is `modifier-...-key`, such as `secondary-shift-b`. `secondary` is Cmd on macOS and Ctrl elsewhere. Modifiers: {}. A shortcut needs a modifier other than `shift`, and no modifier may repeat. The key is a letter, a digit or a punctuation mark, or one of these names: {}.",
            modifiers.join(", "),
            keys.join(", ")
        ),
    );

    heading(out, 3, "inputs");
    let rows: Vec<Vec<String>> = meta::INPUT_FIELDS
        .iter()
        .map(|field| vec![format!("`{field}`"), input_field_help(field).to_owned()])
        .collect();
    table(out, &["Field", "Meaning"], &rows);
    para(
        out,
        &format!(
            "Input types: {}. Inputs are typed before a manual run only; another trigger cannot have them. An input name must be letters, digits and underscores, and not start with a digit, so that `input.inputs.<name>` works.",
            meta::INPUT_TYPES
                .iter()
                .map(|ty| format!("`{ty}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
}

fn input_section(out: &mut String) {
    heading(out, 2, "The input value");
    para(
        out,
        "`execute` receives `input` as its second argument. It is data only, and read-only.",
    );
    let rows = vec![
        row(&[
            "`input.inputs`",
            "The values you typed for the `inputs` of `M.meta`, by name. Each is text.",
        ]),
        row(&[
            "`input.trigger`",
            "The payload of the trigger. Its fields depend on the trigger: `root` for `before.restack`, `parent` for `stack.parent_moved`.",
        ]),
        row(&[
            "`input.repo.root`",
            "The root folder of the repository, when the run starts.",
        ]),
        row(&[
            "`input.repo.branch`",
            "The branch checked out when the run starts.",
        ]),
        row(&["`input.repo.head`", "The head commit when the run starts."]),
        row(&[
            "`input.repo.staged_diff`",
            "The staged diff. A diff over 1 MiB is cut, and then ends with the line `[diff truncated at 1 MiB]`.",
        ]),
        row(&[
            "`input.repo.staged_diff_path`",
            "Only when the diff was cut: the path of a temporary file with the whole diff. It is deleted when the run ends.",
        ]),
    ];
    table(out, &["Field", "Holds"], &rows);
}

fn phase_section(out: &mut String) {
    heading(out, 2, "What `execute` returns");
    para(
        out,
        "The return value depends on the trigger that started the run.",
    );
    let rows = vec![
        row(&[
            "`manual` and events",
            "Ignored. Finishing without an error ends the run as `done`.",
        ]),
        row(&[
            "`before.<operation>`",
            "Ignored. `ctx.block(reason)` ends the run as `blocked` and vetoes the operation. An error also vetoes it.",
        ]),
        row(&[
            "`transform.<field>`",
            "The new value of the field. Return `nil` for no suggestion. If you changed the field meanwhile, the value becomes a suggestion and does not overwrite what you wrote.",
        ]),
        row(&[
            "`after.<operation>`",
            "Ignored. The run only observes the operation. A failure shows a warning.",
        ]),
    ];
    table(out, &["Trigger", "Return value and effect"], &rows);
}

fn level_section(out: &mut String) {
    heading(out, 2, "Effect levels");
    para(
        out,
        "Every call has an effect level. It says what the call can change.",
    );
    let rows = vec![
        row(&[
            "read",
            "Observes the repository or runs a program. It changes nothing.",
        ]),
        row(&[
            "propose",
            "Prepares a result for you to accept, such as a commit message or a plan. Nothing is applied.",
        ]),
        row(&[
            "apply",
            "Changes the repository or the remote. A manual run applies these after you review the script. A run started by an event asks you before each one, unless you granted automatic approval for this version. A run started by an agent always waits for you. After any `ctx.exec` in a run, every later apply call asks again. `ctx.push.lease` always asks.",
        ]),
        row(&[
            "pauses for a human",
            "Stops the run until you answer in the app.",
        ]),
        row(&[
            "pauses until an event",
            "Stops the run until the named event arrives or its timeout ends.",
        ]),
    ];
    table(out, &["Level", "Meaning"], &rows);
}

fn calls_section(out: &mut String, surface: &CtxSurface) {
    heading(out, 2, "Calls");
    para(
        out,
        "Named arguments go in one table, such as `ctx.branch.create({ name = \"fix/login\", checkout = true })`. `ctx.notify`, `ctx.approve`, `ctx.exec` and `ctx.wait` take their first arguments by position, then an optional table `opts`.",
    );
    para(
        out,
        "Outcomes are the values a call can end with. A value other than `done` raises an error unless the call runs inside `ctx.try`; see Outcomes and errors below.",
    );
    for (namespace, functions) in grouped(surface) {
        if namespace.is_empty() {
            heading(out, 3, "Top level");
        } else {
            heading(out, 3, &format!("`ctx.{namespace}`"));
        }
        for f in functions {
            function_entry(out, f);
        }
    }
}

/// What the reference says about one `ctx` function, as plain Markdown without a heading: the
/// effect level, the outcomes, the arguments and the result. Editors show it beside a completion,
/// so the editor and `.api/API.md` cannot disagree.
#[must_use]
pub fn function_summary(f: &CtxFn) -> String {
    let outcomes: Vec<String> = f.outcomes.iter().map(|o| format!("`{o}`")).collect();
    let mut text = format!(
        "Effect level: {}.\n\nOutcomes: {}.",
        level_words(f),
        outcomes.join(", ")
    );
    let rows = argument_rows(f);
    if rows.is_empty() {
        text.push_str("\n\nNo arguments.");
    } else {
        text.push_str("\n\nArguments:");
        for row in rows {
            let _ = write!(
                text,
                "\n- {} ({}, required: {}): {}",
                row[0], row[1], row[2], row[3]
            );
        }
    }
    let _ = write!(text, "\n\nResult: {}", result_words(f.path));
    text
}

/// The call and the documentation of a helper (`now`, `random`, `block`, `fail`, `try`), from the
/// same table as the reference. `None` for a name that is not a helper.
#[must_use]
pub fn helper_summary(name: &str) -> Option<(String, String)> {
    let row = helper_row(name)?;
    let call = row[0].trim_matches('`').to_owned();
    Some((call, format!("Arguments: {}\n\nResult: {}", row[1], row[2])))
}

fn function_entry(out: &mut String, f: &CtxFn) {
    heading(out, 4, &format!("`{}`", f.signature()));
    let outcomes: Vec<String> = f.outcomes.iter().map(|o| format!("`{o}`")).collect();
    para(
        out,
        &format!(
            "Effect level: {}.\n\nOutcomes: {}.",
            level_words(f),
            outcomes.join(", ")
        ),
    );
    arguments_table(out, f);
    para(out, &format!("Result: {}", result_words(f.path)));
}

fn arguments_table(out: &mut String, f: &CtxFn) {
    let rows = argument_rows(f);
    if rows.is_empty() {
        para(out, "No arguments.");
    } else {
        table(out, &["Argument", "Type", "Required", "Meaning"], &rows);
    }
}

fn argument_rows(f: &CtxFn) -> Vec<Vec<String>> {
    let positional = f.positional();
    let by_position = matches!(f.shape, ArgShape::Positional(_));
    f.fields
        .iter()
        .map(|field| {
            let label = if positional.contains(&field.name) || !by_position {
                field.name.to_owned()
            } else {
                format!("opts.{}", field.name)
            };
            vec![
                format!("`{label}`"),
                type_words(field.kind),
                yes_no(field.required).to_owned(),
                field.help.to_owned(),
            ]
        })
        .collect()
}

fn helpers_section(out: &mut String) {
    heading(out, 2, "Helpers");
    para(
        out,
        "These calls do not act on the repository, so they have no effect level or outcome.",
    );
    let rows: Vec<Vec<String>> = HELPERS.iter().filter_map(|name| helper_row(name)).collect();
    table(out, &["Call", "Arguments", "Result"], &rows);
}

fn helper_row(name: &str) -> Option<Vec<String>> {
    let row = match name {
        "try" => row(&[
            "`ctx.try(fn)`",
            "`fn`: a function that makes one call on `ctx`.",
            "A table with `outcome`, the outcome of that call, and the fields of its result. It does not raise. `ctx.try` is not a call in the run record; the call inside it is.",
        ]),
        "fail" => row(&[
            "`ctx.fail(reason)`",
            "`reason`: text.",
            "Ends the run as `failed` with the reason.",
        ]),
        "block" => row(&[
            "`ctx.block(reason)`",
            "`reason`: text.",
            "Ends the run as `blocked` with the reason. In a `before.<operation>` phase it vetoes the operation.",
        ]),
        "now" => row(&[
            "`ctx.now()`",
            "None.",
            "An integer: milliseconds since the Unix epoch. The value is recorded in the run.",
        ]),
        "random" => row(&[
            "`ctx.random()`",
            "None.",
            "A float in [0, 1). The value is recorded in the run.",
        ]),
        _ => return None,
    };
    Some(row)
}

fn outcomes_section(out: &mut String, surface: &CtxSurface) {
    heading(out, 2, "Outcomes and errors");
    let bullets = [
        "A call that ends with an outcome other than `done` raises an error, and the run stops there. `ctx.try(fn)` returns the outcome instead. `ctx.exec` raises for `failed` and `timeout` too, unless it runs inside `ctx.try`.",
        "`ctx.wait` never raises. It returns `{ outcome = \"resumed\" | \"aborted\" | \"timeout\" }` and the script branches on it.",
        "A person who rejects an applying call gives it the outcome `rejected`, which raises like any other outcome other than `done`.",
        "An applying call that was running when the app stopped ends with `interrupted`. It never runs again.",
        "The arguments are checked against the fields before any effect. An unknown field, a missing required field or a value of the wrong type is a Lua error that names the valid fields. `pcall` can catch it; it has no outcome.",
        "A call to a name that is not in `ctx` is an error. The message lists the valid names at that level:",
    ];
    for bullet in bullets {
        push_line(out, &format!("- {bullet}"));
    }
    out.push('\n');
    code(out, "text", &surface.unknown_name_message("branch", "make"));
    para(
        out,
        "Each call is one entry in the run record, with its resolved arguments, result, outcome and duration.",
    );
}

fn sandbox_section(out: &mut String) {
    heading(out, 2, "Sandbox and budgets");
    para(
        out,
        "The script runs in a Lua virtual machine inside the app. These are removed: `io`, `os`, `debug`, `package`, `require`, `load`, `loadstring`, `dofile`, `string.dump`, `os.time`, `os.clock` and `math.random`. Use `ctx.now()` and `ctx.random()` instead; both are recorded in the run.",
    );
    para(
        out,
        "The global environment is read-only and new for each run. The script has no authority of its own: everything it does goes through `ctx`. The sandbox does not limit a program that `ctx.exec` starts.",
    );
    let budgets = Budgets::default();
    para(out, "Each run has these budgets:");
    push_line(out, &format!("- {} instructions executed.", budgets.steps));
    push_line(
        out,
        &format!(
            "- {} s of the script's own running time. Time spent in a host call (an approval, a wait, a program, a Git operation) does not count.",
            budgets.script_time.as_secs()
        ),
    );
    push_line(
        out,
        &format!("- {} MiB of memory.", budgets.memory / (1024 * 1024)),
    );
    push_line(
        out,
        &format!(
            "- Strings of at most {} MiB.",
            budgets.string_bytes / (1024 * 1024)
        ),
    );
    push_line(
        out,
        &format!("- {} calls on `ctx`, helpers included.", budgets.ctx_calls),
    );
    push_line(out, "- The call depth of Luau.");
    out.push('\n');
    para(
        out,
        "A run that exceeds a budget ends as `failed`, with the reason. Cancelling always works, even while the script waits. Cancelling a run stops the script; a waiting call (`ctx.approve`, `ctx.wait`, `ctx.exec`) ends at once, and cancelling `ctx.exec` kills its program.",
    );
}

fn test_section(out: &mut String) {
    heading(out, 2, "Test it");
    para(
        out,
        "Three one-shot commands test a file without the app window. None starts a resident process, and none applies a change.",
    );
    let rows = vec![
        row(&[
            "`gitmaster workflow check <file>`",
            "Compiles the file, validates `M.meta`, and checks the trigger and the inputs. Each problem prints as `file:line: message`. A non-zero exit code means an error.",
        ]),
        row(&[
            "`gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]`",
            "Runs the script against the repository. Reads are real. Calls that propose, apply or run a program are recorded, and each returns a result of the same shape. Programs never run. Prints one numbered line per call, marked `read`, `propose`, `applies`, `asks` or `waits`, and `simulated` when the call got a made-up result. Every call after the first simulated result is marked `(estimate)`, because it depends on a value that is not real. The last line says how the run ended, such as `returned: null`, where `null` means the script returned nothing.",
        ]),
        row(&[
            "`gitmaster workflow test <file> [fixture]`",
            "Runs the script against the canned answers of a fixture, and checks the calls and the return value.",
        ]),
    ];
    table(out, &["Command", "What it does"], &rows);
    heading(out, 3, "Fixtures");
    para(
        out,
        "A fixture is a Lua file that returns one table. It is `<workflows dir>/.tests/<script file stem>.lua`, or the path given as the second argument of `test`. The folder starts with a dot, so it is not read as a workflow.",
    );
    let rows = vec![
        row(&["`input`", "Optional. The `input` value of the run."]),
        row(&[
            "`calls`",
            "Every `ctx` call of the run, in order. `now` and `random` are calls too. Each has `fn` (the name without `ctx.`), and optionally `args`, which must equal the arguments of the call. Then either `reply`, the value the call returns (a call that returns nothing needs no `reply`), or `outcome`, the outcome the call ends with, with an optional `message`. In `args`, a call with positional arguments uses the argument names: `ctx.approve(\"Go?\")` is `args = { summary = \"Go?\" }`, `ctx.notify(\"x\")` is `{ text = \"x\" }`, `ctx.exec({ \"git\" }, { timeout = \"30s\" })` is `{ argv = { \"git\" }, timeout = \"30s\" }` and `ctx.wait(\"restack.resumed\")` is `{ event = \"restack.resumed\" }`.",
        ]),
        row(&[
            "`returns`",
            "Optional. The value `execute` must return. Checked only when present.",
        ]),
        row(&[
            "`returns_nothing`",
            "Optional. `true` when `execute` must return nothing.",
        ]),
        row(&[
            "`ends`",
            "Optional. `done` (the default), `blocked`, `failed` or `cancelled`.",
        ]),
        row(&[
            "`reason`",
            "Optional. The reason expected for `blocked` or `failed`.",
        ]),
    ];
    table(out, &["Field", "Meaning"], &rows);
    para(
        out,
        "A worked fixture for the branch-from-ticket example, saved as `.tests/branch-from-ticket.lua`:",
    );
    code(out, "lua", BRANCH_FROM_TICKET_FIXTURE);
}

fn examples_section(out: &mut String) {
    heading(out, 2, "Examples");
    let examples = [
        (
            "Branch from a ticket",
            "branch-from-ticket.lua",
            "A manual workflow with one input. The naming program's output is the branch name. The branch is created only when you approve.",
            BRANCH_FROM_TICKET,
        ),
        (
            "Block restack on a protected branch",
            "block-restack-on-protected.lua",
            "A gate that runs before every restack.",
            BLOCK_RESTACK_ON_PROTECTED,
        ),
        (
            "Commit message from an LLM",
            "commit-message-with-llm.lua",
            "A transform of the commit message. It tries up to three times and returns nothing when every answer is empty.",
            COMMIT_MESSAGE_WITH_LLM,
        ),
        (
            "Restack children when the parent moves",
            "restack-children-on-parent-move.lua",
            "An event that restacks the children of a parent that moved. It replaces an older run that has not reached its apply step.",
            RESTACK_CHILDREN_ON_PARENT_MOVE,
        ),
    ];
    for (title, file, about, source) in examples {
        heading(out, 3, title);
        para(out, &format!("{about} Save it as `{file}`."));
        code(out, "lua", source);
    }
}

// ---------------------------------------------------------------------------------------------
// The type file
// ---------------------------------------------------------------------------------------------

fn types(surface: &CtxSurface) -> String {
    let mut out = String::new();
    out.push_str(
        "-- Generated by GitMaster from the action catalog, for a Luau language server.\n",
    );
    out.push_str(
        "-- Not checked in an editor: adjust it if your language server reports errors.\n",
    );
    out.push_str("-- Use it as: function M.execute(ctx: Ctx, input: Input) ... end\n\n");
    out.push_str(TYPES_HEAD);
    for f in surface.functions() {
        if let Some(opts) = opts_type(f) {
            out.push_str(&opts);
        }
    }
    let groups = grouped(surface);
    for (namespace, functions) in &groups {
        if namespace.is_empty() {
            continue;
        }
        let _ = writeln!(out, "declare class Ctx{}", pascal(namespace));
        for f in functions {
            let _ = writeln!(out, "  {}: {}", method(f.path), fn_type(f));
        }
        out.push_str("end\n\n");
    }
    out.push_str("declare class Ctx\n");
    for namespace in groups.keys().filter(|ns| !ns.is_empty()) {
        let _ = writeln!(out, "  {namespace}: Ctx{}", pascal(namespace));
    }
    for f in surface
        .functions()
        .iter()
        .filter(|f| namespace(f.path).is_empty())
    {
        let _ = writeln!(out, "  {}: {}", f.path, fn_type(f));
    }
    for helper in HELPERS {
        let _ = writeln!(out, "  {helper}: {}", helper_type(helper));
    }
    out.push_str("end\n");
    out
}

/// The options table of a positional call with fields beyond its positional ones, such as
/// `ExecOpts` for `ctx.exec(argv, opts)`.
fn opts_type(f: &CtxFn) -> Option<String> {
    let ArgShape::Positional(names) = &f.shape else {
        return None;
    };
    let rest: Vec<&FieldSpec> = f
        .fields
        .iter()
        .filter(|field| !names.contains(&field.name))
        .collect();
    if rest.is_empty() {
        return None;
    }
    let mut out = format!("export type {}Opts = {{\n", pascal(f.path));
    for field in rest {
        let _ = writeln!(out, "  {}: {},", field.name, field_type(field));
    }
    out.push_str("}\n\n");
    Some(out)
}

fn fn_type(f: &CtxFn) -> String {
    let params = match &f.shape {
        ArgShape::Table => table_params(f),
        ArgShape::Positional(names) => positional_params(f, names),
    };
    format!("({params}) -> {}", result_type(f.path))
}

/// A call with one table: `args: { name: string, checkout: boolean? }`. The table is optional when
/// no field is required.
fn table_params(f: &CtxFn) -> String {
    if f.fields.is_empty() {
        return String::new();
    }
    let fields: Vec<String> = f
        .fields
        .iter()
        .map(|field| format!("{}: {}", field.name, field_type(field)))
        .collect();
    let optional = if f.fields.iter().any(|field| field.required) {
        ""
    } else {
        "?"
    };
    format!("args: {{ {} }}{optional}", fields.join(", "))
}

fn positional_params(f: &CtxFn, names: &[&str]) -> String {
    let mut params: Vec<String> = names
        .iter()
        .filter_map(|name| f.fields.iter().find(|field| field.name == *name))
        .map(|field| format!("{}: {}", field.name, field_type(field)))
        .collect();
    if f.fields.iter().any(|field| !names.contains(&field.name)) {
        params.push(format!("opts: {}Opts?", pascal(f.path)));
    }
    params.join(", ")
}

fn helper_type(name: &str) -> &'static str {
    match name {
        "now" | "random" => "() -> number",
        "block" | "fail" => "(reason: string) -> ()",
        "try" => "(fn: () -> any) -> TryResult",
        _ => "any",
    }
}

/// The Luau type of one field: `string`, `{ string }`, `boolean`, `any`, with `?` when optional.
fn field_type(field: &FieldSpec) -> String {
    let base = luau_type(field.kind);
    if field.required {
        base.to_owned()
    } else {
        format!("{base}?")
    }
}

fn luau_type(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text | FieldKind::Duration | FieldKind::Choice(_) => "string",
        FieldKind::TextList => "{ string }",
        FieldKind::Bool => "boolean",
        FieldKind::Any => "any",
    }
}

/// The Luau type of a call's result (D105, D106).
fn result_type(path: &str) -> &'static str {
    match path {
        "exec" => "ExecResult",
        "approve" => "boolean",
        "wait" => "{ outcome: string }",
        "commit.create" => "{ commit: string }",
        "restack.plan" | "reparent.plan" => "{ plan_id: string, branch_count: number }",
        "stack.tree" => "{ branches: { { name: string, parent: string?, stale: boolean } } }",
        "stack.stale" => "{ branches: { string } }",
        "scorecards" => {
            "{ { id: number, name: string, branch: string, head: string, status: string, stale: boolean, blocking_open: number, label: string, decorations: { string }, subject: string, body: string, origin: string, model: string?, run_thread: string?, workflow_run: string?, created_at: number } }"
        }
        _ => "()",
    }
}

// ---------------------------------------------------------------------------------------------
// SKILL.md
// ---------------------------------------------------------------------------------------------

fn skill_md() -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("name: gitmaster-workflows\n");
    out.push_str("description: Write, check and test GitMaster workflow scripts (Lua files in the workflows folder). Use when asked to create or change a GitMaster workflow.\n");
    out.push_str("---\n\n");
    out.push_str("# Writing GitMaster workflows\n\n");
    out.push_str("- Write one `.lua` file in the workflows folder, the folder that holds this `.api/` folder: `~/.config/gitmaster/workflows/` (under `$XDG_CONFIG_HOME` when that is set). Do not write into a repository.\n");
    out.push_str("- `.api/ctx.d.luau` has the `ctx` signatures as Luau types, for an editor.\n");
    out.push_str("- Read `.api/API.md` first. It lists every `ctx` function with its arguments, effect level and outcomes, and the shape of `M.meta`.\n");
    out.push_str("- Check, dry-run and test, and repeat until each passes:\n");
    out.push_str("  1. `gitmaster workflow check <file>`. Fix every `file:line: message`.\n");
    out.push_str("  2. `gitmaster workflow dry-run <file>`. Read the list of calls.\n");
    out.push_str("  3. Write `.tests/<file stem>.lua` and run `gitmaster workflow test <file>`.\n");
    out.push_str("- A call that ends with an outcome other than `done` raises, unless it runs inside `ctx.try`. An unknown name in `ctx` fails and lists the valid names: correct the name from that message.\n");
    out.push_str("- Never say that a script is approved or has run. The person reviews it in GitMaster and confirms \"Run this version?\". Each change makes the script unreviewed again.\n");
    out.push_str("- Do not edit the files in `.api/`. The app rewrites them.\n\n");
    out.push_str("In a fixture, `args` of a call with positional arguments uses the argument names (`approve` takes `summary`, `notify` takes `text`, `exec` takes `argv`). A call with no result needs no `reply`.\n\n");
    out.push_str("A fixture, saved as `<workflows dir>/.tests/branch-from-ticket.lua`:\n\n");
    out.push_str("```lua\n");
    out.push_str(BRANCH_FROM_TICKET_FIXTURE.trim_end());
    out.push_str("\n```\n");
    out
}

// ---------------------------------------------------------------------------------------------
// Wording shared by the Markdown and the types
// ---------------------------------------------------------------------------------------------

/// The namespace of a catalog name: `branch` for `branch.create`, `""` for a top-level function.
fn namespace(path: &str) -> &str {
    path.split_once('.').map_or("", |(ns, _)| ns)
}

/// The name of a function inside its namespace: `create` for `branch.create`.
fn method(path: &str) -> &str {
    path.split_once('.').map_or(path, |(_, method)| method)
}

/// The functions grouped by namespace, sorted; the top level is the group `""`.
fn grouped(surface: &CtxSurface) -> BTreeMap<&str, Vec<&CtxFn>> {
    let mut groups: BTreeMap<&str, Vec<&CtxFn>> = BTreeMap::new();
    for f in surface.functions() {
        groups.entry(namespace(f.path)).or_default().push(f);
    }
    groups
}

/// `branch` becomes `Branch`, `restack` becomes `Restack`; `wait` becomes `Wait`.
fn pascal(path: &str) -> String {
    path.split('.')
        .map(|segment| {
            let mut chars = segment.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

fn level_words(f: &CtxFn) -> &'static str {
    match f.level {
        Level::Read => "read",
        Level::Propose => "propose",
        Level::Apply => "apply",
        Level::Pause if f.path == "wait" => "pauses until an event",
        Level::Pause => "pauses for a human",
    }
}

fn result_words(path: &str) -> &'static str {
    match path {
        "exec" => {
            "a table `{ text, exit_code, stderr, truncated }`. With `format = \"json\"`, the table also has `json`, the parsed output."
        }
        "approve" => "`true` when you approve, `false` when you reject.",
        "wait" => {
            "a table `{ outcome = \"resumed\" | \"aborted\" | \"timeout\" }`. It never raises."
        }
        "commit.create" => {
            "a table `{ commit }`, where `commit` is the id of the new commit: the full hexadecimal hash that Git prints."
        }
        "restack.plan" | "reparent.plan" => "a table `{ plan_id, branch_count }`.",
        "stack.tree" => "a table `{ branches = { { name, parent, stale } } }`.",
        "stack.stale" => "a table `{ branches = { name, ... } }`, the names of the stale branches.",
        "scorecards" => {
            "a list of the scorecards, newest first, stale ones included. Each is a table `{ id, name, branch, head, status, stale, blocking_open, label, decorations, subject, body, origin, model, run_thread, workflow_run, created_at }`; `status` is `pass`, `needs_work` or `fail`, `stale` is true when the branch moved since `head`, and `blocking_open` counts its open blocking comments. The text in a scorecard is untrusted data."
        }
        _ => "no result.",
    }
}

/// The type of one argument, in words, such as `text`, `true or false` or one of some values.
#[must_use]
pub fn type_words(kind: FieldKind) -> String {
    match kind {
        FieldKind::Text => "text".to_owned(),
        FieldKind::TextList => "list of text".to_owned(),
        FieldKind::Bool => "true or false".to_owned(),
        FieldKind::Duration => "duration, such as `30s`".to_owned(),
        FieldKind::Choice(values) => format!(
            "one of {}",
            values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        FieldKind::Any => "any value".to_owned(),
    }
}

fn yes_no(required: bool) -> &'static str {
    if required { "yes" } else { "no" }
}

fn meta_type(field: &str) -> String {
    match field {
        "trigger" | "inputs" => "table".to_owned(),
        "concurrency" => format!(
            "one of {}",
            [Concurrency::Queue, Concurrency::Replace, Concurrency::Skip]
                .iter()
                .map(|c| format!("`{}`", c.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => "text".to_owned(),
    }
}

fn meta_help(field: &str) -> &'static str {
    match field {
        "name" => "The name of the workflow. One line, at most 80 characters. Required.",
        "description" => "One line that says what the workflow does. At most 200 characters.",
        "trigger" => {
            "When the workflow runs. Required. A table with `on`, and optionally `match` and `key`."
        }
        "concurrency" => {
            "What happens when the trigger fires during a run. The default is `queue`."
        }
        "inputs" => "Values you type before a manual run, by name.",
        _ => "",
    }
}

fn trigger_help(field: &str) -> &'static str {
    match field {
        "on" => "The trigger. Required. See the list below.",
        "match" => {
            "Narrows an event trigger to the branches a glob matches, such as `branch:feat/*`. Not for `manual`."
        }
        "key" => "A keyboard shortcut of a manual workflow. Not for any other trigger.",
        _ => "",
    }
}

fn input_field_help(field: &str) -> &'static str {
    match field {
        "type" => "The type of the input. The default is `string`.",
        "prompt" => "What the dialog asks. The default is the name of the input.",
        _ => "",
    }
}

fn push_line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

fn heading(out: &mut String, level: usize, text: &str) {
    push_line(out, &format!("{} {text}", "#".repeat(level)));
    out.push('\n');
}

fn para(out: &mut String, text: &str) {
    push_line(out, text);
    out.push('\n');
}

fn code(out: &mut String, lang: &str, body: &str) {
    push_line(out, &format!("```{lang}"));
    push_line(out, body.trim_end());
    push_line(out, "```");
    out.push('\n');
}

fn table(out: &mut String, header: &[&str], rows: &[Vec<String>]) {
    push_line(out, &format!("| {} |", header.join(" | ")));
    push_line(out, &format!("|{}", "---|".repeat(header.len())));
    for row in rows {
        push_line(out, &format!("| {} |", row.join(" | ")));
    }
    out.push('\n');
}

fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|cell| (*cell).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use serde_json::Value;

    use super::*;
    use crate::script::fake::ScriptedHost;
    use crate::script::inspect::inspect;
    use crate::script::luau::LuauEngine;
    use crate::script::port::{Budgets, HostReply, RunEnd, RunRequest, ScriptEngine};

    fn surface() -> CtxSurface {
        CtxSurface::from_catalog()
    }

    #[test]
    fn api_md_names_every_function_its_outcomes_and_its_arguments() {
        let surface = surface();
        let md = generate(&surface, "test").api_md;
        for f in surface.functions() {
            assert!(md.contains(&f.signature()), "{}", f.signature());
            for outcome in f.outcomes {
                assert!(md.contains(&format!("`{outcome}`")), "{} {outcome}", f.path);
            }
            for field in f.fields {
                assert!(md.contains(field.name), "{} {}", f.path, field.name);
            }
        }
    }

    #[test]
    fn the_files_say_what_an_agent_found_missing() {
        // Found by an agent that wrote a script from these files alone: fixture keys of
        // positional calls, the type file, the dry-run markers, optional `reply`, the commit id.
        let files = generate(&surface(), "test");
        for (what, text) in [
            ("approve key", "args = { summary = "),
            ("notify key", "{ text = "),
            ("exec key", "{ argv = "),
            ("type file in API.md", "ctx.d.luau"),
            ("estimate marker", "(estimate)"),
            ("returned line", "returned: null"),
            ("optional reply", "needs no `reply`"),
            ("commit id", "full hexadecimal hash"),
        ] {
            assert!(files.api_md.contains(text), "API.md lacks {what}: {text}");
        }
        assert!(files.skill_md.contains("ctx.d.luau"));
        assert!(files.skill_md.contains("`summary`"));
        assert!(files.skill_md.lines().count() < 60, "SKILL.md stays short");
    }

    #[test]
    fn api_md_covers_the_meta_fields_triggers_and_helpers() {
        let md = generate(&surface(), "test").api_md;
        for field in meta::FIELDS {
            assert!(!meta_help(field).is_empty(), "{field}");
            assert!(md.contains(&format!("`{field}`")), "{field}");
        }
        for on in catalog::triggers() {
            assert!(md.contains(&format!("`{on}`")), "{on}");
        }
        for helper in HELPERS {
            assert!(md.contains(&format!("`ctx.{helper}")), "{helper}");
        }
    }

    #[test]
    fn the_shortcut_notes_are_the_ones_the_parser_accepts() {
        for modifier in MODIFIER_NAMES {
            // A shortcut needs a modifier other than shift, so `shift` comes with `secondary`.
            let text = if *modifier == "secondary" {
                "secondary-b".to_owned()
            } else {
                format!("secondary-{modifier}-b")
            };
            assert!(meta::check_shortcut(&text).is_ok(), "{modifier}");
        }
        for key in KEY_NAMES {
            assert!(
                meta::check_shortcut(&format!("secondary-{key}")).is_ok(),
                "{key}"
            );
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let surface = surface();
        assert_eq!(generate(&surface, "1.2.3"), generate(&surface, "1.2.3"));
        assert_ne!(
            generate(&surface, "1.2.3").api_md,
            generate(&surface, "1.2.4").api_md
        );
    }

    #[test]
    fn every_example_is_a_valid_module() {
        let examples = [
            ("branch.lua", BRANCH_FROM_TICKET),
            ("block.lua", BLOCK_RESTACK_ON_PROTECTED),
            ("llm.lua", COMMIT_MESSAGE_WITH_LLM),
            ("children.lua", RESTACK_CHILDREN_ON_PARENT_MOVE),
            ("skeleton.lua", MODULE_SKELETON),
        ];
        for (name, source) in examples {
            let meta = inspect(&LuauEngine::new(), name, source)
                .unwrap_or_else(|problems| panic!("{name}: {problems:?}"));
            assert!(!meta.name.is_empty(), "{name}");
        }
    }

    #[test]
    fn the_branch_fixture_matches_what_the_example_does() {
        let engine = LuauEngine::new();
        let fixture = engine
            .read_data("branch-from-ticket.lua", BRANCH_FROM_TICKET_FIXTURE)
            .unwrap_or_else(|problems| panic!("{problems:?}"));
        let calls = fixture["calls"].as_array().expect("calls").clone();
        let mut host = ScriptedHost::new();
        for call in &calls {
            let reply = call.get("reply").cloned().unwrap_or(Value::Null);
            host = host.reply(call["fn"].as_str().expect("fn"), HostReply::Value(reply));
        }
        let input = fixture["input"].clone();
        let surface = surface();
        let request = RunRequest {
            chunk: "branch-from-ticket.lua",
            source: BRANCH_FROM_TICKET,
            input: &input,
            surface: &surface,
            budgets: Budgets::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let end = engine.run(&request, &mut host);
        assert_eq!(end, RunEnd::Returned(Value::Null));
        assert_eq!(host.calls.len(), calls.len(), "{:?}", host.calls);
        for (made, want) in host.calls.iter().zip(&calls) {
            assert_eq!(made.0, want["fn"].as_str().expect("fn"));
            assert_eq!(made.1, want["args"], "{}", made.0);
        }
    }

    #[test]
    fn the_types_declare_every_namespace_and_every_top_level_name() {
        let surface = surface();
        let types = generate(&surface, "test").types;
        let opened = types
            .lines()
            .filter(|line| line.starts_with("declare class"))
            .count();
        let closed = types.lines().filter(|line| line.trim() == "end").count();
        assert_eq!(opened, closed);
        for f in surface.functions() {
            if let Some((namespace, _)) = f.path.split_once('.') {
                assert!(
                    types.contains(&format!("declare class Ctx{}\n", pascal(namespace))),
                    "{namespace}"
                );
            }
        }
        let ctx = types
            .split("declare class Ctx\n")
            .nth(1)
            .and_then(|rest| rest.split("\nend").next())
            .expect("the Ctx class");
        for f in surface.functions().iter().filter(|f| !f.path.contains('.')) {
            assert!(ctx.contains(&format!("  {}:", f.path)), "{}", f.path);
        }
        for helper in HELPERS {
            assert!(ctx.contains(&format!("  {helper}:")), "{helper}");
        }
        assert!(types.contains("export type ExecOpts = {"));
        assert!(types.contains("exec: (argv: { string }, opts: ExecOpts?) -> ExecResult"));
    }

    #[test]
    fn skill_has_front_matter_and_stays_short() {
        let skill = generate(&surface(), "test").skill_md;
        assert!(skill.starts_with("---\nname: gitmaster-workflows\ndescription: "));
        assert!(
            skill.lines().count() < 60,
            "{} lines",
            skill.lines().count()
        );
        assert!(skill.contains("Run this version?"));
    }

    #[test]
    fn write_if_stale_writes_when_missing_stale_or_edited() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("made").join("workflows");
        assert!(write_if_stale(&root, "1.0.0").unwrap());
        assert!(!write_if_stale(&root, "1.0.0").unwrap());
        assert!(write_if_stale(&root, "1.1.0").unwrap());
        let version = fs::read_to_string(root.join(".api").join(VERSION_FILE)).unwrap();
        assert!(version.starts_with("1.1.0\n"), "{version}");

        let api = root.join(".api").join(API_FILE);
        fs::write(&api, "edited by hand").unwrap();
        assert!(write_if_stale(&root, "1.1.0").unwrap());
        assert_eq!(
            fs::read_to_string(&api).unwrap(),
            generate(&surface(), "1.1.0").api_md
        );

        fs::remove_file(root.join(".api").join(TYPES_FILE)).unwrap();
        assert!(write_if_stale(&root, "1.1.0").unwrap());
        assert!(root.join(".api").join(TYPES_FILE).is_file());
        assert!(!write_if_stale(&root, "1.1.0").unwrap());
    }

    #[test]
    fn write_if_stale_leaves_everything_outside_api_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("mine.lua"), "-- mine").unwrap();
        fs::create_dir(root.join(".tests")).unwrap();
        assert!(write_if_stale(root, "1.0.0").unwrap());
        assert!(write_if_stale(root, "2.0.0").unwrap());
        assert_eq!(
            fs::read_to_string(root.join("mine.lua")).unwrap(),
            "-- mine"
        );
        assert!(root.join(".tests").is_dir());
        let names: Vec<String> = fs::read_dir(root.join(".api"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 4, "no temporary files are left: {names:?}");
    }

    #[test]
    fn a_target_that_cannot_hold_the_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("a-file");
        fs::write(&blocked, "not a folder").unwrap();
        assert!(write_if_stale(&blocked, "1.0.0").is_err());
    }

    #[test]
    fn a_summary_states_the_outcomes_and_the_arguments_of_the_reference() {
        let surface = CtxSurface::from_catalog();
        let exec = surface.get("exec").unwrap();
        let text = function_summary(exec);
        assert!(text.starts_with("Effect level: "), "{text}");
        for outcome in exec.outcomes {
            assert!(
                text.contains(&format!("`{outcome}`")),
                "{outcome} in {text}"
            );
        }
        assert!(text.contains("\n- `argv` ("), "{text}");
        assert!(text.contains("\n\nResult: "), "{text}");
    }

    #[test]
    fn every_helper_has_a_call_and_a_text_and_no_other_name_does() {
        for name in HELPERS {
            let (call, text) = helper_summary(name).unwrap_or_else(|| panic!("{name}"));
            assert!(call.starts_with("ctx."), "{call}");
            assert!(text.starts_with("Arguments: "), "{text}");
        }
        assert_eq!(helper_summary("branch"), None);
    }
}
