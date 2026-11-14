---
title: Extension API
description: The module an extension returns, the slots it can claim, the document it answers with, the read-only ctx it calls, and the sandbox and limits it runs under.
status: planned
order: 45
---

> **Planned.** Extensions are not available yet.

An extension is a Lua script that fills slots of GitMaster's views with a document. For what extensions are and how you use them, see [Extensions](../guide/extensions.md).

## The module

An extension file returns a table with `meta` and `slots`:

```lua
local M = {}

M.meta = {
  name = "pull-request",
  description = "The pull request that brought the commit in",
  api = 1,
  http = { "code.example.com" },
  credentials = { "code.example.com" },
}

M.slots = {}

function M.slots.commit(ctx, commit)
  local r = ctx.http.get(
    "https://code.example.com/api/commits/" .. commit.hash .. "/pulls",
    { format = "json", credential = "code.example.com" }
  )
  if r.status ~= 200 or r.json[1] == nil then
    return nil
  end
  local pr = r.json[1]
  return ui.section("Pull request #" .. pr.number, {
    ui.field("Title", pr.title),
    ui.field("Author", pr.author),
    ui.field("State", pr.merged and ui.badge("merged", "ok") or ui.badge(pr.state)),
    ui.link("Open in browser", { url = pr.url }),
  })
end

return M
```

GitMaster reads `meta` and the names in `slots` when it loads the file, without running any function, so it knows what each extension claims before it runs it.

### `M.meta`

| Field | Required | Meaning |
|---|---|---|
| `name` | Yes | Unique name. Letters, digits and `-`. Shown wherever the extension's output appears. |
| `description` | No | One line, shown on the Extensions page. |
| `api` | Yes | The version of this API the extension is written for: `1`. A version GitMaster does not know is refused with a message. |
| `exec` | No | The programs `ctx.exec` may run, by name, such as `{ "git-tickets" }`. |
| `http` | No | The hosts `ctx.http.get` may reach, exact names, such as `{ "code.example.com" }`. |
| `credentials` | No | The hosts whose Git credential a request may use. Each must also be in `http`. |

### `M.slots`

A table of functions, one per slot the extension claims. Each function receives `ctx` and the focus of the slot, and returns a document or `nil`. `nil` means there is nothing to show: the slot takes no space. An error ends the run, and the slot shows the reason in one line.

## Slots

### `commit`

Shown in the window under the details of the commit selected in **History**, between the native fields and **Files**; in the terminal companion, under the commit's header on the **Diff** screen.

`slots.commit(ctx, commit)` receives:

