// QuickJS SDK prelude entry.
//
// This file is bundled by `postretro-script-compiler --prelude` and every named
// export is rewritten to `globalThis.<name> = <name>`. It intentionally exports
// more than the public root `postretro` module while imports are stripped
// without alias rewriting. In particular, UI names remain here only as
// implementation plumbing for existing TypeScript authoring. The public root
// module surface is decided by `sdk/lib/index.ts` and `sdk/types/postretro.d.ts`.
//
// Once the compiler rewrites `postretro/ui` imports to explicit aliases instead
// of relying on globals, remove the UI re-exports from this prelude entry.

export * from "./index";

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
  UiTreeRegistrationProps,
  UiTreeRegistration,
} from "./ui/tree";
export { Tree, defineUiTree } from "./ui/tree";

export type {
  LocalStateHandle,
  LocalStateBundle,
  StateBindOptionsFor,
} from "./ui/state";
export { bindState, stateEquals, createLocalState, ui, Switch } from "./ui/state";

export type {
  ThemeColorValue,
  ThemeDefinition,
  ThemeTokenTree,
  DesignTokens,
  DefinedTheme,
} from "./ui/theme";
export { defineTheme, getDesignTokens } from "./ui/theme";

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
  CLOSE_DIALOG_ACTION,
  EXIT_TO_DESKTOP_ACTION,
  QUIT_TO_MENU_ACTION,
  openMenu,
  closeDialog,
  loadLevel,
  restartLevel,
  returnToFrontend,
  updateState,
  appendText,
  backspaceText,
  clearText,
} from "./ui/reactions";
