

declare module "postretro/ui" {
  import type {
    ReadonlyStateRef,
    WritableStateRef,
    ScalarStateValue,
    NumericArrayStateValue,
    GameStateRefs,
    ModUiTree,
    PrimitiveReactionDescriptor,
    NamedReactionDescriptor,
    CrossingCondition,
    CrossingDescriptor,
  } from "postretro";

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
  export type ThemeTokenTree<Leaf> = { readonly [key: string]: Leaf | ThemeTokenTree<Leaf> };
  /** Nested singular token groups accepted by `defineTheme`. */
  export type ThemeDefinition = {
    readonly color?: ThemeTokenTree<ThemeColorValue>;
    readonly font?: ThemeTokenTree<string>;
    readonly spacing?: ThemeTokenTree<number>;
    readonly colors?: never;
    readonly fonts?: never;
    readonly tokens?: never;
  };
  export type JoinThemePath<Prefix extends string, Key extends string> = Prefix extends "" ? Key : `${Prefix}.${Key}`;
  export type FlattenTokenKeys<Tree, Leaf, Prefix extends string = ""> = Tree extends Leaf
    ? Prefix
    : Tree extends Readonly<Record<string, unknown>>
      ? { [K in Extract<keyof Tree, string>]: FlattenTokenKeys<Tree[K], Leaf, JoinThemePath<Prefix, K>> }[Extract<keyof Tree, string>]
      : never;
  export type FlatTokenMap<Tree, Leaf, Value> = Record<FlattenTokenKeys<NonNullable<Tree>, Leaf>, Value>;
  export type DesignTokenTree<Tree, Leaf, Token, Prefix extends string = ""> = Tree extends Leaf
    ? Token
    : Tree extends Readonly<Record<string, unknown>>
      ? { readonly [K in Extract<keyof Tree, string>]: DesignTokenTree<Tree[K], Leaf, Token, JoinThemePath<Prefix, K>> }
      : never;
  export type DesignTokenGroup<Tree, Leaf, Token> = [Tree] extends [undefined] ? {} : DesignTokenTree<NonNullable<Tree>, Leaf, Token>;
  export type DesignTokens<T extends ThemeDefinition> = {
    readonly color: DesignTokenGroup<T["color"], ThemeColorValue, ColorToken>;
    readonly font: DesignTokenGroup<T["font"], string, FontToken>;
    readonly spacing: DesignTokenGroup<T["spacing"], number, SpacingToken>;
  };
  declare const definedThemeBrand: unique symbol;
  /** Manifest-compatible flat theme maps returned from `defineTheme`. */
  export type DefinedTheme<T extends ThemeDefinition> = {
    readonly colors: FlatTokenMap<T["color"], ThemeColorValue, ThemeColorValue>;
    readonly fonts: FlatTokenMap<T["font"], string, string>;
    readonly spacing: FlatTokenMap<T["spacing"], number, number>;
    readonly [definedThemeBrand]: T;
  };
  export function defineTheme<const T extends ThemeDefinition>(theme: T): DefinedTheme<T>;
  export function getDesignTokens<const T extends ThemeDefinition>(theme: DefinedTheme<T>): DesignTokens<T>;

