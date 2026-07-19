// PROPOSED SDK SURFACE — not shipped. Every export here is a WALL: a seam the
// death-as-health-crossing spec would have to build. Kept in a SEPARATE module
// ("postretro/proposed") from the real "postretro" so that, in the adjacent
// spike, every `from "postretro/proposed"` import is visibly a gap.
//
// Real symbols are imported from "postretro" and re-used honestly so the stub
// stays anchored to shipped shapes (PrimitiveReactionDescriptor, Reaction<S>,
// LevelManifest, WritableStateRef, RuntimeValue).

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/proposed" {
  // WALL 1 — per-entity opaque handle for the entity that died. Pass-only, like
  // the shipped ActivatorsTarget/TriggerTarget brands: it may flow into a target
  // slot but is NOT a numeric value and NOT an IR operand.
  const deadEntityBrand: unique symbol;
  export type DeadEntityTarget = { readonly [deadEntityBrand]: true };

  // WALL 2 — the dispatch scope a health-zero crossing publishes. `entity` is a
  // pass-only token; `overkill` is an IR leaf (RuntimeValue). There is
  // deliberately NO string-shaped fact: categoricals would arrive as enum codes.
  export type KillScope = Readonly<{
    entity: DeadEntityTarget;
    overkill: RuntimeValue;
  }>;

  // WALL 3 — a per-entity crossing over the health COMPONENT. Distinct wire shape
  // from the shipped slot/predicate CrossingDescriptor (which keys on a global
  // `slot` string); a per-entity component channel has no single slot, so this is
  // a NEW source kind (new Rust registry field + observer), not a generalization.
  export type HealthWatcherDescriptor = {
    component: "health";
    tag?: string;
    below: number;
    fire: string[];
    levels?: string[];
  };

  // WALL 3b — bind reactions to the health-zero edge. Mirrors onTriggerEvent's
  // shape: a filter, a condition, and a fire-list that may mix unscoped and
  // kill-scoped reactions (and bare names).
  export function onHealthCrossing(
    filter: { tag?: string },
    cond: { below: number },
    fire: (Reaction<{}> | Reaction<KillScope> | string)[],
  ): HealthWatcherDescriptor;

  // WALL 4 — a KillScope tracer overload for defineReaction. The shipped
  // defineReaction only knows CrossingParams / TriggerEventParams tracers.
  export function defineDeathReaction(
    tracer: (on: KillScope) => PrimitiveReactionDescriptor,
  ): Reaction<KillScope>;

  // WALL 5 — script-invoked despawn with script-chosen timing. Rust owns the act
  // of removal; the script owns WHEN. (Different games despawn differently.)
  export function despawn(
    target: DeadEntityTarget,
    opts?: { after?: "anim" | "now" },
  ): PrimitiveReactionDescriptor;

  // WALL 6 — script-invoked death animation. Today the AI brain drives the mesh
  // `death` state automatically; a script-authored death needs to trigger it.
  export function playDeathAnim(target: DeadEntityTarget): PrimitiveReactionDescriptor;

  // WALL 7 — event-driven delta write (read-modify-write add). Today the store
  // surface has only absolute `updateState` and a per-tick `accumulate` schema
  // hook — neither is correct for "add N on this event", and absolute writes are
  // wrong under multi-death-per-frame.
  export function addStore(
    ref: WritableStateRef<number>,
    delta: number | RuntimeValue,
  ): PrimitiveReactionDescriptor;

  // WALL 8 — LevelManifest has no field to carry per-entity health crossings.
  export type DeathManifest = LevelManifest & {
    healthWatchers: HealthWatcherDescriptor[];
  };
}
