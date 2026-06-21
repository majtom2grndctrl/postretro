
-- ---------------------------------------------------------------------------
-- SDK library — embedded into every Luau context via `include_str!` and
-- evaluated during state construction. `world.luau`'s return value becomes
-- global `world`; `util/keyframes.luau` supplies `timeline` and `sequence`.
-- Animation curve construction lives on entity handles
-- (`LightEntityHandle`, `FogVolumeHandle`) as capability methods, not as
-- bare globals.

--- Capability for entities with a scalar animation channel. `Channel` is
--- type-level documentation only; the handle's implementation knows which
--- channel to drive. Composed by `LightEntityHandle` (brightness) and
--- `FogVolumeHandle` (density).
export type AnimatableScalar<Channel> = {
  pulse: (self: any, opts: { min: number, max: number, periodMs: number }) -> {SequenceStep},
  fade: (self: any, opts: { from: number, to: number, periodMs: number }) -> {SequenceStep},
  flicker: (self: any, opts: { min: number, max: number, rate: number }) -> {SequenceStep},
}

--- Capability for entities with a vec3 animation channel.
export type AnimatableVec3<Channel> = {
  cycle: (self: any, opts: { values: {Vec3}, periodMs: number }) -> {SequenceStep},
}

--- Typed light handle returned by `world:query({ component = "light" })`.
--- Composes the brightness scalar capability with vec3 channels declared
--- directly (Luau lacks TS-style multiple-interface extension; secondary
--- channels are inlined).
export type LightEntityHandle = {
  id: EntityId,
  position: Vec3,
  isDynamic: boolean,
  tags: {string},
  component: LightComponent,

  pulse: (self: LightEntityHandle, opts: { min: number, max: number, periodMs: number }) -> {SetLightAnimationStep},
  fade: (self: LightEntityHandle, opts: { from: number, to: number, periodMs: number }) -> {SetLightAnimationStep},
  flicker: (self: LightEntityHandle, opts: { min: number, max: number, rate: number }) -> {SetLightAnimationStep},
  colorShift: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
  sweep: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
}

--- Generic entity handle returned by `world:query` when the component is
--- not "light", "emitter", or "fog_volume". Carries only id, position, and tags.
export type EntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
}

--- Typed fog-volume handle returned by `world:query({ component = "fog_volume" })`.
--- Composes the density scalar capability with secondary saturation
--- methods declared directly.
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,

  pulse: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fade: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
  flicker: (self: FogVolumeHandle, opts: { min: number, max: number, rate: number }) -> {SetFogAnimationStep},
  pulseSaturation: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fadeSaturation: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
}

--- `world` vocabulary global. Wraps `worldQuery` with a typed handle.
--- `"light"` returns `LightEntityHandle` values (with capability methods);
--- `"emitter"` returns `EmitterEntity` values carrying the full
--- `BillboardEmitterComponent` snapshot under `component`; `"fog_volume"`
--- returns `FogVolumeHandle` values; other components fall back to the
--- bare `EntityHandle` shape.
export type World = {
  query: ((self: World, filter: { component: "light", tag: string? }) -> {LightEntityHandle})
       & ((self: World, filter: { component: "emitter", tag: string? }) -> {EmitterEntity})
       & ((self: World, filter: { component: "fog_volume", tag: string? }) -> {FogVolumeHandle})
       & ((self: World, filter: WorldQueryFilter) -> {EntityHandle}),
  --- Current world gravity in m/s² (negative = downward; positive = upward).
  --- Seeded from the worldspawn `initialGravity` KVP at level load and
  --- persists until the next level load or `setGravity` call.
  getGravity: (self: World) -> number,
  --- Set world gravity in m/s² (negative = downward; positive = upward).
  --- NaN and non-finite values are silently ignored with a warning logged.
  --- Effect is immediate and persists until the next level load or another
  --- `setGravity` call.
  setGravity: (self: World, value: number) -> (),
}

--- Per-channel keyframe accepted by `timeline` / `sequence`.
export type Keyframe = {number}

declare world: World

--- Validate `{absolute_ms, ...value}` keyframes; pass-through on success.
declare function timeline(keyframes: {Keyframe}): {Keyframe}

--- Convert `{delta_ms, ...value}` keyframes to absolute-time form.
declare function sequence(keyframes: {Keyframe}): {Keyframe}

-- ---------------------------------------------------------------------------
-- Emitter component vocabulary — pure constructors from
-- `sdk/lib/entities/emitters.luau`, lifted to bare globals by the Luau prelude.

--- Input shape for `emitter()`. Required: `lifetime`, `velocity`, and `sprite`.
--- Other fields fall back to the documented emitter defaults.
export type EmitterProps = {
  rate: number?,
  burst: number?,
  spread: number?,
  lifetime: number,
  velocity: {number},
  buoyancy: number?,
  drag: number?,
  size_over_lifetime: {number}?,
  opacity_over_lifetime: {number}?,
  color: {number}?,
  sprite: string,
  spin_rate: number?,
}

--- Optional override shape accepted by emitter presets.
export type EmitterOverrides = {
  rate: number?,
  burst: number?,
  spread: number?,
  lifetime: number?,
  velocity: {number}?,
  buoyancy: number?,
  drag: number?,
  size_over_lifetime: {number}?,
  opacity_over_lifetime: {number}?,
  color: {number}?,
  sprite: string?,
  spin_rate: number?,
}

--- Build a `billboard_emitter` component descriptor from props.
declare function emitter(props: EmitterProps): ComponentValue

--- Soft, slowly-rising smoke emitter preset.
declare function smokeEmitter(overrides: EmitterOverrides?): ComponentValue

--- Fast, falling, tumbling sparks emitter preset.
declare function sparkEmitter(overrides: EmitterOverrides?): ComponentValue

--- Slow drifting dust motes emitter preset.
declare function dustEmitter(overrides: EmitterOverrides?): ComponentValue

-- ---------------------------------------------------------------------------
-- Data script vocabulary — pure descriptor builders consumed by the engine
-- when `setupLevel` returns. See: context/lib/scripting.md §2.

