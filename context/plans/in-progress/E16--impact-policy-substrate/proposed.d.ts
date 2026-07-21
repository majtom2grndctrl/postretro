// PROPOSED SDK SURFACE — not shipped. Every export is a WALL the spec would build.
// Module "postretro/proposed" so each import in the spike is visibly a gap.
//
// THE MODEL (grounded): the engine owns IMPACT — the damage chokepoint, the single
// HP-decrement site (health.rs:319-340). DEATH is NOT an engine concept; the modder writes
// what counts as death (and the whole life→death lifecycle) as a POLICY over impact facts.
//
// BLESSED-HANDLE DISCIPLINE (mirrors defineStore, grounded in postretro.d.ts:1087-1097):
// every `define*` is a PURE builder — calling it performs no FFI and changes no engine state.
// `defineImpactEvent(...)` returns a blessed `ImpactEvent` HANDLE. Registration happens ONLY
// by returning the handle through a manifest's `events`. To modify an event in a DIFFERENT
// application scope (a map refining a mod-wide behavior), you don't mutate the handle — you
// call `handle.override(...)`, which RETURNS a linked override declaration you return from
// that scope's manifest. The engine merges base + overrides by identity at load; later wins.
// The handle is the cross-scope reference currency; the manifest return is the ONLY
// registration channel. This is the same shape as a mod-scope store ref used in a map-scope
// reaction — nothing self-registers, nothing mutates.
//
// GROUNDED IR CONSTRAINTS the types encode:
//   1. The IR is the shipped closed enum IrNode (15 ops). These refs are an ergonomic skin
//      over the shipped `runtime.*` builder — NOT a new evaluator. `200 * ref` / `ref > 0` fail.
//   2. No and/or/not opcode — boolean composition desugars to `select`.
//   3. `healthAfter` is the TRUE, UNFLOORED post-impact health (pre-`.max(0.0)`); MAY be
//      negative, so a gib-level overshoot is expressible.
//   4. TARGET (damaged) and SOURCE (damager) are distinct handles.

import type {
  Reaction,
  PrimitiveReactionDescriptor,
  LevelManifest as BaseLevelManifest,
  ModManifest as BaseModManifest,
  WritableStateRef,
} from "postretro";

declare module "postretro/proposed" {
  // ---- IR value refs. Operands accept number|NumberRef on both sides. Mirrors runtime.*.
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

  // ---- Effects. The SDK derives each one's arm by identity. Presentation effects resolve to
  //      BAKED curves / local commands (string args OK — not IR); consequential ones carry IR.
  export type Effect = PrimitiveReactionDescriptor | Reaction<{}>;
  export type GatedEffect = { when?: BoolRef; do: readonly Effect[] };
  export type EffectOrGroup = Effect | GatedEffect;   // a policy body may mix bare effects and gated groups

  // ---- Handles on ONE impact occurrence. Accessors return IR REFS; methods return effects.
  //      TARGET (damaged) vs SOURCE (damager) are distinct and non-confusable.
  export interface TargetHandle {
    readonly healthBefore: NumberRef;
    // TRUE, UNFLOORED post-impact health — MAY be negative. `.le(0)` = depleted; a large
    // negative = a big overshoot (gib). Never assume it is >= 0.
    readonly healthAfter: NumberRef;
    readonly maxHealth: NumberRef;                      // from the health descriptor's `max` — for % thresholds
    despawn(opts?: { afterMs?: number }): Effect;       // consequential; you remove the entity — the engine does NOT auto-remove at 0 HP
    // presentation; modder-owned. `clip` names a DECLARED animation state on the entity's mesh
    // (string arg OK — not IR), switched via the id-targeted `switch_animation_state` seam.
    playAnim(clip: string): Effect;
    // WALL-NEW (zombie): absolute entity-health write, optionally deferred (stand back up).
    setHealth(value: NumberValue, opts?: { afterMs?: number }): Effect;
    // WALL-NEW (Doom lifecycle): per-INSTANCE modder-owned state, a number slot keyed by name.
    // The substrate for lifecycle state machines (alive/staggered/downed). Read → IR ref;
    // write → effect. Host-authoritative; per-entity replication is deferred (see roadmap).
    state(name: string): NumberRef;
    setState(name: string, value: NumberValue): Effect;
  }
  const sourceBrand: unique symbol;
  // Opaque token for the damager. Published in the impact scope, but effects that credit the
  // SOURCE (grant xp / ammo / health) are DEFERRED — per-player resources need a replication
  // story first (see roadmap). No v1 methods; the token exists so policies can name the source
  // and the seam is charted.
  export interface SourceHandle {
    readonly [sourceBrand]: true;
  }

  // ---- Store slot handle: additive write.
  export interface NumberSlot {
    add(delta: NumberValue): Effect;
  }
  export function slot(ref: WritableStateRef<number>): NumberSlot;

  // ---- ONE impact OCCURRENCE (the facts passed to a policy builder), distinct from the
  //      defined event that observes it.
  export type Impact = Readonly<{
    target: TargetHandle;
    source: SourceHandle;
    amount: NumberRef;   // damage applied at the chokepoint, pre-floor. Weapon fire is
                         // zone-multiplier-scaled at the fire site; enemy melee passes raw
                         // attack_damage (unscaled). Do not assume it is zone-scaled.
  }>;

  const impactEventBrand: unique symbol;
  // ---- The BLESSED HANDLE for a defined impact event. Pure returnable data (goes into a
  //      manifest's `events`), branded so a bare `{ kind: "impact" }` can't be forged. Thread
  //      it across scopes; `override(...)` returns a LINKED override handle to return from the
  //      overriding scope's manifest. Precedence is LOAD ORDER: the LAST-REGISTERED override wins
  //      (registration/load order, not runtime execution). The filter narrows the base set by an
  //      ADDITIONAL tag (an entity may carry several tags), so an override targets a subset the
  //      base also matches.
  export interface ImpactEvent {
    readonly kind: "impact";
    readonly [impactEventBrand]: true;
    override(
      filter: { tag?: string },
      build: (impact: Impact) => readonly EffectOrGroup[],
    ): ImpactEvent;
  }

  // ---- Define an impact event. Top-level, matching the shipped `define*` family. The filter
  //      is a STANDING selector (spawn-aware — it observes any matching entity's impacts, not a
  //      snapshot). Runs ONCE at load to emit the handle; the builder is not a live callback.
  export function defineImpactEvent(
    filter: { tag?: string },
    build: (impact: Impact) => readonly EffectOrGroup[],
  ): ImpactEvent;

  // ---- Manifests: the proposed `events` child folded onto the REAL Level/Mod manifest (aliased
  //      here as Base*). These shadow the shipped names on purpose — the spec adds `events` to the
  //      real types, at which point these collapse to plain re-exports. The spec's evaluator task
  //      (Task 5) owns the lowering: `events` becomes a chokepoint-registered predicate + effect dispatch
  //      (apply-site, NOT a tick-polled store crossing).
  export type LevelManifest = BaseLevelManifest & { events?: readonly ImpactEvent[] };
  export type ModManifest = BaseModManifest & { events?: readonly ImpactEvent[] };
}
