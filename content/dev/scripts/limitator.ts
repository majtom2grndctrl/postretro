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
// In a fight it takes turns with its squad on the `ranged` token: it strafes
// around its target while it waits, fires a three-shot burst when a turn is
// free, then runs to a flanking slot before it waits again.
//
// Map-placeable via `"classname" "limitator"`.

import { attackTurnFree, brain, defineEntity } from "postretro";
import { ranged } from "./attack-tokens";

// Ranged-combat tuning. Unlike the melee `reference_enemy`, the limitator wants
// to hold a firing standoff rather than close to contact.
const DETECTION_RANGE = 50;
// Firing threshold: at or inside this distance, with a clear sightline, the
// enemy stops running the target down and starts its strafe-and-burst cycle.
// Kept well below DETECTION_RANGE so there is a wide band (FIRE..DETECTION) in
// which he actively runs the target down — otherwise, engaging already-in-range,
// he never visibly chases.
const FIRE_RANGE = 8;
// Break-off distance (hysteresis): once firing, only re-close after the target
// pulls past this. The FIRE..BREAK band keeps a target dancing on the boundary
// from endlessly restarting the cycle.
const BREAK_RANGE = 9;
// The combat-slot standoff the squad strafes, fires and repositions at — held
// strictly INSIDE FIRE_RANGE.
const STANDOFF_DISTANCE = 6;
// Leash: abandon the chase and walk home once dragged this far from the spawn
// anchor, then stand down. Set above DETECTION_RANGE so a target acquired near
// the detection edge isn't leashed the instant he starts closing.
const LEASH_RANGE = 70;
const RETURN_ARRIVAL_EPSILON = 1;

// Firing rhythm: strafe at least WAIT_MS, then burst when a `ranged` turn is
// free. The burst activity holds the turn; BURST_MS covers the weapon's three
// shots (240 ms), so none spills into `move`. Then reposition to a slot 60-120
// degrees around the target from where it fired, for MOVE_MS. BURST_MS + MOVE_MS
// outlasts the weapon's 1 s recovery, so the next burst is ready to fire.
// No reload beat — the available reload clip
// is a crouching animation that swings the right hand through a wide arc, which
// tips the rigidly-mounted rifle upside-down and into the chest. A reload that
// reads correctly needs a clip that keeps the gun hand steady, or a weapon
// animated as a skinned part rather than a rigid socket attachment.
const WAIT_MS = 400;
const BURST_MS = 450;
const MOVE_MS = 1500;
// A burst whose turn went to a squadmate asking on the same tick gives up.
const TURN_LOST_MS = 100;

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
    // One ordinary hit is enough to make this ranged fixture reconsider its target.
    tolerance: 4,
    mesh: {
      model: "models/limitator/model.gltf",
      attachments: {
        hand_r: "models/ar_4/model.gltf",
      },
      animations: {
        idle: { clip: "idle", loop: true },
        idle_aiming: { clip: "idle_aiming", loop: true },
        // The strafe's walk. Facing its target, the engine plays the
        // directional state matching its travel; each clip's stride matches
        // the forward walk's.
        walk: {
          clip: "walk_forward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_left: {
          clip: "walk_left",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_right: {
          clip: "walk_right",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_backward: {
          clip: "walk_backward",
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
        // Attack pose (Basic Shooter Pack): a fast recoil kick, looped through
        // the burst.
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
      // Scale the locomotion clip, the strafe's walk and its directional
      // variants, to actual XZ speed so the stride reads at the agent's real
      // pace; the run's authored stride already matches `moveSpeed`. Required
      // alongside `behavior` to materialize the nav agent that grounds and
      // moves the entity.
      locomotion: { speedScale: true },
    },
    // The behavior graph is what materializes the engine brain AND the nav
    // agent; without it the entity has no floor grounding and never ticks.
    behavior: {
      initial: "idle",
      moveSpeed: 4,
      attacks: {
        // The weapon owns damage, reach, and cooldown; this entry keeps only AI
        // positioning and its draw. Its 12 m reach covers the fire band
        // (entered inside FIRE_RANGE and held to BREAK_RANGE) with a small
        // safety margin.
        shoot: {
          draws: [ranged],
          weapon: "enemy_rifle_burst",
          standoffDistance: STANDOFF_DISTANCE,
        },
      },
      // The strafe ring and reposition slots sit at this radius, the shoot
      // standoff, so the squad circles at its firing distance.
      engagementRadius: STANDOFF_DISTANCE,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        // The composite's `walk` names the graph's locomotion clip, so the
        // strafe's walk and its directional variants scale to its speed.
        engage: {
          animation: "walk",
          layers: {
            // Only `close` takes its motion from here: run the target down
            // until it is inside FIRE_RANGE with a clear sightline.
            move: [
              {
                when: brain.targetDistance
                  .le(FIRE_RANGE)
                  .and(brain.targetVisible),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "close",
              activities: {
                close: { animation: "run" },
                // Strafe around the target, facing it, while a squadmate shoots.
                wait: { animation: "walk", motion: "strafe" },
                // Holds the `ranged` turn for the whole burst.
                burst: {
                  animation: "shoot",
                  motion: "hold",
                  action: { attack: "shoot" },
                  attackTurn: "shoot",
                },
                // Fire, then relocate: a slot to the side of where it fired.
                move: {
                  animation: "run",
                  motion: "reposition",
                  facing: "travel",
                  reposition: { arc: [60, 120] },
                },
              },
              transitions: {
                close: [
                  {
                    to: "wait",
                    when: brain.targetDistance
                      .le(FIRE_RANGE)
                      .and(brain.targetVisible),
                  },
                ],
                // Losing the sightline or the range returns to `close`, after
                // the LOS grace window; distance hysteresis keeps a target on
                // the boundary from churning.
                wait: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  {
                    to: "burst",
                    when: brain.timeInActivityMs
                      .ge(WAIT_MS)
                      .and(attackTurnFree("shoot")),
                  },
                ],
                burst: [
                  {
                    to: "move",
                    when: brain.attacksFiredInActivity
                      .ge(1)
                      .and(brain.timeInActivityMs.ge(BURST_MS)),
                  },
                  {
                    to: "wait",
                    when: brain.hasAttackTurn
                      .not()
                      .and(brain.timeInActivityMs.ge(TURN_LOST_MS)),
                  },
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                ],
                move: [{ to: "wait", when: brain.timeInActivityMs.ge(MOVE_MS) }],
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
          // A selected neutral target is an engine-admitted over-tolerance
          // retaliator, so it remains engageable like an ordinary hostile.
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
