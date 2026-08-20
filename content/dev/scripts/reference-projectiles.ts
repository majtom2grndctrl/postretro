import { defineEntity } from "postretro";

const WORLD_PICKUP_MODEL = "models/smg/model.gltf";

// Reference projectile weapons stay in the default dev loadout so every dev
// map can exercise both body variants without an FGD tuning surface. Their
// body and trail values are presentation only; collision and damage use the
// descriptor's speed, radius, lifetime, and range.
export const referencePlasmaBoltEntity = defineEntity({
  canonicalName: "reference_plasma_bolt",
  components: {
    weapon: {
      damage: 10.0,
      range: 96.0,
      fireRateMs: 180.0,
      fireMode: "semi",
      resolution: "projectile",
      projectile: {
        speed: 72.0,
        radius: 0.15,
        lifetimeMs: 2000.0,
        visual: {
          body: {
            kind: "sprite",
            sprite: "projectiles/plasma_blue_orb.png",
            size: 0.35,
            tint: [0.2, 0.7, 1.0],
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
          // The existing dev grenade-launcher glTF is the model-body fixture;
          // the trailing smoke makes the separate body + trail forms obvious.
          body: { kind: "model", model: WORLD_PICKUP_MODEL },
          trail: {
            sprite: "smoke_puff/smoke_puff_00.png",
            rate: 36.0,
            lifetime: 0.6,
            spread: 0.08,
            velocity: [0.0, 0.2, 0.0],
            buoyancy: 0.15,
            drag: 0.4,
            sizeOverLifetime: [0.18, 0.28, 0.0],
            opacityOverLifetime: [0.65, 0.25, 0.0],
            color: [0.9, 0.9, 0.9],
            spinRate: -0.7,
          },
        },
      },
      creditSource: "player.reference-rocket:primary",
    },
    mesh: { model: WORLD_PICKUP_MODEL },
    touchable: { mode: "press", radius: 1.0 },
  },
});
