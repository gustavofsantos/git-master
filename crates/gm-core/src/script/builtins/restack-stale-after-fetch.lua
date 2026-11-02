-- Restacks the stale stacks after a fetch. Built into GitMaster: it is off in every repository
-- until you switch it on there. Copy to Edit makes an editable copy.
local M = {}

M.meta = {
  name = "restack-stale-after-fetch",
  description = "Restack the stale stacks after a fetch",
  trigger = { on = "fetch.done" },
}

-- Conflicts the run waits through before it gives up.
local MAX_RESUMES = 3

-- One plan of the subtree at `root`, then its apply. Returns the outcome and the branches planned.
-- The apply runs even when the plan moves nothing: a branch that conflicts is blocked, so it moves
-- zero branches and the apply is what ends `conflict`.
local function restack_once(ctx, root)
  local plan = ctx.restack.plan({ root = root })
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  return r.outcome, plan.branch_count
end

-- The same, planned again once when the stacks changed between the plan and the apply.
local function restack(ctx, root)
  local outcome, count = restack_once(ctx, root)
  if outcome == "stale" then
    outcome, count = restack_once(ctx, root)
  end
  return outcome, count
end

-- Restacks one stale subtree, waiting through its conflicts. Returns the outcome and the count.
local function restack_subtree(ctx, root)
  local outcome, count = restack(ctx, root)
  local resumes = 0
  while outcome == "conflict" do
    resumes = resumes + 1
    if resumes > MAX_RESUMES then
      return "conflict", count
    end
    local w = ctx.wait("restack.resumed", { timeout = "24h" })
    if w.outcome ~= "resumed" then return "conflict not resolved", count end
    outcome, count = restack(ctx, root)
  end
  return outcome, count
end

function M.execute(ctx, input)
  local stale = ctx.stack.stale().branches
  if #stale == 0 then return end
  local parent = {}
  for _, branch in ipairs(ctx.stack.tree().branches) do
    parent[branch.name] = branch.parent or ""
  end
  local is_stale = {}
  for _, name in ipairs(stale) do is_stale[name] = true end
  local total = 0
  for _, name in ipairs(stale) do
    -- Restack from the top of each stale subtree: a stale branch under a stale parent is part of
    -- the subtree its parent restacks.
    if not is_stale[parent[name]] then
      local outcome, count = restack_subtree(ctx, name)
      if outcome ~= "done" then
        return ctx.fail("restack " .. name .. " " .. outcome)
      end
      total = total + count
    end
  end
  if total > 0 then
    local word = total == 1 and "branch" or "branches"
    ctx.notify("Restacked " .. total .. " " .. word .. " after the fetch")
  end
end

return M
