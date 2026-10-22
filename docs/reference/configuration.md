---
title: Configuration
description: Where GitMaster keeps its data, the settings and Git configuration keys it reads, the environment variables it uses, and the settings dialog.
status: partial
order: 20
---

GitMaster keeps little configuration of its own. Git options stay in Git's configuration files, app preferences go to a small JSON file in your user config directory, and state about a repository stays in that repository's Git directory. GitMaster never writes into the working tree of a repository.

## Where GitMaster stores data

GitMaster keeps its files in three places: your user config directory, your user data directory, and the Git directory of each repository. Nothing is written into a repository's working tree, so no `.gitignore` entry is needed and no file appears as changed.

### User config directory

| Path | What it holds | Status |
|---|---|---|
| `~/.config/gitmaster/settings.json` (Linux) | App preferences. See the `settings.json` section below. | |
| `~/Library/Application Support/gitmaster/settings.json` (macOS) | App preferences, on macOS. | |
| `~/.config/gitmaster/workflows/` (Linux), `~/Library/Application Support/gitmaster/workflows/` (macOS) | Your workflow files. A workflow file holds only its definition, never run state. | |

### User data directory

| Path | What it holds | Status |
|---|---|---|
| `~/.local/share/gitmaster/recent.json` (Linux), `~/Library/Application Support/gitmaster/recent.json` (macOS) | The recently opened repositories, newest first, up to 10. |  |
| `~/.local/share/gitmaster/hook-approvals.json` (Linux), `~/Library/Application Support/gitmaster/hook-approvals.json` (macOS) | Your approvals of repository hooks. Each approval covers one repository and one hash of the hook command. |  |
| `~/.local/share/gitmaster/gitmaster.db` (Linux), `~/Library/Application Support/gitmaster/gitmaster.db` (macOS) | Which versions of a workflow script you reviewed, and your automatic approvals. Each is tied to the hash of the script text. | |
| `~/.local/share/gitmaster/workflow-trash/` (Linux), `~/Library/Application Support/gitmaster/workflow-trash/` (macOS) | Workflow files you delete. They are moved here instead of being removed. | Planned |

A repository whose folder no longer exists is dropped from the recent list.

### Inside each repository

GitMaster keeps per-repository state in `.git/gitmaster/`, inside the Git common directory. In a linked worktree, that is the common directory shared by all worktrees of the repository, so every worktree sees the same state. To find it, run `git rev-parse --git-common-dir`.

| Path | What it holds | Status |
|---|---|---|
| `.git/gitmaster/state.db` | Workflow run history: every run and every call it made. | |
| `.git/gitmaster/` (local review data) | Comments and snapshots of the local review. | Planned |
| `.git/gitmaster/symbols.db` | The symbol index used by code navigation. | Planned |
| Private refs inside `.git` | Snapshots taken before a restack, which an undo restores. | Planned |

The Git command log, the list of Git commands with side effects that the **Git commands** panel shows, is kept in memory only. It keeps the last 1000 commands and is lost when the app closes.

## settings.json

GitMaster keeps its own preferences in `settings.json` in the user config directory. The app creates the file when a setting first changes and rewrites it when a value changes. If you edit the file by hand, keep it a valid JSON object.

| Key | Type | Values | Default | Meaning |
|---|---|---|---|---|
| `diff_layout` | string | `unified`, `split` | `unified` | The layout of the diff, set by the **Unified** or **Split** selector. The choice applies to the Working Copy diff and to the commit diff. |
| `branch_scope` | string | `mine`, `all` | Not set | The scope of the sidebar's branch filter, **Mine** or **All**. While it is not set, the filter shows **Mine** when Git has a `user.email` and **All** when it does not. |

```json
{
  "diff_layout": "split",
  "branch_scope": "all"
}
```

The app applies these rules when it reads and writes the file:

- A missing file, a missing key, or a value it does not recognize reads as the default.
- A file that is not a JSON object reads as the defaults.
- A save keeps every key the app does not own.
- A save over a file that is not valid JSON replaces it with a valid file.

## Git configuration keys

GitMaster reads Git's own configuration through `git`, so Git's scope rules apply. A key set in `~/.gitconfig` applies to every repository, and a key set in `.git/config` applies to one repository only. The **Scope** column says where a key can be set: **Any** means global or repository.

