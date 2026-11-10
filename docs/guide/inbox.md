---
title: Inbox
description: The Inbox lists what needs you in the repository, in four sections, and clears each item by itself once its cause is gone.
status: partial
order: 5
---

The **Inbox** page lists what needs you in the repository you have open. Open it with **Inbox** in the sidebar, or **Show Inbox** in the command palette. The badge on the sidebar entry counts the items under **Needs you** and **Returned by agents**.

Nothing is stored for the Inbox. Every item comes from the state of the repository and of GitMaster's own records, so an item leaves the Inbox by itself as soon as its cause is gone: the conflict is resolved, the run is approved, the comment is resolved, the branch is up to date.

## Sections

| Section | Items |
|---|---|
| **Needs you** | A conflicted file. A workflow run that waits for your approval. A question an agent asked on your code, until you reply or resolve it. |
| **Returned by agents** | A review comment an agent marked handled. The memories agents proposed, as one item with their count. |
| **Waiting on agents** | A review comment you wrote that is still open: an agent has not handled it yet. |
| **Fell behind** | The current branch behind its upstream. A branch of a stack behind its parent, or whose parent is gone or merged. Stack items need the stacks preview feature. |

Each item shows what it is, where (a file or a branch) and, for a comment, when its last word was written. The header says **This repository**: the Inbox covers the repository open in the window.

## Act on an item

Click an item to select it. The top bar then shows its actions, the primary one first, and **⋯** opens the command palette with all of them. Press `Enter` or double-click the item to run its primary action. `Up` and `Down` move the selection.

| Item | Primary action | Other actions |
|---|---|---|
| Conflicted file | **Resolve Conflict**, or **View File** when the conflict resolver preview is off | **Copy File Path** |
| Workflow approval | **Approve Workflow Step** on the newest run waiting; **Show Workflows** for an older one | **Reject Workflow Step**, **Show Workflows** |
| Comment | **Open Comment in Diff** | **Resolve Comment** and **Reopen Comment** for a handled comment, **Show Local Review** |
| Proposed memories | **Show Memories** | |
| Branch behind its upstream | **Pull** | **Fetch** |
| Stack branch behind its parent | **Restack This Stack** | |
| Stack branch whose parent is gone or merged | **Reparent to Grandparent** | **Remove From Stack** |

> **Planned.** A `before` workflow that blocked an operation is not listed under **Needs you**.
