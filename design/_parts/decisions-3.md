## Implementation notes for the TOML workflow engine (superseded by D93)

These notes describe code that the workflow scripts replace (D93). They are kept as one list because some choices should carry over. Each choice holds until the author decides otherwise.

- `reason` applies to any transition into `end.*`, not only `end.blocked`, because an `end.failed` can also explain why the run ended.
- The parser checks only the shape of a workflow (known keys, types, one type per state, `timeout` as `<integer><s|m|h>`, `for_each` with a non-empty `states`); graph and reference rules stay in the validator, and the Flow shows states even when validation fails (`parse` and `validate` run separately).
- The transition function separates misuse from run failure: calling `next` on a finished run, or with an outcome outside the state's set, is a caller error (`MachineError`); an expression error, a `for_each` list that is not a list, or a guard that fails ends the run in `end.failed`, with the error as the reason.
- The `timeout` outcome of a wait exists only when the state defines `timeout`, and the `timeout` outcome (of `exec` or of a wait) defaults to `end.failed`, as `failed` does, so a simple `exec` does not need `on.timeout` (decided in WF1, 2026-10-19).
- In conditions, a bare name (`a == 1`) is a syntax error: values are written `${path}`, and comparisons do not chain.
- An item of `for_each` cannot be named `inputs`, `trigger`, `repo` or `states`.
- `commit.suggest_message` trims the message and removes one markdown code fence that wraps the whole message (about three backticks on the first and last lines), as an LLM often returns it; a code block inside the message stays (2026-10-19, after the first real use).
- `exec.format` accepts only `json`, as the driver does, and `approve.summary` is optional, as in the driver.
- `set` has a `value` field of type `Any`, which the validator does not check, and `set` accepts any key.
- A value with `${...}` in a `duration` or `choice` field is checked only when the run executes; the validator does not reject it, while text fields (`argv`, `stdin`, `text` and the others) accept templates normally.
- An unknown key in an action that has fields produces a warning, not an error; an action without fields (`noop`, `stage.all` and the ones the driver does not run yet) is not checked, as in the validator contract.
- `set_field` refuses a key that the action does not have (`set` and actions without fields excepted), an empty text in an optional field removes the key, and a `with` that becomes empty through an edit is removed.
- `set_meta` keeps `name` unchanged when it receives `None`, because the name is required.
- The `new-workflow` model (`templates/blank.toml`) comes first in the list, and its name is the file's own `name`, so a copy is called `new-workflow`, `new-workflow-2`, and so on.
- On the Workflows page, saving refuses a file outside the user's workflow directories or inside the repository (D73), and refuses when the file's hash on disk has changed.
- On the Workflows page, running a workflow with unsaved changes is refused with "Save first".
- Saving and deleting workflow files does not enter the command log, because they do not touch Git; deleting moves the file to `<data>/gitmaster/workflow-trash/`.
- The Flow graph had layout choices (layer placement, ordering inside a layer and routing of loops) that are not recorded here.
- Clicking a node or card selects the state; terminal states cannot be selected.
- The inspector sits in the right column at a fixed 320 px, above the cards: the Workflow section always, and the State section for the selected top-level state.
- States of `for_each`, `wait` states and `set` values are edited only in the TOML tab.
- An inspector field is saved on Enter, when focus moves to another field, and before switching tabs, saving or running a dry run, because clicking a button or label does not remove focus in GPUI, so there would be no blur; a rejected edit keeps the typed text and shows the reason in the Flow bar.
- Esc returns a field to the file's value, and a new input is created only with Enter.
- The action picker shows the whole catalog grouped by domain; actions the driver does not run yet appear disabled with "not available yet", and `set` outside `transform.*` appears disabled with "only in a transform trigger".
- The run path appears when "Show in Flow" is clicked on a run opened in the Runs tab (not on every click on a run), and it follows the open run until "Clear" or until another workflow is opened.
- If the run's text differs from the file, the path still appears with the warning "the file changed since this run"; edges that no longer exist are not drawn, and nested states count as their top-level `for_each`.

## D31: worktrees in v1

The author decided on 2026-10-17 that worktrees go into v1, because they are a central part of the author's workflow. The patterns below are recorded as decided, and they change only by a new decision from the author. Change decided by the author on 2026-10-17: the confirmation for forced removal changed from typing the name to ticking a confirmation checkbox (see the "Safe removal" item).

