--!strict
-- World-query vocabulary: typed wrapper around `worldQuery`. Light handle
-- construction delegates via `wrapLightEntity` global installed by the
-- prelude before this module evaluates.

-- `EntityId`, `LightEntity` (the generated snapshot shape),
-- `WorldQueryFilter`, and the `worldQuery` primitive are provided
-- ambiently by the generated `sdk/types/postretro.d.luau` definitions
-- file.
-- `wrapLightEntity` is installed as a temporary global from
-- `sdk/lib/entities/lights.luau` by the prelude evaluator before this
-- module evaluates. It is nil'd out after this module returns so author
-- scripts never see it. See `luau.rs` `evaluate_prelude`.

--- Base entity handle returned by `world:query` when the component type
--- is not "light". Carries id, transform, and tags; use `getComponent`
--- to access the component data for non-light entities.
export type EntityHandle = {
  id: EntityId,
  transform: EntityTransform,
  tags: {string},
}

--- Typed vocabulary object returned from `world:query`.
--- Pass `{ component = "light", tag = "..." }` to get light handle
--- values; any other component filter returns `EntityHandle` values.
export type World = {
  query: (self: World, filter: WorldQueryFilter) -> {EntityHandle},
}

-- ---------------------------------------------------------------------
-- Exported `world` singleton
-- ---------------------------------------------------------------------

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
    local out: {EntityHandle} = {}
    -- Cast at the array level: the primitive returns per-component snapshots
    -- that are supersets of `Entity`, so individual field access is safe.
    local snapshots = raw :: {any}
    for i, snapshot in ipairs(snapshots) do
      out[i] = { id = snapshot.id, transform = snapshot.transform, tags = snapshot.tags }
    end
    return out
  end,
}

return world
