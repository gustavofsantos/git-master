-- Commit the staged changes with a message you type when you run it.
-- Edit: nothing is required. Change the approval summary to show what you want
-- to see before the commit is made.
local M = {}

---@type gitmaster.Meta
M.meta = {
  name = "commit-staged",
  description = "Commit the staged changes with the message you give",
  trigger = { on = "manual" },
  inputs = { message = { type = "string", prompt = "Commit message" } },
}

---@param ctx gitmaster.Ctx
---@param input gitmaster.Input
function M.execute(ctx, input)
  if ctx.approve("Commit the staged changes with message: " .. input.inputs.message) then
    ctx.commit.create({ message = input.inputs.message })
  end
end

return M
