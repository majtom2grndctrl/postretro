// UI widget factories: capitalized constructors for the seven non-container
// widget kinds — Text, Panel, Image, Button, Slider, Bar, Spacer.
// (Containers — VStack/HStack/Grid — live in `./layout`.) Each mirrors the
// `emitter()` precedent: a `Props` object validated synchronously, throwing a
// field-named `Error`, returning a plain descriptor object whose keys are the
// camelCase wire form of the matching `render/ui/descriptor.rs` `Widget` variant.
//
// Pure builders: constructing a widget has no engine side effect — the FFI
// boundary is the eventual `return` of the authored tree. Bound props accept
// state-reference descriptors (`{ slot }`) or presentation-local bind objects;
// `Button.onPress` accepts a reaction handle or a bare name string.
// See: context/lib/ui.md · context/lib/scripting.md §7

import type { LocalizedText } from "./text";

// --- Shared wire-shape aliases ----------------------------------------------

/**
 * A widget color slot: an inline linear-RGBA tuple (`[r, g, b, a]`, 0–1) or a
 * theme-token name resolved against the active theme. Mirrors `descriptor.rs`
 * `ColorValue` (untagged: bare array or bare string).
 */
export type WidgetColor = [number, number, number, number] | string;

/**
 * A spacing slot (gap/padding): an inline logical-px number or a theme-token
 * name. Mirrors `descriptor.rs` `SpacingValue` (untagged: bare number or string).
 */
export type WidgetSpacing = number | string;

/** Cross-axis alignment of a container's children. Mirrors `Align`. */
export type WidgetAlign = "start" | "center" | "end" | "stretch";

/** Easing curve for a value tween. Mirrors `descriptor.rs` `Easing`. */
export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut";

declare const stateRefValueBrand: unique symbol;
declare const writableStateRefBrand: unique symbol;

export type ScalarStateValue = number | boolean | string;
export type NumericArrayStateValue = ReadonlyArray<number>;

/** Readable authoritative state reference. Runtime shape is exactly `{ slot }`. */
export type ReadonlyStateRef<T> = {
  readonly slot: string;
  readonly [stateRefValueBrand]: T;
};

/** Writable authoritative state reference. The writable marker is type-only. */
export type WritableStateRef<T> = ReadonlyStateRef<T> & {
  readonly [writableStateRefBrand]: T;
};

/**
 * Value-tween config for a text/slider/bar bind (number shape). Mirrors
 * `descriptor.rs` `TextTween`: eases the resolved numeric value toward each new
 * target over `durationMs` using `easing`. `from` is the optional explicit start
 * value for the first tween.
 */
export type NumberTween = { durationMs: number; easing: WidgetEasing; from?: number };

/**
 * Value-tween config for a panel bind (color shape). Mirrors `descriptor.rs`
 * `PanelTween`: differs from `NumberTween` only in `from`'s type (a length-4
 * linear-RGBA array).
 */
export type ColorTween = {
  durationMs: number;
  easing: WidgetEasing;
  from?: [number, number, number, number];
};

/**
 * A `{ local }` presentation-cell bind reference: the `.get()` result of a
 * `ui.createLocalState()` handle. Names a cell on the nearest declaring
 * `localState` scope; resolved app-side against the cell store, never the
 * authoritative slot table. Mirrors `descriptor.rs` `BindSource::Local`.
 */
export type LocalBindRef = { local: string };

/**
 * A scalar comparand for a `Predicate` (M13 G2): a number, boolean, or string.
 * Mirrors `descriptor.rs` `PredicateValue`. An array comparand is unrepresentable.
 */
export type PredicateValue = number | boolean | string;

/**
 * A reactive predicate (M13 G2): a `{ slot }` store source or `{ local }` cell
 * source read against an optional `equals` comparand. Mirrors `descriptor.rs`
 * `Predicate`. Constructed by `LocalStateHandle.is(v)` / `stateEquals(ref, v)`;
 * the comparand there is typed to the cell/slot value type.
 */
export type Predicate = (
  | (ReadonlyStateRef<PredicateValue> & { local?: never })
  | LocalBindRef
) & {
  equals?: PredicateValue;
};

/** A11y role override (M13 G2). Mirrors `descriptor.rs` `Role` (camelCase). */
export type WidgetRole =
  | "tab"
  | "tablist"
  | "checkbox"
  | "radio"
  | "listitem"
  | "button"
  | "slider"
  | "progressbar"
  | "image"
  | "group"
  | "none";

