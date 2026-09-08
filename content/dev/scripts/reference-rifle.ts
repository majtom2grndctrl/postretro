import { defineEntity, defineWeaponPlacement } from "postretro";

const riflePlacement = defineWeaponPlacement({
  positionFromCenter: { right: 0.3, up: -0.35, forward: 0.6 },
});

export const referenceRifleEntity = defineEntity({
  canonicalName: "reference_rifle",
  components: {
    weapon: {
      damage: 9.0,
      range: 80.0,
      fireRateMs: 110.0,
      fireMode: "auto",
      resolution: "hitscan",
      // Sustained fire opens the cone quickly, while the upward bias makes its
      // recoil-like accuracy loss legible without moving the camera.
      bloomPerShotDegrees: 1.3,
      bloomMaxDegrees: 8.0,
      bloomDecayDegreesPerSecond: 14.0,
      bloomDecayDelayMs: 120,
      movementSpreadDegrees: 3.0,
      spreadVerticalBias: 0.3,
      thirdPersonModel: "models/cyberpunk_weapons/rifle/model.gltf",
      viewmodel: "models/cyberpunk_weapons/rifle/model.gltf",
      // Authored from the viewmodel's rigid `muzzle` socket.
      muzzleOffset: [0.0, 0.3, -0.9],
      resource: {
        kind: "ammo",
        type: "bullets.rifle",
        magazine: 30,
        reserve: 120,
        reloadMs: 1500,
        reloadStyle: "magazine",
      },
    },
    mesh: { model: "models/cyberpunk_weapons/rifle/model.gltf" },
    touchable: { mode: "auto", radius: 1.0 },
  },
});
