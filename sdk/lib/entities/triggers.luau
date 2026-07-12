--!strict
-- Trigger-volume entity vocabulary. Handles expose closed arm/disarm command
-- builders; raw arming and activation state remain engine-owned.

export type TriggerVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  arm: (self: TriggerVolumeHandle) -> {ArmTriggerStep},
  disarm: (self: TriggerVolumeHandle) -> {DisarmTriggerStep},
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
  }
end

return TriggersSdk
