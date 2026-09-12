// ILLUSTRATIVE — proposed syntax, does not compile. See ./README.md.
// Probes: "READ THE ROOM" — assemble a repertoire of tactics, prune the ones that don't
// apply right now, then choose (sometimes this, sometimes that).
//
// Two findings this file surfaces:
//  (1) Choosing among MULTI-STEP tactics is transition-routing at a dispatcher node, NOT
//      the leaf-only selector layer (`{when, motion, action}` rows carry no sub-graph).
//  (2) "Sometimes" reduces to ONE new fact, `brain.roll`, read by ordinary priority guards.
//      No weighted-selector policy is needed.

import { brain, defineEntity, runtime, sequence, step } from "postretro"; // sequence/step PROPOSED (see 02)
import type { EntityTypeDescriptor } from "postretro";

const FIRE_RANGE = 16;
const CLOSE_RANGE = 4;
const BURST = 3;

export const tacticianEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "tactician",
  components: {
    health: { max: 60 },
    mesh: { model: "tactician" },
    behavior: {
      initial: "idle",
      moveSpeed: 4.5,
      engagementRadius: FIRE_RANGE,
      attacks: {
        shoot: { weapon: "enemy_rifle", standoffDistance: 7 },
        melee: { damage: 14, maxRange: CLOSE_RANGE, cooldownMs: 900 },
      },
      activities: {
        idle: { animation: "idle", motion: "hold" },

        // The dispatcher. On entry, its transitions are the "read the room": each row is an
        // applicability guard; inapplicable tactics are simply guards that read false.
        // `chooseTactic` holds position for the tick it takes to route.
        chooseTactic: {
          animation: "idle_aiming",
          motion: "hold",
        },

        // Each tactic is a sub-activity whose `offense` is a committed sequence (see 02).
        strafeAndFire: {
          animation: "run",
          layers: {
            offense: sequence([
              step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(1) }),
              step({ motion: { verb: "strafe", anchor: "target" }, until: brain.displacementInActivity.ge(2) }),
              step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(1) }),
            ]),
          },
        },
        suppress: {
          animation: "run",
          layers: {
            offense: sequence([
              step({ action: { attack: "shoot" }, until: brain.attacksFiredInActivity.ge(BURST) }),
              step({ motion: "seekCover" }), // PROPOSED verb
            ]),
          },
        },
        rush: {
          animation: "run",
          layers: {
            move: ["chaseTarget"],
            offense: { initial: "melee", activities: { melee: { animation: "attack", action: { attack: "melee" } } }, transitions: {} },
          },
        },
      },

      transitions: {
        idle: [
          { to: "chooseTactic", when: brain.acquisitionDue.and(brain.targetDistance.le(FIRE_RANGE)) },
        ],

        // ---- READ THE ROOM ----
        // Ordered rows = priority. Applicability = the non-roll clauses (prune what doesn't fit).
        // `brain.roll` (PROPOSED: seeded, replicated, re-drawn on activity entry, in [0,1)) turns
        // a priority list into a weighted pick among the *applicable* tactics.
        chooseTactic: [
          // If they're in our face, rush — no dice roll; applicability decides.
          { to: "rush", when: brain.targetDistance.le(CLOSE_RANGE) },
          // Otherwise, sometimes suppress (only if we have a shot), sometimes strafe.
          {
            to: "suppress",
            when: brain.targetVisible.and(brain.roll.lt(0.4)), // PROPOSED fact
          },
          { to: "strafeAndFire", when: brain.targetVisible },
          // Nothing applicable (no line of sight): fall through to close the distance.
          { to: "rush", when: runtime.constant(true) },
        ],

        // Each tactic returns to the dispatcher when done, so the room is re-read (and the
        // roll re-drawn on re-entry) rather than locking into one tactic forever.
        strafeAndFire: [{ to: "chooseTactic", when: brain.attacksFiredInActivity.ge(2) }],
        suppress: [{ to: "chooseTactic", when: brain.timeInActivityMs.ge(1500) }],
        rush: [{ to: "chooseTactic", when: brain.targetDistance.gt(CLOSE_RANGE) }],

        "*": [{ to: "idle", when: brain.targetDied.or(brain.hasTarget.not()) }],
      },
    },
  },
});

// Open questions this file makes concrete:
//  - Refresh cadence of `brain.roll`: re-drawn on activity entry (so the dispatcher's roll is
//    stable for the tick it routes, fresh each re-entry). Does any case want a roll that
//    refreshes without an activity change? If not, per-entry keeps it deterministic + replicable.
//  - Is the dispatcher-node pattern worth a `chooseTactic([...])` sugar that lowers to exactly
//    this activity + transition block, or is the raw form clear enough to leave unsugared?
