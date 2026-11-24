import { activation, defineEntity } from "postretro";
import type { ProjectileDescriptor, WeaponDescriptor } from "postretro";
import { ballistic } from "./damage-types";

// DEMO CONTENT — the enemy rifles: projectile weapons enemy attacks resolve by
// name. Their body and trail reuse the established dev projectile fixtures so
// the slow, dodgeable enemy shot remains visible on combat-demo without new art
// assets.

// The single shot: projectile traps volley it.
export const enemyRifleEntity = defineEntity({
  canonicalName: "enemy_rifle",
  components: { weapon: rifleShot(12) },
});

// The same shot reaching 20 m: the limitator's careful long shot. Its shot
// flies 20 m/s for 2 s, 40 m, so it carries the whole reach.
export const enemyRifleLongEntity = defineEntity({
  canonicalName: "enemy_rifle_long",
  components: { weapon: rifleShot(20) },
});

// One shot reaching `range` m, then 750 ms of recovery.
function rifleShot(range: number): WeaponDescriptor {
  return {
    damage: 10,
    damageType: ballistic,
    range,
    primary: { trigger: "hold", recoveryMs: 750, steps: [activation.shot()] },
    resolution: "projectile",
    // The limitator's shot, heard where it fires (sounds/weapons/CREDITS.txt).
    sounds: { fire: "weapons/enemy_rifle_fire" },
    projectile: enemyRifleProjectile(),
    creditSource: "enemy.rifle",
  };
}

// The limitator's and rifle squads' bursts, the aim's and the panicked
// strafe's: three shots 120 ms apart, then a second's recovery. The squad's
// turn holds through the burst.
export const enemyRifleBurstEntity = defineEntity({
  canonicalName: "enemy_rifle_burst",
  components: {
    weapon: {
      damage: 6,
      damageType: ballistic,
      range: 12,
      primary: {
        trigger: "press",
        recoveryMs: 1000,
        steps: [
          activation.shot(), activation.wait(120),
          activation.shot(), activation.wait(120),
          activation.shot(),
        ],
      },
      resolution: "projectile",
      sounds: { fire: "weapons/enemy_rifle_fire" },
      projectile: enemyRifleProjectile(),
      creditSource: "enemy.rifle",
    },
  },
});

// One look for every enemy rifle shot.
export function enemyRifleProjectile(): ProjectileDescriptor {
  return {
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
  };
}
