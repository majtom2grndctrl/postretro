// DEMO CONTENT — `cyberpunk_warrior_f` descriptor.
//
// A map-placeable character mesh: a descriptor carrying only `components.mesh`.
// The source glTF (`cyberpunk_warrior_f/scene.gltf`) is a STATIC, un-skinned
// model — it exposes no animation clips and no skin — so this is a stateless
// mesh: `animations`/`defaultState` are omitted (the loader binds the single
// identity joint used for rigid, no-skin models). Placed in the world as a
// standing character prop, giving the map cast some variety alongside the
// skinned `sci_fi_trooper_m` and the retro-pixel figures.
//
// Textures were down-rezzed to the engine's chunky/blocky texel budget; the
// character-model sampler magnifies with Nearest, so the low-res atlas reads as
// hard retro texels up close.
//
// Map-placeable via `"classname" "cyberpunk_warrior_f"`; see
// content/dev/maps/campaign-test.map.

import { defineEntity } from "postretro";

export const cyberpunkWarriorEntity = defineEntity({
  canonicalName: "cyberpunk_warrior_f",
  components: {
    mesh: {
      model: "models/cyberpunk_warrior_f/scene.gltf",
    },
  },
});
