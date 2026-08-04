// Data-script vocabulary: pure descriptor builders for `ModManifest` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

import type { ReadonlyStateRef, WritableStateRef } from "./ui/widgets";
import type { RuntimeValue } from "postretro";

/** Dispatch values published by a state-crossing fire. */
export type CrossingParams = Readonly<{
  rising: import("postretro").RuntimeRead;
}>;

/** Dispatch values published while a Number store slot accumulates. */
export type TickParams = Readonly<{
  dt: import("postretro").RuntimeRead;
}>;

declare const activatorsTargetBrand: unique symbol;
declare const triggerTargetBrand: unique symbol;

/** Opaque target for the pawns that caused the current trigger edge. */
export type ActivatorsTarget = Readonly<{ readonly [activatorsTargetBrand]: true }>;
/** Opaque target for the trigger volume that fired the current edge. */
export type TriggerTarget = Readonly<{ readonly [triggerTargetBrand]: true }>;

/** Dispatch values published by an enter/exit trigger event. */
export type TriggerEventParams = Readonly<{
  activators: ActivatorsTarget;
  trigger: TriggerTarget;
  occupancy: import("postretro").RuntimeRead;
}>;

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/** Invokes a named Rust primitive. A non-empty `tag` targets matching entities; tag-targeted primitives include emitter/fog/mover commands, `applyDamage`, `grantHealth`, `grantAmmo`, `setAnimationState`, `updateEnemyState`, `armTrigger`, and `disarmTrigger`. In a trigger-event reaction, `applyDamage`, `grantHealth`, and `grantAmmo` may instead carry `target: "@activators"`. True system reactions carry neither `tag` nor `target` and enqueue typed engine commands such as `playSound`, `rumble`, `flashScreen`, and the UI-stack reactions. `args` carries the primitive's typed payload. */
export type PrimitiveReactionDescriptor = {
  primitive: string;
  tag?: string;
  target?: "@activators";
  args?: Record<string, unknown>;
  onComplete?: string;
};

/**
 * One step in a `sequence` reaction body. Sequence steps target a single `EntityId`;
 * tag-targeted primitives belong on the `Primitive` reaction path, not on `sequence`.
 */
export type SetLightAnimationStep = {
  id: import("postretro").EntityId;
  primitive: "setLightAnimation";
  args: import("postretro").LightAnimation;
};

/** Re-exported fog sequence step shapes — generated from the Rust primitive
 * registry. The SDK exposes them through this module so authors do not have to
 * import directly from `"postretro"` for the common "build a sequence step
 * array" path. */
export type SetFogDensityStep = import("postretro").SetFogDensityStep;
export type SetFogScatterStep = import("postretro").SetFogScatterStep;
export type SetFogEdgeSoftnessStep = import("postretro").SetFogEdgeSoftnessStep;
export type SetFogFalloffStep = import("postretro").SetFogFalloffStep;
export type SetFogParamsStep = import("postretro").SetFogParamsStep;
export type SetFogAnimationStep = import("postretro").SetFogAnimationStep;
export type MoverStartStep = import("postretro").MoverStartStep;
export type MoverStopStep = import("postretro").MoverStopStep;
export type MoverReverseStep = import("postretro").MoverReverseStep;
export type MoverGoToPathNodeStep = import("postretro").MoverGoToPathNodeStep;
export type MoverSetSpinRateStep = import("postretro").MoverSetSpinRateStep;
export type MoverSetBlockPolicyStep = import("postretro").MoverSetBlockPolicyStep;
export type ArmTriggerStep = import("postretro").ArmTriggerStep;
export type DisarmTriggerStep = import("postretro").DisarmTriggerStep;

/** Union of supported sequence step shapes. Mirrors the generated
 * `SequenceStep` in `postretro.d.ts`; new sequenced primitives extend
 * both ends of the union together. */
