
-- ---------------------------------------------------------------------------
-- Luau SDK virtual modules. Runtime module tables contain values only; these
-- structural aliases describe the read-only engine-owned singletons returned by
-- literal `require` calls. Root `postretro` excludes UI and audio authoring APIs;
-- `postretro/audio` holds audio subsystem builders; `postretro/ui`
-- is typed directly from SDK function signatures instead of bare globals.

--- Accessibility toggles a menu button may cycle.
export type AccessibilityToggleField = "reduceMotion" | "flashLimiter" | "monoAudio"

--- Accessibility numeric fields a mod menu button may step, each within its range ([0, 1], or [1, 3] for `holdTimingScale`).
export type AccessibilityNumericField = "screenShakeScale" | "viewFeelScale" | "masterVolume" | "sfxVolume" | "musicVolume" | "uiVolume" | "holdTimingScale"

--- Reserved operations for the engine display-mode picker and confirmation.
export type DisplayModeOperation = "next" | "previous" | "apply" | "keep" | "revert"

export type PostretroUiModule = {
  --- Build a `text` widget descriptor. `props.content` is fallback text; optional `bind` can replace rendered content from state.
  Text: (props: TextProps) -> WidgetDescriptor,
  --- Build a solid panel descriptor. `fill` is required; optional `bind` can replace fill from RGBA state.
  Panel: (props: PanelProps) -> WidgetDescriptor,
  --- Build an image descriptor from a UI texture key. Exactly one of `label` or `decorative = true` is required.
  Image: (props: ImageProps) -> WidgetDescriptor,
  --- Build a spacer descriptor. `flexGrow` is finite and defaults to 1.
  Spacer: (props: SpacerProps?) -> WidgetDescriptor,
  --- Build a glyph for a command on the player's current device; follows rebinding and the confirm/cancel swap.
  Glyph: (props: GlyphProps) -> WidgetDescriptor,
  --- Build an interactive button descriptor. `onPress` accepts a reaction handle, bare reaction name, or reserved `ui.*` action.
  Button: (props: ButtonProps) -> WidgetDescriptor,
  --- Build an interactive slider descriptor. `bind` is writable numeric state/local cell; writes clamp to `[min, max]`.
  Slider: (props: SliderProps) -> WidgetDescriptor,
  --- Build a passive bar descriptor. Displayed fill is `value / max` clamped to `[0, 1]`.
  Bar: (props: BarProps) -> WidgetDescriptor,
  --- Build a passive annulus or angular arc descriptor.
  Ring: (props: RingProps) -> WidgetDescriptor,
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
  --- Build an explicit-id passive presentation template. It is manifest data, not a modal UI tree.
  definePresentationTemplate: (id: string, props: PresentationTemplateProps) -> PresentationTemplate,
  --- Bind producer-stamped per-instance values inside a presentation template.
  fact: {
    number: (name: string, options: { format: string?, decimalPlaces: number?, tween: NumberTween? }?) -> FactBindRef & { format: string?, decimalPlaces: number?, tween: NumberTween? },
    text: (name: string, options: { format: string? }?) -> FactBindRef & { format: string? },
    bool: (name: string, options: { format: string? }?) -> FactBindRef & { format: string? },
  },
  --- Build a damaged-enemy source selected by the host in co-op or locally in single player.
  damagedEnemies: (props: DamagedEnemiesProps) -> DamagedEnemiesSource,
  --- Bind an overlay source to a socket-anchored presentation template.
  defineOverlay: (props: { over: DamagedEnemiesSource, template: PresentationTemplate, maxVisible: number }) -> PresentationOverlay,
  --- Add a passive visual effect to an impact policy `do` array.
  present: (template: PresentationTemplate, value: NumberValue) -> Effect,
  getGameState: typeof(getGameState),
  --- Compose bind-only options onto a state ref; pure and never reads live state.
  bindState: ((ComputedRef<number>, NumberStateBindOptions?) -> ComputedRef<number> & NumberStateBindOptions)
    & ((ComputedRef<{number}>, NumericArrayStateBindOptions?) -> ComputedRef<{number}> & NumericArrayStateBindOptions)
    & ((ComputedRef<string>, ScalarStateBindOptions?) -> ComputedRef<string> & ScalarStateBindOptions)
    & ((ComputedRef<boolean>, ScalarStateBindOptions?) -> ComputedRef<boolean> & ScalarStateBindOptions),
  --- Build a scalar equality predicate for visibility, selection, or checked state.
  stateEquals: <T>(ref: ComputedRef<T>, value: T) -> Predicate,
  --- Declare presentation-local cells. Values may be number, boolean, string, or RGBA tuple.
  createLocalState: (init: { [string]: any }) -> { scope: any, cells: any },
  ui: { createLocalState: (init: { [string]: any }) -> { scope: any, cells: any } },
  --- Expand a string cell into sorted visible subtrees keyed by cell value.
  Switch: (cell: LocalStateHandle<string>, map: { [string]: WidgetDescriptor }) -> {WidgetDescriptor},
  defineTheme: (theme: ThemeDefinition) -> DefinedTheme,
  getDesignTokens: (theme: DefinedTheme) -> DesignTokens,
  --- Build a state-crossing watcher: either a numeric threshold or a Bool predicate over live store slots, a fluent value such as `read(player.swimming)` or a raw `runtime.*` node. A predicate already true at registration only arms; it must return false, then true, to fire.
  onStateCrossing: ((ref: ComputedRef<number>, condition: CrossingCondition, fire: {Reaction<any> | string}) -> CrossingDescriptor)
    & ((predicate: RuntimeValue | BoolRef, fire: {Reaction<any> | string}, options: CrossingOptions?) -> CrossingDescriptor),
  --- Play `sound`. `options.bus` routes to a mixer bus (SFX when omitted); `options.at = on.emitter` positions it where the named gameplay event happened, on the SFX bus.
  playSound: (sound: string, options: PlaySoundOptions?) -> PrimitiveReactionDescriptor,
  --- Trigger gamepad rumble. `strong` and optional `weak` are motor intensities in [0, 1]; `durationMs` is milliseconds.
  rumble: (strong: number, durationMs: number, weak: number?, options: PresentationOptions?) -> PrimitiveReactionDescriptor,
  --- Flash the screen with linear RGBA `color`; `durationMs` is the decay time in milliseconds.
  flashScreen: (color: {number}, durationMs: number, options: PresentationOptions?) -> PrimitiveReactionDescriptor,
  --- Apply a screen-edge vignette. `strength` is peak amount, `durationMs` is total rise+decay time, and optional `color` is linear RGB.
  vignette: (strength: number, durationMs: number, color: {number}?, options: PresentationOptions?) -> PrimitiveReactionDescriptor,
  --- Shake the screen. `amplitude` is logical-reference px, `durationMs` is milliseconds, and optional `frequency` is Hz.
  screenShake: (amplitude: number, durationMs: number, frequency: number?, options: PresentationOptions?) -> PrimitiveReactionDescriptor,
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
  --- Set the live directional sentiment from `from` toward `to` at frame end. Negative values degrade the relationship; positive values bond it.
  setSentiment: (from: string, to: string, value: number) -> PrimitiveReactionDescriptor,
  --- Add `delta` to the current live directional sentiment from `from` toward `to` at frame end. Negative deltas degrade the relationship; positive deltas bond it.
  adjustSentiment: (from: string, to: string, delta: number) -> PrimitiveReactionDescriptor,
  --- Open the engine keyboard modal. Optional `onCommit` fires when text entry commits.
  openTextEntry: (onCommit: string?) -> PrimitiveReactionDescriptor,
  KEYBOARD_TREE: "keyboard",
  CLOSE_DIALOG_ACTION: "ui.closeDialog",
  EXIT_TO_DESKTOP_ACTION: "ui.exitToDesktop",
  --- Reserved button `onPress` action for returning to the frontend; same lifecycle path as `returnToFrontend()`.
  QUIT_TO_MENU_ACTION: "ui.quitToMenu",
  --- Reserved button `onPress` action that opens the engine accessibility panel.
  OPEN_ACCESSIBILITY_ACTION: "ui.openAccessibility",
  --- Reserved button `onPress` action that opens the engine controls panel.
  OPEN_CONTROLS_ACTION: "ui.openControls",
  --- Build a reserved `ui.displayMode.<op>` action. `next`/`previous` browse without saving;
  --- `apply` commits the selection (exclusive opens the engine confirmation).
  displayModeAction: (op: DisplayModeOperation) -> string,
  --- The reserved `onPress` action for one accessibility field. Toggles `cycle`; numeric fields `increase` or `decrease`.
  accessibilityAction: ((field: AccessibilityToggleField, op: "cycle") -> string) & ((field: AccessibilityNumericField, op: "increase" | "decrease") -> string),
  --- Write a literal, fluent or runtime value at game-logic time. Fluent values (`byPlayer(on.player)` reads included) and runtime values may read known Number/Boolean slots, including readonly slots; unknown/nonprojectable inputs and readonly output targets reject.
  updateState: <T>(ref: Ref<T>, value: T | RuntimeValue | NumberRef | BoolRef) -> PrimitiveReactionDescriptor,
  appendText: (ref: Ref<string>, text: string) -> PrimitiveReactionDescriptor,
  backspaceText: (ref: Ref<string>) -> PrimitiveReactionDescriptor,
  clearText: (ref: Ref<string>) -> PrimitiveReactionDescriptor,
}

