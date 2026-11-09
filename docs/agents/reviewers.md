---
title: Reviewers and gates
description: Have an agent on your machine score and review a branch, keep its verdicts, and set rules that stop a push or a restack until the verdicts say the branch is ready.
status: planned
order: 30
---

> **Planned.** Reviewers, verdicts and gates are not available in the app yet.

A reviewer is an agent job with one question, such as "is this change safe?" or "does it still match what the branch set out to do?". You run it on a branch. It answers with a score or a level, the reasons, and findings you can read in the diff. A gate is a rule you set over those answers: it stops a push or a restack while the answers are missing, out of date or bad.

The agent runs in your own agent session, with your own model and your own quota. GitMaster calls no model and stores no key. It records what the agent reports and applies your rules to what was recorded.

## Reviewers

A reviewer has:

| Part | What it is |
|---|---|
| Name and description | What you see in the list. |
| Instructions | What the agent should look at, written in plain words. |
| Signal | What the reviewer reports: a **Score** from 0 to 100, a **Level** (low, medium or high) or a **Pass or fail**. |
| Direction | Whether a higher value is better (confidence) or worse (risk). |
| Subject | What it reviews: the branch against its parent in the stack or its upstream, the staged changes, or one commit. |
| Model | An optional hint for the cheapest model that is good enough. |

GitMaster ships these starting points: **Code review**, **Security**, **Slop** (sloppy patterns), **Intent drift** (the change no longer matches what the branch set out to do) and **Risk**. Edit them or write your own in **Reviewers Settings…**.

### Where reviewers are kept

A reviewer is one Markdown file. The lines between the two `---` marks set the fields, and the text after them is the instructions:

```text
---
description: Looks for security problems in the change.
signal: level
direction: lower_is_better
subject: branch
model: haiku
---
Look at what the change adds. Report input that reaches a shell, a query or a file path
without a check, secrets in the diff, and permissions that grow. Say nothing about style.
```

The file name is the reviewer's name, in lowercase letters, digits and hyphens. GitMaster looks in `.git/gitmaster/reviewers/` of the repository first, then in `reviewers/` in your configuration directory, and the repository's file wins when both have the same name. Nothing is written into the working tree. A starting point is a template: **Add Reviewer…** copies it to a file you own.

## Run a reviewer

GitMaster does not start the agent. Choose **Run Reviewer…** on a branch to see the command for your agent session and copy it, or type it there yourself:

```text
/gitmaster:review security
```

The skill starts a sub-agent for each reviewer, with a cheap model and only the tools it needs: it can read the diff and the repository through GitMaster, and it can record its verdict. It cannot run commands, edit files or reach the network.

Nothing runs by itself. A commit or a push does not start a reviewer.

## Read a verdict

A verdict shows:

- the signal, with the last five verdicts of the same reviewer on the branch, so you can see whether the answers settle;
- the reasons, as the agent wrote them, marked **Agent-written**;
- the findings, each with a severity of **High**, **Medium** or **Low**, anchored to lines of the diff;
- the commit it judged, and the thread of the agent run, so you can read what the agent did.

A verdict is **Stale** when the branch has moved on since it was recorded. A stale verdict stays visible, and it no longer counts for a gate.

### Findings

A finding is a [review comment](local-review.md) that an agent wrote. It sits in the diff where you read it, and it follows its lines when the code moves. Only you resolve or dismiss a finding. The agent can run again and record a new verdict, but it cannot close a finding.

## Gates

A gate is a rule with a clear answer. The agent can read which gates apply and what each one still needs, so it knows which reviewers to run; it cannot change a gate or bypass one. GitMaster decides it from the recorded verdicts and from Git, and gives the same answer for the same data. The model is involved only when a verdict is recorded, never when a gate is checked.

| Gate | It blocks while |
|---|---|
| **Fresh verdict** | A reviewer you named has no verdict for the current head of the branch. |
| **Findings** | An unresolved finding has a severity at or above the one you set. |
| **Signal** | A reviewer's value is outside the bound you set, such as risk above 30. |
| **Up to date** | The branch is behind its parent in the stack. |

Set the gates in **Reviewers Settings…**, for all branches or only for branches that match a pattern such as `feat/*`. A gate applies when you use **Push with lease** or apply a restack from GitMaster. A blocked operation says which gate stopped it and why.

The branch shows its gate state. It reads **Ready** when every gate passes, and otherwise lists what blocks it, for example "Security has no verdict for this commit" or "1 high finding is open". Ready means the recorded verdicts satisfy your rules. It does not mean the code is correct: the verdicts are one agent's opinion.

> **Note.** A gate checks operations done in GitMaster. A `git push` you type in a terminal is not stopped by it.

### Bypass a gate

If you decide to go ahead, choose **Bypass** in the message that names the gate and write the reason. The operation runs, and the gate, the reason and the time are kept in the operation log. Only you can bypass: an agent has no tool for it.

## Review someone else's branch

An agent that reviews a checkout loads that checkout's own agent settings: hooks, MCP servers, skills, commands and plugins. On a branch with commits that are not yours, those come from someone else.

Before it runs a reviewer there, `/gitmaster:review` lists what the branch carries and asks you to confirm. Stop and read the list. If you do not trust it, review in a clean checkout or decline.

## Use verdicts in workflows

A [workflow](../workflows/overview.md) can read the recorded verdicts and findings, for example to add the verdict to a commit message, and a `before.push` workflow can add its own rules on top of the gates. See the [workflow API](../reference/workflow-api.md).

## Tune the reviewers to your repository

```text
/gitmaster:tune-reviewers
```

The skill reads your project's docs, the hotspots of the history, the merged branches and the findings you dismissed in the past, and proposes changes to the instructions of your reviewers. You review the proposal and accept it; nothing is changed without you.

## See also

- [Local review](local-review.md): comment on a diff yourself and have an agent work through the comments.
- [MCP tools](../reference/mcp-tools.md): the verdict and finding tools.
- [Threads](../guide/threads.md): read what a reviewer did.
- [Approvals and security](../workflows/approvals-and-security.md): how GitMaster decides what an agent may do.
