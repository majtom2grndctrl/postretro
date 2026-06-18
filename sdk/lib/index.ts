// Public root SDK entry for `postretro`.
//
// The QuickJS prelude no longer bundles this file directly. It bundles
// `prelude.ts`, which deliberately re-exports additional UI symbols as
// implementation-only globals while imports are still stripped without alias
// rewriting. Keep root public exports here non-UI; a later alias-rewrite plan
// can remove the extra prelude globals without changing this module surface.
// See: context/lib/scripting.md §7
//
// When adding public root exports here, also update TS_SDK_LIB_BLOCK and
// LUAU_SDK_LIB_BLOCK in crates/postretro/src/scripting/typedef.rs.

export type { EntityForComponent, World } from "./world";
export { world } from "./world";

export { runtime } from "./runtime";

export type { AnimatableScalar, AnimatableVec3 } from "./animation";

export { getGameState } from "./game_state";

export type { LightEntityHandle } from "./entities/lights";

export type { FogVolumeHandle } from "./entities/fog_volumes";

export type { Keyframe } from "./util/keyframes";
export { timeline, sequence } from "./util/keyframes";

export type {
  LevelManifest,
  NamedReactionDescriptor,
  ProgressReactionDescriptor,
  PrimitiveReactionDescriptor,
  SequenceReactionDescriptor,
  SequenceStep,
  SetFogAnimationStep,
  SetFogDensityStep,
  SetFogEdgeSoftnessStep,
  SetFogFalloffStep,
  SetFogParamsStep,
  SetFogScatterStep,
  SetLightAnimationStep,
  StateRef,
  StoreDeclaration,
  StoreDefinition,
  StoreSlotSchema,
} from "./data_script";
export {
  defineReaction,
  defineEntity,
  defineMod,
  defineMapCatalog,
  defineStore,
} from "./data_script";

export type {
  BillboardEmitter,
  SpinAnimation,
  EmitterProps,
  ComponentDescriptor,
} from "./entities/emitters";
export {
  emitter,
  smokeEmitter,
  sparkEmitter,
  dustEmitter,
} from "./entities/emitters";
