// Impact-policy builders: the `impact` scope a `defineImpactEvent` build
// receives, its closed effects, gated groups and store-write helpers.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §11

import type { ComputedRef, Ref } from "../ui/widgets";
import type { PresentationTemplate } from "../ui/presentation";
import type { RuntimeValue } from "postretro";
import { numberNode, boolNode, numberRef, boolRef } from "../util/expression_refs";
import type { NumberValue, NumberRef, BoolRef, RuntimeExpressionRefs } from "../util/expression_refs";
import type { OwnerAddressedRef, StateRef } from "../data_script";

declare const sourceBrand: unique symbol;
declare const impactEventBrand: unique symbol;
declare const effectBrand: unique symbol;

type ImpactEffectWire =
  | { primitive: "despawn"; target: "@impact.target"; args: { afterMs?: number } }
  | { primitive: "playAnim"; target: "@impact.target"; args: { clip: string } }
  | { primitive: "present"; target: "@impact.target"; args: { template: string; value: RuntimeValue } }
  | { primitive: "setHealth"; target: "@impact.target"; args: { value: RuntimeValue; afterMs?: number } }
  | { primitive: "setState"; target: "@impact.target"; args: { name: string; value: RuntimeValue } }
  | { primitive: "grantHealth"; target: "@impact.source"; args: { amount: RuntimeValue } }
  | { primitive: "grantAmmo"; target: "@impact.source"; args: { type: string; amount: RuntimeValue } }
  | { primitive: "grantLayer"; target: "@impact.source"; args: { layer: import("postretro").DefenseLayer; amount?: RuntimeValue } }
  | { primitive: "setLifeState"; target: "@impact.source"; args: { state: string } }
  | { primitive: "adjustSentiment"; target: "@impact.target" | "@impact.source"; args: { toward: "@impact.target" | "@impact.source"; delta: RuntimeValue } }
  | { primitive: "slot.set"; args: { slot: string; value: RuntimeValue } }
  | { primitive: "slot.set"; target: "@impact.source"; args: { slot: string; value: RuntimeValue } };

/** Opaque closed impact effect. Construct through TargetHandle, SourceHandle, `set`, or `update`. */
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
  /** Adjust this faction's sentiment toward another impact recipient. Negative values degrade the relationship; positive values strengthen its bond. */
  adjustSentimentToward(toward: TargetHandle | SourceHandle, delta: NumberValue): Effect;
  state(name: string): NumberRef;
  /** The impact target's own layer, read before this fire's effects: `current` and `capacity`. A target without that layer reads 0. */
  defense(layer: import("postretro").DefenseLayer): Readonly<{ current: NumberRef; capacity: NumberRef }>;
  setState(name: string, value: NumberValue): Effect;
}

export interface SourceHandle {
  readonly [sourceBrand]: true;
  /**
   * Add health to the impact damager. A fire with no damager skips this effect;
   * app-drain impacts run no policy in v1.
   */
  grantHealth(amount: NumberValue): Effect;
  /**
   * Add an ammo-pool balance to the impact damager. A fire with no damager
   * skips this effect; app-drain impacts run no policy in v1.
   */
  grantAmmo(type: string, amount: NumberValue): Effect;
  /**
   * Refill the damager's `layer`: omit `amount` to fill to capacity, or add
   * `amount`, clamped to capacity. The capacity is evaluated when the grant
   * applies. A damager without that layer warns and skips; a fire with no
   * damager skips this effect.
   */
  grantLayer(layer: import("postretro").DefenseLayer, amount?: NumberValue): Effect;
  /**
   * Whether the damager is a player pawn in life state `name` at this fire.
   * False for a damager that is not a player. An unknown name skips the policy.
   */
  inLifeState(name: string): BoolRef;
  /**
   * Put the damager player pawn into life state `name` in-tick, as `setLifeState`:
   * a change restarts its elapsed time, the current state is a no-op. A fire with
   * no damager skips it; a damager lacking the state warns and skips.
   */
  setLifeState(name: string): Effect;
  /** Adjust this faction's sentiment toward another impact recipient. Negative values degrade the relationship; positive values strengthen its bond. */
  adjustSentimentToward(toward: TargetHandle | SourceHandle, delta: NumberValue): Effect;
}

declare const assistSideBrand: unique symbol;
/** One side of an assist pair expression, evaluated on the host for each helper and target. */
export interface AssistSide {
  readonly [assistSideBrand]: true;
  /** Whether this side's pawn is in life state `name`. An unknown name rejects the mod at load. */
  inLifeState(name: string): BoolRef;
}
/** The two sides an `assistable` `radius` or `holdMs` expression reads: `byPlayer(assist.helper)`, `assist.target.inLifeState(name)`. */
export type Assist = Readonly<{ helper: AssistSide; target: AssistSide }>;

