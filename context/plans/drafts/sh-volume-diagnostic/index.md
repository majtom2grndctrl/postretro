# SH Volume Diagnostic

## Goal

Add an in-engine visual diagnostic for baked SH irradiance volumes: wireframe AABBs for the base volume and each animated-light delta volume, plus per-probe markers colored by validity. Controlled from the existing egui debug panel. Lets a level author see at a glance whether probe coverage spans playable space and whether delta volumes are placed where expected.

## Scope

### In scope

- Renderer-side debug-line API for drawing axis-aligned wireframe boxes and small probe markers on top of the world.
- SH diagnostics widget in the existing debug panel (`render/debug_ui/`):
  - Toggle: show base volume AABB.
  - Toggle: show base-grid cell wireframe (capped to cells within a radius of the camera; far cells are skipped).
  - Toggle: show per-probe markers.
  - Marker mode: validity (valid = green, invalid = red), or uniform color.
  - Marker scale slider.
  - Cell-draw radius slider (world units).
  - Per-animated-light list: toggle each delta volume's AABB independently; "all on" / "all off" buttons.
- Cell wireframes are colored by camera-visibility, matching the existing world-wireframe convention: green when the cell intersects the visible set, cyan when frustum/portal-culled.
- Per-animated-light delta volumes are labeled by the originating light entity's `targetname` (threaded through the bake into PRL); when a light has no `targetname`, the row falls back to `Delta light #N`.
- Visualizations sample the same `ShVolumeResources` state the renderer already holds; no duplicate CPU mirror of probe data.
- Tool is gated behind the existing `dev-tools` feature flag (same as the rest of the debug UI).

### Out of scope

- New PRL sections — base grid + delta grids already carry origin, cell_size, and grid_dimensions. Adding a per-delta-volume `targetname` string to the existing `DeltaShVolumes` section is the **only** PRL change in scope.
- Per-probe SH coefficient inspection (numeric readout, dominant-direction arrows, sphere shading by SH).
- Picking / clicking individual probes in the 3D view.
- Highlighting the cell the camera is currently inside — that's visually obvious without help.
- Reordering or restructuring the existing world-wireframe pipeline (`Alt+Shift+Backslash`); the new debug-line API is additive.
- A separate debug build target or CLI dump from `prl-build`.

## Acceptance criteria

- [ ] With the debug panel open and "Show base volume AABB" enabled, a wireframe box bounding the entire SH grid is visible, depth-tested against the world but drawn after world geometry so it shows through transparent walls only where the world allows.
- [ ] With "Show base-grid cells" enabled, cells within the configured camera radius are drawn as wireframe boxes; cells outside the radius are not drawn. Visible cells render green; cells fully outside the camera frustum or occluded by portal culling render cyan. Toggling off restores the bare AABB or hides geometry entirely depending on the other toggles.
- [ ] Cell-draw radius slider changes which cells are drawn in real time; reducing the radius reduces the number of cells visible.
- [ ] With "Show probe markers" enabled, every probe position has a small 3-axis cross marker; in validity mode, markers in solid leaves render red and markers in playable leaves render green.
- [ ] Marker scale slider visibly changes marker size in the viewport in real time.
- [ ] Per animated-light delta volumes, the debug panel lists one row per light, labeled by the light's `targetname` (or `Delta light #N` when no `targetname` was authored); toggling a row's checkbox shows/hides that delta volume's wireframe AABB. "All on" / "all off" affect every row.
- [ ] Loading a map with no SH volume section shows the SH diagnostics widget in a disabled state with an explanatory label ("No SH volume baked"); toggles do nothing and no debug geometry is drawn.
- [ ] Loading a map with an SH volume but zero animated lights shows an empty "Animated light delta volumes" list with an explanatory label; base-volume controls still work.
- [ ] Disabling the debug panel (Alt+Shift+Backquote) hides all SH diagnostic geometry, regardless of which toggles were on.
- [ ] All controls persist their state across panel open/close within a single session; defaults on first open are: base AABB on, cells off, markers off.

## Tasks

### Task 1: Debug-line renderer

Add an additive immediate-mode debug-line API to the renderer module: a per-frame CPU buffer of `(start, end, color_rgba)` line segments uploaded to a small vertex buffer and drawn with `LineList` topology after the world pass and before egui. Pipeline reuses the swapchain color format and the depth buffer (depth test on, depth write off) so lines occlude correctly against world geometry without polluting depth for later passes. Buffer is cleared each frame; the SH diagnostics widget is the first consumer. Existing `wireframe_pipeline` is untouched — that pipeline draws world triangles as lines and serves a different purpose.

