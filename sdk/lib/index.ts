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
export { defineReaction, defineEntity, defineStore } from "./data_script";

export type { LocalizedText } from "./ui/text";

export type {
  WidgetColor,
  WidgetSpacing,
  WidgetAlign,
  WidgetEasing,
  ScalarStateValue,
  NumericArrayStateValue,
  ReadonlyStateRef,
  WritableStateRef,
  NumberTween,
  ColorTween,
  TextBindProp,
  PanelBindProp,
  SliderBindProp,
  BarBindProp,
  BarMaxProp,
  LocalBindRef,
  PredicateValue,
  Predicate,
  WidgetRole,
  AnnouncePriority,
  StyleRangeEntry,
  StyleRangesProp,
  BorderProp,
  FocusNeighborsProp,
  RepeatPolicyProp,
  ReactionHandleRef,
  WidgetDescriptor,
  TextProps,
  PanelProps,
  ImageProps,
  SpacerProps,
  ButtonProps,
  SliderProps,
  BarProps,
  AnnounceProps,
} from "./ui/widgets";
export { Text, Panel, Image, Spacer, Button, Slider, Bar, Announce } from "./ui/widgets";

export type {
  FocusKind,
  FocusPolicyProp,
  StackProps,
  GridProps,
} from "./ui/layout";
export { VStack, HStack, Grid } from "./ui/layout";

export type {
  WidgetAnchor,
  WidgetCaptureMode,
  TreeProps,
  AnchoredTreeDescriptor,
} from "./ui/tree";
export { Tree } from "./ui/tree";

export type {
  LocalStateHandle,
  LocalStateBundle,
  StateBindOptionsFor,
} from "./ui/state";
export { bindState, stateEquals, createLocalState, ui, Switch } from "./ui/state";

export type { CrossingCondition, CrossingDescriptor } from "./ui/reactions";
export {
  onStateCrossing,
  playSound,
  rumble,
  flashScreen,
  vignette,
  screenShake,
  showDialog,
  openTextEntry,
  KEYBOARD_TREE,
  openMenu,
  closeDialog,
  updateState,
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
