--!strict
-- Spawner member vocabulary. A spawner's archetype and count are authored on
-- its map entity; `s:fire()` spawns one batch from this spawner only, when its
-- step runs.

--- Spawner member returned by `getMapEntities("spawner")`. `spawnedTags` are
--- the tags each NPC it spawns carries.
export type SpawnerEntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  spawnedTags: {string},
  fire: (self: SpawnerEntityHandle) -> {SpawnFromSpawnerStep},
}

local SpawnersSdk = {}

function SpawnersSdk.wrapSpawnerEntity(snapshot: SpawnerEntity): SpawnerEntityHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    spawnedTags = snapshot.spawnedTags,
    fire = function(_self: SpawnerEntityHandle): {SpawnFromSpawnerStep}
      return { { id = id, primitive = "spawnFromSpawner" } }
    end,
  }
end

return SpawnersSdk
