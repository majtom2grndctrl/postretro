// DEMO CONTENT — `target_dummy` descriptor (M10 entity health + damage).
//
// A map-placeable shooting target: a descriptor carrying `components.health`
// with a `max` HP ceiling AND a `hitbox`. Carrying a hitbox is exactly what
// makes the entity hitscan-targetable — the shipped weapon's ray can hit it,
// route damage through the `apply_damage` chokepoint, and the death sweep
// despawns it once HP reaches zero.
//
// It reuses the only shipped skinned model (`scene.gltf`) for a visible body,
// mirroring `anim-demo-grunt.ts`. No animation state map is declared — the mesh
// loops clip 0 on the animation clock; this entity is about health, not anim.
//
// Sizing:
//   - `max: 30`. The shipped `reference_pistol` deals 12 damage per hitscan hit
//     (see content/dev/scripts/reference-pistol.ts), so a dummy dies in three
//     shots (12 + 12 + 12 = 36 ≥ 30) — enough to *observe* per-hit HP loss
//     before the despawn, not a one-shot.
//   - `hitbox.halfExtents: [0.4, 0.9, 0.4]` → a 0.8 m × 1.8 m × 0.8 m box, a
//     rough human silhouette matching the retro-pixel model. Engine is Y-up, so
//     the middle component is the vertical half-height.
//   - `hitbox.offset: [0, 0.9, 0]` lifts the box center up by its half-height so
//     the box rises from the model's foot-level transform origin to head height.
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
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
    },
  },
});