--- Progress-subscription reaction body: fires `fire` when entities tagged
--- `tag` cross kill ratio `at` (0.0–1.0).
export type ProgressReactionDescriptor = {
  progress: { tag: string, at: number, fire: string },
}

--- Primitive reaction body: invokes the named Rust primitive. With `tag`, it
--- targets entities carrying that tag and mutates them. Without `tag`, it is a
--- system reaction (no entities) that enqueues a typed engine command —
--- `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args`
--- carries the primitive's typed payload (e.g. `{ rate = 0 }` for
--- `setEmitterRate`, `{ sound = "alarm" }` for `playSound`).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- One step in a `sequence` reaction body: invokes the named sequenced
--- primitive against the given entity with `args`. Sequence steps target a
--- single `EntityId`; tag-targeted primitives belong on the `Primitive`
--- reaction path.
export type SetLightAnimationStep = {
  id: EntityId,
  primitive: "setLightAnimation",
  args: LightAnimation,
}

--- Sequence step targeting a single fog volume's `density`. Use directly
--- for a one-shot density change.
export type SetFogDensityStep = {
  id: EntityId,
  primitive: "setFogDensity",
  args: { density: number },
}

--- Sequence step targeting a single fog volume's `glow`.
export type SetFogGlowStep = {
  id: EntityId,
  primitive: "setFogGlow",
  args: { glow: number },
}

--- Sequence step targeting a single fog volume's `edgeSoftness`.
export type SetFogEdgeSoftnessStep = {
  id: EntityId,
  primitive: "setFogEdgeSoftness",
  args: { edgeSoftness: number },
}

--- Sequence step targeting a single fog volume's `falloff`.
export type SetFogFalloffStep = {
  id: EntityId,
  primitive: "setFogFalloff",
  args: { falloff: number },
}

--- Sequence step that updates any subset of
--- `{density, glow, edgeSoftness, falloff, tint, saturation, minBrightness, lightRange}` on a single
--- fog volume in one component write.
export type SetFogParamsStep = {
  id: EntityId,
  primitive: "setFogParams",
  args: { density: number?, glow: number?, edgeSoftness: number?, falloff: number?, tint: {number}?, saturation: number?, minBrightness: number?, lightRange: number? },
}

--- Sequence step that installs (or clears, when `args` is `nil`) a
--- dual-channel animation (density and/or saturation) on a single fog volume.
--- Emitted by the `FogVolumeHandle` capability methods (`pulse`, `fade`,
--- `flicker`, `pulseSaturation`, `fadeSaturation`).
export type SetFogAnimationStep = {
  id: EntityId,
  primitive: "setFogAnimation",
  args: FogAnimation?,
}

--- Union of every supported sequence step shape. New sequenced primitives
--- extend this union.
export type SequenceStep = SetLightAnimationStep | SetFogDensityStep | SetFogGlowStep | SetFogEdgeSoftnessStep | SetFogFalloffStep | SetFogParamsStep | SetFogAnimationStep

--- Sequence reaction body: ordered per-entity primitive invocations. Steps
--- run in array order at dispatch.
export type SequenceReactionDescriptor = {
  sequence: {SequenceStep},
}

