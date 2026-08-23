// DEMO CONTENT — `limitator` descriptor.
//
// A ranged-combat enemy on the Mixamo skeleton (converted from Limitator.fbx via
// mixamo_to_gltf.py). Locomotion / idle / aim / death clips come from the Mixamo
// "Pro Rifle Pack"; the attack (`firing_rifle`) plus `reloading` and
// `hit_reaction` are merged in from the Mixamo "Basic Shooter Pack" — the Pro
// Rifle Pack has no firing clip. The model is baked facing engine +Z (yaw 90)
// so the AI facing (MESH_FORWARD = +Z) turns his front toward the target.
//
// The AR_4 rifle (Quaternius "Modular Sci Fi Guns", CC0) mounts on the `hand_r`
// socket that mixamo_to_gltf.py tagged on the skeleton, with its grip-relative
// orientation re-tuned for this skeleton bake so the barrel points forward.
//
// Map-placeable via `"classname" "limitator"`.

import { brain, defineEntity } from "postretro";

// Ranged-combat tuning. Unlike the melee `reference_enemy`, the limitator wants
// to hold a firing standoff rather than close to contact.
const DETECTION_RANGE = 50;
// Firing threshold: at or inside this distance the move layer releases movement
// and the enemy plants to shoot. Kept well below DETECTION_RANGE so there is a
// wide band (FIRE..DETECTION) in which he actively runs the target down —
// otherwise, engaging already-in-range, he never visibly chases.
const FIRE_RANGE = 8;
// Break-off distance (hysteresis): once firing, only re-close after the target
// pulls past this. The FIRE..BREAK band keeps a target dancing on the boundary
// from endlessly resetting the aim timer.
const BREAK_RANGE = 9;
// Combat-positioning slot ring — held strictly INSIDE FIRE_RANGE. The E10
// positioning system seats the agent at ~engagementRadius from the target and
// the steering hard-stop leaves it a fraction beyond that; if the ring equalled
// the fire guard the agent would settle just outside `le(FIRE_RANGE)` and never
// fire (the standoff deadlock). A ring below the guard guarantees he crosses in.
const ENGAGEMENT_RADIUS = 6;
// Leash: abandon the chase and walk home once dragged this far from the spawn
// anchor, then stand down. Set above DETECTION_RANGE so a target acquired near
// the detection edge isn't leashed the instant he starts closing.
const LEASH_RANGE = 70;
const RETURN_ARRIVAL_EPSILON = 1;

// Firing rhythm: alternate aim -> fire. Each entry into `fire` edge-fires one
// shot (gated by the attack cooldown); AIM_MS + FIRE_MS sits just above the
// cooldown so every fire entry lands. No reload beat — the available reload clip
// is a crouching animation that swings the right hand through a wide arc, which
// tips the rigidly-mounted rifle upside-down and into the chest. A reload that
// reads correctly needs a clip that keeps the gun hand steady, or a weapon
// animated as a skinned part rather than a rigid socket attachment.
const AIM_MS = 550;
const FIRE_MS = 250;

