---
title: Command line
description: The gitmaster command, the arguments it accepts, and the subcommands that check and test workflow scripts.
status: partial
order: 30
---

The `gitmaster` command opens the app. The workflow, thread and agent subcommands run once and exit. They keep no process running and no state between runs, and none of them applies a change to a repository. The section Open a repository covers the arguments the app accepts. The MCP shim is not available yet.

## Open a repository

`gitmaster` takes at most one argument, a path. Each window shows one repository.

```sh
gitmaster                 # open the repository that contains the current directory
gitmaster ~/src/project   # open the repository at this path
gitmaster --version       # print the version and exit
```

- With no argument, the app opens the repository that contains the current directory. Outside a repository, it opens the most recent repository that still exists.
- With a path, the app opens the repository at that path. A relative path is resolved from the current directory.
- `--version` prints `gitmaster` followed by the version number, then exits.
- Any other single argument is taken as a path, so `--help` is not a help option. The word `workflow` is the subcommand below; to open a folder with that name, write `./workflow`.
- Two or more arguments print `usage: gitmaster [PATH]` to the standard error and exit with status `2`.

The theme is chosen with the `GITMASTER_THEME` environment variable, as described in [Configuration](configuration.md). Installation puts the command on your `PATH`, as described in [Installation](../getting-started/installation.md).

## MCP shim

> **Planned.** The `gitmaster mcp` subcommand is not available in the app yet.

`gitmaster mcp` is a stdio server that an MCP client starts. It forwards each request to the running app over a Unix socket, using a token. If the app is closed, the shim opens it. See [MCP server](../agents/mcp-server.md).

## Check a workflow

```sh
gitmaster workflow check <file> [--json]
```

Compiles the script in the same sandbox the app uses, and checks that it is a module with `execute`, that `M.meta` is valid, and that the trigger and the inputs are. Each problem is printed on its own line as `file:line: message`, and a file with no problem prints `file: ok`. The messages name the valid choices, such as the valid triggers, so you can correct the file from them. A problem with no single line prints as `file: message`.

`--json` prints one object instead: `{"file": "...", "ok": false, "problems": [{"line": 4, "message": "..."}]}`.

The exit status is `0` when the file has no problem, `1` when it has one, and `2` for a wrong command line.

## Dry run a workflow

> **Planned.** The stack reads, `ctx.stack.tree` and `ctx.stack.stale`, are simulated until the stack calls are available.

```sh
gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]
```

Runs the script and prints the list of calls it makes, one numbered line each, with its resolved arguments and what kind of call it is: `read`, `propose`, `applies`, `asks` or `waits`.

- `--input k=v` sets the value of an input, by name. Give every input the script declares, and no other.
- `--repo <path>` names the repository that the run reads. Without it, the repository that contains the current directory is read, and outside a repository `input.repo` is empty.
- Calls that propose or apply a change are recorded, and each returns a result of the same shape, so the script continues. Nothing is applied.
- `ctx.exec` never runs a program in a dry run.
- `ctx.approve` is answered as if you approved.
- Anything that follows a simulated result is an estimate, and the line says so.

The last line says how the run ended: `returned: <value>`, `ended: blocked: <reason>`, `ended: failed: <reason>` or `ended: cancelled`. The exit status is `0` when the run returned or was blocked, `1` when it failed or the file has a problem, and `2` for a wrong command line.

## Test a workflow

```sh
gitmaster workflow test <file> [fixture]
```

Runs the script against a fake `ctx`. The fixture gives the answer to each call, in Lua. The command checks the sequence of calls the script makes and the value it returns, and reports every difference. See [Writing workflows with an AI agent](../workflows/ai-agents.md) for a suggested way to write a fixture.

Without a `fixture` argument, the command reads `.tests/<file name without .lua>.lua` in the folder of the script. A folder whose name starts with a dot is not read as a workflow source, so fixtures never load as workflows.

A fixture is a Lua file that returns a table:

```lua
return {
  input = { inputs = { ticket = "ABC-1" } },
  calls = {
    { fn = "exec", reply = { text = "feat/abc-1", exit_code = 0 } },
    { fn = "approve", reply = true },
    { fn = "branch.create", args = { name = "ABC-1", checkout = true } },
  },
}
```

| Field | Meaning |
|---|---|
| `input` | The `input` the script receives: `inputs`, `trigger` and `repo`. Each defaults to an empty table. |
| `calls` | Every call the script must make, in order, including `now` and `random`. Each entry has `fn`, the name without `ctx.`. It may have `args`, which the call's arguments must equal; `reply`, the value the call returns; and `outcome` with an optional `message`, which makes the call end with that outcome. |
| `returns` | The value `M.execute` must return. It is checked only when the field is present. |
| `returns_nothing` | `true` when `M.execute` must return nothing. |
| `ends` | How the run ends: `done` (the default), `blocked`, `failed` or `cancelled`. With `reason` for the text of a block or a failure. |

A passing test prints `file: ok (3 calls)` and exits `0`. Each difference prints on its own line, such as `call 2: expected ctx.approve, got ctx.branch.create` or `missing calls: the script stopped after 2 of 3`, and the exit status is `1`. A missing or malformed fixture exits `2`.

For the calls the script can make, see [Workflow API](workflow-api.md).

## Capture a thread event

```sh
gitmaster thread hook --harness claude-code
```

`--harness` names the agent that sent the event. The integration sets it; an id this version does not know is logged and the event is dropped.

The agent integration runs this command from the agent's hooks. You do not run it yourself. It reads one JSON event from the standard input, redacts it, stores it in the thread database of the repository named in the event, and exits. It starts no resident process and opens no window.

The command always exits with status `0` and prints nothing, even when it cannot store the event, so a failure never reaches the agent. A problem is written to `.git/gitmaster/threads.log`. An event that cannot be written at once is kept redacted in `.git/gitmaster/threads-spool/` and stored later.

## Test a redaction pattern

```sh
gitmaster thread redact-test [--text <text>]
```

Reads a sample from the standard input, or from `--text`, applies the patterns of your `settings.json` and prints the result, with a line per match naming the pattern. Nothing is stored. The exit status is `0` when the patterns are valid, `1` when a pattern is not valid (the problem is printed as `pattern <name>: message`), and `2` for a wrong command line.

## Manage the agent integration

The name after the subcommand is the agent's id. `gitmaster harness list` prints the ids this version knows. An id it does not know exits with status `2` and lists the valid ones.

```sh
gitmaster harness list
gitmaster harness install claude-code
gitmaster harness status claude-code
gitmaster harness uninstall claude-code
```

- `install` generates the plugin in the user data directory and prints the commands that add it to the agent. Run them, or add `--run` to have GitMaster run them when the agent's command is on your `PATH`.
- `status` prints whether the plugin is current, whether its hooks point at this `gitmaster`, whether capture is paused and why, and how many events were skipped or are waiting. The exit status is `0` when the integration works and `1` when it needs attention.
- `uninstall` prints the commands that remove the plugin from the agent, with `--run` as above. Stored threads are kept.

See [Agent integration](../agents/agent-integration.md).

