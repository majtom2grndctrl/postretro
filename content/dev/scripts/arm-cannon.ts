// DEMO CONTENT — `cannon_brute` and its built-in `arm_cannon`.
//
// A Mixamo Mutant whose right hand is replaced by a cannon fused to the
// forearm (`models/mutant_arm_cannon/`, a copy of the mutant with the hand cut
// away and a `cannon_tip` skin joint at the bore mouth). The cannon is part of
// the body, so `mesh.mounts` names no bake for it: `{ item, muzzleSocket }`
// says its shots leave from the posed `cannon_tip`, and the entry mounts nothing.
//
// The mutant has no aim or shoot clip. `smash_windup` ends with the cannon held
// out front (its barrel 26 degrees off the model's +Z front and level), and
// `smash_strike` starts from that pose, so the shot leaves on the strike's
// first tick, with the cannon aimed; the arm then swings through the rest of
// the strike. (`punch` points the cannon 41 degrees off front and 40 degrees
// down at its strike.)
//
// Map-placeable via `"classname" "cannon_brute"`.

import { activation, attackTurnFree, brain, defineAttack, defineEntity, defineStance } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { ranged } from "./attack-tokens";
import { ballistic } from "./damage-types";
import { enemyRifleProjectile } from "./enemy-rifle";

const DETECTION_RANGE = 40;
// Fire from this far in, with the target in sight; chase past it.
const FIRE_RANGE = 14;
const STANDOFF_DISTANCE = 8;
const AIM_HOLD_MS: [number, number] = [500, 900];
const SETTLE_MS = 400;
const LEASH_RANGE = 70;
const RETURN_ARRIVAL_EPSILON = 1;

// The cannon: one slow, dodgeable shot on the enemy rifles' projectile look.
export const armCannonEntity = defineEntity({
  canonicalName: "arm_cannon",
  components: {
    weapon: {
      damage: 14,
      damageType: ballistic,
      range: FIRE_RANGE,
      primary: { trigger: "press", recoveryMs: 1500, steps: [activation.shot()] },
      resolution: "projectile",
      sounds: { fire: "weapons/enemy_rifle_fire" },
      projectile: enemyRifleProjectile(),
      creditSource: "enemy.arm_cannon",
    },
  },
});

// The cannon raised: `smash_windup` ends on the pose `smash_strike` starts from.
const aim = defineStance({ animation: "smash_windup", holdMs: AIM_HOLD_MS });
const settle = defineStance({ animation: "idle" });

// No clip moment: the shot leaves on the release's first tick, at the pose above.
const cannonShot = defineAttack({
  draws: [ranged],
  weapon: "arm_cannon",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "smash_strike", at: false },
});

export const cannonBruteEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "cannon_brute",
  components: {
    health: {
      max: 90,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
    },
    tolerance: 4,
    mesh: {
      model: "models/mutant_arm_cannon/model.gltf",
      // The cannon is part of the body: its shots leave from this joint.
      mounts: [{ item: armCannonEntity, muzzleSocket: "cannon_tip" }],
      // The mutant's states, on the `mixamo_brute` clip library.
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk", loop: true, crossfadeMs: 200, travelSpeed: 1.2 },
        run: { clip: "run", loop: true, crossfadeMs: 200, travelSpeed: 2.2, maxPlaybackRate: 3 },
        smash_windup: { clip: "smash_windup", loop: false, crossfadeMs: 80 },
        smash_strike: { clip: "smash_strike", loop: false, crossfadeMs: 80, interrupt: "snap" },
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
            // Run the target down until it is in range and in sight.
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
              },
              transitions: {
                close: [
                  {
                    to: aim,
                    when: brain.targetVisible
                      .and(brain.targetDistance.le(FIRE_RANGE))
                      .and(attackTurnFree(cannonShot)),
                  },
                ],
                aim: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: cannonShot },
                  { to: "close", when: brain.timeInActivityMs.ge(2000) },
                ],
                settle: [{ to: "close", when: brain.timeInActivityMs.ge(SETTLE_MS) }],
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
