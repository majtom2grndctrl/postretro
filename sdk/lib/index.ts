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
// LUAU_SDK_LIB_BLOCK in crates/scripting-core/src/typedef/.

export type { MapEntityFilter, MapEntityForKind, MapEntityKind } from "./map_entities";
export { getMapEntities } from "./map_entities";
export { getGravity, setGravity } from "./gravity";

export { runtime } from "./runtime";

export type { Activation, ActivationShotOptions, ActivationShotScale } from "./activation";
export { activation } from "./activation";

export type { BrainInputs, CandidateInputs } from "./brain";
export { brain, candidate, state } from "./brain";

export type { AnimatableScalar, AnimatableVec3 } from "./animation";

export { getGameState } from "./game_state";

export type { LightEntityHandle } from "./entities/lights";

export type { FogVolumeHandle } from "./entities/fog_volumes";

export type { MoverEntityHandle } from "./entities/movers";

export type { TriggerVolumeHandle } from "./entities/triggers";

export type { SpawnerEntityHandle } from "./entities/spawners";

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
  SetFogGlowStep,
  SetLightAnimationStep,
  MoverStartStep,
  MoverStopStep,
  MoverReverseStep,
  MoverGoToPathNodeStep,
  MoverSetSpinRateStep,
  ArmTriggerStep,
  DisarmTriggerStep,
  SpawnFromSpawnerStep,
  WaitStep,
  FireStep,
  CrossingParams,
  TickParams,
  TriggerEventParams,
  EmitterParams,
  EmitterTarget,
  TriggerEventDescriptor,
  TriggerEventReaction,
  TriggerEventRule,
  TriggerPoolDescriptor,
  VolumeTriggerEventDescriptor,
  ActivatorsTarget,
  TriggerTarget,
  GroupCommand,
  GroupKind,
  NpcGroup,
  NpcGroupFilter,
  NpcStateUpdateArgs,
  PlayerGroup,
  SubjectTokenCommand,
  SubjectTokenTarget,
  Reaction,
  NumberValue,
  BoolValue,
  NumberRef,
  BoolRef,
  RuntimeExpressionRefs,
  Effect,
  GatedEffect,
  EffectOrGroup,
  TargetHandle,
  SourceHandle,
  Impact,
  ImpactEvent,
  ImpactEventFilter,
  StateRef,
  StoreDeclaration,
  StoreDefinition,
  StoreSlotSchema,
} from "./data_script";
export type { ComputedRef, Ref } from "./ui/widgets";
export {
  defineReaction,
  defineImpactEvent,
  scopeReactions,
  defineEntity,
  defineMod,
  defineFaction,
  sentiment,
  defineMapCatalog,
  defineWeaponPlacement,
  defineTriggerPool,
  defineStore,
  read,
  fromRuntime,
  set,
  update,
  when,
  npcs,
  players,
  wait,
  fire,
  defineTriggerEvent,
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
