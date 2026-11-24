// DEMO CONTENT — `cannon_brute` and its built-in arm cannons.
//
// A Mixamo Mutant whose right hand is replaced by a cannon fused to the
// forearm (`models/mutant_arm_cannon/`, a copy of the mutant with the hand cut
// away and a `cannon_tip` skin joint at the bore mouth). The cannon is part of
// the body, so `mesh.mounts` names no bake for it: `{ item, muzzleSocket }`
// says its shots leave from the posed `cannon_tip`, and the entry mounts nothing.
//
// It fights as the limitator does, on the brute's clips: it runs a far target
// down and now and then stops to sight one long shot; within strafing range it
// strafes around its target, throws the cannon up when a `ranged` turn is free
// and fires a three-shot burst, then runs to a flanking slot, whose roll picks
// the next cycle: strafe and burst again, or stop and aim. Enough damage while
// it sights or aims staggers it. It fights alongside the rifle troopers.
//
// The mutant has no aim or shoot clip. `smash_windup` ends with the cannon held
// out front (its barrel 26 degrees off the model's +Z front and level), and
// `smash_strike` starts from that pose, so every shot leaves on the strike's
// first ticks, with the cannon aimed; each shot flies from the posed muzzle at
// the target. (`punch` points the cannon 41 degrees off front and 40 degrees
// down at its strike.)
//
// Map-placeable via `"classname" "cannon_brute"`.

import { activation, attackTurnFree, brain, defineAttack, defineEntity, defineStance } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { ranged } from "./attack-tokens";
import { ballistic } from "./damage-types";
import { enemyRifleProjectile } from "./enemy-rifle";
import { RIFLE_TOLERANCE, RIFLE_TROOPERS_FACTION } from "./rifle-squads";

// The limitator's ranges, scaled to the cannon's reach.
const DETECTION_RANGE = 50;
const FIRE_RANGE = 10;
const BREAK_RANGE = 11;
const STANDOFF_DISTANCE = 8;
const LEASH_RANGE = 70;
const RETURN_ARRIVAL_EPSILON = 1;

// The far long shot, as the limitator's: after RUN_MS of running, with the
// target in sight within LONG_RANGE, it stops and sights the cannon.
const LONG_RANGE = 20;
const RUN_MS = 2000;
const SIGHT_HOLD_MS: [number, number] = [1000, 1333];
const SIGHT_WAIT_MS = 2000;
const LONG_SHOT_RECOVER_MS = 250;

// The burst cannon's recovery and the span of its three shots: every route
// into the panic runs through the strafe, held at least WAIT_MS.
const BURST_RECOVERY_MS = 1200;
const BURST_STEP_MS = 150;
const BURST_SPAN_MS = 2 * BURST_STEP_MS + 17;
const WAIT_MS = BURST_RECOVERY_MS + BURST_SPAN_MS;
const PANIC_HOLD_MS = 600;
const PANIC_WAIT_MS = 1000;
const MOVE_MS = 1500;
const STRAFE_SHARE = 0.5;
const AIM_HOLD_MS: [number, number] = [500, 900];
const AIM_BREAK_DAMAGE = 25;
const AIM_WAIT_MS = 2000;
// The burst's recover keeps the strike drawn through its later two shots.
const BURST_RECOVER_MS = BURST_SPAN_MS + 50;
const SETTLE_MS = 300;
// `hit_reaction` plays out; the flinch holds its last frame briefly.
const FLINCH_MS = 600;

// The long cannon: one slow, dodgeable shot that reaches LONG_RANGE.
export const armCannonEntity = defineEntity({
  canonicalName: "arm_cannon",
  components: {
    weapon: {
      damage: 14,
      damageType: ballistic,
      range: LONG_RANGE,
      primary: { trigger: "press", recoveryMs: 1500, steps: [activation.shot()] },
      resolution: "projectile",
      sounds: { fire: "weapons/enemy_rifle_fire" },
      projectile: enemyRifleProjectile(),
      creditSource: "enemy.arm_cannon",
    },
  },
});

// The burst cannon: three lighter shots, the spray's and the aim's.
export const armCannonBurstEntity = defineEntity({
  canonicalName: "arm_cannon_burst",
  components: {
    weapon: {
      damage: 8,
      damageType: ballistic,
      range: 14,
      primary: {
        trigger: "press",
        recoveryMs: BURST_RECOVERY_MS,
        steps: [
          activation.shot(), activation.wait(BURST_STEP_MS),
          activation.shot(), activation.wait(BURST_STEP_MS),
          activation.shot(),
        ],
      },
      resolution: "projectile",
      sounds: { fire: "weapons/enemy_rifle_fire" },
      projectile: enemyRifleProjectile(),
      creditSource: "enemy.arm_cannon",
    },
  },
});

// Every stance raises the cannon: `smash_windup` ends on the pose
// `smash_strike` starts from.
const aim = defineStance({
  animation: "smash_windup",
  holdMs: AIM_HOLD_MS,
  breakOn: { damage: AIM_BREAK_DAMAGE, to: "flinch" },
});
const panic = defineStance({ animation: "smash_windup", holdMs: PANIC_HOLD_MS });
const settle = defineStance({ animation: "idle" });
const sight = defineStance({
  animation: "smash_windup",
  holdMs: SIGHT_HOLD_MS,
  breakOn: { damage: AIM_BREAK_DAMAGE, to: "flinch" },
});
const steady = defineStance({ animation: "idle" });

