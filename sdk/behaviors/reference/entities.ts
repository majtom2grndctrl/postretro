// Reference data-context entity descriptors: two bare placeholder archetypes,
// the SDK-owned E21 pose fixture, and a reference enemy whose two attacks
// share one stance. The dev-only reference enemy lives in
// `content/dev/scripts/reference-enemy.ts`.
//
// See: context/lib/scripting.md §2

import { brain, candidate, defaultToken, defineAttack, defineEntity, defineStance } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

/** Classname for a bare placeholder archetype, with no light and no emitter. */
export const ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver";

/** Classname for a bare placeholder archetype, with no light and no emitter. */
export const DAMAGE_SOURCE_CLASSNAME = "game_damage_source";

/** Classname for the minimal E21 pose-modifier fixture enemy. */
export const POSE_FIXTURE_ENEMY_CLASSNAME = "pose_fixture_enemy";

/** Classname for the reference enemy whose two attacks share one stance. */
export const STANCE_REFERENCE_ENEMY_CLASSNAME = "stance_reference_enemy";

const DETECTION_RANGE = 16;
const JAB_RANGE = 2;
const POSE_FIXTURE_AGGRO_RANGE = 50;
const STRIKE_RANGE = 2.5;

// An attack is a handle its binding names. `draws` names the attack tokens a
// turn for the jab takes from its target's allowance. `defaultToken` is what
// an attack without `draws` draws; a mod declares its own with
// `defineAttackToken`.
const jab = defineAttack({ draws: [defaultToken], damage: 8, maxRange: JAB_RANGE, cooldownMs: 1200 });

/**
 * A deliberately minimal AI fixture for E21 pose-modifier verification. Its
 * direct behavior surface is recursively enveloped so it keeps exercising the
 * production brain path without duplicating the dev reference enemy.
 */
export const poseFixtureEnemyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: POSE_FIXTURE_ENEMY_CLASSNAME,
  components: {
    health: {
      max: 60,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
      zoneMultipliers: { head: 2.5 },
    },
    mesh: {
      model: "models/pose-modifier-fixture/joint_zones.gltf",
      attachments: { hand_r: "models/attachment-marker/hand-prop.gltf" },
      animations: {
        idle: { clip: "Rest", loop: true },
        walk: { clip: "Rest", loop: true },
        attack: { clip: "Rest", loop: true },
        death: { clip: "Rest", loop: true },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // No `attacks` list: the behavior has the attacks its activities fire.
      engagementRadius: JAB_RANGE,
      candidateFilter: candidate.died.not().and(
        candidate.distance.le(POSE_FIXTURE_AGGRO_RANGE),
      ),
      activities: {
        idle: { animation: "idle", motion: "hold" },
        alert: { animation: "walk", motion: "chaseTarget" },
        attack: {
          animation: "attack",
          motion: "chaseTarget",
          action: { attack: jab },
        },
      },
      transitions: {
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          { to: "idle", when: brain.targetDied },
          { to: "idle", when: brain.targetDistance.gt(POSE_FIXTURE_AGGRO_RANGE) },
        ],
        idle: [
          {
            to: "attack",
            when: brain.acquisitionDue.and(brain.targetDistance.le(JAB_RANGE)),
          },
          {
            to: "alert",
            when: brain.acquisitionDue.and(brain.targetDistance.le(DETECTION_RANGE)),
          },
        ],
        alert: [
          { to: "attack", when: brain.targetDistance.le(JAB_RANGE) },
          { to: "idle", when: brain.targetDistance.gt(POSE_FIXTURE_AGGRO_RANGE) },
        ],
        attack: [{ to: "alert", when: brain.targetDistance.gt(JAB_RANGE) }],
      },
    },
  },
});

// One stance, two strikes. The raised guard tells the player a strike is
// coming; they cannot tell which one, or when it lands. Each is named by its
// binding: "guard", "chop", "thrust".
const guard = defineStance({
  animation: "guard",
  breakOn: { damage: 20, to: "stagger" }, // enough damage in the pose staggers it
});
const chop = defineAttack({
  damage: 8,
  maxRange: STRIKE_RANGE,
  cooldownMs: 1200,
  beginningStance: guard,
  release: { animation: "chop" }, // fires at the clip's `hit`, or on entry without one
});
const thrust = defineAttack({
  damage: 12,
  maxRange: STRIKE_RANGE,
  cooldownMs: 2000,
  beginningStance: guard,
  release: { animation: "thrust" },
});

/**
 * A reference for typed attacks and a shared stance. It rests in `guard` and
 * strikes `chop` or `thrust` from it; each strike holds the pose for a hidden,
 * per-strike time, then releases and ends back in `guard`.
 */
export const stanceReferenceEnemyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: STANCE_REFERENCE_ENEMY_CLASSNAME,
  components: {
    health: {
      max: 60,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
    },
    // The pose fixture's model: every state plays its no-op `Rest` clip, so
    // the graph is the reference, not the art.
    mesh: {
      model: "models/pose-modifier-fixture/joint_zones.gltf",
      animations: {
        idle: { clip: "Rest", loop: true },
        walk: { clip: "Rest", loop: true },
        guard: { clip: "Rest", loop: false },
        chop: { clip: "Rest", loop: false, interrupt: "snap" },
        thrust: { clip: "Rest", loop: false, interrupt: "snap" },
        stagger: { clip: "Rest", loop: false, interrupt: "snap" },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // Exhaustive when present: only these attacks exist. Omitted, the
      // behavior has the attacks its rows, actions and marks reference.
      attacks: [chop, thrust],
      activities: {
        idle: { animation: "idle", motion: "hold" },
        approach: { animation: "walk", motion: "chaseTarget" },
        stagger: { animation: "stagger", motion: "hold" },
      },
      transitions: {
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          { to: "idle", when: brain.targetDied },
        ],
        idle: [
          {
            to: "approach",
            when: brain.acquisitionDue.and(brain.targetDistance.le(DETECTION_RANGE)),
          },
        ],
        approach: [{ to: guard, when: brain.targetDistance.le(STRIKE_RANGE) }], // take the pose
        // Rows out of the rest, keyed by the stance's name. A row to an attack
        // also waits for its turn, its range and its cooldown.
        guard: [
          { to: chop, when: brain.activityRoll.lt(0.6) }, // one roll per entry of the pose
          { to: thrust }, // no `when`: always enabled
          { to: "approach", when: brain.targetDistance.gt(STRIKE_RANGE + 1) },
        ],
        stagger: [{ to: "approach", when: brain.timeInActivityMs.ge(500) }],
      },
    },
  },
});

/** Data-archetype entries used by the SDK reference behaviors. */
export const referenceEntities: EntityTypeDescriptor[] = [
  defineEntity({
    canonicalName: ROTATOR_DRIVER_CLASSNAME,
    components: { light: null, emitter: null },
  }),
  defineEntity({
    canonicalName: DAMAGE_SOURCE_CLASSNAME,
    components: { light: null, emitter: null },
  }),
  poseFixtureEnemyEntity,
  stanceReferenceEnemyEntity,
];
