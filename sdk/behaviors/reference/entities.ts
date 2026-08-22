// Reference data-context entity descriptors for the rotator and damage-source
// behaviors, plus the SDK-owned E21 pose fixture. The dev-only reference enemy
// lives in `content/dev/scripts/reference-enemy.ts`.
//
// See: context/lib/scripting.md §2

import { brain, candidate, defineEntity } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

/** Classname for entities driven by `rotator_driver.{ts,luau}`. */
export const ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver";

/** Classname for entities targeted/observed by `damage_source.{ts,luau}`. */
export const DAMAGE_SOURCE_CLASSNAME = "game_damage_source";

/** Classname for the minimal E21 pose-modifier fixture enemy. */
export const POSE_FIXTURE_ENEMY_CLASSNAME = "pose_fixture_enemy";

const DETECTION_RANGE = 16;
const JAB_RANGE = 2;
const POSE_FIXTURE_AGGRO_RANGE = 50;

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
      attacks: {
        jab: { damage: 8, maxRange: JAB_RANGE, cooldownMs: 1200 },
      },
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
          action: { attack: "jab" },
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
];
