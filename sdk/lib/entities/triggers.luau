--!strict
-- Trigger-volume member vocabulary: closed arm/disarm command builders and
-- the per-volume `on` event source. Raw arming and activation state remain
-- engine-owned.

--- Trigger member returned by `getMapEntities("trigger")`.
export type TriggerVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  arm: (self: TriggerVolumeHandle) -> {ArmTriggerStep},
  disarm: (self: TriggerVolumeHandle) -> {DisarmTriggerStep},
  --- Fire `fire` on this volume's `event` edge, and on no sibling volume that
  --- shares its tag. Return the entry from `setupLevel`'s `triggerEvents`.
  on: (self: TriggerVolumeHandle, event: "enter" | "exit", fire: {any}) -> VolumeTriggerEventDescriptor,
}

local TriggersSdk = {}

function TriggersSdk.wrapTriggerVolumeEntity(snapshot: TriggerVolumeEntity): TriggerVolumeHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    arm = function(_self: TriggerVolumeHandle): {ArmTriggerStep}
      return { { id = id, primitive = "armTrigger", args = {} } }
    end,
    disarm = function(_self: TriggerVolumeHandle): {DisarmTriggerStep}
      return { { id = id, primitive = "disarmTrigger", args = {} } }
    end,
    on = function(_self: TriggerVolumeHandle, event: "enter" | "exit", fire: {any}): VolumeTriggerEventDescriptor
      -- Reaction handles lower to their dispatch names; bare names pass through.
      local names: {string} = {}
      for i, reaction in ipairs(fire) do
        names[i] = if type(reaction) == "string" then reaction else reaction.name
      end
      return { trigger = id, event = event, fire = names }
    end,
  }
end

return TriggersSdk
