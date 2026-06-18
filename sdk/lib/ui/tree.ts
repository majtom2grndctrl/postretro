// UI placement-envelope factory (M13 G1a, Task 4): `Tree(...)` wraps a root
// widget descriptor (from the `widgets`/`layout` factories) in the `AnchoredTree`
// shape — `anchor`, `offset`, `root`, and the optional `captureMode`/
// `initialFocus`/`textEntryTarget`. Mirrors `render/ui/descriptor.rs`'s
// `AnchoredTree` serde wire form: camelCase keys, and `captureMode` omitted when
// passthrough (the default) so a HUD/pre-F tree round-trips byte-identically.
//
// Pure builder: constructing a tree has no engine side effect — the FFI boundary
// is the eventual `return` of the authored tree. `children`-style nesting is the
// `root` widget's own concern; this factory only places the whole tree once.
// See: context/lib/ui.md · context/lib/scripting.md §7

import type { WidgetDescriptor, WidgetRole, WritableStateRef } from "./widgets";

/**
 * The nine placement anchors a tree may be pinned to. Mirrors `descriptor.rs`
 * `layout::Anchor` (camelCase wire literals). The anchor is both the reference
 * point on the logical-reference canvas and the tree's own pivot; `offset`
 * nudges it from there.
 */
export type WidgetAnchor =
  | "topLeft"
  | "top"
  | "topRight"
  | "left"
  | "center"
  | "right"
  | "bottomLeft"
  | "bottom"
  | "bottomRight";

/**
 * Whether a tree captures input (freezing gameplay + lower trees and releasing
 * the cursor) or passes it through to gameplay (HUD behavior). Mirrors
 * `descriptor.rs` `CaptureMode`. `"passthrough"` is the default and round-trips
 * to omission (the wire form omits the key); only `"capture"` is emitted.
 */
export type WidgetCaptureMode = "capture" | "passthrough";

/**
 * Placement-envelope props for `Tree`. `anchor` and `offset` place the whole
 * tree once against the logical-reference canvas. `captureMode` defaults to
 * `"passthrough"` (omitted from the wire form). `initialFocus` names the node
 * focus starts on when this tree tops the modal stack; `textEntryTarget` is the
 * writable String slot this tree's text entry edits. Both optional and omitted
 * when absent. Mirrors `descriptor.rs` `AnchoredTree`.
 */
export type TreeProps = {
  anchor: WidgetAnchor;
  offset: [number, number];
  captureMode?: WidgetCaptureMode;
  initialFocus?: string;
  textEntryTarget?: WritableStateRef<string>;
  accessibleName?: string;
  role?: WidgetRole;
};

/**
 * The flat envelope descriptor `Tree` produces: the `AnchoredTree` wire shape.
 * `captureMode`/`initialFocus`/`textEntryTarget` appear only when authored
 * (matching each field's `skip_serializing_if`).
 */
export type AnchoredTreeDescriptor = {
  anchor: WidgetAnchor;
  offset: [number, number];
  root: WidgetDescriptor;
  captureMode?: WidgetCaptureMode;
  initialFocus?: string;
  textEntryTarget?: string;
  accessibleName?: string;
  role?: WidgetRole;
};

/** A UI-tree registration entry returned through `ModManifest.uiTrees` or
 * `setupLevel().uiTrees`. This helper preserves the runtime manifest shape
 * while giving authors a typed construction site. */
export type UiTreeRegistrationProps<Name extends string = string> = {
  name: Name;
  tree: AnchoredTreeDescriptor;
  alwaysOn?: boolean;
};

export type UiTreeRegistration<Name extends string = string> =
  import("postretro").ModUiTree & { readonly name: Name };

const ANCHORS: ReadonlySet<string> = new Set([
  "topLeft",
  "top",
  "topRight",
  "left",
  "center",
  "right",
  "bottomLeft",
  "bottom",
  "bottomRight",
]);

const TREE_ROLES: ReadonlySet<string> = new Set([
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

function requireObject(value: unknown, factory: string): void {
  if (value === null || typeof value !== "object") {
    throw new Error(`${factory}: props must be an object`);
  }
}

function requireNonemptyString(value: unknown, field: string, factory: string): void {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${factory}: \`${field}\` must be a nonempty string`);
  }
}

