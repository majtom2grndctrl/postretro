// DEMO CONTENT — `sci_fi_trooper_m` descriptor.
//
// A map-placeable character mesh: a descriptor carrying only `components.mesh`.
// The source Rodin export authored its skinned geometry in a ~234-unit
// coordinate space (~100x too large for this engine, which renders meshes at
// raw glTF POSITION scale). It was baked to a STATIC mesh at rest pose and final
// ~2 m scale (feet at origin), dropping the rig and placeholder proxy meshes —
// so this is a stateless mesh with no animation, matching `cyberpunk_warrior_f`.
//
// Textures were down-rezzed to the engine's chunky/blocky texel budget; the
// character-model sampler magnifies with Nearest, so the low-res baked-armor
// atlases read as hard retro texels up close.
//
// Map-placeable via `"classname" "sci_fi_trooper_m"`; see
// content/dev/maps/campaign-test.map.

import { defineEntity } from "postretro";

export const sciFiTrooperEntity = defineEntity({
  canonicalName: "sci_fi_trooper_m",
  components: {
    mesh: {
      model: "models/rodin_sci-fi_trooper_m/scene.gltf",
    },
  },
});
