// The dev mod's map-placeable hierarchical behavior-statechart fixture.
//
// This is deliberately hand-authored beside `reference-enemy.luau`: the
// scripting-core twin parser test keeps their descriptors byte-for-byte equal
// after canonical conversion.

import {
  attackTurnFree,
  attackTurnHeld,
  brain,
  defense,
  defineEntity,
  runtime,
} from "postretro";
import type { BehaviorGraphEnvelope, EntityTypeDescriptor } from "postretro";
import { heavy, melee } from "./attack-tokens";
import { corrosive } from "./damage-types";
import { armor } from "./defenses";

/** Classname used by dev maps to place this health + mesh + behavior archetype. */
export const REFERENCE_ENEMY_CLASSNAME = "reference_enemy";

const DETECTION_RANGE = 16;
const ALERT_MS = 4000;
const SEARCH_AFTER_MS = 800;
const ARRIVE = 1;
const HALF_PI = Math.PI / 2;
const STARTLE_MS = 200;
const JAB_RANGE = 2;
const SLAM_RANGE = 3.5;
// Combat slots sit inside jab reach, so the brute fights close enough to jab.
const CLOSE_STANDOFF = 1.5;
const LEASH_RANGE = 100;
const RETURN_ARRIVAL_EPSILON = 1;

// `punch`, on the `mixamo_brute` library, runs 1,100 ms. Its swipe connects at
// its `hit` event, 267 ms in; the rest is follow-through and the return to
// rest. The jab fires at `hit`.
// `smash`, the jump attack, runs 3,700 ms. It crouches by 400 ms, where
// `smash_windup` cuts it. Its `hit`, 1,633 ms in, is the hands slamming down
// past camera height, a frame before they reach the floor. The slam's three
// phases share `attack_slam`, so its smash runs on from the windup's entry:
// the windup crouches, the commit fires at `hit` and holds 100 ms past it, and
// the recovery leaves once the smash has played through. The commit must still
// be active when the smash reaches `hit`, or the slam never fires.
const SMASH_CROUCHED_MS = 400;
const SMASH_HIT_MS = 1633;
const SLAM_WINDUP_MS = SMASH_CROUCHED_MS;
const SLAM_COMMIT_MS = SMASH_HIT_MS - SMASH_CROUCHED_MS + 100;
// A jab fires at the punch's `hit`, leaves once the punch has played through,
// then rests JAB_REST_MS before it may jab again. A target stepping out of
// reach can end a jab on the tick after its fire; it then rests and plays a
// fresh punch to `hit`, so the soonest a re-entered jab's moment follows a
// fire is 1 + 9 + 17 = 27 ticks (450 ms) on whole 60 Hz ticks. The cooldown
// sits below JAB_REST_MS plus the punch's `hit` (417 ms), so no jab's moment
// meets it.
const JAB_REST_MS = 150;
const JAB_COOLDOWN_MS = 400;
// After its own slam the brute only jabs, longer than `heavy`'s 4 s refresh,
// so a second brute on the same target takes the next slam, and far longer
// than the slam's 1,800 ms cooldown, so a slam is always ready when its turn
// comes round. `press`'s exit is an outer row, so one passing during a jab's
// punch cuts that jab before `hit`, and it never fires.
const PRESS_MS = 5000;

// The jab loop both fighting phases run: close to jab reach, jab when a
// `melee` turn is free, play the punch through, rest, and jab again. The jab
// activity is marked, so its turn ends as it returns to `stalk`. A target that
// steps out of reach before `hit` ends the jab, and it misses.
const jabs: BehaviorGraphEnvelope = {
  initial: "stalk",
  activities: {
    stalk: { animation: "walk" },
    jab: {
      animation: "attack_jab",
      action: { attack: "jab", at: "hit" },
      attackTurn: "jab",
    },
  },
  transitions: {
    stalk: [
      {
        to: "jab",
        when: brain.targetDistance
          .le(JAB_RANGE)
          .and(brain.timeInActivityMs.ge(JAB_REST_MS))
          .and(attackTurnFree("jab")),
      },
    ],
    jab: [
      { to: "stalk", when: brain.animationDone },
      { to: "stalk", when: brain.targetDistance.gt(JAB_RANGE) },
    ],
  },
};

/**
 * The reference enemy exercises the full recursive statechart path and the
 * attack budget:
 *
 * - `engage` runs a selector movement layer and a nested offense graph.
 * - `hunt` and `press` each nest the jab loop. `hunt` starts a slam the
 *   moment `attackTurnFree("slam")` says its turn would be granted.
 * - the slam is a marked windup → commit → recover chain holding one turn,
 *   one smash across all three; it draws two `melee` and the `heavy`, so jabs
 *   keep drawing the third `melee` while one brute slams.
 * - after its slam the brute presses with jabs only, so with two brutes on
 *   one target the slams alternate as `heavy` refreshes.
 */
