# Slice 6: Inbox

Scope set by Gustavo (2026-11-08): the current repository only; slice 5 (topics, single instance over a socket) is dropped. Decision D210.

- `inbox.rs` (pure, tested): items in four sections derived from the status, the review comments, the workflow runs, the memories and the stacks. No store, so an item clears when its cause clears.
- `shell_inbox.rs`: the page, selection by item key, Enter / double-click runs the primary action, Up / Down move. The top bar shows the selected item's actions; the sidebar badge counts Needs you and Returned by agents. Opening the page reads the review comments.
- Not built: blocked gates (no state remains after a block's notice).
- Docs: `docs/guide/inbox.md`, first-steps table.
- Done by Opus directly.

Acceptance: `an_item_clears_by_itself_when_its_cause_is_gone` and the section tests; `make check` green.
