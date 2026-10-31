# Stacks and restack

Last updated: 2026-10-19. Status of each decision in `design/decisions.md`.

## Model

Each branch has at most one parent and several children: a tree rooted at `main` (D03). Diamonds (a branch that depends on two parents) are not supported. Workaround: an integration branch that merges the two and becomes the child of only one.

```
main
└ ledger-base
  ├ ledger-api
  └ ledger-ui
    └ ledger-e2e
```

## Metadata (decided, D13)

In the repository's own git config (`.git/config`), outside the working tree:

- `branch.<name>.stackParent`: name of the parent.
- `branch.<name>.stackParentBase`: SHA of the parent's tip when the branch was created or last re-anchored.

Scalar values, no lists. The rules for the recorded base, the derived states and the grandparent are in D138.

## Derived states

- **Stale**: the current tip of the parent differs from `stackParentBase`. Emits `stack.parent_moved`.
- **Diverged base**: the recorded base no longer marks where the child's own commits start: it is not an ancestor of the child, or the merge base with the parent strictly descends from it (someone rebased or merged outside the app). Flag it when the repo is opened. Exact rule in D138.
- **Parent gone or merged**: the parent's branch was deleted, or its content is already in `main` (patch-id). Offer "Reparent to grandparent", with confirmation. Typical case: squash merge of the parent. Emits `stack.parent_merged`.

The calculation runs outside the UI thread. The interface shows "calculating" while it runs.

## Primitives and policies (decided, D68)

The engine exposes the stack primitives as Commands: `stack.tree`, `stack.stale`, `restack.plan`, `restack.apply`, `restack.undo`, `reparent.plan`, `reparent.apply`, `push.lease`. It emits the derived events `stack.parent_moved` and `stack.parent_merged`.

Policies (when to restack, which subtree, whether to push, what to notify) are workflows, not code. See the TOML workflow specification, available in git history at commit 5f88889 (docs/10-workflows.md); replaced by design/workflow-scripts.md. Built-in templates, disabled by default:

- "Restack children when parent moves"
- "Restack stale stacks after fetch"
- "Reparent to grandparent when parent is merged"

## Restack (proposed, D14)

- Walk the tree in pre-order (parents before children).
- Per edge: `rebase --onto <new parent tip> <stackParentBase> <child>`. When it completes, `stackParentBase` receives the new parent's tip.
- **Exact, deterministic plan**: simulates the same sequence of cherry-picks, commit by commit, with the same algorithm used for execution (e.g. `git merge-tree --write-tree`). The plan is tied to the SHAs of the refs and is invalidated if any ref changes. Never a "probable conflict".
- **In-memory execution** (libgit2's in-memory rebase; check its coverage), without touching the working copy.
- **Ref snapshot** before starting, **atomic undo** of all affected branches, and a record in the operation log (a model similar to `jj op log`).
- **Persisted state**: survives closing or freezing the app.
- **Conflict** pauses only the affected subtree. Independent siblings continue; descendants stay blocked.
- In automatic execution (workflow), a conflict pauses only that execution, in the restack conflict state and in the affected subtree, and notifies. It never resolves on its own.
- **rerere** enabled: the same conflict reappears on every restack, and the recorded resolution is reapplied deterministically.
- **Push with lease** (D15) is a separate, explicit step, never automatic. In a workflow, `push.lease` always asks for approval (D15, D64).
- Dirty working copy with the current branch in the plan: block with a banner (Q2).

## UI flow

1. **Plan**: ordered list of branches, how many commits each one reapplies, and what depends on what.
2. **Running**: progress per branch, in memory.
3. **Conflict**: the branch pauses. Actions: "Resolve conflict", "Skip branch and children" or "Abort all".
4. **Done**: "Push with lease" and "Undo restack".

A workflow that calls `restack.apply` shows this same flow.

## Integration

- Hooks: `pre-restack` and `post-restack` become the workflows `before.restack` (gate, receives the plan) and `after.restack` (observes) (D75). See design/hooks-transitional.md and the TOML workflow specification, available in git history at commit 5f88889 (docs/10-workflows.md); replaced by design/workflow-scripts.md.
- Workflows: see the TOML workflow specification, available in git history at commit 5f88889 (docs/10-workflows.md); replaced by design/workflow-scripts.md.
- MCP: `stack_tree`, `restack_plan`, `restack_apply(plan_id)`. See docs/agents/mcp-server.md and docs/agents/local-review.md.
- Sidebar and history: see design/screens.md.
