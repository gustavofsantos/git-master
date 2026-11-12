---
title: Workflows overview
description: What a workflow is, where its file lives, how you run one, and how the Workflows page shows what it did.
status: partial
order: 10
---

A workflow is a small script that automates a Git task in GitMaster. You write it in Lua, and it acts only through a `ctx` object, so it can do what GitMaster can do and nothing else. This page explains the model, how you run a workflow, and how to read what it did. To write one, read [Writing workflows](writing-workflows.md).

## What a workflow is

A workflow is one Lua file ending in `.lua`. The file returns a module with two parts:

- `M.meta`, the metadata: the name, a description, when the workflow runs, and the values it asks you for. GitMaster reads it without running any logic.
- `M.execute(ctx, input)`, the function that runs when the workflow runs.

A script has no shell, no file access and no network access of its own. Every action with an effect goes through `ctx`:

- `ctx.exec` starts a program that you already have installed. It is the only way a script reaches outside GitMaster. The program gets its arguments as a list, never through a shell.
- The other `ctx` calls are GitMaster actions, the same ones the menus, the command palette and MCP use. Each action has a level: **read** looks at the repository, **propose** suggests a change for you to decide on, and **apply** changes the repository.

`input` holds the values you entered, the payload of the trigger, and a read-only snapshot of the repository. The script can read these values but cannot change them.

Workflows replace GitMaster's own hooks: the points where a script runs inside an operation are phases of a workflow. Git's own hooks, such as `pre-commit`, still belong to Git.

## What workflows are for

- **Commit message suggestions.** A command-line LLM that you already use writes a message from the staged diff, and the commit composer shows it as a suggestion. You decide whether to use it.
- **Branch names from a ticket.** You enter a ticket key, a program builds the branch name, you confirm it, and GitMaster creates and checks out the branch.
- **Gates before an operation.** A workflow runs before an operation, such as a restack, and can block it with a reason. For example, a restack on `main` can be forbidden.
- **Automatic stack policies.** A workflow reacts to a stack event, such as a parent branch receiving new commits, and restacks the branches above it.

