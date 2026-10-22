-- Suggest a commit message from the staged diff.
-- Edit: replace `llm-cli` in the argv with the command line of your own LLM
-- tool. It reads the staged diff on stdin and must print the message on stdout.
-- The prompt is in the argv; change it to suit your team's commit style.
local M = {}

M.meta = {
  name = "suggest-commit-message",
  description = "Suggest a commit message from the staged diff",
  trigger = { on = "manual" },
}

function M.execute(ctx, input)
  for _ = 1, 3 do
    local r = ctx.exec({ "llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only, as plain text, without code fences." },
                       { stdin = input.repo.staged_diff, timeout = "60s" })
    if #r.text > 0 then
      ctx.commit.suggest_message({ message = r.text })
      return
    end
  end
  ctx.fail("llm-cli printed no message")
end

return M
