// DEMO CONTENT — `cyberpunk_warrior_f` descriptor.
//
// A map-placeable character prop: mesh and idle state, no behavior. The model is
// rigged by Mixamo's auto-rigger and ships with no clips; `idle` binds from the
// shared clip library. A skinned model with no state would draw a T-pose. The
// enemy that shares this model is `rifle_warrior` (rifle-squads.ts).
//
// Map-placeable via `"classname" "cyberpunk_warrior_f"`; see
// content/dev/maps/campaign-test.map.

import { defineEntity } from "postretro";

export const cyberpunkWarriorEntity = defineEntity({
  canonicalName: "cyberpunk_warrior_f",
  components: {
    mesh: {
      model: "models/cyberpunk_warrior_f/model.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
      },
      defaultState: "idle",
    },
  },
});
