// Reference data-context entity descriptors for the rotator and damage-source
// reference behaviors, plus the map-placeable reference enemy. Spread
// `referenceEntities` into `ModManifest.entities` to register the archetypes.
//
// See: context/lib/scripting.md §2

import { brain, candidate, defineEntity, runtime } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

/** Classname for entities driven by `rotator_driver.{ts,luau}`. */
export const ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver";

/** Classname for entities targeted/observed by `damage_source.{ts,luau}`. */
export const DAMAGE_SOURCE_CLASSNAME = "game_damage_source";

/**
 * Classname for the map-placeable reference enemy
 * (`health` + `mesh` + `behavior`).
 */
export const REFERENCE_ENEMY_CLASSNAME = "reference_enemy";

/** Classname for the minimal E21 pose-modifier fixture enemy. */
export const POSE_FIXTURE_ENEMY_CLASSNAME = "pose_fixture_enemy";

/** Distance at which the reference enemy notices a player, in metres. */
const REFERENCE_DETECTION_RANGE = 16;
/** Distance within which its melee swing connects, in metres. */
const REFERENCE_ATTACK_RANGE = 2;
/** Authored distance from the spawn anchor that begins a retreat, in metres. */
const REFERENCE_LEASH_RANGE = 20;
/**
 * The retreat-to-patrol threshold, in metres. It deliberately exceeds the
 * engine's 0.5 m position-goal arrival epsilon: a smaller guard would wedge
 * after steering clears at that epsilon.
 */
const REFERENCE_RETURN_ARRIVAL_EPSILON = 1;
/** Acquisition and stand-down radius for the separate pose-fixture graph. */
const POSE_FIXTURE_AGGRO_RANGE = 50;

/**
 * The map-placeable reference enemy: a full health + animated-mesh + behavior-
 * graph archetype that exercises the M10 enemy loop end to end. It is directly
 * placeable from a `.map` via `"classname" "reference_enemy"` because it carries
 * `components.health` and `components.mesh` (the canonicalName dispatch keys off
 * those placeable components).
 *
 * Model: the CC0 KayKit "Adventurers" Knight (Kay Lousberg), converted to the
 * engine's external-glTF layout under
 * `content/dev/models/reference_enemy_kaykit_knight/` and pruned to the four
 * clips named below. See that folder's `license.txt`.
 *
 * The `mesh.animations` keys are author-defined names; each maps to one of the
 * model's real clip names. Every `behavior.states.*.animation` names one of
 * them — the cross-component link the brain drives each tick.
 *
 * The graph is the reference authoring of an untargeted patrol that engages,
 * retreats to its spawn anchor, and resumes its route:
 *
 * ```text
 *   ANY     --(!hasTarget || !targetHostile)--> patrol (interrupt)
 *   patrol  --(acquisitionDue && dist <= detectionRange)--> alert
 *   alert   --(dist <= attackRange)--> attack
 *   alert/attack --(distanceFromAnchor > leash)--> retreat
 *   retreat --(distanceFromAnchor <= arrivalEpsilon)--> patrol
 * ```
 *
 * Authoring notes worth copying:
 *
 * - **Anchor and patrol.** The home anchor is this entity's spawn position, so
 *   the anchor-relative route works wherever the map places it. The cursor is
 *   brain state, not state-entry state: leaving and re-entering `patrol`
 *   resumes the route instead of restarting it.
 * - **Stand down into the active untargeted state.** Both any-state interrupts
 *   target `patrol`, the state this graph rests in. A stand-down to some other
 *   state would re-fire every tick after returning to patrol and oscillate.
 *   The `not hasTarget` row must be first. The friendly-flip row uses
 *   `select(targetHostile, false, true)`, which is true for *both* friendly and
 *   untargeted targets (unlike `targetDied`), so it must follow that row and
 *   share its destination.
 * - **Fresh acquisition is strided.** `targetDistance`, `targetHostile`, and
 *   `targetReachable` are target-side facts. On a non-engaged patrol tick
 *   between scans they hold no-target values, so detection must conjunct
 *   `acquisitionDue`. `targetReachable` exists too, but its pathfinder verdict
 *   has a known wraparound limitation; this reference intentionally omits the
 *   reachability waiting demo until that pursuit fix lands.
 * - **Leash and arrival are authored.** There is no engine leash field: this
 *   graph enters `retreat` through `distanceFromAnchor`. Its return guard must
 *   be at least the engine position-goal arrival epsilon (0.5 m); a smaller
 *   threshold wedges because movement clears at the engine epsilon first.
 *
 * There is no `death` state: death is not a graph transition. The engine's
 * death sweep latches a zero-HP enemy and the authored impact policy plays the
 * `death` mesh clip and despawns after its own delay — despawn timing belongs
 * entirely to that policy's `despawn` effect, and the behavior block carries no
 * despawn field of its own.
 *
 * `@state.faction` is intentionally absent from this graph. It is an opaque,
 * interim identity seed underneath the durable `targetHostile` fact; write
 * policy against the fact rather than depending on the numeric representation.
 */