  export type LocalizedText = string;
  export type WidgetColor = [number, number, number, number] | ColorToken;
  export type WidgetSpacing = number | SpacingToken;
  export type WidgetAlign = "start" | "center" | "end" | "stretch";
  export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut";
  export type NumberTween = { durationMs: number; easing: WidgetEasing; from?: number };
  export type ColorTween = { durationMs: number; easing: WidgetEasing; from?: [number, number, number, number] };
  export type LocalBindRef = { local: string };
  export type PredicateValue = number | boolean | string;
  export type Predicate = ((ReadonlyStateRef<PredicateValue> & { local?: never }) | LocalBindRef) & { equals?: PredicateValue };
  export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none";
  export type AnnouncePriority = "polite" | "assertive";
  export type TextBindProp = ((ReadonlyStateRef<ScalarStateValue> & { local?: never }) | LocalBindRef) & { format?: string; tween?: NumberTween };
  export type PanelBindProp = ((ReadonlyStateRef<NumericArrayStateValue> & { local?: never; format?: never }) | LocalBindRef) & { tween?: ColorTween };
  export type SliderBindProp = ((WritableStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  export type BarBindProp = ((ReadonlyStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  export type BarMaxProp = number | ReadonlyStateRef<number>;
  export type StyleRangeEntry = { upTo?: number; color?: WidgetColor; pulse?: { periodMs: number }; flash?: { durationMs: number } };
  export type StyleRangesProp = { max: number; entries: StyleRangeEntry[] };
  export type BorderProp = { texture: string; slice: [number, number, number, number]; tint: WidgetColor };
  export type FocusNeighborsProp = { up?: string; down?: string; left?: string; right?: string };
  export type RepeatPolicyProp = { initialDelayMs: number; intervalMs: number };
  export type ReactionHandleRef = { name: string };
  export type WidgetDescriptor = { kind: string; [field: string]: unknown };

  /** Props for `Text`. `content` is the fallback/display string; `fontSize` is a finite logical-px number defaulting to 12; `color` is an RGBA tuple or color token defaulting to white. `bind` may replace rendered content from state; `styleRanges` recolors by normalized value. */
  export type TextProps = { content: LocalizedText; fontSize?: number; color?: WidgetColor; font?: FontToken; bind?: TextBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** Build a `text` widget descriptor. Pure: returns data retained by Rust after manifest/setup load. */
  export function Text(props: TextProps): WidgetDescriptor;
  /** Props for `Panel`. `fill` is required RGBA/token color; `border` is optional 9-slice data; `bind` may replace fill from a numeric RGBA state value. */
  export type PanelProps = { fill: WidgetColor; border?: BorderProp; bind?: PanelBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** Build a solid panel widget descriptor. Pure; no engine side effect. */
  export function Panel(props: PanelProps): WidgetDescriptor;
  /** Props for `Image`. `asset` is a UI texture key. Exactly one accessible-name path is required: `label` for meaningful images or `decorative: true` for ignored imagery. */
  export type ImageProps = { asset: string; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: string; decorative?: never } | { decorative: true; label?: never });
  /** Build an image widget descriptor sized from the texture asset's natural dimensions. */
  export function Image(props: ImageProps): WidgetDescriptor;
  /** Props for `Spacer`. `flexGrow` is a finite proportional share of leftover space; defaults to 1. */
  export type SpacerProps = { flexGrow?: number; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** Build a spacer widget descriptor. */
  export function Spacer(props?: SpacerProps): WidgetDescriptor;
  /** Props for `Button`. `id` is required for focus/activation. `onPress` accepts a `defineReaction` handle, bare reaction name, or reserved `ui.*` action. Exactly one of `label` or `labelledBy` is required. */
  export type ButtonProps = { id: string; onPress: ReactionHandleRef | string; repeatOnHold?: RepeatPolicyProp; focusNeighbors?: FocusNeighborsProp; selected?: Predicate; checked?: Predicate; bind?: Predicate; styleRanges?: StyleRangesProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** Build an interactive button descriptor. Pure; activation is resolved by the app at runtime. */
  export function Button(props: ButtonProps): WidgetDescriptor;
  /** Props for `Slider`. `bind` must be writable numeric state/local cell. `min`, `max`, and `step` are finite numbers; navigation clamps writes into `[min, max]`. Exactly one of `label` or `labelledBy` is required. */
  export type SliderProps = { id: string; bind: SliderBindProp; min: number; max: number; step: number; capturesNav?: string[]; focusNeighbors?: FocusNeighborsProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** Build an interactive slider descriptor. */
  export function Slider(props: SliderProps): WidgetDescriptor;
  /** Props for `Bar`. `bind` is readonly numeric state/local value; `max` is a finite number or readonly numeric state ref; `fill` and `background` are RGBA/token colors. */
  export type BarProps = { bind: BarBindProp; max: BarMaxProp; fill: WidgetColor; background: WidgetColor; styleRanges?: StyleRangesProp; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** Build a passive bar descriptor. Displayed fill is `value / max` clamped to `[0, 1]`. */
  export function Bar(props: BarProps): WidgetDescriptor;
  /** Props for `Announce`. `priority` defaults to `"polite"`; `visibleWhen` gates whether the live-region message is active. */
  export type AnnounceProps = { priority?: AnnouncePriority; visibleWhen?: Predicate };
  /** Build a non-visual live-region announcement. `text` is positional display text. */
  export function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor;

  export type FocusKind = "linear" | "spatial";
  export type FocusPolicyProp = FocusKind | { policy: FocusKind; wrap?: boolean; repeat?: RepeatPolicyProp };
  /** Props for `VStack`/`HStack`. `gap`/`padding` default to 0, `align` defaults to `"start"`, and optional `localState` declares presentation-only cells scoped to this container. */
  export type StackProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; fill?: WidgetColor; border?: BorderProp; localState?: { scope: string; cells: Record<string, CellInit> }; visibleWhen?: Predicate; role?: WidgetRole };
  /** Props for `Grid`. `cols` is required and must be an integer >= 1; children flow row-major. */
  export type GridProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; cols: number; visibleWhen?: Predicate; role?: WidgetRole };
  /** Build a vertical stack descriptor. `children` is positional, not a prop. */
  export function VStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** Build a horizontal stack descriptor. `children` is positional, not a prop. */
  export function HStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** Build a grid descriptor. `children` is positional, not a prop. */
  export function Grid(props: GridProps, children?: WidgetDescriptor[]): WidgetDescriptor;

  export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight";
  export type WidgetCaptureMode = "capture" | "passthrough";
  /** Props for `Tree`. `anchor` and `offset` place the root in 1280x720 logical UI space. `captureMode` defaults to `"passthrough"`; `initialFocus` names a widget id; `textEntryTarget` is a writable string state ref. */
  export type TreeProps = { anchor: WidgetAnchor; offset: [number, number]; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: WritableStateRef<string>; accessibleName?: string; role?: WidgetRole };
  export type AnchoredTreeDescriptor = { anchor: WidgetAnchor; offset: [number, number]; root: WidgetDescriptor; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: string; accessibleName?: string; role?: WidgetRole };
  /** Wrap a root widget in an anchored tree placement envelope. Pure; registration happens through `defineUiTree` and manifest data. */
  export function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor;
  /** Props accepted by `defineUiTree`. `name` is the registry key; `tree` is from `Tree`; `alwaysOn` renders as a base layer such as HUD. */
  export type UiTreeRegistrationProps<Name extends string = string> = { name: Name; tree: AnchoredTreeDescriptor; alwaysOn?: boolean };
  export type UiTreeRegistration<Name extends string = string> = ModUiTree & { readonly name: Name };
  /** Build a UI-tree registration object. Pure; include the result in `ModManifest.uiTrees` or `setupLevel().uiTrees` to register it. */
  export function defineUiTree<const Name extends string>(registration: UiTreeRegistrationProps<Name>): UiTreeRegistration<Name>;

  export type StateBindOptionsFor<T> =
    T extends number ? { format?: string; tween?: NumberTween; slot?: never; local?: never } :
    T extends NumericArrayStateValue ? { tween?: ColorTween; slot?: never; local?: never } :
    T extends ScalarStateValue ? { format?: string; slot?: never; local?: never } :
    never;
  /** Compose bind-only options onto a state ref. Pure; it emits `{ slot, ...options }` for widget props and never reads live state. */
  export function bindState<T>(ref: ReadonlyStateRef<T>): ReadonlyStateRef<T>;
  export function bindState<T, Options extends StateBindOptionsFor<T>>(ref: ReadonlyStateRef<T>, options: Options): ReadonlyStateRef<T> & Omit<Options, "slot" | "local">;
  /** Build a scalar equality predicate for `visibleWhen`, `selected`, or `checked`. */
  export function stateEquals<T extends PredicateValue>(ref: ReadonlyStateRef<T>, value: T): Predicate;
  type CellInit = number | boolean | string | [number, number, number, number];
  export type LocalStateHandle<T extends CellInit> = { get(): LocalBindRef; set(value: T): PrimitiveReactionDescriptor; is(value: T): Predicate };
  export type LocalStateBundle<I extends Record<string, CellInit>> = { scope: { scope: string; cells: I }; cells: { [K in keyof I]: LocalStateHandle<I[K]> } };
  /** Declare presentation-local cells. `init` keys are cell names; values may be number, boolean, string, or RGBA tuple. Pure; cells live only inside the nearest container using the returned `scope`. */
  export function createLocalState<I extends Record<string, CellInit>>(init: I): LocalStateBundle<I>;
  export function Switch(cell: LocalStateHandle<string>, map: Record<string, WidgetDescriptor>): WidgetDescriptor[];
  export const ui: { createLocalState: typeof createLocalState };
  export function getGameState(): GameStateRefs;

  /** Build a state-crossing watcher for numeric refs. `condition` gives exactly one finite `below` or `above` threshold; optional `max` is a finite denominator. `fire` accepts reaction handles or names. */
  export function onStateCrossing(ref: ReadonlyStateRef<number>, condition: CrossingCondition, fire: (NamedReactionDescriptor | string)[]): CrossingDescriptor;
  /** Play `sound` on optional mixer `bus`; omitted/null bus uses the engine default. */
  export function playSound(sound: string, bus?: string | null): PrimitiveReactionDescriptor;
  /** Trigger gamepad rumble. `strong` and optional `weak` are motor intensities in [0, 1]; `durationMs` is milliseconds. */
  export function rumble(strong: number, durationMs: number, weak?: number | null): PrimitiveReactionDescriptor;
  /** Flash the screen with linear RGBA `color`; `durationMs` is the decay time in milliseconds. */
  export function flashScreen(color: [number, number, number, number], durationMs: number): PrimitiveReactionDescriptor;
  /** Apply a screen-edge vignette. `strength` is the peak amount, `durationMs` is total rise+decay time, and optional `color` is linear RGB. */
  export function vignette(strength: number, durationMs: number, color?: [number, number, number] | null): PrimitiveReactionDescriptor;
  /** Shake the screen. `amplitude` is logical-reference px, `durationMs` is milliseconds, and optional `frequency` is Hz. */
  export function screenShake(amplitude: number, durationMs: number, frequency?: number | null): PrimitiveReactionDescriptor;
  /** Push UI tree `tree` as a modal; optional `onCommit` names a reaction fired on commit. Unknown tree names warn and no-op. */
  export function showDialog(tree: string, onCommit?: string | null): PrimitiveReactionDescriptor;
  export const KEYBOARD_TREE: "keyboard";
  export const CLOSE_DIALOG_ACTION: "ui.closeDialog";
  export const EXIT_TO_DESKTOP_ACTION: "ui.exitToDesktop";
  /** Reserved `Button.onPress` action for returning to the frontend; same lifecycle path as `returnToFrontend()`. */
  export const QUIT_TO_MENU_ACTION: "ui.quitToMenu";
  /** Open the engine keyboard modal. Optional `onCommit` names a reaction fired when text entry commits. */
  export function openTextEntry(onCommit?: string | null): PrimitiveReactionDescriptor;
  /** Push a menu tree by registry name. Unknown tree names warn and no-op. */
  export function openMenu(tree: string): PrimitiveReactionDescriptor;
  /** Pop the active modal. Empty stack warns and no-ops. */
  export function closeDialog(): PrimitiveReactionDescriptor;
  /** Queue a catalog map load. `id` must match a committed `ModMapEntry.id`; unknown ids warn and no-op. */
  export function loadLevel(id: string): PrimitiveReactionDescriptor;
  /** Reload the active level from retained catalog id or raw dev path. No active level means no-op. */
  export function restartLevel(): PrimitiveReactionDescriptor;
  /** Return to the frontend menu and reload its optional backdrop level. */
  export function returnToFrontend(): PrimitiveReactionDescriptor;
  /** Write `value` to a writable state ref at game-logic time; runtime validates type/range and rejects readonly slots. */
  export function updateState<T>(ref: WritableStateRef<T>, value: T): PrimitiveReactionDescriptor;
  export function appendText(ref: WritableStateRef<string>, text: string): PrimitiveReactionDescriptor;
  export function backspaceText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor;
  export function clearText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor;
}
