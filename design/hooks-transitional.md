# Hooks (transitional)

GitMaster hooks are replaced by workflows (D75, D93). The `prepare-commit` hook keeps working until script workflows reach parity; then GitMaster stops reading the `[gitmaster "<hook>"]` sections and the 'Convert hooks to workflows' action writes equivalent scripts. Native Git hooks always stay with Git (D18, D29).

Last updated: 2026-10-19. Status of each decision in `design/decisions.md`.

This document describes the current model, which stays in effect until the migration in WF3. The new design is in design/workflow-scripts.md.

## Central idea

Events of the app itself (not Git's native hooks, which stay with Git: D29) trigger scripts configured in `.gitconfig`. The model is inspired by agent event hooks, with a versioned input and output schema per event (D04). Execution is asynchronous (D05), with a correlation id (D06) and the response on stdout (D07).

## Classes (proposed, D16)

| Class | What the hook can do |
|---|---|
| Gate | veto the action |
| Transform | rewrite one field of the event |
| Observe | only react afterwards |

Gates and observers run in parallel (the result of gates is an AND, so order does not matter). Transforms run in sequence, in the order of the resolved config, and each one receives the output of the previous one.

## Initial catalog

| Event | Class | Effect |
|---|---|---|
| `pre-push` | gate | out of v1: the name is native to Git (D29) |
| `pre-restack` | gate | receives the plan; vetoes |
| `prepare-commit` | transform | rewrites the message (slice 2) |
| `prepare-branch-name` | transform | rewrites the name (e.g. an issue-tracker key) |
| `resolve-conflict` | transform | proposes a resolution for one hunk |
| `post-commit` | observe | out of v1: the name is native to Git (D29) |
| `post-push` | observe | out of v1: the name is native to Git (D29) |
| `post-restack` | observe | |

## Configuration (proposed)

One hook per subsection, so each can be named and turned off. The contract is always JSON (D30); there is no `mode`.

```ini
[gitmaster "prepare-commit.llm"]
    command = ~/bin/suggest-commit-message
    match   = branch:feat/*
    timeout = 30s

[gitmaster "post-restack.testes"]
    command = cargo test --quiet
```

The execution order is the order in which the sections appear in the resolved config (including `includeIf`). Inspect it with `git config --list --show-origin`.

### Trust (D17, decided)

A hook in a repository's `.gitconfig` is code execution coming from whoever created the repo. Default: read hooks only from the global and user config. Repository hooks require approval. The approval is stored outside the repo and tied to the hash of the command (like `direnv allow`). `includeIf "gitdir:~/trabalho/"` activates hooks per directory tree without writing anything to the repository.

### Native hooks (D18, D29)

Git's native hooks (`.git/hooks`, `core.hooksPath`, Husky) keep running and stay Git's. GitMaster does not reimplement them, does not wrap them, and shows no chip for them. The `pre-commit`, for example, runs inside `git commit`. If it vetoes, its output is the commit error (already the case since slice 1b).

## JSON contract

### Input (stdin, JSON)

```json
{
  "schema_version": 1,
  "event": "prepare-commit",
  "invocation_id": "01J9Y...",
  "repo": { "root": "/work/ledger", "branch": "ledger-ui", "head": "a1f3c9e" },
  "input": { "message": "", "staged_diff": "diff --git ..." }
}
```

Large content (e.g. a diff above a size limit) comes as `*_path`, pointing to a temporary file. This keeps the contract from varying surprisingly with size.

### Output (stdout, JSON)

```json
{ "invocation_id": "01J9Y...", "output": { "message": "feat(ledger): accept Money in total()" } }
```

Gate:

```json
{ "invocation_id": "01J9Y...", "decision": "block", "reason": "protected branch" }
```

### Rules

- An `invocation_id` different from the one sent is a hook error. The result is discarded and the UI shows a visible warning.
- An exit code other than zero means the hook broke. A broken gate is fail-closed: it vetoes and shows stderr. A broken observer only warns.
- A timeout or cancellation kills the process. Since the response only comes through stdout, there is no late response to reconcile.

## Reconciliation

The engine stores the fingerprint of the target when the invocation starts (e.g. HEAD SHA, index tree, message hash) and compares it when the response arrives. The hook never sees this fingerprint.

1. Unknown `invocation_id`, or one that is not pending: discard it and log it.
2. Target state changed: a transform becomes a **suggestion** (the user decides). A gate runs again.
3. Same state: apply the result and record it in the operation log.

Example: the user typed the message while the LLM was generating. The suggestion appears with a "Replace" button and does not overwrite the text.

## Asynchronous execution in the UI

- An asynchronous gate is not an optional gate: the action stays "pending" until all gates respond.
- The approval holds for the SHAs at that moment. If HEAD changed, the approval is invalidated.
- Each hook appears as a chip (running, ok, blocked, failed), with cancel.
- Chained transforms add latency, so there is one chip per hook.

## LLM output

LLM output is not deterministic, so it is treated only as a proposal. It fills the message field. The commit happens only by user action. The operation log records the command, the command hash and the exit code of each hook.

## Q1 resolved (D30)

Writing JSON in shell one-liners requires `jq`. Decision by the author (2026-10-16): pure JSON, no text mode. Whoever wants text writes a short script that reads the JSON.

## Hooks and workflows (D75, decided)

The hook classes map to the phases of script workflows (design/workflow-scripts.md):

- Gate → `before.<operation>` phase. The operation waits (asynchronous, does not block the UI, D05). The workflow vetoes with `ctx.block` and gives the reason. A failure in a `before` phase vetoes (fail-closed).
- Transform → `transform.<field>` phase. The workflow returns the new value of the field (e.g. `transform.commit_message`). The Reconciliation rules in this document apply: if the user changed the field meanwhile, the value becomes a suggestion.
- Observe → `after.<operation>` phase. Runs afterwards. A failure only warns.

- `exec` runs a single command line (argv and stdin), without a shell (D74). This way an LLM CLI works without an intermediate script.
- Migration in WF3, with the "Convert hooks to workflows" action, which writes equivalent workflows into the user's workflow directory.
- Native Git hooks do not change: they stay with Git (D18, D29).
