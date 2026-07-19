// PROPOSED — ERGONOMICS PROBE (module "postretro/ergo"). Isolated from the main
// proposed surface so it can't destabilize the death spike. Explores the
// JavaScript-event-listener FEEL — method-style handles + sequence/parallel — while
// keeping the hard invariant: the builder returns DATA (a tree), it is not a live
// callback. See event-ergonomics.spike.ts.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  WritableStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/ergo" {
  // A composed effect TREE — the DATA a builder returns. Leaves are descriptors;
  // `sequence` is ordered, `parallel` is a concurrent group. NOTE the real cost:
  // this is a genuinely NEW capability. Shipped reaction bodies are a single
  // primitive OR a mover/trigger/light step list — not an arbitrary tree of
  // arbitrary effects. That tree is the price of this ergonomics.
  export type Node = PrimitiveReactionDescriptor | Reaction<{}> | SeqNode | ParNode;
  export type SeqNode = { readonly seq: Node[] };
  export type ParNode = { readonly par: Node[] };
  export function sequence(nodes: Node[]): SeqNode;
  export function parallel(nodes: Node[]): ParNode;

  // Method-style handles: the pass-only token PLUS builder methods that RETURN
  // descriptors. The subject/source distinction is encoded as which methods
  // exist — despawning the source or crediting the subject is a missing method,
  // discoverable in autocomplete.
  export interface SubjectHandle {
    despawn(opts?: { after?: "anim" | "now" }): PrimitiveReactionDescriptor;
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

  // The builder MUST return a Node. This is the guard: a void statement-body
  // (discarded builder calls) is not assignable and fails to compile.
  export function onEvent<P>(event: EventDescriptor<P>, build: (event: P) => Node): { readonly event: string };
}
