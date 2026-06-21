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
- [ ] Spatial tab can show compiled BVH leaf AABBs for the loaded level.
- [ ] BVH AABB overlay has at least one useful color mode: cull status,
      material bucket, or cell id.
- [ ] BVH AABB overlay has a budget guard or sampling control so dense maps do
      not flood the debug-line buffer silently.
- [ ] Spatial tab can show BSP/cell bounds for the loaded level.
- [ ] Spatial tab can show portal edges or polygons for the loaded level.
- [ ] Spatial context overlays can distinguish currently visible cells from
      non-visible cells.
- [ ] Spatial overlays no-op cleanly when no level or no BVH is loaded.
- [ ] No wgpu types or GPU calls leave renderer modules.
- [ ] Existing diagnostic input tests still pass.
- [ ] Renderer/debug-line tests cover any new overlay primitive, cap, or mode
      behavior that can be tested without a GPU.

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
wireframe mode, BVH overlay visibility, BVH color mode, and any budget/filter
controls. Add renderer setters/getters where needed, following the current
panel-to-renderer pattern used by lighting and SDF controls.

Keep wireframe naming clear. The current overlay is a culling-status triangle
wireframe. The user-facing label should not imply it is an authoring brush
outline or a purely visible-surface mesh.

### Task 3: Wireframe Mode Cleanup

Make world triangle wireframe behavior explicit in renderer code and docs.
Current source has a mismatch: `rendering_pipeline.md` describes a depth-tested
wireframe overlay, while `crates/postretro/src/render/renderer_init_pipelines.rs`
creates the wireframe pipeline with `CompareFunction::Always`. Decide and
implement distinct modes instead of a hidden behavior.

Minimum useful modes:

- Off.
- Cull-status triangle wireframe, always-on-top.
- Visible triangle wireframe, depth-tested.

Implementation may use separate pipelines or a compact pipeline selector.
`record_wireframe_overlay` should select behavior from renderer state instead
of a single boolean. Keep the existing `Alt+Shift+Backslash` behavior as a
fast toggle for the current cull-status overlay unless implementation finds a
compatibility issue. The Spatial tab exposes the full selector.

### Task 4: BVH Leaf AABB Overlay

Add a renderer diagnostic emitter that walks the loaded `BvhLeaf` list and
pushes AABB wires into `DebugLineRenderer`. Use `push_aabb` for depth-tested
inspection and `push_aabb_overlay` only for an explicit x-ray mode. Convert
`BvhLeaf.aabb_min` / `aabb_max` into `Vec3` in renderer code.

The overlay should support a first color mode. Prefer cull status if the
renderer has a CPU-readable status already available for the frame; otherwise
use stable material-bucket or cell-id hashing. Do not add GPU readback just to
color boxes in the first pass.

Add a budget guard. Acceptable approaches include max boxes per frame, stride
sampling, visible-cell-only filtering, or using the debug-line renderer cap with
a clear UI label. The behavior must be deterministic enough for visual
comparison.

### Task 5: Cell and Portal Context

Add context overlays for cells and portals. Reuse existing level runtime data
where possible:

- BSP leaf/cell bounds from the loaded level.
- Portal polygons or edges from loaded portal data.
- The current visible-cell mask from portal traversal.

These overlays should be separate toggles in Spatial. Cell overlays should make
the current visible set readable, using the visible-cell mask already available
in the frame loop. They should use the same per-frame debug-line lifecycle as
SH and nav overlays: clear once, then append all selected diagnostics before
`render_frame_indirect`.

### Task 6: Integration and Tests

Wire Spatial diagnostics into the current frame path beside
`emit_sh_diagnostics`, `emit_nav_diagnostics`, and agent path overlays. Add
unit tests for tab defaults, diagnostic chord stability, wireframe mode state,
and any non-GPU color/budget helper. Update `context/lib/rendering_pipeline.md`
only if the implementation changes durable renderer behavior; otherwise leave
library updates for promotion.

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
overlay, choose a CPU-local color mode first and leave exact cull-status box
color as a follow-up.

## Open questions

- Should BVH boxes default to depth-tested or x-ray? Depth-tested is less
  misleading; x-ray is better for whole-map structure. Default to depth-tested
  unless early implementation screenshots show it hides too much structure.