/** Live-region announcement urgency (M13 G2). `"polite"` round-trips to omission. */
export type AnnouncePriority = "polite" | "assertive";

/**
 * State binding for a `text` widget. The source is either a `{ slot }`
 * authoritative state reference or a `{ local }` presentation-cell binding;
 * `format` is an optional one-`{}` template; `tween` eases the resolved numeric
 * value. Mirrors `descriptor.rs` `TextBind`.
 */
export type TextBindProp = (
  | (ReadonlyStateRef<ScalarStateValue> & { local?: never })
  | LocalBindRef
) & {
  format?: string;
  tween?: NumberTween;
};

/**
 * State binding for a `panel` widget. The source resolves a length-4 linear-RGBA
 * fill from a `{ slot }` store binding or a `{ local }` cell; `tween` eases the
 * resolved color. Mirrors `descriptor.rs` `PanelBind`.
 */
export type PanelBindProp = (
  | (ReadonlyStateRef<NumericArrayStateValue> & { local?: never; format?: never })
  | LocalBindRef
) & {
  tween?: ColorTween;
};

/**
 * State binding for a `slider` (a writable numeric `{ slot }` or `{ local }`
 * source + optional number-shape tween). Mirrors `descriptor.rs` `SliderBind`.
 */
export type SliderBindProp = (
  | (WritableStateRef<number> & { local?: never; format?: never })
  | LocalBindRef
) & {
  tween?: NumberTween;
};

export type BarBindProp = (
  | (ReadonlyStateRef<number> & { local?: never; format?: never })
  | LocalBindRef
) & {
  tween?: NumberTween;
};

export type BarMaxProp = number | ReadonlyStateRef<number>;

/** One band in a `styleRanges` map. Mirrors `descriptor.rs` `StyleEntry`. */
export type StyleRangeEntry = {
  upTo?: number;
  color?: WidgetColor;
  pulse?: { periodMs: number };
  flash?: { durationMs: number };
};

/**
 * Continuous value→style map (text/panel/bar). `value/max` matches the first
 * covering band; a trailing no-`upTo` band is the default. Mirrors
 * `descriptor.rs` `StyleRanges`.
 */
export type StyleRangesProp = { max: number; entries: StyleRangeEntry[] };

/** 9-slice border descriptor. Mirrors `descriptor.rs` `Border`. */
export type BorderProp = {
  texture: string;
  slice: [number, number, number, number];
  tint: WidgetColor;
};

/**
 * Per-direction focus-neighbor overrides. Each set direction names the node id
 * focus jumps to. Mirrors `descriptor.rs` `FocusNeighbors` (camelCase keys).
 */
export type FocusNeighborsProp = {
  up?: string;
  down?: string;
  left?: string;
  right?: string;
};

/**
 * Hold-to-repeat timing (shared by container nav repeat and a button's
 * `repeatOnHold`). Mirrors `descriptor.rs` `RepeatPolicy`.
 */
export type RepeatPolicyProp = { initialDelayMs: number; intervalMs: number };

/**
 * A typed reaction handle (`defineReaction` result) — anything carrying a `.name`
 * string. `Button.onPress` accepts one of these or a bare name string.
 */
export type ReactionHandleRef = { name: string };

/**
 * The flat descriptor a widget factory produces: a `kind`-tagged object whose
 * sibling keys are the camelCase wire payload. Containers add a positional
 * `children` array.
 */
export type WidgetDescriptor = { kind: string; [field: string]: unknown };

// --- Internal validation helpers --------------------------------------------

function requireObject(props: unknown, factory: string): void {
  if (props === null || typeof props !== "object") {
    throw new Error(`${factory}: props must be an object`);
  }
}

function requireString(value: unknown, field: string, factory: string): void {
  if (typeof value !== "string") {
    throw new Error(`${factory}: \`${field}\` must be a string`);
  }
}

function requireNonemptyString(value: unknown, field: string, factory: string): void {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${factory}: \`${field}\` must be a nonempty string`);
  }
}

function requireFiniteNumber(value: unknown, field: string, factory: string): void {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${factory}: \`${field}\` must be a finite number`);
  }
}

