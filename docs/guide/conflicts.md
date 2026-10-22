---
title: Resolving conflicts
description: The three-pane conflict resolver, role-labeled sides, per-hunk actions, recorded resolutions, script suggestions and the record of who resolved each hunk.
status: planned
order: 90
---

> **Planned.** The conflict resolver is not available in the app yet.

When Git stops on a conflict, the resolver shows both sides next to the result. You choose what goes into each part of the file, and GitMaster records how each part was resolved.

## Layout

The resolver has three panes:

- **Left** shows one side of the conflict.
- **Result** is the file as it is saved. You can edit it directly.
- **Right** shows the other side of the conflict.

Changes that do not conflict are applied to the result for you. The resolver shows how many were applied, for example `3 changes applied`, so you can check them.

For small windows, the resolver offers an alternative layout, unified or stacked, instead of the three panes.

## Sides are labeled by role

The resolver does not use the names *ours* and *theirs*. In a rebase, Git swaps their meaning: *ours* is the branch you rebase onto, and *theirs* is the commit being replayed. Each side is labeled instead with:

- the name of the branch,
- its role, such as **new parent**,
- the subject of its commit.

## Resolve a conflict

1. Read each conflict in the left and right panes.
2. For each conflict, choose **Left**, **Right** or **Both**. Each action applies to that conflict only.
3. To write different text, edit the result pane directly.
4. Show the base when you need the common ancestor. Conflicts use Git's `zdiff3` style, set by `merge.conflictStyle`, and the base is shown on demand.
5. Watch the counter **N of M conflicts resolved**.
6. Apply the result. If conflicts remain, the apply action answers with a message that tells you what is still unresolved. It does not sit disabled.

When a restack pauses on a conflict, choose **Resolve conflict** to open the resolver for that branch. See [Stacks and restack](stacks.md).

## Recorded resolutions

Git's rerere records how you resolved a conflict, and reapplies that resolution when the same conflict appears again. GitMaster enables rerere, so a restack reapplies the resolutions it has recorded.

A hunk resolved this way is labeled **resolved by recorded resolution**. For each one you can review the resolution, or forget it so the conflict is resolved again from the start.

## Script suggestions

A workflow using the `transform.conflict_hunk` trigger can suggest a resolution for a hunk. The suggestion appears as a third option, labeled **Script suggestion**, next to **Left**, **Right** and **Both**. GitMaster never applies a script suggestion by itself. You choose it, or you choose another option.

An agent can propose a resolution in the same way, through MCP. See the [MCP server](../agents/mcp-server.md) page. For how workflows run their triggers, see [Triggers and phases](../workflows/triggers-and-phases.md).

## Who resolved each hunk

The operation log records how every hunk was resolved: by you (**human**), by a recorded resolution (**rerere**), or by a script suggestion (**script**). You can check the origin of any resolution after the fact.