export const referenceEnemyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: REFERENCE_ENEMY_CLASSNAME,
  components: {
    // Hit points + a hitscan hitbox so the shipped reference pistol's ray can
    // target and kill it. ~1.8 m tall human silhouette; Y-up, so the middle
    // half-extent is the vertical half-height and the offset lifts the box from
    // the foot-level transform origin to mid-body.
    health: {
      max: 70,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
      // A headshot deals 2.5x, a leg shot 0.5x; unlisted zones apply 1.0.
      zoneMultipliers: {
        head: 2.5,
        leg: 0.5,
      },
    },
    // Animated skinned mesh. The four state names are the engine-author vocab;
    // each `clip` is one of the model's real (pruned) clip names.
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
        attack: {
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
      locomotion: {
        speedScale: true,
      },
    },
    // The behavior state graph. Ranges are in metres; cooldown in ms; moveSpeed
    // in m/s. Every `animation` names a `mesh.animations` key above.
    behavior: {
      initial: "patrol",
      moveSpeed: 3,
      attack: { damage: 8, range: REFERENCE_ATTACK_RANGE, cooldownMs: 1200 },
      // Where engaged chasers STAND: the radius of the ring of combat slots the
      // engine spreads them around the target. Pure spacing, and distinct from
      // `attack.range` above, which gates DAMAGE and nothing else. Omitting it
      // falls back to `attack.range`, which is the value used here — it is
      // authored explicitly anyway, because the two are separate knobs and a
      // graph that later retunes its swing reach should not silently re-space
      // its pack. A pure-pursuit graph (`chaseTarget`, no `action`) has no
      // `attack.range` to fall back on and wants this field outright.
      engagementRadius: REFERENCE_ATTACK_RANGE,
      patrol: {
        mode: "pingPong",
        points: [[0, 0], [6, 0], [6, 6]],
      },
      // Both stand-downs target the untargeted-active resting state. They are
      // skipped while already patrolling, so the cursor keeps advancing.
      interrupts: [
        {
          to: "patrol",
          when: runtime.select(brain.hasTarget, false, true),
        },
        {
          to: "patrol",
          when: runtime.select(brain.targetHostile, false, true),
        },
      ],
      states: {
        patrol: {
          animation: "walk",
          motion: "patrol",
          transitions: [
            {
              to: "alert",
              when: runtime.select(
                brain.acquisitionDue,
                runtime.le(brain.targetDistance, REFERENCE_DETECTION_RANGE),
                false,
              ),
            },
          ],
        },
        alert: {
          animation: "walk",
          motion: "chaseTarget",
          transitions: [
            {
              to: "attack",
              when: runtime.le(brain.targetDistance, REFERENCE_ATTACK_RANGE),
            },
            {
              to: "retreat",
              when: runtime.gt(brain.distanceFromAnchor, REFERENCE_LEASH_RANGE),
            },
          ],
        },
        // Contact damage on the graph's `attack` cooldown while closing the
        // last metre.
        attack: {
          animation: "attack",
          motion: "chaseTarget",
          action: "attack",
          transitions: [
            {
              to: "retreat",
              when: runtime.gt(brain.distanceFromAnchor, REFERENCE_LEASH_RANGE),
            },
            {
              to: "alert",
              when: runtime.gt(brain.targetDistance, REFERENCE_ATTACK_RANGE),
            },
          ],
        },
        // Retreat never relies on target facts: this position-goal state is
        // non-engaged, drops its target, and returns to the persisted patrol.
        retreat: {
          animation: "walk",
          motion: "moveToAnchor",
          transitions: [
            {
              to: "patrol",
              when: runtime.le(
                brain.distanceFromAnchor,
                REFERENCE_RETURN_ARRIVAL_EPSILON,
              ),
            },
          ],
        },
      },
    },
  },
});

