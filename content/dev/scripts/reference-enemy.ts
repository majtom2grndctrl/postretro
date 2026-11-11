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

// The committed slam cycle is 1,850 ms, longer than the slam's 1,800 ms
// cooldown, so a slam is always ready when its turn comes round.
const SLAM_WINDUP_MS = 250;
const SLAM_COMMIT_MS = 150;
const SLAM_RECOVER_MS = 1450;
// A jab plays its punch, whose strike lands about 250 ms in, then rests;
// strike plus rest covers the jab's 1,200 ms cooldown.
const JAB_STRIKE_MS = 450;
const JAB_REST_MS = 750;
// After its own slam the brute only jabs, longer than `heavy`'s 4 s refresh,
// so a second brute on the same target takes the next slam.
const PRESS_MS = 5000;

// The jab loop both fighting phases run: close to jab reach, jab when a
// `melee` turn is free, recover the punch, rest, and jab again. The jab
// activity is marked, so its turn ends as it returns to `stalk`.
const jabs: BehaviorGraphEnvelope = {
  initial: "stalk",
  activities: {
    stalk: { animation: "walk" },
    jab: {
      animation: "attack_jab",
      action: { attack: "jab" },
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
      {
        to: "stalk",
        when: brain.attacksFiredInActivity
          .ge(1)
          .and(brain.timeInActivityMs.ge(JAB_STRIKE_MS)),
      },
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
 * - the slam is a marked windup → commit → recover chain holding one turn;
 *   it draws two `melee` and the `heavy`, so jabs keep drawing the third
 *   `melee` while one brute slams.
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
        // The punch's strike lands about 250 ms in, with the slam's commit.
        // Stances replace this cycle with `smash` and `swipe`.
        attack_slam: {
          clip: "punch",
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
        jab: { draws: [melee], damage: 8, maxRange: JAB_RANGE, cooldownMs: 1200 },
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
                windup: { animation: "attack_slam", attackTurn: "slam" },
                commit: {
                  animation: "attack_slam",
                  action: { attack: "slam" },
                  attackTurn: "slam",
                },
                recover: { animation: "attack_slam", attackTurn: "slam" },
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
                recover: [
                  { to: "press", when: brain.timeInActivityMs.ge(SLAM_RECOVER_MS) },
                ],
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
