// Data-script vocabulary: pure descriptor builders for `ModManifest` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

import type { ComputedRef, Ref } from "./ui/widgets";
import type { PresentationTemplate } from "./ui/presentation";
import type { RuntimeValue } from "postretro";
import { numberNode, boolNode, numberRef, boolRef, loweredValue } from "./util/expression_refs";
import type { NumberValue, BoolValue, NumberRef, BoolRef, RuntimeExpressionRefs } from "./util/expression_refs";
export type { NumberValue, BoolValue, NumberRef, BoolRef, RuntimeExpressionRefs } from "./util/expression_refs";
import { DISPATCH_PARAMS } from "./data_script/reactions";
import type {
  ActivatorsTarget,
  GroupCommand,
  PlayerTarget,
  SubjectTokenCommand,
  TriggerTarget,
  TeleporterTarget,
  SeamTarget,
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
  RespawnOptions,
  PlayerTarget,
  SubjectTokenCommand,
  SubjectTokenTarget,
  TriggerTarget,
  TeleporterTarget,
  SeamTarget,
} from "./data_script/commands";
export { assistGiven, assistReceived, becomes, ceases } from "./data_script/player_events";
export type {
  PlayerEventDescriptor,
  PlayerEventDiscreteWord,
  PlayerEventEdge,
  PlayerEventEdgeWord,
  PlayerEventOptions,
  PlayerEventReaction,
} from "./data_script/player_events";
export { defineTriggerEvent, defineTriggerPool } from "./data_script/trigger_events";
import { BINDING_NAME_SUGAR_DIAGNOSTIC } from "./data_script/impact_events";
import type { AssistSide, ImpactEvent, SourceHandle } from "./data_script/impact_events";
export { assist, defineImpactEvent, fromRuntime, inLifeState, present, read, set, update, when } from "./data_script/impact_events";
export type { Assist, AssistSide, Effect, EffectOrGroup, GatedEffect, Impact, ImpactEvent, ImpactEventFilter, ImpactEventOverrideFilter, SourceHandle, TargetHandle } from "./data_script/impact_events";
import { attackTokenDeclarationOf } from "./data_script/attack_budget";
export { defaultToken, defineAttackToken } from "./data_script/attack_budget";
export { defineAttack, defineStance } from "./data_script/attacks";
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

export type TeleportEventParams = Readonly<{ activators: ActivatorsTarget; teleporter: TeleporterTarget }>;

/** Dispatch values published by a level seam's `arrive` event: the arriving pawn and the seam it arrived by. */
export type SeamEventParams = Readonly<{ activators: ActivatorsTarget; seam: SeamTarget }>;

/** Dispatch values published by a player event (`players().on`): the player it fired for. */
export type PlayerEventParams = Readonly<{
  player: PlayerTarget;
}>;

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

declare const reactionBodyScopeBrand: unique symbol;

/** A body retains the fire context its builders require, including direct-body registration. */
export type ReactionBodyScope<S> = {
  readonly [reactionBodyScopeBrand]?: (scope: S) => void;
};

/** Invokes a named Rust primitive. A group command (`npcs(...)`, `players()`) carries `kind` and an optional `tag` filter; a subject-token command (`on.activators`, `on.trigger`, `on.teleporter`, `on.player`) carries `target: "@activators"`, `"@trigger"`, `"@teleporter"` or `"@player"`. A raw descriptor with only a non-empty `tag` resolves over every entity carrying it (fog, emitter and animation primitives have no typed builder). True system reactions carry neither and enqueue typed engine commands such as `playSound`, `rumble`, `flashScreen`, and the UI-stack reactions. `args` carries the primitive's typed payload. */
export type PrimitiveReactionDescriptor<S = {}> = ReactionBodyScope<S> & {
  primitive: string;
  kind?: "npc" | "player";
  tag?: string;
  target?: "@activators" | "@trigger" | "@teleporter" | "@seam" | "@player";
  args?: Record<string, unknown> | null;
  onComplete?: string;
};

/** A system reaction (`playSound`, `updateState`, the Ghost bus setter, …): a body
 * on its own, and a sequence step unspread, doing what the same reaction does
 * as a body. It names no target. */
