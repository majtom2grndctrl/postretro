// PROPOSED SDK SURFACE — not shipped. Every export is a WALL the spec would build.
// Module "postretro/proposed" so each import in the spike is visibly a gap.
//
// THE MODEL (grounded): the engine owns IMPACT — the damage chokepoint, the single
// HP-decrement site (health.rs:319-329). DEATH is NOT an engine concept. The engine
// emits ONE net-new event (impact); the modder writes what counts as death, per entity
// kind, as a POLICY over impact facts.
//
//   Quake zombie: 0 HP means FLOP-AND-RESURRECT, not death — only a gib-level overshoot
//   actually kills. The same `healthAfter <= 0` means "dead" for a grunt and "temporarily
//   down" for a zombie. No engine predicate can decide that, so there is none. There is
//   deliberately NO blessed `died()` — an author computes their own kill condition in IR.
//
// COMPOSITION IS OVERRIDE-BY-ORDER, not fan-out: re-`query` a narrower set and define its
// `onImpact` LATER; the later definition wins for entities both queries match. One uniform
// syntax covers the global reference behavior AND per-arena overrides. (No named-event
// value, no consumer registry — that two-stage machinery bought nothing here.)
//
// GROUNDED IR CONSTRAINTS the types encode:
//   1. The IR is the shipped closed enum IrNode (15 ops). These refs are an ergonomic skin
//      over the already-shipped `runtime.*` builder — NOT a new evaluator. JS arithmetic on
//      a ref (`200 * ref`, `ref > 0`) must fail.
//   2. No and/or/not opcode — boolean composition desugars to `select`.
//   3. `healthAfter` is the TRUE, UNFLOORED post-impact health (the pre-`.max(0.0)` value
//      the chokepoint holds). It MAY be negative. The stored component still floors at 0 for
//      the HUD; the impact fact carries the real overshoot so the AUTHOR decides what
//      health<=0 means. This is what makes both grunt-death and zombie-gib expressible.
//   4. TARGET (damaged) and SOURCE (damager) are distinct handles.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest,
  WritableStateRef,
} from "postretro";

declare module "postretro/proposed" {
  // ---- IR value refs. Operands accept number|NumberRef on both sides. Mirrors the
  //      shipped runtime.* op-set rather than hand-picking a slice.
  const numBrand: unique symbol;
  const boolBrand: unique symbol;

  export type NumberValue = number | NumberRef;
  export type BoolValue = boolean | BoolRef;

  export interface NumberRef {
    readonly [numBrand]: true;
    plus(n: NumberValue): NumberRef;                    // runtime.add
    minus(n: NumberValue): NumberRef;                   // runtime.sub
    times(n: NumberValue): NumberRef;                   // runtime.mul
    dividedBy(n: NumberValue): NumberRef;               // runtime.div (÷0 → 0; total evaluator)
    clamp(lo: NumberValue, hi: NumberValue): NumberRef; // runtime.clamp
    lerp(to: NumberValue, t: NumberValue): NumberRef;   // runtime.lerp
    lt(n: NumberValue): BoolRef;  le(n: NumberValue): BoolRef;
    gt(n: NumberValue): BoolRef;  ge(n: NumberValue): BoolRef;
    eq(n: NumberValue): BoolRef;  ne(n: NumberValue): BoolRef;
  }

  // Boolean composition is PURE SUGAR over `select` (the only conditional opcode):
  //   a.and(b) => select(a, b, false)   a.or(b) => select(a, true, b)
  //   a.not()  => select(a, false, true)   a.select(x, y) => branchless numeric pick
  export interface BoolRef {
    readonly [boolBrand]: true;
    and(other: BoolValue): BoolRef;
    or(other: BoolValue): BoolRef;
    not(): BoolRef;
    select(whenTrue: NumberValue, whenFalse: NumberValue): NumberRef;
  }

  // ---- Effects. The SDK derives each one's arm by identity. Presentation effects resolve
  //      to BAKED curves / local commands (and MAY take string args — they are not IR);
  //      consequential ones carry IR. The two-arm split IS the IR/bake boundary.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;
  export type GatedEffect = { when?: BoolRef; do: readonly Effect[] };
  export type EffectOrGroup = Effect | GatedEffect;   // an onImpact body may mix bare effects and gated groups

  // ---- Handles on the impact event. Accessors return IR REFS; methods return effect
  //      descriptors. TARGET (damaged) vs SOURCE (damager) are distinct and non-confusable.
  export interface TargetHandle {
    readonly healthBefore: NumberRef;
    // TRUE, UNFLOORED post-impact health — MAY be negative. `.le(0)` = depleted; a large
    // negative = a big overshoot (gib). Never assume it is >= 0.
    readonly healthAfter: NumberRef;
    readonly level: NumberRef;                          // per-entity stat → a leaf
    despawn(opts?: { afterMs?: number }): Effect;       // consequential; you remove the entity — the engine does NOT auto-remove at 0 HP
    playAnim(clip: string): Effect;                     // presentation; modder-owned (string arg OK — not IR)
    // WALL-NEW capability the zombie surfaces: an absolute entity-health write, optionally
    // deferred by a timer (stand back up after N ms). Needs an engine deferred-write path.
    setHealth(amount: NumberValue, opts?: { afterMs?: number }): Effect;
  }
  export interface SourceHandle {
    grant(resource: string, amount: NumberValue): Effect; // consequential
  }

  // ---- Store slot handle: additive write.
  export interface NumberSlot {
    add(delta: NumberValue): Effect;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  // ---- The ONE net-new engine event: impact, at the damage chokepoint.
  export type ImpactEvent = Readonly<{
    target: TargetHandle;
    source: SourceHandle;
    amount: NumberRef;   // requested damage (pre-floor)
  }>;

  const behaviorBrand: unique symbol;
  // Opaque, branded so a data-losing `{ kind: "impact" }` literal can't be forged into the
  // manifest — a behavior can only come from `onImpact`, carrying its authored effects/IR.
  export interface EventBehavior {
    readonly kind: "impact";
    readonly tag?: string;
    readonly [behaviorBrand]: true;
  }

  // ---- Entity query: a LIVE STANDING selector — spawn-aware, distinct from the shipped
  //      world.query snapshot (postretro.d.ts:188). `onImpact` runs ONCE at load to emit a
  //      behavior descriptor; it is not a live callback. Re-query a narrower set and define
  //      its onImpact LATER to OVERRIDE (later wins for entities both queries match).
  export interface EntitySet {
    onImpact(build: (impact: ImpactEvent) => readonly EffectOrGroup[]): EventBehavior;
  }
  export interface Entities {
    query(filter: { tag?: string; zone?: string }): EntitySet;
  }
  export const entities: Entities;

  // ---- Manifest: setupLevel returns the real LevelManifest; the impact behaviors are its
  //      `events` child, in PRECEDENCE ORDER (later overrides earlier per entity). TODO(spec):
  //      `events` almost certainly LOWERS INTO reactions + a chokepoint-registered predicate
  //      (the impact event is apply-site, NOT a tick-polled store crossing). Placeholder alias.
  export type LevelManifestWithEvents = LevelManifest & {
    events?: readonly EventBehavior[];
  };
}
