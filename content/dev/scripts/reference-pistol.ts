import { defineEntity } from "postretro";

export const referencePistolEntity = defineEntity({
  canonicalName: "reference_pistol",
  components: {
    weapon: {
      damage: 12.0,
      range: 64.0,
      fireRateMs: 180.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: "models/smg/model.gltf",
      viewmodel: "models/smg/model.gltf",
      resource: {
        kind: "ammo",
        type: "bullets.light",
        magazine: 12,
        reserve: 48,
        reloadMs: 500,
        reloadStyle: "magazine",
      },
    },
    // The dev pistol doubles as a visible world item for the E16 fixture and
    // gives the default player loadout a recoverable drop path.
    mesh: { model: "models/smg/model.gltf" },
    touchable: { mode: "auto", radius: 1.0 },
  },
});
