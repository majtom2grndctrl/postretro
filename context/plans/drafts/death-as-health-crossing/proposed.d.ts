// PROPOSED SDK SURFACE — not shipped. Every export here is a WALL: a seam the
// death-as-health-crossing spec would have to build. Kept in a SEPARATE module
// ("postretro/proposed") from the real "postretro" so that, in the adjacent
// spike, every `from "postretro/proposed"` import is visibly a gap.
//
// Real symbols are imported from "postretro" and re-used honestly so the stub
// stays anchored to shipped shapes.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/proposed" {
  // WALL 1 — two opaque per-entity tokens the kill scope publishes. Pass-only,
  // like the shipped ActivatorsTarget/TriggerTarget brands: a token may flow into
  // a target slot but is NOT a number and NOT an IR operand. VICTIM and KILLER are
  // DISTINCT types (sharpened from the Fable consensus) so the type system stops
  // you despawning the killer or crediting the corpse.
  const victimBrand: unique symbol;
  const attackerBrand: unique symbol;
  export type DeadEntityTarget = { readonly [victimBrand]: true };
  export type AttackerTarget = { readonly [attackerBrand]: true };

  // WALL 2 — the dispatch scope a health-zero crossing publishes. Tokens are
  // pass-only; measures are IR leaves (RuntimeValue). There is deliberately NO
  // string-shaped fact. `attacker`/`attributedDamage` come straight from the
  // SHIPPED contributor ledger (E16 source-id-ledger already stages these at the
  // sweep), so exposing them is re-surfacing staged data, not new bookkeeping —
  // this is where the consensus's "widened death channel" insight now lives.
  // Field-validity: a hit-less death (DoT, lava, fall) has no killer, so
  // `attacker`/`attributedDamage` read type-zero, never null.
  export type KillScope = Readonly<{
    entity: DeadEntityTarget;       // the victim — despawn / death-anim target
    attacker: AttackerTarget;       // the killer — credit/grant target (type-zero if hit-less)
    overkill: RuntimeValue;         // IR leaf: HP below zero at the crossing
    attributedDamage: RuntimeValue; // IR leaf: the killer's share, from the ledger
  }>;

  // WALL 3 — the two-arm dispatch split (the biggest Fable-consensus correction).
  // The SDK partitions each effect BY IDENTITY, exactly like trigger commands:
  //   consequential → runs IN-TICK, host-authoritative, replicates via shared slots
  //   presentation  → runs on the app drain, LOCAL to each client
  // The author still writes ONE flat fire-list — there is no lane to mistype — so
  // the arm is a property of the primitive, surfaced as a phantom marker here so
  // the assignment stays legible (the consensus asked for an explicit arm table).
  export type Arm = "consequential" | "presentation";
  export type Effect<A extends Arm> = PrimitiveReactionDescriptor & { readonly __arm?: A };

  // WALL 4 — a per-entity crossing over the health COMPONENT. Distinct wire shape
  // from the shipped slot/predicate CrossingDescriptor (which keys on a global
  // `slot` string); a per-entity channel has no single slot, so this is a NEW
  // source kind (new Rust registry field + observer), not a generalization.
  export type HealthWatcherDescriptor = {
    component: "health";
    tag?: string;
    below: number;
    fire: string[];
    levels?: string[];
  };
  export function onHealthCrossing(
    filter: { tag?: string },
    cond: { below: number },
    fire: (Reaction<{}> | Reaction<KillScope> | string)[],
  ): HealthWatcherDescriptor;

  // WALL 5 — a KillScope tracer overload for defineReaction (the shipped one knows
  // only CrossingParams / TriggerEventParams tracers).
  export function defineDeathReaction(
    tracer: (on: KillScope) => PrimitiveReactionDescriptor,
  ): Reaction<KillScope>;

  // WALL 6 — script-invoked, script-timed despawn. Rust owns the act of removal;
  // the script owns WHEN. Consequential. Takes the VICTIM token only.
  export function despawn(
    target: DeadEntityTarget,
    opts?: { after?: "anim" | "now" },
  ): Effect<"consequential">;

  // WALL 7 — script-invoked death animation (today the AI brain auto-drives the
  // mesh `death` state). Presentation. Takes the VICTIM token.
  export function playDeathAnim(target: DeadEntityTarget): Effect<"presentation">;

  // WALL 8 — credit a resource to the KILLER (the resource-grant chokepoint from
  // the consensus). Consequential. Takes the ATTACKER token.
  export function grant(
    target: AttackerTarget,
    resource: string,
    amount: number | RuntimeValue,
  ): Effect<"consequential">;

  // WALL 9 — event-driven delta write (read-modify-write add). Consequential.
  // Today the store surface has only absolute `updateState` and a per-tick
  // `accumulate` hook; neither is correct for "add N on THIS event", and absolute
  // writes are wrong under multi-death-per-frame. Deltas COMPOSE — which is why
  // the CSS-style cascade/tie-break DSL we explored is unnecessary for the common
  // case (accumulation needs no precedence; only true overrides would).
  export function addStore(
    ref: WritableStateRef<number>,
    delta: number | RuntimeValue,
  ): Effect<"consequential">;

  // WALL 10 — LevelManifest has no field to carry per-entity health crossings.
  export type DeathManifest = LevelManifest & {
    healthWatchers: HealthWatcherDescriptor[];
  };
}