export type SequenceStep =
  | SetLightAnimationStep
  | SetFogDensityStep
  | SetFogScatterStep
  | SetFogEdgeSoftnessStep
  | SetFogFalloffStep
  | SetFogParamsStep
  | SetFogAnimationStep
  | MoverStartStep
  | MoverStopStep
  | MoverReverseStep
  | MoverGoToPathNodeStep
  | MoverSetSpinRateStep
  | MoverSetBlockPolicyStep
  | ArmTriggerStep
  | DisarmTriggerStep;

/** Ordered per-entity primitive invocations. Steps run in array order at dispatch time. */
export type SequenceReactionDescriptor = {
  sequence: SequenceStep[];
};

/** `name` is merged into the descriptor at the top level so the Rust deserializer reads event name and body from one flat object. */
export type NamedReactionDescriptor = { name: string; levels?: string[] } & (
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor
  | SequenceReactionDescriptor
);

/**
 * Deserialized once at level load; the data-script VM is dropped immediately after.
 *
 * Entity-type registrations are not part of `LevelManifest`. Export them from
 * the mod manifest's `entities` field instead — entity types are mod-level,
 * not level-level.
 */
export type LevelManifest = {
  reactions: NamedReactionDescriptor[];
  /** Impact-policy declarations. Registration occurs only through this manifest child. */
  events?: readonly ImpactEvent[];
  /** State-crossing watchers (HUD dynamics). See `onStateCrossing`. */
  crossings?: import("./ui/reactions").CrossingDescriptor[];
  triggerEvents?: TriggerEventDescriptor[];
  triggerPools?: TriggerPoolDescriptor[];
  /** Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same
   * shape as `ModManifest.uiTrees` but level-scoped (cleared on unload).
   * Malformed entries are logged and skipped. */
  uiTrees?: import("postretro").ModUiTree[];
};

/** One slot inside a `defineStore` schema. Every slot needs `default`. `type: "number"` accepts a finite numeric default plus optional inclusive `range: [min, max]`; `"boolean"` and `"string"` require matching defaults; `"enum"` requires non-empty `values` and a default in that list; `"array"` is a finite-number array. `persist` saves on clean exit; `readonly` blocks script writes. */
export type StoreSlotSchema = (
  | { type: "number"; readonly?: boolean; accumulate?: never }
  | { type: "number"; readonly?: false; accumulate: (t: TickParams) => import("postretro").RuntimeValue }
  | { type: "boolean" | "string" | "enum" | "array"; readonly?: boolean; accumulate?: never }
) & Record<string, unknown>;

export type StoreDeclaration = {
  namespace: string;
  schema: Record<string, StoreSlotSchema>;
};

export type StateRef<T = unknown> = ReadonlyStateRef<T> | WritableStateRef<T>;

export type StoreStateRefForSlot<Slot, T> =
  Slot extends { readonly: true } ? ReadonlyStateRef<T> : WritableStateRef<T>;

export type StateValueForSlot<Slot> =
  Slot extends { type: "number" } ? StoreStateRefForSlot<Slot, number> :
  Slot extends { type: "boolean" } ? StoreStateRefForSlot<Slot, boolean> :
  Slot extends { type: "array" } ? StoreStateRefForSlot<Slot, ReadonlyArray<number>> :
  StoreStateRefForSlot<Slot, string>;

export type StoreDefinition<S extends Record<string, StoreSlotSchema>> = {
  readonly declaration: StoreDeclaration;
  readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };
};

type ReactionBody =
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor
  | SequenceReactionDescriptor;

declare const reactionScopeBrand: unique symbol;

/** A named reaction whose phantom dispatch scope is enforced only by TypeScript. */
export type Reaction<S = {}> = NamedReactionDescriptor & {
  readonly [reactionScopeBrand]?: (scope: S) => void;
};

// Impact policies use the shipped, closed runtime IR without widening its
// evaluator vocabulary. Refs keep the raw node private so only descriptor
// builders can lower them into manifest data.
declare const numBrand: unique symbol;
declare const boolBrand: unique symbol;
declare const sourceBrand: unique symbol;
declare const impactEventBrand: unique symbol;
declare const effectBrand: unique symbol;

export type NumberValue = number | NumberRef;
export type BoolValue = boolean | BoolRef;