export type SystemReactionStep<S = {}> = ReactionBodyScope<S> & {
  primitive: string;
  args?: Record<string, unknown> | null;
  id?: never;
  kind?: never;
  tag?: never;
  target?: never;
  onComplete?: never;
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
export type ArmTeleporterStep = { id: import("postretro").EntityId; primitive: "armTeleporter"; args: Record<string, never> };
export type DisarmTeleporterStep = { id: import("postretro").EntityId; primitive: "disarmTeleporter"; args: Record<string, never> };
export type ArmSeamStep = { id: import("postretro").EntityId; primitive: "armSeam"; args: Record<string, never> };
export type DisarmSeamStep = { id: import("postretro").EntityId; primitive: "disarmSeam"; args: Record<string, never> };
/** Member categories a ghost link shows; an omitted category keeps its value. */
export type GhostLinkMembersArgs = { players?: boolean; entities?: boolean; projectiles?: boolean; movers?: boolean };
/** A ghost link's colour filter: each named field ramps on its own over `periodMs` (default 0); an omitted field keeps its value and any ramp in flight. `tint` is `[r, g, b]`; `tint` components, `opacity` and `saturation` lie in 0..=1, and `periodMs` is finite and at least 0. Naming none of `tint`, `opacity` and `saturation` warns and drops the step. */
export type GhostColorFilterArgs = { tint?: readonly [number, number, number]; opacity?: number; saturation?: number; periodMs?: number };
export type ArmGhostLinkStep = { id: import("postretro").EntityId; primitive: "armGhostLink"; args: Record<string, never> };
export type DisarmGhostLinkStep = { id: import("postretro").EntityId; primitive: "disarmGhostLink"; args: Record<string, never> };
export type SetGhostLinkMembersStep = { id: import("postretro").EntityId; primitive: "setGhostLinkMembers"; args: GhostLinkMembersArgs };
export type SetGhostLinkColorFilterStep = { id: import("postretro").EntityId; primitive: "setGhostLinkColorFilter"; args: GhostColorFilterArgs };
export type ArmTriggerStep = import("postretro").ArmTriggerStep;
export type DisarmTriggerStep = import("postretro").DisarmTriggerStep;
export type SpawnFromSpawnerStep = import("postretro").SpawnFromSpawnerStep;
export type FireTrapStep = import("postretro").FireTrapStep;
export type StartTrapStep = import("postretro").StartTrapStep;
export type StopTrapStep = import("postretro").StopTrapStep;
export type StartAttackPoolStep = import("postretro").StartAttackPoolStep;
export type StopAttackPoolStep = import("postretro").StopAttackPoolStep;
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
  | ArmTeleporterStep
  | DisarmTeleporterStep
  | ArmSeamStep
  | DisarmSeamStep
  | ArmGhostLinkStep
  | DisarmGhostLinkStep
  | SetGhostLinkMembersStep
  | SetGhostLinkColorFilterStep
  | ArmTriggerStep
  | DisarmTriggerStep
  | SpawnFromSpawnerStep
  | FireTrapStep
  | StartTrapStep
  | StopTrapStep
  | StartAttackPoolStep
  | StopAttackPoolStep
  | GroupCommand
  | SubjectTokenCommand
  | SystemReactionStep<any>
  | WaitStep
  | FireStep;

/** A step as an array or `sequence` entry. A reaction handle, a `progress`
 * body, a nested `{ sequence }` and an `onComplete` follow-up are bodies, not
 * steps: fire a handle with `fire(r)`. */
export type ReactionStepEntry = SequenceStep & {
  name?: never;
  progress?: never;
  sequence?: never;
  onComplete?: never;
};

/** Steps in authored order. Nested arrays flatten at any depth, so the arrays
 * member methods, `wait()` and `fire()` return need no spread. */
export type ReactionStepList = readonly (ReactionStepEntry | ReactionStepList)[];

/** An authored `{ sequence }` body: ordered member, group, subject-token,
 * system and control steps. The list may nest; `defineReaction` flattens it.
 * Steps begin in array order; `fire` queues a named dispatch, while `wait`
 * stops the current drain and resumes the remaining tail after its delay. */
export type SequenceReactionDescriptor = {
  sequence: ReactionStepList;
};

/** A `{ sequence }` as the manifest and every `Reaction` carry it: one flat
 * list of steps. Only `defineReaction` flattens, so a raw manifest literal
 * spreads the arrays builders return. */
export type ManifestSequenceDescriptor = {
  sequence: readonly ReactionStepEntry[];
};

/** `name` is merged into the descriptor at the top level so the Rust deserializer reads event name and body from one flat object. */
export type NamedReactionDescriptor<S = any> = { name: string; levels?: string[] } & ManifestReactionBody<S>;

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
  teleportEvents?: import("./entities/teleporters").TeleportEventDescriptor[];
  /** Level seam events, keyed by seam: build each with a seam member's `on`. */
  seamEvents?: import("./entities/seams").SeamEventDescriptor[];
  triggerPools?: TriggerPoolDescriptor[];
  /** Per-player events, built with `players().on`. A level's entries belong to that level; an entry carrying `levels` is skipped with a warning (its siblings install). */
  playerEvents?: PlayerEventDescriptor[];
  /** Per-level UI trees (name + `AnchoredTree` + optional `alwaysOn` / `hideBelow`). Optional; same
   * shape as `ModManifest.uiTrees` but level-scoped (cleared on unload).
   * Malformed entries are logged and skipped. */
  uiTrees?: import("postretro").ModUiTree[];
};