function requireColor(value: unknown, field: string, factory: string): void {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error(`${factory}: \`${field}\` color token must be a nonempty string`);
    }
    return;
  }
  if (!Array.isArray(value) || value.length !== 4) {
    throw new Error(
      `${factory}: \`${field}\` must be a [r, g, b, a] tuple or a theme-token string`,
    );
  }
  for (let i = 0; i < 4; i++) {
    const c = value[i];
    if (typeof c !== "number" || !Number.isFinite(c)) {
      throw new Error(`${factory}: \`${field}\` color element ${i} is not a finite number`);
    }
  }
}

function requireSpacing(value: unknown, field: string, factory: string): void {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error(`${factory}: \`${field}\` spacing token must be a nonempty string`);
    }
    return;
  }
  requireFiniteNumber(value, field, factory);
}

function validateEasing(value: unknown, field: string, factory: string): void {
  const ok = value === "linear" || value === "easeIn" || value === "easeOut" || value === "easeInOut";
  if (!ok) {
    throw new Error(
      `${factory}: \`${field}\` must be one of "linear" | "easeIn" | "easeOut" | "easeInOut"`,
    );
  }
}

/**
 * Resolve a bind prop to its wire `{ slot, ... }` form. `slot` comes from an
 * authoritative state reference; `{ local }` comes from a presentation cell.
 * Validates the optional `tween`; the panel path expects a color-shape `from`,
 * the number path a numeric `from`. Returns `undefined` when no bind was authored
 * so the factory omits the `bind` key (wire identity).
 */
function buildBind(
  bind: unknown,
  factory: string,
  kind: "text" | "panel" | "slider",
): { slot?: string; local?: string; format?: string; tween?: unknown } | undefined {
  if (bind === undefined) return undefined;
  if (bind === null || typeof bind !== "object") {
    throw new Error(`${factory}: \`bind\` must be an object`);
  }
  const b = bind as Record<string, unknown>;
  // The bind source is either a `{ slot }` store binding or a `{ local }`
  // presentation-cell binding. `slot` wins when present; the emitted wire object
  // carries exactly one of the two keys.
  const out: { slot?: string; local?: string; format?: string; tween?: unknown } =
    b.slot !== undefined
      ? (requireNonemptyString(b.slot, "bind.slot", factory), { slot: b.slot as string })
      : (requireNonemptyString(b.local, "bind.local", factory), { local: b.local as string });

  if (kind === "text" && b.format !== undefined) {
    requireString(b.format, "bind.format", factory);
    out.format = b.format as string;
  }

  if (b.tween !== undefined) {
    const t = b.tween;
    if (t === null || typeof t !== "object") {
      throw new Error(`${factory}: \`bind.tween\` must be an object`);
    }
    const tw = t as Record<string, unknown>;
    requireFiniteNumber(tw.durationMs, "bind.tween.durationMs", factory);
    validateEasing(tw.easing, "bind.tween.easing", factory);
    const tween: Record<string, unknown> = { durationMs: tw.durationMs, easing: tw.easing };
    if (tw.from !== undefined) {
      if (kind === "panel") {
        requireColor(tw.from, "bind.tween.from", factory);
      } else {
        requireFiniteNumber(tw.from, "bind.tween.from", factory);
      }
      tween.from = tw.from;
    }
    out.tween = tween;
  }

  return out;
}

/**
 * Validate + clone a `styleRanges` prop, returning the wire object or
 * `undefined` when absent (so the factory omits the key). Shared by
 * text/panel/bar.
 */
function buildStyleRanges(value: unknown, factory: string): StyleRangesProp | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`styleRanges\` must be an object`);
  }
  const sr = value as Record<string, unknown>;
  requireFiniteNumber(sr.max, "styleRanges.max", factory);
  if (!Array.isArray(sr.entries)) {
    throw new Error(`${factory}: \`styleRanges.entries\` must be an array`);
  }
  const entries: StyleRangeEntry[] = sr.entries.map((raw, i) => {
    if (raw === null || typeof raw !== "object") {
      throw new Error(`${factory}: \`styleRanges.entries[${i}]\` must be an object`);
    }
    const e = raw as Record<string, unknown>;
    const out: StyleRangeEntry = {};
    if (e.upTo !== undefined) {
      requireFiniteNumber(e.upTo, `styleRanges.entries[${i}].upTo`, factory);
      out.upTo = e.upTo as number;
    }
    if (e.color !== undefined) {
      requireColor(e.color, `styleRanges.entries[${i}].color`, factory);
      out.color = e.color as WidgetColor;
    }
    if (e.pulse !== undefined) {
      const p = e.pulse as Record<string, unknown>;
      if (p === null || typeof p !== "object") {
        throw new Error(`${factory}: \`styleRanges.entries[${i}].pulse\` must be an object`);
      }
      requireFiniteNumber(p.periodMs, `styleRanges.entries[${i}].pulse.periodMs`, factory);
      out.pulse = { periodMs: p.periodMs as number };
    }
    if (e.flash !== undefined) {
      const f = e.flash as Record<string, unknown>;
      if (f === null || typeof f !== "object") {
        throw new Error(`${factory}: \`styleRanges.entries[${i}].flash\` must be an object`);
      }
      requireFiniteNumber(f.durationMs, `styleRanges.entries[${i}].flash.durationMs`, factory);
      out.flash = { durationMs: f.durationMs as number };
    }
    return out;
  });
  return { max: sr.max as number, entries };
}

