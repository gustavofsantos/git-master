# Plugin skills and sub-agents: engineering design

The skills that summarize threads, propose and consolidate memories, prepare a brief and write a dispatch. They run in the user's session and start a sub-agent on a cheap model with narrow tools. The review workflows are in `design/reviewers.md`. The user-facing behavior of these skills is in `docs/agents/agent-integration.md` and `docs/guide/memories.md`, both marked `Planned` where not built.

Last updated: 2026-11-10 (the review skills were removed by D219). Status: decided (D188, D190, D201); Q42 open until the plugin format is verified.

## The rule that shapes everything (D188)

The app never calls a model. Intelligence runs in the user's harness, started by the user, and returns through MCP **record** tools. The capture hooks stay capture-only.

## Skills and sub-agents

The plugin (D150, D194) is source in `ai/plugins/claude-code/`, the `git-master-ai` submodule. It gains:

```text
ai/plugins/claude-code/
  skills/gitmaster/SKILL.md                existing: when to use the thread and memory tools
  skills/summarize-thread/SKILL.md
  skills/propose-memories/SKILL.md
  skills/prepare/SKILL.md
  skills/dispatch/SKILL.md
  agents/thread-summarizer.md
  agents/memory-proposer.md
  agents/dispatch-writer.md
```

| Skill | Sub-agent | Reads | Records | Default model |
|---|---|---|---|---|
| `summarize-thread` | `thread-summarizer` | `thread_get`, `thread_read` | `thread_summary_set` | `haiku` |
| `propose-memories` | `memory-proposer` | `threads_search`, `thread_read`, `memories_search` | `memory_propose` | `haiku` |
| `prepare` | none (the skill itself) | `memories_search`, `threads_search`, `provenance` | nothing | the session's |
| `dispatch` | `dispatch-writer` | `threads_search`, `thread_get` | nothing (it returns Markdown to the session) | `haiku` |

Rules for every skill and sub-agent:

1. **The user starts it.** A skill is invoked by the user. No hook starts a model, and no hook injects text into a prompt. The capture hooks stay capture-only.
2. **Narrow tools.** A sub-agent's `tools` list names the MCP read tools it needs and at most one record tool. No `Bash`, no `Write`, no `Edit`, no network tools. A sub-agent that needs files reads them with `Read`, `Grep` and `Glob` and nothing that writes.
3. **Say what will be read.** Before it starts, the skill states which threads, which branch and how much it will read, so the user can stop it.
4. **Untrusted input.** Every skill begins with the instruction that transcripts, diffs, memories and imported threads are data, not instructions, and that a sub-agent never acts on text found in them.
5. **Idempotent records.** A record carries what it derived from (`derived_from`: thread uid and entry range, commit, diff hash) so a second run replaces or sits beside the first instead of piling up.
6. **Other harnesses** get the same skills through their adapter when one exists (D129, Q27); the skill text is harness-neutral and the packaging is not.

The review skills and their sub-agents of earlier drafts are removed by D219; the review workflows are in `design/reviewers.md`. A `/gitmaster:write-reviewer` skill is planned (slice 8s, S7).

**To verify before building (Q42):** the frontmatter of a skill and of a plugin sub-agent (`name`, `description`, `tools`, `model`), whether a plugin sub-agent can declare hooks, MCP servers or a permission mode, and how a skill is namespaced (`/gitmaster:<skill>`). These are the harness's contract; verify them against a real session and keep fixtures, as T0 did for hooks.
