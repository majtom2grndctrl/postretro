// PROPOSED SDK SURFACE — not shipped. Every export is a WALL the spec would build.
// Module "postretro/proposed" so each import in the spike is visibly a gap.
//
// THE MODEL (grounded): the engine owns IMPACT — the damage chokepoint
// (apply_damage_with_context, the single HP-decrement site, health.rs:319-329). Every
// HP reduction flows through it, so DEATH is always DERIVABLE from impact. The engine
// does NOT own death; it emits ONE net-new event (impact), and modders DEFINE a named
// derived event (kill) ONCE and let any number of consumers bind effects to it.
//
// GROUNDED CONSTRAINTS the types encode:
//   1. The IR is the shipped closed enum IrNode (15 ops: add/sub/mul/div/clamp/lerp,
//      lt/le/gt/ge/eq/ne, select, const, input). Number/Bool only. These refs are an
//      ergonomic SKIN over the already-shipped `runtime.*` builder — NOT a new
//      evaluator. JS arithmetic on a ref (`200 * ref`, `ref > 0`) must fail.
//   2. There is NO and/or/not opcode — boolean composition desugars to `select`.
//   3. Health is FLOORED at 0 at the chokepoint (health.rs:329): `healthAfter` is
//      never negative. Kill/overkill MUST derive from healthBefore/amount, not from
//      `healthAfter < 0` (which can never be true). This is why `died()` exists and
//      `healthBefore` is exposed.
//   4. TARGET (damaged) and SOURCE (damager) are distinct handles.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
} from "postretro";

declare module "postretro/proposed" {
  // ---- IR value refs. Operands accept number|NumberRef on BOTH sides so two DYNAMIC
  //      quantities can combine (amount vs healthBefore). Mirrors runtime.*'s op-set
  //      rather than hand-picking a slice — a hand-picked set churns on every new need.
  const numBrand: unique symbol;
  const boolBrand: unique symbol;

  export type NumberValue = number | NumberRef;
  export type BoolValue = boolean | BoolRef;

  export interface NumberRef {
    readonly [numBrand]: true;
    plus(n: NumberValue): NumberRef;                    // runtime.add
    minus(n: NumberValue): NumberRef;                   // runtime.sub — overkill magnitude
    times(n: NumberValue): NumberRef;                   // runtime.mul
    dividedBy(n: NumberValue): NumberRef;               // runtime.div (÷0 → 0; total evaluator)
    clamp(lo: NumberValue, hi: NumberValue): NumberRef; // runtime.clamp
    lerp(to: NumberValue, t: NumberValue): NumberRef;   // runtime.lerp
    lt(n: NumberValue): BoolRef;  le(n: NumberValue): BoolRef;
    gt(n: NumberValue): BoolRef;  ge(n: NumberValue): BoolRef;
    eq(n: NumberValue): BoolRef;  ne(n: NumberValue): BoolRef;
  }

  // Boolean composition is PURE SUGAR over `select` (the only conditional opcode) —
  // the shipped IR has no and/or/not:
  //   a.and(b) => select(a, b, false)   a.or(b) => select(a, true, b)
  //   a.not()  => select(a, false, true)   a.select(x, y) => branchless numeric pick
  export interface BoolRef {
    readonly [boolBrand]: true;
    and(other: BoolValue): BoolRef;
    or(other: BoolValue): BoolRef;
    not(): BoolRef;
    select(whenTrue: NumberValue, whenFalse: NumberValue): NumberRef;
  }

  // ---- Effects. The SDK derives each one's arm by identity. Presentation effects
  //      resolve to BAKED curves (like setLightAnimation's sample table); consequential
  //      ones carry IR. The two-arm split IS the IR/bake boundary.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;
  export type GatedEffect = { when?: BoolRef; do: readonly Effect[] };
  export type EffectOrGroup = Effect | GatedEffect;   // consumers may return bare effects OR gated groups

