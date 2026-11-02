# Documentation authoring guide

This directory is the **official GitMaster documentation**. It is the source of the documentation website: `tools/docsite` turns every page here into a static site (see [The site](#the-site)). This file and `sections.toml` are the only files in `docs/` that are not published as pages.

## Desired state, not history

`docs/` describes the product **as it should be at launch**, in the present tense, the way a Kubernetes manifest describes the state you want. The code converges on the docs, not the other way around:

1. To change behavior, change the page first.
2. Mark what the app does not do yet (below).
3. Implement until the marker can be removed.

Before go-live, every page is `status: implemented`. Anything that will not ship at launch is removed from the docs, not left marked.

The docs therefore never contain history, dates, slice names (`WF3`, `N4`), decision numbers (`D75`), "currently", "for now" or "in a future version". Why a choice was made lives in [`design/decisions.md`](../design/decisions.md); in what order the work happens lives in [`design/roadmap.md`](../design/roadmap.md).

## Layout

| Directory | Content |
|---|---|
| `getting-started/` | Install, first run |
| `concepts/` | Principles, trust model |
| `guide/` | One page per area of the app |
| `workflows/` | Automating GitMaster with scripts |
| `agents/` | MCP server and the local review loop |
| `reference/` | Keyboard shortcuts, configuration, CLI, workflow API, MCP tools |

`index.md` is the landing page. `sections.toml` gives the title and order of each directory.

## Page format

Every page starts with this front matter and nothing else before it:

```yaml
---
title: Working Copy
description: One sentence that says what the page covers.
status: implemented
order: 10
---
```

| Field | Rule |
|---|---|
| `title` | Short, sentence case. The site renders it as the page heading, so the body has **no H1**. |
| `description` | One sentence, used for search and link previews. |
| `status` | `implemented`, `partial` or `planned` (see below). |
| `order` | Integer. Pages in a directory sort by `order`, then by title. |
| `preview` | Optional. The id of the preview feature the page describes (`stacks`, `conflict-resolver`). The site marks the page Preview and says how to turn the feature on. |

Body rules:

- Start with a short lead paragraph, then `##` sections. Do not skip heading levels.
- Address the reader as "you". Short sentences, imperative for steps ("Open the palette").
- Write UI labels exactly as the app shows them, in **bold**: **Stage Chunk**.
- Keyboard shortcuts in code style: `Ctrl+K`. When the macOS key differs, add it once in parentheses: `Ctrl+K` (`⌘K` on macOS). The reference page [`reference/keyboard-shortcuts.md`](reference/keyboard-shortcuts.md) is the full table.
- Links to other pages are relative and keep the `.md` extension: `[Stacks](stacks.md)`, `[Workflow API](../reference/workflow-api.md)`.
- Every code block has a language tag (`lua`, `sh`, `ini`, `json`, `text`).
- Callouts are blockquotes that start with a bold label: `> **Note.**`, `> **Warning.**`. Do not use other markup.
- American English. No emoji. No marketing language.
- Never name proprietary products (other Git clients, IDEs, hosted services) in the docs. Open source projects and libraries may be named.
- Everything the docs promise must be something the project intends to ship. Do not describe behavior that is not decided.

## Preview features

Some areas are still being finished. The release build hides them. The `make` targets build them in, and the `GITMASTER_PREVIEW` variable turns them on or off when the app starts (`all`, `none`, or a comma list of ids). A preview area is marked on its pages with `preview: <id>` in the front matter. The site adds a **Preview** badge and a note under the title that names the variable that turns the feature on, `GITMASTER_PREVIEW=<id>`, and the Availability page lists the preview pages in their own section. The status of the page still says what works, as for any other page. The ids are the ones the app lists; `docsite check` rejects any other value.

## Status markers

`status` on a page:

| Value | Meaning |
|---|---|
| `implemented` | Everything on the page works in the app, and the page has no `Planned` marker. |
| `partial` | Part of the page works. Every part that does not has a `Planned` marker. |
| `planned` | Nothing on the page works yet. The page starts with a `Planned` marker. |

The `Planned` marker is a blockquote that starts with exactly `> **Planned.**` followed by one sentence saying what is missing. Put it directly under the heading it applies to. For a whole page, put it immediately after the front matter:

```markdown
## Restack

> **Planned.** Restack is not available in the app yet.
```

In a table, mark a row with `Planned` in a status column instead of a blockquote.

The marker text is what the site turns into a badge and lists on its **Availability** page, and what `docsite status` counts. That command is the equivalent of a diff between desired and actual state: run it to see what is left to build.

```sh
make docs-status   # one line per page, and a total
make docs          # check the pages against the rules in this guide (the CI runs it)
```

## The site

`tools/docsite` is a small Rust program (its own Cargo workspace; it is not part of the app). It reads `docs/` and writes a static website that can be deployed anywhere: plain HTML, CSS and JavaScript, with relative links, no external requests and no server-side rewrites. It also opens straight from disk.

```sh
make docs-site                       # writes build/site
python3 -m http.server -d build/site # preview at http://localhost:8000
```

The program has three commands (`cargo run --manifest-path tools/docsite/Cargo.toml -- help`):

| Command | What it does |
|---|---|
| `docsite check [--strict]` | Enforces the rules of this guide. Errors: bad or missing front matter, status that disagrees with the markers, a planned page that does not start with its marker, an H1, a skipped heading level, a code block without a language, a link that does not resolve (page or heading), an absolute link. Warnings: history words and decision numbers in the prose, repeated `order`. `--strict` fails on warnings too. |
| `docsite status` | Lists every page with its status and the number of `Planned` markers. |
| `docsite build [--out DIR] [--base-url URL] [--version LABEL]` | Checks, then writes the site (default `build/site`). `--base-url` adds canonical links, `sitemap.xml` and `robots.txt`; leave it out for a site that may live at any URL. `--version` is shown in the footer. It refuses to replace a directory it did not write. |

What the site has: a sidebar built from `sections.toml` and each page's `order`, an "On this page" outline, previous and next links, search over all pages (press `/`), light and dark themes that follow the system and can be switched, syntax highlighting, copy buttons on code blocks, a status badge on each page that is not fully available, and an **Availability** page that lists every `Planned` marker by page. Pages are written to `build/site` with the same path as the source and an `.html` extension: `docs/guide/branches.md` becomes `guide/branches.html`.

Images and other files under `docs/` (anything that is not Markdown) are copied to the site at the same path. Link to them with a relative path.
