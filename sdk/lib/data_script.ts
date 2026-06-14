// Data-script vocabulary: pure descriptor builders for `setupMod` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/** Invokes a named Rust primitive. With `tag`, it targets entities carrying that tag and mutates them. Without `tag`, it is a system reaction (no entities) that enqueues a typed engine command — `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args` carries the primitive's typed payload. */
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
  | SetFogAnimationStep;

/** Ordered per-entity primitive invocations. Steps run in array order at dispatch time. */
export type SequenceReactionDescriptor = {
  sequence: SequenceStep[];
};

/** `name` is merged into the descriptor at the top level so the Rust deserializer reads event name and body from one flat object. */
export type NamedReactionDescriptor = { name: string } & (
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor
  | SequenceReactionDescriptor
);

/**
 * Deserialized once at level load; the data-script VM is dropped immediately after.
 *
 * Entity-type registrations are not part of `LevelManifest`. Return them in
 * `setupMod`'s `entities` field instead — entity types are mod-level, not
 * level-level.
 */
export type LevelManifest = {
  reactions: NamedReactionDescriptor[];
  /** State-crossing watchers (HUD dynamics). See `onStateCrossing`. */
  crossings?: import("./ui/reactions").CrossingDescriptor[];
};

type ReactionBody =
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor
  | SequenceReactionDescriptor;

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
 * Returns a plain object — pure builder, no engine side effects. `name` is
 * optional: when omitted a deterministic, run-stable id is derived from the
 * body (see `autoReactionId`). The returned handle doubles as a typed reaction
 * reference for `Button`'s `onPress` and crossing `fire` entries.
 */
export function defineReaction(body: ReactionBody): NamedReactionDescriptor;
export function defineReaction(
  name: string,
  descriptor: ReactionBody,
): NamedReactionDescriptor;
export function defineReaction(
  nameOrBody: string | ReactionBody,
  descriptor?: ReactionBody,
): NamedReactionDescriptor {
  const [name, body] =
    typeof nameOrBody === "string"
      ? [nameOrBody, descriptor as ReactionBody]
      : [autoReactionId(nameOrBody), nameOrBody];
  return { name, ...body } as NamedReactionDescriptor;
}

/** Identity builder — gives authors a typed construction site for entity
 * type descriptors returned from `setupMod()`. Pure: no engine side effects. */
export function defineEntity(
  descriptor: import("postretro").EntityTypeDescriptor,
): import("postretro").EntityTypeDescriptor {
  return descriptor;
}
