// The stances brief's Scripting surface example as dev content: a brute-like
// offense whose poses telegraph its strikes. Not placed in any map.
//
// Hand-authored beside `stance-surface.luau`: the scripting-core twin parser
// test keeps their lowered descriptors equal.
// See: context/lib/entity_model.md §7c (Stances and attacks)

import { brain, defineAttack, defineEntity, defineStance } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { heavy, melee } from "./attack-tokens";

/** Classname a dev map would place this archetype by. */
export const STANCE_SURFACE_CLASSNAME = "stance_surface_brute";

// Poses. Each is named by its binding: "overhead", "coil", "bellow".
const overhead = defineStance({
  animation: "smash_windup", // the held pose names the threat
  // Heard once per pose entry. Stance sounds have no assets yet; a dev
  // fixture stands in for the example's `brute/stance_overhead`.
  sound: "fixtures/growl",
  holdMs: [500, 1200], // release time drawn per strike, hidden from guards
  breakOn: { damage: 25, to: "flinch" }, // rest and holds
});
const coil = defineStance({ animation: "swipe_windup" });
const bellow = defineStance({ animation: "roar" });

// Strikes. Two begin in `overhead`: same pose, different hit.
const slam = defineAttack({
  draws: [melee.with({ cost: 2 }), heavy],
  damage: 14,
  maxRange: 3.5,
  cooldownMs: 1800,
  beginningStance: overhead,
  endingStance: coil, // lands coiled, ready to lunge
  release: { animation: "smash_strike", at: "hit" }, // the default recover runs the clip out
});
const sweep = defineAttack({
  draws: [melee],
  damage: 8,
  maxRange: 3.5,
  cooldownMs: 1200,
  beginningStance: overhead, // ends back in `overhead`
  release: { animation: "swipe_strike" }, // fires at its clip's `hit` by default
});
const lunge = defineAttack({
  draws: [melee],
  damage: 10,
  maxRange: 3.5,
  cooldownMs: 1500,
  beginningStance: coil,
  release: { animation: "swipe_strike", at: "hit" },
});
const charge = defineAttack({
  draws: [heavy],
  damage: 18,
  maxRange: 12,
  cooldownMs: 4000,
  beginningStance: bellow,
  release: { animation: "run", motion: "chaseTarget", speed: 2, ms: 1500, at: false }, // hits on reach
  recover: { animation: "idle", ms: 600 }, // required after a moving release
});

export const stanceSurfaceEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: STANCE_SURFACE_CLASSNAME,
  components: {
    health: {
      max: 70,
      hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] },
    },
    // The Mixamo Mutant, on the `mixamo_brute` clip library: each state the
    // example names plays its namesake clip.
    mesh: {
      model: "models/mutant/model.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk", loop: true, crossfadeMs: 200 },
        run: { clip: "run", loop: true, crossfadeMs: 120, travelSpeed: 2.2 },
        smash_windup: { clip: "smash_windup", loop: false, crossfadeMs: 80 },
        swipe_windup: { clip: "swipe_windup", loop: false, crossfadeMs: 80 },
        roar: { clip: "roar", loop: false, crossfadeMs: 80 },
        smash_strike: { clip: "smash_strike", loop: false, interrupt: "snap" },
        swipe_strike: { clip: "swipe_strike", loop: false, interrupt: "snap" },
        flinch: { clip: "hit_reaction", loop: false, crossfadeMs: 80, interrupt: "snap" },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "walk",
          layers: {
            move: ["chaseTarget"],
            // The example's offense; the behavior registers every attack its
            // rows strike.
            offense: {
              initial: "approach",
              activities: {
                approach: { animation: "walk" },
                flinch: { animation: "flinch", motion: "hold" },
              },
              transitions: {
                approach: [
                  { to: overhead, when: brain.targetDistance.le(3.5) }, // take the pose
                  { to: bellow, when: brain.targetDistance.ge(8) },
                ],
                // Rows out of each rest. A row to an attack also waits for its
                // turn and its range.
                overhead: [
                  { to: slam, when: brain.activityRoll.lt(0.5) }, // one roll per entry of the pose
                  { to: sweep },
                  { to: "approach", when: brain.targetDistance.gt(4.5) },
                ],
                coil: [
                  { to: lunge },
                  { to: "approach", when: brain.timeInActivityMs.ge(800) },
                ],
                bellow: [
                  { to: charge },
                  { to: "approach", when: brain.timeInActivityMs.ge(1500) },
                ],
                flinch: [{ to: "approach", when: brain.timeInActivityMs.ge(450) }],
              },
            },
          },
        },
      },
      transitions: {
        idle: [{ to: "engage", when: brain.hasTarget }],
        engage: [{ to: "idle", when: brain.hasTarget.not() }],
      },
    },
  },
});
