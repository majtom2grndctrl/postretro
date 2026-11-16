// The dev mod's map-placeable hierarchical behavior-statechart fixture.
//
// This is deliberately hand-authored beside `reference-enemy.luau`: the
// scripting-core twin parser test keeps their descriptors byte-for-byte equal
// after canonical conversion.
// See: context/lib/entity_model.md §7c (Stances and attacks)

import {
  brain,
  defense,
  defineAttack,
  defineEntity,
  defineStance,
  runtime,
} from "postretro";
import type { EntityTypeDescriptor } from "postretro";
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
const LUNGE_RANGE = 3;
const SLAM_RANGE = 3.5;
// The charge's contact reach. Its `maxRange` both opens its queue and lands
// its contact, so the brute bellows only with its target this close; a target
// that backs off during the roar is chased down at twice the brute's pace.
const CHARGE_REACH = 2.5;
// Combat slots sit inside jab reach, so the brute fights close enough to jab.
const CLOSE_STANDOFF = 1.5;
const LEASH_RANGE = 100;
const RETURN_ARRIVAL_EPSILON = 1;
// Patrol and investigation stroll on the walk: 0.4 of `moveSpeed` is 1.2 m/s,
// the walk clip's planted-foot speed.
const STROLL = 0.4;
// `approach` picks its tell from one roll per entry: the crouch at slam range,
// the roar at charge reach, else the cocked fist at jab reach.
const OVERHEAD_SHARE = 0.4;
const BELLOW_FROM = 0.85;
// From the crouch, half the strikes slam and half lunge: one pose, two timings.
const SLAM_SHARE = 0.5;
// A tell waiting in line this long, or a target this far, sends the brute back
// to `approach`, which rolls a fresh tell.
const OVERHEAD_WAIT_MS = 3000;
const COCK_WAIT_MS = 2000;
const BELLOW_WAIT_MS = 2500;
const OVERHEAD_LEAVE_RANGE = SLAM_RANGE + 1;
const COCK_LEAVE_RANGE = JAB_RANGE + 1;
// The slam lands coiled and catches its breath before the next tell.
const COIL_MS = 500;
const FLINCH_MS = 700;

// Poses, each holding its attack's windup cut from the `mixamo_brute` library:
// the crouch's bottom, the lean back, the fist drawn back, the roar. Stance
// sounds have no assets yet; dev fixtures stand in.
const overhead = defineStance({
  animation: "smash_windup",
  sound: "fixtures/jump",
  holdMs: [500, 1100],
  breakOn: { damage: 30, to: "flinch" },
});
// The slam's landing. No strike begins here: the brute's four strikes fill
// three tells, the crouch serving two.
const coil = defineStance({ animation: "swipe_windup", sound: "fixtures/land" });
const cock = defineStance({
  animation: "punch_windup",
  sound: "fixtures/bite",
  holdMs: [400, 800],
  breakOn: { damage: 20, to: "flinch" },
});
const bellow = defineStance({
  animation: "roar",
  sound: "fixtures/growl",
  holdMs: [800, 1400],
  breakOn: { damage: 40, to: "flinch" },
});

// Strikes. Each release plays the rest of its attack from its tell's cut and
// fires at the clip's `hit`, unasked: the slam 1,200 ms in, the lunge 400 ms,
// the jab 100 ms. `slam` draws two `melee` and the `heavy`, so another
// brute's jab or lunge keeps drawing the third `melee` while one slams.
const jab = defineAttack({
  draws: [melee],
  damage: 8,
  maxRange: JAB_RANGE,
  cooldownMs: 800,
  beginningStance: cock,
  endingStance: overhead, // the jab drops into the crouch: slam or lunge next
  release: { animation: "punch_strike" },
});
const slam = defineAttack({
  draws: [melee.with({ cost: 2 }), heavy],
  damage: 14,
  maxRange: SLAM_RANGE,
  cooldownMs: 1800,
  beginningStance: overhead,
  endingStance: coil,
  release: { animation: "smash_strike" },
});
const lunge = defineAttack({
  draws: [melee],
  damage: 10,
  maxRange: LUNGE_RANGE,
  cooldownMs: 1500,
  beginningStance: overhead,
  endingStance: cock, // the claw lands with the fist drawn for a jab
  release: { animation: "swipe_strike" },
});
// The roar releases into a run at twice the brute's pace, landing its contact
// on reach: at once if the target held its ground, else when it catches up.
// It runs for a combat slot at jab reach, well inside its contact reach;
// without a standoff its slot would sit at `maxRange` and it would pull up
// short of contact.
const charge = defineAttack({
  draws: [heavy],
  damage: 16,
  maxRange: CHARGE_REACH,
  standoffDistance: CLOSE_STANDOFF,
  cooldownMs: 5000,
  beginningStance: bellow,
  endingStance: cock,
  release: { animation: "run", motion: "chaseTarget", speed: 2, ms: 1200, at: false },
  recover: { animation: "idle", ms: 500 },
});
// `claw` is not wired into the graph; the explicit `attacks` list keeps it on
// the archetype.
const claw = defineAttack({ damage: 15, damageType: corrosive, maxRange: 2, cooldownMs: 800 });

