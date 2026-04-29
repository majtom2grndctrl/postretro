// Data-script vocabulary: pure descriptor builders for `registerLevelManifest`.
// `registerReaction` and `registerEntities` construct typed plain objects that
// the engine deserializes from the manifest's return value. They never call
// back into Rust — the FFI boundary is the `return` statement of
// `registerLevelManifest`.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// ---------------------------------------------------------------------------
// Canonical author example — wave-on-clear scripted reveal.
//
// ```typescript
// import { registerReaction, registerEntities } from "postretro";
// import { Grunt, HeavyGunner } from "./entities";
//
// export function registerLevelManifest(_ctx: unknown): LevelManifest {
//   return {
//     entities: registerEntities([Grunt, HeavyGunner]),
//     reactions: [
//       registerReaction("reactorWave1", {
//         progress: { tag: "reactorWave1Monsters", at: 1.0, fire: "wave1Complete" },
//       }),
//       registerReaction("wave1Complete", {
//         primitive: "moveGeometry",
//         tag: "reactorChambers",
//         onComplete: "wave2Revealed",
//       }),
//     ],
//   };
// }
// ```
// ---------------------------------------------------------------------------

/** Progress-subscription reaction: fires `fire` when entities tagged `tag` cross the kill ratio `at` (0.0–1.0). */
export type ProgressReactionDescriptor = {
  progress: { tag: string; at: number; fire: string };
};

/**
 * Primitive reaction: invokes the named Rust primitive on entities tagged `tag`,
 * optionally firing `onComplete` when it finishes. `args` carries the
 * primitive's typed payload (e.g. `{ rate: 0 }` for `setEmitterRate`).
 */
export type PrimitiveReactionDescriptor = {
  primitive: string;
  tag: string;
  args?: Record<string, unknown>;
  onComplete?: string;
};

/**
 * One step in a `sequence` reaction body: invokes the named sequenced
 * primitive against the given entity with `args`. Sequence steps target a
 * single `EntityId`; tag-targeted primitives (`setEmitterRate`, `setSpinRate`)
 * belong on the `reactions:` → `Primitive` path, not on `sequence`.
 */
export type SetLightAnimationStep = {
  id: import("postretro").EntityId;
  primitive: "setLightAnimation";
  args: import("postretro").LightAnimation;
};

/** Union of every supported sequence step shape. Add new step types here as more sequenced primitives land. */
export type SequenceStep = SetLightAnimationStep;

/**
 * Sequence reaction: ordered per-entity primitive invocations. Steps run in
 * array order at dispatch time. Build the step array inline at the call site;
 * this descriptor is just a thin wrapper around it.
 */
export type SequenceReactionDescriptor = {
  sequence: SequenceStep[];
};

/**
 * Descriptor produced by `registerReaction`. The `name` field is merged into
 * the descriptor at the top level so the Rust deserializer can read both the
 * event name and the descriptor body from a single flat object.
 */
export type NamedReactionDescriptor = { name: string } & (
  | ProgressReactionDescriptor
  | PrimitiveReactionDescriptor
  | SequenceReactionDescriptor
);

/** Descriptor produced by `registerEntities` — one entry per registered class. */
export type EntityTypeDescriptor = { classname: string };

/**
 * Bundle returned from `registerLevelManifest`. The engine deserializes this
 * shape in one pass at level load and drops the data-script VM context after
 * the return.
 */
export type LevelManifest = {
  entities: EntityTypeDescriptor[];
  reactions: NamedReactionDescriptor[];
};

/**
 * Build a named reaction descriptor. Pure: returns a plain object, does not
 * register anything in the engine. The engine consumes the object when
 * `registerLevelManifest` returns.
 */
export function registerReaction(
  name: string,
  descriptor:
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor,
): NamedReactionDescriptor {
  return { name, ...descriptor } as NamedReactionDescriptor;
}

/**
 * Build the entity-type descriptor list for `LevelManifest.entities`. Each
 * input class is reduced to its `classname` — that's the only field the
 * engine reads at registration time. Pure: returns a fresh array.
 */
export function registerEntities(
  types: ReadonlyArray<{ classname: string }>,
): EntityTypeDescriptor[] {
  return types.map((t) => ({ classname: t.classname }));
}