export type Impact = Readonly<{
  target: TargetHandle;
  source: SourceHandle;
  /** Points dealt: the total drained from layers and health after type multipliers and resistances. */
  amount: NumberRef;
  /** The incoming amount after hit zones, before type and layers. */
  raw: NumberRef;
  /** The layers' share of `amount`. */
  absorbed: NumberRef;
  /** This hit took some layer from above zero to zero. */
  broke: BoolRef;
  /** True when the hit dealt `damageType`, the mod's default type included. */
  type(damageType: import("postretro").DamageType): BoolRef;
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

/** Public lifting helpers keep the raw-node adapters private to the SDK. */
export const fromRuntime: RuntimeExpressionRefs = Object.freeze({
  number: (value) => numberRef(value),
  bool: (value) => boolRef(value),
});

/** Test the evaluated player in player events, or the local player in crossings. */
export function inLifeState(name: string): BoolRef {
  return boolRef({ op: "input", name: `player.inLifeState:${name}` });
}

export function read(ref: StateRef<number>): NumberRef;
export function read(ref: StateRef<boolean>): BoolRef;
export function read(ref: StateRef<number> | StateRef<boolean>): NumberRef | BoolRef {
  const owner = "owner" in ref ? ref.owner : undefined;
  const node: RuntimeValue = owner === undefined
    ? { op: "input", name: ref.slot }
    : { op: "input", name: ref.slot, owner };
  return ref.kind === "number" ? numberRef(node) : boolRef(node);
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

function sentimentImpactEffect(
  target: "@impact.target" | "@impact.source",
  toward: TargetHandle | SourceHandle,
  delta: NumberValue,
): Effect {
  const towardToken = toward === IMPACT_TARGET
    ? "@impact.target"
    : toward === IMPACT_SOURCE
      ? "@impact.source"
      : "@invalid";
  return {
    primitive: "adjustSentiment",
    target,
    args: { toward: towardToken, delta: numberNode(delta) },
  } as ImpactEffectWire as unknown as Effect;
}

/** The `@defense.` input a layer handle reads; a bad reference (a bare string,
 * a damage-type handle) names no layer, so its bind fails at install. */
function defenseInput(layer: unknown, capacity: boolean): string {
  const handle = layer as { kind?: unknown; name?: unknown } | null;
  const name =
    handle !== null && typeof handle === "object" && handle.kind === "defenseLayer" && typeof handle.name === "string"
      ? handle.name
      : "@invalid";
  return `@defense.${name}${capacity ? ".capacity" : ""}`;
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
  adjustSentimentToward(toward, delta) {
    return sentimentImpactEffect("@impact.target", toward, delta);
  },
  state(name) {
    return numberRef({ op: "input", name: `@state.${name}` });
  },
  defense(layer) {
    return Object.freeze({
      current: numberRef({ op: "input", name: defenseInput(layer, false) }),
      capacity: numberRef({ op: "input", name: defenseInput(layer, true) }),
    });
  },
  setState(name, value) {
    return impactEffect("setState", { name, value: numberNode(value) });
  },
});

const impactSource = {
  grantHealth: (amount: NumberValue) => sourceImpactEffect("grantHealth", { amount: numberNode(amount) }),
  grantAmmo: (type: string, amount: NumberValue) => sourceImpactEffect("grantAmmo", { type, amount: numberNode(amount) }),
  grantLayer: (layer: import("postretro").DefenseLayer, amount?: NumberValue) =>
    sourceImpactEffect("grantLayer", amount === undefined ? { layer } : { layer, amount: numberNode(amount) }),
  inLifeState: (name: string) =>
    boolRef({ op: "input", owner: "@impact.source", name: `player.inLifeState:${name}` }),
  setLifeState: (name: string) => sourceImpactEffect("setLifeState", { state: name }),
  adjustSentimentToward: (toward: TargetHandle | SourceHandle, delta: NumberValue) =>
    sentimentImpactEffect("@impact.source", toward, delta),
};
// Engine state refs' `byPlayer` recognizes the source by this wire spelling.
Object.defineProperty(impactSource, "__wire", { value: "@impact.source", enumerable: false });
const IMPACT_SOURCE: SourceHandle = Object.freeze(impactSource) as unknown as SourceHandle;

function assistSide(wire: "@assist.helper" | "@assist.target"): AssistSide {
  const side = {
    inLifeState: (name: string) => boolRef({ op: "input", owner: wire, name: `player.inLifeState:${name}` }),
  };
  // `byPlayer` recognizes each side by this wire spelling.
  Object.defineProperty(side, "__wire", { value: wire, enumerable: false });
  return Object.freeze(side) as unknown as AssistSide;
}

/** The helper and target of an assist pair, for `assistable.radius` and `assistable.holdMs` expressions. */
export const assist: Assist = Object.freeze({
  helper: assistSide("@assist.helper"),
  target: assistSide("@assist.target"),
});

// A bad reference (a bare string, a layer handle) names no registered type, so
// its input fails the bind and the policy holding it drops at install.
function impactTypeName(handle: unknown): string {
  const record = handle as { readonly kind?: unknown; readonly name?: unknown } | null;
  return record !== null && typeof record === "object" && record.kind === "damageType" && typeof record.name === "string"
    ? record.name
    : "@invalid";
}

const IMPACT: Impact = Object.freeze({
  target: IMPACT_TARGET,
  source: IMPACT_SOURCE,
  amount: numberRef({ op: "input", name: "@impact.amount" }),
  raw: numberRef({ op: "input", name: "@impact.raw" }),
  absorbed: numberRef({ op: "input", name: "@impact.absorbed" }),
  broke: boolRef({ op: "input", name: "@impact.broke" }),
  type: (damageType: import("postretro").DamageType) =>
    boolRef({ op: "input", name: `@impact.type.${impactTypeName(damageType)}` }),
});

/**
 * Spawn a passive presentation at the current impact target, shared with every
 * participating peer. Rust freezes `value` and the anchor before a subsequent
 * despawn can remove the target; source identity does not restrict the audience.
 */
export function present(template: PresentationTemplate, value: NumberValue): Effect {
  if (template === null || typeof template !== "object" || typeof template.id !== "string") {
    throw new TypeError("present: template must come from definePresentationTemplate");
  }
  return impactEffect("present", { template: template.id, value: numberNode(value) });
}

/** Build the closed absolute store-write effect. */
export function set(ref: Ref<number>, value: NumberValue): Effect {
  // Keep the ordinary global wire exactly as it was. An explicit owner is the
  // only signal that this must become the source-addressed command path.
  if (!("owner" in ref)) {
    return {
      primitive: "slot.set",
      args: {
        slot: ref.slot,
        value: numberNode(value),
      },
    } as ImpactEffectWire as unknown as Effect;
  }
  const owner = (ref as OwnerAddressedRef<number>).owner;
  return {
    primitive: "slot.set",
    target: owner,
    args: {
      slot: ref.slot,
      value: numberNode(value),
    },
  } as ImpactEffectWire as unknown as Effect;
}

/**
 * Build a read-modify-write effect. `build` receives `cur` during impact plan
 * phase, before effects apply. Owner-addressed reads resolve the live per-seat
 * map; `cur` is not a frozen snapshot.
 */
export function update(ref: Ref<number>, build: (cur: NumberRef) => NumberValue): Effect {
  return set(ref, build(read(ref)));
}

/** Build a deferred impact-effect group guarded by a Bool expression. */
export function when(cond: BoolRef, effects: readonly Effect[]): GatedEffect {
  return { when: cond, do: effects };
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

const IMPACT_EVENT_ID_DIAGNOSTIC = "impact-event `id` must be a single ASCII segment using only [A-Za-z0-9_.-], at most 64 bytes; the engine prefixes the mod id";
export const BINDING_NAME_SUGAR_DIAGNOSTIC = "defineStore/defineImpactEvent/defineDefenseLayer/defineDamageType/defineStatus without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration";

function validateImpactEventId(id: string): void {
  const valid = id.length > 0
    && id.length <= 64
    && /^[A-Za-z0-9_.-]+$/.test(id);
  if (!valid) throw new TypeError(IMPACT_EVENT_ID_DIAGNOSTIC);
}

/** Define a pure impact-policy descriptor. The engine prefixes its single-segment authored id with the mod id. Omit `id` only in a TypeScript direct top-level binding declaration compiled by scripts-build. Registration occurs only through a manifest's `events`. */
export function defineImpactEvent(
  filter: ImpactEventFilter,
  build: (impact: Impact) => readonly EffectOrGroup[],
): ImpactEvent;
export function defineImpactEvent(
  id: string,
  filter: ImpactEventFilter,
  build: (impact: Impact) => readonly EffectOrGroup[],
): ImpactEvent;
export function defineImpactEvent(
  idOrFilter: string | ImpactEventFilter,
  filterOrBuild: ImpactEventFilter | ((impact: Impact) => readonly EffectOrGroup[]),
  build?: (impact: Impact) => readonly EffectOrGroup[],
): ImpactEvent {
  if (arguments.length === 2) throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const id = idOrFilter as string;
  const filter = filterOrBuild as ImpactEventFilter;
  validateImpactEventId(id);
  const eventFilter = Object.freeze({ tag: filter.tag });
  const policy = lowerImpactPolicy(build!(IMPACT));
  return impactEvent(id, eventFilter, policy, filter.levels, false);
}
