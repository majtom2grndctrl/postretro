
  // -------------------------------------------------------------------------
  // SDK library — globals installed by the runtime prelude. Import by bare specifier; the bundler strips the import at compile time.

  /** Capability for entities with a scalar animation channel (brightness, density, etc.). `Channel` is type-level documentation — the handle's implementation closure knows which descriptor channel to drive. */
  export interface AnimatableScalar<Channel extends string> {
    /** Sine pulse oscillating between `min` and `max` over `periodMs`. Loops forever. */
    pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
    /** One-shot linear ramp from `from` to `to` over `periodMs`. Plays exactly once. */
    fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
    /** Irregular flicker between `min` and `max` at `rate` Hz. Loops forever. */
    flicker(opts: { min: number; max: number; rate: number }): SequenceStep[];
    readonly __channel?: Channel;
  }

  /** Capability for entities with a vec3 animation channel. */
  export interface AnimatableVec3<Channel extends string> {
    /** Uniform cycle through the given vectors over `periodMs`. */
    cycle(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
    readonly __channel?: Channel;
  }

  /** Typed light handle returned by `world.query({ component: "light" })`. Composes the brightness scalar capability with vec3 channels declared directly (TypeScript collapses duplicate method names, so secondary vec3 channels are not pulled in via `AnimatableVec3` extension). */
  export interface LightEntityHandle extends LightEntity, AnimatableScalar<"brightness"> {
    /** Cycle through RGB colors over `periodMs`. Dynamic lights only. */
    colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
    /** Sweep the `direction` channel through unit vectors over `periodMs`. */
    sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
  }

  /** Typed fog-volume handle returned by `world.query({ component: "fog_volume" })`. Composes the density scalar capability with secondary saturation methods declared directly. */
  export interface FogVolumeHandle extends FogVolumeEntity, AnimatableScalar<"density"> {
    /** Looping sine pulse on the `saturation` channel. */
    pulseSaturation(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
    /** One-shot linear ramp on the `saturation` channel. */
    fadeSaturation(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
  }

  /** Maps a component-name literal to the rich entity handle type. `"light"`
   * yields `LightEntityHandle` (capability methods); `"emitter"` yields
   * `EmitterEntity` (id, position, tags, plus the full `BillboardEmitterComponent`
   * snapshot under `component`); `"fog_volume"` yields `FogVolumeHandle`.
   * Other component names fall back to the bare `Entity` shape (`id`,
   * `position`, `tags`). */
  export type EntityForComponent<T extends WorldQueryComponent> =
    T extends "light" ? LightEntityHandle :
    T extends "emitter" ? EmitterEntity :
    T extends "fog_volume" ? FogVolumeHandle :
    Entity;

  /** Vocabulary object installed as `globalThis.world`. */
  export interface World {
    query<T extends WorldQueryComponent>(filter: {
      component: T;
      tag?: string | null;
    }): EntityForComponent<T>[];
    /** Current world gravity in m/s² (negative = downward; positive = upward). Seeded from the worldspawn `initialGravity` KVP at level load and persists until the next level load or `setGravity` call. */
    getGravity(): number;
    /** Set world gravity in m/s² (negative = downward; positive = upward). NaN and non-finite values are silently ignored with a warning logged. Effect is immediate and persists until the next level load or another `setGravity` call. */
    setGravity(value: number): void;
  }

  /** `world` vocabulary global. Wraps `worldQuery` with a typed handle. */
  export const world: World;

  /** Per-channel keyframe accepted by `timeline` / `sequence`. */
  export type Keyframe<T extends number[]> = [number, ...T];

  /** Validate `[absolute_ms, ...value]` keyframes; pass-through on success. */
  export function timeline<T extends number[]>(
    keyframes: [number, ...T][],
  ): [number, ...T][];

  /** Convert `[delta_ms, ...value]` keyframes to absolute-time form. */
  export function sequence<T extends number[]>(
    keyframes: [number, ...T][],
  ): [number, ...T][];

  // -------------------------------------------------------------------------
  // Data script vocabulary — pure descriptor builders consumed by the engine
  // when `setupLevel` returns. See: context/lib/scripting.md §2.

  /** Progress-subscription reaction body: fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
  export type ProgressReactionDescriptor = {
    progress: { tag: string; at: number; fire: string };
  };

  /** Primitive reaction body: invokes the named Rust primitive. With `tag`, it targets entities carrying that tag and mutates them. Without `tag`, it is a system reaction (no entities) that enqueues a typed engine command — `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args` carries the primitive's typed payload (e.g. `{ rate: 0 }` for `setEmitterRate`, `{ sound: "alarm" }` for `playSound`). */
  export type PrimitiveReactionDescriptor = {
    primitive: string;
    tag?: string;
    args?: Record<string, unknown>;
    onComplete?: string;
  };

  /** One step in a `sequence` reaction body: invokes the named sequenced primitive against the given entity with `args`. Sequence steps target a single `EntityId`; tag-targeted primitives belong on the `Primitive` reaction path. */
  export type SetLightAnimationStep = {
    id: EntityId;
    primitive: "setLightAnimation";
    args: LightAnimation;
  };

  /** Sequence step targeting a single fog volume's `density`. Use directly for a one-shot density change. */
  export type SetFogDensityStep = {
    id: EntityId;
    primitive: "setFogDensity";
    args: { density: number };
  };

  /** Sequence step targeting a single fog volume's `glow`. */
  export type SetFogGlowStep = {
    id: EntityId;
    primitive: "setFogGlow";
    args: { glow: number };
  };

  /** Sequence step targeting a single fog volume's `edgeSoftness`. */
  export type SetFogEdgeSoftnessStep = {
    id: EntityId;
    primitive: "setFogEdgeSoftness";
    args: { edgeSoftness: number };
  };

  /** Sequence step targeting a single fog volume's `falloff`. */
  export type SetFogFalloffStep = {
    id: EntityId;
    primitive: "setFogFalloff";
    args: { falloff: number };
  };

  /** Sequence step that updates any subset of `{density, glow, edgeSoftness, falloff, tint, saturation, minBrightness, lightRange}` on a single fog volume in one component write. */
  export type SetFogParamsStep = {
    id: EntityId;
    primitive: "setFogParams";
    args: {
      density?: number;
      glow?: number;
      edgeSoftness?: number;
      falloff?: number;
      tint?: readonly [number, number, number];
      saturation?: number;
      minBrightness?: number;
      lightRange?: number;
    };
  };

  /** Sequence step that installs (or clears, when `args` is `null`) a dual-channel animation (density and/or saturation) on a single fog volume. Emitted by the `FogVolumeHandle` capability methods (`pulse`, `fade`, `flicker`, `pulseSaturation`, `fadeSaturation`). */
  export type SetFogAnimationStep = {
    id: EntityId;
    primitive: "setFogAnimation";
    args: FogAnimation | null;
  };

  /** Union of every supported sequence step shape. New sequenced primitives extend this union. */
  export type SequenceStep =
    | SetLightAnimationStep
    | SetFogDensityStep
    | SetFogGlowStep
    | SetFogEdgeSoftnessStep
    | SetFogFalloffStep
    | SetFogParamsStep
    | SetFogAnimationStep;

  /** Sequence reaction body: ordered per-entity primitive invocations. Steps run in array order at dispatch. */
  export type SequenceReactionDescriptor = {
    sequence: SequenceStep[];
  };

  /** Descriptor produced by `defineReaction`. The `name` field is merged into the descriptor at the top level so the Rust deserializer reads both fields from one flat object. */
  export type NamedReactionDescriptor = { name: string; levels?: string[] } & (
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor
  );

  /** Crossing condition: fires when the watched slot crosses the threshold in one direction. Exactly one of `below`/`above` is given. `max` is the denominator the threshold is a fraction of; omit it for a raw-value comparison (`max` defaults to `1.0`). */
  export type CrossingCondition =
    | { below: number; above?: never; max?: number }
    | { above: number; below?: never; max?: number };

  /** A state-crossing watcher entry as it appears in `setupLevel().crossings` or `ModManifest.crossings`. The condition fields are flattened in beside `slot` and `fire`; `fire` lists the named reactions dispatched when the crossing occurs. `levels` scopes mod-global crossings by map-catalog tags; omit it for every level. */
  export type CrossingDescriptor = {
    slot: string;
    max?: number;
    fire: string[];
    levels?: string[];
  } & ({ below: number } | { above: number });

  /** Bundle returned from `setupLevel`. The engine deserializes this shape in one pass at level load. */
  export type LevelManifest = {
    reactions: NamedReactionDescriptor[];
    crossings?: CrossingDescriptor[];
    /** Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same shape as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed entries are logged and skipped. */
    uiTrees?: ReadonlyArray<ModUiTree>;
  };

  /** Build a named reaction descriptor. Pure: returns a plain object, no FFI.
   * `descriptor` accepts exactly one body shape: `progress` (kill-ratio trigger),
   * `primitive` (named Rust primitive with optional entity `tag` and typed
   * `args`), or `sequence` (ordered per-entity steps). `name` is optional; when
   * omitted a deterministic, run-stable id is derived from the body. Use explicit
   * names when TS and Luau scripts must agree. The returned handle can be passed
   * to `Button.onPress` or crossing `fire` entries.
   * @param name Stable event/reaction name consumed by dispatch. Optional.
   * @param descriptor Reaction body data consumed later by Rust. */
  export function defineReaction(
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;
  export function defineReaction(
    name: string,
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;

  /** Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are matched against `ModMapEntry.tags`; omit scoping for every level. */
  export function scopeReactions(
    tags: string[],
    list: NamedReactionDescriptor[],
  ): NamedReactionDescriptor[];

  // -------------------------------------------------------------------------
  // State-store declarations. `defineStore` is special-cased in the typedef
  // generator (mirroring `worldQuery`): per-slot value types live only in the
  // runtime `schema` argument, absent at typedef emission, so the typed state
  // reference map is supplied by this hand-written generic instead of registry
  // emission.

  declare const stateRefValueBrand: unique symbol;
  declare const writableStateRefBrand: unique symbol;
  export type ScalarStateValue = number | boolean | string;
  export type NumericArrayStateValue = ReadonlyArray<number>;
  export type ReadonlyStateRef<T> = { readonly slot: string; readonly [stateRefValueBrand]: T };
  export type WritableStateRef<T> = ReadonlyStateRef<T> & { readonly [writableStateRefBrand]: T };

  /** One slot inside a `defineStore` schema. Every slot needs `default`. `type: "number"` accepts a finite numeric default plus optional inclusive `range: [min, max]`; `"boolean"` and `"string"` require matching defaults; `"enum"` requires non-empty `values` and a default in that list; `"array"` is a finite-number array. `persist` saves on clean exit; `readonly` blocks script writes. */
  export type StoreSlotSchema = { type: "number" | "boolean" | "string" | "enum" | "array"; readonly?: boolean } & Record<string, unknown>;

  /** Plain declaration data returned through `ModManifest.stores`. */
  export type StoreDeclaration = { namespace: string; schema: Record<string, StoreSlotSchema> };

  /** Maps one schema slot's `type` discriminant to its handle value type:
   * `{type:"number"}` → number ref, `{type:"boolean"}` →
   * boolean ref, `array` → numeric-array ref, and `string`/`enum` →
   * string ref. Slots with `readonly: true` produce `ReadonlyStateRef<T>`;
   * all other slots produce `WritableStateRef<T>`. */
  export type StoreStateRefForSlot<Slot, T> =
    Slot extends { readonly: true } ? ReadonlyStateRef<T> : WritableStateRef<T>;

  export type StateValueForSlot<Slot> =
    Slot extends { type: "number" } ? StoreStateRefForSlot<Slot, number> :
    Slot extends { type: "boolean" } ? StoreStateRefForSlot<Slot, boolean> :
    Slot extends { type: "array" } ? StoreStateRefForSlot<Slot, ReadonlyArray<number>> :
    StoreStateRefForSlot<Slot, string>;

  /** Result of a pure `defineStore` call. Return `declaration` from `ModManifest.stores`; use `state` references in descriptors. */
  export type StoreDefinition<S extends Record<string, StoreSlotSchema>> = {
    readonly declaration: StoreDeclaration;
    readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };
  };

  /** Build a state-store declaration. Pure: calling it performs no FFI and changes no engine state. `namespace` prefixes returned refs as `namespace.slotName`; `schema` declares slot names and validation rules. Returned declarations commit atomically only after the mod manifest succeeds. */
  export function defineStore<const S extends Record<string, StoreSlotSchema>>(
    namespace: string,
    schema: S,
  ): StoreDefinition<S>;

  // -------------------------------------------------------------------------
  // UI theme helpers. `defineTheme` accepts nested singular token groups and
  // returns the runtime wire shape (flat colors/fonts/spacing maps).
  // `getDesignTokens` exposes a nested token-name tree for authoring sites
  // without changing descriptor wire data.

  /** Linear RGBA color token value. Components are in display-linear 0-1 space; alpha is the fourth element. */
  export type ThemeColorValue = readonly [number, number, number, number];
  declare const themeTokenBrand: unique symbol;
  /** Runtime-authenticated SDK token record. Widget factories unwrap only records produced by `getDesignTokens(theme)`, not hand-built lookalikes. */
  export type ThemeToken<Category extends "color" | "font" | "spacing"> = Readonly<{
    __postretroToken: Category;
    token: string;
    readonly [themeTokenBrand]: Category;
  }>;
  export type ColorToken = ThemeToken<"color">;
  export type FontToken = ThemeToken<"font">;
  export type SpacingToken = ThemeToken<"spacing">;
  /** Nested authored token tree for one theme category. Leaf values are category-specific token definitions; object keys become dot-joined token names. */
  export type ThemeTokenTree<Leaf> = { readonly [key: string]: Leaf | ThemeTokenTree<Leaf> };
  /** Nested theme authoring input. Use singular `color`, `font`, and `spacing`; plural `colors` / `fonts` input is unsupported. */
  export type ThemeDefinition = {
    readonly color?: ThemeTokenTree<ThemeColorValue>;
    readonly font?: ThemeTokenTree<string>;
    readonly spacing?: ThemeTokenTree<number>;
    readonly colors?: never;
    readonly fonts?: never;
    readonly tokens?: never;
  };
  type ThemeJoinPath<Prefix extends string, Key extends string> = Prefix extends "" ? Key : `${Prefix}.${Key}`;
  type ThemeFlattenTokenKeys<Tree, Leaf, Prefix extends string = ""> =
    Tree extends Leaf ? Prefix :
    Tree extends Readonly<Record<string, unknown>> ? {
      [K in Extract<keyof Tree, string>]: ThemeFlattenTokenKeys<Tree[K], Leaf, ThemeJoinPath<Prefix, K>>
    }[Extract<keyof Tree, string>] : never;
  type ThemeFlatTokenMap<Tree, Leaf, Value> = Record<ThemeFlattenTokenKeys<NonNullable<Tree>, Leaf>, Value>;
  type ThemeDesignTokenTree<Tree, Leaf, Token, Prefix extends string = ""> =
    Tree extends Leaf ? Token :
    Tree extends Readonly<Record<string, unknown>> ? {
      readonly [K in Extract<keyof Tree, string>]: ThemeDesignTokenTree<Tree[K], Leaf, Token, ThemeJoinPath<Prefix, K>>
    } : never;
  type ThemeDesignTokenGroup<Tree, Leaf, Token> = [Tree] extends [undefined] ? {} : ThemeDesignTokenTree<NonNullable<Tree>, Leaf, Token>;
  /** Nested token tree returned by `getDesignTokens`. Leaves are branded objects that widget factories unwrap to the flat token path consumed by descriptors. */
  export type DesignTokens<T extends ThemeDefinition> = {
    readonly color: ThemeDesignTokenGroup<T["color"], ThemeColorValue, ColorToken>;
    readonly font: ThemeDesignTokenGroup<T["font"], string, FontToken>;
    readonly spacing: ThemeDesignTokenGroup<T["spacing"], number, SpacingToken>;
  };
  declare const definedThemeBrand: unique symbol;
  /** A theme returned by `defineTheme`: enumerable flat manifest maps plus SDK metadata for `getDesignTokens`. Pass this object directly as `ModManifest.theme`. */
  export type DefinedTheme<T extends ThemeDefinition> = {
    readonly colors: ThemeFlatTokenMap<T["color"], ThemeColorValue, ThemeColorValue>;
    readonly fonts: ThemeFlatTokenMap<T["font"], string, string>;
    readonly spacing: ThemeFlatTokenMap<T["spacing"], number, number>;
    readonly [definedThemeBrand]: T;
  };
  /** Define a custom theme from nested singular groups while preserving the runtime theme shape. */
  export function defineTheme<const T extends ThemeDefinition>(theme: T): DefinedTheme<T>;
  /** Return the nested token-name tree for the exact object returned by `defineTheme`; plain manifest themes and clones throw. */
  export function getDesignTokens<const T extends ThemeDefinition>(theme: DefinedTheme<T>): DesignTokens<T>;

  // -------------------------------------------------------------------------
  // Shared UI widget value slots (M13 Goal F). Type-only aliases for the slot
  // and value types the widget factory props compose (camelCase wire shape).

  /** The type of every user-facing text string a widget displays. A single alias (`= string` today) so a future localization scheme — message keys, ICU handles — is one edit, not a sweep across every text prop. */
  export type LocalizedText = string;

  /** A widget color slot: either an inline linear-RGBA tuple `[r, g, b, a]` or a branded color token from `getDesignTokens(theme)`. */
  export type WidgetColor = [number, number, number, number] | ColorToken;

  /** A numeric state bind descriptor shared by low-level `slider`/`bar` wire shapes: a dotted slot name plus an optional number tween. Most authors should call `bindState(ref, options)` instead of constructing this manually. */
  export type SliderBind = { slot: string; tween?: { durationMs: number; easing: "linear" | "easeIn" | "easeOut" | "easeInOut"; from?: number } };

  /** Continuous value-to-style map. Text and panel widgets normalize their rendered numeric value by `max`; bars evaluate their displayed fill fraction, so health bands usually use `max: 1.0`. Entries are checked in order; the first `upTo` threshold that contains the normalized value wins, and a trailing entry without `upTo` is the default band. */
  export type WidgetStyleRanges = { max: number; entries: { upTo?: number; color?: WidgetColor; pulse?: { periodMs: number }; flash?: { durationMs: number } }[] };

  // -------------------------------------------------------------------------
  // UI widget / layout / tree / state factories (M13 G1a). Pure builders
  // installed as prelude globals: each returns the camelCase wire descriptor of
  // the matching `render/ui/descriptor.rs` variant and throws a field-named
  // `Error` on invalid props. Source of truth: sdk/lib/ui/{widgets,layout,tree,
  // state}.ts. Containers and `Tree` take `children`/`root` as a POSITIONAL
  // second argument (Compose/SwiftUI lineage), not a prop.

  /** A spacing slot for gaps and padding: either an inline logical-pixel number or a branded spacing token from `getDesignTokens(theme)`. Theme spacing affects styling/layout rhythm, not anchored tree placement. */
  export type WidgetSpacing = number | SpacingToken;
  /** Cross-axis alignment inside a stack/grid. Valid values: `"start"`, `"center"`, `"end"`, `"stretch"`. */
  export type WidgetAlign = "start" | "center" | "end" | "stretch";
  /** Easing curve for a UI presentation tween. Valid values: `"linear"`, `"easeIn"`, `"easeOut"`, `"easeInOut"`. Tweens change renderer-local display state only. */
  export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut";
  /** Number-shape value tween for text, slider, and bar binds. `durationMs` is milliseconds; optional `from` seeds the first displayed value before normal retargeting takes over. */
  export type NumberTween = { durationMs: number; easing: WidgetEasing; from?: number };
  /** Color-shape value tween for panel binds. `from` is an optional initial RGBA tuple; later target changes retween from the current displayed color. */
  export type ColorTween = { durationMs: number; easing: WidgetEasing; from?: [number, number, number, number] };
  /** A presentation-local bind reference produced by `ui.createLocalState(...).cells.<name>.get()`. It resolves inside the nearest declaring `localState` scope, not the engine state store. */
  export type LocalBindRef = { local: string };
  /** A scalar comparand for UI visibility/selection predicates: number, boolean, or string. Arrays are intentionally excluded from equality predicates. */
  export type PredicateValue = number | boolean | string;
  /** A reactive condition used by `visibleWhen`, `selected`, and `checked`: read either an engine state ref or presentation-local cell and compare it to `equals` when provided. */
  export type Predicate = ((ReadonlyStateRef<PredicateValue> & { local?: never }) | LocalBindRef) & { equals?: PredicateValue };
  /** Accessibility role override. Valid values: `"tab"`, `"tablist"`, `"checkbox"`, `"radio"`, `"listitem"`, `"button"`, `"slider"`, `"progressbar"`, `"image"`, `"group"`, `"none"`. Omit to use the widget's implicit role. */
  export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none";
  /** Live-region announcement urgency. Valid values: `"polite"` (default, interrupt less) and `"assertive"` (interrupt sooner). */
  export type AnnouncePriority = "polite" | "assertive";
  /** State binding for a `Text` widget. The source is a readable engine state ref or presentation-local cell; `format` is a one-placeholder string such as `"HP {}"`; numeric sources may also tween. */
  export type TextBindProp = ((ReadonlyStateRef<ScalarStateValue> & { local?: never }) | LocalBindRef) & { format?: string; tween?: NumberTween };
  /** State binding for a `Panel` fill color. The source resolves to a numeric RGBA array; `tween` eases the displayed color and never writes back to state. */
  export type PanelBindProp = ((ReadonlyStateRef<NumericArrayStateValue> & { local?: never; format?: never }) | LocalBindRef) & { tween?: ColorTween };
  /** State binding for a writable numeric `Slider`. Engine refs must be writable; local cells are valid. The optional number tween controls displayed thumb movement only. */
  export type SliderBindProp = ((WritableStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  /** State binding for a readonly numeric `Bar`. The value is displayed against `max`; it is not interactive and never writes state. */
  export type BarBindProp = ((ReadonlyStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  /** Bar denominator: either a literal number or a readonly numeric state ref such as `getGameState().player.maxHealth`. */
  export type BarMaxProp = number | ReadonlyStateRef<number>;
  /** One band in a `styleRanges` map. `upTo` is an inclusive normalized threshold; omit it on the final entry to make that entry the default band. `color`, `pulse`, and `flash` affect the rendered style, not authoritative state. */
  export type StyleRangeEntry = { upTo?: number; color?: WidgetColor; pulse?: { periodMs: number }; flash?: { durationMs: number } };
  /** Continuous value-to-style map for text, panel, and bar widgets. Values are normalized by `max`; entries are evaluated in order, and bars commonly use `max: 1.0` because they style their displayed fill fraction. */
  export type StyleRangesProp = { max: number; entries: StyleRangeEntry[] };
  /** 9-slice border descriptor. `texture` names a UI texture asset; `slice` is `[left, top, right, bottom]` in source pixels; `tint` is an inline color or theme token. */
  export type BorderProp = { texture: string; slice: [number, number, number, number]; tint: WidgetColor };
  /** Per-direction focus-neighbor overrides. Each set direction names the widget id focus should jump to, bypassing automatic spatial/linear focus search for that direction. */
  export type FocusNeighborsProp = { up?: string; down?: string; left?: string; right?: string };
  /** Hold-to-repeat timing in milliseconds. Used by repeatable buttons and container nav-repeat policies: wait `initialDelayMs`, then fire every `intervalMs` while held. */
  export type RepeatPolicyProp = { initialDelayMs: number; intervalMs: number };
  /** A typed reaction handle returned by `defineReaction`; passing the handle lets the SDK read `.name` and emit the same wire string without duplicating names manually. */
  export type ReactionHandleRef = { name: string };
  /** The flat `kind`-tagged descriptor produced by widget factories. It is retained by Rust after setup; author scripts do not hold live widget instances. */
  export type WidgetDescriptor = { kind: string; [field: string]: unknown };

  /** Props for `Text`. `content` is `LocalizedText`. `fontSize` defaults to 12; `color` to opaque white. */
  export type TextProps = { content: LocalizedText; fontSize?: number; color?: WidgetColor; font?: FontToken; bind?: TextBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `text` leaf. An optional `bind` resolves the rendered string from a store slot; `styleRanges` recolors by value. */
  export function Text(props: TextProps): WidgetDescriptor;

  /** Props for `Panel`. `bind` is a `PanelBindProp` (color slot). */
  export type PanelProps = { fill: WidgetColor; border?: BorderProp; bind?: PanelBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `panel` leaf: a solid `fill` with an optional 9-slice `border`. */
  export function Panel(props: PanelProps): WidgetDescriptor;

  /** Props for `Image`. No bind. Name-XOR-decorative (M13 G2): exactly one of `label` or `decorative: true` (the union narrows it; neither/both throws). */
  export type ImageProps = { asset: string; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: string; decorative?: never } | { decorative: true; label?: never });
  /** An `image` leaf referencing a texture asset by key; sizes from the asset's natural dimensions. Exactly one of `label` / `decorative: true` is required. */
  export function Image(props: ImageProps): WidgetDescriptor;

  /** Props for `Spacer`. `flexGrow` defaults to 1. No bind. */
  export type SpacerProps = { flexGrow?: number; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `spacer` leaf claiming a proportional share of leftover space. */
  export function Spacer(props?: SpacerProps): WidgetDescriptor;

  /** Props for `Button`. `onPress` is a reaction handle or a bare name string. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `selected`/`checked` are reactive predicates; `bind`+`styleRanges` drive the highlight; `disabled` makes it non-interactive. */
  export type ButtonProps = { id: string; onPress: ReactionHandleRef | string; repeatOnHold?: RepeatPolicyProp; focusNeighbors?: FocusNeighborsProp; selected?: Predicate; checked?: Predicate; bind?: Predicate; styleRanges?: StyleRangesProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** An interactive `button`. `id` is required. `onPress` accepts a `defineReaction` handle (its `.name` is read) or a bare reaction-name string, emitting the unchanged `onPress: string` wire form. Exactly one of `label` / `labelledBy` is required. */
  export function Button(props: ButtonProps): WidgetDescriptor;

  /** Props for `Slider`. `bind` is a `SliderBindProp` (numeric slot); required. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `disabled` makes it non-interactive. */
  export type SliderProps = { id: string; bind: SliderBindProp; min: number; max: number; step: number; capturesNav?: string[]; focusNeighbors?: FocusNeighborsProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** An interactive `slider`. Nav wires in `capturesNav` step the bound value by `step` within `[min, max]`. Exactly one of `label` / `labelledBy` is required. */
  export function Slider(props: SliderProps): WidgetDescriptor;

  /** Props for `Bar`. `bind` is a readonly numeric bind; `max` is a number or readonly numeric ref. */
  export type BarProps = { bind: BarBindProp; max: BarMaxProp; fill: WidgetColor; background: WidgetColor; styleRanges?: StyleRangesProp; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`. `styleRanges` recolors the fill from that displayed fraction. */
  export function Bar(props: BarProps): WidgetDescriptor;

  /** Props for `Announce`. `text` is the POSITIONAL second argument; `priority` defaults to `"polite"` (round-trips to omission). */
  export type AnnounceProps = { priority?: AnnouncePriority; visibleWhen?: Predicate };
  /** A non-visual `announce` widget (M13 G2): a live-region message routed to the platform a11y layer at the declared `priority`. `text` is a POSITIONAL second argument. */
  export function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor;

  /** Container focus traversal kind. `"linear"` follows child order; `"spatial"` chooses by geometry in the requested nav direction. */
  export type FocusKind = "linear" | "spatial";
  /** A container focus policy. Use a bare `"linear"`/`"spatial"` shorthand or an object with `wrap` and `repeat` options; `repeat` controls held navigation events inside the container. */
  export type FocusPolicyProp = FocusKind | { policy: FocusKind; wrap?: boolean; repeat?: RepeatPolicyProp };
  /** Props for `VStack`/`HStack`. `gap` and `padding` default to 0; `align` defaults to `"start"`; optional `fill`/`border` draw a backdrop behind the arranged children. Stack containers may declare `localState`; stack and grid containers both accept `visibleWhen` and `role`. */
  export type StackProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; fill?: WidgetColor; border?: BorderProp; localState?: { scope: string; cells: Record<string, CellInit> }; visibleWhen?: Predicate; role?: WidgetRole };
  /** Props for `Grid`. `cols` is required and must be an integer >= 1. Children flow row-major across columns; grid currently has no backdrop fill/border and no `localState`. */
  export type GridProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; cols: number; visibleWhen?: Predicate; role?: WidgetRole };

  /** A vertical stack (`vstack`): `children` is a POSITIONAL second argument. */
  export function VStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** A horizontal stack (`hstack`): `children` is a POSITIONAL second argument. */
  export function HStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** A `grid` container: flows `children` across `cols` columns. `children` is a POSITIONAL second argument. */
  export function Grid(props: GridProps, children?: WidgetDescriptor[]): WidgetDescriptor;

  /** Tree viewport anchor. Valid values: `"topLeft"`, `"top"`, `"topRight"`, `"left"`, `"center"`, `"right"`, `"bottomLeft"`, `"bottom"`, `"bottomRight"`. */
  export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight";
  /** Tree input behavior. `"capture"` makes this tree consume UI input and freeze lower modal layers; `"passthrough"` is the HUD/default mode and lets game input continue. */
  export type WidgetCaptureMode = "capture" | "passthrough";
  /** Placement envelope props for `Tree`. `anchor` + `offset` position the root in 1280x720 logical UI space; `captureMode`, `initialFocus`, and `textEntryTarget` control modal/input behavior. */
  export type TreeProps = { anchor: WidgetAnchor; offset: [number, number]; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: WritableStateRef<string>; accessibleName?: string; role?: WidgetRole };
  /** The flat `AnchoredTree` envelope produced by `Tree(...)` and stored in UI registries. `textEntryTarget` is serialized to its dotted state-slot name. */
  export type AnchoredTreeDescriptor = { anchor: WidgetAnchor; offset: [number, number]; root: WidgetDescriptor; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: string; accessibleName?: string; role?: WidgetRole };
  /** Wrap a root widget descriptor in the `AnchoredTree` placement envelope. `root` is a POSITIONAL second argument. */
  export function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor;
  /** Props accepted by `defineUiTree`. The returned object preserves the runtime manifest entry shape `{ name, tree, alwaysOn? }`. */
  export type UiTreeRegistrationProps<Name extends string = string> = { name: Name; tree: AnchoredTreeDescriptor; alwaysOn?: boolean };
  /** A typed UI-tree registration entry for `ModManifest.uiTrees` / `setupLevel().uiTrees`. */
  export type UiTreeRegistration<Name extends string = string> = ModUiTree & { readonly name: Name };
  /** Pure helper for defining a named UI-tree registration. Registration still happens only when the returned object is included in a manifest `uiTrees` array. */
  export function defineUiTree<const Name extends string>(registration: UiTreeRegistrationProps<Name>): UiTreeRegistration<Name>;

  /** Options accepted by `bindState` for each state value type. Numbers may format and tween, numeric arrays may color-tween, and scalar strings/booleans may format. */
  export type StateBindOptionsFor<T> =
    T extends number ? { format?: string; tween?: NumberTween; slot?: never; local?: never } :
    T extends NumericArrayStateValue ? { tween?: ColorTween; slot?: never; local?: never } :
    T extends ScalarStateValue ? { format?: string; slot?: never; local?: never } :
    never;
  /** Compose bind-only options onto a state ref, emitting `{ slot, ...options }`. */
  export function bindState<T>(ref: ReadonlyStateRef<T>): ReadonlyStateRef<T>;
  export function bindState<T, Options extends StateBindOptionsFor<T>>(ref: ReadonlyStateRef<T>, options: Options): ReadonlyStateRef<T> & Omit<Options, "slot" | "local">;
  /** Build `{ slot, equals }` for scalar state refs. */
  export function stateEquals<T extends PredicateValue>(ref: ReadonlyStateRef<T>, value: T): Predicate;

  /** A presentation-cell initial value (`CellInit` wire shapes). */
  type CellInit = number | boolean | string | [number, number, number, number];
  /** A presentation-cell handle (`ui.createLocalState`): `.get()` yields a `{ local }` bind ref; `.set(v)` emits a `cellWrite` reaction (NEVER `setState`); `.is(v)` produces an equality `Predicate` (comparand typed to the cell's `T`). Presentation-only. */
  export type LocalStateHandle<T extends CellInit> = { get(): LocalBindRef; set(value: T): PrimitiveReactionDescriptor; is(value: T): Predicate };
  /** The `{ scope, cells }` bundle `ui.createLocalState` returns: splice `scope` onto the declaring container's `localState`; bind widgets to `cells.<name>.get()`. */
  export type LocalStateBundle<I extends Record<string, CellInit>> = { scope: { scope: string; cells: I }; cells: { [K in keyof I]: LocalStateHandle<I[K]> } };
  /** Declare a presentation-cell scope (M13 G1b). SDK-lib function, not a registered primitive. Pure: no engine side effect. `.set()` emits `cellWrite`, never writing the authoritative store. */
  export function createLocalState<I extends Record<string, CellInit>>(init: I): LocalStateBundle<I>;
  /** `Switch(cell, map)` (M13 G2) — expand a string-valued cell's `map` of `value → subtree` into an array, injecting `visibleWhen: cell.is(key)` onto each subtree in LEXICOGRAPHICALLY-SORTED key order (byte-identical TS/Luau). Splice the result into a container's `children`. */
  export function Switch(cell: LocalStateHandle<string>, map: Record<string, WidgetDescriptor>): WidgetDescriptor[];
  /** State-helper namespace (state helpers are namespaced; reactions stay bare). */
  export const ui: { createLocalState: typeof createLocalState };

  /** Pure identity builder for entity-type descriptors. Returned from `ModManifest.entities`; `descriptor` is the full archetype object: optional `canonicalName`, optional `defaultWeapon`, and optional component presets. */
  export function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor;
  /** Pure identity builder for the mod manifest consumed from the default export. `config.name` is required; optional arrays include `entities`, `maps`, `uiTrees`, `reactions`, `crossings`, and `stores`. */
  export function defineMod(config: ModManifest): ModManifest;
  /** Pure identity builder for a mod map catalog. Entries require `id`, `path`, and `name`; optional `tags` default to empty and drive filtering plus `levels` selectors. */
  export function defineMapCatalog(entries: ModMapEntry[]): ModMapEntry[];

  // -------------------------------------------------------------------------
  // Runtime-value vocabulary — the typed command buffer (scripting.md §11). The
  // `runtime.*` builders assemble these node objects as plain data; constructing
  // a node has no FFI side effect. The union below is the *closure* of the
  // vocabulary: an author cannot name an op outside it. Field names match the
  // Rust `IrNode` wire format byte-for-byte (`a`/`b`, `x`/`lo`/`hi`, `cond`,
  // `name`, `value`) so builder output deserializes straight into `IrNode`.
  // (Author surface is `runtime`/`RuntimeValue`; the Rust substrate and wire
  // op tags keep the `ir` names — scripting.md §11, "Author-facing naming".)
  // Source of truth: crates/postretro/src/scripting/ir/mod.rs + sdk/lib/runtime.ts.
  // Static block (not registry-emitted): `register_tagged_union` /
  // `TypeShape::TaggedUnion` renders one payload *type name* per variant under
  // a fixed tag key — it cannot express per-variant inline struct fields (e.g.
  // `value`, `a`/`b`, `cond`) or the recursive `RuntimeValue` self-reference
  // that every non-leaf variant requires.

  /** Literal scalar leaf: `{ op: "const", value }`. `value` is a number or boolean. */
  export type RuntimeConst = { op: "const"; value: number | boolean };
  /** Named-input leaf: `{ op: "input", name }`. Bound to live state by the Rust evaluator. */
  export type RuntimeRead = { op: "input"; name: string };
  /** Addition: `a + b` (number). */
  export type RuntimeAdd = { op: "add"; a: RuntimeValue; b: RuntimeValue };
  /** Subtraction: `a - b` (number). */
  export type RuntimeSub = { op: "sub"; a: RuntimeValue; b: RuntimeValue };
  /** Multiplication: `a * b` (number). */
  export type RuntimeMul = { op: "mul"; a: RuntimeValue; b: RuntimeValue };
  /** Division: `a / b` (number). */
  export type RuntimeDiv = { op: "div"; a: RuntimeValue; b: RuntimeValue };
  /** Clamp `x` to `[lo, hi]` (number). */
  export type RuntimeClamp = { op: "clamp"; x: RuntimeValue; lo: RuntimeValue; hi: RuntimeValue };
  /** Linear interpolation between `a` and `b` by `t` (number). */
  export type RuntimeLerp = { op: "lerp"; a: RuntimeValue; b: RuntimeValue; t: RuntimeValue };
  /** Less-than comparison (boolean). */
  export type RuntimeLt = { op: "lt"; a: RuntimeValue; b: RuntimeValue };
  /** Less-than-or-equal comparison (boolean). */
  export type RuntimeLe = { op: "le"; a: RuntimeValue; b: RuntimeValue };
  /** Greater-than comparison (boolean). */
  export type RuntimeGt = { op: "gt"; a: RuntimeValue; b: RuntimeValue };
  /** Greater-than-or-equal comparison (boolean). */
  export type RuntimeGe = { op: "ge"; a: RuntimeValue; b: RuntimeValue };
  /** Equality comparison (boolean). */
  export type RuntimeEq = { op: "eq"; a: RuntimeValue; b: RuntimeValue };
  /** Inequality comparison (boolean). */
  export type RuntimeNe = { op: "ne"; a: RuntimeValue; b: RuntimeValue };
  /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
  export type RuntimeSelect = { op: "select"; cond: RuntimeValue; a: RuntimeValue; b: RuntimeValue };

  /** A node in the authored runtime-value tree. Closed vocabulary: every node
   * the evaluator accepts is one of these variants. New opcodes extend this
   * union in lockstep with the Rust `IrNode` enum. */
  export type RuntimeValue =
    | RuntimeConst
    | RuntimeRead
    | RuntimeAdd
    | RuntimeSub
    | RuntimeMul
    | RuntimeDiv
    | RuntimeClamp
    | RuntimeLerp
    | RuntimeLt
    | RuntimeLe
    | RuntimeGt
    | RuntimeGe
    | RuntimeEq
    | RuntimeNe
    | RuntimeSelect;

  /** A builder operand: an already-built node, or a bare `number`/`boolean`
   * literal that the builder auto-wraps into a `const` node. */
  type RuntimeOperand = RuntimeValue | number | boolean;

  /** Pure builder vocabulary for runtime values, installed as
   * `globalThis.runtime`. Every method returns a plain `RuntimeValue` object;
   * constructing a node has no FFI side effect. Bare `number`/`boolean`
   * operands are auto-wrapped into `const` nodes. Import via
   * `import { runtime } from "postretro"`. */
  export interface Runtime {
    /** Literal scalar leaf. `const` is reserved, so the builder is `constant`. */
    constant(value: number | boolean): RuntimeConst;
    /** Named-input leaf, bound to live state by name in the Rust evaluator. */
    read(name: string): RuntimeRead;
    /** `a + b` (number). */
    add(a: RuntimeOperand, b: RuntimeOperand): RuntimeAdd;
    /** `a - b` (number). */
    sub(a: RuntimeOperand, b: RuntimeOperand): RuntimeSub;
    /** `a * b` (number). */
    mul(a: RuntimeOperand, b: RuntimeOperand): RuntimeMul;
    /** `a / b` (number). */
    div(a: RuntimeOperand, b: RuntimeOperand): RuntimeDiv;
    /** Clamp `x` to `[lo, hi]` (number). */
    clamp(x: RuntimeOperand, lo: RuntimeOperand, hi: RuntimeOperand): RuntimeClamp;
    /** Linear interpolation between `a` and `b` by `t` (number). */
    lerp(a: RuntimeOperand, b: RuntimeOperand, t: RuntimeOperand): RuntimeLerp;
    /** `a < b` (boolean). */
    lt(a: RuntimeOperand, b: RuntimeOperand): RuntimeLt;
    /** `a <= b` (boolean). */
    le(a: RuntimeOperand, b: RuntimeOperand): RuntimeLe;
    /** `a > b` (boolean). */
    gt(a: RuntimeOperand, b: RuntimeOperand): RuntimeGt;
    /** `a >= b` (boolean). */
    ge(a: RuntimeOperand, b: RuntimeOperand): RuntimeGe;
    /** `a == b` (boolean). */
    eq(a: RuntimeOperand, b: RuntimeOperand): RuntimeEq;
    /** `a != b` (boolean). */
    ne(a: RuntimeOperand, b: RuntimeOperand): RuntimeNe;
    /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
    select(cond: RuntimeOperand, a: RuntimeOperand, b: RuntimeOperand): RuntimeSelect;
  }

  /** Runtime-value builder vocabulary global. */
  export const runtime: Runtime;

  // -------------------------------------------------------------------------
  // UI navigation intents — the closed gamepad-first nav vocabulary the input
  // stage produces (keyboard arrows/enter/escape, D-pad, stick edges) and that
  // UI authors reference in `capturesNav` and focus policy. Wire names mirror
  // the Rust `NavIntent` enum (input/ui_nav.rs). Template-literal-typed so a
  // typo in a `"nav.*"` string is a compile error.
  // See: context/research/ui-layer.md §16.

  /** The bare nav-intent names without the `nav.` prefix. */
  export type NavIntentName =
    | "up" | "down" | "left" | "right"
    | "next" | "prev"
    | "confirm" | "cancel"
    | "menu" | "options";

  /** A UI navigation intent wire name. Template-literal type over the closed
   * `NavIntentName` set, so only `"nav.up"` … `"nav.options"` type-check. */
  export type NavIntent = `nav.${NavIntentName}`;
