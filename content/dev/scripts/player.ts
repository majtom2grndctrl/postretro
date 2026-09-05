import { defineEntity, runtime } from "postretro";
import { referencePistolEntity } from "./reference-pistol";
import { referenceShotgunEntity } from "./reference-shotgun";
import {
  referencePlasmaBoltEntity,
  referenceRocketEntity,
} from "./reference-projectiles";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    // The body-spanning hitbox makes the player a normal targetable presence.
    // It mirrors the 0.2 m-radius, 0.8 m-half-height movement capsule: feet at
    // y=0, head at y=1.6, and the 0.5 m camera eye lies inside its Y range.
    // Player weapon queries exclude their firing pawn, preventing that interior
    // eye ray from reporting a range-0 self-hit.
    health: {
      max: 100,
      hitbox: {
        halfExtents: [0.2, 0.8, 0.2],
        offset: [0, 0.8, 0],
      },
    },
    inventory: {
      // Projectile references live in the standard dev loadout, so any dev
      // map can fire both the sprite-bolt and model-plus-trail variants.
      loadout: [
        referenceShotgunEntity,
        referencePistolEntity,
        referencePlasmaBoltEntity,
        referenceRocketEntity,
      ],
    },
    mesh: {
      model: "models/exo_red/model.gltf",
      shadowOnly: true,
      // exo_red declares a `hand_r` socket for the runtime third-person weapon.
      animations: {
        idle: { clip: "idle", loop: true, crossfadeMs: 50 },
        walk_forward: {
          clip: "walk_forward",
          loop: true,
          crossfadeMs: 50,
          travelSpeed: 7.0,
        },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
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
        boostSpeed: 42.0,
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
        dashDrag: 0.1,
        cooldownMs: 600,
        airDashes: 1,
        preserveVertical: true,
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
