// The dev mod's map-placeable hierarchical behavior-statechart fixture.
//
// This is deliberately hand-authored beside `reference-enemy.luau`: the
// scripting-core twin parser test keeps their descriptors byte-for-byte equal
// after canonical conversion.

import { brain, defineEntity, runtime } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

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
const LEASH_RANGE = 100;
const RETURN_ARRIVAL_EPSILON = 1;

// The committed slam cycle is 1,850 ms, longer than the slam's 1,800 ms
// cooldown. A successful commit therefore leaves the counter-driven rotation
// able to fire on its next visit instead of permanently observing cooldown.
const SLAM_WINDUP_MS = 250;
const SLAM_COMMIT_MS = 150;
const SLAM_RECOVER_MS = 1450;

/**
 * The reference enemy exercises the full recursive statechart path:
 *
 * - `engage` runs a selector movement layer and a nested offense graph.
 * - offense approaches, jabs once, then enters a committed slam
 *   windup → commit → recover cycle.
 * - entering `jab` or `commit` edge-fires its action; the jab's successful
 *   fire advances `attacksFiredInActivity`, which selects the next slam.
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
    },
    // A single normal hit is enough to redirect this playtest fixture.
    tolerance: 4,
    mesh: {
      model: "models/reference_enemy_kaykit_knight/scene.gltf",
      animations: {
        idle: { clip: "Idle", loop: true },
        walk: {
          clip: "Walking_A",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 3,
        },
        attack_jab: {
          clip: "1H_Melee_Attack_Slice_Horizontal",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        attack_slam: {
          clip: "1H_Melee_Attack_Slice_Horizontal",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        // The reference model has no dedicated pain clip; this distinct state
        // still makes the authored startle observable to replication clients.
        flinch: {
          clip: "1H_Melee_Attack_Slice_Horizontal",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        death: {
          clip: "Death_A",
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
        jab: { damage: 8, maxRange: JAB_RANGE, cooldownMs: 1200 },
        slam: {
          damage: 14,
          maxRange: SLAM_RANGE,
          cooldownMs: 1800,
          engagementRadius: SLAM_RANGE,
        },
      },
      engagementRadius: JAB_RANGE,
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
            // At the slam standoff, release movement but retain the selected
            // target for offense; outside it, the fallback chases the target.
            move: [
              {
                when: brain.targetDistance.le(SLAM_RANGE),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "approach",
              activities: {
                approach: { animation: "walk" },
                jab: {
                  animation: "attack_jab",
                  action: { attack: "jab" },
                },
                windup: { animation: "attack_slam" },
                commit: {
                  animation: "attack_slam",
                  action: { attack: "slam" },
                },
                recover: { animation: "attack_slam" },
              },
              transitions: {
                approach: [
                  { to: "jab", when: brain.targetDistance.le(JAB_RANGE) },
                  { to: "windup", when: brain.targetDistance.le(SLAM_RANGE) },
                ],
                // The counter is read next tick, after the entry-edge jab fire.
                jab: [
                  {
                    to: "windup",
                    when: brain.attacksFiredInActivity.ge(1),
                  },
                ],
                windup: [
                  { to: "commit", when: brain.timeInActivityMs.ge(SLAM_WINDUP_MS) },
                ],
                commit: [
                  { to: "recover", when: brain.timeInActivityMs.ge(SLAM_COMMIT_MS) },
                ],
                recover: [
                  { to: "approach", when: brain.timeInActivityMs.ge(SLAM_RECOVER_MS) },
                ],
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
