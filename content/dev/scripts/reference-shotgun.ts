import { defineEntity } from "postretro";

export const referenceShotgunEntity = defineEntity({
  canonicalName: "reference_shotgun",
  components: {
    weapon: {
      damage: 12.0,
      range: 64.0,
      fireRateMs: 700.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: "models/smg/model.gltf",
      viewmodel: "models/smg/model.gltf",
      resource: {
        kind: "ammo",
        type: "shells.buck",
        magazine: 8,
        reserve: 32,
        reloadMs: 450,
        reloadStyle: "perShell",
      },
    },
    // A press-mode drop makes the fixture exercise deliberate re-acquisition
    // as well as the pistol's automatic enter-edge path.
    mesh: { model: "models/smg/model.gltf" },
    touchable: { mode: "press", radius: 1.0 },
  },
});