export interface NumberRef {
  readonly [numBrand]: true;
  plus(n: NumberValue): NumberRef;
  minus(n: NumberValue): NumberRef;
  times(n: NumberValue): NumberRef;
  dividedBy(n: NumberValue): NumberRef;
  clamp(lo: NumberValue, hi: NumberValue): NumberRef;
  lerp(to: NumberValue, t: NumberValue): NumberRef;
  lt(n: NumberValue): BoolRef;
  le(n: NumberValue): BoolRef;
  gt(n: NumberValue): BoolRef;
  ge(n: NumberValue): BoolRef;
  eq(n: NumberValue): BoolRef;
  ne(n: NumberValue): BoolRef;
}

export interface BoolRef {
  readonly [boolBrand]: true;
  and(other: BoolValue): BoolRef;
  or(other: BoolValue): BoolRef;
  not(): BoolRef;
  select(whenTrue: NumberValue, whenFalse: NumberValue): NumberRef;
}

type ImpactEffectWire =
  | { primitive: "despawn"; target: "@impact.target"; args: { afterMs?: number } }
  | { primitive: "playAnim"; target: "@impact.target"; args: { clip: string } }
  | { primitive: "setHealth"; target: "@impact.target"; args: { value: RuntimeValue; afterMs?: number } }
  | { primitive: "setState"; target: "@impact.target"; args: { name: string; value: RuntimeValue } }
  | { primitive: "grantHealth"; target: "@impact.source"; args: { amount: RuntimeValue } }
  | { primitive: "grantAmmo"; target: "@impact.source"; args: { type: string; amount: RuntimeValue } }
  | { primitive: "slot.add"; args: { slot: string; delta: RuntimeValue } };

/** Opaque closed impact effect. Construct through TargetHandle, SourceHandle, or slot(...).add(). */
export interface Effect {
  readonly [effectBrand]: true;
}
export type GatedEffect = { when?: BoolRef; do: readonly Effect[] };
export type EffectOrGroup = Effect | GatedEffect;
export type ImpactEventFilter = { tag?: string; levels?: readonly string[] };
export type ImpactEventOverrideFilter = { tag: string; levels?: readonly string[] };

export interface TargetHandle {
  readonly healthBefore: NumberRef;
  readonly healthAfter: NumberRef;
  readonly maxHealth: NumberRef;
  despawn(opts?: { afterMs?: number }): Effect;
  playAnim(clip: string): Effect;
  /** Clamp to the health range. Only a positive stored result recovers and re-arms; zero stays down. Literals must be finite, and non-finite IR arithmetic resolves to zero. */
  setHealth(value: NumberValue, opts?: { afterMs?: number }): Effect;
  state(name: string): NumberRef;
  setState(name: string, value: NumberValue): Effect;
}

export interface SourceHandle {
  readonly [sourceBrand]: true;
  /**
   * Add health to the impact damager. A fire with no damager skips this effect;
   * app-drain impacts run no policy in v1. Amount expressions read impact-target
   * facts and state only: v1 has no source-scoped fact vocabulary.
   */
  grantHealth(amount: NumberValue): Effect;
  /**
   * Add an ammo-pool balance to the impact damager. A fire with no damager
   * skips this effect; app-drain impacts run no policy in v1. Amount expressions
   * remain impact-target scoped; v1 has no source facts.
   */
  grantAmmo(type: string, amount: NumberValue): Effect;
}

export interface NumberSlot {
  add(delta: NumberValue): Effect;
}

export type Impact = Readonly<{
  target: TargetHandle;
  source: SourceHandle;
  amount: NumberRef;
}>;

export interface ImpactEvent {
  readonly kind: "impact";
  readonly isOverride: boolean;
  readonly levels?: readonly string[];
  readonly [impactEventBrand]: true;
  override(
    filter: ImpactEventOverrideFilter,
    build: (impact: Impact) => readonly EffectOrGroup[],
  ): ImpactEvent;
}

const numberNodes = new WeakMap<object, RuntimeValue>();
const boolNodes = new WeakMap<object, RuntimeValue>();

