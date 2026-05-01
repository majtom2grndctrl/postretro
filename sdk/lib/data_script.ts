// Data-script vocabulary: pure descriptor builders for `registerLevelManifest`.
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

/** Union of supported sequence step shapes. */
export type SequenceStep = SetLightAnimationStep;

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

/** Deserialized once at level load; the data-script VM is dropped immediately after. */
export type LevelManifest = {
  reactions: NamedReactionDescriptor[];
};

/** Returns a plain object — does not register anything in the engine despite the name. */
export function registerReaction(
  name: string,
  descriptor:
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor,
): NamedReactionDescriptor {
  return { name, ...descriptor } as NamedReactionDescriptor;
}
