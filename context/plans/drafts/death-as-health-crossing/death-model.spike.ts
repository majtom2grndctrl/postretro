// DESIGN SPIKE — not a shipping content script. Type-checks against the real
// postretro.d.ts + postretro/ui plus proposed.d.ts. Pressure-tests the model in a
// persisted, tsc-checked file so drift and shape errors surface mechanically.
//
// MODEL: Rust owns weapon impact (collision), state mutation (applyDamage), and
// the act of despawning. It does NOT own "death". The engine emits a NEUTRAL
// event — "an entity's health crossed below N" — carrying the crossed entity plus
// its observable component state (the ledger is component state, so `source` /
// `attributedDamage` ride along; nothing here is a "kill payload"). "Death" is the
// name THIS SCRIPT gives to a LISTENER over that neutral event. Fork the listener
// to change what death is.
//
// THE SYMMETRY (this revision): we had NamedEventDispatch (firing) but no unified
// listener. Now `defineEvent` / `fire` / `onEvent` are the symmetric pair.
//   - Engine-PREDEFINED events (healthCrossing, stateCrossing): the engine ships
//     the descriptor; you listen, you don't fire.
//   - SCRIPT events (defineEvent): the modder owns both sides — fire and listen.
//
// RECONCILED WITH THE FABLE CONSENSUS:
//   SUPERSEDED — "combatKill is an engine event." Death is a listener over a
//     neutral health-crossing event; the engine has no kill concept.
//   SURVIVES — two-arm split (Effect<Arm>); addStore as a hard requirement;
//     attribution from the SHIPPED E16 ledger (now framed as component state);
//     ProgressTracker retires into a recipe (flagship below).
//   SHARPENED — subject vs source are DISTINCT tokens (guardrails at the bottom).
//
// COORDINATION: live neighbor is E18-C (spawner containment), not M10 (shipped).
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineReaction, defineStore, runtime } from "postretro";
import { playSound, loadLevel, restartLevel } from "postretro/ui";
import {
  addStore, defineEvent, despawn, fire, grant, healthCrossing, onEvent,
  playDeathAnim, stateCrossing,
  type EventManifest,
} from "postretro/proposed";

// Score + kill counter. Both `network: "shared"` — consequential, replicated.
// (The store DECLARATION commits via ModManifest.stores; a level manifest has
// no `stores` field.)
const econ = defineStore("arena", {
  score:  { type: "number", default: 0, network: "shared" },
  deaths: { type: "number", default: 0, network: "shared" },
});

// Events. Engine ships `healthCrossing`/`stateCrossing`; the modder defines
// `waveCleared`. "enemyDied"/"playerDied" are just LISTENER-side names for a
// neutral health crossing — the engine never hears the word "death".
const enemyDied  = healthCrossing({ tag: "enemy" }, { below: 0 });
const playerDied = healthCrossing({ tag: "player" }, { below: 1 });
const waveCleared = defineEvent("arena.waveCleared"); // modder-owned; fired + heard below

// Reusable, param-free reactions (Reaction<{}> — bind to any event).
const enemyReward = defineReaction(addStore(econ.state.score, 100));
const countKill   = defineReaction(addStore(econ.state.deaths, 1));
const enemySfx    = defineReaction(playSound("enemyDown"));
const playerReset = defineReaction(restartLevel());
const openBoss    = defineReaction(loadLevel("arena-boss"));

// === Wiring. Listeners over events; the return is a manifest, not logic. ======
export function setupLevel(): EventManifest {
  return {
    reactions: [enemyReward, countKill, enemySfx, playerReset, openBoss],
    events: [waveCleared],
    listeners: [
      // "Death" is this listener. Param-reading effects are inline tracers
      // (on => …); reusable param-free reactions are passed by handle.
      onEvent(enemyDied, [
        // presentation arm (local)
        (on) => playDeathAnim(on.subject),
        enemySfx,
        // consequential arm (in-tick, replicated)
        (on) => despawn(on.subject, { after: "anim" }),
        (on) => grant(on.source, "ammo", 5),          // credits the SOURCE
        (on) => addStore(econ.state.score, on.overkill), // IR leaf
        enemyReward,
        countKill,
      ]),
      // A validly DIFFERENT death over the same event: the player never despawns.
      onEvent(playerDied, [playerReset]),

      // FLAGSHIP — retire the Rust ProgressTracker as a recipe, via the symmetry:
      // a SHIPPED state-crossing is just another event; listen to it and FIRE the
      // modder's own event when the (spawn-aware) kill count hits the threshold.
      onEvent(stateCrossing(econ.state.deaths, { above: 7 }), [() => fire(waveCleared)]),
      onEvent(waveCleared, [openBoss]),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. `@ts-expect-error` asserts rejection;
// if a line stops erroring, tsc fails. Mirrors sdk/type-tests/e18-dispatch-params.
void onEvent(enemyDied, [
  (on) => {
    // @ts-expect-error you despawn the SUBJECT, not the damage source.
    despawn(on.source);
    // @ts-expect-error you credit the SOURCE, not the subject.
    grant(on.subject, "ammo", 5);
    // @ts-expect-error an entity token is not a numeric delta.
    addStore(econ.state.score, on.subject);
    // @ts-expect-error an entity token is not a runtime IR operand.
    runtime.add(on.source, 1);
    // @ts-expect-error there is no string-shaped fact on the event param.
    const weaponName: string = on.weaponName;
    void weaponName;
    return despawn(on.subject);
  },
]);
