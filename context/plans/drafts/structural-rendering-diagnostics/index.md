# Structural Rendering Diagnostics

## Goal

Add a tabbed diagnostics UI and a spatial diagnostics tab for inspecting
world-rendering structure. The tab should make wireframe modes explicit and
show compiled BVH volumes so level authors can understand how BSP cells,
portal visibility, and BVH culling shape rendered geometry.

## Scope

### In scope

- Refactor the existing egui Diagnostics window into tabs.
- Keep the existing debug-panel shortcut as the only panel entry point.
- Move current lighting, shadow, SH, SDF, fog, and GPU timing controls into
  non-spatial tabs without changing their behavior.
- Add a Spatial tab for world-structure diagnostics.
- Add renderer-side controls for world triangle wireframe modes.
- Add renderer-side debug-line overlays for compiled BVH leaf AABBs.
- Add cell and portal context overlays to explain BVH output.
- Keep all GPU work inside renderer modules.

### Out of scope

- PRL compiler optimization.
- Structural/detail brush authoring semantics.
- New PRL sections or binary format changes.
- Runtime editor picking or object selection.
- Persistent user settings for diagnostic tabs.
- Replacing egui with the engine-authored UI system.
- A second debug panel shortcut.

## Acceptance criteria

- [ ] Opening Diagnostics still uses the existing debug-panel chord.
- [ ] Diagnostics window exposes tabs, and the selected tab persists while the
      panel remains open.
- [ ] Existing lighting, shadow, SH, SDF, fog, and GPU timing controls remain
      available and keep their current runtime effects.
- [ ] Spatial tab can toggle the existing world triangle wireframe overlay.
- [ ] Spatial tab distinguishes at least depth-tested and always-on-top
      triangle wireframe semantics in labels or controls.
- [ ] Wireframe modes define which triangles are drawn, whether cull status is
      shown, and which depth behavior is used.
- [ ] Spatial tab can show compiled BVH leaf AABBs for the loaded level.
- [ ] BVH AABB overlay defaults to stable cell-id coloring.
- [ ] BVH AABB overlay does not add GPU readback; cull-status coloring is
      exposed only if an existing CPU-readable status source is present,
      otherwise it is omitted or disabled.
- [ ] BVH AABB overlay exposes depth-tested and explicit x-ray depth behavior,
      with depth-tested as the default.
- [ ] BVH AABB overlay has a budget guard or sampling control so dense maps do
      not flood the debug-line buffer silently. The guard is local to the BVH
      overlay and deterministic before lines are appended to the shared debug
      line buffer.
- [ ] Spatial tab can show BSP/cell bounds for the loaded level.
- [ ] Spatial tab can show portal edges or polygons for the loaded level.
- [ ] Spatial context overlays can distinguish currently visible cells from
      non-visible cells.
- [ ] Spatial visible-cell coloring uses the drawable `VisibleCells` result,
      not the wider fog/light reachability mask.
- [ ] Spatial overlays no-op cleanly when no level or no BVH is loaded.
- [ ] No wgpu types or GPU calls leave renderer modules.
- [ ] Existing diagnostic input tests still pass.
- [ ] Renderer/debug-line tests cover pure helpers introduced for color
      selection, budget selection, tab defaults, and enum/mode mapping where
      those helpers exist.

## Tasks

### Task 1: Tabbed Diagnostics Shell

Refactor `crates/postretro/src/render/debug_ui/mod.rs` so the Diagnostics
window has tab state and renders one tab body at a time. Add a small enum for
tabs, likely stored beside `DiagnosticsState` or in `DebugUi`. Keep current
controls functionally unchanged. Split large tab bodies into local helpers if
that keeps `draw_diagnostics_panel` readable.

Existing controls should land in these groups unless implementation finds a
cleaner split:

- Lighting: lighting systems, SDF shadow mode, dynamic direct controls,
  freeze time.
- Volumes: SH volumes and SDF/fog quality controls.
- Performance: GPU timing.
- Spatial: new structure overlays.

No new `DiagnosticAction` is needed for opening Spatial. The existing
`DiagnosticAction::ToggleDebugPanel` remains the single entry point.

