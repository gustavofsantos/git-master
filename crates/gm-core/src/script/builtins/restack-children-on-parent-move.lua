-- Restacks the branches above a parent when the parent gets new commits. Built into GitMaster:
-- it is off in every repository until you switch it on there. Copy to Edit makes an editable copy.
local M = {}

M.meta = {
  name = "restack-children-on-parent-move",
  description = "Restack the branches above a parent when it moves",
  trigger = { on = "stack.parent_moved" },
  concurrency = "replace",
}

-- Conflicts the run waits through before it gives up.
local MAX_RESUMES = 3

-- One plan of the subtree at `root`, then its apply. Returns the outcome and the branches planned.
-- The apply runs even when the plan moves nothing: a branch that conflicts is blocked, so it moves
-- zero branches and the apply is what ends `conflict`.
local function restack(ctx, root)
  local plan = ctx.restack.plan({ root = root })
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  return r.outcome, plan.branch_count
end

function M.execute(ctx, input)
  -- The child that became stale: its subtree is what moved.
  local root = input.trigger.branch
  local outcome, count = restack(ctx, root)
  if outcome == "stale" then
    -- The stacks changed between the plan and the apply: plan and apply once more.
    outcome, count = restack(ctx, root)
  end
  local resumes = 0
  while outcome == "conflict" do
    resumes = resumes + 1
    if resumes > MAX_RESUMES then
      return ctx.fail("the restack still conflicts after " .. MAX_RESUMES .. " resumes")
    end
    -- The run waits until the conflict is resolved and its restack finished (or cancelled).
    local w = ctx.wait("restack.resumed", { timeout = "24h" })
    if w.outcome ~= "resumed" then return ctx.fail("conflict not resolved") end
    outcome, count = restack(ctx, root)
  end
  if outcome ~= "done" then return ctx.fail("restack " .. outcome) end
  -- Done with nothing planned: the children were already on the parent.
  if count == 0 then return end
  ctx.notify("Restacked " .. count .. " branches on " .. input.trigger.parent)
end

return M