/** Validate + clone an optional `focusNeighbors` prop, or `undefined` when empty. */
function buildFocusNeighbors(value: unknown, factory: string): FocusNeighborsProp | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`focusNeighbors\` must be an object`);
  }
  const fn = value as Record<string, unknown>;
  const out: FocusNeighborsProp = {};
  for (const dir of ["up", "down", "left", "right"] as const) {
    if (fn[dir] !== undefined) {
      requireNonemptyString(fn[dir], `focusNeighbors.${dir}`, factory);
      out[dir] = fn[dir] as string;
    }
  }
  return Object.keys(out).length === 0 ? undefined : out;
}

function buildRepeatPolicy(value: unknown, field: string, factory: string): RepeatPolicyProp {
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`${field}\` must be an object`);
  }
  const r = value as Record<string, unknown>;
  requireFiniteNumber(r.initialDelayMs, `${field}.initialDelayMs`, factory);
  requireFiniteNumber(r.intervalMs, `${field}.intervalMs`, factory);
  return { initialDelayMs: r.initialDelayMs as number, intervalMs: r.intervalMs as number };
}

const ROLES: ReadonlySet<string> = new Set([
  "tab",
  "tablist",
  "checkbox",
  "radio",
  "listitem",
  "button",
  "slider",
  "progressbar",
  "image",
  "group",
  "none",
]);

/** Validate an optional `role` prop, appending it to `out` when present. */
function applyRole(out: WidgetDescriptor, role: unknown, factory: string): void {
  if (role === undefined) return;
  if (typeof role !== "string" || !ROLES.has(role)) {
    throw new Error(
      `${factory}: \`role\` must be one of "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"`,
    );
  }
  out.role = role;
}

/**
 * Validate + clone a `Predicate` prop (`visibleWhen`/`selected`/`checked`, or a
 * Button `bind`), or `undefined` when absent. Source is `{ slot }` or `{ local }`;
 * `equals` is an optional scalar comparand. `slot` wins when present.
 */
function buildPredicate(value: unknown, field: string, factory: string): Predicate | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`${field}\` must be an object`);
  }
  const p = value as Record<string, unknown>;
  let out: Predicate;
  if (p.slot !== undefined) {
    requireNonemptyString(p.slot, `${field}.slot`, factory);
    out = { slot: p.slot as string };
  } else {
    requireNonemptyString(p.local, `${field}.local`, factory);
    out = { local: p.local as string } as Predicate;
  }
  if (p.equals !== undefined) {
    const e = p.equals;
    if (typeof e !== "number" && typeof e !== "boolean" && typeof e !== "string") {
      throw new Error(`${factory}: \`${field}.equals\` must be a number, boolean, or string`);
    }
    if (typeof e === "number" && !Number.isFinite(e)) {
      throw new Error(`${factory}: \`${field}.equals\` number must be finite`);
    }
    out.equals = e;
  }
  return out;
}

function buildBarMax(value: unknown, factory: string): number | { slot: string } {
  if (typeof value === "number") {
    requireFiniteNumber(value, "max", factory);
    return value;
  }
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`max\` must be a finite number or a state reference`);
  }
  const ref = value as Record<string, unknown>;
  requireNonemptyString(ref.slot, "max.slot", factory);
  return { slot: ref.slot as string };
}

/** Append an optional `visibleWhen` predicate + `role` shared by every widget. */
function applyA11yFields(
  out: WidgetDescriptor,
  props: { visibleWhen?: unknown; role?: unknown },
  factory: string,
): void {
  const visibleWhen = buildPredicate(props.visibleWhen, "visibleWhen", factory);
  if (visibleWhen !== undefined) out.visibleWhen = visibleWhen;
  applyRole(out, props.role, factory);
}

