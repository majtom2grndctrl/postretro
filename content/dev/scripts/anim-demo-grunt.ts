// DEMO CONTENT — exercises the skinned-animation runtime (M10).
//
// `anim_demo_grunt` is a map-placeable animated-mesh entity: a descriptor that
// carries `components.mesh` with a per-entity logical animation-state map. A
// descriptor with `components.mesh` is directly placeable from a `.map` via
// `"classname" "anim_demo_grunt"` (build_pipeline.md §Built-in Classname
// Routing, second sweep — matches placements against `canonicalName`).
//
// Both states below reuse the SINGLE clip shipped by the only skinned model in
// the repo (`scene.gltf` exposes exactly one clip named `mixamo.com`). That is
// intentional for the demo: it proves the full descriptor → mesh component →
// animation-sampling path end to end, and even shows a real crossfade — the
// `alert` state fades in over 250 ms from the running `idle` timeline, blending
// two divergent clip-local poses of the same clip.
//
// TO SWAP IN A REAL MULTI-CLIP MODEL:
//   1. Drop a multi-clip glTF under `content/dev/models/<your_model>/`.
//   2. Point `model` at `models/<your_model>/<file>.gltf`.
//   3. Give each state a DISTINCT authored clip name from that model, e.g.
//        idle:  { clip: "Idle",  loop: true }
//        alert: { clip: "Alert", loop: false, crossfadeMs: 250, interrupt: "smooth" }
//   `clip` is resolved against the model's clip metadata at level load.

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
