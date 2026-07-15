// Data-script vocabulary: pure descriptor builders for `ModManifest` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

import type { ReadonlyStateRef, WritableStateRef } from "./ui/widgets";

/** Dispatch values published by a state-crossing fire. */
export type CrossingParams = Readonly<{
  rising: import("postretro").RuntimeRead;
}>;

/** Dispatch values published while a Number store slot accumulates. */
export type TickParams = Readonly<{
  dt: import("postretro").RuntimeRead;
}>;

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/** Invokes a named Rust primitive. With `tag`, it targets entities carrying that tag and mutates them. Tag-targeted primitives include emitter/fog/mover commands, `applyDamage`, `setAnimationState`, `armTrigger`, and `disarmTrigger`; arm/disarm use empty args. Without `tag`, it is a system reaction (no entities) that enqueues a typed engine command — `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args` carries the primitive's typed payload. */
export type PrimitiveReactionDescriptor = {
  primitive: string;
  tag?: string;
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
  /** State-crossing watchers (HUD dynamics). See `onStateCrossing`. */
  crossings?: import("./ui/reactions").CrossingDescriptor[];
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

type ReactionTracer<S> = (params: S) => ReactionBody;

// This is deliberately one plain merged object rather than a Proxy. Sibling
// dispatch specs may add opaque, non-IR leaves alongside these input nodes.
const DISPATCH_PARAMS = Object.freeze({
  rising: Object.freeze({ op: "input", name: "@rising" } as const),
  dt: Object.freeze({ op: "input", name: "@dt" } as const),
});

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
export function defineReaction(
  name: string,
  descriptor: ReactionBody,
): Reaction<{}>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<CrossingParams>,
): Reaction<CrossingParams>;
export function defineReaction(
  nameOrBody: string | ReactionBody | ReactionTracer<CrossingParams>,
  descriptor?: ReactionBody | ReactionTracer<CrossingParams>,
): Reaction<{}> | Reaction<CrossingParams> {
  const authored = typeof nameOrBody === "string" ? descriptor : nameOrBody;
  const tracedBody = typeof authored === "function"
    ? authored(DISPATCH_PARAMS)
    : authored as ReactionBody;
  const [name, body] =
    typeof nameOrBody === "string"
      ? [nameOrBody, tracedBody]
      : [autoReactionId(tracedBody), tracedBody];
  return { name, ...body } as Reaction<{}> | Reaction<CrossingParams>;
}

/** Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are matched against `ModMapEntry.tags`; omit scoping for every level. */
export function scopeReactions<S>(
  tags: string[],
  list: Reaction<S>[],
): Reaction<S>[] {
  return list.map((reaction) => ({ ...reaction, levels: tags }));
}

/** Identity builder for entity type descriptors returned from `ModManifest.entities`. `descriptor` is the full archetype object: optional `canonicalName`, optional `defaultWeapon`, and optional component presets. Pure: no engine side effects. */
export function defineEntity(
  descriptor: import("postretro").EntityTypeDescriptor,
): import("postretro").EntityTypeDescriptor {
  return descriptor;
}

/** Identity builder for the mod manifest consumed from the default export. `config.name` is required; optional arrays include `entities`, `maps`, `uiTrees`, `reactions`, `crossings`, and `stores`. Pure: no engine side effects until the manifest is returned and validated. */
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
