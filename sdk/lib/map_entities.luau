--!strict
-- Map members: `getMapEntities(kind, { tag = ... })` returns the map-placed
-- instances of one kind, each snapshot wrapped in that kind's handle
-- (`entities/*.luau`). Membership is fixed at install, so these are plain
-- arrays the author inspects.
--
-- `wrapLightEntity`, `wrapFogVolumeEntity`, `wrapMoverEntity`,
-- `wrapTriggerVolumeEntity` and `wrapSpawnerEntity` are installed as temporary
-- globals by the prelude evaluator before this module evaluates, captured
-- below, and nil'd out after it returns so author scripts never see them.
-- See: context/lib/scripting.md §12 (Entity addressing)

--- Map-placed kinds `getMapEntities` accepts. NPCs and players are groups
--- (`npcs`, `players`), not members.
export type MapEntityKind = "mover" | "trigger" | "light" | "fog" | "emitter" | "spawner"

--- Narrows a member query to instances carrying `tag`.
export type MapEntityFilter = { tag: string? }

local wrapLightEntity = wrapLightEntity
local wrapFogVolumeEntity = wrapFogVolumeEntity
local wrapMoverEntity = wrapMoverEntity
local wrapTriggerVolumeEntity = wrapTriggerVolumeEntity
local wrapSpawnerEntity = wrapSpawnerEntity

local function projectEmitter(snapshot: any): any
  return {
    id = snapshot.id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,
  }
end

-- Each map kind's engine component name and member wrapper.
local MAP_KINDS: { [string]: { component: string, wrap: (any) -> any } } = {
  mover = { component = "kinematic_mover", wrap = wrapMoverEntity },
  trigger = { component = "trigger_volume", wrap = wrapTriggerVolumeEntity },
  light = { component = "light", wrap = wrapLightEntity },
  fog = { component = "fog_volume", wrap = wrapFogVolumeEntity },
  emitter = { component = "emitter", wrap = projectEmitter },
  spawner = { component = "spawner", wrap = wrapSpawnerEntity },
}
local KIND_NAMES = '"mover", "trigger", "light", "fog", "emitter", "spawner"'

local MapEntitiesSdk = {}

--- Return the map-placed members of `kind`, optionally only those carrying
--- `filter.tag`, in authored map order. An instance a runtime spawn carries
--- never appears. Returns an empty array on no match. Callable only inside a
--- level's `setupLevel`; elsewhere it raises.
function MapEntitiesSdk.getMapEntities(kind: MapEntityKind, filter: MapEntityFilter?): {any}
  local entry = if type(kind) == "string" then MAP_KINDS[kind] else nil
  if entry == nil then
    error(
      string.format(
        "getMapEntities: unknown kind %s; expected one of %s",
        if type(kind) == "string" then string.format("%q", kind) else tostring(kind),
        KIND_NAMES
      ),
      2
    )
  end
  -- The raw engine query stays out of the author-facing typedefs: its
  -- kindless component spelling is what `getMapEntities` replaces.
  local raw: {any} = (_G :: any).worldQuery({
    component = entry.component,
    tag = if filter ~= nil then filter.tag else nil,
  })
  local out: {any} = {}
  for i, snapshot in ipairs(raw) do
    out[i] = entry.wrap(snapshot)
  end
  return out
end

return MapEntitiesSdk
