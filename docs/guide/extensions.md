---
title: Extensions
description: Add your own information to GitMaster's views with small Lua scripts, such as the pull request a commit came from, shown the same way in the window and in the terminal companion.
status: planned
order: 112
---

> **Planned.** Extensions are not available yet.

An extension is a small Lua script that adds information to a view of GitMaster. It **claims a slot**, a named place in a view, and when you look at something there, GitMaster asks the extension what to show and draws its answer. The first slot is the commit: an extension can show, under the details of the commit you selected, the pull request that brought it in, the ticket it refers to or the release that shipped it.

An extension only shows. It cannot change your repository, so it never asks for approval while it runs and nothing it does enters the operation log. To automate Git operations, see [Workflows](../workflows/overview.md).

An extension does not draw. It returns a small document made of sections, fields, text, lists, badges and links, and GitMaster draws it with its own theme, folding and keys. The same extension therefore looks right in the window and in the [terminal companion](terminal-companion.md).

## Where extensions live

Each extension is one `.lua` file in your extensions directory:

| System | Folder |
|---|---|
| Linux, macOS | `~/.config/gitmaster/extensions/` (under `$XDG_CONFIG_HOME` when it is set) |

You can add more folders with `gitmaster.extensionPath` (see [Configuration](../reference/configuration.md)). Folders whose names start with a dot are skipped. GitMaster never reads an extension from a repository, and an extension runs in every repository you open; where it has nothing to say, such as a repository with no remote on the code host it reads, it shows nothing.

GitMaster reads the folders when it starts and again whenever a file in them changes, so a saved edit takes effect at once.

## A first extension

This extension shows the ticket a commit refers to, from a `Refs:` trailer of its message, with a link to the tracker. It needs no network.

```lua
local M = {}

M.meta = {
  name = "ticket",
  description = "The ticket a commit refers to",
  api = 1,
}

M.slots = {}

function M.slots.commit(ctx, commit)
  for _, t in ipairs(commit.trailers) do
    if t.key == "Refs" then
      return ui.section("Ticket " .. t.value, {
        ui.link("Open in the tracker", { url = "https://tracker.example.com/browse/" .. t.value }),
      })
    end
  end
  return nil
end

return M
```

Save it as `~/.config/gitmaster/extensions/ticket.lua`, select a commit whose message has `Refs: PAY-218`, and review the extension when GitMaster asks (see [Review before it runs](#review-before-it-runs)). A **Ticket PAY-218** section appears in the commit details.

The full set of calls, nodes and fields is in the [Extension API](../reference/extension-api.md).

## The commit slot

In the window, select a commit in **History**. The commit details show the native fields first, then one section for each extension that has something to say, then **Files**. In the terminal companion, open a commit (`Enter` on a commit row) and the sections appear under the commit's header on the **Diff** screen.

- The native details never wait for an extension. While an extension works, a muted line with its name stands in for it.
- When you move through commits quickly, GitMaster waits until you stop before it asks, and it stops the work for a commit you have already left.
- An answer is kept for a few minutes, so going back to a commit shows it at once. **Refresh**, at the end of an extension's sections, asks again now.
- A section folds like any other: click its title in the window, `Tab` in the companion. It stays folded on the next commit.
- If an extension fails, its place shows one line with the reason, such as `ticket: timed out after 15 s`. The rest of the view is unaffected.

### Links

A link in an extension's answer does one of three things, and you always click it yourself:

- Opens a web address (`https` or `http`) in your browser.
- Shows another commit in the app.
- Opens a file at a line in the app's viewer, or on the **File** screen in the companion.

## Reach the network

An extension that needs data from a server has two ways to get it, and must declare either one in its `meta`:

- **Run a program you already have**, such as the command-line client of your code host, which is already logged in. The program must be listed in `meta.exec`. It runs without a shell, in the repository's root folder.
- **Make an HTTPS request** from GitMaster itself. The host must be listed in `meta.http`. Only reads (`GET`) are allowed.

A request can use a credential you already gave Git for a host: list the host in `meta.credentials` and GitMaster asks Git's credential helper for it and adds it to the request. The extension never sees the credential, so it cannot copy it anywhere.

## Review before it runs

GitMaster runs an extension only after you have reviewed its current text. A new extension, or one that changed since you reviewed it, by you or by an agent, does not run. Its slot shows one line instead:

> **ticket is not reviewed.** **Review…**

**Review…** shows the script and what it may reach: the programs it may run, the hosts it may contact and the credentials it may use. **Run This Version** marks this text as reviewed and runs it. Any later edit returns it to not reviewed.

What an extension returns is treated as data. GitMaster shows its text as plain text: it does not follow markup, and in the terminal it removes control characters and escape sequences, so an extension cannot change how your terminal behaves.

## The Extensions page

Open **Extensions** from the palette in the window, or press `Space x` in the companion. It lists every extension with:

- Its state: **ok**, **error**, **not reviewed** or **off**.
- Its file, the slots it claims, and the programs, hosts and credentials it may use.
- Its last run: what it was asked about, how long it took and how it ended.
- Any problem found when the file was read, with the line, such as `ticket.lua:12: slots.commit must be a function`.

From the page you can **Review** an extension, **Turn Off** or **Turn On** an extension in all repositories, **Open in Editor** and **Copy Path**.

**Messages** lists the recent runs of all extensions, newest first, with the lines each wrote with `ctx.log`. It is kept while the app runs and is not saved.

## Write and test an extension from the command line

You, or an agent, can check an extension and see its answer without the app:

```sh
gitmaster extension check ~/.config/gitmaster/extensions/ticket.lua
gitmaster extension render ~/.config/gitmaster/extensions/ticket.lua --slot commit --commit HEAD
```

`render` prints the document as the companion would draw it, or as JSON with `--json`. See [Command line](../reference/cli.md#check-and-render-an-extension).

GitMaster also writes the API reference and type definitions for the Lua language server into `~/.config/gitmaster/extensions/.api/`, so your editor completes `ui` and `ctx`.
