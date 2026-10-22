---
title: Command line
description: The gitmaster command, the arguments it accepts, and the subcommands that check and test workflow scripts.
status: partial
order: 30
---

The `gitmaster` command opens the app. The workflow subcommands run once and exit. They keep no process running and no state between runs, and none of them applies a change to a repository. The section Open a repository covers the arguments the app accepts. The other sections cover subcommands that the app does not provide yet.

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
- Any other single argument is taken as a path, so `--help` is not a help option.
- Two or more arguments print `usage: gitmaster [PATH]` to the standard error and exit with status `2`.

The theme is chosen with the `GITMASTER_THEME` environment variable, as described in [Configuration](configuration.md). Installation puts the command on your `PATH`, as described in [Installation](../getting-started/installation.md).

## MCP shim

> **Planned.** The `gitmaster mcp` subcommand is not available in the app yet.

`gitmaster mcp` is a stdio server that an MCP client starts. It forwards each request to the running app over a Unix socket, using a token. If the app is closed, the shim opens it. See [MCP server](../agents/mcp-server.md).

## Check a workflow

> **Planned.** The `gitmaster workflow check` subcommand is not available in the app yet.

```sh
gitmaster workflow check <file> [--json]
```

Compiles the script in the same sandbox the app uses, and checks `M.meta`, the trigger and the inputs. Each problem is printed on its own line as `file:line: message`. `--json` prints the problems as JSON instead. The command exits with a non-zero status when it finds an error.

## Dry run a workflow

> **Planned.** The `gitmaster workflow dry-run` subcommand is not available in the app yet.

```sh
gitmaster workflow dry-run <file> [--input k=v] [--repo <path>]
```

Runs the script and prints the list of calls it makes.

- `--input k=v` sets the value of an input, by name.
- `--repo <path>` names the repository that the run reads.
- Reads, such as `ctx.stack.tree`, are real.
- Calls that propose or apply a change are recorded, and each returns a result of the same shape, so the script continues.
- `ctx.exec` never runs a program in a dry run.
- Anything that follows a stubbed result is an estimate, and the list marks it as one.

## Test a workflow

> **Planned.** The `gitmaster workflow test` subcommand is not available in the app yet.

```sh
gitmaster workflow test <file> [fixture]
```

Runs the script against a fake `ctx`. The fixture gives the answer to each call, in Lua. The command checks the sequence of calls the script makes and the value it returns, and reports any difference. See [Writing workflows with an AI agent](../workflows/ai-agents.md) for a suggested way to write a fixture.

For the calls the script can make, see [Workflow API](workflow-api.md).
