// Reference data-context entity descriptors for the rotator and damage-source
// reference behaviors. Spread `referenceEntities` into the `entities` array
// returned from `setupMod()` to register the archetypes.
//
// See: context/lib/scripting.md §2

import { defineEntity } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

/** Classname for entities driven by `rotator_driver.{ts,luau}`. */
export const ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver";

/** Classname for entities targeted/observed by `damage_source.{ts,luau}`. */
export const DAMAGE_SOURCE_CLASSNAME = "game_damage_source";

/**
 * Data-archetype entries used by the reference behaviors. Components are
 * intentionally empty — both archetypes are pure tag/transform carriers; the
 * behaviors locate their work via `worldQuery` filters on tags authored on
 * the placement.
 *
 * Spread into the `entities` array returned from `setupMod()`.
 */
export const referenceEntities: EntityTypeDescriptor[] = [
  defineEntity({
    classname: ROTATOR_DRIVER_CLASSNAME,
    components: { light: null, emitter: null },
  }),
  defineEntity({
    classname: DAMAGE_SOURCE_CLASSNAME,
    components: { light: null, emitter: null },
  }),
];
