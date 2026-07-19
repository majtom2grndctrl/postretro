// PROPOSED — ERGONOMICS PROBE (module "postretro/ergo"). Isolated from the main
// proposed surface. Explores the JavaScript-event-listener FEEL — method-style
// handles — while keeping the hard invariant: the builder returns DATA, it is not
// a live callback.
//
// REVISED after grounding the sequence/parallel question against shipped code:
//   * Shipped `sequence` is instant (no delay/wait/duration) — so a tree cannot
//     mean temporal "then".  (reaction_dispatch.rs:424, reactions.rs:29)
//   * Cross-arm order is FIXED: consequential in-tick, THEN presentation on the
//     app drain — so a cross-arm "then" is not authorable (and is backwards vs a
//     recipe like "anim then despawn").  (sim/mod.rs:211 → main.rs:2075)
//   * "despawn after death" is already an engine timer PROPERTY (death_despawn_ms),
//     not composition.  (health.rs:143, ai.rs:622)
// So the rich sequence/parallel TREE collapses: a handler is a flat effect SET,
// auto-partitioned by arm; timing lives on effect properties; a real timed pause
// is a SEPARATE WALL (a duration/wait primitive). `parallel` was the flat set all
// along.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  WritableStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/ergo" {
  // A single effect leaf. A handler returns one, or a flat SET of them (an array).
  // The SDK auto-partitions the set into the two arms (consequential in-tick,
  // presentation app-drain). Within an arm, additive deltas commute; only same-slot
  // absolute writes are order-sensitive (a footgun — prefer deltas). There is no
  // temporal composition here: everything in the set dispatches at one instant.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;

  // Method-style handles: the pass-only token PLUS builder methods that RETURN
  // descriptors. Subject vs source is encoded as which methods exist.
  export interface SubjectHandle {
    // timer property, NOT a sequence edge; reuses death_despawn_ms.
    despawn(opts?: { afterMs?: number }): PrimitiveReactionDescriptor;
    playDeathAnim(): PrimitiveReactionDescriptor;
  }
  export interface SourceHandle {
    grant(resource: string, amount: number | RuntimeValue): PrimitiveReactionDescriptor;
  }
  export interface NumberSlot {
    add(delta: number | RuntimeValue): PrimitiveReactionDescriptor;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  export type HealthEvent = Readonly<{
    target: SubjectHandle;
    source: SourceHandle;
    overkill: RuntimeValue;
  }>;

  const eventBrand: unique symbol;
  export type EventDescriptor<P> = { readonly name: string; readonly [eventBrand]?: (p: P) => void };
  export function healthCrossing(filter: { tag?: string }, cond: { below: number }): EventDescriptor<HealthEvent>;

  // The builder MUST return an Effect (or a set). A void statement-body — the
  // JS-callback trap — is not assignable and fails to compile.
  export function onEvent<P>(event: EventDescriptor<P>, build: (event: P) => Effect | readonly Effect[]): { readonly event: string };
}
