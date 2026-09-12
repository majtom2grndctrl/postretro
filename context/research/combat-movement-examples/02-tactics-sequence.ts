// ILLUSTRATIVE — proposed syntax, does not compile. See ./README.md.
// Probes: SEQUENCE SUGAR vs. today's hand-rolled nested graph, and where the ESCAPE
// HATCH lives. A committed micro-combo is an ordered chain with per-step completion
// conditions; commitment must not become an engine latch (guards evaluate every tick).

import { brain, defineEntity, runtime, sequence, step } from "postretro"; // `sequence`/`step` are PROPOSED
import type { EntityTypeDescriptor } from "postretro";

const JAB_RANGE = 2.5;
const FIRE_RANGE = 14;
const STRAFE_METERS = 2;
const BURST = 3;

export const skirmisherEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "skirmisher",
  components: {
    health: { max: 50 },
    mesh: { model: "skirmisher" },
    behavior: {
      initial: "idle",
      moveSpeed: 4,
      engagementRadius: FIRE_RANGE,
      attacks: {
        shoot: { weapon: "enemy_rifle", standoffDistance: 6 },
      },
      activities: {
        idle: { animation: "idle", motion: "hold" },

        engage: {
          animation: "run",
          layers: {
            // ---- The tactic: "fire one, strafe 2m, fire one." ----

            // Option A — PROPOSED SUGAR. Reads top-to-bottom like the tactic itself.
            // Each step runs until its `until` guard is true, then the next begins.
            offense: sequence([
              step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(1) }),
              step({
                motion: { verb: "strafe", anchor: "target" }, // PROPOSED verb (see 01)
                until: brain.displacementInActivity.ge(STRAFE_METERS), // PROPOSED fact
              }),
              step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(1) }),
            ]),

            // Option B — what Option A LOWERS TO, and what you write TODAY: a linear
            // nested graph. Same behavior, more ceremony, and the step boundaries are
            // implicit in the transition guards rather than named.
            // offense: {
            //   initial: "fire1",
            //   activities: {
            //     fire1: { animation: "shoot", action: { attack: "shoot" } },
            //     reposition: { animation: "run", motion: { verb: "strafe", anchor: "target" } },
            //     fire2: { animation: "shoot", action: { attack: "shoot" } },
            //   },
            //   transitions: {
            //     fire1: [{ to: "reposition", when: brain.attacksFiredInActivity.ge(1) }],
            //     reposition: [{ to: "fire2", when: brain.displacementInActivity.ge(STRAFE_METERS) }],
            //     fire2: [{ to: "fire1", when: brain.attacksFiredInActivity.ge(1) }],
            //   },
            // },
          },
        },
      },

      transitions: {
        idle: [
          { to: "engage", when: brain.acquisitionDue.and(brain.targetDistance.le(FIRE_RANGE)) },
        ],

        // THE ESCAPE HATCH — and the key point: commitment needs no new mechanism.
        // The sequence above has no inner bail-out rows, so it runs to completion...
        // ...UNLESS one of these wildcard/outer rows preempts the whole `engage` activity
        // (and with it the sequence) on a tick. This is the existing outer-to-inner,
        // wildcard-first rule doing the work.
        "*": [
          { to: "flee", when: brain.health.le(10) }, // took heavy damage: drop the combo now
          { to: "idle", when: brain.targetDied }, // target gone: no point finishing
        ],
        engage: [{ to: "idle", when: brain.targetDistance.gt(FIRE_RANGE) }],
        flee: [{ to: "idle", when: brain.timeInActivityMs.ge(2000) }],
      },
    },
  },
});

// A second tactic, "fire x3 then seek cover", to show the burst-count spelling is pure
// sugar over an EXISTING fact, while `seekCover` is a PROPOSED verb backed by new nav floor:
//
//   offense: sequence([
//     step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(BURST) }),
//     step({ motion: "seekCover" }), // PROPOSED verb; runs until the parent transitions out
//   ]),
//
// Open question this file makes concrete: how does `step({...})` surface interrupts so an
// author gets commitment WITHOUT accidentally authoring an uninterruptible combo? Proposal:
// steps never carry their own bail rows; interruption is always an outer wildcard, exactly
// as above — the sugar simply refuses a per-step `when`-to-abort field.
