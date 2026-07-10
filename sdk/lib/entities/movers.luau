--!strict
-- Kinematic-mover entity vocabulary. Handles expose closed command-reaction
-- builders; scripts cannot attach or mutate raw KinematicMover components.

export type MoverEntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  start: (self: MoverEntityHandle) -> {MoverStartStep},
  stop: (self: MoverEntityHandle) -> {MoverStopStep},
  reverse: (self: MoverEntityHandle) -> {MoverReverseStep},
  goToPathNode: (self: MoverEntityHandle, node: string) -> {MoverGoToPathNodeStep},
}

local MoversSdk = {}

function MoversSdk.wrapMoverEntity(snapshot: MoverEntity): MoverEntityHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    start = function(_self: MoverEntityHandle): {MoverStartStep}
      return { { id = id, primitive = "moverStart", args = {} } }
    end,
    stop = function(_self: MoverEntityHandle): {MoverStopStep}
      return { { id = id, primitive = "moverStop", args = {} } }
    end,
    reverse = function(_self: MoverEntityHandle): {MoverReverseStep}
      return { { id = id, primitive = "moverReverse", args = {} } }
    end,
    goToPathNode = function(_self: MoverEntityHandle, node: string): {MoverGoToPathNodeStep}
      return { { id = id, primitive = "moverGoToPathNode", args = { node = node } } }
    end,
  }
end

return MoversSdk
