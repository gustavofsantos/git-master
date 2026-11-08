# GitMaster UI redesign: brief for Claude Code

Date: 2026-11-08. Canvas (private to Gustavo): https://claude.ai/artifact/LdH3L8Ds8jehSBkwADTkJv
This brief stands alone. The canvas shows the same decisions as mockups; board M (Feature map) lists every documented feature with its new home.

## Goal
Make the UI calmer and simpler without dropping any documented function:
1. Show state all the time; show actions on intent (selection, hover, ⌘K).
2. Keep one instance of the app for many repositories, grouped by topic (the change you are making).
3. Stay close to the players: Tower for the Git client shell, Graphite for review and inbox, Entire for agent sessions and cross-repo changes.
Nothing here changes Git behavior, storage rules or the "never write to the working tree" principle.

## Principles
- One action registry. Context actions, menus, palette and shortcuts read the same registry. The top bar's context actions are a filtered, ranked view of it for the current selection.
- State is always visible. Sync state, stack staleness, hook status and agent activity never hide. Their buttons can.
- Show on state, not on capability. A control earns permanent space only when the current state makes it relevant (Restack appears when a stack is behind).
- Shortcut rule from the docs. No new single-letter shortcuts. R stays Refresh. Diff views keep ⇧⌘ combinations. Enter runs the primary action of a selected list item.
- Docs move with the code.

## The shell
Top bar, three zones:
- Left: topic picker (before topics exist: the repository name). Second line: repo, then the branch picker ("on ledger-ui ▾"), which replaces today's branch line.
- Middle: "Selected" + what is selected (file, commit, thread, inbox item) and up to three ranked actions from the registry, primary first. "⋯ ⌘K" opens the palette filtered to that selection.
- Right: state group: one sync button whose label is the state (Push ↑2 ▾, Pull ↓3 ▾, Up to date); its menu holds Fetch, Pull, Push with lease. Fetch also runs in the background. "Restack N behind" appears only when the current stack is stale.
Sidebar (one component):
1. Pages: Inbox, Changes, Review, History, Code, Agents, with count badges.
2. Topic section: STACK (stack tree with restack N, ↑N, amber dot, parent gone/merged/diverged tags; drag to set parent) or MEMBERS (repo · branch · state).
3. "BRANCHES · <repo>" fold: today's By name tree, filter, N of M, HEAD badge. Collapsed by default.
4. WORKTREES fold: today's section (here, missing, locked, ×, +). Collapsed by default.
5. Footer: Stashes, Reflog, Workflows, Settings.
Removed as separate pages: Search (joins Code as a tab), Threads (joins Agents), Local review (becomes Review), Labels (become filter chips and Tags…). The By stack / By name toggle goes away. Workspaces become a topic action in slice 5; keep them reachable from ⌘K until then.
Status bar: hook chips, transient notices, "Undo ⌘Z" for the last operation from the operation log (Tower parity), "Git log" button (red when the newest command failed), watching-limited notice. The path and the staged/unstaged counts leave it.

