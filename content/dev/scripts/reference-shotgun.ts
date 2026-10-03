import { activation, defineEntity, defineWeaponPlacement } from "postretro";

const shotgunPlacement =  defineWeaponPlacement({
  positionFromCenter: { right: 0.3, up: -0.45, forward: 0.5 },
})


export const referenceShotgunEntity = defineEntity({
  canonicalName: "reference_shotgun",
  components: {
    weapon: {
      // Damage is per pellet; an eight-pellet full connect deals 8 × 3 = 24.
      damage: 3.0,
      pelletCount: 8,
      spreadDegrees: 5,
      range: 64.0,
      primary: { trigger: "press", recoveryMs: 700.0, steps: [activation.shot()] },
      resolution: "hitscan",
      thirdPersonModel: "models/cyberpunk_weapons/shotgun/model.gltf",
      viewmodel: "models/cyberpunk_weapons/shotgun/model.gltf",
      placement: shotgunPlacement,
      // Authored from the viewmodel's rigid `muzzle` socket.
      muzzleOffset: [0.0, 0.34, -1.137],
      resource: {
        kind: "ammo",
        type: "shells.buck",
        magazine: 8,
        reserve: 32,
        reloadMs: 450,
        reloadStyle: "perShell",
      },
      // Curated from Snake's Authentic Gun Sounds packs (sounds/weapons/CREDITS.txt).
      sounds: {
        fire: "weapons/shotgun_fire",
        dryFire: "weapons/dry_fire",
        reloadStart: "weapons/shotgun_pump_back",
        reloadShell: "weapons/shotgun_shell_load",
        reloadComplete: "weapons/shotgun_pump_forward",
      },
    },
    // A press-mode drop makes the fixture exercise deliberate re-acquisition
    // as well as the pistol's automatic enter-edge path.
    mesh: { model: "models/cyberpunk_weapons/shotgun/model.gltf" },
    touchable: { mode: "press", radius: 1.0 },
  },
});