function constant(value: number | boolean): RuntimeValue {
  return { op: "const", value };
}

function numberNode(value: NumberValue): RuntimeValue {
  return typeof value === "number" ? constant(value) : numberNodes.get(value)!;
}

function boolNode(value: BoolValue): RuntimeValue {
  return typeof value === "boolean" ? constant(value) : boolNodes.get(value)!;
}

function numberRef(node: RuntimeValue): NumberRef {
  const ref: NumberRef = {
    plus: (n) => numberRef({ op: "add", a: node, b: numberNode(n) }),
    minus: (n) => numberRef({ op: "sub", a: node, b: numberNode(n) }),
    times: (n) => numberRef({ op: "mul", a: node, b: numberNode(n) }),
    dividedBy: (n) => numberRef({ op: "div", a: node, b: numberNode(n) }),
    clamp: (lo, hi) => numberRef({ op: "clamp", x: node, lo: numberNode(lo), hi: numberNode(hi) }),
    lerp: (to, t) => numberRef({ op: "lerp", a: node, b: numberNode(to), t: numberNode(t) }),
    lt: (n) => boolRef({ op: "lt", a: node, b: numberNode(n) }),
    le: (n) => boolRef({ op: "le", a: node, b: numberNode(n) }),
    gt: (n) => boolRef({ op: "gt", a: node, b: numberNode(n) }),
    ge: (n) => boolRef({ op: "ge", a: node, b: numberNode(n) }),
    eq: (n) => boolRef({ op: "eq", a: node, b: numberNode(n) }),
    ne: (n) => boolRef({ op: "ne", a: node, b: numberNode(n) }),
  } as NumberRef;
  numberNodes.set(ref, node);
  return Object.freeze(ref);
}

function boolRef(node: RuntimeValue): BoolRef {
  const ref: BoolRef = {
    and: (other) => boolRef({ op: "select", cond: node, a: boolNode(other), b: constant(false) }),
    or: (other) => boolRef({ op: "select", cond: node, a: constant(true), b: boolNode(other) }),
    not: () => boolRef({ op: "select", cond: node, a: constant(false), b: constant(true) }),
    select: (whenTrue, whenFalse) => numberRef({
      op: "select",
      cond: node,
      a: numberNode(whenTrue),
      b: numberNode(whenFalse),
    }),
  } as BoolRef;
  boolNodes.set(ref, node);
  return Object.freeze(ref);
}

function impactEffect(
  primitive: string,
  args?: Record<string, unknown>,
): Effect {
  return { primitive, target: "@impact.target", args } as ImpactEffectWire as unknown as Effect;
}

function sourceImpactEffect(
  primitive: string,
  args?: Record<string, unknown>,
): Effect {
  return { primitive, target: "@impact.source", args } as ImpactEffectWire as unknown as Effect;
}

const IMPACT_TARGET: TargetHandle = Object.freeze({
  healthBefore: numberRef({ op: "input", name: "@impact.healthBefore" }),
  healthAfter: numberRef({ op: "input", name: "@impact.healthAfter" }),
  maxHealth: numberRef({ op: "input", name: "@impact.maxHealth" }),
  despawn(opts) {
    return impactEffect("despawn", opts?.afterMs === undefined ? {} : { afterMs: opts.afterMs });
  },
  playAnim(clip) {
    return impactEffect("playAnim", { clip });
  },
  setHealth(value, opts) {
    const args: Record<string, unknown> = { value: numberNode(value) };
    if (opts?.afterMs !== undefined) args.afterMs = opts.afterMs;
    return impactEffect("setHealth", args);
  },
  state(name) {
    return numberRef({ op: "input", name: `@state.${name}` });
  },
  setState(name, value) {
    return impactEffect("setState", { name, value: numberNode(value) });
  },
});

const IMPACT_SOURCE: SourceHandle = Object.freeze({
  grantHealth: (amount) => sourceImpactEffect("grantHealth", { amount: numberNode(amount) }),
  grantAmmo: (type, amount) => sourceImpactEffect("grantAmmo", { type, amount: numberNode(amount) }),
}) as SourceHandle;