- **Destination:** a new worktree goes into a configurable base directory, whose default is a sibling of the repository: `../<repo>-<branch>`.
- **Safe removal:** removing a worktree that is dirty, has unpushed commits or is locked is blocked by default. Forced removal runs only after the confirmation checkbox in the dialog is ticked. The checkbox has an English label that names what will be discarded, starts unticked, and the Remove button stays disabled until it is ticked. The main checkout, the current window and a missing folder stay refused, with or without the checkbox. Changed on 2026-10-17 (before: typing the worktree name). The engine repeats the check and refuses forced removal without the confirmation.
- **One window per worktree:** each worktree opens in its own window, in line with one repository per window (Q11).
- **Main checkout first:** the main checkout appears as the first row of the "Worktrees" section of the sidebar.
- **Branch already in use:** checking out a branch that is already in another worktree shows the Git error and the action "Go to that worktree".
- **Color:** the worktree color comes from a hash of its name, stable across sessions (the same rule as the lanes in D25).
- **Out of v1:** submodules, sparse checkout and bare repositories.
- **No agent:** this decision does not include integration of worktrees with agents. There is no agent tag in the list and no `Actor::Agent` path for worktrees; see Q12.

The worktree actions in the registry stay disabled with "Not available yet" until the matching sub-slice of slice 1c (item 5 of design/roadmap.md) lands. Create (W3), open (W4), remove (W5), lock, unlock and clean up (W6), and the branch-menu variant (W8) are already in place.

## Decisions of the history slice (slice 3)

Details of each sub-slice (H1 to H8) are in design/roadmap.md.

Outside slice 3 (decided by the author on 2026-10-17, with no ID of its own): stack scope, commit owner, stack parent, dashed edge, Stacks tab, Restack counter, Fetch and Pull, reflog, stashes and Local review. The stack items are marked "Not available yet".

## D38: settings dialog

On 2026-10-17 the author decided that the settings dialog will be built in **Slice 9 (premium polish)** and will take the form of a **modal dialog**. When it is implemented, it will cover everything the app allows the user to configure at that point.

- **Modal format:** a dialog centered over the current window, with a darkened backdrop (`black/50`), closable with the `Esc` key, a click on the backdrop or the close button. The layout has a sidebar of categories on the left for navigation and a form of typed fields on the right.
- **Access:** the `⌘,` shortcut (macOS) and `Ctrl+,` (Linux/Windows); the typed action `open_settings` in the action registry (`action_registry.rs`); the command palette (`Open Settings`); and a button or icon in the footer or header.
- **Strict persistence outside the working tree (Principle 5):**
  - Git config: changes to identity, default branch, worktree root and hooks go to `~/.gitconfig` (global) or `.git/config` (local), with an explicit scope selector.
  - App settings: visual and behavioral preferences of the app go to `settings.json` in the user's config directory (`~/.config/gitmaster/` on Linux).
  - Security and authorizations: the list of approved repository hooks goes to `hook-approvals.json` in the user's data directory (`~/.local/share/gitmaster/`).
  - No "Save" button: writes happen immediately when a field is changed or loses focus, with inline validation.
- **Broad scope in Slice 9:**
  - *General:* theme (System/Dark/Light), UI and code fonts, clearing recent projects, and the file limit of the status watcher.
  - *Git & Identity:* `user.name`, `user.email` (Global vs Repository), `init.defaultBranch`, `core.editor`.
  - *Worktrees:* the default root for new worktrees (`gitmaster.worktree.root`), and the safe removal rules (D31).
  - *Diff & Files:* the default algorithm (`histogram`, `myers`, `patience`), context lines, and ignoring spaces by default.
  - *Hooks & Security:* the catalog of active hooks, management of script approvals with hashes, and a revoke button.
  - *Stacks & Restack:* restack policies with a dirty repository (Q2), and push with lease as the default (D15).
  - *Conflict Resolver:* conflict style (`merge.conflictStyle`) and rerere.
  - *MCP & Agents:* activation of the MCP server, socket path, tokens, and authorization policies for agent actions (D20).
  - *Local Review:* storage of local comments and snapshots (D11).

## Open questions

