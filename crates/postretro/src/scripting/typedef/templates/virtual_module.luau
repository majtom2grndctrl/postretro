
-- ---------------------------------------------------------------------------
-- Luau SDK virtual modules. Runtime module tables contain values only; these
-- structural aliases describe the read-only engine-owned singletons returned by
-- literal `require` calls. Root `postretro` excludes UI authoring APIs; `postretro/ui`
-- is typed directly from SDK function signatures instead of bare globals.

export type PostretroUiModule = {
  --- Build a `text` widget descriptor. `props.content` is fallback text; optional `bind` can replace rendered content from state.
  Text: (props: TextProps) -> WidgetDescriptor,
  --- Build a solid panel descriptor. `fill` is required; optional `bind` can replace fill from RGBA state.
  Panel: (props: PanelProps) -> WidgetDescriptor,
  --- Build an image descriptor from a UI texture key. Exactly one of `label` or `decorative = true` is required.
  Image: (props: ImageProps) -> WidgetDescriptor,
  --- Build a spacer descriptor. `flexGrow` is finite and defaults to 1.
  Spacer: (props: SpacerProps?) -> WidgetDescriptor,
  --- Build an interactive button descriptor. `onPress` accepts a reaction handle, bare reaction name, or reserved `ui.*` action.
  Button: (props: ButtonProps) -> WidgetDescriptor,
  --- Build an interactive slider descriptor. `bind` is writable numeric state/local cell; writes clamp to `[min, max]`.
  Slider: (props: SliderProps) -> WidgetDescriptor,
  --- Build a passive bar descriptor. Displayed fill is `value / max` clamped to `[0, 1]`.
  Bar: (props: BarProps) -> WidgetDescriptor,
  --- Build a non-visual live-region announcement. `text` is positional display text.
  Announce: (props: AnnounceProps, text: LocalizedText) -> WidgetDescriptor,
  --- Build a vertical stack. `children` is positional, not a prop.
  VStack: (props: StackProps?, children: {WidgetDescriptor}?) -> WidgetDescriptor,
  --- Build a horizontal stack. `children` is positional, not a prop.
  HStack: (props: StackProps?, children: {WidgetDescriptor}?) -> WidgetDescriptor,
  --- Build a grid. `props.cols` must be an integer >= 1; children flow row-major.
  Grid: (props: GridProps, children: {WidgetDescriptor}?) -> WidgetDescriptor,
  --- Wrap a root widget in an anchored tree placement envelope.
  Tree: (props: TreeProps, root: WidgetDescriptor) -> AnchoredTreeDescriptor,
  --- Build a UI-tree registration object for `ModManifest.uiTrees` or `setupLevel().uiTrees`.
  defineUiTree: (registration: UiTreeRegistrationProps) -> UiTreeRegistration,
  getGameState: typeof(getGameState),
  --- Compose bind-only options onto a state ref; pure and never reads live state.
  bindState: ((ReadonlyStateRef<number>, NumberStateBindOptions?) -> ReadonlyStateRef<number> & NumberStateBindOptions)
    & ((ReadonlyStateRef<{number}>, NumericArrayStateBindOptions?) -> ReadonlyStateRef<{number}> & NumericArrayStateBindOptions)
    & ((ReadonlyStateRef<string>, ScalarStateBindOptions?) -> ReadonlyStateRef<string> & ScalarStateBindOptions)
    & ((ReadonlyStateRef<boolean>, ScalarStateBindOptions?) -> ReadonlyStateRef<boolean> & ScalarStateBindOptions),
  --- Build a scalar equality predicate for visibility, selection, or checked state.
  stateEquals: <T>(ref: ReadonlyStateRef<T>, value: T) -> Predicate,
  --- Declare presentation-local cells. Values may be number, boolean, string, or RGBA tuple.
  createLocalState: (init: { [string]: any }) -> { scope: any, cells: any },
  ui: { createLocalState: (init: { [string]: any }) -> { scope: any, cells: any } },
  --- Expand a string cell into sorted visible subtrees keyed by cell value.
  Switch: (cell: LocalStateHandle<string>, map: { [string]: WidgetDescriptor }) -> {WidgetDescriptor},
  defineTheme: (theme: ThemeDefinition) -> DefinedTheme,
  getDesignTokens: (theme: DefinedTheme) -> DesignTokens,
  --- Build a numeric state-crossing watcher. `condition` gives exactly one finite `below` or `above`; optional `max` is a finite denominator.
  onStateCrossing: (ref: ReadonlyStateRef<number>, condition: CrossingCondition, fire: {NamedReactionDescriptor | string}) -> CrossingDescriptor,
  --- Play `sound` on optional mixer `bus`; nil bus uses the engine default.
  playSound: (sound: string, bus: string?) -> PrimitiveReactionDescriptor,
  --- Trigger gamepad rumble. `strong` and optional `weak` are motor intensities in [0, 1]; `durationMs` is milliseconds.
  rumble: (strong: number, durationMs: number, weak: number?) -> PrimitiveReactionDescriptor,
  --- Flash the screen with linear RGBA `color`; `durationMs` is the decay time in milliseconds.
  flashScreen: (color: {number}, durationMs: number) -> PrimitiveReactionDescriptor,
  --- Apply a screen-edge vignette. `strength` is peak amount, `durationMs` is total rise+decay time, and optional `color` is linear RGB.
  vignette: (strength: number, durationMs: number, color: {number}?) -> PrimitiveReactionDescriptor,
  --- Shake the screen. `amplitude` is logical-reference px, `durationMs` is milliseconds, and optional `frequency` is Hz.
  screenShake: (amplitude: number, durationMs: number, frequency: number?) -> PrimitiveReactionDescriptor,
  --- Push UI tree `tree` as a modal; optional `onCommit` names a reaction fired on commit.
  showDialog: (tree: string, onCommit: string?) -> PrimitiveReactionDescriptor,
  --- Push a menu tree by registry name. Unknown tree names warn and no-op.
  openMenu: (tree: string) -> PrimitiveReactionDescriptor,
  --- Pop the active modal. Empty stack warns and no-ops.
  closeDialog: () -> PrimitiveReactionDescriptor,
  --- Queue a catalog map load by committed `ModMapEntry.id`; unknown ids warn and no-op.
  loadLevel: (id: string) -> PrimitiveReactionDescriptor,
  --- Reload the active level from retained catalog id or raw dev path. No active level means no-op.
  restartLevel: () -> PrimitiveReactionDescriptor,
  --- Return to the frontend menu and reload its optional backdrop level.
  returnToFrontend: () -> PrimitiveReactionDescriptor,
  --- Open the engine keyboard modal. Optional `onCommit` fires when text entry commits.
  openTextEntry: (onCommit: string?) -> PrimitiveReactionDescriptor,
  KEYBOARD_TREE: "keyboard",
  CLOSE_DIALOG_ACTION: "ui.closeDialog",
  EXIT_TO_DESKTOP_ACTION: "ui.exitToDesktop",
  --- Reserved button `onPress` action for returning to the frontend; same lifecycle path as `returnToFrontend()`.
  QUIT_TO_MENU_ACTION: "ui.quitToMenu",
  --- Write `value` to a writable state ref at game-logic time; runtime validates type/range and rejects readonly slots.
  updateState: <T>(ref: WritableStateRef<T>, value: T) -> PrimitiveReactionDescriptor,
  appendText: (ref: WritableStateRef<string>, text: string) -> PrimitiveReactionDescriptor,
  backspaceText: (ref: WritableStateRef<string>) -> PrimitiveReactionDescriptor,
  clearText: (ref: WritableStateRef<string>) -> PrimitiveReactionDescriptor,
}

export type PostretroModule = {
  world: World,
  runtime: Runtime,
  getGameState: typeof(getGameState),
  timeline: typeof(timeline),
  sequence: typeof(sequence),
  --- Build a named reaction descriptor. Accepts `progress`, `primitive`, or `sequence` body data.
  defineReaction: typeof(defineReaction),
  --- Stamp a shared map-tag scope onto a reaction list.
  scopeReactions: typeof(scopeReactions),
  --- Pure identity builder for entity type descriptors returned from `ModManifest.entities`.
  defineEntity: typeof(defineEntity),
  --- Pure identity builder for the mod manifest. `config.name` is required.
  defineMod: typeof(defineMod),
  --- Pure identity builder for map catalog entries with required `id`, `path`, and `name`.
  defineMapCatalog: typeof(defineMapCatalog),
  --- Pure state-store builder returning `{ declaration, state }`.
  defineStore: typeof(defineStore),
  emitter: typeof(emitter),
  smokeEmitter: typeof(smokeEmitter),
  sparkEmitter: typeof(sparkEmitter),
  dustEmitter: typeof(dustEmitter),
}

declare require: ((path: "postretro") -> PostretroModule)
  & ((path: "postretro/ui") -> PostretroUiModule)
  & ((path: string) -> any)
