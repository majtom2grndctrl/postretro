// Data-script vocabulary: pure descriptor builders for `setupMod` and `setupLevel`.
// FFI boundary is the `return` statement — these functions never call back into Rust.
// See: context/lib/scripting.md §2 (Data context lifecycle)

/** Fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/** Invokes a named Rust primitive on entities tagged `tag`, optionally firing `onComplete`. `args` carries the primitive's typed payload. */
export type PrimitiveReactionDescriptor = {
  primitive: string;
  tag: string;
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
};

/** Returns a plain object — pure builder, no engine side effects. */
export function defineReaction(
  name: string,
  descriptor:
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor,
): NamedReactionDescriptor {
  return { name, ...descriptor } as NamedReactionDescriptor;
}

/** Identity builder — gives authors a typed construction site for entity
 * type descriptors returned from `setupMod()`. Pure: no engine side effects. */
export function defineEntity(
  descriptor: import("postretro").EntityTypeDescriptor,
): import("postretro").EntityTypeDescriptor {
  return descriptor;
}
