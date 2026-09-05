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
        },
      },
      creditSource: "enemy.rifle",
    },
  },
});
