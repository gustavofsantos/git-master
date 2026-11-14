---
title: Command line
description: The gitmaster command, the arguments it accepts, the terminal companion, the subcommands that list and create worktrees and workspaces, and the ones that check and test workflow scripts.
status: partial
order: 30
---

The `gitmaster` command opens the app. The worktree, workspace, workflow, thread and agent subcommands run once and exit. They keep no process running and no state between runs. Only `gitmaster worktree add`, `remove` and `include`, and `gitmaster workspace create`, `add` and `remove`, change a repository. The section Open a repository covers the arguments the app accepts. `gitmaster mcp` is the MCP server your agent starts. `gitmaster tui` runs GitMaster in the terminal until you quit it.

## Open a repository

`gitmaster` takes at most one argument, a path. Each window shows one repository.

```sh
gitmaster                 # open the repository that contains the current directory
gitmaster ~/src/project   # open the repository at this path
gitmaster --version       # print the version and exit
```

- With no argument, the app opens the repository that contains the current directory. Outside a repository, it opens the most recent repository that still exists.
- With a path, the app opens the repository at that path. A relative path is resolved from the current directory.
- `--version` prints `gitmaster` followed by the version number, then exits. When a preview feature is on, it also prints a `preview features:` line. See [Preview features](configuration.md#preview-features).
- Any other single argument is taken as a path, so `--help` is not a help option. The words `worktree`, `workspace`, `workflow` and `tui` are the subcommands below; to open a folder with one of those names, write `./worktree`, `./workspace`, `./workflow` or `./tui`.
- Two or more arguments print `usage: gitmaster [PATH]` to the standard error and exit with status `2`.

The theme is chosen with the `GITMASTER_THEME` environment variable, as described in [Configuration](configuration.md). Installation puts the command on your `PATH`, as described in [Installation](../getting-started/installation.md).

## Worktrees

```sh
gitmaster worktree list [--format paths|branches|tsv|json] [--linked]
gitmaster worktree branches [--missing]
gitmaster worktree path <branch>
gitmaster worktree add <branch> [<base>] [--root <dir>] [--include symlink|reflink|none]
gitmaster worktree remove <branch> [--force]
gitmaster worktree include [<branch>] [--include symlink|reflink] [--dry-run]
```

These subcommands work on the repository that contains the current directory, from the main checkout or from any worktree. Every one can work on other repositories too, as the section Several repositories describes. They are made for shell scripts and key bindings: the result is on the standard output, one item per line, and messages are on the standard error. Worktrees whose folder is gone are left out, because there is nowhere to go.

The exit status is `0` on success, `1` when the work fails and `2` for a wrong command line.

### List worktrees

`list` prints one line per worktree, the main checkout first. `--format` chooses what a line holds:

| Format | A line holds |
|---|---|
| `paths` (the default) | The absolute path of the worktree. |
| `branches` | The branch the worktree has checked out. A detached worktree has no line. |
| `tsv` | The branch, a tab and the path. A detached worktree shows `(detached)` as its branch. |
| `json` | One line with a JSON array that has an object per worktree: `path`, `head`, `branch`, `detached`, `bare`, `is_main`, `locked` and `prunable`. |

`--linked` leaves out the main checkout.

### Find a branch

`branches` prints the names of the branches a worktree can be made for: the local branches, then the branches that exist only on a remote, each once and without the remote's name. `--missing` leaves out the branches that already have a worktree.

`path <branch>` prints the path of the worktree that has `<branch>` checked out. It prints nothing and exits `1` when no worktree has it. Use it to jump to a worktree only when it exists, and `add` to jump to it and create it when it does not.

### Create a worktree

`add <branch>` prints the path of the worktree for `<branch>`, and creates the worktree when there is none. The last line of the output is always the path, so `cd "$(gitmaster worktree add feature/login)"` works whether or not the worktree existed.

- When a worktree has the branch checked out, `add` prints its path and changes nothing.
- When the local branch exists, it is checked out in the new worktree. A `<base>` is ignored, with a note.
- When only a remote has the branch and there is no `<base>`, a local branch is created that tracks it. A branch on `origin` is preferred.
- Otherwise a new branch is created from `<base>`, or from `HEAD` without one.

The new folder must not exist. A `/` in the branch name is written as `-` in the folder name.

The folder comes from the first of these that is set:

| Source | The worktree goes to |
|---|---|
| `--root <dir>` | `<dir>/<repo>/<branch>` |
| The Git key `gitmaster.worktree.root` | `<dir>/<repo>/<branch>` |
| The environment variable `WORKTREES_HOME` | `<dir>/<repo>/<branch>` |
| None | `../<repo>-<branch>`, next to the main checkout |

`<repo>` is the name of the main checkout's folder, so the same branch of two repositories never collides. A leading `~` is the home directory. A relative `--root` is taken from the current directory and a relative key from the main checkout.

### Remove a worktree

`remove <branch>` removes the worktree that has `<branch>` checked out and prints its path. The branch is kept. It follows the rules of the **Remove Worktree** dialog and never deletes a folder by hand: Git does it, so the files the worktree shares through `.worktreeinclude` disappear as links and the files they point to stay in the main checkout.

- It refuses the main checkout, and the worktree that contains the current directory. `--force` does not lift either.
- It refuses a worktree that is locked, has staged, unstaged or untracked files, or has commits that no other local or remote branch has, or that are ahead of its upstream. It says which, and removes nothing. `--force` removes it anyway, with what it discards.
- A worktree whose folder is gone is not listed; clear its record with `git worktree prune`.
- When the worktree sat under a `--root`, `gitmaster.worktree.root` or `WORKTREES_HOME` folder and that `<repo>` folder is now empty, it is removed too.

### Share ignored files: `.worktreeinclude`

A new worktree has only the files Git tracks. Local files that Git ignores, such as `.env` or `node_modules/`, stay behind. A `.worktreeinclude` file at the root of the main checkout lists the ones to bring along, one pattern per line, with the syntax of `.gitignore`: comments, `!` to exclude, `/` to anchor and a trailing `/` for folders.

```gitignore
# .worktreeinclude
.env
.env.local
node_modules/
```

A path is brought along only when a pattern of the file matches it **and** Git ignores it. A tracked file is never touched. Nothing is copied: each path becomes a symbolic link to the file in the main checkout, so every worktree sees one file and no disk space is used. A pattern that matches a folder links the folder with one link. A file pattern such as `.env` also finds `.env` inside an ignored folder, which is then a real folder that holds the links.

`add` does this when it creates the worktree, and prints a line on the standard error for each path: `linked <path>`, `cloned <path>` or `skipped <path>: <why>`. A path that exists in the worktree is skipped, never replaced. A problem here, such as a pattern that does not compile, is a warning: the worktree is still created.

- `--include symlink` is the default.
- `--include reflink` makes a copy-on-write clone instead, with `cp --reflink=always` on Linux and `cp -c` on macOS. The clone takes no space until one side changes, and the two sides are independent afterwards. On a file system that cannot clone, the path is linked instead and the line says so.
- `--include none` shares nothing.
- The Git key `gitmaster.worktree.include` sets the default for the repository or for every repository.

A link to a folder does not match a folder pattern such as `node_modules/`. GitMaster lists linked folders in a marked block of `.git/info/exclude`, which every worktree reads, so they do not appear as untracked.

`include [<branch>]` shares the paths into a worktree that already exists: the one that has `<branch>`, or the current one. It skips what is there, so it is safe to repeat. `--dry-run` prints what it would do and writes nothing. It refuses the main checkout.

### Several repositories

Every worktree subcommand takes `--repo <folder>` to work on a repository other than the current one, from any directory. A relative folder is taken from the current directory, and a leading `~` is the home directory.

```sh
gitmaster worktree add feature/x --repo ~/src/billing    # create it in another repository
gitmaster worktree path feature/x --repo ~/src/app       # find its worktree there
```

`list` and `branches` can read more than one repository. The command never looks for a list of repositories itself: you pass it.

- `--repo <folder>`, more than once.
- `--repos-from <file>` reads one folder per line from the file, or from the standard input when the file is `-`. Blank lines are ignored.

```sh
ls -d ~/src/*/ | gitmaster worktree list --repos-from - --format tsv
gitmaster worktree branches --repos-from repos.txt
```

The other subcommands change or look up one thing, so they take `--repo` once. `--repos-from` or a second `--repo` is a wrong command line.

With a list, the output says which repository a line is from:

| Output | A line holds |
|---|---|
| `list --format paths` | The path, as always. A path belongs to one repository. |
| `list --format branches` | The project, a tab and the branch. |
| `list --format tsv` | The project, the branch and the path, separated by tabs. |
| `list --format json` | The same objects, each with `project` and `repo`, the path of the main checkout. |
| `branches` | The project, the branch and the path of the main checkout, separated by tabs. |

The project is the folder name of the main checkout. A list is more than one `--repo` or any `--repos-from`; with a single `--repo` the output is the one shown above. A repository is read once, however many of its folders the list holds: a linked worktree and its main checkout count as one. A folder in `--repos-from` that is not a repository is skipped, since the list may hold other folders, but a folder named with `--repo` that is not one is an error. A list that is empty, or has no repository, gives no output and does not fall back to the current directory.

A command that works on another repository reads that repository's own Git configuration, so its `gitmaster.worktree.root` and `gitmaster.worktree.include` apply to it, not the current one's.

### Example: a tmux binding

```sh
# pick a branch of any repository in a list; a line holds the project, the branch and the checkout
IFS=$'\t' read -r _ branch repo < <(my-repos | gitmaster worktree branches --repos-from - | fzf --delimiter='\t') &&
  tmux-sessionizer "$(gitmaster worktree add "$branch" --repo "$repo")"
```

## Workspaces

```sh
gitmaster workspace create <name> [--root <dir>] [--branch <branch> --repos-from <file|-> [--base <rev>] [--existing]]
gitmaster workspace add <repo> <branch> [<base>] [--workspace <name|dir>] [--root <dir>] [--as <member>] [--include symlink|reflink|none]
gitmaster workspace list [--format paths|names|tsv|json] [--root <dir>]
gitmaster workspace path <name> [--root <dir>]
gitmaster workspace status [--workspace <name|dir>] [--root <dir>] [--format tsv|json]
gitmaster workspace remove <name> [--root <dir>] [--force]
```

These subcommands create and look after [workspaces](../guide/workspaces.md). They follow the rules of the worktree subcommands: the result is on the standard output, one item per line, messages are on the standard error, and the exit status is `0` on success, `1` when the work fails and `2` for a wrong command line.

A workspace is a folder `<root>/<name>` that holds a Git repository. The root comes from the first of these that is set:

| Source | Root |
|---|---|
| `--root <dir>` | `<dir>` |
| The Git key `gitmaster.workspace.root` | Its value |
| None | `~/workspaces` |

A leading `~` is the home directory, and a relative root is taken from the current directory. A workspace's repository has the Git key `gitmaster.workspace` set to `true` in its own `.git/config`; a folder without it is not a workspace.

`--workspace <name|dir>` chooses the workspace a command works on. A value with a `/` is a folder; any other value is a name under the root. `--root` chooses the root a name is looked up under, as for `create`. Without it, the command works on the workspace that contains the current directory, from its root or from inside a member.

### Create a workspace

`create <name>` creates the folder `<root>/<name>`, makes it a Git repository on a branch `main`, marks it as a workspace and prints its path. The folder must not exist. A name is a single folder name: it has no `/` and does not start with `.` or `-`.

`--branch <branch>` together with `--repos-from <file|->` adds every repository of the list as a member on `<branch>`, the way `add` does. Where a repository does not have the branch yet, it is created, so a workspace for a new change needs no branch made beforehand. The list is read like the one of `gitmaster worktree list`: a folder that is not a repository is skipped, with a note.

- A branch that exists only on a remote is created to track it, even when `--base` is given: a second branch with the same name is never started.
- A new branch starts from `--base <rev>` when it is given, in every repository of the list. Otherwise it starts from the repository's base branch: the local branch that `origin/HEAD` names, else the first of `main`, `master`, `trunk` and `develop`, else the current branch.
- The standard error says, for each member, whether its branch was checked out, created to track a remote branch, or created from a base, and which.
- `--existing` adds only the repositories that already have `<branch>`, as a local branch or on a remote, and skips the others with a note. Use it to find which repositories a change already touches.

Every repository of the list is checked before anything is created. When one of them cannot be added, for example because the branch is checked out in another worktree or two repositories have the same folder name, `create` says which, exits `1` and creates nothing. It also exits `1` and creates nothing when no repository would be added: an empty list, a list with no repository, or, with `--existing`, no repository that has the branch.

### Add a member

`add <repo> <branch>` adds the repository `<repo>` to the workspace as a member checked out on `<branch>`, commits the change in the workspace's repository and prints the member's path.

1. A worktree of `<repo>` is created in the workspace folder, at `<member>`, the folder name of the repository's main checkout unless `--as` names another. The branch is the local branch, else a new local branch that tracks the remote one, else a new branch from `<base>` or, without one, from the repository's base branch, as `create` chooses it. The `.worktreeinclude` paths are shared into it, as `--include` says.
2. The worktree is registered in the workspace's repository as a submodule that follows `<branch>`, with the path of the main checkout as its URL.
3. The workspace's repository commits `.gitmodules` and the member.

`add` refuses a branch that is checked out in another worktree, and says which. It refuses a `<member>` folder that exists, and a repository that is already a member. When the commit fails, for example because Git has no user name, the member stays added and staged, the message says how to commit it, and the exit status is `1`. When registering the submodule fails, the new worktree is removed again.

### List and find workspaces

`list` prints one line per workspace under the root, by name. `--format` chooses what a line holds:

| Format | A line holds |
|---|---|
| `paths` (the default) | The absolute path of the workspace. |
| `names` | The name of the workspace. |
| `tsv` | The name, the number of members and the path, separated by tabs. |
| `json` | One line with a JSON array that has an object per workspace: `name`, `path` and `members`, the number of members. |

`path <name>` prints the path of the workspace `<name>`. It prints nothing and exits `1` when there is none.

### Show a workspace

`status` prints one line per member, in the order of `.gitmodules`. A `tsv` line (the default) holds, separated by tabs:

| Field | Value |
|---|---|
| Member | The member's folder name. |
| Branch | The branch checked out in the member, or `(detached)`. |
| Changes | `dirty` when the member has staged, unstaged or untracked files, else `clean`. |
| New commits | How many commits the member's `HEAD` has that the commit recorded in the workspace does not. |
| Repository | The path of the main checkout the member belongs to. |

`--format json` prints one line with a JSON array that has an object per member: `member`, `branch` (`null` when detached), `detached`, `dirty`, `new_commits`, `recorded`, `head` and `repo`.

A member whose folder is gone prints `(missing)` as its branch, and the command exits `1` after the last line.

### Remove a workspace

`remove <name>` removes every member's worktree from its repository, then the workspace folder, and prints the folder's path. The branches stay in the repositories.

- It refuses a workspace that contains the current directory. `--force` does not lift that.
- It refuses when a member has staged, unstaged or untracked files, when a member is on a detached `HEAD` that holds commits no local or remote branch has, or when the workspace's repository has changes other than the commits its members point to. It says which, and removes nothing. `--force` removes it anyway, with what it discards.
- A member whose folder is gone is pruned from its repository's worktree records.
- It refuses a workspace whose member's repository is gone, even with `--force`, because Git cannot remove that member's worktree.
- Members are removed one at a time. When one fails, the ones before it stay removed, the message says which failed, and the workspace folder is kept.

## Terminal companion

```sh
gitmaster tui [PATH]
```

Runs GitMaster in the terminal, for reading, staging and commenting on the changes next to a coding agent. With no path it works on the repository that contains the current directory; a path is resolved as for the app. It opens no window and needs no display. It runs until you press `q`, then restores the terminal. See [Terminal companion](../guide/terminal-companion.md).

The exit status is `0` after you quit, `1` when the repository cannot be opened or the terminal cannot be used (standard input or output is not a terminal), and `2` for a wrong command line.

## MCP server

```sh
gitmaster mcp [--repo <path>]
```

Runs an MCP server on the standard input and output, for an MCP client to start. It works on the repository that contains the current directory, or on the one at `--repo`. When the current directory is not in a repository and `CLAUDE_PROJECT_DIR` is set, as Claude Code sets it, it uses that directory instead. It exits when the client closes the input. It does not need the app to be open, opens no socket and keeps no state outside the repository's `.git/gitmaster/` directory. See [MCP server](../agents/mcp-server.md).

The exit status is `0` after a normal end, `1` when the repository cannot be opened, and `2` for a wrong command line.

## Check a workflow

```sh
gitmaster workflow check <file> [--json]
```

Compiles the script in the same sandbox the app uses, and checks that it is a module with `execute`, that `M.meta` is valid, and that the trigger and the inputs are. Each problem is printed on its own line as `file:line: message`, and a file with no problem prints `file: ok`. The messages name the valid choices, such as the valid triggers, so you can correct the file from them. A problem with no single line prints as `file: message`.

`--json` prints one object instead: `{"file": "...", "ok": false, "problems": [{"line": 4, "message": "..."}]}`.

The exit status is `0` when the file has no problem, `1` when it has one, and `2` for a wrong command line.

## Check and render an extension

```sh
gitmaster extension check <file> [--json]
gitmaster extension render <file> --slot <slot> [--commit <rev>] [--repo <path>] [--json] [--allow-exec]
```

`check` compiles the extension in the same sandbox the app uses and checks `M.meta` and `M.slots`, with the same output and exit status as [Check a workflow](#check-a-workflow).

`render` runs one slot function for real and prints the document it returns, drawn as the terminal companion draws it, or as JSON with `--json`. It is read-only, like every extension.

- `--slot` names the slot, such as `commit`.
- `--commit <rev>` is the commit given to the `commit` slot; the default is `HEAD`.
- `--repo <path>` names the repository. Without it, the repository that contains the current directory is used.
- `ctx.exec` and `ctx.http.get` run only when the current text of the file is reviewed in the app, or with `--allow-exec`, which you type yourself. Without either, they fail with a message that says so.

The exit status is `0` when the function returned a document or `nil`, `1` when it failed or the file has a problem, and `2` for a wrong command line.

## Dry run a workflow

> **Planned.** In a dry run, `ctx.stack.tree` and `ctx.stack.stale` return an empty list instead of the real stacks.

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

`--harness` names the agent that sent the event. The plugin's hooks set it; an id this version does not know is logged and the event is dropped.

The [agent integration](../agents/agent-integration.md) runs this command from the agent's hooks. You do not run it yourself. It reads one JSON event from the standard input, redacts it, stores it in the thread database of the repository named in the event, and exits. It starts no resident process and opens no window.

The command always exits with status `0` and prints nothing, even when it cannot store the event, so a failure never reaches the agent. A problem is written to `.git/gitmaster/threads.log`. An event that cannot be written at once is kept redacted in `.git/gitmaster/threads-spool/` and stored later.

## Test a redaction pattern

```sh
gitmaster thread redact-test [--text <text>]
```

Reads a sample from the standard input, or from `--text`, applies the `gitmaster-redact` patterns and the other `gitmaster.threads.*` keys that apply in the current directory and prints the result, with a line per match naming the pattern. Nothing is stored. The exit status is `0` when the patterns are valid, `1` when a pattern is not valid (the problem is printed as `pattern <name>: message`), and `2` for a wrong command line.

## List the agents

```sh
gitmaster harness list
```

Prints the id and the name of each agent this version knows, one per line. The agent integration is installed with the agent's own plugin commands, not with GitMaster. See [Agent integration](../agents/agent-integration.md).

`harness` takes no other subcommand. Any other word exits with status `2`.