| Key | Scope | Default | Used for | Status |
|---|---|---|---|---|
| `user.email` | Any | Not set | Decides the default of the branch filter. With an email set, the sidebar starts on **Mine**, which shows your current branch and the branches whose latest commit is yours. The match ignores case. | |
| `core.editor` | Any | Not set | The program that **Open in editor** starts. | Planned |
| `merge.conflictStyle` | Any | `zdiff3` | The style of the conflict markers the conflict resolver shows. | Planned |
| `gitmaster.workflowPath` | Any, can be set more than once | Not set | Extra folders to read workflow files from, after your workflows directory. A folder inside the repository's working tree is refused. | |
| `gitmaster.worktree.root` | Any | `../<repo>-<branch>` | The base folder for new worktrees. See [Worktrees](../guide/worktrees.md). | Planned |
| `branch.<name>.stackParent` | Repository | Not set | The parent branch of a branch in a stack. See [Stacks](../guide/stacks.md). | Planned |
| `branch.<name>.stackParentBase` | Repository | Not set | The commit the parent branch pointed to when this branch was last created or re-anchored. | Planned |

## Environment variables

| Variable | Values | Effect |
|---|---|---|
| `GITMASTER_THEME` | `dark`, `light`, `system` | Sets the theme when the app starts. The value is not case sensitive. When it is unset, or set to any other value, the theme is `system`, which follows the appearance of your operating system. |
| `SHELL` | Path to a shell | The shell that GitMaster asks for the PATH. See below. |
| `XDG_CONFIG_HOME` | Absolute path | The base of the user config directory on Linux. Defaults to `~/.config`. A relative or empty value is ignored. |
| `XDG_DATA_HOME` | Absolute path | The base of the user data directory on Linux. Defaults to `~/.local/share`. A relative or empty value is ignored. |
| `HOME` | Absolute path | The base of the default folders when the XDG variables are not set. On macOS, the folders are always under `Library/Application Support` in your home directory. |

### Login-shell PATH

A window opened from a desktop launcher, the Finder or the Dock starts with a minimal PATH. Git and the tools your shell profile adds, such as Homebrew's, may then not be found. To avoid this, GitMaster reads the PATH of your login shell once, in the background, when the app starts:

1. GitMaster runs the shell named by `SHELL` as a login, interactive shell that prints its environment.
2. It waits up to 10 seconds. A shell that takes longer is stopped.
3. If the shell prints a PATH, Git and the other programs GitMaster starts after that point run with it. These are the file watcher and the steps of hooks.

Programs started before the PATH is read use the PATH the app started with. If `SHELL` is not set, the shell does not answer in time, or it prints no PATH, GitMaster keeps the PATH it started with. GitMaster does not change its own environment variables to do this.

## Settings dialog

> **Planned.** The settings dialog is not available in the app yet.

Open the settings dialog with `Ctrl+,` (`⌘,` on macOS), with **Open Settings** in the command palette, or with the gear button at the bottom of the window. The dialog is centered over the window, and the rest of the window is dimmed. Close it with `Esc`, a click outside it, or its close button. The categories are on the left, and the form for the selected category is on the right.

Changes apply as you make them, and there is no **Save** button. A value is saved when you change it, and a text field is saved when it loses focus. A value that is not valid is flagged inline and is not saved. Each change goes to the right place:

- Git options go to Git's configuration. The scope selector sets where they go: **Global** writes to `~/.gitconfig`, and **This Repository** writes to `.git/config`.
- App preferences go to `settings.json`.
- Approvals of hooks and workflows go to your user data directory.
- Nothing is written to the working tree.

### General

| Field | What it does |
|---|---|
| **Appearance** | Sets the theme to **System**, **Dark** or **Light**. Saved in `settings.json`. |
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

| Field | What it does |
|---|---|
| **Server** | Turns the MCP server on or off. |
| **Socket Path** | Shows and sets the path of the local socket the MCP server listens on. |
| **Tokens** | Manages the tokens that agents use to connect to the server. |
| **Agent Policy** | Chooses which agent actions need your approval. See [MCP server](../agents/mcp-server.md). |

### Local Review

| Field | What it does |
|---|---|
| **Storage** | Shows where review comments and local snapshots are stored. See [Local review](../agents/local-review.md). |

### Workflows

| Field | What it does |
|---|---|
| **Directories** | Shows the folders that workflows are read from: your workflows directory and the folders in `gitmaster.workflowPath`. |
| **Enabled per Repository** | Chooses which workflows run in each repository. |
| **Automatic Approvals** | Lists the approvals that let a workflow run without asking again. Each approval is tied to one script hash. Use **Revoke** to remove an approval. See [Approvals and security](../workflows/approvals-and-security.md). |
| **Run History** | Keeps the last 200 runs of each workflow in each repository. An output larger than 64 KiB is stored as its hash, its size and its first 4 KiB. |
