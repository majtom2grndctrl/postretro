// Data-script vocabulary: pure descriptor builders for `ModManifest` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

import type { ComputedRef, Ref } from "./ui/widgets";
import type { PresentationTemplate } from "./ui/presentation";
import type { RuntimeValue } from "postretro";
import { numberNode, boolNode, numberRef, boolRef } from "./util/expression_refs";
import type { NumberValue, BoolValue, NumberRef, BoolRef, RuntimeExpressionRefs } from "./util/expression_refs";
export type { NumberValue, BoolValue, NumberRef, BoolRef, RuntimeExpressionRefs } from "./util/expression_refs";
import { DISPATCH_PARAMS } from "./data_script/reactions";
import { PLAYER_TARGET } from "./data_script/commands";
import type {
  ActivatorsTarget,
  GroupCommand,
  PlayerTarget,
  SubjectTokenCommand,
  TriggerTarget,
} from "./data_script/commands";
import type { VolumeTriggerEventDescriptor, TriggerPoolDescriptor } from "./data_script/trigger_events";
import type { PlayerEventDescriptor } from "./data_script/player_events";
export { defineReaction, scopeReactions, wait, fire } from "./data_script/reactions";
export { npcs, players } from "./data_script/commands";
export type {
  ActivatorsTarget,
  GroupCommand,
  GroupKind,
  NpcGroup,
  NpcGroupFilter,
  NpcStateUpdateArgs,
  PlayerGroup,
  PlayerTarget,
  SubjectTokenCommand,
  SubjectTokenTarget,
  TriggerTarget,
} from "./data_script/commands";
export { becomes, ceases } from "./data_script/player_events";
export type {
  PlayerEventDescriptor,
  PlayerEventEdge,
  PlayerEventEdgeWord,
  PlayerEventOptions,
  PlayerEventReaction,
} from "./data_script/player_events";
export { defineTriggerEvent, defineTriggerPool } from "./data_script/trigger_events";
export type {
  TriggerEventDescriptor,
  TriggerEventReaction,
  TriggerEventRule,
  TriggerPoolDescriptor,
  VolumeTriggerEventDescriptor,
} from "./data_script/trigger_events";

/** Dispatch values published by a state-crossing fire. */
export type CrossingParams = Readonly<{
  rising: import("postretro").RuntimeRead;
}>;

/** Dispatch values published while a Number store slot accumulates. */
export type TickParams = Readonly<{
  dt: import("postretro").RuntimeRead;
}>;

declare const emitterTargetBrand: unique symbol;

/** Opaque anchor for where the current named gameplay event happened. Legal only as `playSound`'s `at`. */
export type EmitterTarget = Readonly<{ readonly [emitterTargetBrand]: true }>;

/** Dispatch values published by a named gameplay event: weapon, reload, impact, enemy, movement and mover events. */
export type EmitterParams = Readonly<{
  emitter: EmitterTarget;
}>;

/** Dispatch values published by an enter/exit trigger event. */
export type TriggerEventParams = Readonly<{
  activators: ActivatorsTarget;
  trigger: TriggerTarget;
  occupancy: import("postretro").RuntimeRead;
}>;

/** Dispatch values published by a player event (`players().on`): the player it fired for. */
export type PlayerEventParams = Readonly<{
  player: PlayerTarget;
}>;

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/** Invokes a named Rust primitive. A group command (`npcs(...)`, `players()`) carries `kind` and an optional `tag` filter; a subject-token command (`on.activators`, `on.trigger`, `on.player`) carries `target: "@activators"`, `"@trigger"` or `"@player"`. A raw descriptor with only a non-empty `tag` resolves over every entity carrying it (fog, emitter and animation primitives have no typed builder). True system reactions carry neither and enqueue typed engine commands such as `playSound`, `rumble`, `flashScreen`, and the UI-stack reactions. `args` carries the primitive's typed payload. */
export type PrimitiveReactionDescriptor = {
  primitive: string;
  kind?: "npc" | "player";
  tag?: string;
  target?: "@activators" | "@trigger" | "@player";
  args?: Record<string, unknown>;
  onComplete?: string;
};

