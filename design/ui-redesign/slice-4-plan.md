# Slice 4: Review

## Scope

- Replies on review comments, handled, resolve, reopen and Before/after already exist (D198, D200); the MCP loop is unchanged.
- Review page (`shell_review.rs`, `shell_review_files.rs`): left column with the Changes, Gates and Reviewers (planned) cards and the conversations; middle continuous diff with a **Viewed** box per file (`review_files.rs`, pure, tested); right the selected conversation.
- Viewed: fingerprint of the file's diff when marked; clears when it differs; kept with the window only.
- Gates card: the `before.<operation>` workflows switched on; push has none today.

## Decisions (least change)

See D209. Not built: Goal (topic text, slice 5), bypass with a reason (reviewer gates, planned), inline conversations in the continuous diff (the Changes diff keeps them), persistence of Viewed marks.

## Tasks

Done by Opus directly (the session was running short; no delegation in this slice).

## Acceptance

- MCP review tests and Before/after tests unchanged and green; `make check` green.