--- Descriptor produced by `defineReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer reads
--- both fields from one flat table.
export type ProgressNamedReactionDescriptor = { name: string, levels: {string}?, progress: { tag: string, at: number, fire: string } }
export type PrimitiveNamedReactionDescriptor = { name: string, levels: {string}?, primitive: string, tag: string?, args: { [string]: any }?, onComplete: string? }
export type SequenceNamedReactionDescriptor = { name: string, levels: {string}?, sequence: {SequenceStep} }
export type NamedReactionDescriptor = ProgressNamedReactionDescriptor | PrimitiveNamedReactionDescriptor | SequenceNamedReactionDescriptor

--- Crossing condition: fires when the watched slot crosses the threshold in
--- one direction. Exactly one of `below`/`above` is given. `max` is the
--- denominator the threshold is a fraction of; omit it for a raw-value
--- comparison (`max` defaults to `1.0`).
export type CrossingCondition =
  { below: number, above: nil?, max: number? }
  | { below: nil?, above: number, max: number? }

--- A state-crossing watcher entry as it appears in `setupLevel().crossings` or
--- `ModManifest.crossings`. The condition fields are flattened in beside `slot`
--- and `fire`; `fire` lists the named reactions dispatched when the crossing
--- occurs. `levels` scopes mod-global crossings by map-catalog tags; omit it
--- for every level.
export type CrossingDescriptor = { slot: string, below: number?, above: number?, max: number?, fire: {string}, levels: {string}? }

--- Bundle returned from `setupLevel`. The engine deserializes
--- this shape in one pass at level load.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
  crossings: {CrossingDescriptor}?,
  --- Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same shape
  --- as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed
  --- entries are logged and skipped.
  uiTrees: {ModUiTree}?,
}

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
--- `descriptor` accepts exactly one body shape: `progress` (kill-ratio trigger),
--- `primitive` (named Rust primitive with optional entity `tag` and typed
--- `args`), or `sequence` (ordered per-entity steps). `name` is optional; when
--- omitted a deterministic, run-stable id is derived from the body. Use explicit
--- names when TS and Luau scripts must agree. The returned handle can be passed
--- to button `onPress` or crossing `fire` entries.
declare defineReaction: (
  ((descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor) -> NamedReactionDescriptor)
  & ((name: string, descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor) -> NamedReactionDescriptor)
)

--- Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are
--- matched against `ModMapEntry.tags`; omit scoping for every level.
declare function scopeReactions(tags: {string}, list: {NamedReactionDescriptor}): {NamedReactionDescriptor}

--- Pure identity builder for entity-type descriptors returned from
--- `ModManifest.entities`. `descriptor` is the full archetype table: optional
--- `canonicalName`, optional `defaultWeapon`, and optional component presets.
declare function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor
--- Pure identity builder for the mod manifest consumed from the chunk return.
--- `config.name` is required; optional arrays include `entities`, `maps`,
--- `uiTrees`, `reactions`, `crossings`, and `stores`.
declare function defineMod(config: ModManifest): ModManifest
--- Pure identity builder for a mod map catalog. Entries require `id`, `path`,
--- and `name`; optional `tags` default to empty and drive filtering plus
--- `levels` selectors.
declare function defineMapCatalog(entries: {ModMapEntry}): {ModMapEntry}

--- Build a state-crossing watcher. Pure: returns a plain table, no FFI. Place
--- the result in `setupLevel().crossings` or `ModManifest.crossings` with
--- optional `levels` scoping. On a crossing in the condition's direction the
--- engine fires every reaction in `fire` exactly once, re-arming only after a
--- crossing back; a registration against a non-Number slot warns and is skipped
--- at load. Each `fire` entry is a
--- `defineReaction` handle (typed) or a bare reaction-name string (the shipped
--- path); handles are reduced to their `.name`, so the wire `CrossingDescriptor.fire`
--- stays a `{string}`.
declare function onStateCrossing(
  ref: ReadonlyStateRef<number>,
  condition: CrossingCondition,
  fire: {NamedReactionDescriptor | string}
): CrossingDescriptor

--- System-reaction body: play `sound` through the M12 audio module on the
--- optional named `bus` (omitted when nil -> engine default bus). Pure:
--- returns a `PrimitiveReactionDescriptor`, no FFI. Pass to
--- `defineReaction("name", playSound(...))`.
declare function playSound(sound: string, bus: string?): PrimitiveReactionDescriptor

--- System-reaction body: drive gilrs gamepad force feedback. `strong` and the
--- optional `weak` (omitted when nil) are 0-1 motor intensities; `durationMs`
--- is the rumble length. Warn-once no-op without force-feedback hardware.
--- Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function rumble(strong: number, durationMs: number, weak: number?): PrimitiveReactionDescriptor

--- System-reaction body: flash the screen by writing the engine-owned
--- `screen.flash` RGBA slot, which decays to transparent. `color` is
--- `{r, g, b, a}` (0-1); `durationMs` is the decay time. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function flashScreen(color: {number}, durationMs: number): PrimitiveReactionDescriptor

--- System-reaction body: darken (or tint) the screen edges by writing the
--- engine-owned `screen.vignette` slot, which rises to peak then decays to rest.
--- `strength` is the peak edge-darken amount; `durationMs` is the total
--- rise-plus-decay time. Optional `color` is an `{r, g, b}` linear-RGB tint
--- (omitted when nil -> black, a pure strength-only edge-darken). Pure: returns
--- a `PrimitiveReactionDescriptor`, no FFI.
declare function vignette(strength: number, durationMs: number, color: {number}?): PrimitiveReactionDescriptor

--- System-reaction body: shake the screen by writing the engine-owned
--- `screen.shake` offset slot, a decaying oscillation that fades to rest.
--- `amplitude` is the peak displacement in logical-reference px; `durationMs`
--- is the total decay time. Optional `frequency` is the oscillation rate in Hz
--- (omitted when nil -> the engine applies its default frequency). Pure: returns
--- a `PrimitiveReactionDescriptor`, no FFI.
declare function screenShake(amplitude: number, durationMs: number, frequency: number?): PrimitiveReactionDescriptor

--- System-reaction body: push the dialog UI tree `tree` onto the modal stack,
--- with an optional `onCommit` reaction (omitted when nil). An unknown tree name
--- warns and no-ops at dispatch time. Pure: returns a `PrimitiveReactionDescriptor`,
--- no FFI.
declare function showDialog(tree: string, onCommit: string?): PrimitiveReactionDescriptor

--- The engine-shipped on-screen keyboard's registry name (M13 Text Entry).
--- `openTextEntry` opens this tree; the engine loads its descriptor from
--- `content/base/ui/keyboard.json` at boot. Edits the `ui.textEntry` slot.
declare KEYBOARD_TREE: "keyboard"

--- Reserved button `onPress` action that closes the active modal. The App
--- intercepts this exact wire value before named-reaction dispatch.
declare CLOSE_DIALOG_ACTION: "ui.closeDialog"

--- Reserved button `onPress` action that requests a clean app shutdown. The App
--- intercepts this exact wire value before named-reaction dispatch.
declare EXIT_TO_DESKTOP_ACTION: "ui.exitToDesktop"

--- Reserved button `onPress` action that returns to the frontend menu. The App
--- intercepts this exact wire value before named-reaction dispatch. Same
--- lifecycle path as `returnToFrontend()`: unload gameplay, show the declared
--- frontend menu, and reload its optional backdrop level.
declare QUIT_TO_MENU_ACTION: "ui.quitToMenu"

--- System-reaction body (M13 Text Entry): open the engine-shipped on-screen
--- keyboard, a capturing modal that edits the `ui.textEntry` slot. Optional
--- `onCommit` (omitted when nil) names a reaction fired on commit (the on-screen
--- `done` key or hardware Enter); `nav.cancel` closes without firing it. The same
--- `ui.textEntry` slot also receives the hardware-keyboard path's edits. Wraps
--- `showDialog("keyboard", onCommit)`. Pure: returns a `PrimitiveReactionDescriptor`.
declare function openTextEntry(onCommit: string?): PrimitiveReactionDescriptor

--- System-reaction body: push the menu UI tree `tree` onto the modal stack. A
--- v1 alias of `showDialog` (identical push behavior) without `onCommit`.
--- An unknown tree name warns and no-ops at dispatch time. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function openMenu(tree: string): PrimitiveReactionDescriptor

--- System-reaction body: pop the top UI tree off the modal stack. An empty stack
--- warns and no-ops at dispatch time. Pure: returns a `PrimitiveReactionDescriptor`,
--- no FFI.
declare function closeDialog(): PrimitiveReactionDescriptor

--- System-reaction body: load a map by catalog id. `id` must match a committed
--- `ModMapEntry.id`; unknown ids warn and no-op at dispatch time. The engine
--- queues the lifecycle load when the reaction fires. Pure: returns a
--- PrimitiveReactionDescriptor.
declare function loadLevel(id: string): PrimitiveReactionDescriptor

--- System-reaction body: reload the currently-active map source. Uses the
--- retained catalog id for catalog loads or the raw dev path for direct path
--- loads. Runtime no-ops when no level is active. Pure: returns a
--- PrimitiveReactionDescriptor.
declare function restartLevel(): PrimitiveReactionDescriptor

--- System-reaction body: return to the frontend menu, including its optional
--- declared backdrop level. Use from authored reactions; use
--- `QUIT_TO_MENU_ACTION` for direct button `onPress` values that should take
--- the same lifecycle path. Pure: returns a PrimitiveReactionDescriptor.
declare function returnToFrontend(): PrimitiveReactionDescriptor

--- System-reaction body (M13 Goal F): write `value` to a writable state ref at
--- the game-logic stage. Emits the existing `setState` wire descriptor.
--- Readonly-gated at runtime -- a readonly slot warns and stays unchanged; an
--- engine-owned writable slot is valid. `value` is coerced to the slot's
--- declared type. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function updateState<T>(ref: WritableStateRef<T>, value: T): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): append `text` to the current string
--- value of the writable String slot `slot` at the game-logic stage. Readonly-
--- gated through the same writable-slot gate as `setState` -- a readonly slot
--- warns and stays unchanged; an engine-owned writable slot (e.g. `ui.textEntry`)
--- is valid. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function appendText(ref: WritableStateRef<string>, text: string): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): remove the last character -- one
--- Unicode scalar value (char-pop floor: never splits a UTF-8 sequence, but does
--- not segment grapheme clusters) -- from the writable String slot `slot` at the
--- game-logic stage. Empty is a no-op with no warning.
--- Readonly-gated like `setState`. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function backspaceText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): empty the writable String slot `slot`
--- at the game-logic stage. Readonly-gated like `setState`. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function clearText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor

-- ---------------------------------------------------------------------------
-- State-store declarations. `defineStore` is special-cased in the typedef
-- generator (mirroring `worldQuery`): per-slot value types live only in the
-- runtime `schema` argument, absent at typedef emission. Luau cannot map schema
-- keys through conditional types, so the returned `state` map preserves readable
-- vs writable capability as a union instead of treating every slot as writable.

export type ScalarStateValue = number | boolean | string
export type NumericArrayStateValue = {number}
export type ReadonlyStateRef<T> = { slot: string, __stateRefValueBrand: T? }
export type WritableStateRef<T> = ReadonlyStateRef<T> & { __writableStateRefBrand: T }

--- One slot inside a `defineStore` schema. Every slot needs `default`.
--- `type = "number"` accepts a finite numeric default plus optional inclusive
--- `range = {min, max}`; `"boolean"` and `"string"` require matching defaults;
--- `"enum"` requires non-empty `values` and a default in that list; `"array"` is
--- a finite-number array. `persist` saves on clean exit; `readonly` blocks
--- script writes.
export type StoreSlotSchema = { type: string, readonly: boolean?, [string]: any }

--- Plain declaration data returned through `ModManifest.stores`.
export type StoreDeclaration = { namespace: string, schema: { [string]: StoreSlotSchema } }
export type StoreStateRef<T> = ReadonlyStateRef<T> | WritableStateRef<T>

--- Result of a pure `defineStore` call. Return `declaration` from
--- `ModManifest.stores`; use `state` references in descriptors.
export type StoreDefinition = {
  declaration: StoreDeclaration,
  state: { [string]: StoreStateRef<any> },
}

--- Build a state-store declaration. Pure: calling it performs no FFI and changes
--- no engine state. `namespace` prefixes returned refs as `namespace.slotName`;
--- `schema` declares slot names and validation rules. Returned declarations
--- commit atomically only after the mod manifest succeeds.
declare function defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition

-- ---------------------------------------------------------------------------
-- UI theme helpers. `defineTheme` accepts nested singular token groups and
-- returns the manifest-compatible flat token maps consumed by Rust.

--- Linear RGBA color token value. Components are in display-linear 0-1 space;
--- alpha is the fourth element.
export type ThemeColorValue = {number}
export type ThemeTokenTree<Leaf> = { [string]: Leaf | ThemeTokenTree<Leaf> }
--- Nested singular token groups accepted by `defineTheme`.
export type ThemeDefinition = {
  color: ThemeTokenTree<ThemeColorValue>?,
  font: ThemeTokenTree<string>?,
  spacing: ThemeTokenTree<number>?,
}
--- Runtime-authenticated SDK token records. These Luau aliases are structural
--- for type checking only; widget/layout factories accept only records produced
--- by `getDesignTokens(theme)`.
export type ColorToken = { __postretroToken: "color", token: string }
export type FontToken = { __postretroToken: "font", token: string }
export type SpacingToken = { __postretroToken: "spacing", token: string }
--- Luau cannot express the exact nested token shape derived from a concrete
--- `defineTheme` argument. This open tree preserves token category safety while
--- runtime `getDesignTokens` remains authoritative for which paths exist.
export type DesignTokenTree<Token> = Token & { [string]: DesignTokenTree<Token> }
export type DesignTokens = {
  color: DesignTokenTree<ColorToken>,
  font: DesignTokenTree<FontToken>,
  spacing: DesignTokenTree<SpacingToken>,
}
--- Manifest-compatible flat theme maps returned from `defineTheme`.
export type DefinedTheme = {
  colors: { [string]: ThemeColorValue },
  fonts: { [string]: string },
  spacing: { [string]: number },
}
declare function defineTheme(theme: ThemeDefinition): DefinedTheme
declare function getDesignTokens(theme: DefinedTheme): DesignTokens

-- ---------------------------------------------------------------------------
-- Interactive UI widget descriptors (M13 Goal F, Task 4). Authored as data in a
-- UI tree descriptor; the engine builds the retained tree from them. These
-- type-only aliases pin the wire shape (camelCase, internally tagged on `kind`).

--- The type of every user-facing text string a widget displays. A single alias
--- (`= string` today) so a future localization scheme -- message keys, ICU
--- handles -- is one edit, not a sweep across every text prop.
export type LocalizedText = string

--- A widget color slot: either an inline linear-RGBA tuple `{r, g, b, a}` or a
--- color token from `getDesignTokens(theme)`.
export type WidgetColor = {number} | ColorToken

--- A numeric state bind descriptor shared by low-level `slider`/`bar` wire
--- shapes: a dotted slot name plus an optional number tween. Most authors should
--- call `bindState(ref, options)` instead of constructing this manually.
export type SliderBind = { slot: string, tween: { durationMs: number, easing: string, from: number? }? }

--- Continuous value-to-style map. Text and panel widgets normalize their rendered
--- numeric value by `max`; bars evaluate their displayed fill fraction, so health
--- bands usually use `max = 1.0`. Entries are checked in order; the first `upTo`
--- threshold that contains the normalized value wins, and a trailing entry
--- without `upTo` is the default band.
export type WidgetStyleRanges = { max: number, entries: { upTo: number?, color: WidgetColor?, pulse: { periodMs: number }?, flash: { durationMs: number }? } }

-- ---------------------------------------------------------------------------
-- UI widget / layout / tree / state factories (M13 G1a). Pure builders are
-- available through `require("postretro/ui")`: each returns the camelCase wire
-- descriptor of the matching `render/ui/descriptor.rs` variant and errors with a
-- field-named message on invalid props. Source of truth: sdk/lib/ui/{widgets,
-- layout,tree,state}.luau. Containers and `Tree` take `children`/`root` as a
-- POSITIONAL second argument (Compose/SwiftUI lineage), not a prop.

--- A spacing slot for gaps and padding: either an inline logical-pixel number or
--- a spacing token from `getDesignTokens(theme)`. Theme spacing affects styling/layout rhythm,
--- not anchored tree placement.
export type WidgetSpacing = number | SpacingToken
--- Cross-axis alignment inside a stack/grid. Valid values: `"start"`, `"center"`,
--- `"end"`, `"stretch"`.
export type WidgetAlign = "start" | "center" | "end" | "stretch"
--- Easing curve for a UI presentation tween. Valid values: `"linear"`,
--- `"easeIn"`, `"easeOut"`, `"easeInOut"`. Tweens change renderer-local display
--- state only.
export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut"
--- Number-shape value tween for text, slider, and bar binds. `durationMs` is
--- milliseconds; optional `from` seeds the first displayed value before normal
--- retargeting takes over.
export type NumberTween = { durationMs: number, easing: WidgetEasing, from: number? }
--- Color-shape value tween for panel binds. `from` is an optional initial RGBA
--- tuple; later target changes retween from the current displayed color.
export type ColorTween = { durationMs: number, easing: WidgetEasing, from: {number}? }
--- A presentation-local bind reference produced by
--- `ui.createLocalState(...).cells.<name>:get()`. It resolves inside the nearest
--- declaring `localState` scope, not the engine state store.
export type LocalBindRef = { ["local"]: string }
--- A scalar comparand for UI visibility/selection predicates: number, boolean,
--- or string. Arrays are intentionally excluded from equality predicates.
export type PredicateValue = number | boolean | string
--- A reactive condition used by `visibleWhen`, `selected`, and `checked`: read
--- either an engine state ref or presentation-local cell and compare it to
--- `equals` when provided.
export type Predicate = (ReadonlyStateRef<PredicateValue> | LocalBindRef) & { equals: PredicateValue? }
--- Accessibility role override. Valid values: `"tab"`, `"tablist"`,
--- `"checkbox"`, `"radio"`, `"listitem"`, `"button"`, `"slider"`,
--- `"progressbar"`, `"image"`, `"group"`, `"none"`. Omit to use the widget's
--- implicit role.
export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"
--- Live-region announcement urgency. Valid values: `"polite"` (default,
--- interrupt less) and `"assertive"` (interrupt sooner).
export type AnnouncePriority = "polite" | "assertive"
--- State binding for a `Text` widget. The source is a readable engine state ref
--- or presentation-local cell; `format` is a one-placeholder string such as
--- `"HP {}"`; numeric sources may also tween.
export type TextBindProp = (ReadonlyStateRef<ScalarStateValue> | LocalBindRef) & { format: string?, tween: NumberTween? }
--- State binding for a `Panel` fill color. The source resolves to a numeric RGBA
--- array; `tween` eases the displayed color and never writes back to state.
export type PanelBindProp = (ReadonlyStateRef<NumericArrayStateValue> | LocalBindRef) & { tween: ColorTween? }
--- State binding for a writable numeric `Slider`. Engine refs must be writable;
--- local cells are valid. The optional number tween controls displayed thumb
--- movement only.
export type SliderBindProp = (WritableStateRef<number> | LocalBindRef) & { tween: NumberTween? }
--- One band in a `styleRanges` map. `upTo` is an inclusive normalized threshold;
--- omit it on the final entry to make that entry the default band. `color`,
--- `pulse`, and `flash` affect the rendered style, not authoritative state.
export type StyleRangeEntry = { upTo: number?, color: WidgetColor?, pulse: { periodMs: number }?, flash: { durationMs: number }? }
--- Continuous value-to-style map for text, panel, and bar widgets. Values are
--- normalized by `max`; entries are evaluated in order, and bars commonly use
--- `max = 1.0` because they style their displayed fill fraction.
export type StyleRangesProp = { max: number, entries: {StyleRangeEntry} }
--- 9-slice border descriptor. `texture` names a UI texture asset; `slice` is
--- `{left, top, right, bottom}` in source pixels; `tint` is an inline color or
--- theme token.
export type BorderProp = { texture: string, slice: {number}, tint: WidgetColor }
--- Per-direction focus-neighbor overrides. Each set direction names the widget id
--- focus should jump to, bypassing automatic spatial/linear focus search for
--- that direction.
export type FocusNeighborsProp = { up: string?, down: string?, left: string?, right: string? }
--- Hold-to-repeat timing in milliseconds. Used by repeatable buttons and
--- container nav-repeat policies: wait `initialDelayMs`, then fire every
--- `intervalMs` while held.
export type RepeatPolicyProp = { initialDelayMs: number, intervalMs: number }
--- A typed reaction handle returned by `defineReaction`; passing the handle lets
--- the SDK read `.name` and emit the same wire string without duplicating names
--- manually.
export type ReactionHandleRef = { name: string }
--- The flat `kind`-tagged descriptor produced by widget factories. It is retained
--- by Rust after setup; author scripts do not hold live widget instances.
export type WidgetDescriptor = { [string]: any }

--- Props for `Text`. `content` is `LocalizedText`. `fontSize` defaults to 12; `color` to opaque white.
export type TextProps = { content: LocalizedText, fontSize: number?, color: WidgetColor?, font: FontToken?, bind: TextBindProp?, styleRanges: StyleRangesProp?, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `text` leaf. An optional `bind` resolves the rendered string from a store slot; `styleRanges` recolors by value.
declare function Text(props: TextProps): WidgetDescriptor

--- Props for `Panel`. `bind` is a `PanelBindProp` (color slot).
export type PanelProps = { fill: WidgetColor, border: BorderProp?, bind: PanelBindProp?, styleRanges: StyleRangesProp?, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `panel` leaf: a solid `fill` with an optional 9-slice `border`.
declare function Panel(props: PanelProps): WidgetDescriptor

--- Props for `Image`. No bind. Name-XOR-decorative (M13 G2): exactly one of `label` or `decorative = true` (the union narrows it; neither/both errors).
export type ImageProps = { asset: string, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: string } | { decorative: true })
--- An `image` leaf referencing a texture asset by key; sizes from the asset's natural dimensions. Exactly one of `label` / `decorative = true` is required.
declare function Image(props: ImageProps): WidgetDescriptor

--- Props for `Spacer`. `flexGrow` defaults to 1. No bind.
export type SpacerProps = { flexGrow: number?, id: string?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `spacer` leaf claiming a proportional share of leftover space.
declare function Spacer(props: SpacerProps?): WidgetDescriptor

--- Props for `Button`. `onPress` is a reaction handle or a bare name string. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `selected`/`checked` are reactive predicates; `bind`+`styleRanges` drive the highlight; `disabled` makes it non-interactive.
export type ButtonProps = { id: string, onPress: ReactionHandleRef | string, repeatOnHold: RepeatPolicyProp?, focusNeighbors: FocusNeighborsProp?, selected: Predicate?, checked: Predicate?, bind: Predicate?, styleRanges: StyleRangesProp?, disabled: boolean?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: LocalizedText } | { labelledBy: string })
--- An interactive `button`. `id` is required. `onPress` accepts a `defineReaction` handle (its `.name` is read) or a bare reaction-name string, emitting the unchanged `onPress: string` wire form. Exactly one of `label` / `labelledBy` is required.
declare function Button(props: ButtonProps): WidgetDescriptor

--- Props for `Slider`. `bind` is a `SliderBindProp` (numeric slot); required. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `disabled` makes it non-interactive.
export type SliderProps = { id: string, bind: SliderBindProp, min: number, max: number, step: number, capturesNav: {string}?, focusNeighbors: FocusNeighborsProp?, disabled: boolean?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: LocalizedText } | { labelledBy: string })
--- An interactive `slider`. Nav wires in `capturesNav` step the bound value by `step` within `[min, max]`. Exactly one of `label` / `labelledBy` is required.
declare function Slider(props: SliderProps): WidgetDescriptor

--- State binding for a readonly numeric `Bar`. The value is displayed against
--- `max`; it is not interactive and never writes state.
export type BarBindProp = (ReadonlyStateRef<number> | LocalBindRef) & { tween: NumberTween? }
--- Bar denominator: either a literal number or a readonly numeric state ref such
--- as `getGameState().player.maxHealth`.
export type BarMaxProp = number | ReadonlyStateRef<number>
--- Props for `Bar`. `bind` is a readonly numeric bind; `max` is a number or readonly numeric ref.
export type BarProps = { bind: BarBindProp, max: BarMaxProp, fill: WidgetColor, background: WidgetColor, styleRanges: StyleRangesProp?, id: string?, visibleWhen: Predicate?, role: WidgetRole? }
--- A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`. `styleRanges` recolors the fill from that displayed fraction.
declare function Bar(props: BarProps): WidgetDescriptor

