import { defineEntity, defineWeaponPlacement } from "postretro";

const pistolPlacement =  defineWeaponPlacement({
  positionFromCenter: { right: 0.25, up: -0.3, forward: 0.5 },
})

export const referencePistolEntity = defineEntity({
  canonicalName: "reference_pistol",
  components: {
    weapon: {
      damage: 12.0,
      range: 64.0,
      fireRateMs: 180.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: "models/cyberpunk_weapons/pistol/model.gltf",
      viewmodel: "models/cyberpunk_weapons/pistol/model.gltf",
      placement: pistolPlacement,
      // Authored from the viewmodel's rigid `muzzle` socket. Hitscan ignores
      // it today; retaining it keeps this model ready for projectile tuning.
      muzzleOffset: [0.0, 0.225, -0.574],
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
    mesh: { model: "models/cyberpunk_weapons/pistol/model.gltf" },
    touchable: { mode: "auto", radius: 1.0 },
  },
});
