// DEMO CONTENT — `anim_demo_grunt` descriptor (M10 skinned animation).
//
// A map-placeable animated-mesh entity: a descriptor carrying `components.mesh`
// with a per-entity animation-state map (`idle`, `alert`) and a `defaultState`.
// Both states intentionally reuse the single clip the only shipped skinned model
// exposes (`scene.gltf` → one clip named `mixamo.com`).
//
// See content/dev/maps/anim-demo.README.md for what the demo proves, why the
// levelLoad state switch is a hard cut (not a crossfade), and the multi-clip
// model swap instructions (kept there, the single source of truth).

import { defineEntity } from "postretro";

export const animDemoGruntEntity = defineEntity({
  canonicalName: "anim_demo_grunt",
  components: {
    mesh: {
      model: "models/decraniated_low_poly_retro_pixel/scene.gltf",
      animations: {
        idle: { clip: "mixamo.com", loop: true },
        alert: {
          clip: "mixamo.com",
          loop: false,
          crossfadeMs: 250,
          interrupt: "smooth",
        },
      },
      defaultState: "idle",
    },
  },
});