/** One slot inside a `defineStore` schema. Every slot needs `default`. `type: "number"` accepts a finite numeric default plus optional inclusive `range: [min, max]`; `"boolean"` and `"string"` require matching defaults; `"enum"` requires non-empty `values` and a default in that list; `"array"` is a finite-number array. `persist: true` saves a global slot in the world file and a per-owner slot in the character; `persist: "run"` saves a global slot in the current run and is refused on a per-owner slot. On connected clients, `perOwner: true, persist: true` saves the local owner's value every ~60 seconds and on clean exit; it travels via that player's join seed. `readonly` blocks script writes. `perOwner: true` creates one value per player seat and permits only omitted `network` (host-local) or `"ownerPrivate"`; `"shared"` is for global slots. A mod-owned persisted writable or replicated slot requires a minted `<mod-root>/identity.json` entry; run `postretro-tool mint-identity <mod>`. Keep its durable key when renaming the store or slot. */
export type StoreSlotSchema = (
  | { type: "number"; readonly?: boolean; network?: "shared"; perOwner?: false; persist?: boolean | "run"; accumulate?: never }
  | { type: "number"; readonly?: boolean; network?: "ownerPrivate"; perOwner: true; persist?: boolean; accumulate?: never }
  | { type: "number"; readonly?: false; network?: "shared"; perOwner?: false; persist?: boolean | "run"; accumulate: (t: TickParams) => import("postretro").RuntimeValue }
  | { type: "boolean" | "string" | "enum" | "array"; readonly?: boolean; network?: "shared"; perOwner?: false; persist?: boolean | "run"; accumulate?: never }
  | { type: "boolean" | "string" | "enum" | "array"; readonly?: boolean; network?: "ownerPrivate"; perOwner: true; persist?: boolean; accumulate?: never }
) & Record<string, unknown>;

export type StoreDeclaration = {
  namespace: string;
  schema: Record<string, StoreSlotSchema>;
};

/** The wire owner a `byPlayer` read or write addresses. */
export type OwnerToken = "@impact.source" | "@player" | "@assist.helper" | "@assist.target";
/** A ref with an explicit owner token, suitable for owner-addressed read/write lowering. */
export type OwnerAddressedComputedRef<T> = ComputedRef<T> & { readonly owner: OwnerToken };
export type OwnerAddressedRef<T> = Ref<T> & { readonly owner: OwnerToken };
/** Who `byPlayer` addresses: the impact damager (`impact.source`), a player event's player (`on.player`), or a side of an assist pair expression (`assist.helper`, `assist.target`). */
export type PlayerOwner = SourceHandle | PlayerTarget | AssistSide;

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

/** A body as the manifest carries it: one primitive, a flat `{ sequence }` or
 * a `{ progress }`. */
export type ManifestReactionBody<S = {}> = ReactionBodyScope<S> & (
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor<S>
  | ManifestSequenceDescriptor
);

/** A reaction body as authored: a `{ progress }`, a primitive, a `{ sequence }`
 * whose list may nest, or an array of steps, which `defineReaction` lowers to
 * a flat `{ sequence }`. An empty array is an inert reaction. */
export type ReactionBody<S = {}> =
  | (ReactionBodyScope<S> & (
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor<S>
    | SequenceReactionDescriptor
  ))
  | ReactionStepList;

