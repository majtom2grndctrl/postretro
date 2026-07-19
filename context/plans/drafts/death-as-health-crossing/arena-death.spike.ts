// DESIGN SPIKE — the CANONICAL pre-spec artifact (fuses the earlier death-model +
// ergonomics probes). Type-checks against the real postretro.d.ts + postretro/ui
// plus proposed.d.ts (the WALLs). A persisted, tsc-checked target for /draft-plan.
//
// MODEL: Rust owns collision, state mutation, and the act of despawning — NOT
// "death". The engine emits a NEUTRAL health-crossing event; "death" is the name
// THIS SCRIPT gives to a LISTENER over it. defineEvent / fire / onEvent are the
// dispatch<->listen pair; events are DERIVED from a query-shaped surface.
//
// GROUNDED: a listener returns a FLAT effect SET, auto-partitioned into the two
// arms (consequential in-tick / presentation app-drain). No temporal tree; timing
// is a property (despawn afterMs). A real timed pause is a separate WALL.
//
// RECONCILED WITH THE FABLE CONSENSUS: combatKill-as-engine-event is SUPERSEDED
// (death is a listener); the two-arm split, addStore, ledger attribution, and
// ProgressTracker-retirement all SURVIVE (flagship below); subject vs source is
// SHARPENED into distinct handles. Coordination: E18-C, not M10 (shipped).
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineReaction, defineStore } from "postretro";
import { playSound, loadLevel, restartLevel } from "postretro/ui";
import {
  defineEvent, entities, fire, onEvent, slot,
  type LevelManifestWithEvents,
} from "postretro/proposed";

// Score + kill counter. Both `network: "shared"` — consequential, replicated.
// (Store DECLARATION commits via ModManifest.stores; not the level manifest.)
const econ = defineStore("arena", {
  score:  { type: "number", default: 0, network: "shared" },
  deaths: { type: "number", default: 0, network: "shared" },
});
const score  = slot(econ.state.score);
const deaths = slot(econ.state.deaths);

// LIVE standing entity sets (spawn-aware), named as queries. Events derive from
// them. "gruntDied"/"playerDied" are LISTENER-side names for a neutral crossing —
// the engine never hears the word "death".
const grunts  = entities.query({ tag: "grunt" });
const players = entities.query({ tag: "player" });
const gruntDied  = grunts.healthCrossing({ below: 0 });
const playerDied = players.healthCrossing({ below: 1 });
const waveCleared = defineEvent("arena.waveCleared"); // modder-owned; fired + heard below

// Reusable, param-free reactions (Reaction<{}> — bind to any event).
const enemySfx    = defineReaction(playSound("enemyDown"));
const openBoss    = defineReaction(loadLevel("arena-boss"));
const playerReset = defineReaction(restartLevel());

// === setupLevel returns a LevelManifest; the event surface is a nested child. ==
export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [enemySfx, openBoss, playerReset],
    events: {
      defined: [waveCleared],
      listeners: [
        // "Death" IS this listener. One JS-familiar builder returns a flat effect
        // SET; the SDK partitions it into the two arms.
        onEvent(gruntDied, (event) => [
          event.subject.playDeathAnim(),           // presentation
          event.source.grant("ammo", 5),           // consequential — credits the SOURCE
          score.add(event.overkill),               // consequential — IR leaf
          enemySfx,
          deaths.add(1),                           // consequential — retires ProgressTracker
          event.subject.despawn({ afterMs: 1500 }),// consequential — timer property
        ]),
        // A validly DIFFERENT death over the same event: the player never despawns.
        onEvent(playerDied, [playerReset]),

        // FLAGSHIP — retire the Rust ProgressTracker via the symmetry: a store
        // slot's crossing is just another event; listen to it and FIRE the modder's
        // own event when the (spawn-aware) kill count crosses the threshold.
        onEvent(deaths.crossing({ above: 7 }), [() => fire(waveCleared)]),
        onEvent(waveCleared, [openBoss]),
      ],
    },
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================

// The JS-callback trap: a statement body returns void, not an Effect — the
// discarded builder calls would record NOTHING (no live VM runs them).
// @ts-expect-error statement body returns void, not an Effect/Effect set.
const imperativeBody: (e: import("postretro/proposed").HealthEvent) => import("postretro/proposed").Effect =
  (e) => { e.subject.despawn(); };
void imperativeBody;

// Subject/source distinction, enforced by which methods EXIST:
onEvent(gruntDied, (event) => [
  // @ts-expect-error `source` has no despawn — you despawn the SUBJECT.
  event.source.despawn(),
  // @ts-expect-error `subject` has no grant — you credit the SOURCE.
  event.subject.grant("ammo", 5),
]);