| Field | Type | Meaning |
|---|---|---|
| `hash` | string | The full hash |
| `short_hash` | string | The abbreviated hash |
| `subject` | string | The first line of the message |
| `body` | string | The rest of the message |
| `trailers` | list of `{ key, value }` | The trailers of the message, such as `Refs` or `Co-authored-by`, in order |
| `author`, `committer` | `{ name, email, time }` | `time` in seconds since the epoch |
| `parents` | list of strings | The parents' hashes |
| `repo` | table | `root` (the repository's folder), `head`, `branch` (or `nil` when detached) and `remotes`, a list of `{ name, url }` |

## The document

A document is one node or a list of nodes. The `ui` helpers build nodes; each returns a plain table, and you may also write the tables yourself, such as `{ type = "badge", text = "merged", tone = "ok" }`.

### Block nodes

| Helper | Table | Draws |
|---|---|---|
| `ui.section(title, children, opts)` | `{ type = "section", title, children, folded }` | A titled section that folds. `opts.folded = true` starts it folded. Sections nest up to three levels. |
| `ui.field(label, value)` | `{ type = "field", label, value }` | A label and its value on one row. Labels of consecutive fields line up. |
| `ui.text(inlines)` | `{ type = "text", inlines }` | A paragraph, wrapped to the width. |
| `ui.list(items)` | `{ type = "list", items }` | One row per item; each item is a string, an inline node or a list of them. |

### Inline nodes

A string is shown as plain text. Where a value is expected, you can give a string, one inline node or a list of them.

| Helper | Table | Draws |
|---|---|---|
| `ui.span(text, tone)` | `{ type = "span", text, tone }` | Text in a tone |
| `ui.badge(text, tone)` | `{ type = "badge", text, tone }` | A short status, such as `merged` or `failing` |
| `ui.code(text)` | `{ type = "code", text }` | A hash, a path or a name, in the code font |
| `ui.link(text, target)` | `{ type = "link", text, target }` | A link; see below |

`tone` is one of `default`, `muted`, `ok`, `warning`, `danger` and `info`. GitMaster turns it into the colors of the theme in use. When it is left out, it is `default`.

`target` is one of:

| Target | Does |
|---|---|
| `{ url = "https://…" }` | Opens the address in your browser. Only `https` and `http`. |
| `{ commit = "<hash or ref>" }` | Shows that commit in the app. |
| `{ file = "<path>", line = n }` | Opens the file, relative to the repository's root, at line `n` (optional). |

### Limits and checks

A document has at most 200 nodes, nests at most 4 levels, and is at most 64 KiB; each string is at most 2,000 characters. A document that breaks a limit, or a node with a wrong field, is not drawn; the slot shows where the problem is, such as `pull-request: commit: [1].children[2].value: expected text or an inline node, found a number`.

Text is shown as text. Markup is not interpreted, and control characters and escape sequences are removed.

## `ctx`

`ctx` is read-only: nothing an extension calls changes the repository.

| Call | Arguments | Returns |
|---|---|---|
| `ctx.exec(argv, opts)` | `argv`: a list of strings; the first must be in `meta.exec`. `opts.stdin`: text for standard input. `opts.timeout`: a duration such as `"10s"`, default `10s`. `opts.format`: `"json"` or not set. | `{ text, stderr, exit_code, truncated }`, and `json` with `format = "json"` |
| `ctx.http.get(url, opts)` | `url`: an `https` address whose host is in `meta.http`. `opts.headers`: a table of header names and values. `opts.timeout`: default `10s`. `opts.format`: `"json"` or not set. `opts.credential`: a host in `meta.credentials`. `opts.credential_scheme`: `"bearer"` (default) or `"basic"`. | `{ status, text, headers, truncated }`, and `json` with `format = "json"` |
| `ctx.log(text)` | A line of text | Nothing. The line appears under **Messages** on the Extensions page. |
| `ctx.now()` | | The time in seconds since the epoch |

### `ctx.exec`

The program runs without a shell, in the repository's root folder: each element of `argv` is one argument. Standard output and standard error are each kept up to 1 MiB; past that, `truncated` is `true`. A program that runs past its timeout is stopped and the call fails with `timed out`. A program not listed in `meta.exec` fails before it starts.

### `ctx.http.get`

- Only `GET`, and only `https`.
- The host of the address must be listed in `meta.http`. A redirect is followed only to a listed host.
- The body is kept up to 1 MiB; past that, `truncated` is `true`.
- A status that is not 2xx is not an error: read `status`. A connection failure or a timeout is an error.
- With `credential = "<host>"`, GitMaster asks Git's credential helper for that host, as `git credential fill` does, without ever prompting you. It sends the password in an `Authorization` header: `Bearer <password>`, or basic authentication with `credential_scheme = "basic"`. If Git has no credential for the host, the request is sent without one. The credential never reaches the script: it is not in the result, in a log line or in an error.

## Sandbox

An extension runs in the same sandbox as a workflow (see [Workflow API](workflow-api.md#sandbox)): no `io`, `os`, `debug`, `require`, `load` or `string.dump`; a fresh, read-only global environment for each run; `ctx.now()` instead of the clock. Its only way out is `ctx`.

## Budgets

| Budget | Value |
|---|---|
| Wall-clock time of a run, including `exec` and `http` calls | 15 s |
| Default timeout of an `exec` or `http` call | 10 s |
| Calls on `ctx` | 50 |

The VM's step, memory and string limits are those of workflows. A run that exceeds a budget ends, and the slot shows which budget it hit.

## When a slot function runs

- When the focus of the slot changes, after you stop moving for 150 ms.
- At most four extensions run at once. A run for a focus you have left is stopped, and a program it started is killed.
- An answer is kept for 5 minutes per extension, text of the extension and focus. Coming back to a focus within that time shows the kept answer without running again; after it, the kept answer shows while a new run starts. **Refresh** runs again now.
- Only reviewed text runs. See [Review before it runs](../guide/extensions.md#review-before-it-runs).