  // ---- Handles on the impact event. Accessors return IR REFS; methods return effect
  //      descriptors. TARGET (damaged) vs SOURCE (damager) are distinct and non-confusable.
  export interface TargetHandle {
    // BOTH sides of the impact — required for a sound kill/overkill test. Health is
    // floored at 0 at the chokepoint, so `healthAfter` is NEVER < 0. Never test a kill
    // or overkill against `healthAfter.lt(0)`.
    readonly healthBefore: NumberRef;
    readonly healthAfter: NumberRef;
    readonly level: NumberRef;                          // per-entity stat → a leaf
    // The BLESSED kill edge: healthBefore > 0 && healthAfter <= 0. Inclusive lower edge
    // (a lethal hit lands exactly on 0 after the floor) and excludes an already-dead
    // target (healthBefore == 0). Desugars to select(healthBefore.gt(0), healthAfter.le(0), false).
    died(): BoolRef;
    despawn(opts?: { afterMs?: number }): Effect;       // consequential; timer property
    playDeathAnim(): Effect;                            // presentation; baked curve
  }
  export interface SourceHandle {
    grant(resource: string, amount: NumberValue): Effect; // consequential
  }

  // ---- Store slot handle: additive write, symmetric with the derived-event surface.
  export interface NumberSlot {
    add(delta: NumberValue): Effect;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  // ---- The ONE net-new engine event: impact, at the damage chokepoint. Carries the
  //      damaged/damaging entities and the amount dealt. DEATH is DERIVED, not emitted.
  export type ImpactEvent = Readonly<{
    target: TargetHandle;
    source: SourceHandle;
    amount: NumberRef;   // requested damage (pre-floor); overkill = amount − healthBefore
  }>;

  // ---- Derived-event payload: named IR refs + pass-through entity tokens. NB: BOTH the
  //      enrich builder AND every consumer run ONCE AT LOAD to emit data — the payload is
  //      authored IR, not a live fire-time object. A handle in the payload is a token you
  //      derive more refs/effects from; it does not "survive" to fire time.
  export type PropValue = NumberRef | BoolRef | TargetHandle | SourceHandle;
  export type Props = Record<string, PropValue>;
  export type Enrichment<P extends Props> = { when?: BoolRef; props: P };

  const behaviorBrand: unique symbol;
  // Carries its payload type P so a data-losing `{ kind: "impact" }` stub does NOT
  // type-check as a behavior: the authored gated effects + IR must provably survive the
  // load→manifest seam. The brand symbol is module-private, so a behavior can only come
  // from `.on(...)`, never a hand-written literal.
  export interface EventBehavior<P extends Props = Props> {
    readonly kind: "impact";
    readonly tag?: string;
    readonly [behaviorBrand]: P;
  }

  // ---- A named derived event: DEFINE it once (enrich impact → firing edge + payload),
  //      then any number of independent consumers bind effects to it. This is the literal
  //      "we're defining a new event," and reuse is WHY death is derived not engine-owned.
  export interface DerivedEvent<P extends Props> {
    on(consume: (payload: P) => readonly EffectOrGroup[]): EventBehavior<P>;
  }

  // ---- Entity query: a LIVE STANDING selector — spawn-aware, distinct from the shipped
  //      world.query snapshot (postretro.d.ts:188). `defineEvent` runs ONCE at load to
  //      emit a derived-event descriptor; it is not a live callback.
  export interface EntitySet {
    defineEvent<P extends Props>(
      build: (impact: ImpactEvent) => Enrichment<P>,
    ): DerivedEvent<P>;
  }
  export interface Entities {
    query(filter: { tag?: string }): EntitySet;
  }
  export const entities: Entities;

  // ---- Manifest: setupLevel returns the real LevelManifest; the derived-event behaviors
  //      are an OPTIONAL CHILD. TODO(spec): `events` almost certainly LOWERS INTO
  //      reactions + a chokepoint-registered predicate at build time (the impact event is
  //      apply-site, NOT a tick-polled store crossing), rather than sitting orthogonal to
  //      reactions/crossings. Modeled here as an intersection alias only as a placeholder.
  export type LevelManifestWithEvents = LevelManifest & {
    events?: readonly EventBehavior[];
  };
}