/** Append an optional `focusNeighbors`/`id` to a leaf widget descriptor in place. */
function applyFocusFields(
  out: WidgetDescriptor,
  props: { id?: string; focusNeighbors?: FocusNeighborsProp },
  factory: string,
): void {
  if (props.id !== undefined) {
    requireNonemptyString(props.id, "id", factory);
    out.id = props.id;
  }
  const neighbors = buildFocusNeighbors(props.focusNeighbors, factory);
  if (neighbors !== undefined) out.focusNeighbors = neighbors;
}

// --- Text -------------------------------------------------------------------

/** Props for `Text`. `content` is `LocalizedText`. `bind` is a `TextBindProp`. */
export type TextProps = {
  content: LocalizedText;
  fontSize?: number;
  color?: WidgetColor;
  font?: string;
  bind?: TextBindProp;
  styleRanges?: StyleRangesProp;
  id?: string;
  focusNeighbors?: FocusNeighborsProp;
  visibleWhen?: Predicate;
  role?: WidgetRole;
};

/**
 * A `text` leaf. `content` is the literal/fallback string; `fontSize` defaults to
 * 12 (logical px); `color` defaults to opaque white. An optional `bind` resolves
 * the rendered string from a store slot; `styleRanges` recolors by value.
 * Mirrors `descriptor.rs` `TextWidget`.
 */
export function Text(props: TextProps): WidgetDescriptor {
  requireObject(props, "Text");
  requireString(props.content, "content", "Text");
  const fontSize = props.fontSize ?? 12.0;
  requireFiniteNumber(fontSize, "fontSize", "Text");
  const color = props.color ?? [1.0, 1.0, 1.0, 1.0];
  requireColor(color, "color", "Text");

  const out: WidgetDescriptor = { kind: "text", content: props.content, fontSize, color };
  applyFocusFields(out, props, "Text");
  if (props.font !== undefined) {
    requireNonemptyString(props.font, "font", "Text");
    out.font = props.font;
  }
  const bind = buildBind(props.bind, "Text", "text");
  if (bind !== undefined) out.bind = bind;
  const styleRanges = buildStyleRanges(props.styleRanges, "Text");
  if (styleRanges !== undefined) out.styleRanges = styleRanges;
  applyA11yFields(out, props, "Text");
  return out;
}

// --- Panel ------------------------------------------------------------------

/** Props for `Panel`. `bind` is a `PanelBindProp` (color slot). */
export type PanelProps = {
  fill: WidgetColor;
  border?: BorderProp;
  bind?: PanelBindProp;
  styleRanges?: StyleRangesProp;
  id?: string;
  focusNeighbors?: FocusNeighborsProp;
  visibleWhen?: Predicate;
  role?: WidgetRole;
};

/**
 * A `panel` leaf: a solid `fill` (linear RGBA or token) with an optional 9-slice
 * `border`. An optional `bind` resolves the fill from a length-4 RGBA slot. A
 * border-less panel OMITS the `border` key (TS/Luau parity — the Luau lua→json
 * walker cannot carry an explicit `null`); the Rust `PanelWidget.border` is
 * `#[serde(default)]`, so an absent key deserializes to `None` and re-serializes
 * as `border: null`, byte-identical to the shipped fixtures. Mirrors `PanelWidget`.
 */
export function Panel(props: PanelProps): WidgetDescriptor {
  requireObject(props, "Panel");
  requireColor(props.fill, "fill", "Panel");

  const out: WidgetDescriptor = { kind: "panel", fill: props.fill };
  if (props.border !== undefined) {
    out.border = validateBorder(props.border, "Panel");
  }
  applyFocusFields(out, props, "Panel");
  const bind = buildBind(props.bind, "Panel", "panel");
  if (bind !== undefined) out.bind = bind;
  const styleRanges = buildStyleRanges(props.styleRanges, "Panel");
  if (styleRanges !== undefined) out.styleRanges = styleRanges;
  applyA11yFields(out, props, "Panel");
  return out;
}

