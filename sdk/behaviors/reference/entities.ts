// Reference data-context registrations for the rotator and damage-source
// reference behaviors. `registerEntity` is `DefinitionOnly`, so these
// calls must run inside `registerLevelManifest` (the only data-script
// entry point); they will throw `WrongContext` if invoked from a behavior
// script.
//
// See: context/lib/scripting.md §2

import { registerEntity } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

/** Classname for entities driven by `rotator_driver.{ts,luau}`. */
export const ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver";

/** Classname for entities targeted/observed by `damage_source.{ts,luau}`. */
export const DAMAGE_SOURCE_CLASSNAME = "game_damage_source";

/**
 * Register the data-archetype entries used by the reference behaviors.
 * Components are intentionally empty — both archetypes are pure tag/
 * transform carriers; the behaviors locate their work via `worldQuery`
 * filters on tags authored on the placement.
 *
 * Call this from a data script's `registerLevelManifest(ctx)` body.
 */
export function registerReferenceEntities(): void {
  const rotatorDriver: EntityTypeDescriptor = {
    classname: ROTATOR_DRIVER_CLASSNAME,
    components: { light: null, emitter: null },
  };
  const damageSource: EntityTypeDescriptor = {
    classname: DAMAGE_SOURCE_CLASSNAME,
    components: { light: null, emitter: null },
  };
  registerEntity(rotatorDriver);
  registerEntity(damageSource);
}
