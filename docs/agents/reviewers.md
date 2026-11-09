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

> **Planned.** Only reviewers with the **Branch** subject can record verdicts. The **Staged changes** and **One commit** subjects are not available yet.

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
paths: services/billing/**, libs/ledger/**
---
Look at what the change adds. Report input that reaches a shell, a query or a file path
without a check, secrets in the diff, and permissions that grow. Say nothing about style.
```

`paths` and `branches` say which changes the reviewer is for; see [Reviewers for one change](#reviewers-for-one-change). The file name is the reviewer's name, in lowercase letters, digits and hyphens. GitMaster looks in `.git/gitmaster/reviewers/` of the repository first, then in `reviewers/` in your configuration directory, and the repository's file wins when both have the same name. Nothing is written into the working tree. A starting point is a template: **Add Reviewer…** copies it to a file you own. Until you do, it is not a reviewer: a gate that names it blocks and says so, and an agent cannot record a verdict for it.

## Run a reviewer

GitMaster does not start the agent. Choose **Run Reviewer…** on a branch to see the command for your agent session and copy it, or type it there yourself:

```text
/gitmaster:review security
```

With no name, the skill finds its own way:

1. It reads which gates apply to the branch and which reviewers still need a verdict for the current commit, then lists them and says what it will run, so you can stop it.
2. It starts one sub-agent for each of those reviewers, at the same time. Each sub-agent runs with the reviewer's own instructions and its **Model**, or `haiku` when the reviewer sets none.
3. Each sub-agent can read the diff and the repository, and it can record its verdict and findings. It cannot run commands, edit files, change a gate or reach the network.
4. When they have all returned, the skill reads the gates again and tells you whether the branch is **Ready** or what still blocks it. It never bypasses a gate.

With names, such as `/gitmaster:review security risk`, it runs those reviewers whether or not a gate asks for them.

Nothing runs by itself. A commit or a push does not start a reviewer.

## Read a verdict

A verdict shows:

- the signal, with the last five verdicts of the same reviewer on the branch, so you can see whether the answers settle;
- the reasons, as the agent wrote them, marked **Agent-written**;
- the findings, each with a severity of **High**, **Medium** or **Low**, anchored to lines of the diff;
- the commit it judged, and the thread of the agent run, so you can read what the agent did.

A verdict is **Stale** when the branch has moved on since it was recorded. A stale verdict stays visible, and it no longer counts for a gate.

### Findings

A finding is a [review comment](local-review.md) that an agent wrote. It sits in the diff where you read it, and it follows its lines when the code moves. Only you resolve or dismiss a finding. The agent can run again and record a new verdict, and a coding agent can mark a finding handled, but neither can close it.

## Gates

A gate is a rule with a clear answer. The agent can read which gates apply and what each one still needs, so it knows which reviewers to run; it cannot change a gate or bypass one. GitMaster decides it from the recorded verdicts and from Git, and gives the same answer for the same data. The model is involved only when a verdict is recorded, never when a gate is checked.

| Gate | It blocks while |
|---|---|
| **Fresh verdict** | A reviewer you named has no verdict for the current head of the branch. It checks that a verdict exists, not what it says: use **Signal** for that. |
| **Findings** | An unresolved finding has a severity at or above the one you set. |
| **Signal** | A reviewer's value is outside the bound you set, such as risk above 30. |
| **Up to date** | The branch is behind its parent in the stack. |
| **Human approval** | You have not approved the current commit of the branch. |

Set the gates in **Reviewers Settings…**, for all branches or only for branches that match a pattern such as `feat/*`. A gate applies when you use **Push with lease** or apply a restack from GitMaster, and when a workflow does either. A restack is checked for every branch it moves. A blocked operation says which gate stopped it and why.

The branch shows its gate state. It reads **Ready** when every gate passes, and otherwise lists what blocks it, for example "Security has no verdict for this commit" or "1 high finding is open". Ready means the recorded verdicts satisfy your rules. It does not mean the code is correct: the verdicts are one agent's opinion. Any agent connected to GitMaster can record a verdict, so **Fresh verdict** and **Signal** say that an agent reported, not that the change was checked independently. **Human approval** and **Findings** are the two gates an agent cannot satisfy.

> **Note.** A gate checks operations done in GitMaster. A `git push` you type in a terminal is not stopped by it. Gates are a check for your own safety, not a sandbox: the files that define them are in `.git/gitmaster/`, and an agent that can run commands or write files can edit them. If a gates file has an error, GitMaster blocks the push and the restack and shows the error until you fix it or bypass. If you rename a branch outside GitMaster, its approval no longer counts, and its open findings still count because they follow the commits they were written for.

### Bypass a gate

If you decide to go ahead, choose **Bypass** in the message that names the gate and write the reason. The operation runs, and the gates you bypassed, your reason, the branch and its commit are kept in a list of bypasses that survives closing GitMaster. If GitMaster cannot write that record, it does not bypass. If another gate starts to block before you confirm, GitMaster does not bypass it: it shows the block again. Only you can bypass: an agent has no tool for it.

## The intent of a change

> **Planned.** **Edit Branch Description…** is not in the menu yet, and no skill passes the intent to a reviewer yet. An agent can already read it with `gates_status`.

A reviewer such as **Intent drift** compares the change with what it set out to do. That intent is the description of the branch, the one Git keeps for it. Choose **Edit Branch Description…** in the branch menu to write it. GitMaster stores it in the repository's `.git/config`, where `git branch --edit-description` puts it, and writes nothing into the working tree. The agent reads it and gives it to every reviewer it starts, as data.

## Approve a branch

> **Planned.** **Approve Branch** is not available yet.

Choose **Approve Branch** on a branch to say you read its current commit and accept it. You can add a note. The approval stops counting when the branch moves on, and **Revoke Approval** withdraws it. The **Human approval** gate blocks until the current commit is approved. An agent cannot approve a branch, and nothing an agent records can satisfy this gate.

## Fix the findings

> **Planned.** Findings do not carry a suggested fix yet, and an agent cannot mark a finding handled.

A finding can carry a **Suggested fix**, written by the reviewer and shown under the finding. To have your coding agent work through the findings, ask it from its session:

```text
/gitmaster:fix-findings
```

The skill reads the open findings of the branch, checks each one against the code, changes the code or explains why it does not, and marks each finding **Handled** with a short summary. It does not resolve or dismiss anything, and it does not push or merge. A handled finding still counts for the **Findings** gate. You read the summary and the change, then resolve or dismiss the finding yourself.

## Reviewers for one change

> **Planned.** The **Ended** tag is not shown yet. Everything else on this page about `branches` and `paths` works.

A large legacy repository or a monorepo has no reviewer that fits every change. A reviewer or a gate can say which changes it is for:

| Field | What it limits |
|---|---|
| `branches` | The branches it applies to, as patterns such as `feat/billing-*`. A plain name makes it a reviewer for that one branch. |
| `paths` | The files the change must touch, as patterns such as `services/billing/**`. |

With neither, it applies to every change. The change is the set of files that differ between the branch and what it is based on: its parent in the stack, else its upstream, else the default branch of the remote. A gate whose paths the change does not touch is **Not applicable** and blocks nothing, and the branch says why. If GitMaster cannot tell what the branch is based on, the gate applies.

A reviewer or gate made for one branch shows **Ended** once that branch is merged or deleted. GitMaster never deletes it for you.

### Where defaults and custom ones live

Your defaults live in your configuration directory, in `reviewers/` and `gates.json`. A repository adds its own in `.git/gitmaster/`, and a [workspace](../guide/workspaces.md) adds its own in the workspace repository. When the same name appears in more than one place, the most specific one wins as a whole, and **Reviewers Settings…** shows where each one comes from.

## Have Claude propose reviewers and gates

> **Planned.** An agent cannot propose reviewers or gates yet.

```text
/gitmaster:plan-review
```

The skill tells you what it will read, then reads the change, the reviewers and gates you already have, the [memories](../guide/memories.md) that apply to the files the change touches, and the history of those files. It then proposes the reviewers and gates this change needs: for example a reviewer that knows the billing rules of a legacy module, and a gate that requires it whenever `services/billing/**` changes.

A proposal appears in **Reviewers Settings…** as **Proposed**, marked **Agent-written**. A proposed gate does not block anything. You can edit it, then choose **Accept** and pick where it is saved, or **Deny**. A denied proposal stays in the list so it is not proposed again. An agent cannot change, loosen or remove a reviewer or gate you accepted, and cannot accept its own proposal. If it thinks one should change, it proposes the change and you see the old and the new side by side.

## Reviewers and gates in a workspace

> **Planned.** Workspaces have no reviewers or gates yet.

A [workspace](../guide/workspaces.md) can have reviewers and gates of its own, saved in the workspace repository, with the members they are for. A reviewer runs on one member's checkout, and its verdict is kept in that member's repository. Each member's gates are checked against that member's branch and files. The workspace shows **Ready** only when every gate that applies to every member passes. A gate that asks for all members to be ready is a readiness signal and stops no operation, because no operation acts on the whole workspace.

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
