// DEMO CONTENT — `target_dummy` descriptor (M10 entity health + damage).
//
// A map-placeable shooting target: a descriptor carrying `components.health`
// with a `max` HP ceiling. The
// shipped weapon's ray hits it and routes damage through the `apply_damage`
// chokepoint. The dev mod's impact policy, not the health component, decides
// whether the target downs, recovers, or despawns.
//
// It reuses the only shipped skinned model (`scene.gltf`) for a visible body,
// mirroring `anim-demo-grunt.ts`. No animation state map is declared — the mesh
// loops clip 0 on the animation clock.
//
// Sizing:
//   - `max: 30`. The shipped `reference_shotgun` deals 12 damage per hitscan hit
//     (see content/dev/scripts/reference-shotgun.ts), so a dummy downs in three
//     shots (12 + 12 + 12 = 36 ≥ 30), then a fourth shot demonstrates the
//     authored follow-up finisher.
//   - The model supplies its own torso/head/limb hit-zone capsules. The demo
//     uses those authored zones directly, so aiming at its torso is the most
//     reliable way to demonstrate the fixed 12-damage shotgun hits.
//
// See content/dev/maps/combat-demo.README.md for the full end-to-end loop.

import { defineEntity } from "postretro";

export const targetDummyEntity = defineEntity({
  canonicalName: "target_dummy",
  components: {
    mesh: {
      model: "models/decraniated_low_poly_retro_pixel/scene.gltf",
      animations: {
        idle: { clip: "mixamo.com", loop: true },
      },
      defaultState: "idle",
    },
    health: {
      max: 30,
    },
  },
});