/**
 * A deliberately minimal AI fixture for E21 pose-modifier verification. Its
 * model is the content-facing copy of the model crate's three-joint
 * `joint_zones` test fixture, with a no-op clip so target acquisition supplies
 * the animated mesh's pose inputs. This is a triangle marker, not production
 * character art.
 *
 * It keeps a minimal direct behavior graph so its animated mesh receives the
 * production brain inputs without duplicating the reference enemy's patrol.
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
      zoneMultipliers: {
        head: 2.5,
      },
    },
    mesh: {
      model: "models/pose-modifier-fixture/joint_zones.gltf",
      attachments: {
        hand_r: "models/attachment-marker/hand-prop.gltf",
      },
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
      attack: { damage: 8, range: REFERENCE_ATTACK_RANGE, cooldownMs: 1200 },
      engagementRadius: REFERENCE_ATTACK_RANGE,
      candidateFilter: runtime.select(
        candidate.died,
        false,
        runtime.le(candidate.distance, POSE_FIXTURE_AGGRO_RANGE),
      ),
      interrupts: [
        { to: "idle", when: runtime.select(brain.hasTarget, false, true) },
        { to: "idle", when: brain.targetDied },
        {
          to: "idle",
          when: runtime.gt(brain.targetDistance, POSE_FIXTURE_AGGRO_RANGE),
        },
      ],
      states: {
        idle: {
          animation: "idle",
          motion: "hold",
          transitions: [
            {
              to: "attack",
              when: runtime.select(
                brain.acquisitionDue,
                runtime.le(brain.targetDistance, REFERENCE_ATTACK_RANGE),
                false,
              ),
            },
            {
              to: "alert",
              when: runtime.select(
                brain.acquisitionDue,
                runtime.le(brain.targetDistance, REFERENCE_DETECTION_RANGE),
                false,
              ),
            },
          ],
        },
        alert: {
          animation: "walk",
          motion: "chaseTarget",
          transitions: [
            {
              to: "attack",
              when: runtime.le(brain.targetDistance, REFERENCE_ATTACK_RANGE),
            },
            {
              to: "idle",
              when: runtime.gt(brain.targetDistance, POSE_FIXTURE_AGGRO_RANGE),
            },
          ],
        },
        attack: {
          animation: "attack",
          motion: "chaseTarget",
          action: "attack",
          transitions: [
            {
              to: "alert",
              when: runtime.gt(brain.targetDistance, REFERENCE_ATTACK_RANGE),
            },
          ],
        },
      },
    },
  },
});

/**
 * Data-archetype entries used by the reference behaviors. The rotator and
 * damage-source entries are pure tag/transform carriers; the behaviors locate
 * their work via `worldQuery` filters on tags authored on the placement. The
 * reference enemy is a full health + mesh + behavior archetype, map-placeable
 * by its `canonicalName`.
 *
 * Spread into `ModManifest.entities`.
 */
export const referenceEntities: EntityTypeDescriptor[] = [
  defineEntity({
    canonicalName: ROTATOR_DRIVER_CLASSNAME,
    components: { light: null, emitter: null },
  }),
  defineEntity({
    canonicalName: DAMAGE_SOURCE_CLASSNAME,
    components: { light: null, emitter: null },
  }),
  referenceEnemyEntity,
  poseFixtureEnemyEntity,
];
