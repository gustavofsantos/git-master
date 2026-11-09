---
title: Configuration
description: Where GitMaster keeps its data, the settings and Git configuration keys it reads, the environment variables it uses, and the settings dialog.
status: partial
order: 20
---

GitMaster keeps little configuration of its own. Git options and app preferences both live in Git's configuration files, as `gitmaster.*` keys, so they can be tracked with your dotfiles and overridden per repository. State about a repository stays in that repository's Git directory. GitMaster never writes into the working tree of a repository.

## Where GitMaster stores data

GitMaster keeps its files in three places: your user config directory, your user data directory, and the Git directory of each repository. Nothing is written into a repository's working tree, so no `.gitignore` entry is needed and no file appears as changed.

### User config directory

| Path | What it holds | Status |
|---|---|---|
| `~/.config/gitmaster/workflows/` | Your workflow files. A workflow file holds only its definition, never run state. | |

### User data directory

| Path | What it holds | Status |
|---|---|---|
| `~/.local/share/gitmaster/recent.json` | The recently opened repositories, newest first, up to 10. |  |
| `~/.local/share/gitmaster/gitmaster.db` | Which versions of a workflow script you reviewed, and your automatic approvals. Each is tied to the hash of the script text. | |
| `~/.local/share/gitmaster/workflow-trash/` | Workflow files you delete. They are moved here instead of being removed. | |
| `~/.local/share/gitmaster/contexts.db` | Your [contexts](../guide/contexts.md): their names, nesting and repositories, and the memories attached to them, with a search index. Readable only by you. | Planned |

A repository whose folder no longer exists is dropped from the recent list.

### Inside each repository

GitMaster keeps per-repository state in `.git/gitmaster/`, inside the Git common directory. In a linked worktree, that is the common directory shared by all worktrees of the repository, so every worktree sees the same state. To find it, run `git rev-parse --git-common-dir`.

| Path | What it holds | Status |
|---|---|---|
| `.git/gitmaster/state.db` | Workflow run history: every run and every call it made. | |
| `.git/gitmaster/review.db` | The comments of the [local review](../agents/local-review.md) and their states. Their snapshots are kept in `refs/gitmaster/review/`. | |
| `.git/gitmaster/threads.db` | The [threads](../guide/threads.md) captured from agent sessions in this repository: conversation, tool calls and edits, with a search index. Readable only by you. | |
| `.git/gitmaster/repo-id` | A random id for this clone, written when the repository first joins a [context](../guide/contexts.md), so the membership survives moving the folder. | Planned |
| `.git/gitmaster/threads-spool/`, `threads-tmp/`, `threads.log` | Events waiting to be written when the database was busy, temporary copies of files the agent is about to edit, and the capture log. The log keeps at most 256 KiB. | |
| `.git/gitmaster/restack.json` | The record of the last restack, which **Undo restack** reads to put the branches back. | |
| `refs/gitmaster/` (private refs in `.git`) | Snapshots of the local review and of file moves, and the conflict resolutions of restacks. These refs are not branches, so `git branch` does not list them. | |

The Git command log, the list of Git commands with side effects that the **Git commands** panel shows, is kept in memory only. It keeps the last 1000 commands and is lost when the app closes.

## Preferences

GitMaster keeps its own preferences as `gitmaster.*` keys in Git's configuration. Put your base values in `~/.gitconfig`, so they are tracked with the rest of your dotfiles, and override them in one repository with `git config` or by editing its `.git/config`. Git's rules apply: the system file, then `~/.gitconfig`, then the repository's `.git/config`, then the worktree's config, and each later layer wins. `include` and `includeIf` work too, so a work repository can take a different set from a personal one.

```gitconfig
# ~/.gitconfig
[gitmaster]
	diffLayout = split
[gitmaster "threads"]
	maxResultBytes = 131072
	redactPath = .env
	redactPath = secrets/**
[gitmaster-redact "acme-token"]
	regex = acme_[A-Za-z0-9]{32}
```

```sh
git config --global gitmaster.diffLayout split   # for every repository
git config gitmaster.threads.capture false       # for this repository only
```

When you change a preference in the app, it is written to `~/.gitconfig` (`git config --global`). Only a value that differs from the default is stored, and a value that goes back to its default is removed, so the file holds only what you chose. The app never changes a repository's `.git/config`, so an override you wrote there stays and wins. A repository's preferences are read when its window opens.