/** Validate a 9-slice border prop, returning the wire object. */
export function validateBorder(value: unknown, factory: string): BorderProp {
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: \`border\` must be an object`);
  }
  const b = value as Record<string, unknown>;
  requireString(b.texture, "border.texture", factory);
  if (!Array.isArray(b.slice) || b.slice.length !== 4) {
    throw new Error(`${factory}: \`border.slice\` must be a [left, top, right, bottom] tuple`);
  }
  for (let i = 0; i < 4; i++) {
    requireFiniteNumber(b.slice[i], `border.slice[${i}]`, factory);
  }
  requireColor(b.tint, "border.tint", factory);
  return {
    texture: b.texture as string,
    slice: b.slice as [number, number, number, number],
    tint: b.tint as WidgetColor,
  };
}

// --- Image ------------------------------------------------------------------

/**
 * Props for `Image`. No bind. Name-XOR-decorative (M13 G2): exactly one of
 * `label` or `decorative: true` is required — neither or both throws. The union
 * narrows this at compile time.
 */
export type ImageProps = {
  asset: string;
  id?: string;
  focusNeighbors?: FocusNeighborsProp;
  visibleWhen?: Predicate;
  role?: WidgetRole;
} & ({ label: string; decorative?: never } | { decorative: true; label?: never });

/**
 * An `image` leaf referencing a texture asset by key; it sizes from the asset's
 * natural pixel dimensions. No bind capability. Exactly one of `label` /
 * `decorative: true` is required (the bridge enforces the same precondition).
 * Mirrors `ImageWidget`.
 */
export function Image(props: ImageProps): WidgetDescriptor {
  requireObject(props, "Image");
  requireNonemptyString(props.asset, "asset", "Image");
  const p = props as { label?: unknown; decorative?: unknown };
  const hasLabel = p.label !== undefined;
  if (p.decorative !== undefined && typeof p.decorative !== "boolean") {
    throw new Error("Image: `decorative` must be a boolean");
  }
  const decorative = p.decorative === true;
  if (hasLabel && decorative) {
    throw new Error("Image: set exactly one of `label` or `decorative: true`, not both");
  }
  if (!hasLabel && !decorative) {
    throw new Error(
      "Image: needs an accessible name: set exactly one of `label` or `decorative: true`",
    );
  }
  const out: WidgetDescriptor = { kind: "image", asset: props.asset };
  applyFocusFields(out, props, "Image");
  if (hasLabel) {
    requireNonemptyString(p.label, "label", "Image");
    out.label = p.label;
  }
  if (decorative) out.decorative = true;
  applyA11yFields(out, props, "Image");
  return out;
}

// --- Spacer -----------------------------------------------------------------

/** Props for `Spacer`. No bind. */
export type SpacerProps = {
  flexGrow?: number;
  id?: string;
  visibleWhen?: Predicate;
  role?: WidgetRole;
};

/**
 * A `spacer` leaf claiming a proportional share of leftover space (`flexGrow`,
 * default 1). No bind capability. Mirrors `SpacerWidget`.
 */
export function Spacer(props: SpacerProps = {}): WidgetDescriptor {
  requireObject(props, "Spacer");
  const flexGrow = props.flexGrow ?? 1.0;
  requireFiniteNumber(flexGrow, "flexGrow", "Spacer");
  const out: WidgetDescriptor = { kind: "spacer", flexGrow };
  if (props.id !== undefined) {
    requireNonemptyString(props.id, "id", "Spacer");
    out.id = props.id;
  }
  applyA11yFields(out, props, "Spacer");
  return out;
}

// --- Button -----------------------------------------------------------------

/**
 * Props for `Button`. `onPress` is a reaction handle or a bare name string.
 * Name-XOR (M13 G2): exactly one of `label` (inline accessible name) or
 * `labelledBy` (a node id whose text names this button) is required — neither or
 * both throws, and the union narrows it at compile time. `selected`/`checked` are
 * reactive `Predicate`s; `bind`+`styleRanges` drive the reactive highlight;
 * `disabled` makes it non-interactive.
 */
export type ButtonProps = {
  id: string;
  onPress: ReactionHandleRef | string;
  repeatOnHold?: RepeatPolicyProp;
  focusNeighbors?: FocusNeighborsProp;
  selected?: Predicate;
  checked?: Predicate;
  bind?: Predicate;
  styleRanges?: StyleRangesProp;
  disabled?: boolean;
  visibleWhen?: Predicate;
  role?: WidgetRole;
} & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });

/**
 * An interactive `button`. `id` is required (activation resolves the focused
 * node id back to `onPress`). `onPress` accepts a reaction handle (the `.name`
 * is read) or a bare reaction-name string. Exactly one of `label` / `labelledBy`
 * is required (the bridge enforces the same precondition).
 */
