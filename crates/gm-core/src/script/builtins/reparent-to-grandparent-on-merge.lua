-- Moves a branch onto its grandparent when its parent is merged or deleted. Built into GitMaster:
-- it is off in every repository until you switch it on there. Copy to Edit makes an editable copy.
local M = {}

M.meta = {
  name = "reparent-to-grandparent-on-merge",
  description = "Reparent a branch to its grandparent when its parent is merged",
  trigger = { on = "stack.parent_merged" },
}

function M.execute(ctx, input)
  local branch = input.trigger.branch
  local grandparent = input.trigger.grandparent
  if grandparent == nil then
    return ctx.fail(branch .. " has no grandparent to move onto: its parent has no parent in the stack")
  end
  local plan = ctx.reparent.plan({ branch = branch, parent = grandparent })
  local r = ctx.try(function() return ctx.reparent.apply({ plan_id = plan.plan_id }) end)
  if r.outcome ~= "done" then return ctx.fail("reparent " .. r.outcome) end
  ctx.notify("Reparented " .. branch .. " onto " .. grandparent)
end

return M