const IMPACT: Impact = Object.freeze({
  target: IMPACT_TARGET,
  source: IMPACT_SOURCE,
  amount: numberRef({ op: "input", name: "@impact.amount" }),
});

/** Build the closed additive store-write effect. */
export function slot(ref: WritableStateRef<number>): NumberSlot {
  return Object.freeze({
    add(delta: NumberValue): Effect {
      return {
        primitive: "slot.add",
        args: {
          slot: ref.slot,
          delta: numberNode(delta),
        },
      } as ImpactEffectWire as unknown as Effect;
    },
  });
}

function impactEvent(
  id: string,
  filter: { tag?: string },
  policy: readonly EffectOrGroup[],
  levels?: readonly string[],
  isOverride = false,
): ImpactEvent {
  const handle = {
    kind: "impact" as const,
    id,
    isOverride,
    filter,
    policy,
    ...(levels === undefined ? {} : { levels }),
    override(overrideFilter: ImpactEventOverrideFilter, build: (impact: Impact) => readonly EffectOrGroup[]) {
      if (typeof overrideFilter.tag !== "string") {
        throw new TypeError("impact-event override filter requires `tag`");
      }
      return impactEvent(
        id,
        Object.freeze({ tag: overrideFilter.tag }),
        lowerImpactPolicy(build(IMPACT)),
        overrideFilter.levels,
        true,
      );
    },
  } as ImpactEvent;
  return Object.freeze(handle);
}

function lowerImpactPolicy(policy: readonly EffectOrGroup[]): readonly EffectOrGroup[] {
  assertDenseImpactArray(policy, "impact policy");
  return policy.map((entry) => {
    if ("do" in entry) {
      const gated = entry as GatedEffect;
      assertDenseImpactArray(gated.do, "impact policy group `do`");
      const group: { when?: RuntimeValue; do: readonly Effect[] } = { do: gated.do };
      if (gated.when !== undefined) group.when = boolNode(gated.when);
      return group as EffectOrGroup;
    }
    return entry;
  });
}

function assertDenseImpactArray(values: readonly unknown[], context: string): void {
  for (let i = 0; i < values.length; i += 1) {
    if (!(i in values)) throw new TypeError(`${context} must be a dense array; holes are not allowed`);
  }
}

const IMPACT_EVENT_ID_DIAGNOSTIC = "impact-event `id` must be a namespaced ASCII string (for example \"salvage:crate-break\") using only [A-Za-z0-9_.-] within each colon-separated segment, at most 128 bytes";

function validateImpactEventId(id: string): void {
  const valid = id.length > 0
    && id.length <= 128
    && /^[A-Za-z0-9_.-]+(?::[A-Za-z0-9_.-]+)+$/.test(id);
  if (!valid) throw new TypeError(IMPACT_EVENT_ID_DIAGNOSTIC);
}

/** Define a pure impact-policy descriptor. Registration occurs only through a manifest's `events`. */
export function defineImpactEvent(
  id: string,
  filter: ImpactEventFilter,
  build: (impact: Impact) => readonly EffectOrGroup[],
): ImpactEvent {
  validateImpactEventId(id);
  const eventFilter = Object.freeze({ tag: filter.tag });
  const policy = lowerImpactPolicy(build(IMPACT));
  return impactEvent(id, eventFilter, policy, filter.levels, false);
}

type ReactionTracer<S> = (params: S) => ReactionBody;

// This is deliberately one plain merged object rather than a Proxy. Sibling
// dispatch specs add opaque, non-IR leaves (activators, trigger) alongside
// these input nodes.
const ACTIVATORS_TARGET = Object.freeze({}) as ActivatorsTarget;
const TRIGGER_TARGET = Object.freeze({}) as TriggerTarget;

const DISPATCH_PARAMS = Object.freeze({
  rising: Object.freeze({ op: "input", name: "@rising" } as const),
  dt: Object.freeze({ op: "input", name: "@dt" } as const),
  activators: ACTIVATORS_TARGET,
  trigger: TRIGGER_TARGET,
  occupancy: Object.freeze({ op: "input", name: "@occupancy" } as const),
});

