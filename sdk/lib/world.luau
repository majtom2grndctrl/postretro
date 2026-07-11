--!strict
-- World-query vocabulary: typed wrapper around `worldQuery`. Light,
-- fog-volume, mover, and trigger handle construction delegate via temporary wrapper
-- globals installed by the prelude before this module evaluates.

-- `EntityId`, `LightEntity` (the generated snapshot shape),
-- `WorldQueryFilter`, and the `worldQuery` primitive are provided
-- ambiently by the generated `sdk/types/postretro.d.luau` definitions
-- file.
-- `wrapLightEntity`, `wrapFogVolumeEntity`, `wrapMoverEntity`, and
-- `wrapTriggerVolumeEntity` are
-- installed as temporary globals by the prelude evaluator before this module
-- evaluates. They are nil'd out after this module returns so author scripts
-- never see them. See `luau.rs` `evaluate_prelude`.

--- Base entity handle returned by `world:query` when the component type
--- is not "light". Carries id, position, and tags.
export type EntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
}

--- Typed vocabulary object returned from `world:query`.
--- Pass `{ component = "light", tag = "..." }` to get light handles
--- with capability methods (`:pulse`, `:fade`, `:flicker`, `:colorShift`,
--- `:sweep`); `{ component = "fog_volume", ... }` yields fog volume handles
--- with capability methods (`:pulse`, `:fade`, `:flicker`, `:pulseSaturation`,
--- `:fadeSaturation`); `{ component = "kinematic_mover", ... }` yields mover
--- handles with `:start`, `:stop`, `:reverse`, and `:goToPathNode`; `{ component = "trigger_volume", ... }`
--- yields trigger handles with `:arm` and `:disarm`; `{ component = "emitter", ... }` yields
--- handles carrying the full `BillboardEmitterComponent` snapshot under
--- `component`; any other component filter returns `EntityHandle` values
--- (id, position, tags).
---
--- Supported component strings: `"light"`, `"fog_volume"`, `"kinematic_mover"`, `"trigger_volume"`, `"transform"`,
--- `"emitter"`, `"particle"`, `"sprite_visual"`. `"particle"` and
--- `"sprite_visual"` always return `{}` (engine-managed). Unknown strings
--- error at runtime.
export type World = {
  query: (self: World, filter: WorldQueryFilter) -> {EntityHandle},
  --- Current world gravity in m/s² (negative = downward; positive = upward).
  --- Seeded from worldspawn `initialGravity` at level load.
  getGravity: (self: World) -> number,
  --- Set world gravity in m/s². Non-finite values are silently ignored.
  setGravity: (self: World, value: number) -> (),
}

-- ---------------------------------------------------------------------
-- Exported `world` singleton
-- ---------------------------------------------------------------------

-- Capture temporary wrapper bridges before the prelude evaluator nils their
-- globals. `world:query` must retain these upvalues after sandboxing, while
-- author scripts must not see the bridges as bare globals.
local wrapLightEntity = wrapLightEntity
local wrapFogVolumeEntity = wrapFogVolumeEntity
local wrapMoverEntity = wrapMoverEntity
local wrapTriggerVolumeEntity = wrapTriggerVolumeEntity

local world: World = {
  query = function(self: World, filter: WorldQueryFilter): {EntityHandle}
    local normalized: WorldQueryFilter = {
      component = filter.component,
      tag = filter.tag,
    }
    local raw = worldQuery(normalized)
    if filter.component == "light" then
      local out: {any} = {}
      for i, snapshot in ipairs(raw) do
        out[i] = wrapLightEntity(snapshot :: any)
      end
      return out :: any
    end
    if filter.component == "fog_volume" then
      local out: {any} = {}
      for i, snapshot in ipairs(raw) do
        out[i] = wrapFogVolumeEntity(snapshot :: any)
      end
      return out :: any
    end
    if filter.component == "kinematic_mover" then
      local out: {any} = {}
      for i, snapshot in ipairs(raw) do
        out[i] = wrapMoverEntity(snapshot :: any)
      end
      return out :: any
    end
    if filter.component == "trigger_volume" then
      local out: {any} = {}
      for i, snapshot in ipairs(raw) do
        out[i] = wrapTriggerVolumeEntity(snapshot :: any)
      end
      return out :: any
    end
    local out: {any} = {}
    -- Cast at the array level: the primitive returns per-component snapshots
    -- that are supersets of `Entity`, so individual field access is safe.
    -- Thread `component` through unchanged when present (e.g. `"emitter"`
    -- handles); leave it absent for queries whose Rust handle omits it
    -- (e.g. `"transform"`), matching the bare `EntityHandle` shape.
    local snapshots = raw :: {any}
    for i, snapshot in ipairs(snapshots) do
      local handle: any = {
        id = snapshot.id,
        position = snapshot.position,
        tags = snapshot.tags,
      }
      if snapshot.component ~= nil then
        handle.component = snapshot.component
      end
      out[i] = handle
    end
    return out :: any
  end,
  getGravity = function(_self: World): number
    return worldGetGravity()
  end,
  setGravity = function(_self: World, value: number): ()
    worldSetGravity(value)
  end,
}

return world