- **Q1. Hook contract for shell:** (A) plain JSON; (B) text pipe; (C) canonical JSON with an optional `mode = text` per hook (codec on the host). **Decided (2026-10-16): A**, see D30.
- **Q2. Restack with a dirty working copy:** block with a banner, or use a deterministic autostash recorded in the operation log. Start with blocking.
- **Q3. Local review:** confirm its inclusion in the product, and where to store comments and snapshots (SQLite in `.git/` or in the app's data directory).
- **Q4. Config key names:** `gitmaster.*`, or mirror the recent Git config format for hooks (`hook.<name>.command`, to be verified).
- **Q5. `split-commits` hook:** a proposal to group hunks into several commits, confirmed by a human. An idea for after the base.
- **Q6. libgit2 versus gitoxide:** check the maturity of status, merge and in-memory rebase before closing D26. **Spike C (2026-10-16, `spikes/c-rebase`):** libgit2's in-memory rebase matches `git rebase --onto` in 12 scenarios (clean stack, conflict with all three sides, rename, merge commit, binary, mode, 3000 files, amended parent), without touching the index or the working tree. 200 commits took 135 ms (against 2377 ms for `git merge-tree` per commit). A commit that ends up empty requires treating `Applied` as a skip. `merge-tree --merge-base` requires git 2.40+. Not tested: rerere, rebase hooks, signing and submodules. Gitoxide was not tested. **Spike D (`spikes/d-status`):** status by subprocess is 2.5x faster than libgit2 at 500 thousand files, and `--no-optional-locks` avoids breaking concurrent commits; this confirms the status-by-subprocess choice in D26. fsmonitor has no verdict (the daemon is unavailable on this machine).
- **Q7. Editable text in GPUI:** the conflict resolver's result panel and the diff view need editable, virtualized text. Evaluate third-party libraries (for example `gpui-component`) and their licenses (check the license of the Zed crates beyond `gpui`). **Spike B (2026-10-16, `spikes/b-text`):** port the text area from rvw (same author, MIT; 55 tests pass, the window opens; mouse, IME, clipboard and wrap were only read from the code, so manual testing is still needed). Zed's editor is GPL-3.0-or-later; `gpui-component` is Apache-2.0 but depends on another version of gpui. The whole text is shaped on every frame, so it suits the composer, not the conflicts panel. Author decision (2026-10-16): GitMaster will be fully open source, and the rvw code may be used.
- **Q8. Scope items from reference clients without a decision:** pull requests, submodules and LFS. Worktrees were decided on 2026-10-17 (D31). For worktrees, submodules are out of v1; the question of general submodule support stays open.
- **Q9. Alternative layout for the conflict resolver** (unified or stacked) for small screens.
- **Q10. License and product model:** how to combine open source with a "premium product" (open core, a paid binary with open source code, etc.).
- **Q11. Slice 1b (resolved on 2026-10-16):** (a) checkout with a dirty working copy passes Git's error through, with no autostash and no "carry changes over" option. (b) One repository per window in v1. (c) The command log stays in memory only in v1.
- **Q12. Worktrees and agents:** an agent creating, marking or removing worktrees (agent tag in the list, `Actor::Agent`, actions through MCP) is not part of D31. Decide together with slice 7 (MCP), with human approval through the UI.
- **Q13. Code navigation (design/code-navigation.md), resolved on 2026-10-21:** it goes into the product (D43), and its position is right after H5: the author postponed H6 to H8 and asked for 3c next (N1 and N2 together, with no smoke scenario, as in D88).
- **Q14. Symbol engine:** embedded tree-sitter (D45), or only file name and text in the first version? Decide with the numbers from spike S-B (build time, binary size on Linux and macOS, query quality). Also: which languages come in the first wave, and where the index lives (`.git/gitmaster/` or the app's data directory).
- **Q15. Workflows in the repository: Resolved (2026-10-19):** no sharing; the v1 persona is a solo engineer; workflows come only from the user's config (D73). A team shares workflows through a versioned folder in the repository. Read them with approval by hash, as in D17, or never read workflows from the working tree?
- **Q16. Built-in `llm` step: Resolved (2026-10-19):** command line only: the LLM is a command-line program the user already has; no HTTP step and no key stored by the app (D74). HTTP to a provider with the user's key, without an account, or only `exec`?
- **Q17. Time triggers: Resolved (2026-10-19):** not now; no time triggers in v1. Every N minutes while the app is open, in a local app?
- **Q18. Hooks as workflows: Resolved (2026-10-19):** yes: workflows replace the GitMaster hooks (D75). In the long run, should hooks be expressed as workflows (one engine), or stay separate?
- **Q19. Validation subcommand: Reopened and resolved (2026-10-22), D97:** yes, as subcommands for one-off use (`check`, `dry-run`, `test`); they are not a headless mode. Before (2026-10-19): not now; in the first version, validation stays only in the UI (Workflows page, D76). Is `gitmaster workflow check <file>`, to validate in a dotfiles repository, allowed by D02 (it is a validator, not a headless mode)?
- **Q20. Roadmap position: Resolved (2026-10-19):** confirmed: slice 3w comes right after slice 3, before 3c. Stack actions (WF4) come with slices 4 and 5.
- **Q21. Dry run of `exec`: Resolved (2026-10-19):** it never runs in the dry run; it only shows the `argv` and `stdin` that would be sent. Should it never run, or allow running `exec` steps whose `argv` the user marked as `pure = true`? (default: never)
- **Q22. Script engine:** Luau via `mlua` (default), Lua 5.4 or Rhai. Decide with spike S-D (design/roadmap.md): sandbox escapes, budgets, cancellation latency, binary size, and building on the three platforms and in CI (Luau is C++). Details in design/workflow-scripts.md.
- **Q23. Durable execution:** re-run the script from the journal, as Temporal does, postponed by D95. Reopen when losing runs when the app closes becomes a problem.
