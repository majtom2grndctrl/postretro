// DESIGN SPIKE — not a shipping content script. Type-checks against the real
// `postretro.d.ts` plus `proposed.d.ts` (the WALLs). Its job is to pressure-test
// the "death is a per-entity health crossing" model in a persisted, tsc-checked
// file so drift and shape errors surface mechanically instead of by re-reading.
//
// The model: Rust owns weapon impact (collision), state mutation (applyDamage),
// and the act of despawning. It does NOT own "death". Death is script observing
// the health COMPONENT cross below a threshold, and deciding what that means.
// Different games fork the reactions below.
//
// Everything imported from "postretro"          → SHIPPED and load-bearing.
// Everything imported from "postretro/proposed"  → a WALL (see proposed.d.ts).

import { defineReaction, defineStore, runtime } from "postretro";
import type { Reaction } from "postretro";
// Presentation / game-flow primitives live behind the `postretro/ui` surface —
// the root module deliberately exposes only manifest data shapes.
import { playSound, restartLevel } from "postretro/ui";
import {
  addStore,
  defineDeathReaction,
  despawn,
  onHealthCrossing,
  playDeathAnim,
  type DeathManifest,
} from "postretro/proposed";

// Score lives in a store. NOTE: the store DECLARATION commits via
// ModManifest.stores at mod init, NOT through the level manifest below — the
// level script only USES `.state` refs. (A detail the in-chat sketches missed:
// LevelManifest has no `stores` field.)
const econ = defineStore("arena", {
  score: { type: "number", default: 0, network: "shared" },
});

// --- Reference death implementation. THIS is "death", and it's forkable. ------
// Each effect is its own reaction, because a reaction body is a single primitive
// (a real constraint — you cannot bundle anim+sound+score+despawn into one
// `sequence`; that step union is mover/trigger/light only). The crossing binds
// several reactions together.

// Fact-scoped reactions read the KillScope tracer. `on.entity` is a pass-only
// token; `on.overkill` is an IR leaf.
const enemyDeathFx     = defineDeathReaction((on) => playDeathAnim(on.entity));
const enemyDespawn     = defineDeathReaction((on) => despawn(on.entity, { after: "anim" }));
const enemyOverkillXp  = defineDeathReaction((on) => addStore(econ.state.score, on.overkill));

// Unscoped reactions read no facts, so they are Reaction<{}> and bind anywhere.
const enemyDeathSfx: Reaction<{}> = defineReaction(playSound("enemyDown"));
const enemyReward: Reaction<{}>   = defineReaction(addStore(econ.state.score, 100));

// A validly DIFFERENT death over the same mechanism: the player never despawns.
const playerDeath: Reaction<{}>   = defineReaction(restartLevel());

// --- Wiring. The return is a manifest, not logic. -----------------------------
export function setupLevel(): DeathManifest {
  return {
    reactions: [
      enemyDeathFx, enemyDespawn, enemyOverkillXp, enemyDeathSfx, enemyReward, playerDeath,
    ],
    healthWatchers: [
      onHealthCrossing({ tag: "enemy" }, { below: 0 },
        [enemyDeathFx, enemyDeathSfx, enemyReward, enemyOverkillXp, enemyDespawn]),
      onHealthCrossing({ tag: "player" }, { below: 1 }, [playerDeath]),
    ],
  };
}

// --- GUARDRAILS — these MUST NOT compile. `@ts-expect-error` asserts the type
// system rejects them; if any line stops erroring, tsc fails, telling us the
// discipline eroded. Mirrors sdk/type-tests/e18-dispatch-params.ts.
defineDeathReaction((on) => {
  // @ts-expect-error entity token is not a numeric delta for addStore.
  addStore(econ.state.score, on.entity);
  // @ts-expect-error entity token is not a runtime IR operand.
  runtime.add(on.entity, 1);
  // @ts-expect-error there is no string-shaped fact on the kill scope.
  const weaponName: string = on.weaponName;
  void weaponName;
  return despawn(on.entity);
});

// @ts-expect-error a kill-scoped reaction cannot be erased to an unscoped one.
const erased: Reaction<{}> = enemyDespawn;
void erased;
