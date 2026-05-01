--!strict
-- Reference data-context registrations for the rotator and damage-source
-- reference behaviors. `registerEntity` is `DefinitionOnly`, so these
-- calls must run inside `registerLevelManifest` (the only data-script
-- entry point); they will throw `WrongContext` if invoked from a
-- behavior script.
--
-- See: context/lib/scripting.md §2

local M = {}

--- Classname for entities driven by `rotator_driver.{ts,luau}`.
M.ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver"

--- Classname for entities targeted/observed by `damage_source.{ts,luau}`.
M.DAMAGE_SOURCE_CLASSNAME = "game_damage_source"

--- Register the data-archetype entries used by the reference behaviors.
--- Components are intentionally empty -- both archetypes are pure tag/
--- transform carriers; the behaviors locate their work via `worldQuery`
--- filters on tags authored on the placement.
function M.registerReferenceEntities()
  registerEntity({
    classname = M.ROTATOR_DRIVER_CLASSNAME,
    components = { light = nil, emitter = nil },
  })
  registerEntity({
    classname = M.DAMAGE_SOURCE_CLASSNAME,
    components = { light = nil, emitter = nil },
  })
end

return M