## Pages
- Changes (Tower layout): composer above the file list (subject with counter, description, Amend, Suggest message, Commit on <branch> ⌘↵; the prepare-commit suggestion keeps Replace / Discard and never overwrites typed text); checkbox per file and tri-state per folder; in a cross-repo topic files group by member and the commit target follows the selected file's repo; compact Staged | Unstaged and Unified | Split; chunk actions on the @@ header; half-staged pair banner with "Select the pair"; agent-edited files show thread, turn and edit state (in working tree / in HEAD / changed since).
- Review (Graphite PR page plus Entire Trail): left column with Goal (editable topic text), Changes per member (branch → base, +/−, state), Gates card, Reviewers (planned, layout only), Conversations list; right side one continuous diff with a Viewed checkbox per file (the documented reviewed marker; clears when the file's change changes), inline conversations. Conversations: a review comment starts a thread; you and the agent reply (the agent over MCP); the agent marks it handled; you Resolve or Reply and reopen. Before/after snapshots stay. Outdated stays a flag on any state. Gates card shows only gates that exist today; Push with lease shows how many block; bypass needs a written reason.
- Inbox (Graphite sections): Needs you (conflicts, workflow approvals, blocked gates), Returned by agents (handled conversations, proposed memories when they exist), Waiting on agents, Fell behind. Item = what, where (repo · branch), when; selecting it fills the context actions; Enter runs the primary action; scope All topics / this topic. Derived from existing state (operation log, workflow runs, review store, stack states, threads); no new notification store; an item clears when its cause clears.
- History: unchanged behavior; scope pill next to the title; Conversations in commit details; Reflog in the sidebar footer.
- Code: Files | Search tabs (Search keeps Aa, ab, .*, new, pathspec box, limits). Line selection offers Comment for agent ⇧⌘M, Why is this code here?, Open in editor.
- Agents (Entire Sessions style): Threads tab with list grouped by day; row = title, agent chip, model, repo · branch, turns and edits, open-session dot, star, redacted mark; filters Has edits, Starred, Label, Agent. Reader: header chips (commits, files, edits, note), star, Copy as Markdown, ⋯ (rename, note, tags, link to commit, link to thread, export, delete); turn outline; Turn n dividers; collapsed turn ranges; tool groups with counts by tool and failures, edits and failures visible when closed; Expand all / Collapse all; edit lines with state tags, Open diff, View file. Memories tab: planned, layout only.
- Restack, Conflicts, Workflows: same behavior; only the shell changes. Workflow runs awaiting approval also appear in Inbox.
- Branch menu gains "Add to Topic…": dialog picks repository, branch, and where it is checked out (not checked out / new worktree with suggested folder and .worktreeinclude / main checkout, showing Git's refusal up front).

## Topics (slice 5)
- A topic is a named list of members; a member is (repository, branch) plus where it is checked out, if anywhere. Optional goal text. Stored in the app data directory, never in a repository.
- Opening a repository with no topic creates one named after its branch. A stack is one topic; a cross-repo change is one topic.
- One running instance holds all open topics. `gitmaster PATH` hands the path to the running instance over the existing socket instead of starting a new process. ⌘1–9 switch topics.
- One actor per checkout (one writer per repository). Background checkouts stay warm (watcher, fingerprints) and drop heavy caches; focusing one rebuilds them.
- Workspaces become a topic action: "Give agents one folder" creates the workspace from the members; Record combination lives in the topic menu.
- Membership is derived state: when a branch is deleted or merged, mark the member and offer to drop it. Never guess.

## Do not remove
Context menus and their order; New branch, Delete branch (with the discard checkbox), Delete merged branches; worktree create, remove checks, lock, prune; discard confirmations; move file to branch or worktree by drag (drop targets: stack, Branches and Worktrees rows); Git commands panel; welcome screen and recent repositories; Quick Open; stacked commit diff; conflict resolver; restack dialog; workflows page; all documented shortcuts.

## Slices (in order, each shippable)
1. Shell: sidebar component and regrouping, top bar zones with context actions and the sync button, status bar with Undo, palette ranking (enabled before disabled, disabled collapsed). Done when every page reachable before is still reachable and the documented shortcuts all work.
2. Changes: composer on top, checkboxes and tri-state, compact diff header, half-staged banner. Done when git add -p + commit is still fully replaceable and the staging invariant tests pass.
3. Code and Agents: Search into Code; Threads into Agents with the new list and reader; Labels as chips. Done when every thread action from the docs is reachable.
4. Review conversations: replies on review comments, Review page with continuous diff, Viewed, Changes, Conversations and today's gates. Done when the agent loop over MCP works with replies and Before/after still measures the change.
5. Topics and single instance: model, storage, IPC, members, grouped Changes, Add to topic, ⌘1–9, workspace action. Done when two repositories and a stack are open in one instance and switching keeps each checkout's state.
6. Inbox: derived items and sections. Done when each item clears by itself once its cause is gone.
7. Planned surfaces: out of scope.

## Open questions for Gustavo
- The name: topic, change, trail? (Use "topic" in code and UI for now.)
- Composer on top (Tower) versus at the bottom (today's app). (Go with on top; it is one component move.)
- Should the Branches fold follow the selected member, or always show the topic's main repository? (Follow the selected member.)
