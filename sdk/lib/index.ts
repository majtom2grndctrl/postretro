// SDK library entry — re-exports every public symbol the prelude installs as
// a global. Consumed at build time by `crates/postretro/build.rs` (via
// `postretro-script-compiler`); also callable via `scripts-build --prelude`.
// See: context/lib/scripting.md §7
//
// When adding exports here, also update TS_SDK_LIB_BLOCK and LUAU_SDK_LIB_BLOCK
// in crates/postretro/src/scripting/typedef.rs.

export type { EntityForComponent, World } from "./world";
export { world } from "./world";

export { runtime } from "./runtime";

export type { AnimatableScalar, AnimatableVec3 } from "./animation";

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
} from "./data_script";
export { defineReaction, defineEntity } from "./data_script";

export type { LocalizedText } from "./ui/text";

export type { CrossingCondition, CrossingDescriptor } from "./ui/reactions";
export {
  onStateCrossing,
  playSound,
  rumble,
  flashScreen,
  showDialog,
  openTextEntry,
  KEYBOARD_TREE,
  openMenu,
  closeDialog,
  setState,
  appendText,
  backspaceText,
  clearText,
} from "./ui/reactions";

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
