--!nocheck
-- Trigger-event and trigger-pool declaration builders. A level binds a trigger
-- member's events with `t:on` (`entities/triggers.luau`); a mod declares a
-- standing tag-keyed rule with `defineTriggerEvent`.
-- Part chunk of `data_script.luau`: evaluated before it, published through the
-- temporary `__postretroDataScriptParts` bridge, and merged into its returned
-- builder table. Type names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local TriggerEventsSdk = {}

--- Mod-global trigger event: a standing rule keyed by tag, accepted only in
--- `ModManifest.triggerEvents`.
export type TriggerEventDescriptor = {
  tag: string,
  event: "enter" | "exit",
  fire: {string},
  levels: {string}?,
}

--- Level trigger event: one volume's edge, keyed by the member's id. Built by
--- `t:on`; accepted only in `setupLevel`'s `triggerEvents`.
export type VolumeTriggerEventDescriptor = {
  trigger: EntityId,
  event: "enter" | "exit",
  fire: {string},
}

export type TriggerPoolDescriptor = {
  tag: string,
  arm: number?,
  armPercentage: number?,
  levels: {string}?,
}

--- Authored form of `defineTriggerEvent`: `fire` takes reaction handles or names.
export type TriggerEventRule = {
  tag: string,
  event: "enter" | "exit",
  fire: {Reaction<any> | string},
  levels: {string}?,
}

--- Declare a mod-global trigger event: every volume carrying `tag` fires
--- `fire` on `event`, in each level the `levels` selector matches (every level
--- when omitted). Return it from `ModManifest.triggerEvents`; a level script
--- binds its own volumes with a trigger member's `on`.
function TriggerEventsSdk.defineTriggerEvent(rule: TriggerEventRule): TriggerEventDescriptor
  local names = {}
  for i, reaction in ipairs(rule.fire) do
    names[i] = if type(reaction) == "string" then reaction else (reaction :: Reaction<any>).name
  end
  local out: TriggerEventDescriptor = { tag = rule.tag, event = rule.event, fire = names }
  if rule.levels ~= nil then
    out.levels = rule.levels
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