### Task 2: PRL — thread animated-light `targetname` into delta volumes

Add an optional `name: String` per delta volume to the `DeltaShVolumes` section (section 27). Bumps the section version. Compiler side (`delta_sh_bake.rs` and the upstream entity-parsing path) writes the originating light entity's `targetname` when set, empty string when absent. Loader side reads the field; runtime exposes it via the renderer accessor introduced in Task 3. Pre-release move-fast applies (per `feedback_api_stability`): no compat shim for the old format — old `.prl` files re-bake.

### Task 3: SH diagnostic geometry emission

Add an `sh_diagnostics` submodule under `render/` that, each frame, reads `ShVolumeResources` (base grid origin/cell_size/dimensions, per-probe validity, animated-light delta grids and names) plus a small `ShDiagnosticsState` struct (panel-controlled toggles, marker mode, marker scale, cell radius, per-light visibility bitmap) and emits line segments into the debug-line renderer. Helpers: AABB → 12 edges, grid cell within radius → 12 edges with culling color, probe → 3-axis cross marker (6 vertices). Cell visibility / culled coloring uses the same cull-status source feeding the existing world-wireframe pipeline. Probe validity is read from a CPU-side copy of the SH section; if not retained at load today, this task adds retention. Animated-light delta grids and names come from a new `Renderer::sh_delta_volumes()` accessor reading the same data `sh_compose.rs` already loads.

### Task 4: Debug panel UI

Extend `draw_diagnostics_panel` (or add a sibling panel — implementer's call) with an "SH Volumes" collapsing section containing the controls listed in scope. Wire each control to a field on `ShDiagnosticsState`, which lives next to `DiagnosticsState` on `DebugUi`. Per-animated-light rows: pull names from the runtime delta-volume list (Task 2 / Task 3); empty names fall back to `Delta light #N`. Seed control state once on first open, mirroring the existing `DiagnosticsState::seeded` pattern.

## Sequencing

**Phase 1 (concurrent):** Task 1 (debug-line renderer), Task 2 (PRL `name` plumbing) — independent surfaces.
**Phase 2 (sequential):** Task 3 — consumes Task 1's line API and Task 2's name accessor; introduces `ShDiagnosticsState` shape.
**Phase 3 (sequential):** Task 4 — depends on the state struct and accessor surface from Task 3.

## Rough sketch

- New module: `crates/postretro/src/render/debug_lines.rs` — `DebugLineRenderer { vertex_buf, pipeline, segments: Vec<DebugLineVertex> }`. `push_line`, `push_aabb`, `push_marker` helpers. Drawn from a new render pass scheduled in `Renderer::render_frame` between the world overlay pass and egui.
- New module: `crates/postretro/src/render/sh_diagnostics.rs` — `ShDiagnosticsState` (panel-bound), `emit(state, sh: &ShVolumeResources, lines: &mut DebugLineRenderer)`.
- Cell-radius culling: iterate only cells whose center is within `radius` of the camera; map each cell to its containing leaf (or AABB-test against the visible-cell set) for the green/cyan color decision, mirroring the world wireframe's cull-status coloring.
- Probe validity access: `ShVolumeResources` today holds GPU textures only; add a CPU-side `Vec<u8>` (one byte per probe) populated at load time. Cheap — for a typical map this is a few thousand bytes.
- Delta volume metadata access: add `Renderer::sh_delta_volumes(&self) -> &[DeltaVolumeMeta]` returning origin / cell_size / grid_dimensions / `name: String` per animated light, sourced from the same data the SH compose path already loads.
- PRL `name` field: append a length-prefixed UTF-8 string after each delta volume's existing fields in section 27; bump the section version. Empty string when the originating light has no `targetname`.
- Panel: add a collapsing `egui::CollapsingHeader::new("SH Volumes")` block to `draw_diagnostics_panel`.

## Open questions

- Cell-radius default: pick a value that shows "the area around the player" without flooding. 8–16 m feels right for typical map scales; tune during Task 3.
- Cull-status source for cells: the world-wireframe pipeline reads a per-face cull-status buffer. Cells aren't faces — decide whether to (a) reuse the existing per-leaf cull set and color each cell by the leaf its center sits in, or (b) compute a fresh per-cell frustum/portal test in `sh_diagnostics`. Option (a) is cheaper and consistent with how the engine already thinks about visibility; default to (a) unless leaf granularity proves too coarse.