export type TriggerEventDescriptor = {
  tag: string;
  event: "enter" | "exit";
  fire: string[];
  levels?: string[];
};

export type TriggerPoolDescriptor = {
  tag: string;
  arm?: number;
  armPercentage?: number;
  levels?: string[];
};

export type TriggerEventOptions = { levels?: string[] };

type TriggerEventReaction = Reaction<{}> | Reaction<TriggerEventParams> | string;

/** Build a trigger-event observer descriptor, lowering reaction handles to names. */
export function onTriggerEvent(
  filter: { tag: string },
  event: "enter" | "exit",
  fire: TriggerEventReaction[],
  options?: TriggerEventOptions,
): TriggerEventDescriptor {
  const descriptor: TriggerEventDescriptor = {
    tag: filter.tag,
    event,
    fire: fire.map((reaction) => typeof reaction === "string" ? reaction : reaction.name),
  };
  if (options?.levels !== undefined) descriptor.levels = options.levels;
  return descriptor;
}

/** Apply damage to the current trigger activators or every entity with a tag. */
export function damage(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "applyDamage", tag: target, args: { amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return { primitive: "applyDamage", target: wireTarget, args: { amount } } as PrimitiveReactionDescriptor;
}

/** Grant health to the current trigger activators or every entity with a tag. */
export function grantHealth(
  target: ActivatorsTarget | string,
  amount: number,
): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "grantHealth", tag: target, args: { amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return { primitive: "grantHealth", target: wireTarget, args: { amount } } as PrimitiveReactionDescriptor;
}

/** Grant an ammo-reserve pool to the current trigger activators or every entity with a tag. */
export function grantAmmo(
  target: ActivatorsTarget | string,
  type: string,
  amount: number,
): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "grantAmmo", tag: target, args: { type, amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return {
    primitive: "grantAmmo",
    target: wireTarget,
    args: { type, amount },
  } as PrimitiveReactionDescriptor;
}

/** Arm the trigger volume that fired the current event. */
export function armTrigger(target: TriggerTarget): SequenceStep[] {
  const wireTarget = target === TRIGGER_TARGET ? "@trigger" : "@invalid";
  return [{ id: wireTarget, primitive: "armTrigger", args: {} } as SequenceStep];
}

/** Disarm the trigger volume that fired the current event. */
export function disarmTrigger(target: TriggerTarget): SequenceStep[] {
  const wireTarget = target === TRIGGER_TARGET ? "@trigger" : "@invalid";
  return [{ id: wireTarget, primitive: "disarmTrigger", args: {} } as SequenceStep];
}

/**
 * Deterministic, run-stable id derived from a reaction body. Content-derived
 * (a stable string serialization of the body hashed with FNV-1a) so re-running
 * registration yields the same id — crossings and the `onPress` wire form
 * reference it, so it must not vary across runs.
 *
 * NOTE: the auto-id is run-stable within a runtime but NOT identical across
 * TS and Luau — each uses a different stable-stringify implementation. Do not
 * assume cross-runtime id parity; use an explicit `name` when the id must
 * match across both runtimes.
 */
function autoReactionId(descriptor: ReactionBody): string {
  const serialized = stableStringify(descriptor);
  // FNV-1a (32-bit). Deterministic and dependency-free; collision risk is
  // acceptable for author-named reaction ids and an explicit `name` overrides it.
  let hash = 0x811c9dc5;
  for (let i = 0; i < serialized.length; i++) {
    hash ^= serialized.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return `reaction_${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

/** Order-stable JSON serialization: object keys are emitted sorted so two
 * structurally identical bodies always serialize identically. */
function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  const keys = Object.keys(value as Record<string, unknown>).sort();
  const entries = keys.map(
    (k) => `${JSON.stringify(k)}:${stableStringify((value as Record<string, unknown>)[k])}`,
  );
  return `{${entries.join(",")}}`;
}

/**
 * Build a named reaction descriptor. Pure: returns a plain object and performs
 * no FFI. `descriptor` accepts exactly one body shape: `progress`, `primitive`,
 * or `sequence`. `name` is optional; when omitted a deterministic, run-stable id
 * is derived from the body. Use explicit names when TS and Luau scripts must
 * agree. The returned handle can be passed to `Button.onPress` or crossing
 * `fire` entries.
 *
 * @param name Stable event/reaction name consumed by dispatch. Optional.
 * @param descriptor Reaction body data consumed later by Rust.
 */
export function defineReaction(body: ReactionBody): Reaction<{}>;
export function defineReaction(tracer: ReactionTracer<CrossingParams>): Reaction<CrossingParams>;
export function defineReaction(tracer: ReactionTracer<TriggerEventParams>): Reaction<TriggerEventParams>;
export function defineReaction(
  name: string,
  descriptor: ReactionBody,
): Reaction<{}>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<CrossingParams>,
): Reaction<CrossingParams>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<TriggerEventParams>,
): Reaction<TriggerEventParams>;
export function defineReaction(
  nameOrBody: string | ReactionBody | ReactionTracer<CrossingParams | TriggerEventParams>,
  descriptor?: ReactionBody | ReactionTracer<CrossingParams | TriggerEventParams>,
): Reaction<{}> | Reaction<CrossingParams> | Reaction<TriggerEventParams> {
  const authored = typeof nameOrBody === "string" ? descriptor : nameOrBody;
  const tracedBody = typeof authored === "function"
    ? authored(DISPATCH_PARAMS)
    : authored as ReactionBody;
  const [name, body] =
    typeof nameOrBody === "string"
      ? [nameOrBody, tracedBody]
      : [autoReactionId(tracedBody), tracedBody];
  return { name, ...body } as Reaction<{}> | Reaction<CrossingParams> | Reaction<TriggerEventParams>;
}

/** Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are matched against `ModMapEntry.tags`; omit scoping for every level. */
export function scopeReactions<S>(
  tags: string[],
  list: Reaction<S>[],
): Reaction<S>[] {
  return list.map((reaction) => ({ ...reaction, levels: tags }));
}

/** Identity builder for entity type descriptors returned from `ModManifest.entities`. `descriptor` is the full archetype object: optional `canonicalName`, optional `components.inventory.loadout`, and optional component presets. Pure: no engine side effects. */
/**
 * Lowers authored weapon descriptor references to the canonical names carried
 * across the manifest boundary. This compares descriptor values only: module
 * identity is not stable across the separate script VMs.
 */
function lowerLoadoutReferences(descriptor: import("postretro").EntityTypeDescriptor): void {
  const loadout = descriptor.components?.inventory?.loadout;
  if (loadout === undefined) return;
  const loweredLoadout = loadout as unknown as string[];

  for (let index = 0; index < loadout.length; index += 1) {
    const entry = loadout[index];
    const entryName = `components.inventory.loadout[${index}]`;
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) {
      throw new Error(`${entryName} must reference an entity descriptor`);
    }
    const weapon = entry.components?.weapon;
    if (weapon === null || typeof weapon !== "object" || Array.isArray(weapon)) {
      throw new Error(`${entryName} must reference a descriptor with a weapon block`);
    }
    if (typeof entry.canonicalName !== "string" || entry.canonicalName.length === 0) {
      throw new Error(`${entryName} must reference a descriptor with a canonical name`);
    }
    loweredLoadout[index] = entry.canonicalName;
  }
}

export function defineEntity<T extends import("postretro").EntityTypeDescriptor>(
  descriptor: T,
): T {
  lowerLoadoutReferences(descriptor);
  return descriptor;
}

/**
 * Identity builder for the mod manifest consumed from the default export.
 * `config.name`, `config.id`, and `config.version` are required. The id gates
 * multiplayer admission; the version is display-only and never compared. The
 * first committed id and version remain active across staged reloads. Optional
 * arrays include `entities`, `maps`, `uiTrees`, `reactions`, `events`,
 * `crossings`, `triggerEvents`, `triggerPools`, and `stores`. Pure: no engine
 * side effects until the manifest is returned and validated.
 */
export function defineMod(
  config: import("postretro").ModManifest,
): import("postretro").ModManifest {
  return config;
}

/** Identity builder for a mod map catalog. `entries` are `ModMapEntry` objects with required `id`, `path`, and `name`; optional `tags` default to empty and drive filtering plus `levels` selectors. Pure: no engine side effects. */
export function defineMapCatalog(
  entries: import("postretro").ModMapEntry[],
): import("postretro").ModMapEntry[] {
  return entries;
}

/** Identity builder for a trigger-pool declaration returned from a level or mod manifest. Engine parsing owns arming validation. */
export function defineTriggerPool(pool: TriggerPoolDescriptor): TriggerPoolDescriptor {
  return pool;
}

const MAGIC_SCHEMA_KEYS = new Set(["__proto__", "constructor", "prototype"]);

function schemaPath(parent: string, key: string): string {
  return /^[A-Za-z_$][\w$]*$/.test(key) ? `${parent}.${key}` : `${parent}[${JSON.stringify(key)}]`;
}

function rejectMagicSchemaKey(key: string, path: string): void {
  if (MAGIC_SCHEMA_KEYS.has(key)) {
    throw new Error(`defineStore schema key ${schemaPath(path, key)} is reserved`);
  }
}

function cloneAndFreeze<T>(
  value: T,
  path = "schema",
  seen = new WeakMap<object, unknown>(),
  visiting = new WeakSet<object>(),
): T {
  if (value === null || typeof value !== "object") {
    return value;
  }
  if (!Array.isArray(value)) {
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) {
      throw new Error(`defineStore schema object at ${path} must be a plain object`);
    }
  }
  if (visiting.has(value as object)) {
    throw new Error(`defineStore schema contains a cycle at ${path}`);
  }
  const existing = seen.get(value as object);
  if (existing !== undefined) {
    return existing as T;
  }
  visiting.add(value as object);
  if (Array.isArray(value)) {
    const clone: unknown[] = [];
    seen.set(value, clone);
    for (const item of value) {
      clone.push(cloneAndFreeze(item, `${path}[]`, seen, visiting));
    }
    visiting.delete(value);
    return Object.freeze(clone) as T;
  }
  const clone: Record<string, unknown> = Object.create(null);
  seen.set(value as object, clone);
  for (const key of Object.keys(value as Record<string, unknown>)) {
    rejectMagicSchemaKey(key, path);
    clone[key] = cloneAndFreeze((value as Record<string, unknown>)[key], schemaPath(path, key), seen, visiting);
  }
  visiting.delete(value as object);
  return Object.freeze(clone) as T;
}

/** Pure state-store builder. `namespace` prefixes every returned state ref as `namespace.slotName`; `schema` declares the slot names and validation rules. The engine consumes `declaration` only when it is returned from `ModManifest.stores`; unreturned declarations are discarded with the setup VM. */
export function defineStore<const S extends Record<string, StoreSlotSchema>>(
  namespace: string,
  schema: S,
): StoreDefinition<S> {
  const tracedSchema: Record<string, StoreSlotSchema> = Object.create(null);
  for (const [slot, input] of Object.entries(schema)) {
    if (input !== null && typeof input === "object" && typeof input.accumulate === "function") {
      tracedSchema[slot] = { ...input, accumulate: input.accumulate(DISPATCH_PARAMS) } as StoreSlotSchema;
    } else {
      tracedSchema[slot] = input;
    }
  }
  const frozenSchema = cloneAndFreeze(tracedSchema) as S;
  const state: Record<string, StateRef> = Object.create(null);
  for (const slot of Object.keys(frozenSchema)) {
    state[slot] = Object.freeze({ slot: `${namespace}.${slot}` }) as StateRef;
  }
  return Object.freeze({
    declaration: Object.freeze({ namespace, schema: frozenSchema }),
    state: Object.freeze(state) as { readonly [K in keyof S]: StateValueForSlot<S[K]> },
  });
}