| Key | Values | Default | Meaning |
|---|---|---|---|
| `gitmaster.diffLayout` | `unified`, `split` | `unified` | The layout of the diff, set by the **Unified** or **Split** selector. The choice applies to the Working Copy diff and to the commit diff. |
| `gitmaster.authorPictures` | boolean | `false` | Whether author pictures are looked up on Gravatar. |
| `gitmaster.threads.capture` | boolean | `true` | Turns [thread](../guide/threads.md) capture off when `false`. |
| `gitmaster.threads.maxResultBytes` | integer | `65536` | The most bytes of one tool result that are stored. A longer result is cut, and the entry records its original size. |
| `gitmaster.threads.redactBuiltin` | boolean | `true` | Turns the built-in redaction patterns on or off as a group. Leave it on unless you replace them with your own. |
| `gitmaster.threads.redactDisabledBuiltin` | name, can be set more than once | none | A built-in pattern to turn off, for example `jwt`. A name that is not a built-in pattern is ignored with a warning. |
| `gitmaster.threads.redactPath` | glob, can be set more than once | `.env`, `.env.*`, `*.pem`, `id_rsa*` | A glob on repository-relative paths. The content of a matching file is never stored; the path is. |
| `gitmaster-redact.<name>.regex` | regular expression | none | A redaction pattern of your own. `<name>` is the label in `[REDACTED:<name>]`. |
| `gitmaster-redact.<name>.group` | integer | `0` | The capture group that holds the secret. `0` is the whole match. Use a group to keep a readable name, as in `(password=)(\S+)` with `group` set to `2`. |

The agent integration reads these keys each time a hook runs, from the repository the event comes from, so a change applies to the next event. A pattern `<name>` uses letters, digits, `-` and `_`, and it must not repeat a built-in or another pattern. The regular expression has the syntax of Rust's `regex` crate and matches in linear time, so a pattern cannot make the hook hang. A pattern that is not valid pauses capture in every repository until it is fixed. See [Secrets](../guide/threads.md#secrets).

Rules for lists and patterns:

- A key that can be set more than once adds up across the layers, as Git's own lists do. A repository adds its entries to the global ones.
- An empty value clears what the layers below it set. `redactPath =` in a repository removes the global paths; add your own after it. A single `redactPath =` in `~/.gitconfig` means no paths at all.
- A pattern whose `regex` is empty is dropped, so a repository can turn off a global pattern with `[gitmaster-redact "acme-token"]` and `regex =`.
- Git reads a backslash in a value as an escape. Write `\\d` for the regular expression `\d`, or let `git config gitmaster-redact.acme.regex '\d+'` write it for you.
- A value that is not valid, such as `maxResultBytes = lots`, reads as the default.

## Git configuration keys

GitMaster reads Git's own configuration through `git`, so Git's scope rules apply. A key set in `~/.gitconfig` applies to every repository, and a key set in `.git/config` applies to one repository only. The **Scope** column says where a key can be set: **Any** means global or repository.

