// UI layout factories (M13 G1a, Task 3): capitalized constructors for the three
// container widget kinds — VStack, HStack, Grid. Compose/SwiftUI lineage: the
// props object comes first and `children` is a POSITIONAL second argument, not a
// prop. Each mirrors `emitter()`: synchronous field-named validation, documented
// defaults, a plain descriptor whose keys are the camelCase wire form of the
// matching `render/ui/descriptor.rs` container variant.
//
// Containers take NO state bind (only text/panel/slider/bar bind). They may
// carry a backdrop `fill`/`border`, a `focus` policy, and focus fields. Field
// emission order matches the Rust struct declaration so the JSON re-serializes
// byte-identically against the descriptor round-trip fixtures.
// See: context/lib/ui.md · context/lib/scripting.md §7

import {
  type WidgetDescriptor,
  type WidgetSpacing,
  type WidgetAlign,
  type WidgetColor,
  type BorderProp,
  type FocusNeighborsProp,
  type RepeatPolicyProp,
  validateBorder,
} from "./widgets";

// --- Focus policy (container-only) ------------------------------------------

/** Traversal kind for a container's `focus` policy. */
export type FocusKind = "linear" | "spatial";

/**
 * A container focus policy: a bare-string shorthand (`"linear"`/`"spatial"`,
 * default wrap, no repeat) or a detailed object. Mirrors `descriptor.rs`
 * `FocusPolicy` (untagged: string shorthand or object).
 */
export type FocusPolicyProp =
  | FocusKind
  | { policy: FocusKind; wrap?: boolean; repeat?: RepeatPolicyProp };

/** Common container props shared by VStack/HStack/Grid (minus children). */
type ContainerCommonProps = {
  gap?: WidgetSpacing;
  padding?: WidgetSpacing;
  align?: WidgetAlign;
  id?: string;
  focusNeighbors?: FocusNeighborsProp;
  focus?: FocusPolicyProp;
  restoreOnReturn?: boolean;
};

/** Props for `VStack`/`HStack`. A stack may carry a backdrop `fill`/`border`. */
export type StackProps = ContainerCommonProps & {
  fill?: WidgetColor;
  border?: BorderProp;
};

/** Props for `Grid`. Adds the required `cols`; no backdrop fill/border. */
export type GridProps = ContainerCommonProps & {
  cols: number;
};

// --- Validation helpers (container-local) ------------------------------------

function requireObject(props: unknown, factory: string): void {
  if (props === null || typeof props !== "object") {
    throw new Error(`${factory}: props must be an object`);
  }
}

function requireFiniteNumber(value: unknown, field: string, factory: string): void {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${factory}: \`${field}\` must be a finite number`);
  }
}

function requireNonemptyString(value: unknown, field: string, factory: string): void {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${factory}: \`${field}\` must be a nonempty string`);
  }
}

function validateSpacing(value: unknown, field: string, factory: string): WidgetSpacing {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error(`${factory}: \`${field}\` spacing token must be a nonempty string`);
    }
    return value;
  }
  requireFiniteNumber(value, field, factory);
  return value as number;
}

function validateAlign(value: unknown, factory: string): WidgetAlign {
  const ok = value === "start" || value === "center" || value === "end" || value === "stretch";
  if (!ok) {
    throw new Error(`${factory}: \`align\` must be one of "start" | "center" | "end" | "stretch"`);
  }
  return value as WidgetAlign;
}

function validateColor(value: unknown, field: string, factory: string): WidgetColor {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error(`${factory}: \`${field}\` color token must be a nonempty string`);
    }
    return value;
  }
  if (!Array.isArray(value) || value.length !== 4) {
    throw new Error(`${factory}: \`${field}\` must be a [r, g, b, a] tuple or a theme-token string`);
  }
  for (let i = 0; i < 4; i++) {
    requireFiniteNumber(value[i], `${field}[${i}]`, factory);
  }
  return value as WidgetColor;
}

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

function buildFocusPolicy(value: unknown, factory: string): FocusPolicyProp | undefined {
  if (value === undefined) return undefined;
  if (value === "linear" || value === "spatial") return value;
  if (value === null || typeof value !== "object") {
    throw new Error(
      `${factory}: \`focus\` must be "linear" | "spatial" or a { policy, wrap?, repeat? } object`,
    );
  }
  const f = value as Record<string, unknown>;
  if (f.policy !== "linear" && f.policy !== "spatial") {
    throw new Error(`${factory}: \`focus.policy\` must be "linear" | "spatial"`);
  }
  const out: { policy: FocusKind; wrap?: boolean; repeat?: RepeatPolicyProp } = {
    policy: f.policy as FocusKind,
  };
  // `wrap` skip-serializes only when true (its default), so an authored `false`
  // is emitted and an authored/omitted `true` is dropped — matching the Rust
  // `skip_serializing_if = "is_true"`.
  if (f.wrap !== undefined) {
    if (typeof f.wrap !== "boolean") {
      throw new Error(`${factory}: \`focus.wrap\` must be a boolean`);
    }
    if (f.wrap === false) out.wrap = false;
  }
  if (f.repeat !== undefined) {
    const r = f.repeat as Record<string, unknown>;
    if (r === null || typeof r !== "object") {
      throw new Error(`${factory}: \`focus.repeat\` must be an object`);
    }
    requireFiniteNumber(r.initialDelayMs, "focus.repeat.initialDelayMs", factory);
    requireFiniteNumber(r.intervalMs, "focus.repeat.intervalMs", factory);
    out.repeat = {
      initialDelayMs: r.initialDelayMs as number,
      intervalMs: r.intervalMs as number,
    };
  }
  return out;
}