### Task 2: Spatial Diagnostics State and Renderer API

Add spatial diagnostics state for the new tab. It should hold selected
wireframe mode, BVH overlay visibility, BVH color mode, BVH depth mode, and
BVH budget/filter controls. Add this state in `debug_ui/mod.rs` and renderer
state/accessors in `renderer_types.rs` and `renderer_diagnostics.rs`, following
the current panel-to-renderer pattern used by lighting and SDF controls.

The BVH budget/filter state should include a deterministic local guard before
any AABB lines are appended to `DebugLineRenderer`, such as `max_boxes` plus
stride sampling and/or visible-cells-only filtering. Do not model this as only
a shared debug-line cap.

Keep wireframe naming clear. The current overlay is a culling-status triangle
wireframe. The user-facing label should not imply it is an authoring brush
outline or a purely visible-surface mesh.

### Task 3: Wireframe Mode Cleanup

Make world triangle wireframe behavior explicit in renderer code and docs.
Current source has a mismatch: `rendering_pipeline.md` describes a depth-tested
wireframe overlay, while `crates/postretro/src/render/renderer_init_pipelines.rs`
creates the wireframe pipeline with `CompareFunction::Always`, disables depth
writes, and `record_wireframe_overlay` draws every BVH leaf while tinting by
cull status. Decide and implement distinct modes instead of a hidden behavior.

Minimum useful modes:

- Off.
- Cull-status triangle wireframe: draws all loaded world triangles from every
  BVH leaf, keeps cull-status tinting, and renders always-on-top. This is the
  current diagnostic behavior and should be labeled as a culling diagnostic.
- Visible triangle wireframe: draws only triangles submitted as visible by the
  current frame's CPU drawable `VisibleCells` path, suppresses cull-status
  tinting, and renders depth-tested so hidden/cut-off surfaces do not read as
  visible structure. It does not mean final GPU BVH/frustum survivors; current
  cull status is GPU-resident, and this plan should not add GPU readback for
  wireframe filtering.

Implementation may use separate pipelines or a compact pipeline selector.
`record_wireframe_overlay` should select behavior from renderer state instead
of a single boolean. Keep the existing `Alt+Shift+Backslash` behavior as a
fast toggle from Off to `CullStatusAlwaysOnTop`, and back to Off when that mode
is active. The Spatial tab exposes the full selector.

### Task 4: BVH Leaf AABB Overlay

Add a renderer diagnostic emitter in `renderer_diagnostics.rs` that walks the
loaded `BvhLeaf` list and pushes AABB wires into `DebugLineRenderer`. Use
`push_aabb` for depth-tested inspection and `push_aabb_overlay` only for an
explicit x-ray mode. Convert `BvhLeaf.aabb_min` / `aabb_max` into `Vec3` in
renderer code. This task should create the emitter and state plumbing; final
frame call-site wiring can happen in Task 6.

The overlay should default to stable cell-id coloring, because this diagnostic
is meant to explain structural partitioning and visibility. If the renderer has
CPU-readable cull status already available for the frame, add cull-status
coloring as an additional mode. Do not add GPU readback just to color boxes in
the first pass. Current cull status is GPU-resident, so the first implementation
should not expose a cull-status AABB mode unless a CPU mirror already exists.
Defer material-bucket coloring unless a later batching/material diagnostic
needs it.

Add a budget guard. Acceptable approaches include max boxes per frame, stride
sampling, or visible-cell-only filtering. Do not rely solely on the shared
debug-line renderer cap for BVH budgeting, because that would make BVH output
depend on other overlays and append order. The behavior must be deterministic
enough for visual comparison.

### Task 5: Cell and Portal Context

Add context overlays for cells and portals. Reuse existing level runtime data
where possible:

- BSP leaf/cell bounds from the decoded `LevelWorld` leaf data.
- Portal polygons or edges from decoded `LevelWorld` portal data.
- The current drawable visible-cell result from per-frame visibility
  determination.

