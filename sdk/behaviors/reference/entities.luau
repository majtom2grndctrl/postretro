--!strict
-- Reference data-context entity descriptors for the rotator and
-- damage-source reference behaviors. Spread `M.referenceEntities` into the
-- `ModManifest.entities` array to register the archetypes.
--
-- See: context/lib/scripting.md §2

local M = {}

--- Classname for entities driven by `rotator_driver.{ts,luau}`.
M.ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver"

--- Classname for entities targeted/observed by `damage_source.{ts,luau}`.
M.DAMAGE_SOURCE_CLASSNAME = "game_damage_source"

--- Data-archetype entries used by the reference behaviors. Components are
--- intentionally empty -- both archetypes are pure tag/transform carriers;
--- the behaviors locate their work via `worldQuery` filters on tags
--- authored on the placement.
M.referenceEntities = {
  defineEntity({
    canonicalName = M.ROTATOR_DRIVER_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
  defineEntity({
    canonicalName = M.DAMAGE_SOURCE_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
}

return M
