---
title: Code health
description: See which files change the most, which quiet files have started to change, which files change together and how widely a file is owned, from the history of the repository, and give an agent the same picture before it edits.
status: planned
order: 65
---

> **Planned.** Code health is not available in the app yet.

When agents write much of the code, the team's understanding of a codebase can fall behind faster than the code itself shows. Code health reads how the code has changed over time, from the history Git already keeps, and shows where that matters: the files that change the most, quiet files that suddenly change a lot, files that always change together, and files that nobody clearly owns.

You see these indicators on the **Health** tab of the **Code** page and as heat dots in the folder tree. An agent reads the same indicators through the MCP server before it edits, so what you see is exactly what the agent is told.

Every indicator is computed from the history of the repository and from your configuration. The same commit and the same configuration always give the same answer. Nothing is predicted, and nothing is scored by a model.

## The indicators

| Indicator | What it measures | Why it matters |
|---|---|---|
| **Heat** | How much a file changed recently. Each commit that changed the file counts, and its weight halves every *half-life* of active days. | Files that keep changing are where problems collect and where a review costs the most. |
| **Trend** | Whether the file changes more or less than before: **rising**, **steady** or **falling**. | A rising file is becoming a hotspot. |
| **Dormancy break** | A file that was quiet for a long time and is now changing a lot. | The people who understood it may be gone, so a change there needs the most care. |
| **Changes together with** | Other files that change in the same commits as this one, with how often they do. | Two files that always change together depend on each other, even when the code does not say so. |
| **Ownership** | How many authors changed the file, the share of its commits by the most frequent author, and how many authors made fewer than 5% of its commits. | A file with many occasional authors and no clear owner has nobody who holds its whole design. |

Files are ranked by heat. A file's **rank** is its place in that order, and its **percentile** says how much of the repository is colder than it.

Ownership never names a person. Code health shows counts and shares by file, never a list of authors and never a ranking of people.

### Active days

Heat does not count calendar time. It counts **active days**: days on which at least one commit changed the files of a scope. A module that changes every day and a module that changes twice a month both cool down after the same number of their own working days, and a series of small commits on one day counts as one day.

Dormancy uses the active days of the whole repository instead. A quiet file has no active days of its own, so its sleep shows only against the rest of the repository.

All days are counted back from the commit you look at, never from today's date. Looking at the same commit tomorrow gives the same answer.

### Heat in detail

The heat of a file is the sum, over every commit in the analysis window that changed it, of `2^(-age / half-life)`, where `age` is how many active days of the file's scope have passed since that commit. With the default half-life of 30 active days, a change made 30 active days ago weighs half as much as a change made today.

The half-life is set in active days. To keep extreme values out, the length it stands for in calendar time is kept between 14 and 365 days: in a scope that is busy every day, 30 active days is about six weeks; in a scope that changes twice a month, it would be more than a year, so it is held at one year.

A file that was renamed keeps its history: commits made under its earlier name count toward the file as it is named now.

### Trend in detail

The trend compares the commits that changed the file in its last half-life of active days with those in the half-life before it. **Rising** means at least half again as many, **falling** means at most two thirds as many, and anything between is **steady**.

### Dormancy break in detail

A file has a dormancy break when, before its recent changes, it went at least 120 active days of the repository without a change, and it then changed on at least 3 of the last 30 active days of the repository.

### Changes together in detail

Two files change together when they appear in the same commits. The degree is the number of commits that changed both, divided by the mean of the number of commits that changed each. A pair is listed when at least 5 commits changed both and the degree is at least 0.3.

Commits that changed more than 30 files are left out of this count: mass renames, formatting sweeps and dependency updates would otherwise tie unrelated files together. To keep the count fast in a large repository, partners are computed for the 1,000 hottest files.

## What is counted

