// M13 G1a author-facing fixture — typed handle surfaces.
//
// This file is a documented REVIEW GATE, not a runtime script. The repo has no
// `tsc` CI, so the `@ts-expect-error` lines below assert the intended compile-
// time behavior for a human reviewer (and for an author who opens it in an
// IDE): each marked line MUST be a type error. If a future change makes one of
// them compile cleanly, `tsc --noEmit` would flag the now-unused
// `@ts-expect-error` — the contract has drifted and the review gate fails.
//
// See: context/lib/scripting.md §5, context/lib/ui.md §3, M13 G1a Task 1.

import { defineStore, defineReaction, type StateValue } from "postretro";
import { player } from "postretro/game-state";

// --- (1) Value-typed slot handles -------------------------------------------
// `defineStore` infers each slot's value type from its `type` discriminant.
const opts = defineStore("fixtureOpts", {
  volume: { type: "number", default: 0.8 },
  muted: { type: "boolean", default: false },
  preset: { type: "string", default: "default" },
});

// Correct types: a `number` slot is `StateValue<number>`, a `boolean` slot is
// `StateValue<boolean>`, a `string` slot is `StateValue<string>`.
const _volume: StateValue<number> = opts.volume;
const _muted: StateValue<boolean> = opts.muted;
const _preset: StateValue<string> = opts.preset;

// The documented mismatch: a `boolean` slot handle is NOT assignable to a
// numeric-typed binding. This is the `@ts-expect-error` fixture the AC requires.
// @ts-expect-error — `muted` is StateValue<boolean>, not StateValue<number>.
const _wrong: StateValue<number> = opts.muted;

// --- (2) Read-only engine-slot handles --------------------------------------
// `player.health.get()` is a read-only `ReadonlyStateValue<number>` bind ref.
const _health = player.health.get();

// Engine slots are read-only to mods: `.set(...)` is absent from the handle.
// @ts-expect-error — engine slots have no `.set()`; they are read-only to mods.
player.health.set(100);

// --- (3) Typed reaction handles ---------------------------------------------
// `defineReaction` accepts an optional `name`; omitted → deterministic auto-id.
// The returned handle is the typed reaction reference (go-to-definition, no
// silent name typos) a `Button.onPress` or crossing `fire` entry accepts.
const _named = defineReaction("explicitName", {
  primitive: "playSound",
  args: { sound: "click" },
});
const _auto = defineReaction({
  primitive: "playSound",
  args: { sound: "confirm" },
});

void _volume;
void _muted;
void _preset;
void _wrong;
void _health;
void _named;
void _auto;