--- Props for `Announce`. `text` is the POSITIONAL second argument; `priority` defaults to `"polite"` (round-trips to omission).
export type AnnounceProps = { priority: AnnouncePriority?, visibleWhen: Predicate? }
--- A non-visual `announce` widget (M13 G2): a live-region message routed to the platform a11y layer at the declared `priority`. `text` is a POSITIONAL second argument.
declare function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor

--- Container focus traversal kind. `"linear"` follows child order; `"spatial"`
--- chooses by geometry in the requested nav direction.
export type FocusKind = "linear" | "spatial"
--- A container focus policy. Use a bare `"linear"`/`"spatial"` shorthand or a
--- table with `wrap` and `repeat` options; `repeat` controls held navigation
--- events inside the container.
export type FocusPolicyProp = FocusKind | { policy: FocusKind, wrap: boolean?, ["repeat"]: RepeatPolicyProp? }
--- Props for `VStack`/`HStack`. `gap` and `padding` default to 0; `align`
--- defaults to `"start"`; optional `fill`/`border` draw a backdrop behind the
--- arranged children. Stack containers may declare `localState`; stack and grid
--- containers both accept `visibleWhen` and `role`.
export type StackProps = { gap: WidgetSpacing?, padding: WidgetSpacing?, align: WidgetAlign?, id: string?, focusNeighbors: FocusNeighborsProp?, focus: FocusPolicyProp?, restoreOnReturn: boolean?, fill: WidgetColor?, border: BorderProp?, localState: { scope: string, cells: { [string]: number | boolean | string | {number} } }?, visibleWhen: Predicate?, role: WidgetRole? }
--- Props for `Grid`. `cols` is required and must be an integer >= 1. Children
--- flow row-major across columns; grid currently has no backdrop fill/border
--- and no `localState`.
export type GridProps = { gap: WidgetSpacing?, padding: WidgetSpacing?, align: WidgetAlign?, id: string?, focusNeighbors: FocusNeighborsProp?, focus: FocusPolicyProp?, restoreOnReturn: boolean?, cols: number, visibleWhen: Predicate?, role: WidgetRole? }

