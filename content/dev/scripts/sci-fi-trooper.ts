// DEMO CONTENT — `sci_fi_trooper_m` descriptor.
//
// A map-placeable character prop: mesh and idle state, no behavior. The model is
// rigged by Mixamo's auto-rigger and ships with no clips; `idle` binds from the
// shared clip library. A skinned model with no state would draw a T-pose. The
// enemy that shares this model is `rifle_trooper` (rifle-squads.ts).
//
// Map-placeable via `"classname" "sci_fi_trooper_m"`; see
// content/dev/maps/campaign-test.map.

import { defineEntity } from "postretro";

export const sciFiTrooperEntity = defineEntity({
  canonicalName: "sci_fi_trooper_m",
  components: {
    mesh: {
      model: "models/rodin_sci-fi_trooper_m/model.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
      },
      defaultState: "idle",
    },
  },
});