These overlays should be separate toggles in Spatial. Cell overlays should make
the current visible set readable, using `VisibleCells` from the frame loop or
renderer frame input. Use `VisibleCells::Culled` as the exact visible set for
drawable BSP cells. Treat `VisibleCells::DrawAll` as all non-solid drawable
cells visible, and label or color that fallback distinctly enough that it does
not look like a successful portal walk. The wider `fog_reachable` /
`light_reachable_leaf_mask` data remains reserved for fog and dynamic-light
isolation and should not drive Spatial visible-cell coloring unless a future
mode explicitly names that behavior.

All first-pass cell and portal overlays should consume existing decoded PRL
runtime data only. Do not add PRL sections, compiler output, GPU readback, or
wgpu-facing APIs for these overlays. Renderer-facing API additions should pass
plain CPU data or renderer-owned state across the existing renderer boundary.

Implement the public overlay emitters in `renderer_diagnostics.rs`. They should
use the same per-frame debug-line lifecycle as SH and nav overlays: in
`main.rs`, near `clear_debug_lines` and `emit_sh_diagnostics`, clear once, then
append all selected diagnostics before `render_frame_indirect`. Do not reuse
`light_reachable_leaf_mask` for Spatial visible-cell coloring; pass or derive
the visible coloring directly from `visible_cells`.

### Task 6: Integration and Tests

Wire Spatial diagnostics into the current frame path beside
`emit_sh_diagnostics`, `emit_nav_diagnostics`, and agent path overlays. Add
unit tests for tab defaults, diagnostic chord stability, wireframe mode state,
and non-GPU color/budget helpers. Discover the final Spatial state and renderer
API names created by earlier tasks in `debug_ui/mod.rs` and
`renderer_diagnostics.rs`, then wire them beside `emit_sh_diagnostics` in
`main.rs`.

Diagnostic chord tests should assert that `ToggleDebugPanel` remains
`Alt+Shift+Backquote`, no Spatial-specific chord is added, and the egui
consumed-event gate still lets only `ToggleDebugPanel` pass while the panel has
focus. Update `context/lib/rendering_pipeline.md` as part of implementation so
it documents the final wireframe modes, cull-status semantics, depth behavior,
and Spatial overlay data sources.

## Sequencing

**Phase 1 (sequential):** Task 1 — creates the tab shell and reduces control overload before new controls land.

**Phase 2 (sequential):** Task 2 — defines Spatial UI state and renderer access plumbing for later tasks.

**Phase 3 (concurrent):** Task 3, Task 4 — independent renderer diagnostics once Spatial state exists.

**Phase 4 (sequential):** Task 5 — consumes the Spatial tab and debug-line integration shape from Tasks 2 and 4.

**Phase 5 (sequential):** Task 6 — final wiring, tests, and documentation alignment.

## Rough sketch

`DebugUi` already stores CPU-side egui state. Add selected-tab state there or
inside `DiagnosticsState`. `draw_diagnostics_panel` can render a top row of
`selectable_value` controls and dispatch to helper functions for each tab.

Renderer state already owns `wireframe_enabled`, `bvh_leaves`,
`compute_cull`, and `debug_lines`. Replace the single wireframe bool with a
small mode enum, or add a mode field while preserving the existing bool as a
derived compatibility layer. `record_wireframe_overlay` should read the mode
and select the right pipeline/depth behavior.

`DebugLineRenderer` already supports depth-tested and always-on-top AABBs.
The BVH overlay should use those helpers rather than adding a new GPU pass.
Call the new spatial emitter from the same frame section that currently clears
debug lines and emits SH/nav overlays in `main.rs`.

The first implementation should avoid GPU readback. Culling status is already
written on GPU by compute cull, but if there is no CPU copy at the time of the
overlay, keep cell-id coloring as the first-pass mode and leave exact
cull-status box color as a follow-up.

For Spatial visible-cell coloring, prefer the drawable `VisibleCells` value
that already feeds the indirect render path. Do not substitute
`light_reachable_leaf_mask`: it intentionally represents the wider fog/light
reachability set and includes empty leaves that do not draw world geometry.

## Open questions

None.