declare const reactionScopeBrand: unique symbol;

/** A named reaction whose phantom dispatch scope is enforced only by TypeScript. */
export type Reaction<S = {}> = NamedReactionDescriptor<S> & {
  readonly [reactionScopeBrand]?: (scope: S) => void;
};

// Impact policies use the shipped, closed runtime IR without widening its
// evaluator vocabulary. Refs keep the raw node private so only descriptor
// builders can lower them into manifest data.
const storeDeclarations = new WeakMap<object, StoreDeclaration>();

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

/**
 * Replaces each fluent ref in a life state's `assistable` with its IR: a ref
 * keeps its node outside the object, so it would not survive serialization.
 */
function lowerAssistableReferences(descriptor: import("postretro").EntityTypeDescriptor): void {
  const states = descriptor.components?.lifeStates?.states;
  if (states === null || typeof states !== "object") return;
  for (const profile of Object.values(states)) {
    const assistable = (profile as { assistable?: unknown } | null)?.assistable;
    if (assistable === null || typeof assistable !== "object") continue;
    const fields = assistable as Record<string, unknown>;
    for (const field of ["radius", "holdMs"]) {
      if (fields[field] !== undefined) fields[field] = loweredValue(fields[field]);
    }
  }
}

/**
 * Replaces each fluent ref in a health layer's parameters and in each
 * `resist` (a layer's and health's own) with its IR, as `assistable` does.
 */
function lowerDefenseLayerReferences(descriptor: import("postretro").EntityTypeDescriptor): void {
  const health = descriptor.components?.health as
    | { layers?: unknown; resist?: unknown }
    | null
    | undefined;
  if (health === null || typeof health !== "object") return;
  const lowerResist = (resist: unknown) => {
    if (resist === null || typeof resist !== "object") return;
    const map = resist as Record<string, unknown>;
    for (const key of Object.keys(map)) map[key] = loweredValue(map[key]);
  };
  if (Array.isArray(health.layers)) {
    for (const entry of health.layers) {
      if (entry === null || typeof entry !== "object") continue;
      const fields = entry as Record<string, unknown>;
      for (const field of ["capacity", "rechargeDelayMs", "rechargeRate"]) {
        if (fields[field] !== undefined) fields[field] = loweredValue(fields[field]);
      }
      lowerResist(fields.resist);
    }
  }
  lowerResist(health.resist);
}

export function defineEntity<T>(
  descriptor: T & import("postretro").EntityTypeDescriptor,
): T {
  lowerLoadoutReferences(descriptor);
  lowerAssistableReferences(descriptor);
  lowerDefenseLayerReferences(descriptor);
  return descriptor;
}

/**
 * Identity builder for the mod manifest consumed from the default export.
 * `config.name`, `config.id`, and `config.version` are required. The id gates
 * multiplayer admission; the version is display-only and never compared. The
 * first committed id and version remain active across staged reloads. Optional
 * arrays include `entities`, `factions`, `sentiment`, `fluids`, `ghostStyles`, `maps`, `uiTrees`, `presentationTemplates`,
 * `reactions`, `events`, `crossings`, `triggerEvents`, `triggerPools`,
 * `playerEvents`, and `stores`; `presentationOverlays` accepts one descriptor or an array of them, at most one per source kind. Pure: no engine side
 * effects until the manifest is returned and validated. `factionSentimentDecay`
 * is an optional non-negative global return-to-baseline rate; it defaults to
 * zero (hold), and an individual sentiment row may override it with `decay`.
 */