/**
 * One step in a `sequence` reaction body. A member step targets a single
 * `EntityId`; a group command (`GroupCommand`) resolves its group when it runs;
 * a subject-token command (`SubjectTokenCommand`) addresses the fire's subject
 * and is legal only before any `wait`.
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
export type SetFogGlowStep = import("postretro").SetFogGlowStep;
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
export type SpawnFromSpawnerStep = import("postretro").SpawnFromSpawnerStep;
export type WaitStep = import("postretro").WaitStep;
export type FireStep = import("postretro").FireStep;

/** Union of supported sequence step shapes. Mirrors the generated
 * `SequenceStep` in `postretro.d.ts`; new sequenced primitives extend
 * both ends of the union together. */
export type SequenceStep =
  | SetLightAnimationStep
  | SetFogDensityStep
  | SetFogGlowStep
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
  | DisarmTriggerStep
  | SpawnFromSpawnerStep
  | GroupCommand
  | SubjectTokenCommand
  | WaitStep
  | FireStep;

/** Ordered member, group, subject-token and control steps. Steps begin in
 * array order; `fire` queues a named dispatch, while `wait` stops the current
 * drain and resumes the remaining tail after its delay. */
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
  /** Level trigger events, keyed by volume: build each with a trigger member's `on`. */
  triggerEvents?: VolumeTriggerEventDescriptor[];
  triggerPools?: TriggerPoolDescriptor[];
  /** Per-player events, built with `players().on`. A level's entries belong to that level; an entry carrying `levels` is skipped with a warning (its siblings install). */
  playerEvents?: PlayerEventDescriptor[];
  /** Per-level UI trees (name + `AnchoredTree` + optional `alwaysOn` / `hideBelow`). Optional; same
   * shape as `ModManifest.uiTrees` but level-scoped (cleared on unload).
   * Malformed entries are logged and skipped. */
  uiTrees?: import("postretro").ModUiTree[];
};

/** One slot inside a `defineStore` schema. Every slot needs `default`. `type: "number"` accepts a finite numeric default plus optional inclusive `range: [min, max]`; `"boolean"` and `"string"` require matching defaults; `"enum"` requires non-empty `values` and a default in that list; `"array"` is a finite-number array. `persist` saves global slots on clean exit. On connected clients, `perOwner: true, persist: true` saves the local owner's value every ~60 seconds and on clean exit; it travels via that player's join seed. `readonly` blocks script writes. `perOwner: true` creates one value per player seat and permits only omitted `network` (host-local) or `"ownerPrivate"`; `"shared"` is for global slots. A mod-owned persisted writable or replicated slot requires a minted `<mod-root>/identity.json` entry; run `postretro-tool mint-identity <mod>`. Keep its durable key when renaming the store or slot. */
export type StoreSlotSchema = (
  | { type: "number"; readonly?: boolean; network?: "shared"; perOwner?: false; accumulate?: never }
  | { type: "number"; readonly?: boolean; network?: "ownerPrivate"; perOwner: true; persist?: boolean; accumulate?: never }
  | { type: "number"; readonly?: false; network?: "shared"; perOwner?: false; accumulate: (t: TickParams) => import("postretro").RuntimeValue }
  | { type: "boolean" | "string" | "enum" | "array"; readonly?: boolean; network?: "shared"; perOwner?: false; accumulate?: never }
  | { type: "boolean" | "string" | "enum" | "array"; readonly?: boolean; network?: "ownerPrivate"; perOwner: true; persist?: boolean; accumulate?: never }
) & Record<string, unknown>;

export type StoreDeclaration = {
  namespace: string;
  schema: Record<string, StoreSlotSchema>;
};

/** The wire owner a `byPlayer` read or write addresses. */
export type OwnerToken = "@impact.source" | "@player";
/** A ref with an explicit owner token, suitable for owner-addressed read/write lowering. */
export type OwnerAddressedComputedRef<T> = ComputedRef<T> & { readonly owner: OwnerToken };
export type OwnerAddressedRef<T> = Ref<T> & { readonly owner: OwnerToken };
/** Who `byPlayer` addresses: the impact damager (`impact.source`) or a player event's player (`on.player`). */
export type PlayerOwner = SourceHandle | PlayerTarget;

type StoreComputedRef<T> = ComputedRef<T> & {
  byPlayer(owner: PlayerOwner): OwnerAddressedComputedRef<T>;
};
type StoreRef<T> = Ref<T> & {
  byPlayer(owner: PlayerOwner): OwnerAddressedRef<T>;
};

export type StateRef<T = unknown> = StoreComputedRef<T> | StoreRef<T>;

