// ILLUSTRATIVE — proposed syntax, does not compile. See ./README.md.
// Probes: CO-OP COMPANION — the north-star consumer of the same motion primitive.
// A companion is structurally an enemy brain with allied sentiment PLUS two net-new pieces:
//   (1) a DYNAMIC FRIENDLY ANCHOR — a position goal offset from a live entity (the player),
//       where today anchors are fixed points or the hostile target only;
//   (2) a PLAYER→COMPANION COMMAND CHANNEL — an external input the brain scope has no
//       equivalent of (all 20 brain facts are engine-computed perception/state).

import { brain, command, defineEntity, runtime } from "postretro"; // `command` is PROPOSED
import type { EntityTypeDescriptor } from "postretro";

const FOLLOW_STANDOFF = 3; // hold this far from the player
const CATCH_UP = 8; // sprint to close if we fall this far behind
const ENGAGE_RANGE = 14;

// PROPOSED command channel. The player issues these to their companion; the value is a
// replicated per-companion input (host-authoritative), read like any leaf. Spelled here as a
// small enum surfaced through `command.*`. Open: which crate owns this input (netcode/sim/ai).
// command.mode  -> "follow" | "hold" | "attack"
// command.mark  -> the entity the player flagged with "attack that" (an anchor, see below)

export const allyEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "ally",
  components: {
    health: { max: 100 },
    mesh: { model: "ally" },
    // TODAY: faction + sentiment already express "allied to the player" (positive sentiment).
    faction: "player_allies",
    behavior: {
      initial: "follow",
      moveSpeed: 5,
      engagementRadius: ENGAGE_RANGE,
      attacks: {
        shoot: { weapon: "ally_rifle", standoffDistance: 6 },
      },

      // TODAY candidacy narrows *who to attack*. For an ally, hostiles only — the engine
      // floor already offers only entities the ally is hostile toward (sentiment < 0).
      // No extra work here; shown for completeness.

      activities: {
        // Follow the player. PROPOSED: `motion` anchors to a LIVE ENTITY, not a fixed point.
        follow: {
          animation: "run",
          motion: {
            verb: "follow", // PROPOSED verb, or Option-B `positionGoal` with an entity anchor
            anchor: "player", // PROPOSED: dynamic entity anchor (the commanding player)
            standoff: FOLLOW_STANDOFF,
            engaged: false, // non-combat position goal: no combat slot
          },
        },

        // Hold at the current spot on command. TODAY `moveToAnchor` re-homes to spawn; here we
        // want "hold HERE", which is a re-home to the current position — a runtime re-home path.
        hold: { animation: "idle_aiming", motion: "hold" },

        // Engage a hostile while staying tethered to the player (companion doesn't wander off).
        engage: {
          animation: "run",
          layers: {
            move: [
              // Break off and rejoin the player if we've strayed too far, even mid-fight.
              { when: brain.distanceFromAnchor.gt(CATCH_UP), motion: { verb: "follow", anchor: "player", standoff: FOLLOW_STANDOFF } },
              // Otherwise hold a firing standoff on the hostile target.
              { when: brain.targetVisible, motion: { verb: "strafe", anchor: "target", standoff: 6 } },
              "chaseTarget",
            ],
            offense: {
              initial: "aim",
              activities: {
                aim: { animation: "idle_aiming" },
                fire: { animation: "shoot", action: { attack: "shoot" } },
              },
              transitions: {
                aim: [{ to: "fire", when: brain.timeInActivityMs.ge(300) }],
                fire: [{ to: "aim", when: brain.timeInActivityMs.ge(200) }],
              },
            },
          },
        },
      },

      transitions: {
        // Commands take priority every tick — a wildcard row reading the PROPOSED command channel.
        "*": [
          { to: "hold", when: command.mode.eq("hold") }, // PROPOSED
          // "attack that": the player-marked entity becomes the acquisition preference. Engaging
          // it still flows through normal candidacy/hostility; the mark biases *which* hostile.
          { to: "engage", when: command.mode.eq("attack").and(brain.hasTarget) }, // PROPOSED
        ],

        follow: [
          // Autonomously engage a nearby hostile while in follow mode (still tethered).
          { to: "engage", when: brain.acquisitionDue.and(brain.targetDistance.le(ENGAGE_RANGE)) },
        ],
        hold: [
          { to: "follow", when: command.mode.eq("follow") }, // PROPOSED: released from hold
        ],
        engage: [
          { to: "follow", when: brain.targetDied.or(brain.hasTarget.not()) },
        ],
      },
    },
  },
});

// Open questions this file makes concrete:
//  - Dynamic anchor scope: does `anchor: "player"` name a role the engine resolves to an
//    entity, or an entity handle the script holds? A role ("the commanding player") avoids
//    scripts naming EntityIds and works in co-op where the anchor is a remote client's pawn.
//  - Command channel placement + wire path: the command originates on a (possibly remote)
//    client, but graph eval is host-only — so the command is replicated INPUT, distinct from
//    the host-authoritative brain facts. Where it lives across netcode/sim/ai is a real fork.
//  - "hold here" needs runtime re-home (anchor := current position), an additive path §7c notes
//    but that isn't built.
