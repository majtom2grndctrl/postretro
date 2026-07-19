// DESIGN SPIKE — not a shipping content script. Type-checks against the real
// postretro.d.ts + postretro/ui plus proposed.d.ts (the 10 WALLs). Its job is to
// pressure-test the "death is a per-entity health crossing" model in a persisted,
// tsc-checked file so drift and shape errors surface mechanically.
//
// MODEL: Rust owns weapon impact (collision), state mutation (applyDamage), and
// the act of despawning. It does NOT own "death". Death is script observing the
// health COMPONENT cross below a threshold and deciding what it means. Fork the
// reactions to change what death is.
//
// RECONCILED WITH THE EARLIER FABLE CONSENSUS:
//   SUPERSEDED — "combatKill is an engine event with a widened Vec<String> death
//     channel." Death is not an engine event; it's a health crossing. The
//     widened-channel insight SURVIVES as: the crossing's KillScope must carry the
//     ledger facts the sweep already stages (attacker / attributedDamage).
//   SURVIVES — the two-arm consequential/presentation split (grouped below, and
//     Effect<Arm> in proposed.d.ts); addStore as a hard requirement (absolute
//     writes break under multi-death/frame); attribution from the SHIPPED E16
//     ledger; ProgressTracker retires into a recipe (flagship below).
//   SHARPENED — victim vs killer are DISTINCT tokens: you cannot despawn the
//     killer or credit the corpse (enforced by the guardrails at the bottom).
//
// COORDINATION: the live neighbor is E18-C (spawner containment), not M10
// (shipped). Area/encounter kill-progress is a recipe over THIS crossing (see the
// flagship), not a second Rust tracker — and it is spawn-aware for free.
//
// "postretro" / "postretro/ui" imports → SHIPPED.  "postretro/proposed" → a WALL.

import { defineReaction, defineStore, runtime } from "postretro";
import type { Reaction } from "postretro";
import { playSound, loadLevel, restartLevel, onStateCrossing } from "postretro/ui";
import {
  addStore,
  defineDeathReaction,
  despawn,
  grant,
  onHealthCrossing,
  playDeathAnim,
  type DeathManifest,
} from "postretro/proposed";

// Score + a kill counter. Both `network: "shared"` — they are consequential,
// host-authoritative, replicated state. (The store DECLARATION commits via
// ModManifest.stores at mod init; a level manifest has no `stores` field.)
const econ = defineStore("arena", {
  score:  { type: "number", default: 0, network: "shared" },
  deaths: { type: "number", default: 0, network: "shared" },
});

// === Reference death implementation. THIS is "death", and it's forkable. ======
// Each effect is its own reaction — a reaction body is a SINGLE primitive (the
// sequence step-union is mover/trigger/light only, so anim+sound+score+despawn
// cannot be one body). The crossing binds several reactions together.

// --- Consequential arm: in-tick, host-authoritative, replicates via slots. ----
const enemyDespawn = defineDeathReaction((on) => despawn(on.entity, { after: "anim" }));
const enemyCredit  = defineDeathReaction((on) => grant(on.attacker, "ammo", 5)); // credits the KILLER
const overkillXp   = defineDeathReaction((on) => addStore(econ.state.score, on.overkill)); // IR leaf
const enemyReward: Reaction<{}> = defineReaction(addStore(econ.state.score, 100));
const countKill:   Reaction<{}> = defineReaction(addStore(econ.state.deaths, 1));

// --- Presentation arm: app-drain, LOCAL to each client. -----------------------
const enemyDeathFx = defineDeathReaction((on) => playDeathAnim(on.entity));
const enemyDeathSfx: Reaction<{}> = defineReaction(playSound("enemyDown"));

// A validly DIFFERENT death over the same mechanism: the player never despawns.
const playerDeath: Reaction<{}> = defineReaction(restartLevel());

// === FLAGSHIP — retire the Rust ProgressTracker as a recipe. =================
// `countKill` increments a shared slot on each enemy death (proposed addStore);
// a SHIPPED onStateCrossing reads that count to advance the encounter. The ONLY
// new seam is the event-driven delta — the threshold read already ships — and it
// is spawn-aware for free (the counter is whatever the crossing feeds it), which
// is exactly what E18-C needs and the load-time-total ProgressTracker cannot do.
const advanceEncounter: Reaction<{}> = defineReaction(loadLevel("arena-boss"));

// === Wiring. The return is a manifest, not logic. ============================
export function setupLevel(): DeathManifest {
  return {
    reactions: [
      enemyDespawn, enemyCredit, overkillXp, enemyReward, countKill,
      enemyDeathFx, enemyDeathSfx, playerDeath, advanceEncounter,
    ],
    crossings: [
      onStateCrossing(econ.state.deaths, { above: 7 }, [advanceEncounter]),
    ],
    healthWatchers: [
      onHealthCrossing({ tag: "enemy" }, { below: 0 }, [
        enemyDespawn, enemyCredit, overkillXp, enemyReward, countKill,
        enemyDeathFx, enemyDeathSfx,
      ]),
      onHealthCrossing({ tag: "player" }, { below: 1 }, [playerDeath]),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. `@ts-expect-error` asserts the type
// system rejects them; if a line stops erroring, tsc fails, telling us the
// discipline eroded. Mirrors sdk/type-tests/e18-dispatch-params.ts.
defineDeathReaction((on) => {
  // @ts-expect-error you despawn the VICTIM, not the killer.
  despawn(on.attacker);
  // @ts-expect-error you credit the KILLER, not the corpse.
  grant(on.entity, "ammo", 5);
  // @ts-expect-error an entity token is not a numeric delta.
  addStore(econ.state.score, on.entity);
  // @ts-expect-error an entity token is not a runtime IR operand.
  runtime.add(on.attacker, 1);
  // @ts-expect-error there is no string-shaped fact on the kill scope.
  const weaponName: string = on.weaponName;
  void weaponName;
  return despawn(on.entity);
});

// @ts-expect-error a kill-scoped reaction cannot be erased to an unscoped one.
const erased: Reaction<{}> = enemyDespawn;
void erased;