export function defineMod(
  config: ModManifestInput,
): import("postretro").ModManifest {
  const manifest: Record<string, unknown> = { ...config };
  if (config.stores !== undefined) {
    manifest.stores = config.stores.map((entry) => storeDeclarations.get(entry as object) ?? entry);
  }
  // A handle crosses as `{ kind, name }`; the manifest list carries its whole
  // declaration. A bare string or a handle of the other kind passes through,
  // and the engine rejects it naming the list entry.
  if (config.defenseLayers !== undefined) {
    manifest.defenseLayers = config.defenseLayers.map((entry) => declarationOf(defenseLayerDeclarations, entry));
  }
  if (config.damageTypes !== undefined) {
    manifest.damageTypes = config.damageTypes.map((entry) => declarationOf(damageTypeDeclarations, entry));
  }
  if (config.statuses !== undefined) {
    manifest.statuses = config.statuses.map((entry) => declarationOf(statusDeclarations, entry));
  }
  if (config.attackTokens !== undefined) {
    manifest.attackTokens = config.attackTokens.map(attackTokenDeclarationOf);
  }
  return manifest as import("postretro").ModManifest;
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

/** Identity builder for a fluid declaration in `defineMod({ fluids: [...] })`.
 * A map's `fluid_volume` names it through its `fluid` key; declaration order
 * assigns its index. `viscosity` is drag while swimming (1/s, >= 0);
 * `viewTint` is rgb plus strength, `muffle` and `surfaceOpacity` each in
 * [0, 1]; `enterSound` and `exitSound` are optional sound keys. Optional
 * presentation: `surfaceReflectivity` (0..50, default 1: 0 none, 1 water, 50 a
 * mirror) scales how strongly the surface reflects the room around it;
 * `fogDistance` (metres seen into the fluid from outside, > 0)
 * and `fogColor` (rgb in `viewTint`'s space; omitted, `viewTint`'s rgb) add
 * murk; `wavePeriod` (seconds, >= 6) turns on a slow surface wave whose
 * `waveAmplitude` (texels, 0..16, default 8) and `waveLength` (texels: a
 * number or `[min, max]`, at most 128, min >= 8 x amplitude) shape it. A field
 * out of range, or a repeated name, is rejected with a warning naming the
 * field, and the first declaration stands. `status`, a status handle, is the
 * exception: one naming a status `defineMod` does not register fails the
 * load. Pure: no engine side effects. */
export function defineFluid(
  desc: import("postretro").FluidDescriptor,
): import("postretro").FluidDescriptor {
  return desc;
}

/** Identity builder for a ghost style in `defineMod({ ghostStyles: [...] })`.
 * A map's `ghost_link` names it through its `ghost_style` key. `name` is
 * required; every other field is optional and an omitted one takes the floor
 * style's value: `tint` (rgb, default `[1, 1, 1]`), `opacity` (default 0.5)
 * and `saturation` (default 0, greyscale), each in [0, 1]; `audio` (default
 * false) plays members' sounds through the Ghost bus, and `impactAudio`
 * (default false) adds their impact sounds, only when `audio` is true. A
 * malformed field drops the whole style with a warning naming it, a repeated
 * name is dropped and the first declaration stands, and a link naming no
 * declared style uses the floor style. Pure: no engine side effects. */
export function defineGhostStyle(
  desc: import("postretro").GhostStyleDescriptor,
): import("postretro").GhostStyleDescriptor {
  return desc;
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

// Owner tokens `byPlayer` accepts, by the `__wire` spelling each owner handle
// carries. The drift guard in postretro-sim holds this list to the engine's
// `BY_PLAYER_OWNER_TOKENS`.
const BY_PLAYER_OWNER_TOKENS: readonly string[] = ["@player", "@impact.source", "@assist.helper", "@assist.target"];

function storeRef(slot: string, kind: StateRef["kind"], perOwner: boolean): StateRef {
  const ref = { slot, kind };
  Object.defineProperty(ref, "byPlayer", {
    value: (owner: PlayerOwner): OwnerAddressedRef<unknown> => {
      if (!perOwner) {
        throw new TypeError(`state slot \`${slot}\` is global and cannot be addressed with byPlayer`);
      }
      const wire = owner !== null && typeof owner === "object"
        ? (owner as { readonly __wire?: unknown }).__wire
        : undefined;
      const token = typeof wire === "string" && BY_PLAYER_OWNER_TOKENS.includes(wire) ? wire : "@invalid";
      return Object.freeze({ slot, kind, owner: token }) as OwnerAddressedRef<unknown>;
    },
    enumerable: false,
    configurable: false,
    writable: false,
  });
  return Object.freeze(ref) as StateRef;
}

// Declarations a `defineMod` list resolves its handles through, as stores do.
const defenseLayerDeclarations = new WeakMap<object, object>();
const damageTypeDeclarations = new WeakMap<object, object>();
const statusDeclarations = new WeakMap<object, object>();

function declarationOf(declarations: WeakMap<object, object>, entry: unknown): unknown {
  if (entry === null || typeof entry !== "object") return entry;
  return declarations.get(entry) ?? entry;
}

function hideProperty(target: object, key: string, value: unknown): void {
  Object.defineProperty(target, key, { value, enumerable: false, configurable: false, writable: false });
}

/** Declare a defense layer kind: a shield, armor or any pool that drains before health. Omit `name` only in a TypeScript direct top-level binding declaration compiled by scripts-build, which names it by its binding. Register it with `defineMod({ defenseLayers: [layer] })`, then attach it to descriptors in `components.health.layers`. `current` and `capacity` read the local player's layer. `options` is reserved and must be empty. */
export function defineDefenseLayer(options?: import("postretro").DefenseLayerOptions): import("postretro").DefenseLayer;
export function defineDefenseLayer(name: string, options?: import("postretro").DefenseLayerOptions): import("postretro").DefenseLayer;
export function defineDefenseLayer(
  nameOrOptions?: string | import("postretro").DefenseLayerOptions,
  options?: import("postretro").DefenseLayerOptions,
): import("postretro").DefenseLayer {
  if (typeof nameOrOptions !== "string") throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const name = nameOrOptions;
  const handle = { kind: "defenseLayer", name };
  hideProperty(handle, "current", storeRef(`player.defense.${name}`, "number", true));
  hideProperty(handle, "capacity", storeRef(`player.defense.${name}.capacity`, "number", true));
  Object.freeze(handle);
  defenseLayerDeclarations.set(handle, Object.freeze({ kind: "defenseLayer", name, options: options ?? {} }));
  return handle as unknown as import("postretro").DefenseLayer;
}

/** Declare a damage type by its matrix: how much one raw point takes from each pool kind, keyed by layer name or `health`. Each value is a multiplier in [0, 64] or, on a layer only, `"bypass"`; a missing pool is ×1. Omit `name` only in a TypeScript direct top-level binding declaration compiled by scripts-build. Register it with `defineMod({ damageTypes: [type] })`. `options` is reserved and must be empty. */
export function defineDamageType(matrix: import("postretro").DamageTypeMatrix, options?: import("postretro").DamageTypeOptions): import("postretro").DamageType;
export function defineDamageType(name: string, matrix: import("postretro").DamageTypeMatrix, options?: import("postretro").DamageTypeOptions): import("postretro").DamageType;
export function defineDamageType(
  nameOrMatrix: string | import("postretro").DamageTypeMatrix,
  matrix?: import("postretro").DamageTypeMatrix | import("postretro").DamageTypeOptions,
  options?: import("postretro").DamageTypeOptions,
): import("postretro").DamageType {
  if (typeof nameOrMatrix !== "string") throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const name = nameOrMatrix;
  const handle = Object.freeze({ kind: "damageType", name });
  damageTypeDeclarations.set(
    handle,
    Object.freeze({ kind: "damageType", name, matrix: { ...(matrix ?? {}) }, options: options ?? {} }),
  );
  return handle as unknown as import("postretro").DamageType;
}

/** Declare a status kind: damage over time, or with `damage: 0` a marker other policies read. Omit `name` only in a TypeScript direct top-level binding declaration whose only argument is an object literal, compiled by scripts-build, which names it by its binding. Register it with `defineMod({ statuses: [status] })`, then apply it from impact policies (`impact.target.applyStatus`), reactions (`players().applyStatus`) or a fluid's `status`. `active` reads whether the local player carries it. `maxStacks` is a whole number in [1, 64], default 1. */
export function defineStatus(options: import("postretro").StatusOptions): import("postretro").Status;
export function defineStatus(name: string, options: import("postretro").StatusOptions): import("postretro").Status;
export function defineStatus(
  nameOrOptions: string | import("postretro").StatusOptions,
  options?: import("postretro").StatusOptions,
): import("postretro").Status {
  if (typeof nameOrOptions !== "string") throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const name = nameOrOptions;
  const handle = { kind: "status", name };
  // A local-only read: no `byPlayer`, as `player.lifeState` has none.
  hideProperty(handle, "active", Object.freeze({ slot: `player.status.${name}`, kind: "boolean" }));
  Object.freeze(handle);
  statusDeclarations.set(handle, Object.freeze({ ...(options ?? {}), kind: "status", name }));
  return handle as unknown as import("postretro").Status;
}
