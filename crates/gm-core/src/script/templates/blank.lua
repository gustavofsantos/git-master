-- An empty workflow: it runs when you start it and sends one notice.
-- Edit: change `execute` to do the work you need. Each ctx call is one step of the run.
local M = {}

M.meta = {
  name = "new-workflow",
  description = "An empty workflow to build in the Flow tab",
  trigger = { on = "manual" },
}

function M.execute(ctx, input)
  ctx.notify("Hello from new-workflow")
end

return M
