
  // -------------------------------------------------------------------------
  // UI manifest wire types used by `ModManifest.uiTrees` / `LevelManifest.uiTrees`.
  //
  // Root `postretro` intentionally exposes only data shapes needed by manifest
  // declarations. UI authoring factories, layout helpers, state helpers,
  // reactions, and theme helpers are excluded from this root module; they live
  // behind the `postretro/ui` surface. The QuickJS prelude still installs
  // UI globals from `sdk/lib/prelude.ts` as temporary implementation plumbing
  // while import stripping lacks alias rewriting.

  /** The flat `kind`-tagged descriptor retained by Rust after setup. */
  export type WidgetDescriptor = { kind: string; [field: string]: unknown };
  /** Accessibility role override carried on widget and tree descriptors. */
  export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none";
  /** Tree viewport anchor. */
  export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight";
  /** Tree input behavior. */
  export type WidgetCaptureMode = "capture" | "passthrough";
  /** Flat `AnchoredTree` manifest envelope stored in UI registries. */
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