function validateChildren(children: unknown, factory: string): WidgetDescriptor[] {
  if (!Array.isArray(children)) {
    throw new Error(`${factory}: \`children\` must be an array of widget descriptors`);
  }
  children.forEach((c, i) => {
    if (c === null || typeof c !== "object" || typeof (c as { kind?: unknown }).kind !== "string") {
      throw new Error(`${factory}: \`children[${i}]\` must be a widget descriptor (a \`kind\`-tagged object)`);
    }
  });
  return children as WidgetDescriptor[];
}

/**
 * Build the shared container fields in Rust declaration order, appending into
 * `out` (which already carries `kind` plus `gap`/`padding`/`align` — and `cols`
 * for a grid). Emits `id`, `focusNeighbors`, `focus`, `restoreOnReturn` only
 * when authored (matching each field's `skip_serializing_if`), then `children`
 * last (always present, even when empty). Stack-only `fill`/`border` are inserted
 * by the caller BEFORE this runs so the field order stays correct.
 */
function applyCommonTail(
  out: WidgetDescriptor,
  props: ContainerCommonProps,
  children: WidgetDescriptor[],
  factory: string,
): void {
  if (props.id !== undefined) {
    requireNonemptyString(props.id, "id", factory);
    out.id = props.id;
  }
  const neighbors = buildFocusNeighbors(props.focusNeighbors, factory);
  if (neighbors !== undefined) out.focusNeighbors = neighbors;
  const focus = buildFocusPolicy(props.focus, factory);
  if (focus !== undefined) out.focus = focus;
  if (props.restoreOnReturn !== undefined) {
    if (typeof props.restoreOnReturn !== "boolean") {
      throw new Error(`${factory}: \`restoreOnReturn\` must be a boolean`);
    }
    // Skip-serializes when false (the default) — emit only true.
    if (props.restoreOnReturn) out.restoreOnReturn = true;
  }
  out.children = children;
}

// --- Stacks -----------------------------------------------------------------

/**
 * A vertical stack (`vstack`): lays `children` top-to-bottom with `gap` between
 * them, `padding` inside, cross-axis `align`. `gap`/`padding` default to 0,
 * `align` to `"start"`. `children` is a positional second argument. May carry a
 * backdrop `fill`/`border`. Mirrors `descriptor.rs` `ContainerWidget`.
 */
export function VStack(props: StackProps = {}, children: WidgetDescriptor[] = []): WidgetDescriptor {
  return buildStack("vstack", "VStack", props, children);
}

/**
 * A horizontal stack (`hstack`): lays `children` left-to-right. Same props and
 * defaults as `VStack`. `children` is a positional second argument. Mirrors
 * `descriptor.rs` `ContainerWidget`.
 */
export function HStack(props: StackProps = {}, children: WidgetDescriptor[] = []): WidgetDescriptor {
  return buildStack("hstack", "HStack", props, children);
}

function buildStack(
  kind: "vstack" | "hstack",
  factory: string,
  props: StackProps,
  children: WidgetDescriptor[],
): WidgetDescriptor {
  requireObject(props, factory);
  const gap = validateSpacing(props.gap ?? 0.0, "gap", factory);
  const padding = validateSpacing(props.padding ?? 0.0, "padding", factory);
  const align = validateAlign(props.align ?? "start", factory);
  const kids = validateChildren(children, factory);

  const out: WidgetDescriptor = { kind, gap, padding, align };
  // fill/border come before the common tail in the Rust struct order.
  if (props.fill !== undefined) out.fill = validateColor(props.fill, "fill", factory);
  if (props.border !== undefined) out.border = validateBorder(props.border, factory);
  applyCommonTail(out, props, kids, factory);
  return out;
}

// --- Grid -------------------------------------------------------------------

/**
 * A `grid` container: flows `children` across `cols` columns with `gap`,
 * `padding`, and cross-axis `align`. `cols` is required (an integer ≥ 1). No
 * backdrop fill/border (a grid carries none in the wire model). `children` is a
 * positional second argument. Mirrors `descriptor.rs` `GridWidget`.
 */
export function Grid(props: GridProps, children: WidgetDescriptor[] = []): WidgetDescriptor {
  requireObject(props, "Grid");
  const gap = validateSpacing(props.gap ?? 0.0, "gap", "Grid");
  const padding = validateSpacing(props.padding ?? 0.0, "padding", "Grid");
  const align = validateAlign(props.align ?? "start", "Grid");
  if (
    typeof props.cols !== "number" ||
    !Number.isFinite(props.cols) ||
    props.cols < 1 ||
    Math.floor(props.cols) !== props.cols
  ) {
    throw new Error("Grid: `cols` must be an integer >= 1");
  }
  const kids = validateChildren(children, "Grid");

  const out: WidgetDescriptor = { kind: "grid", gap, padding, align, cols: props.cols };
  applyCommonTail(out, props, kids, "Grid");
  return out;
}
