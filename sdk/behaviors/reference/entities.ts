// Reference data-context entity descriptors for the rotator and damage-source
// reference behaviors, plus the map-placeable reference enemy. Spread
// `referenceEntities` into `ModManifest.entities` to register the archetypes.
//
// See: context/lib/scripting.md §2

import { brain, defineEntity, runtime } from "postretro";
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
/** Distance past which it gives up and returns to rest, in metres. */
const REFERENCE_LEASH_RANGE = 50;

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
 * The graph is the reference authoring of the classic three-state pursuit
 * shape:
 *
 * ```text
 *   idle  --(acquisitionDue && dist <= attackRange)-->  attack
 *   idle  --(acquisitionDue && dist <= detectionRange)--> alert
 *   alert --(dist <= attackRange)-->                    attack
 *   alert --(dist >  leashRange)-->                     idle
 *   attack --(dist >  attackRange)-->                   alert
 * ```
 *
 * Two authoring notes worth copying:
 *
 * - **`acquisitionDue` conjunction.** Detection is time-sliced by the engine's
 *   think stride, so both `idle` edges only fire on an acquisition tick. The IR
 *   has no `and` opcode yet, so the conjunction is spelled
 *   `select(cond, inner, false)`. The attack-range and leash edges are
 *   deliberately NOT gated: they must answer every tick, so a strided
 *   acquisition gap can never suppress an in-range swing or hold a fled player
 *   under pursuit.
 * - **`idle → attack` is declared first.** Guards are first-true-wins in
 *   declaration order, so the "already in contact range on the tick we notice
 *   them" edge has to precede the plain detection edge to be reachable.
 *
 * There is no `death` state: death is not a graph transition. The engine's
 * death sweep latches a zero-HP enemy and the authored impact policy plays the
 * `death` mesh clip and despawns after its own delay. `deathDespawnMs` below
 * is carried for legacy `components.ai` shape parity only — nothing reads it;
 * despawn timing belongs entirely to the impact policy's `despawn` effect.
 *
 * The graph also owns its own leash: there is no engine-side range limit on
 * an authored `chaseTarget` state. `alert`'s `dist > REFERENCE_LEASH_RANGE`
 * exit above is what stops pursuit — omit an exit guard like it and the
 * enemy chases from anywhere on the level. Engagement and disengagement are
 * both graph-authored here, mirrored against the same `brain.targetDistance`
 * read the entry guards use.
 */
export const referenceEnemyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: REFERENCE_ENEMY_CLASSNAME,
  components: {
    // Hit points + a hitscan hitbox so the shipped reference pistol's ray can
    // target and kill it. ~1.8 m tall human silhouette; Y-up, so the middle
    // half-extent is the vertical half-height and the offset lifts the box from
    // the foot-level transform origin to mid-body.
    health: {
      max: 60,
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
      initial: "idle",
      moveSpeed: 3,
      // Legacy-parity field only; no runtime consumer. Despawn timing is the
      // impact policy's `despawn` effect, not this value. See the class doc
      // comment above.
      deathDespawnMs: 4000,
      attack: { damage: 8, range: REFERENCE_ATTACK_RANGE, cooldownMs: 1200 },
      states: {
        // At rest. `initial` doubles as the state the engine forces when the
        // aggro gate closes or no player is around, and as the animation a
        // travelling state falls back to at a standstill — so it is authored
        // rest-appropriate.
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
        // Pursuit. No action of its own, so the engine treats it as the
        // locomotion state: it plays `walk` while travelling and yields to the
        // `idle` rest animation when stopped.
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
              when: runtime.gt(brain.targetDistance, REFERENCE_LEASH_RANGE),
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
 * A deliberately minimal AI fixture for E21 pose-modifier verification. Its
 * model is the content-facing copy of the model crate's three-joint
 * `joint_zones` test fixture, with a no-op clip so target acquisition supplies
 * the animated mesh's pose inputs. This is a triangle marker, not production
 * character art.
 *
 * It stays on the legacy `components.ai` block on purpose: `ai` lowers to a
 * behavior graph at spawn, and keeping one shipped archetype on that spelling
 * keeps the lowering path exercised by real content rather than by tests alone.
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
    ai: {
      detectionRange: 16,
      attackRange: 2,
      leashRange: 50,
      attackDamage: 8,
      attackCooldownMs: 1200,
      moveSpeed: 3,
      deathDespawnMs: 4000,
      states: {
        idle: "idle",
        alert: "walk",
        attack: "attack",
        death: "death",
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
