---
title: Resolving conflicts
description: The three-pane conflict resolver, role-labeled sides, per-hunk actions, recorded resolutions, script suggestions and the record of who resolved each hunk.
status: partial
order: 90
---

When Git stops on a conflict, the resolver shows both sides next to the result. You choose what goes into each part of the file, and GitMaster records how each part was resolved.

## Layout

> **Planned.** The resolver does not count the changes Git applied on its own, and it has no unified or stacked layout for small windows yet.

The resolver has three panes:

- **Left** shows one side of the conflict.
- **Result** is the file as it is saved. You can edit it directly.
- **Right** shows the other side of the conflict.

Changes that do not conflict are already in the result, since Git applied them. The header shows the counter of conflicts that are resolved.

## Sides are labeled by role

The resolver does not use the names *ours* and *theirs*. In a rebase, Git swaps their meaning: *ours* is the branch you rebase onto, and *theirs* is the commit being replayed. Each side is labeled instead with:

- the name of the branch,
- its role, such as **new parent**,
- the subject of its commit.

## Open the resolver

In the Working Copy, right-click a conflicted file and choose **Resolve Conflict**. The command palette offers the same action. The resolver opens on that file.

## Resolve a conflict

1. Read each conflict in the left and right panes. The selected conflict is highlighted.
2. For each conflict, choose **Left**, **Right** or **Both**. Each action applies to that conflict only. **Clear** takes a choice back, so the conflict shows its markers again.
3. To write different text, edit the result pane directly.
4. Choose **Show base** when you need the common ancestor. Conflicts use Git's `zdiff3` style, whatever `merge.conflictStyle` says, and the base is shown on demand.
5. Use **Previous unresolved** and **Next unresolved** to move between the conflicts that still need a choice.
6. Watch the counter **N of M conflicts resolved**.
7. Choose **Apply**. If conflicts remain, Apply answers with a message that tells you what is still unresolved. It does not sit disabled. When every conflict is resolved, GitMaster writes the file and stages it, and the resolver shows **Resolved and staged.** Choose **Back** to return to the Working Copy.

## Resolve in a restack

> **Planned.** A restack that pauses on a conflict does not offer **Resolve conflict** yet. See [Stacks and restack](stacks.md).

When a restack pauses on a conflict, choose **Resolve conflict** to open the resolver for that branch.

## Recorded resolutions

Git's rerere records how you resolved a conflict, and reapplies that resolution when the same conflict appears again. GitMaster uses rerere on each Git command it runs for the resolver. It does not change the repository's rerere setting.

When Git has a recorded resolution for the file, the resolver shows the banner **Resolved by recorded resolution** above the panes. The Result pane still shows the conflict as Git wrote it, so the banner has two actions:

- **Review** loads the recorded text into the Result pane, in place of the text it held. You can then edit it, and Apply writes it as usual.
- **Forget** removes the recorded resolution and restores the conflict markers, so the conflict is open again. The resolver reads the file once more.

## Script suggestions

> **Planned.** Workflows do not suggest resolutions for hunks yet, so no **Script suggestion** option appears.

A workflow using the `transform.conflict_hunk` trigger can suggest a resolution for a hunk. The suggestion appears as a third option, labeled **Script suggestion**, next to **Left**, **Right** and **Both**. GitMaster never applies a script suggestion by itself. You choose it, or you choose another option.

An agent can propose a resolution in the same way, through MCP. See the [MCP server](../agents/mcp-server.md) page. For how workflows run their triggers, see [Triggers and phases](../workflows/triggers-and-phases.md).

## Who resolved each hunk

> **Planned.** No workflow suggests a resolution yet, so no hunk is recorded as **script**.

The operation log records how every hunk was resolved: by you (**human**), or by a recorded resolution (**recorded resolution**). A hunk you choose or edit is **human**. A hunk whose text came from the recorded resolution, and that you have not changed since, is **recorded resolution**. Each hunk card shows its origin next to its choice.