| Key | Scope | Default | Used for | Status |
|---|---|---|---|---|
| `core.editor` | Any | Not set | The program that **Open in editor** starts. | Planned |
| `merge.conflictStyle` | Any | `zdiff3` | The style of the conflict markers the conflict resolver shows. | Planned |
| `gitmaster.workflowPath` | Any, can be set more than once | Not set | Extra folders to read workflow files from, after your workflows directory. A folder inside the repository's working tree is ignored, with a warning. | |
| `gitmaster.worktree.root` | Any | `../<repo>-<branch>` | The base folder for new worktrees. `gitmaster worktree add` reads it; see [Command line](cli.md#create-a-worktree). The worktree dialog does not read it yet. | Planned |
| `gitmaster.worktree.include` | Any | `symlink` | How `gitmaster worktree add` brings the paths of `.worktreeinclude` into a new worktree: `symlink`, `reflink` or `none`. See [Command line](cli.md#share-ignored-files-worktreeinclude). | |
| `gitmaster.workspace.root` | Any | `~/workspaces` | The base folder for new [workspaces](../guide/workspaces.md). | Planned |
| `branch.<name>.stackParent` | Repository | Not set | The parent branch of a branch in a stack. See [Stacks](../guide/stacks.md). | |
| `branch.<name>.stackParentBase` | Repository | Not set | The commit the parent branch pointed to when this branch was last created or re-anchored. | |

The `branch.<name>.` keys belong to stacks, which is a preview feature. See [Preview features](#preview-features).

## Environment variables

| Variable | Values | Effect |
|---|---|---|
| `GITMASTER_THEME` | `dark`, `light`, `system` | Sets the theme when the app starts. The value is not case sensitive. When it is unset, or set to any other value, the theme is `system`, which follows the appearance of your operating system. |
| `SHELL` | Path to a shell | The shell that GitMaster asks for the PATH. See below. |
| `XDG_CONFIG_HOME` | Absolute path | The base of the user config directory. Defaults to `~/.config`. A relative or empty value is ignored. |
| `XDG_DATA_HOME` | Absolute path | The base of the user data directory. Defaults to `~/.local/share`. A relative or empty value is ignored. |
| `HOME` | Absolute path | The base of the default folders when the XDG variables are not set. The same folders are used on macOS and Linux. |
| `GITMASTER_ENABLE_STACKS` | `1`, `0`, or unset | Turns the stacks preview feature on or off. See [Preview features](#preview-features). |
| `GITMASTER_ENABLE_CONFLICT_RESOLVER` | `1`, `0`, or unset | Turns the conflict resolver preview feature on or off. See [Preview features](#preview-features). |

### Login-shell PATH

A window opened from a desktop launcher, the Finder or the Dock starts with a minimal PATH. Git and the tools your shell profile adds, such as Homebrew's, may then not be found. To avoid this, GitMaster reads the PATH of your login shell once, in the background, when the app starts:

1. GitMaster runs the shell named by `SHELL` as a login, interactive shell that prints its environment.
2. It waits up to 10 seconds. A shell that takes longer is stopped.
3. If the shell prints a PATH, Git and the other programs GitMaster starts after that point run with it. These are the file watcher and the programs that workflows start with `ctx.exec`.

Programs started before the PATH is read use the PATH the app started with. If `SHELL` is not set, the shell does not answer in time, or it prints no PATH, GitMaster keeps the PATH it started with. GitMaster does not change its own environment variables to do this.

## Preview features

Some areas are finished in the code but not yet part of the release. A preview feature is hidden in the release build. Each one has its own environment variable, which turns it on or off when GitMaster starts:

| Variable | Feature | What it shows |
|---|---|---|
| `GITMASTER_ENABLE_STACKS` | Stacks and restack | The by-stack view, the stack actions, restack and the stack marks in the graph. See [Stacks](../guide/stacks.md). |
| `GITMASTER_ENABLE_CONFLICT_RESOLVER` | Conflict resolver | The conflict resolver, which resolves a file's conflicts by hand. See [Conflicts](../guide/conflicts.md). |

The name of the variable is the id of the feature in capitals, with `-` replaced by `_`.

### Values

| Value | Effect |
|---|---|
| `1`, `true`, `on` or `yes` | The feature is on. |
| `0`, `false`, `off` or `no` | The feature is off. |
| Unset, or empty | The default of the build applies. |

The value is not case sensitive, and spaces around it are ignored. A value that is not in this table prints a warning on stderr, and the default of the build applies. A variable that starts with `GITMASTER_ENABLE_` and names no feature prints a warning and has no effect. Each variable changes only its own feature.

### Build defaults

A release build has every preview feature off. A build made with `make build`, `make run` or `make install` has every preview feature on. The variables override these defaults.

### Examples

Turn stacks on in the release build:

```sh
GITMASTER_ENABLE_STACKS=1 gitmaster
```

Turn the conflict resolver off in your own build, to see what users get:

```sh
GITMASTER_ENABLE_CONFLICT_RESOLVER=0 gitmaster
```

Check which preview features are on with `gitmaster --version`. When one or more are on, it prints a second line, `preview features:`, followed by their ids, such as `preview features: stacks`. See [Command line](cli.md#open-a-repository).

## Settings dialog

> **Planned.** The settings dialog is not available in the app yet.

Open the settings dialog with `Ctrl+,` (`⌘,` on macOS), with **Open Settings** in the command palette, or with the gear button at the bottom of the window. The dialog is centered over the window, and the rest of the window is dimmed. Close it with `Esc`, a click outside it, or its close button. The categories are on the left, and the form for the selected category is on the right.

Changes apply as you make them, and there is no **Save** button. A value is saved when you change it, and a text field is saved when it loses focus. A value that is not valid is flagged inline and is not saved. Each change goes to the right place:

- Git options go to Git's configuration. The scope selector sets where they go: **Global** writes to `~/.gitconfig`, and **This Repository** writes to `.git/config`.
- App preferences go to `~/.gitconfig` as `gitmaster.*` keys, whatever the scope selector says.
- Workflow reviews and automatic approvals go to your user data directory.
- Nothing is written to the working tree.

### General

| Field | What it does |
|---|---|
| **Appearance** | Sets the theme to **System**, **Dark** or **Light**. |
| **Typography** | Sets the font families for the interface and for code. Each choice shows a preview. |
| **Recent Repositories** | Lists the recently opened repositories. You can remove one entry or clear the list. |
| **File Watcher** | Sets the maximum number of directories the file watcher follows, and shows whether it has fallen back to refreshing on focus. |

### Git & Identity

| Field | What it does |
|---|---|
| **Scope Selector** | Chooses **Global** or **This Repository** for the fields below. |
| **Author Details** | Sets `user.name` and `user.email`. |
| **Default Branch** | Sets `init.defaultBranch`, the name for new branches and new repositories. |
| **Editor** | Sets `core.editor`, the program that **Open in editor** starts. |
| **Signing** | Shows whether commits are signed (`commit.gpgSign`) and which GPG or SSH key signs them. |

### Worktrees

| Field | What it does |
|---|---|
| **Default Location** | Sets the base folder for new worktrees (`gitmaster.worktree.root`). The default is `../<repo>-<branch>`. |
| **Safety Rules** | Shows the rules for removing worktrees that have unpushed commits, uncommitted changes or a lock. See [Worktrees](../guide/worktrees.md). |

### Diff & Files

| Field | What it does |
|---|---|
| **Algorithm** | Chooses the diff algorithm: `histogram` (the default), `myers` or `patience`. |
| **Context** | Sets the number of context lines around each hunk (`diff.context`). |
| **Display Options** | Ignores whitespace changes by default (`ignore-space-change`), and turns line wrap on or off. |

### Stacks & Restack

| Field | What it does |
|---|---|
| **Restack with Dirty Working Tree** | Sets what restack does when the working copy has uncommitted changes. See [Stacks](../guide/stacks.md). |
| **Push Strategy** | Sets how restacked branches are pushed. The default is push with lease. |

### Conflict Resolver

| Field | What it does |
|---|---|
| **Conflict Style** | Sets `merge.conflictStyle`, the style of the conflict markers. The default is `zdiff3`. |
| **Git rerere** | Turns the reuse of recorded conflict resolutions on or off. See [Conflicts](../guide/conflicts.md). |

### MCP & Agents

The MCP server is started by your agent with `gitmaster mcp`, so it has no switch, socket or token to set. Which tool calls ask for your approval is set in your agent's permissions. See [MCP server](../agents/mcp-server.md#claude-code-permissions).

### Threads

The settings dialog is not available yet, so these fields are edited today in **Threads Settings…**, which has the same fields.

| Field | What it does |
|---|---|
| **Capture Threads** | Turns capture on or off everywhere. |
| **Built-in Patterns** | Lists the built-in redaction patterns, each with a switch. |
| **Your Patterns** | Adds, edits and removes your own patterns. An invalid pattern is flagged inline and pauses capture until it is fixed. |
| **Sensitive Paths** | Edits the list of paths whose content is never stored. |
| **Test** | Shows the result of redacting a sample text with the current patterns. |
| **Result Size** | Sets the most bytes of one tool result that are stored. |

### Reviewers

> **Planned.** The Reviewers fields are not available yet.

| Field | What it does |
|---|---|
| **Reviewers** | Lists your reviewers. Each has instructions, a signal, a direction, a subject and an optional model, kept as a Markdown file in `.git/gitmaster/reviewers/` of the repository or in `reviewers/` of your configuration directory. See [Reviewers and gates](../agents/reviewers.md). |
| **Gates** | Sets which gates apply to a push and to a restack, for all branches or for branches that match a pattern. They are kept in `.git/gitmaster/gates.json` of the repository. |

### Local Review

| Field | What it does |
|---|---|
| **Storage** | Shows where review comments and local snapshots are stored. See [Local review](../agents/local-review.md). |

### Workflows

> **Planned.** There is no settings dialog yet, so the **Automatic Approvals** and **Enabled per Repository** fields are not shown. Until it exists, the switch **Enabled in this repository** is on the Workflows page, and each workflow's grant is in its review line.

| Field | What it does |
|---|---|
| **Directories** | Shows the folders that workflows are read from: your workflows directory and the folders in `gitmaster.workflowPath`. |
| **Enabled per Repository** | Chooses which workflows run in each repository. Your own workflows are on unless you switch them off; the built-in stack workflows are off unless you switch them on. |
| **Automatic Approvals** | Lists the approvals that let a workflow run without asking again. Each approval is tied to one script hash. Use **Revoke** to remove an approval. See [Approvals and security](../workflows/approvals-and-security.md). |
| **Run History** | Keeps the last 200 runs of each workflow in each repository. An output larger than 64 KiB is stored as its hash, its size and its first 4 KiB. |