- **The analysis window** is the last three years of commits before the commit you look at, measured by commit date. Older history barely changes heat, and leaving it out keeps a very large repository fast.
- **The commits** are the commits reachable from the commit you look at. A merge commit adds no change of its own: the commits it brought in already count.
- **Lock files** of common package managers are left out, such as `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`, `poetry.lock` and `Gemfile.lock`.
- **Paths you exclude** with `gitmaster.health.exclude`, such as vendored or generated code, are left out.
- **Commits you ignore** with `gitmaster.health.ignoreRev`, such as a formatting sweep, are left out.

See [Configuration](#configuration) for the keys.

## The Health tab

Open the **Code** page and choose its **Health** tab, or choose **Show Code Health** in the command palette.

The tab has three parts:

- **Dormancy breaks**, at the top: each file that has one, with how long it was quiet and how many of the last 30 active days changed it. Click a file to select it.
- **Hotspots**: the files ranked by heat, hottest first. Each row shows the rank, the path, a heat bar, a small activity chart of the last 180 active days of the repository and the trend. The list draws only the rows you can see, so it stays fast with any number of files.
- **The file details**, on the right, for the selected file: its rank and percentile, its activity chart, the files it changes together with and its ownership. **Open File** shows it on the **Files** tab.

Type in the filter box above the list to show only the files under a folder, such as `src/billing`.

### Hotspot map

> **Planned.** The hotspot map is not available in the app yet.

A map of the folder tree shows each file as a box, its size by the size of the file and its color by heat, so the hot areas of a large repository stand out at a glance.

### Changes by agents

> **Planned.** Splitting changes by agent is not available in the app yet.

For a folder, the tab shows how much of the recent change came from agents, matched to their [threads](threads.md), and how much is unattributed, and how soon each kind of change was rewritten. It never says "human": work done in an agent whose sessions are not captured has no thread, so its lines look like anyone else's. The split is always **agent (observed)** against **unattributed**.

## Heat in the folder tree

On the **Files** tab, a file among the hottest tenth of the repository has a heat dot after its name. The dot is brightest for the hottest 1%, then for the hottest 5%, then for the hottest 10%. A folder shows the dot of the hottest file inside it. Hover a dot to read the file's rank.

## Building the index

The first time you open the **Health** tab in a repository, GitMaster reads the history once and keeps what it learns in `.git/gitmaster/health.db`. It reads the last six months first, so the tab can answer within minutes in a very large repository, and then the rest of the analysis window. Until it has finished, the tab says that the answers are partial.

After that, only new commits are read. A commit is read once and never again, whichever branch you look at. The window and `gitmaster mcp` share the index, so each commit is read once whichever of them sees it first.

Reading runs in the background. The window stays responsive, and closing the tab or switching repositories stops the reading; it continues where it stopped the next time.

> **Note.** GitMaster reads a commit-graph file when the repository has one, and never writes one. In a very large repository, running `git commit-graph write --reachable --changed-paths` yourself makes reading the history faster.

## What an agent receives

An agent reads code health through two tools of the [MCP server](../agents/mcp-server.md), described in [MCP tools](../reference/mcp-tools.md):

- `health_context` takes the paths the agent is about to change and returns each file's indicators, plus a list of `constraints`.
- `hotspots_list` lists the hottest files under a folder, a page at a time.

`constraints` turn the indicators into instructions. Each one is a fixed rule over the indicators, never a score the agent is asked to lower:

| When | Constraint |
|---|---|
| The file has a dormancy break | Explain the current behavior of the file back before changing it, and keep the change small. |
| The file is among the hottest 5% | Keep the diff small enough for one reviewer. |
| The file changes together with another file with a degree of at least 0.5 | Review the change together with the other file. |
| The file's most frequent author made less than 20% of its commits, or at least 5 authors made fewer than 5% each | Keep the diff small enough for one reviewer, and explain any change of behavior. |

Each answer carries the commit it was computed at and a `config_hash` of the configuration that applied, so two answers can be compared honestly. Paths and constraints come from the history of the repository, so the answer is marked `"untrusted": true`, like any other text from the repository.

For example, before it changes `src/billing/Invoice.java`, an agent asks:

```json
{ "paths": ["src/billing/Invoice.java"] }
```

and receives:

```json
{
  "rev": "a1b2c3d4e5f6",
  "config_hash": "3fa9e1c2",
  "untrusted": true,
  "complete": true,
  "paths": [{
    "path": "src/billing/Invoice.java",
    "known": true,
    "heat": { "rank": 3, "of": 1240, "percentile": 99.8, "value": 18.4, "trend": "rising" },
    "dormancy_break": null,
    "changes_together": [
      { "path": "src/billing/Ledger.java", "degree": 0.72, "shared_commits": 23 }
    ],
    "ownership": { "authors": 14, "top_author_share": 0.18, "minor_contributors": 9 },
    "constraints": [
      "Hot file (rank 3 of 1240): keep the diff small enough for one reviewer.",
      "Changes together with src/billing/Ledger.java (degree 0.72): review the change together with that file.",
      "No clear owner (top author 18% of commits, 9 occasional authors): keep the diff small enough for one reviewer, and explain any change of behavior."
    ]
  }]
}
```

### The prepare skill

> **Planned.** The prepare skill does not read code health yet.

The `prepare` skill of the agent plugin calls `health_context` for the files a task is likely to touch and puts the constraints in its brief. For a file with a dormancy break, the agent explains the file's current behavior back before it changes it, and can propose that explanation as a [memory](memories.md), so the next session starts from it.

## Configuration

Code health reads its settings from Git's configuration, so a value in `~/.gitconfig` applies to every repository and a value in `.git/config` applies to one. GitMaster never writes them. To share settings with a team without changing the repository, keep them in a file of your own and add it with Git's `include.path`.

```ini
# ~/.gitconfig, or .git/config for one repository
[gitmaster "health"]
    halfLife = 30
    exclude = vendor/
    exclude = **/*.generated.ts
    ignoreRev = 4f3c2a1b9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b

# A scope: a folder with settings of its own
[gitmaster-health "services/billing"]
    halfLife = 15

[gitmaster-health "legacy/core"]
    halfLife = 60
    dormancyGap = 200
```

| Key | Default | Meaning |
|---|---|---|
| `gitmaster.health.halfLife` | `30` | The half-life of heat, in active days. |
| `gitmaster.health.minHalfLifeDays` | `14` | The shortest calendar length the half-life may stand for. |
| `gitmaster.health.maxHalfLifeDays` | `365` | The longest calendar length the half-life may stand for. |
| `gitmaster.health.dormancyGap` | `120` | The active days of the repository without a change that make a file dormant. |
| `gitmaster.health.windowDays` | `1095` | The analysis window, in calendar days before the commit you look at. |
| `gitmaster.health.exclude` | Not set | A path to leave out. A value ending in `/` is a folder; a value with `*`, `?` or `[` is a pattern, where `**` matches any number of folders. Can be set more than once. |
| `gitmaster.health.ignoreRev` | Not set | The full id of a commit to leave out, such as a formatting sweep. Can be set more than once. |
| `gitmaster-health.<path>.halfLife` | The repository's value | The half-life for the files under `<path>`. |
| `gitmaster-health.<path>.dormancyGap` | The repository's value | The dormancy gap for the files under `<path>`. |

A scope's `<path>` is a folder relative to the root of the repository, such as `services/billing`, without a pattern. A file belongs to the scope with the longest `<path>` that contains it, and a file under no scope uses the repository's values. A scope has its own active days: the days on which a commit changed a file under it.

A value that is not a valid number is ignored and the default applies.

## Privacy

Code health measures files, not people. It never shows an author's name, never compares authors and never ranks them. The index holds commit ids, dates, paths, line counts and a hash of each author's address, which is only ever counted.