export type StoreStateRefForSlot<Slot, T> =
  Slot extends { readonly: true } ? StoreComputedRef<T> : StoreRef<T>;

export type StateValueForSlot<Slot> =
  Slot extends { type: "number" } ? StoreStateRefForSlot<Slot, number> :
  Slot extends { type: "boolean" } ? StoreStateRefForSlot<Slot, boolean> :
  Slot extends { type: "array" } ? StoreStateRefForSlot<Slot, ReadonlyArray<number>> :
  StoreStateRefForSlot<Slot, string>;

declare const storeHandleBrand: unique symbol;

/** A frozen store handle whose enumerable keys are its schema's slot refs. */
export type StoreDefinition<S extends Record<string, StoreSlotSchema>> = {
  readonly [K in keyof S]: StateValueForSlot<S[K]>;
} & {
  readonly [storeHandleBrand]: S;
};

type StoreHandle = { readonly [storeHandleBrand]: unknown };
type ModManifestInput = Omit<import("postretro").ModManifest, "stores"> & {
  readonly stores?: readonly (StoreDeclaration | StoreHandle)[];
};

export type ReactionBody =
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
  /** Adjust this faction's sentiment toward another impact recipient. Negative values degrade the relationship; positive values strengthen its bond. */
  adjustSentimentToward(toward: TargetHandle | SourceHandle, delta: NumberValue): Effect;
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

const storeDeclarations = new WeakMap<object, StoreDeclaration>();

/** Public lifting helpers keep the raw-node adapters private to the SDK. */
export const fromRuntime: RuntimeExpressionRefs = Object.freeze({
  number: (value) => numberRef(value),
  bool: (value) => boolRef(value),
});

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
  setState(name, value) {
    return impactEffect("setState", { name, value: numberNode(value) });
  },
});

const impactSource = {
  grantHealth: (amount: NumberValue) => sourceImpactEffect("grantHealth", { amount: numberNode(amount) }),
  grantAmmo: (type: string, amount: NumberValue) => sourceImpactEffect("grantAmmo", { type, amount: numberNode(amount) }),
  adjustSentimentToward: (toward: TargetHandle | SourceHandle, delta: NumberValue) =>
    sentimentImpactEffect("@impact.source", toward, delta),
};
// Engine state refs' `byPlayer` recognizes the source by this wire spelling.
Object.defineProperty(impactSource, "__wire", { value: "@impact.source", enumerable: false });
const IMPACT_SOURCE: SourceHandle = Object.freeze(impactSource) as unknown as SourceHandle;

const IMPACT: Impact = Object.freeze({
  target: IMPACT_TARGET,
  source: IMPACT_SOURCE,
  amount: numberRef({ op: "input", name: "@impact.amount" }),
});

/**
 * Spawn a passive presentation at the current impact target. The target token
 * is closed over here; Rust stamps the dispatch source as its future presenter
 * route and freezes `value` before a subsequent despawn can remove the target.
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
const BINDING_NAME_SUGAR_DIAGNOSTIC = "defineStore/defineImpactEvent without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration";

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

export function defineEntity<T>(
  descriptor: T & import("postretro").EntityTypeDescriptor,
): T {
  lowerLoadoutReferences(descriptor);
  return descriptor;
}

/**
 * Identity builder for the mod manifest consumed from the default export.
 * `config.name`, `config.id`, and `config.version` are required. The id gates
 * multiplayer admission; the version is display-only and never compared. The
 * first committed id and version remain active across staged reloads. Optional
 * arrays include `entities`, `factions`, `sentiment`, `maps`, `uiTrees`, `presentationTemplates`,
 * `reactions`, `events`, `crossings`, `triggerEvents`, `triggerPools`,
 * `playerEvents`, and `stores`; `presentationOverlays` accepts one descriptor. Pure: no engine side
 * effects until the manifest is returned and validated. `factionSentimentDecay`
 * is an optional non-negative global return-to-baseline rate; it defaults to
 * zero (hold), and an individual sentiment row may override it with `decay`.
 */
export function defineMod(
  config: ModManifestInput,
): import("postretro").ModManifest {
  if (config.stores === undefined) return config as import("postretro").ModManifest;
  return {
    ...config,
    stores: config.stores.map((entry) => storeDeclarations.get(entry as object) ?? entry),
  } as import("postretro").ModManifest;
}

