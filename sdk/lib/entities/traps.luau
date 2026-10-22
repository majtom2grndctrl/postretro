--!strict
-- Projectile trap member vocabulary. A trap's weapon, interval and phase are
-- authored on its map entity; `t:fire()`, `t:start()` and `t:stop()` return
-- steps that act on the host only, when they run.
-- See: context/lib/scripting.md §12 (Entity addressing)

--- Projectile trap member returned by `getMapEntities("trap")`.
export type TrapEntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  fire: (self: TrapEntityHandle) -> {FireTrapStep},
  start: (self: TrapEntityHandle, options: TrapStartOptions?) -> {StartTrapStep},
  stop: (self: TrapEntityHandle) -> {StopTrapStep},
}

local TrapsSdk = {}

function TrapsSdk.wrapTrapEntity(snapshot: TrapEntity): TrapEntityHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    fire = function(_self: TrapEntityHandle): {FireTrapStep}
      return { { id = id, primitive = "fireTrap" } }
    end,
    start = function(_self: TrapEntityHandle, options: TrapStartOptions?): {StartTrapStep}
      -- Only the fields the author named reach the wire: an omitted one takes
      -- the trap's authored value, while an explicit bad value is rejected.
      local args: TrapStartOptions = {}
      if options ~= nil then
        args.intervalMs = options.intervalMs
        args.phaseMs = options.phaseMs
      end
      return { { id = id, primitive = "startTrap", args = args } }
    end,
    stop = function(_self: TrapEntityHandle): {StopTrapStep}
      return { { id = id, primitive = "stopTrap" } }
    end,
  }
end

return TrapsSdk