--- Ghost bus fields `setGhostAudio` may set. Volume and drive are linear 0..=1; cutoffs are 20..=20000 Hz with the high-pass below the low-pass; `periodMs` (finite, >= 0, default 0) is the ramp length.
export type GhostAudioFields = { volume: number?, highPassHz: number?, lowPassHz: number?, drive: number?, periodMs: number? }

export type PostretroAudioModule = {
  --- Change the mod-wide Ghost bus mix during play; a primitive reaction body (no target). Applies on the host at its next tick and replicates; does nothing on a connected client. Each named field ramps on its own over `periodMs`; omitted fields keep their value and ramp. Out-of-range, non-finite, empty, or crossing-cutoff calls warn and drop.
  setGhostAudio: (fields: GhostAudioFields) -> PrimitiveReactionDescriptor,
}

export type PostretroModule = {
  runtime: Runtime,
  --- Pure bounded weapon-action data builders with read-only normalized charge.
  activation: Activation,
  --- Map-placed members of one kind (`mover`, `trigger`, `light`, `fog`, `emitter`, `spawner`, `trap`), as handles.
  --- Callable only inside a level's data script (module evaluation or `setupLevel`).
  getMapEntities: typeof(getMapEntities),
  --- Level default gravity in m/s² (negative = downward): the gravity outside every gravity volume. Inside a `gravity_volume` the volume's own value holds; this still returns the level default.
  getGravity: typeof(getGravity),
  --- Set the level default gravity in m/s²: the gravity outside every gravity volume. A `gravity_volume`'s value holds inside it regardless. Non-finite values are ignored with a warning.
  setGravity: typeof(setGravity),
  getGameState: typeof(getGameState),
  timeline: typeof(timeline),
  sequence: typeof(sequence),
  --- Build a named reaction descriptor. Accepts `progress`, `primitive`, or `sequence` body data.
  defineReaction: typeof(defineReaction),
  --- Define a pure impact-policy descriptor. Register it only through `events`.
  defineImpactEvent: typeof(defineImpactEvent),
  --- Address NPCs, optionally by tag. An opaque group resolved when each command takes effect.
  npcs: typeof(npcs),
  --- Address every seat-bound player pawn. An opaque group resolved when each command takes effect.
  players: typeof(players),
  --- Player-event edge word: fire when a Bool condition turns true for a player.
  becomes: typeof(becomes),
  --- Player-event edge word: fire when a Bool condition turns false for a player.
  ceases: typeof(ceases),
  wait: typeof(wait),
  fire: typeof(fire),
  --- Declare a mod-global, tag-keyed trigger event for `ModManifest.triggerEvents`.
  defineTriggerEvent: typeof(defineTriggerEvent),
  --- Stamp a shared map-tag scope onto a reaction list.
  scopeReactions: typeof(scopeReactions),
  --- Pure identity builder for entity type descriptors returned from `ModManifest.entities`.
  defineEntity: typeof(defineEntity),
  --- Pure identity builder for the mod manifest. `config.name`, `config.id`, and `config.version` are required.
  defineMod: typeof(defineMod),
  --- Pure builder for a stable named faction declaration. Names beginning with `@postretro.` are engine-reserved.
  defineFaction: typeof(defineFaction),
  --- Build one directed faction relationship for a mod manifest.
  sentiment: typeof(sentiment),
  --- Pure identity builder for a fluid declaration in a mod manifest's `fluids`.
  defineFluid: typeof(defineFluid),
  --- Pure identity builder for a ghost style in a mod manifest's `ghostStyles`.
  defineGhostStyle: typeof(defineGhostStyle),
  --- Pure identity builder for map catalog entries with required `id`, `path`, and `name`.
  defineMapCatalog: typeof(defineMapCatalog),
  --- Pure identity builder for reusable first-person weapon placement data.
  defineWeaponPlacement: typeof(defineWeaponPlacement),
  --- Pure identity builder for trigger-pool declarations returned from level or mod manifests.
  defineTriggerPool: typeof(defineTriggerPool),
  --- Pure state-store builder returning a flattened, frozen schema-keyed handle.
  defineStore: typeof(defineStore),
  --- Read a state ref into an expression value.
  read: typeof(read),
  --- Lift raw `runtime.*` expressions into public NumberRef/BoolRef values.
  fromRuntime: typeof(fromRuntime),
  --- Build an absolute writable-number store-slot write.
  set: typeof(set),
  --- Build a writable-number store-slot read-modify-write; `cur` is read during plan phase before effects apply. Owner-addressed reads resolve the live per-seat map, not a frozen snapshot.
  update: typeof(update),
  --- Gate impact effects with a Bool expression.
  when: typeof(when),
  --- Pre-wrapped behavior-graph guard inputs for the fixed `@brain.*` namespace.
  brain: BrainInputs,
  --- Pre-wrapped candidate-filter inputs for the fixed `@candidate.*` namespace.
  candidate: CandidateInputs,
  --- Build a `@state.<name>` guard input leaf for a per-entity state field.
  state: typeof(state),
  emitter: typeof(emitter),
  smokeEmitter: typeof(smokeEmitter),
  sparkEmitter: typeof(sparkEmitter),
  dustEmitter: typeof(dustEmitter),
}

declare require: ((path: "postretro") -> PostretroModule)
  & ((path: "postretro/ui") -> PostretroUiModule)
  & ((path: "postretro/audio") -> PostretroAudioModule)
  & ((path: string) -> any)