export function Button(props: ButtonProps): WidgetDescriptor {
  requireObject(props, "Button");
  requireNonemptyString(props.id, "id", "Button");
  const p = props as { label?: unknown; labelledBy?: unknown };
  const hasLabel = p.label !== undefined;
  const hasLabelledBy = p.labelledBy !== undefined;
  if (hasLabel && hasLabelledBy) {
    throw new Error("Button: set exactly one of `label` or `labelledBy`, not both");
  }
  if (!hasLabel && !hasLabelledBy) {
    throw new Error("Button: needs an accessible name: set exactly one of `label` or `labelledBy`");
  }

  const onPress = resolveReactionName(props.onPress, "Button");
  const out: WidgetDescriptor = { kind: "button", id: props.id, onPress };
  if (hasLabel) {
    requireString(p.label, "label", "Button");
    out.label = p.label;
  } else {
    requireNonemptyString(p.labelledBy, "labelledBy", "Button");
    out.labelledBy = p.labelledBy;
  }
  if (props.focusNeighbors !== undefined) {
    const neighbors = buildFocusNeighbors(props.focusNeighbors, "Button");
    if (neighbors !== undefined) out.focusNeighbors = neighbors;
  }
  if (props.repeatOnHold !== undefined) {
    out.repeatOnHold = buildRepeatPolicy(props.repeatOnHold, "repeatOnHold", "Button");
  }
  const selected = buildPredicate(props.selected, "selected", "Button");
  if (selected !== undefined) out.selected = selected;
  const checked = buildPredicate(props.checked, "checked", "Button");
  if (checked !== undefined) out.checked = checked;
  const bind = buildPredicate(props.bind, "bind", "Button");
  if (bind !== undefined) out.bind = bind;
  const styleRanges = buildStyleRanges(props.styleRanges, "Button");
  if (styleRanges !== undefined) out.styleRanges = styleRanges;
  if (props.disabled !== undefined) {
    if (typeof props.disabled !== "boolean") {
      throw new Error("Button: `disabled` must be a boolean");
    }
    if (props.disabled) out.disabled = true;
  }
  applyA11yFields(out, props, "Button");
  return out;
}

/**
 * Read a reaction name from a handle (`.name`) or accept a bare name string.
 * Exported so `layout`/callers can reuse it; throws naming the factory.
 */
export function resolveReactionName(value: unknown, factory: string): string {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error(`${factory}: \`onPress\` must be a nonempty reaction name`);
    }
    return value;
  }
  if (value !== null && typeof value === "object" && typeof (value as { name?: unknown }).name === "string") {
    const name = (value as { name: string }).name;
    if (name.length === 0) {
      throw new Error(`${factory}: \`onPress\` handle has an empty \`.name\``);
    }
    return name;
  }
  throw new Error(`${factory}: \`onPress\` must be a reaction handle or a reaction-name string`);
}

// --- Slider -----------------------------------------------------------------

/**
 * Props for `Slider`. `bind` is a `SliderBindProp` (numeric slot). Name-XOR
 * (M13 G2): exactly one of `label` / `labelledBy` is required, mirroring
 * `Button`. `disabled` makes it non-interactive.
 */
export type SliderProps = {
  id: string;
  bind: SliderBindProp;
  min: number;
  max: number;
  step: number;
  capturesNav?: string[];
  focusNeighbors?: FocusNeighborsProp;
  disabled?: boolean;
  visibleWhen?: Predicate;
  role?: WidgetRole;
} & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });

/**
 * An interactive `slider`. Nav wires in `capturesNav` step the bound value by
 * `step` within `[min, max]`. `id` is required. Exactly one of `label` /
 * `labelledBy` is required (the bridge enforces the same precondition).
 */