/** Pure builder for a stable named faction declaration. Include the result in `defineMod({ factions: [...] })`; archetypes refer to it by `components.faction`. Names beginning with `@postretro.` are reserved by the engine. */
export function defineFaction(name: string): import("postretro").FactionDescriptor {
  return { name };
}

/** Build one directed faction relationship for `defineMod({ sentiment: [...] })`.
 * Negative values are hostile, zero is neutral, and positive is allied. An
 * optional non-negative `decay` rate overrides the manifest-wide return rate;
 * zero holds this relationship after it changes at runtime. */
export function sentiment(
  fromFaction: string,
  toFaction: string,
  values: Pick<
    import("postretro").FactionSentimentDescriptor,
    "sentiment" | "tolerance" | "decay"
  >,
): import("postretro").FactionSentimentDescriptor {
  return { fromFaction, toFaction, ...values };
}

/** Identity builder for a mod map catalog. `entries` are `ModMapEntry` objects with required `id`, `path`, and `name`; optional `tags` default to empty and drive filtering plus `levels` selectors. Pure: no engine side effects. */
export function defineMapCatalog(
  entries: import("postretro").ModMapEntry[],
): import("postretro").ModMapEntry[] {
  return entries;
}

/** Pure identity builder for a reusable first-person weapon placement. The returned descriptor may be shared by weapon `placement` fields and `defineMod({ defaultWeaponPlacement })`; it performs no FFI or registration. */
export function defineWeaponPlacement(
  desc: import("postretro").WeaponPlacementDescriptor,
): import("postretro").WeaponPlacementDescriptor {
  return desc;
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

/** Pure state-store builder. `namespace` prefixes every returned slot ref as `namespace.slotName` and neither it nor a slot name may contain `:`. Omit `namespace` only in a TypeScript direct top-level binding declaration compiled by scripts-build. Pass the returned handle through `defineMod({ stores: [store] })` to resolve its declaration; unreturned handles are discarded with the setup VM. */
export function defineStore<const S extends Record<string, StoreSlotSchema>>(
  schema: S,
): StoreDefinition<S>;
export function defineStore<const S extends Record<string, StoreSlotSchema>>(
  namespace: string,
  schema: S,
): StoreDefinition<S>;
export function defineStore<const S extends Record<string, StoreSlotSchema>>(
  namespaceOrSchema: string | S,
  schema?: S,
): StoreDefinition<S> {
  if (arguments.length === 1) throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const namespace = namespaceOrSchema as string;
  const namedSchema = schema!;
  if (namespace.includes(":")) throw new TypeError("defineStore `namespace` must not contain `:`");
  const tracedSchema: Record<string, StoreSlotSchema> = Object.create(null);
  for (const [slot, input] of Object.entries(namedSchema)) {
    if (slot.includes(":")) throw new TypeError(`defineStore slot name ${JSON.stringify(slot)} must not contain \`:\``);
    if (input !== null && typeof input === "object" && typeof input.accumulate === "function") {
      tracedSchema[slot] = { ...input, accumulate: input.accumulate(DISPATCH_PARAMS) } as StoreSlotSchema;
    } else {
      tracedSchema[slot] = input;
    }
  }
  const frozenSchema = cloneAndFreeze(tracedSchema) as S;
  const store: Record<string, StateRef> = Object.create(null);
  for (const slot of Object.keys(frozenSchema)) {
    store[slot] = storeRef(
      `${namespace}.${slot}`,
      frozenSchema[slot].type,
      frozenSchema[slot].perOwner === true,
    );
  }
  const handle = Object.freeze(store) as StoreDefinition<S>;
  storeDeclarations.set(handle, Object.freeze({ namespace, schema: frozenSchema }));
  return handle;
}

function storeRef(slot: string, kind: StateRef["kind"], perOwner: boolean): StateRef {
  const ref = { slot, kind };
  Object.defineProperty(ref, "byPlayer", {
    value: (owner: PlayerOwner): OwnerAddressedRef<unknown> => {
      if (!perOwner) {
        throw new TypeError(`state slot \`${slot}\` is global and cannot be addressed with byPlayer`);
      }
      const token = owner === IMPACT_SOURCE
        ? "@impact.source"
        : (owner as unknown) === PLAYER_TARGET
          ? "@player"
          : "@invalid";
      return Object.freeze({ slot, kind, owner: token }) as OwnerAddressedRef<unknown>;
    },
    enumerable: false,
    configurable: false,
    writable: false,
  });
  return Object.freeze(ref) as StateRef;
}
