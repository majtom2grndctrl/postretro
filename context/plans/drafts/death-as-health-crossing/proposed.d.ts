// PROPOSED SDK SURFACE — not shipped. Every export here is a WALL the
// death-as-health-crossing spec would build. Separate module ("postretro/proposed")
// so each import in the spike is visibly a gap.
//
// This revision adds the SYMMETRIC half of NamedEventDispatch: a listen/observe
// surface. We had firing (dispatch) but no unified listener, so every "listen"
// was re-invented per source. Here:
//   defineEvent()          — declare a modder-owned (script-fired) event
//   fire(event, params?)   — emit a script event from a reaction
//   onEvent(event, react)  — LISTEN (bind reactions; the tracer runs once at load)
//   healthCrossing(...)     — an engine-PREDEFINED event (you listen, you don't fire)
//
// There is deliberately NO "kill"/"death" concept in the engine surface. The
// engine emits a NEUTRAL "health crossed below N" event; "death" is the name a
// modder gives to a LISTENER over it.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
  ReadonlyStateRef,
  RuntimeValue,
} from "postretro";

declare module "postretro/proposed" {
  // ---- Tokens: pass-only, neutral. SUBJECT (the entity an event is about) and
  //      SOURCE (the last damage contributor, from the ledger) are DISTINCT so the
  //      type system stops you despawning the source or crediting the subject.
  const subjectBrand: unique symbol;
  const sourceBrand: unique symbol;
  export type SubjectRef = { readonly [subjectBrand]: true };
  export type SourceRef = { readonly [sourceBrand]: true };

  // ---- Two-arm dispatch: consequential (in-tick, host-authoritative, replicated)
  //      vs presentation (app-drain, local). The SDK partitions by identity; the
  //      arm is a legible phantom marker on each primitive.
  export type Arm = "consequential" | "presentation";
  export type Effect<A extends Arm> = PrimitiveReactionDescriptor & { readonly __arm?: A };

  // ==== The symmetric event surface ==========================================

  // An event you can listen to. `P` is the param published when it fires.
  const eventParamBrand: unique symbol;
  export type EventDescriptor<P> = { readonly name: string; readonly [eventParamBrand]?: (p: P) => void };

  // A reaction bound to an event: a param-free handle (reusable anywhere), a
  // param-reading inline tracer (runs once at load → a descriptor, NOT a live
  // callback), or a name. This is where `onEvent(evt, (param) => …)` lives.
  export type EventReaction<P> =
    | ((on: P) => PrimitiveReactionDescriptor)
    | Reaction<{}>
    | Reaction<P>
    | string;

  export type ListenerDescriptor = { event: string; fire: string[]; levels?: string[] };

  // DECLARE a modder-owned event (pure script pub/sub; needs no engine edge).
  export function defineEvent<P = {}>(name: string): EventDescriptor<P>;
  // FIRE a script event from a reaction. Consequential (routes the drain).
  export function fire<P>(event: EventDescriptor<P>, params?: P): Effect<"consequential">;
  // LISTEN. Bind reactions/tracers to any event — engine-predefined or script.
  export function onEvent<P>(event: EventDescriptor<P>, react: EventReaction<P>[]): ListenerDescriptor;
  export function onEvent<P>(event: EventDescriptor<P>, tracer: (on: P) => PrimitiveReactionDescriptor): ListenerDescriptor;

  // ==== Engine-PREDEFINED events (the engine ships the descriptor) ============

  // A per-entity health-COMPONENT crossing. NEUTRAL: no "kill" concept. The param
  // is the crossed entity plus its observable component state — the ledger is
  // component state, like health.current, so `source`/`attributedDamage` ride
  // along the same way. Field-validity: a hit-less crossing (DoT, lava, fall) has
  // no contributor, so `source`/`attributedDamage` read type-zero, never null.
  export type HealthCrossing = Readonly<{
    subject: SubjectRef;             // the entity that crossed
    source: SourceRef;               // last damage contributor (type-zero if none)
    overkill: RuntimeValue;          // IR leaf: how far below the threshold
    attributedDamage: RuntimeValue;  // IR leaf: the source's ledger share
  }>;
  export function healthCrossing(filter: { tag?: string }, cond: { below: number }): EventDescriptor<HealthCrossing>;

  // The SHIPPED state-crossing, re-expressed as an event constructor so the SAME
  // onEvent listens to it (the shipped onStateCrossing folds into onEvent).
  export type Crossing = Readonly<{ rising: RuntimeValue }>;
  export function stateCrossing(slot: ReadonlyStateRef<number>, cond: { below?: number; above?: number }): EventDescriptor<Crossing>;

  // ==== Effects the death recipe uses ========================================
  export function despawn(target: SubjectRef, opts?: { after?: "anim" | "now" }): Effect<"consequential">;
  export function playDeathAnim(target: SubjectRef): Effect<"presentation">;
  export function grant(target: SourceRef, resource: string, amount: number | RuntimeValue): Effect<"consequential">;
  export function addStore(ref: WritableStateRef<number>, delta: number | RuntimeValue): Effect<"consequential">;

  // Unified manifest: one `listeners` array replaces the separate crossings /
  // triggerEvents fields, plus a registry for script-defined `events`.
  export type EventManifest = LevelManifest & {
    events?: EventDescriptor<any>[];
    listeners: ListenerDescriptor[];
  };
}
