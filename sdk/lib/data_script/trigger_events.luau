--!nocheck
-- Trigger-event and trigger-pool declaration builders.
-- Part chunk of `data_script.luau`: evaluated before it, published through the
-- temporary `__postretroDataScriptParts` bridge, and merged into its returned
-- builder table. Type names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local TriggerEventsSdk = {}

export type TriggerEventDescriptor = {
  tag: string,
  event: "enter" | "exit",
  fire: {string},
  levels: {string}?,
}

export type TriggerPoolDescriptor = {
  tag: string,
  arm: number?,
  armPercentage: number?,
  levels: {string}?,
}

export type TriggerEventOptions = { levels: {string}? }

function TriggerEventsSdk.onTriggerEvent(
  filter: { tag: string },
  event: "enter" | "exit",
  fire: {Reaction<any> | string},
  options: TriggerEventOptions?
): TriggerEventDescriptor
  local names = {}
  for i, reaction in ipairs(fire) do
    names[i] = if type(reaction) == "string" then reaction else (reaction :: Reaction<any>).name
  end
  local out: TriggerEventDescriptor = { tag = filter.tag, event = event, fire = names }
  if options ~= nil and options.levels ~= nil then
    out.levels = options.levels
  end
  return out
end

--- Identity builder for a trigger-pool declaration returned from a level or
--- mod manifest. Engine parsing owns arming validation.
function TriggerEventsSdk.defineTriggerPool(pool: TriggerPoolDescriptor): TriggerPoolDescriptor
  return pool
end

return {
  builders = TriggerEventsSdk,
}