/**
 * The reference enemy exercises the full recursive statechart path, stances
 * and the attack budget:
 *
 * - `engage` runs a selector movement layer and a nested offense graph.
 * - `approach` closes on the target and takes a tell: `overhead`, `cock` or
 *   `bellow`. Each tell waits in line for its strike's turn, holds a hidden
 *   time, and releases.
 * - `overhead` leads to two strikes, the slam and the lunge, so the crouch
 *   names the threat and not its timing.
 * - the strikes chain through their ending stances: jab → crouch, lunge →
 *   cocked fist, charge → cocked fist, slam → coiled, then `approach`.
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
    // The Mixamo Mutant, on the `mixamo_brute` clip library. Each state plays
    // its namesake clip; travel speeds are the clips' planted-foot speeds.
    mesh: {
      model: "models/mutant/model.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk", loop: true, crossfadeMs: 200, travelSpeed: 1.2 },
        run: { clip: "run", loop: true, crossfadeMs: 200, travelSpeed: 2.2 },
        smash_windup: { clip: "smash_windup", loop: false, crossfadeMs: 80 },
        swipe_windup: { clip: "swipe_windup", loop: false, crossfadeMs: 80 },
        punch_windup: { clip: "punch_windup", loop: false, crossfadeMs: 80 },
        roar: { clip: "roar", loop: false, crossfadeMs: 120 },
        smash_strike: { clip: "smash_strike", loop: false, crossfadeMs: 80, interrupt: "snap" },
        swipe_strike: { clip: "swipe_strike", loop: false, crossfadeMs: 80, interrupt: "snap" },
        punch_strike: { clip: "punch_strike", loop: false, crossfadeMs: 80, interrupt: "snap" },
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
      attacks: [jab, slam, lunge, charge, claw],
      engagementRadius: CLOSE_STANDOFF,
      patrol: {
        mode: "pingPong",
        points: [[0, 0], [6, 0], [6, 6]],
      },
      activities: {
        idle: { animation: "idle", motion: "hold" },
        patrol: { animation: "walk", motion: "patrol", speed: STROLL },
        engage: {
          animation: "run",
          layers: {
            // In jab reach, release movement but retain the selected target
            // for offense; outside it, the fallback chases the target. Every
            // pose and strike stands its ground.
            move: [
              {
                when: brain.targetDistance.le(JAB_RANGE),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "approach",
              activities: {
                approach: { animation: "run" },
                flinch: { animation: "flinch", motion: "hold" },
              },
              transitions: {
                approach: [
                  {
                    to: overhead,
                    when: brain.targetDistance
                      .le(SLAM_RANGE)
                      .and(brain.activityRoll.lt(OVERHEAD_SHARE)),
                  },
                  {
                    to: bellow,
                    when: brain.targetDistance
                      .le(CHARGE_REACH)
                      .and(brain.activityRoll.ge(BELLOW_FROM)),
                  },
                  { to: cock, when: brain.targetDistance.le(JAB_RANGE) },
                ],
                // Rows out of each rest. A row to an attack also waits for its
                // turn, its range and its cooldown.
                overhead: [
                  { to: slam, when: brain.activityRoll.lt(SLAM_SHARE) },
                  { to: lunge },
                  { to: "approach", when: brain.targetDistance.gt(OVERHEAD_LEAVE_RANGE) },
                  { to: "approach", when: brain.timeInActivityMs.ge(OVERHEAD_WAIT_MS) },
                ],
                cock: [
                  { to: jab },
                  { to: "approach", when: brain.targetDistance.gt(COCK_LEAVE_RANGE) },
                  { to: "approach", when: brain.timeInActivityMs.ge(COCK_WAIT_MS) },
                ],
                bellow: [
                  { to: charge },
                  { to: "approach", when: brain.timeInActivityMs.ge(BELLOW_WAIT_MS) },
                ],
                coil: [{ to: "approach", when: brain.timeInActivityMs.ge(COIL_MS) }],
                flinch: [{ to: "approach", when: brain.timeInActivityMs.ge(FLINCH_MS) }],
              },
            },
          },
        },
        investigate: { animation: "walk", motion: "moveToLastKnown", speed: STROLL },
        startle: { animation: "flinch", motion: "hold" },
        retreat: { animation: "run", motion: "moveToAnchor" },
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