--- A vertical stack (`vstack`): `children` is a POSITIONAL second argument.
declare function VStack(props: StackProps?, children: {WidgetDescriptor}?): WidgetDescriptor
--- A horizontal stack (`hstack`): `children` is a POSITIONAL second argument.
declare function HStack(props: StackProps?, children: {WidgetDescriptor}?): WidgetDescriptor
--- A `grid` container: flows `children` across `cols` columns. `children` is a POSITIONAL second argument.
declare function Grid(props: GridProps, children: {WidgetDescriptor}?): WidgetDescriptor

--- Tree viewport anchor. Valid values: `"topLeft"`, `"top"`, `"topRight"`,
--- `"left"`, `"center"`, `"right"`, `"bottomLeft"`, `"bottom"`,
--- `"bottomRight"`.
export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight"
--- Tree input behavior. `"capture"` makes this tree consume UI input and freeze
--- lower modal layers; `"passthrough"` is the HUD/default mode and lets game
--- input continue.
export type WidgetCaptureMode = "capture" | "passthrough"
--- Placement envelope props for `Tree`. `anchor` + `offset` position the root in
--- 1280x720 logical UI space; `captureMode`, `initialFocus`, and
--- `textEntryTarget` control modal/input behavior.
export type TreeProps = { anchor: WidgetAnchor, offset: {number}, captureMode: WidgetCaptureMode?, initialFocus: string?, textEntryTarget: WritableStateRef<string>?, accessibleName: string?, role: WidgetRole? }
--- The flat `AnchoredTree` envelope produced by `Tree(...)` and stored in UI
--- registries. `textEntryTarget` is serialized to its dotted state-slot name.
export type AnchoredTreeDescriptor = { anchor: WidgetAnchor, offset: {number}, root: WidgetDescriptor, captureMode: WidgetCaptureMode?, initialFocus: string?, textEntryTarget: string?, accessibleName: string?, role: WidgetRole? }
--- Wrap a root widget descriptor in the `AnchoredTree` placement envelope. `root` is a POSITIONAL second argument.
declare function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor
--- Props accepted by `defineUiTree`. The returned object preserves the runtime
--- manifest entry shape `{ name, tree, alwaysOn? }`.
export type UiTreeRegistrationProps = { name: string, tree: AnchoredTreeDescriptor, alwaysOn: boolean? }
--- A typed UI-tree registration entry for `ModManifest.uiTrees` / `setupLevel().uiTrees`.
export type UiTreeRegistration = ModUiTree
--- Pure helper for defining a named UI-tree registration. Registration still
--- happens only when the returned object is included in a manifest `uiTrees`
--- array.
declare function defineUiTree(registration: UiTreeRegistrationProps): UiTreeRegistration

