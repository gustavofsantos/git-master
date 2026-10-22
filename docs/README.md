# Documentation authoring guide

This directory is the **official GitMaster documentation**. It is the source for the documentation website: a bundler (not written yet) turns every page here into a site page. This file is the only file in `docs/` that is not published.

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

`index.md` is the landing page. `sections.yml` gives the title and order of each directory.

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

## Status markers

`status` on a page:

| Value | Meaning |
|---|---|
| `implemented` | Everything on the page works in the app, and the page has no `Planned` marker. |
| `partial` | Part of the page works. Every part that does not has a `Planned` marker. |
| `planned` | Nothing on the page works yet. The page starts with a `Planned` marker. |

The `Planned` marker is a blockquote that starts with exactly `> **Planned.**` followed by one sentence saying what is missing. Put it directly under the heading (or the lead paragraph, for a whole page) it applies to:

```markdown
## Restack

> **Planned.** Restack is not available in the app yet.
```

In a table, mark a row with `Planned` in a status column instead of a blockquote.

The marker text is what the bundler turns into a badge, and what `scripts/docs-status.sh` counts. That script is the equivalent of a diff between desired and actual state: run it to see what is left to build.

```sh
scripts/docs-status.sh          # one line per page, and a total
scripts/docs-status.sh --check  # also fails on a missing field or a status that disagrees with the markers
```

## Bundle contract (for the future bundler)

- Input: every `*.md` under `docs/` except `docs/README.md`.
- Navigation: directories in the order given by `sections.yml`; pages by `order`, then `title`.
- Output per page: the front matter fields, the body, and `planned_count`, the number of `> **Planned.**` markers.
- Links between pages resolve inside the bundle; a broken link fails the build.
