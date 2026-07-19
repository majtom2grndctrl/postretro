// PROPOSED SDK SURFACE — not shipped. Every export is a WALL the
// death-as-health-crossing spec would build. Module "postretro/proposed" so each
// import in the spike is visibly a gap. This is the UNIFIED surface (it replaced
// the earlier split proposed/proposed-ergo probes).
//
// THE MODEL, in one breath: Rust owns collision, state mutation, and the act of
// despawning. It does NOT own "death". The engine emits a NEUTRAL event — an
// entity's health crossed below N — carrying the crossed entity plus its
// observable component state (the ledger is component state, so source /
// attributedDamage ride along; nothing is a "kill payload"). "Death" is the name a
// modder gives to a LISTENER over that event.
//
// SYMMETRY: defineEvent / fire / onEvent are the dispatch<->listen pair. Events
// are DERIVED from things via a query-shaped surface: entities.query(...).
// healthCrossing(...) and slot(...).crossing(...). onEvent binds listeners.
//
// GROUNDED DECISIONS (see the four-question probe): a listener returns a FLAT
// effect SET, auto-partitioned into the two arms (consequential in-tick /
// presentation app-drain) by primitive identity — no temporal sequence/parallel
// tree (shipped sequencing is instant; cross-arm order is fixed). Timing is a
// PROPERTY (despawn afterMs, reusing death_despawn_ms), not composition. A real
// timed pause is a SEPARATE WALL (a duration primitive).

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/proposed" {
  // ---- Effects. A single leaf; the SDK derives its arm (consequential /
  //      presentation) by identity. Additive deltas commute; same-slot absolute
  //      writes are order-sensitive (prefer deltas). No temporal composition.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;

  // ---- Method-style handles: the pass-only token PLUS builder methods returning
  //      descriptors. SUBJECT (the entity an event is about) and SOURCE (last
  //      damage contributor, from the ledger) are DISTINCT — despawning the source
  //      or crediting the subject is a missing method.
  export interface SubjectHandle {
    // WALL: script-invoked despawn. Timing is a PROPERTY reusing death_despawn_ms
    // (an ms timer, NOT wait-for-anim-completion). Consequential.
    despawn(opts?: { afterMs?: number }): PrimitiveReactionDescriptor;
    // WALL: script-invoked death animation (today the AI brain auto-drives it). Presentation.
    playDeathAnim(): PrimitiveReactionDescriptor;
  }
  export interface SourceHandle {
    // WALL: the resource-grant chokepoint, crediting the killer. Consequential.
    grant(resource: string, amount: number | RuntimeValue): PrimitiveReactionDescriptor;
  }

  // ---- Store slot handle: write (.add — WALL: event-driven delta) + derive a
  //      crossing event (.crossing), symmetric with entities.query().healthCrossing.
  export interface NumberSlot {
    add(delta: number | RuntimeValue): PrimitiveReactionDescriptor;
    crossing(cond: { above?: number; below?: number }): EventDescriptor<CrossingEvent>;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  // ---- Entity query: a LIVE STANDING selector — distinct from the shipped
  //      world.query SNAPSHOT. Spawn-aware: entities matching the tag that spawn
  //      later are included (the E18-C need). Events hang off it, so the query
  //      reads as a query.
  export interface EntitySet {
    healthCrossing(cond: { below: number }): EventDescriptor<HealthEvent>;
    // future: impact(), damaged(), ...
  }
  export interface Entities {
    query(filter: { tag?: string }): EntitySet;
  }
  export const entities: Entities;

  // ---- The symmetric event surface ----
  const eventBrand: unique symbol;
  export type EventDescriptor<P> = { readonly name: string; readonly [eventBrand]?: (p: P) => void };

  // A bound reaction: a param-reading inline builder (runs ONCE at load → data, not
  // a live callback), a param-free reusable handle, or a name.
  export type EventReaction<P> = ((on: P) => Effect | readonly Effect[]) | Effect | string;
  export type ListenerDescriptor = { event: string; fire: string[]; levels?: string[] };

  // DECLARE a modder-owned (script-fired) event. FIRE it. LISTEN to any event.
  export function defineEvent<P = {}>(name: string): EventDescriptor<P>;
  export function fire<P>(event: EventDescriptor<P>, params?: P): PrimitiveReactionDescriptor;
  export function onEvent<P>(event: EventDescriptor<P>, react: EventReaction<P>[]): ListenerDescriptor;
  export function onEvent<P>(event: EventDescriptor<P>, build: (on: P) => Effect | readonly Effect[]): ListenerDescriptor;

  // ---- Neutral event params (tokens pass-only; measures are IR leaves; NO string
  //      fact). Field-validity: a hit-less crossing (DoT/lava/fall) has no
  //      contributor, so source / attributedDamage read type-zero, never null.
  export type HealthEvent = Readonly<{
    subject: SubjectHandle;
    source: SourceHandle;
    overkill: RuntimeValue;
    attributedDamage: RuntimeValue;
  }>;
  export type CrossingEvent = Readonly<{ rising: RuntimeValue }>;

  // ---- Manifest. setupLevel returns the real LevelManifest; the event surface is
  //      a nested `events` CHILD (the real spec would add `events?` to LevelManifest).
  export type EventManifest = { defined?: EventDescriptor<any>[]; listeners: ListenerDescriptor[] };
  export type LevelManifestWithEvents = LevelManifest & { events?: EventManifest };
}