export const limitatorEntity = defineEntity({
  canonicalName: "limitator",
  components: {
    health: {
      max: 100,
      // ~1.82 m tall (model baked at scale 0.68); hitbox spans feet to head,
      // centered on the torso. The head primitive tops out at y≈1.82, so the
      // box half-height/offset are 0.91 (not the body primitive's 0.81 max) —
      // otherwise upper-head shots above y≈1.62 miss the health hitbox.
      hitbox: {
        halfExtents: [0.27, 0.91, 0.27],
        offset: [0, 0.91, 0],
      },
      zoneMultipliers: {
        head: 2.5,
        leg: 0.5,
      },
    },
    mesh: {
      model: "models/limitator/model.gltf",
      attachments: {
        hand_r: "models/ar_4/model.gltf",
      },
      animations: {
        idle: { clip: "idle", loop: true },
        idle_aiming: { clip: "idle_aiming", loop: true },
        walk: {
          clip: "walk_forward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        run: {
          clip: "run_forward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 4,
        },
        // Attack pose (Basic Shooter Pack): a fast recoil kick. Looped so a fire
        // state re-entered every cooldown restarts the kick with a quick blend.
        shoot: {
          clip: "firing_rifle",
          loop: true,
          crossfadeMs: 80,
        },
        // Reload clip available but NOT wired: it is a crouching reload that
        // swings the gun hand through a wide arc, tipping the rigidly-mounted AR
        // upside-down and into the chest. Needs a steady-hand reload clip (or a
        // skinned weapon) before it can drive a reload beat.
        reload: { clip: "reloading", loop: false, crossfadeMs: 150 },
        // Flinch clip is available but NOT wired: an impact-policy `playAnim`
        // only switches the state, it does not auto-recover, and the engine's
        // only recovery path (`is_downed_for_recovery`) applies solely at zero
        // HP with a pending revive. So a non-lethal flinch freezes on its last
        // frame whenever the enemy then goes idle (target lost / player dead),
        // which reads as a broken pose. Re-wiring needs an engine-side
        // "stagger N ms then resume" impact effect.
        hit_reaction: {
          clip: "hit_reaction",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        death: {
          clip: "death_from_front_headshot",
          loop: false,
          crossfadeMs: 120,
          interrupt: "snap",
        },
      },
      defaultState: "idle",
      // Scale the travel clips to actual XZ speed so the run cycle reads at the
      // agent's real pace. Required alongside `behavior` to materialize the nav
      // agent that grounds and moves the entity.
      locomotion: { speedScale: true },
    },
    // The behavior graph is what materializes the engine brain AND the nav
    // agent; without it the entity has no floor grounding and never ticks.
    behavior: {
      initial: "idle",
      moveSpeed: 4,
      attacks: {
        // Hitscan-style ranged shot: long reach, held at a standoff. Cooldown
        // sits just under AIM_MS + FIRE_MS so each fire-state entry lands.
        // maxRange only needs to cover the fire band (a shot can only be issued
        // from the `fire` state, entered inside FIRE_RANGE and held to
        // BREAK_RANGE); a small margin past BREAK_RANGE is defense in depth.
        shoot: {
          damage: 10,
          maxRange: BREAK_RANGE + 3,
          cooldownMs: 750,
          engagementRadius: ENGAGEMENT_RADIUS,
        },
      },
      engagementRadius: ENGAGEMENT_RADIUS,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "run",
          layers: {
            // Plant on the firing line once inside FIRE_RANGE; otherwise run the
            // target back into range. The hold guard (FIRE_RANGE) sits above the
            // slot ring (ENGAGEMENT_RADIUS) so he settles comfortably inside the
            // fire threshold rather than deadlocking just outside it.
            move: [
              { when: brain.targetDistance.le(FIRE_RANGE), motion: "hold" },
              "chaseTarget",
            ],
            offense: {
              initial: "close",
              activities: {
                // Run in until within FIRE_RANGE, then alternate aim/fire so
                // each fire entry edge-fires the shot under its cooldown.
                close: { animation: "run" },
                aim: { animation: "idle_aiming" },
                fire: { animation: "shoot", action: { attack: "shoot" } },
              },
              transitions: {
                // Enter the firing cycle inside FIRE_RANGE; only drop back to
                // `close` once the target pulls past BREAK_RANGE, so a target
                // hovering at the threshold keeps him firing instead of churning.
                close: [
                  { to: "aim", when: brain.targetDistance.le(FIRE_RANGE) },
                ],
                aim: [
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: "fire", when: brain.timeInActivityMs.ge(AIM_MS) },
                ],
                fire: [
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: "aim", when: brain.timeInActivityMs.ge(FIRE_MS) },
                ],
              },
            },
          },
        },
        // Leashed: walk back to the spawn anchor after being dragged too far.
        retreat: { animation: "run", motion: "moveToAnchor" },
      },
      transitions: {
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          { to: "idle", when: brain.targetHostile.not() },
        ],
        idle: [
          {
            to: "engage",
            when: brain.acquisitionDue.and(
              brain.targetDistance.le(DETECTION_RANGE),
            ),
          },
        ],
        // Give up the chase once dragged past the leash; stand down at home.
        engage: [
          { to: "retreat", when: brain.distanceFromAnchor.gt(LEASH_RANGE) },
        ],
        retreat: [
          { to: "idle", when: brain.distanceFromAnchor.le(RETURN_ARRIVAL_EPSILON) },
        ],
      },
    },
  },
});