// No clip moment: each release fires on its first tick, at the raised pose.
const shoot = defineAttack({
  draws: [ranged],
  weapon: "arm_cannon_burst",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "smash_strike", at: false },
  recover: { animation: "smash_strike", ms: BURST_RECOVER_MS },
});
const spray = defineAttack({
  draws: [ranged],
  weapon: "arm_cannon_burst",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: panic,
  endingStance: settle,
  release: { animation: "smash_strike", at: false },
  recover: { animation: "smash_strike", ms: BURST_RECOVER_MS },
});
const longshot = defineAttack({
  draws: [ranged],
  weapon: "arm_cannon",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: sight,
  endingStance: steady,
  release: { animation: "smash_strike", at: false },
  recover: { animation: "smash_strike", ms: LONG_SHOT_RECOVER_MS },
});

export const cannonBruteEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "cannon_brute",
  components: {
    faction: RIFLE_TROOPERS_FACTION,
    health: {
      max: 100,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
      zoneMultipliers: {
        head: 2.5,
        leg: 0.5,
      },
    },
    tolerance: RIFLE_TOLERANCE,
    mesh: {
      model: "models/mutant_arm_cannon/model.gltf",
      // The cannon is part of the body: both cannons' shots leave this joint.
      mounts: [
        { item: armCannonBurstEntity, muzzleSocket: "cannon_tip" },
        { item: armCannonEntity, muzzleSocket: "cannon_tip" },
      ],
      // The mutant's states, on the `mixamo_brute` clip library. It has no
      // directional walks, so the strafe plays `walk` whichever way it moves.
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk", loop: true, crossfadeMs: 200, travelSpeed: 1.2 },
        run: { clip: "run", loop: true, crossfadeMs: 200, travelSpeed: 2.2, maxPlaybackRate: 3 },
        smash_windup: { clip: "smash_windup", loop: false, crossfadeMs: 80 },
        smash_strike: { clip: "smash_strike", loop: false, crossfadeMs: 80, interrupt: "snap" },
        hit_reaction: { clip: "hit_reaction", loop: false, crossfadeMs: 80, interrupt: "snap" },
        death: { clip: "death", loop: false, crossfadeMs: 120, interrupt: "snap" },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      engagementRadius: STANDOFF_DISTANCE,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "walk",
          layers: {
            // Run the target down until it is in range and in sight; every
            // stance stands still.
            move: [
              {
                when: brain.targetDistance.le(FIRE_RANGE).and(brain.targetVisible),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "close",
              activities: {
                close: { animation: "run" },
                wait: { animation: "walk", motion: "strafe" },
                move: {
                  animation: "run",
                  motion: "reposition",
                  facing: "travel",
                  reposition: { arc: [60, 120] },
                },
                flinch: { animation: "hit_reaction", motion: "hold" },
              },
              transitions: {
                close: [
                  {
                    to: "wait",
                    when: brain.targetDistance.le(FIRE_RANGE).and(brain.targetVisible),
                  },
                  {
                    to: sight,
                    when: brain.timeInActivityMs
                      .ge(RUN_MS)
                      .and(brain.targetVisible)
                      .and(brain.targetDistance.gt(FIRE_RANGE))
                      .and(brain.targetDistance.le(LONG_RANGE))
                      .and(attackTurnFree(longshot)),
                  },
                ],
                wait: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  {
                    to: panic,
                    when: brain.timeInActivityMs.ge(WAIT_MS).and(attackTurnFree(spray)),
                  },
                ],
                panic: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: spray },
                  { to: "wait", when: brain.timeInActivityMs.ge(PANIC_WAIT_MS) },
                ],
                sight: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(LONG_RANGE) },
                  { to: "close", when: brain.targetDistance.le(FIRE_RANGE) },
                  { to: longshot },
                  { to: "close", when: brain.timeInActivityMs.ge(SIGHT_WAIT_MS) },
                ],
                steady: [{ to: "close", when: brain.timeInActivityMs.ge(SETTLE_MS) }],
                move: [
                  {
                    to: "wait",
                    when: brain.timeInActivityMs
                      .ge(MOVE_MS)
                      .and(brain.activityRoll.lt(STRAFE_SHARE)),
                  },
                  { to: aim, when: brain.timeInActivityMs.ge(MOVE_MS) },
                ],
                aim: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: shoot },
                  { to: "wait", when: brain.timeInActivityMs.ge(AIM_WAIT_MS) },
                ],
                settle: [{ to: "move", when: brain.timeInActivityMs.ge(SETTLE_MS) }],
                flinch: [
                  {
                    to: "close",
                    when: brain.timeInActivityMs
                      .ge(FLINCH_MS)
                      .and(brain.targetDistance.gt(BREAK_RANGE)),
                  },
                  { to: "move", when: brain.timeInActivityMs.ge(FLINCH_MS) },
                ],
              },
            },
          },
        },
        retreat: { animation: "run", motion: "moveToAnchor" },
      },
      transitions: {
        "*": [{ to: "idle", when: brain.hasTarget.not() }],
        idle: [
          {
            to: "engage",
            when: brain.acquisitionDue.and(brain.targetDistance.le(DETECTION_RANGE)),
          },
        ],
        engage: [{ to: "retreat", when: brain.distanceFromAnchor.gt(LEASH_RANGE) }],
        retreat: [
          { to: "idle", when: brain.distanceFromAnchor.le(RETURN_ARRIVAL_EPSILON) },
        ],
      },
    },
  },
});
