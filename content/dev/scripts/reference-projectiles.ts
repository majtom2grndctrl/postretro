import { defineEntity, defineWeaponPlacement } from "postretro";

const PLASMA_RIFLE_MODEL = "models/cyberpunk_weapons/rpg/model.gltf";
const ROCKET_LAUNCHER_MODEL = "models/cyberpunk_weapons/sci_fi_weapon/model.gltf";

const plasmaRiflePlacement = defineWeaponPlacement({
  positionFromCenter: { right: 0.42, up: -0.6, forward: 0.98 },
  rotation: { yaw: -8, pitch: 1, roll: -5 },
});

const rocketLauncherPlacement = defineWeaponPlacement({
  positionFromCenter: { right: 0.4, up: -0.45, forward: 0.75 },
  rotation: { yaw: -4, pitch: 1, roll: -2 },
});

// Reference projectile weapons use their matching Cyberpunk Guns models for
// pickups and stay in the default dev loadout so every map can exercise both
// body variants.
// Their body and trail values are presentation only; collision and damage use
// the descriptor's speed, radius, lifetime, and range.
export const referencePlasmaBoltEntity = defineEntity({
  canonicalName: "reference_plasma_bolt",
  components: {
    weapon: {
      damage: 10.0,
      range: 96.0,
      fireRateMs: 130.0,
      fireMode: "auto",
      resolution: "projectile",
      thirdPersonModel: PLASMA_RIFLE_MODEL,
      viewmodel: PLASMA_RIFLE_MODEL,
      placement: plasmaRiflePlacement,
      muzzleOffset: [0.0, 0.372, -0.984],
      projectile: {
        speed: 40.0,
        radius: 0.5,
        lifetimeMs: 5000.0,
        visual: {
          body: {
            kind: "sprite",
            sprite: "plasma_bolt",
            size: 1.5,
            // tint: [0.2, 0.7, 1.0],
            emissive: 0.85,
            frameDurationMs: 60.0,
          },
          light: {
            color: [0.2, 0.7, 1.0],
            intensity: 0.5,
            falloffRange: 7.0,
          },
          // A brief static blue-white contact pop.
          impactLight: {
            color: [0.55, 0.85, 1.0],
            intensity: 0.85,
            radius: 20.0,
            fadeMs: 180.0,
          },
        },
      },
      creditSource: "player.reference-plasma:primary",
    },
    mesh: { model: PLASMA_RIFLE_MODEL },
    touchable: { mode: "auto", radius: 1.0 },
  },
});

export const referenceRocketEntity = defineEntity({
  canonicalName: "reference_rocket",
  components: {
    weapon: {
      damage: 36.0,
      range: 128.0,
      fireRateMs: 750.0,
      fireMode: "semi",
      resolution: "projectile",
      thirdPersonModel: ROCKET_LAUNCHER_MODEL,
      viewmodel: ROCKET_LAUNCHER_MODEL,
      placement: rocketLauncherPlacement,
      muzzleOffset: [0.0, -0.05, -0.834],
      projectile: {
        speed: 30.0,
        radius: 0.25,
        lifetimeMs: 4000.0,
        visual: {
          // The rocket-launcher model is the model-body fixture;
          // the trailing smoke makes the separate body + trail forms obvious.
          body: { kind: "model", model: ROCKET_LAUNCHER_MODEL },
          light: {
            color: [1.0, 0.65, 0.25],
            intensity: 1.5,
            falloffRange: 12.0,
          },
          // A larger warm shockwave expands as it fades.
          impactLight: {
            color: [1.0, 0.5, 0.18],
            intensity: 3.0,
            radius: 30.0,
            fadeMs: 340.0,
          },
          trail: {
            sprite: "smoke_puff/smoke_puff_00.png",
            rate: 60.0,
            lifetime: 1.75,
            spread: 1.75,
            velocity: [0.8, 0.4, 0.2],
            buoyancy: 0.03,
            drag: 1.0,
            sizeOverLifetime: [0.33, 1.5, 2.5],
            opacityOverLifetime: [0.9, 0.3, 0.0],
            color: [0.9, 0.9, 0.9],
            spinRate: -1.5,
          },
        },
      },
      creditSource: "player.reference-rocket:primary",
    },
    mesh: { model: ROCKET_LAUNCHER_MODEL },
    touchable: { mode: "press", radius: 1.0 },
  },
});
