import { defineEntity } from "postretro";

// DEMO CONTENT — projectile weapon resolved by the limitator's `shoot` attack.
// Its body and trail reuse the established dev projectile fixtures so the slow,
// dodgeable enemy shot remains visible on combat-demo without new art assets.
export const enemyRifleEntity = defineEntity({
  canonicalName: "enemy_rifle",
  components: {
    weapon: {
      damage: 10,
      range: 12,
      fireRateMs: 750,
      fireMode: "auto",
      resolution: "projectile",
      projectile: {
        speed: 20,
        radius: 0.15,
        lifetimeMs: 2000,
        visual: {
          body: {
            kind: "sprite",
            sprite: "projectiles/plasma_blue_diamond.png",
            size: 0.55,
            emissive: 2.5,
          },
          light: {
            color: [0.25, 0.7, 1.0],
            intensity: 1.5,
            falloffRange: 4.0,
          },
          trail: {
            sprite: "smoke_puff/smoke_puff_00.png",
            rate: 32,
            lifetime: 0.45,
            spread: 0.06,
            velocity: [0.0, 0.15, 0.0],
            buoyancy: 0.08,
            drag: 0.6,
            sizeOverLifetime: [0.12, 0.3, 0.0],
            opacityOverLifetime: [0.7, 0.2, 0.0],
            color: [0.3, 0.75, 1.0],
          },
        },
      },
      creditSource: "enemy.rifle",
    },
  },
});
