// SDK library entry — re-exports every public symbol the prelude installs as
// a global. The `--prelude` mode of `scripts-build` consumes this file.
// See: context/lib/scripting.md §7
//
// When adding exports here, also update TS_SDK_LIB_BLOCK and LUAU_SDK_LIB_BLOCK in crates/postretro/src/scripting/typedef.rs and regenerate sdk/lib/prelude.js.

export type { EntityForComponent, World } from "./world";
export { world } from "./world";

export type { EasingCurve, LightEntity } from "./entities/lights";
export { flicker, pulse, colorShift, sweep } from "./entities/lights";

export type { AnimationController, FogVolumeHandle } from "./entities/fog_volumes";
export { pulseDensity } from "./entities/fog_volumes";

export type { Keyframe } from "./util/keyframes";
export { timeline, sequence } from "./util/keyframes";

export type {
  LevelManifest,
  NamedReactionDescriptor,
  ProgressReactionDescriptor,
  PrimitiveReactionDescriptor,
  SequenceReactionDescriptor,
  SequenceStep,
  SetLightAnimationStep,
} from "./data_script";
export { registerReaction } from "./data_script";

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