export function Slider(props: SliderProps): WidgetDescriptor {
  requireObject(props, "Slider");
  requireNonemptyString(props.id, "id", "Slider");
  const p = props as { label?: unknown; labelledBy?: unknown };
  const hasLabel = p.label !== undefined;
  const hasLabelledBy = p.labelledBy !== undefined;
  if (hasLabel && hasLabelledBy) {
    throw new Error("Slider: set exactly one of `label` or `labelledBy`, not both");
  }
  if (!hasLabel && !hasLabelledBy) {
    throw new Error("Slider: needs an accessible name: set exactly one of `label` or `labelledBy`");
  }
  requireFiniteNumber(props.min, "min", "Slider");
  requireFiniteNumber(props.max, "max", "Slider");
  requireFiniteNumber(props.step, "step", "Slider");
  const bind = buildBind(props.bind, "Slider", "slider");
  if (bind === undefined) {
    throw new Error("Slider: `bind` is required");
  }

  const out: WidgetDescriptor = { kind: "slider", id: props.id };
  if (hasLabel) {
    requireString(p.label, "label", "Slider");
    out.label = p.label;
  } else {
    requireNonemptyString(p.labelledBy, "labelledBy", "Slider");
    out.labelledBy = p.labelledBy;
  }
  out.bind = bind;
  out.min = props.min;
  out.max = props.max;
  out.step = props.step;
  if (props.capturesNav !== undefined) {
    if (!Array.isArray(props.capturesNav)) {
      throw new Error("Slider: `capturesNav` must be a string array");
    }
    props.capturesNav.forEach((n, i) => requireNonemptyString(n, `capturesNav[${i}]`, "Slider"));
    if (props.capturesNav.length > 0) out.capturesNav = props.capturesNav.slice();
  }
  if (props.focusNeighbors !== undefined) {
    const neighbors = buildFocusNeighbors(props.focusNeighbors, "Slider");
    if (neighbors !== undefined) out.focusNeighbors = neighbors;
  }
  if (props.disabled !== undefined) {
    if (typeof props.disabled !== "boolean") {
      throw new Error("Slider: `disabled` must be a boolean");
    }
    if (props.disabled) out.disabled = true;
  }
  applyA11yFields(out, props, "Slider");
  return out;
}

// --- Bar --------------------------------------------------------------------

/** Props for `Bar`. `bind` is a `SliderBindProp` (numeric slot). */
export type BarProps = {
  bind: BarBindProp;
  max: BarMaxProp;
  fill: WidgetColor;
  background: WidgetColor;
  styleRanges?: StyleRangesProp;
  id?: string;
  visibleWhen?: Predicate;
  role?: WidgetRole;
};

/**
 * A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`.
 * `styleRanges` recolors the fill.
 */
export function Bar(props: BarProps): WidgetDescriptor {
  requireObject(props, "Bar");
  const bind = buildBind(props.bind, "Bar", "slider");
  if (bind === undefined) {
    throw new Error("Bar: `bind` is required");
  }
  const max = buildBarMax(props.max, "Bar");
  requireColor(props.fill, "fill", "Bar");
  requireColor(props.background, "background", "Bar");

  const out: WidgetDescriptor = {
    kind: "bar",
    bind,
    max,
    fill: props.fill,
    background: props.background,
  };
  if (props.id !== undefined) {
    requireNonemptyString(props.id, "id", "Bar");
    out.id = props.id;
  }
  const styleRanges = buildStyleRanges(props.styleRanges, "Bar");
  if (styleRanges !== undefined) out.styleRanges = styleRanges;
  applyA11yFields(out, props, "Bar");
  return out;
}

// --- Announce ---------------------------------------------------------------

/**
 * Props for `Announce`. `text` is the positional second argument (not a prop);
 * `priority` defaults to `"polite"` and round-trips to omission. `visibleWhen`
 * gates the announcement reactively.
 */
export type AnnounceProps = {
  priority?: AnnouncePriority;
  visibleWhen?: Predicate;
};

/**
 * A non-visual `announce` widget (M13 G2): lays out as nothing; its `text` is a
 * live-region message routed to the platform a11y layer at the declared
 * `priority`. `text` is the POSITIONAL second argument. `priority` is emitted
 * ONLY for `"assertive"`; `"polite"` (and an omitted value) drop the key,
 * matching the Rust `skip_serializing_if = "is_polite"`. Mirrors `AnnounceWidget`.
 */
export function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor {
  requireObject(props, "Announce");
  requireString(text, "text", "Announce");
  const out: WidgetDescriptor = { kind: "announce", text };
  if (props.priority !== undefined) {
    if (props.priority !== "polite" && props.priority !== "assertive") {
      throw new Error('Announce: `priority` must be "polite" or "assertive"');
    }
    if (props.priority === "assertive") out.priority = "assertive";
  }
  const visibleWhen = buildPredicate(props.visibleWhen, "visibleWhen", "Announce");
  if (visibleWhen !== undefined) out.visibleWhen = visibleWhen;
  return out;
}
