-- Create a branch named after a ticket, after you approve the name.
-- Edit: replace `ticket-branch-name` in the argv with your own script. It gets
-- the ticket key as its argument and must print the branch name on stdout.
-- The approval shows that name before anything is created.
local M = {}

---@type gitmaster.Meta
M.meta = {
  name = "branch-from-ticket",
  description = "Create and check out a branch named from a ticket key",
  trigger = { on = "manual" },
  concurrency = "queue",
  inputs = { ticket = { type = "string", prompt = "Ticket key" } },
}

---@param ctx gitmaster.Ctx
---@param input gitmaster.Input
function M.execute(ctx, input)
  local r = ctx.exec({ "ticket-branch-name", input.inputs.ticket }, { timeout = "30s" })
  if ctx.approve("Create branch " .. r.text .. "?") then
    ctx.branch.create({ name = r.text, checkout = true })
  end
end

return M
