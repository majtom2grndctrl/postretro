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
//   - `max: 48`. At point-blank range the shipped `reference_shotgun` lands all
//     eight 3-damage pellets for 24 damage per shell (see
//     content/dev/scripts/reference-shotgun.ts), so a dummy goes 48 → 24 → 0
//     and downs on the final pellet of the second shell. The next shell's first
//     corpse pellet reads raw `healthAfter = -3` and demonstrates the authored
//     follow-up finisher.
//   - The model supplies its own torso/head/limb hit-zone capsules. The demo
//     uses those authored zones directly, so aiming at its torso is the most
//     reliable way to demonstrate the full-connect shotgun shells.
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
      max: 48,
    },
  },
});