/**
 * Build a placement envelope wrapping `root`. Props come first, the root widget
 * descriptor (from `widgets`/`layout`) is a POSITIONAL second argument — the same
 * Compose/SwiftUI lineage as the container factories' `children`. `offset` is an
 * `[x, y]` logical-reference px tuple (+x right, +y down).
 *
 * Field emission order matches the Rust struct declaration (anchor, offset, root,
 * captureMode, initialFocus, textEntryTarget) so the JSON re-serializes
 * byte-identically against the `descriptor.rs` round-trip. `captureMode` is
 * emitted ONLY for `"capture"`: `"passthrough"` (and an omitted `captureMode`)
 * drop the key, matching the Rust `skip_serializing_if = "is_passthrough"` so a
 * passthrough tree round-trips to omission.
 */
export function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor {
  requireObject(props, "Tree");
  if (!ANCHORS.has(props.anchor as string)) {
    throw new Error(
      'Tree: `anchor` must be one of "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight"',
    );
  }
  const offset = props.offset;
  if (
    !Array.isArray(offset) ||
    offset.length !== 2 ||
    typeof offset[0] !== "number" ||
    !Number.isFinite(offset[0]) ||
    typeof offset[1] !== "number" ||
    !Number.isFinite(offset[1])
  ) {
    throw new Error("Tree: `offset` must be an [x, y] tuple of finite numbers");
  }
  if (root === null || typeof root !== "object" || typeof (root as { kind?: unknown }).kind !== "string") {
    throw new Error("Tree: `root` must be a widget descriptor (a `kind`-tagged object)");
  }

  const out: AnchoredTreeDescriptor = {
    anchor: props.anchor,
    offset: [offset[0], offset[1]],
    root,
  };

  // captureMode skip-serializes when passthrough (the default), so emit the key
  // ONLY for "capture"; "passthrough" and an omitted value drop the key.
  if (props.captureMode !== undefined) {
    if (props.captureMode !== "capture" && props.captureMode !== "passthrough") {
      throw new Error('Tree: `captureMode` must be "capture" or "passthrough"');
    }
    if (props.captureMode === "capture") out.captureMode = "capture";
  }
  if (props.initialFocus !== undefined) {
    requireNonemptyString(props.initialFocus, "initialFocus", "Tree");
    out.initialFocus = props.initialFocus;
  }
  if (props.textEntryTarget !== undefined) {
    if (
      props.textEntryTarget === null ||
      typeof props.textEntryTarget !== "object" ||
      typeof props.textEntryTarget.slot !== "string"
    ) {
      throw new Error("Tree: `textEntryTarget` must be a writable string state reference");
    }
    requireNonemptyString(props.textEntryTarget.slot, "textEntryTarget.slot", "Tree");
    out.textEntryTarget = props.textEntryTarget.slot;
  }
  if (props.accessibleName !== undefined) {
    requireNonemptyString(props.accessibleName, "accessibleName", "Tree");
    out.accessibleName = props.accessibleName;
  }
  if (props.role !== undefined) {
    if (typeof props.role !== "string" || !TREE_ROLES.has(props.role)) {
      throw new Error(
        'Tree: `role` must be one of "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"',
      );
    }
    out.role = props.role;
  }
  return out;
}

/**
 * Define a named UI-tree registration while preserving the manifest wire shape:
 * `{ name, tree, alwaysOn? }`. Pure builder; registration still happens only
 * when the returned object is included in `ModManifest.uiTrees`.
 */
export function defineUiTree<const Name extends string>(
  registration: UiTreeRegistrationProps<Name>,
): UiTreeRegistration<Name> {
  requireObject(registration, "defineUiTree");
  requireNonemptyString(registration.name, "name", "defineUiTree");
  if (
    registration.tree === null ||
    typeof registration.tree !== "object" ||
    (registration.tree as { root?: unknown }).root === null ||
    typeof (registration.tree as { root?: unknown }).root !== "object"
  ) {
    throw new Error("defineUiTree: `tree` must be an anchored tree descriptor");
  }
  if (registration.alwaysOn !== undefined && typeof registration.alwaysOn !== "boolean") {
    throw new Error("defineUiTree: `alwaysOn` must be a boolean when present");
  }

  const out: import("postretro").ModUiTree = {
    name: registration.name,
    tree: registration.tree,
  };
  if (registration.alwaysOn !== undefined) {
    out.alwaysOn = registration.alwaysOn;
  }
  return out as UiTreeRegistration<Name>;
}