--- Bind-only options for number refs (text/bar/slider-compatible number shape).
export type NumberStateBindOptions = { format: string?, tween: NumberTween? }
--- Bind-only options for numeric-array refs (panel color shape).
export type NumericArrayStateBindOptions = { tween: ColorTween? }
--- Bind-only options for string/boolean scalar refs (text format only).
export type ScalarStateBindOptions = { format: string? }
--- Compose bind-only options onto a state ref, emitting `{ slot, ...options }`.
declare bindState: ((ReadonlyStateRef<number>, NumberStateBindOptions?) -> ReadonlyStateRef<number> & NumberStateBindOptions)
  & ((ReadonlyStateRef<{number}>, NumericArrayStateBindOptions?) -> ReadonlyStateRef<{number}> & NumericArrayStateBindOptions)
  & ((ReadonlyStateRef<string>, ScalarStateBindOptions?) -> ReadonlyStateRef<string> & ScalarStateBindOptions)
  & ((ReadonlyStateRef<boolean>, ScalarStateBindOptions?) -> ReadonlyStateRef<boolean> & ScalarStateBindOptions)
--- Build `{ slot, equals }` for scalar state refs.
declare function stateEquals<T>(ref: ReadonlyStateRef<T>, value: T): Predicate

--- A presentation-cell handle (`ui.createLocalState`): `:get()` yields a `{ ["local"] }`
--- bind ref; `:set(v)` emits a `cellWrite` reaction (NEVER `setState`); `:is(v)` produces
--- an equality `Predicate` (comparand typed to the cell's `T`). Presentation-only.
export type LocalStateHandle<T> = {
  get: (self: LocalStateHandle<T>) -> LocalBindRef,
  set: (self: LocalStateHandle<T>, value: T) -> PrimitiveReactionDescriptor,
  is: (self: LocalStateHandle<T>, value: T) -> Predicate,
}
--- Declare a presentation-cell scope (M13 G1b). SDK-lib function, not a registered primitive. Pure: no engine side effect. Returns a `{ scope, cells }` bundle.
declare function createLocalState(init: { [string]: any }): { scope: any, cells: any }
--- `Switch(cell, map)` (M13 G2) — expand a string-valued cell's `map` of `value -> subtree` into an array, injecting `visibleWhen = cell:is(key)` onto each subtree in LEXICOGRAPHICALLY-SORTED key order (byte-identical TS/Luau). Splice the result into a container's `children`.
declare function Switch(cell: LocalStateHandle<string>, map: { [string]: WidgetDescriptor }): {WidgetDescriptor}
--- State-helper namespace (state helpers are namespaced; reactions stay bare).
declare ui: { createLocalState: (init: { [string]: any }) -> { scope: any, cells: any } }

-- ---------------------------------------------------------------------------
-- Runtime-value vocabulary — the typed command buffer (scripting.md §11). The
-- `runtime.*` builders assemble these node tables as plain data; constructing a
-- node has no FFI side effect. The `RuntimeValue` union is the *closure* of the
-- vocabulary: an author cannot name an op outside it. Field names match the
-- Rust `IrNode` wire format byte-for-byte (`a`/`b`, `x`/`lo`/`hi`, `cond`,
-- `name`, `value`) so builder output deserializes straight into `IrNode`.
-- (Author surface is `runtime`/`RuntimeValue`; the Rust substrate and wire op
-- tags keep the `ir` names — scripting.md §11, "Author-facing naming".)
-- Source of truth: crates/postretro/src/scripting/ir/mod.rs + sdk/lib/runtime.luau.
-- Static block (not registry-emitted): `register_tagged_union` /
-- `TypeShape::TaggedUnion` renders one payload type name per variant under a
-- fixed tag key — it cannot express per-variant inline struct fields (e.g.
-- `value`, `a`/`b`, `cond`) or the recursive `RuntimeValue` self-reference that
-- every non-leaf variant requires.

--- Literal scalar leaf: `{ op = "const", value }`. `value` is a number or boolean.
export type RuntimeConst = { op: "const", value: number | boolean }
--- Named-input leaf: `{ op = "input", name }`. Bound to live state by the Rust evaluator.
export type RuntimeRead = { op: "input", name: string }
--- Addition: `a + b` (number).
export type RuntimeAdd = { op: "add", a: RuntimeValue, b: RuntimeValue }
--- Subtraction: `a - b` (number).
export type RuntimeSub = { op: "sub", a: RuntimeValue, b: RuntimeValue }
--- Multiplication: `a * b` (number).
export type RuntimeMul = { op: "mul", a: RuntimeValue, b: RuntimeValue }
--- Division: `a / b` (number).
export type RuntimeDiv = { op: "div", a: RuntimeValue, b: RuntimeValue }
--- Clamp `x` to `[lo, hi]` (number).
export type RuntimeClamp = { op: "clamp", x: RuntimeValue, lo: RuntimeValue, hi: RuntimeValue }
--- Linear interpolation between `a` and `b` by `t` (number).
export type RuntimeLerp = { op: "lerp", a: RuntimeValue, b: RuntimeValue, t: RuntimeValue }
--- Less-than comparison (boolean).
export type RuntimeLt = { op: "lt", a: RuntimeValue, b: RuntimeValue }
--- Less-than-or-equal comparison (boolean).
export type RuntimeLe = { op: "le", a: RuntimeValue, b: RuntimeValue }
--- Greater-than comparison (boolean).
export type RuntimeGt = { op: "gt", a: RuntimeValue, b: RuntimeValue }
--- Greater-than-or-equal comparison (boolean).
export type RuntimeGe = { op: "ge", a: RuntimeValue, b: RuntimeValue }
--- Equality comparison (boolean).
export type RuntimeEq = { op: "eq", a: RuntimeValue, b: RuntimeValue }
--- Inequality comparison (boolean).
export type RuntimeNe = { op: "ne", a: RuntimeValue, b: RuntimeValue }
--- Branchless select: `cond ? a : b`. `a` and `b` share a type.
export type RuntimeSelect = { op: "select", cond: RuntimeValue, a: RuntimeValue, b: RuntimeValue }

--- A node in the authored runtime-value tree. Closed vocabulary: every node the
--- evaluator accepts is one of these variants. New opcodes extend this union
--- in lockstep with the Rust `IrNode` enum.
export type RuntimeValue = RuntimeConst | RuntimeRead | RuntimeAdd | RuntimeSub | RuntimeMul | RuntimeDiv | RuntimeClamp | RuntimeLerp | RuntimeLt | RuntimeLe | RuntimeGt | RuntimeGe | RuntimeEq | RuntimeNe | RuntimeSelect

--- A builder operand: an already-built node, or a bare `number`/`boolean`
--- literal that the builder auto-wraps into a `const` node.
export type RuntimeOperand = RuntimeValue | number | boolean

--- Pure builder vocabulary for runtime values, installed as global `runtime`.
--- Every method returns a plain `RuntimeValue` table; constructing a node has
--- no FFI side effect. Bare `number`/`boolean` operands are auto-wrapped into
--- `const` nodes.
--- Builders are dot-called (`runtime.add(...)`), not method-called, so the
--- signatures take no `self` parameter.
export type Runtime = {
  --- Literal scalar leaf. `const` is reserved, so the builder is `constant`.
  constant: (value: number | boolean) -> RuntimeConst,
  --- Named-input leaf, bound to live state by name in the Rust evaluator.
  read: (name: string) -> RuntimeRead,
  --- `a + b` (number).
  add: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeAdd,
  --- `a - b` (number).
  sub: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeSub,
  --- `a * b` (number).
  mul: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeMul,
  --- `a / b` (number).
  div: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeDiv,
  --- Clamp `x` to `[lo, hi]` (number).
  clamp: (x: RuntimeOperand, lo: RuntimeOperand, hi: RuntimeOperand) -> RuntimeClamp,
  --- Linear interpolation between `a` and `b` by `t` (number).
  lerp: (a: RuntimeOperand, b: RuntimeOperand, t: RuntimeOperand) -> RuntimeLerp,
  --- `a < b` (boolean).
  lt: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeLt,
  --- `a <= b` (boolean).
  le: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeLe,
  --- `a > b` (boolean).
  gt: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeGt,
  --- `a >= b` (boolean).
  ge: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeGe,
  --- `a == b` (boolean).
  eq: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeEq,
  --- `a ~= b` (boolean).
  ne: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeNe,
  --- Branchless select: `cond ? a : b`. `a` and `b` share a type.
  select: (cond: RuntimeOperand, a: RuntimeOperand, b: RuntimeOperand) -> RuntimeSelect,
}

--- Runtime-value builder vocabulary global.
declare runtime: Runtime

-- UI navigation intents — the closed gamepad-first nav vocabulary the input
-- stage produces (keyboard arrows/enter/escape, D-pad, stick edges) and that UI
-- authors reference in `capturesNav` and focus policy. Wire names mirror the
-- Rust `NavIntent` enum (input/ui_nav.rs). Luau has no template-literal types,
-- so this is a flat string union over the same closed set.
-- See: context/research/ui-layer.md §16.
export type NavIntent = "nav.up" | "nav.down" | "nav.left" | "nav.right" | "nav.next" | "nav.prev" | "nav.confirm" | "nav.cancel" | "nav.menu" | "nav.options"

