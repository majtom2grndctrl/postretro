// DEMO CONTENT — rifle squads on the shared Mixamo clip library.
//
// Two enemies that reuse the limitator's statechart and rifle attack, each with
// a faction of its own, so a dev map can stage groups that hold separate ground
// and then meet. Their models ship with no clips; the animations they inherit
// from the limitator bind from `models/clip_libraries/mixamo_rifle/library.gltf`
// at load.
//
// Each carries its own AR_4 bake: a rigid mount is solved against one
// character's `hand_r` frame, and these rigs differ from the limitator's.
//
// Map-placeable via `"classname" "rifle_trooper"` / `"rifle_warrior"`.

import { defineEntity } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { limitatorEntity } from "./limitator";

export const RIFLE_TROOPERS_FACTION = "rifle.troopers";
export const RIFLE_WARRIORS_FACTION = "rifle.warriors";
// Matches the limitator's own tolerance, which both squads inherit.
export const RIFLE_TOLERANCE = 4;

const limitator = limitatorEntity.components;
const limitatorHealth = limitator.health;
const limitatorMesh = limitator.mesh;
if (
  limitatorHealth === undefined ||
  limitatorHealth === null ||
  limitatorMesh === undefined ||
  limitatorMesh === null
) {
  throw new Error("limitator must retain its health and mesh for the rifle squads");
}

function rifleEnemy(
  canonicalName: string,
  faction: string,
  model: string,
  rifle: string,
  height: number,
): EntityTypeDescriptor {
  return defineEntity({
    canonicalName,
    components: {
      ...limitator,
      faction,
      health: {
        ...limitatorHealth,
        // Feet to head, centered on the torso.
        hitbox: {
          halfExtents: [0.27, height / 2, 0.27],
          offset: [0, height / 2, 0],
        },
      },
      mesh: {
        ...limitatorMesh,
        model,
        attachments: { hand_r: rifle },
      },
    },
  });
}

export const rifleTrooperEntity = rifleEnemy(
  "rifle_trooper",
  RIFLE_TROOPERS_FACTION,
  "models/rodin_sci-fi_trooper_m/model.gltf",
  "models/ar_4_trooper/model.gltf",
  2.0,
);

export const rifleWarriorEntity = rifleEnemy(
  "rifle_warrior",
  RIFLE_WARRIORS_FACTION,
  "models/cyberpunk_warrior_f/model.gltf",
  "models/ar_4_warrior/model.gltf",
  1.9,
);
