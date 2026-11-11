--!strict
-- Attack pool member vocabulary. A pool's member tag and budget profile are
-- authored on its `ai_attack_pool` map entity; `p:start()` and `p:stop()`
-- return steps that act on the host only, when they run.
-- See: context/lib/scripting.md §12 (Entity addressing)

--- Attack pool member returned by `getMapEntities("attackPool")`.
export type AttackPoolEntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  --- Start the pool: every enemy carrying its member tag takes turns in it, under its budget profile, and a spawn carrying the tag joins while it runs. On a running pool, re-assigns its current tag holders; turns, waits and refreshing counts stand.
  start: (self: AttackPoolEntityHandle) -> {StartAttackPoolStep},
  --- Stop the pool: its members return to the default pool and its refreshing counts drop. A no-op on a stopped pool.
  stop: (self: AttackPoolEntityHandle) -> {StopAttackPoolStep},
}

local AttackPoolsSdk = {}

function AttackPoolsSdk.wrapAttackPoolEntity(snapshot: AttackPoolEntity): AttackPoolEntityHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    start = function(_self: AttackPoolEntityHandle): {StartAttackPoolStep}
      return { { id = id, primitive = "startAttackPool" } }
    end,
    stop = function(_self: AttackPoolEntityHandle): {StopAttackPoolStep}
      return { { id = id, primitive = "stopAttackPool" } }
    end,
  }
end

return AttackPoolsSdk
