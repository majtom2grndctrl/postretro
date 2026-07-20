// PROPOSED SDK SURFACE — not shipped. Every export is a WALL the spec would build.
// Module "postretro/proposed" so each import in the spike is visibly a gap.
//
// THE MODEL (grounded): the engine owns IMPACT — the damage chokepoint
// (apply_damage_with_context, the single HP-decrement site, health.rs:329). Every
// HP reduction flows through it and nothing reaches <=0 HP any other way, so DEATH
// is always DERIVABLE from impact. The engine does NOT own death; it emits ONE
// net-new event (impact), and modders DEFINE kill / overkill / xp by deriving them
// from it. There is no engine "health-crossing event" — a crossing is an apply-time
// PREDICATE on the impact.
//
// Two hard rules the types enforce below:
//   1. Derived facts (isKill, xpReward) are IR EXPRESSIONS, not live JS — there is
//      no VM at fire time. `200 * ref` must fail; `ref.times(200)` is the way.
//   2. SUBJECT (damaged) and SOURCE (damager) are distinct handles.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
} from "postretro";

declare module "postretro/proposed" {
  // ---- IR value refs. The IR is Number/Bool; distinct TS brands give safety.
  //      Accessors return refs (never raw values), so JS arithmetic on them fails.
  const numBrand: unique symbol;
  const boolBrand: unique symbol;
  export interface NumberRef {
    readonly [numBrand]: true;
    times(n: NumberValue): NumberRef;
    plus(n: NumberValue): NumberRef;
    lt(n: number): BoolRef;
  }
  export interface BoolRef {
    readonly [boolBrand]: true;
  }
  export type NumberValue = number | NumberRef;

  // ---- Effects; the SDK derives each one's arm (consequential / presentation) by
  //      identity. Additive deltas commute; timing is a property, not composition.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;

  // ---- Handles on the impact event. Accessors return IR REFS; methods return
  //      effect descriptors. Subject (damaged) vs source (damager) are distinct.
  export interface SubjectHandle {
    // apply-time edge predicate: true only on the impact that crossed the
    // boundary (once-per-death — derivable from pre/post health at the chokepoint).
    crossedBelow(threshold: number): BoolRef;
    readonly healthAfter: NumberRef;
    readonly level: NumberRef;                            // per-entity stat → a leaf
    despawn(opts?: { afterMs?: number }): Effect;         // consequential; timer property
    playDeathAnim(): Effect;                              // presentation
  }
  export interface SourceHandle {
    grant(resource: string, amount: NumberValue): Effect; // consequential
  }

  // ---- Store slot handle: write (delta) + derive a crossing event, symmetric
  //      with entities.query().onImpact.
  export interface NumberSlot {
    add(delta: NumberValue): Effect;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  // ---- The ONE net-new engine event: impact, at the damage chokepoint. Carries
  //      the damaged/damaging entities and the amount dealt. DEATH is derived, not
  //      emitted.
  export type ImpactEvent = Readonly<{
    target: SubjectHandle;
    source: SourceHandle;
    amount: NumberRef;
  }>;

  // A conditional effect group: `when` (a BoolRef) gates the group at fire time;
  // omitted = always. This is how derived semantics (kill, overkill) drive effects.
  export type GatedEffect = { when?: BoolRef; do: readonly Effect[] };

  // ---- Entity query: a LIVE STANDING selector — spawn-aware, distinct from the
  //      shipped world.query snapshot. `onImpact` DEFINES a derived behavior over
  //      the impact stream: the builder computes IR facts and returns gated effects.
  //      (Runs ONCE at load to emit data — it is not a live callback.)
  export interface EntitySet {
    // ONE-STAGE: the impact handler computes IR facts AND returns gated effects.
    // Complete for a single behavior; the facts are private to this handler.
    onImpact(build: (impact: ImpactEvent) => readonly GatedEffect[]): EventBehavior;

    // TWO-STAGE (ALTERNATIVE — a fork, not a committed wall): the handler ENRICHES
    // impact into a NAMED DERIVED EVENT (an edge + a payload of IR facts / tokens),
    // and a SEPARATE consumer does the effects. This is "we aren't adding a listener,
    // we're DEFINING a new event" — it pays off when MANY consumers want "kill".
    defineEvent<P extends Props>(
      build: (impact: ImpactEvent) => Enrichment<P>,
    ): DerivedEvent<P>;
  }
  export interface Entities {
    query(filter: { tag?: string }): EntitySet;
  }
  export const entities: Entities;

  export type EventBehavior = { readonly kind: "impact"; readonly tag?: string };

  // ---- TWO-STAGE support types. A payload is a record of IR refs and pass-through
  //      tokens (no raw values — JS math on them still fails). `when` is the derived
  //      event's firing edge; `props` becomes the consumer's payload, typed exactly.
  export type PropValue = NumberRef | BoolRef | SubjectHandle | SourceHandle;
  export type Props = Record<string, PropValue>;
  export type Enrichment<P extends Props> = { when?: BoolRef; props: P };
  export type EffectOrGroup = Effect | GatedEffect;
  export interface DerivedEvent<P extends Props> {
    on(consume: (payload: P) => readonly EffectOrGroup[]): EventBehavior;
  }

  // ---- Manifest: setupLevel returns the real LevelManifest; the derived-event
  //      behaviors are an OPTIONAL CHILD (the spec adds `events?` to LevelManifest
  //      itself). The same shape lives at ModManifest scope for the global
  //      reference behavior; a map overrides by adding its own.
  export type LevelManifestWithEvents = LevelManifest & { events?: EventBehavior[] };
}
