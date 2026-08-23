import { defineEntity } from "postretro";

const WORLD_PICKUP_MODEL = "models/smg/model.gltf";

// Reference projectile weapons use the existing SMG dev model for pickups and
// stay in the default dev loadout so every map can exercise both body variants.
// Their body and trail values are presentation only; collision and damage use
// the descriptor's speed, radius, lifetime, and range.
export const referencePlasmaBoltEntity = defineEntity({
  canonicalName: "reference_plasma_bolt",
  components: {
    weapon: {
      damage: 10.0,
      range: 96.0,
      fireRateMs: 180.0,
      fireMode: "auto",
      resolution: "projectile",
      projectile: {
        speed: 43.0,
        radius: 0.25,
        lifetimeMs: 2000.0,
        visual: {
          body: {
            kind: "sprite",
            sprite: "plasma_bolt",
            size: 1.5,
            // tint: [0.2, 0.7, 1.0],
            emissive: 200.0,
            frameDurationMs: 60.0,
          },
          light: {
            color: [0.2, 0.7, 1.0],
            intensity: 1.0,
            falloffRange: 10.0,
          },
          // A brief static blue-white contact pop.
          impactLight: {
            color: [0.55, 0.85, 1.0],
            intensity: 12.0,
            peakRadius: 90.0,
            radius: 10.0,
            fadeMs: 180.0,
          },
        },
      },
      creditSource: "player.reference-plasma:primary",
    },
    mesh: { model: WORLD_PICKUP_MODEL },
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
      projectile: {
        speed: 40.0,
        radius: 0.25,
        lifetimeMs: 4000.0,
        visual: {
          // The existing SMG dev model is the model-body fixture;
          // the trailing smoke makes the separate body + trail forms obvious.
          body: { kind: "model", model: WORLD_PICKUP_MODEL },
          light: {
            color: [1.0, 0.65, 0.25],
            intensity: 3.0,
            falloffRange: 8.0,
          },
          // A larger warm shockwave expands as it fades.
          impactLight: {
            color: [1.0, 0.5, 0.18],
            intensity: 20.0,
            radius: 2.0,
            peakRadius: 120.0,
            fadeMs: 340.0,
          },
          trail: {
            sprite: "smoke_puff/smoke_puff_00.png",
            rate: 60.0,
            lifetime: 9.0,
            spread: 20.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.25,
            drag: 1.4,
            sizeOverLifetime: [0.18, 3.28, 0.1],
            opacityOverLifetime: [0.65, 0.25, 0.0],
            color: [0.9, 0.9, 0.9],
            spinRate: -2.0,
          },
        },
      },
      creditSource: "player.reference-rocket:primary",
    },
    mesh: { model: WORLD_PICKUP_MODEL },
    touchable: { mode: "press", radius: 1.0 },
  },
});
