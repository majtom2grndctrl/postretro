import { defineEntity, runtime } from "postretro";

export const playerEntity = defineEntity({
  canonicalName: "player",
  defaultWeapon: "reference_pistol",
  components: {
    // The player pawn carries health but DELIBERATELY no `hitbox`: a hitbox is
    // what makes an entity hitscan-targetable, so omitting it keeps the player
    // out of weapon ray-targeting (and forecloses self-hit). HP is driven only
    // through the `apply_damage` chokepoint — e.g. the combat-demo level's
    // `applyDamage` reaction. Without this block the engine `player.health`
    // producer, its `[0, max]` slot range, and any player-damage reaction all
    // silently no-op. `max: 100` is the conventional full-health baseline.
    health: { max: 100 },
    movement: {
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: { walk: 7.0, run: 11.0, crouch: 3.0 },
        accel: 8.0,
        stepHeight: 0.5,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.5,
        accel: 10,
        maxControlSpeed: 2,
        bunnyHop: true,
        jumps: 0,
        jumpVelocity: 9,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 40.0 },
      dash: {
        boostSpeed: 22.0,
        // Runtime expression (entry-moment): keep less ground momentum than air
        // momentum. Grounded dashes feel snappier/more committed; airborne dashes
        // preserve more of the incoming arc. `grounded` is a boolean input, so
        // `select` branches on it directly.
        momentumRetention: runtime.select(runtime.read("grounded"), 0.4, 0.7),
        // Runtime expression (per-tick): steering authority ramps from 0 up to
        // full across the first 150 ms of the dash, so the burst starts committed
        // and becomes steerable as it settles. 150 ms sits inside the engine's
        // 200 ms `DASH_MAX_MS` hard bound, so the whole ramp is observable before
        // the dash exits. `clamp` holds the ratio in [0, 1] once `elapsedMs`
        // passes the ramp window.
        steerControl: runtime.clamp(
          runtime.div(runtime.read("elapsedMs"), 150.0),
          0.0,
          1.0,
        ),
        dashDrag: 0,
        cooldownMs: 600,
        airDashes: 1,
        preserveVertical: false,
      },
      crouch: {
        halfHeight: 0.4,
        eyeHeight: 0.3,
        transitionRate: 8.0,
      },
      viewFeel: {
        bob: {
          verticalFrequency: 0.25,
          lateralFrequency: 0.125,
          verticalAmplitude: 0.05,
          lateralAmplitude: 0.075,
          speedThreshold: 10.0,
        },
        tilt: {
          speedReference: 10,
          maxAngle: 4,
          tension: 15,
        },
      },
    },
  },
});