> **Note.** Stacks, restack and their workflow phases are a preview feature. Turn them on with `GITMASTER_ENABLE_STACKS=1`. See [Stacks](../guide/stacks.md) and [Preview features](../reference/configuration.md#preview-features).

## Where workflow files live

GitMaster reads workflow files from these places:

| Location | Path |
|---|---|
| Your configuration directory | `~/.config/gitmaster/workflows/`, or under `$XDG_CONFIG_HOME` when that is set |
| Extra directories | Each folder listed in the Git config key `gitmaster.workflowPath` |

To add an extra folder, for example a folder of dotfiles, run:

```sh
git config --global --add gitmaster.workflowPath <folder>
```

The key takes several values. To apply a folder only to repositories in one directory tree, set the key inside an `includeIf` section of your Git configuration.

These rules apply:

- Nothing is read from the working tree of a repository. The one folder GitMaster reads inside a repository is `.git/gitmaster/workflows/`, which every worktree of the repository shares. A `gitmaster.workflowPath` that points inside the working tree of the open repository is ignored, and GitMaster shows a warning.
- Files in directories whose names start with a dot are not read as workflows. GitMaster writes generated reference files for editors and agents into a `.api` directory in the workflows folder, and a `.luarc.json` file beside it for the Lua language server. Those files are not workflows.
- A `.toml` file in a workflows directory is ignored, and GitMaster shows a warning for it.
- A workflow file holds only the definition. Whether a workflow is enabled for a repository, and your approvals, are kept by GitMaster, not in the file.
- Your own workflows run in every repository. To keep one out of a repository, switch off **Enabled in this repository** on the Workflows page. The choice is kept in that repository's `.git/gitmaster/` folder, never in the working tree. The built-in stack workflows work the other way round: they are off until you switch them on for a repository.
- Nothing is written into the working tree of a repository.

## Running a workflow

You can start a workflow in three ways:

1. **Command palette.** Press `Ctrl+K` (`⌘K` on macOS), choose **Run Workflow…**, and pick the workflow. If the workflow declares inputs, a dialog asks for their values.
2. **A trigger.** A workflow can run by itself when an operation starts or ends, a repository opens, or a ref changes. See [Triggers and phases](triggers-and-phases.md).
3. **A key binding.** A manual workflow can declare a keyboard shortcut with `key` in its `trigger` table. If the key clashes with a built-in shortcut, the Script tab warns you.

> **Planned.** Runs started by an agent are not available in the app yet. An agent connected through MCP will be able to list workflows and run them, and the apply actions of such a run will always wait for your approval. See [AI agents](ai-agents.md).

### Review a version before its first run

The first time you run a version of a script, GitMaster shows you the script and asks **Run this version?**. Confirming marks that exact text as reviewed. Any edit, made by you or by an agent, makes the version unreviewed again, and GitMaster asks once more.

Confirming is also the moment you can allow event-triggered apply actions to run without asking. That choice applies only to the version you reviewed. See [Approvals and security](approvals-and-security.md).

A workflow that runs on a trigger or in a phase never passes through **Run**, so you review it on the Workflows page: **Review** shows the script and asks the same question, with the choice of automatic approval for its event runs. When such a workflow wants to run before you reviewed it, a notice says so, with **Review**.

### Dry run

A dry run executes a script against your real repository without changing it. Reads run for real, except the stack reads, `ctx.stack.tree` and `ctx.stack.stale`, and the review reads, `ctx.scorecards` and `ctx.change`, which return an empty result. Actions that propose, apply or run `exec` are recorded with the values they would receive, but they do not run, and `exec` never runs in a dry run. The dry run lists the calls in order. A value that comes after a simulated value is an estimate, and GitMaster marks it as one.

In the **Script** tab, **Dry Run** walks the file as it is on disk. From the command line, `gitmaster workflow dry-run <file>` does the same. See [CLI](../reference/cli.md).

## The Workflows page

The Workflows page lists your workflows on the left, with the built-in stack workflows under **Built-in**, and shows the selected one on the right. A built-in workflow has no file: **Copy to Edit** makes a file of it that you can change. Under the name of each workflow, **Enabled in this repository** switches it on or off in this repository. The right side has two tabs:

| Tab | What it shows |
|---|---|
| **Script** | The workflow file as it is on disk, read only. You edit the file in your own editor: **Open in Editor** starts it on the file, and **Copy Path** copies the path. The tab shows each change a moment after you save it, with the problems of the file and the line of each one. **Dry Run** walks the file. Each change makes the version unreviewed. See [Edit in your own editor](writing-workflows.md#edit-in-your-own-editor). |
| **Runs** | One entry per run of this workflow in this repository. See [Runs](#runs). |

**New from Template…** lists the built-in templates, and choosing one creates a workflow file from it, ready to open in your editor. Deleting a workflow moves its file to the trash, after you confirm.

Press **Run** to start the open workflow.

### Colors in the Script tab

The Script tab colors keywords, strings, comments and the rest of the code with the same twelve colors as the file viewer, in the light and the dark theme. See [Syntax highlighting](../guide/code-navigation.md#syntax-highlighting).

Cancel and approve work from the command palette:

- **Cancel Workflow Run** stops a run at any time, even while the script waits for you or waits for a program to finish.
- **Approve Workflow Step** and **Reject Workflow Step** answer a run that waits for your approval.

The Workflows page offers the same answers for the open workflow: **Approve** and **Reject** beside a run that waits for your approval, and **Cancel run** while a run is active.

## Runs

The **Runs** tab shows what a run actually did. GitMaster records each call the script makes to `ctx` as it happens, so the record shows what the run did, not what the script reports about itself.

> **Planned.** Runs started by an agent are not available yet.

Each run has a header with:

- its status: Running, Waiting, Awaiting approval, Done, Failed, Blocked, Cancelled or Interrupted. Cancelling a run ends it as Cancelled;
- who started it: `human` for a manual run, `event:<name>` for a trigger, or `agent:<client>` for an agent over MCP;
- its start and end times, and the reason it ended.

Below the header, the run lists one entry per `ctx` call, in order:

- the function called and its arguments, with values resolved;
- the result;
- the outcome, such as done or conflict;
- the duration.

The run also shows the inputs, the trigger and the repository snapshot as they were when it started. A large staged diff appears shortened. Messages the script sends with `ctx.notify` are marked as coming from the script, so you can tell them apart from GitMaster's own records. Each run keeps the text of the script it ran, even after the file changes.

GitMaster keeps the last 200 runs of each workflow in each repository. Large outputs are shortened.

## Runs are not resumed

Runs live only while the app is open. Closing GitMaster ends any run in progress. The next time you open the app, a run that did not finish shows as **interrupted**, and GitMaster does not continue it. An action that had started but has no recorded result also shows as interrupted, and it never runs again. To run the workflow again, start it again.

Waits for an event, and their timeouts, also exist only while the app is open.

## Related pages

- [Writing workflows](writing-workflows.md): the module shape, the metadata, the `ctx` calls, and worked examples.
- [Triggers and phases](triggers-and-phases.md): the events, operations and phases a workflow can run on.
- [Approvals and security](approvals-and-security.md): review, approvals and the sandbox.
- [AI agents](ai-agents.md): how an agent lists and runs workflows through MCP.
- [Workflow API](../reference/workflow-api.md): every `ctx` function, with its arguments and outcomes.
- [CLI](../reference/cli.md): the `gitmaster workflow` commands.
- [Configuration](../reference/configuration.md): the `gitmaster.workflowPath` key and other settings.