export const referenceEnemyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: REFERENCE_ENEMY_CLASSNAME,
  components: {
    health: {
      max: 70,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
      zoneMultipliers: {
        head: 2.5,
        leg: 0.5,
      },
      // Armored, and its flesh shock-resistant (health only).
      layers: [{ layer: armor, capacity: 120 }],
      resist: { shock: 0.5 },
    },
    // A single normal hit is enough to redirect this playtest fixture.
    tolerance: 4,
    // The Mixamo Mutant, on the `mixamo_brute` clip library.
    mesh: {
      model: "models/mutant/model.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
        // Every move runs at `moveSpeed`, so locomotion plays the run; its
        // planted foot travels about 2.2 m/s.
        walk: {
          clip: "run",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 2.2,
        },
        attack_jab: {
          clip: "punch",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        // The smash's hands pass camera height at its `hit`, 1,633 ms in,
        // during the slam's commit. Stances split it into `smash_windup` and
        // `smash_strike`.
        attack_slam: {
          clip: "smash",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        flinch: {
          clip: "hit_reaction",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        death: {
          clip: "death",
          loop: false,
          crossfadeMs: 120,
          interrupt: "snap",
        },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      attacks: {
        jab: {
          draws: [melee],
          damage: 8,
          maxRange: JAB_RANGE,
          cooldownMs: JAB_COOLDOWN_MS,
        },
        slam: {
          draws: [melee.with({ cost: 2 }), heavy],
          damage: 14,
          maxRange: SLAM_RANGE,
          cooldownMs: 1800,
        },
        claw: { damage: 15, damageType: corrosive, maxRange: 2, cooldownMs: 800 },
      },
      engagementRadius: CLOSE_STANDOFF,
      patrol: {
        mode: "pingPong",
        points: [[0, 0], [6, 0], [6, 6]],
      },
      activities: {
        idle: { animation: "idle", motion: "hold" },
        patrol: { animation: "walk", motion: "patrol" },
        engage: {
          animation: "walk",
          layers: {
            // In jab reach, release movement but retain the selected target
            // for offense; outside it, the fallback chases the target.
            move: [
              {
                when: brain.targetDistance.le(JAB_RANGE),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "hunt",
              activities: {
                hunt: { layers: { offense: jabs } },
                press: { layers: { offense: jabs } },
                // The slam stands its ground through the whole smash rather
                // than chasing, so the brute does not walk off its target
                // before `hit`.
                windup: {
                  animation: "attack_slam",
                  motion: "hold",
                  attackTurn: "slam",
                },
                commit: {
                  animation: "attack_slam",
                  motion: "hold",
                  action: { attack: "slam", at: "hit" },
                  attackTurn: "slam",
                },
                recover: {
                  animation: "attack_slam",
                  motion: "hold",
                  attackTurn: "slam",
                },
              },
              transitions: {
                hunt: [
                  {
                    to: "windup",
                    when: brain.targetDistance
                      .le(SLAM_RANGE)
                      .and(attackTurnFree("slam")),
                  },
                ],
                windup: [
                  {
                    to: "commit",
                    when: brain.timeInActivityMs
                      .ge(SLAM_WINDUP_MS)
                      .and(attackTurnHeld("slam")),
                  },
                  // Another brute asked on the same tick and won the turn.
                  { to: "hunt", when: brain.timeInActivityMs.ge(SLAM_WINDUP_MS) },
                ],
                commit: [
                  { to: "recover", when: brain.timeInActivityMs.ge(SLAM_COMMIT_MS) },
                ],
                recover: [{ to: "press", when: brain.animationDone }],
                press: [{ to: "hunt", when: brain.timeInActivityMs.ge(PRESS_MS) }],
              },
            },
          },
        },
        investigate: { animation: "walk", motion: "moveToLastKnown" },
        startle: { animation: "flinch", motion: "hold" },
        retreat: { animation: "walk", motion: "moveToAnchor" },
      },
      transitions: {
        "*": [
          // A missing target is not enough to stand down: recent damage can
          // still point at an unreached memory.
          {
            to: "patrol",
            when: brain.hasTarget
              .not()
              .and(brain.timeSinceDamageMs.gt(ALERT_MS))
              .and(brain.distanceToLastKnown.le(ARRIVE)),
          },
          // A selected neutral target is an engine-admitted over-tolerance
          // retaliator, so it remains engageable like an ordinary hostile.
          {
            to: "startle",
            when: brain.damageSourceKnown
              .and(brain.timeSinceDamageMs.le(STARTLE_MS))
              .and(
                brain.damageBearing
                  .gt(HALF_PI)
                  .or(brain.damageBearing.lt(-HALF_PI)),
              ),
          },
        ],
        idle: [{ to: "patrol", when: runtime.constant(true) }],
        patrol: [
          {
            to: "engage",
            when: brain.acquisitionDue.and(
              brain.targetDistance.le(DETECTION_RANGE),
            ),
          },
          {
            to: "investigate",
            // Generic damage recency also resets for contextless reactions;
            // provenance proves this hit supplied the remembered position.
            when: brain.damageSourceKnown
              .and(brain.timeSinceDamageMs.le(ALERT_MS))
              .and(brain.distanceToLastKnown.gt(ARRIVE)),
          },
        ],
        engage: [
          { to: "retreat", when: brain.distanceFromAnchor.gt(LEASH_RANGE) },
          // Its own layer, as `state(name)` reads its own state.
          { to: "retreat", when: defense(armor).current.le(0) },
          {
            to: "investigate",
            when: brain.timeSinceTargetVisible.ge(SEARCH_AFTER_MS),
          },
        ],
        investigate: [
          { to: "engage", when: brain.targetVisible },
          { to: "patrol", when: brain.distanceToLastKnown.le(ARRIVE) },
        ],
        startle: [
          { to: "patrol", when: brain.timeInActivityMs.ge(STARTLE_MS) },
        ],
        retreat: [
          { to: "patrol", when: brain.distanceFromAnchor.le(RETURN_ARRIVAL_EPSILON) },
        ],
      },
    },
  },
});
