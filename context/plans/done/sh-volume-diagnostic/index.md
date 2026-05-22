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
- Cell wireframes colored by camera-visibility: green when the cell intersects the visible set, cyan when frustum/portal-culled. Coloring derived from the existing per-leaf cull set (option a): each cell is colored by the leaf its center sits in.
- Per-animated-light delta volumes labeled by index (`Delta light #N`).
- Tool is gated behind the existing `dev-tools` feature flag.

### Out of scope

- PRL changes — base grid and delta grids already carry origin, cell_size, and grid_dimensions. No PRL sections added or modified.
- Compiler changes — no `targetname` plumbing; `Delta light #N` labels are sufficient for v1.
- Per-probe SH coefficient inspection (numeric readout, dominant-direction arrows, sphere shading by SH).
- Picking / clicking individual probes in the 3D view.
- Highlighting the cell the camera is currently inside — visually obvious without help.
- Reordering or restructuring the existing world-wireframe pipeline (`Alt+Shift+Backslash`); the new debug-line API is additive.
- A separate debug build target or CLI dump from `prl-build`.

## Acceptance criteria

- [ ] With the debug panel open and "Show base volume AABB" enabled, a wireframe box bounding the entire SH grid is visible, depth-tested against the opaque depth buffer; lines behind opaque geometry are hidden; transparent geometry (billboards, fog) does not occlude lines.
- [ ] With "Show base-grid cells" enabled, cells within the configured camera radius are drawn as wireframe boxes; cells outside the radius are not drawn. Cells portal-reachable from the camera's current leaf render green; cells not reachable via portals render cyan. (No frustum check — coloring reflects portal connectivity only, using the same `fog_reachable` leaf mask as dynamic-light gating.) An empty mask (no portals, exterior camera, fallback paths) treats all cells as visible. Toggling off restores the bare AABB or hides geometry entirely depending on the other toggles.
- [ ] Cell-draw radius slider changes which cells are drawn in real time; reducing the radius reduces the number of cells visible.
- [ ] With "Show probe markers" enabled, every probe position has a small 3-axis cross marker; in validity mode, markers in solid leaves render red and markers in playable leaves render green.
- [ ] Marker scale slider visibly changes marker size in the viewport in real time.
- [ ] Per animated-light delta volumes, the debug panel lists one row per light labeled `Delta light #N`; toggling a row's checkbox shows/hides that delta volume's wireframe AABB. "All on" / "all off" affect every row.
- [ ] Loading a map with no SH volume section shows the SH diagnostics widget in a disabled state with an explanatory label ("No SH volume baked"); toggles do nothing and no debug geometry is drawn.
- [ ] Loading a map with an SH volume but zero animated lights shows an empty "Animated light delta volumes" list with an explanatory label; base-volume controls still work.
- [ ] Disabling the debug panel (Alt+Shift+Backquote) hides all SH diagnostic geometry, regardless of which toggles were on.
- [ ] All controls persist their state across panel open/close within a single session; defaults on first open are: base AABB on, cells off, markers off.

## Tasks

### Task 1: Debug-line renderer

Add an immediate-mode debug-line API to the renderer module: a per-frame CPU buffer of `(start, end, color_rgba)` line segments, capped at a fixed maximum (overflow: log once + truncate). Buffer uploaded to a `LineList` vertex buffer and drawn in `Renderer::render_frame_indirect` after the fog composite pass and before egui. Pipeline: swapchain color format, depth test on (matching the world render target's sample count), depth write off. Buffer cleared each frame. Existing `wireframe_pipeline` is untouched.

### Task 2: SH diagnostic geometry emission

Add `crates/postretro/src/render/sh_diagnostics.rs`. Each frame, when the debug panel is visible, reads `ShVolumeResources` (base grid origin/cell_size/dimensions, per-probe validity) and `ShDiagnosticsState` (panel-controlled toggles, marker mode, marker scale, cell radius, per-light visibility bitmap) and emits line segments into the debug-line renderer. When the panel is hidden, skip emission entirely.

Emit helpers: AABB → 12 edges, grid cell within radius → 12 edges with per-leaf cull color, probe → 3-axis cross (6 segments). Per-probe validity comes from the existing per-probe validity byte baked into section 20; retain it as a CPU-side `Vec<u8>` on `ShVolumeResources` at load time (one byte per probe, z-major order, matching the section layout). Delta volume metadata comes from `Renderer::sh_delta_volumes(&self) -> &[DeltaVolumeMeta]`, where `DeltaVolumeMeta` holds origin / cell_size / grid_dimensions per animated light, sourced from the same data `sh_compose.rs` already loads.

Add `Renderer::has_sh_volume() -> bool` for the panel to query whether a baked SH volume is present.

### Task 3: Debug panel UI

Extend `draw_diagnostics_panel` (or add a sibling — implementer's call) with a collapsing "SH Volumes" section containing the controls listed in scope. Wire each control to a field on `ShDiagnosticsState`, which lives alongside `DiagnosticsState` on `DebugUi`. Per-animated-light rows labeled `Delta light #N`. On maps with no SH volume (`has_sh_volume()` returns false), show the disabled-state label; all toggles no-op. Seed control state once on first open, mirroring the existing `DiagnosticsState::seeded` pattern. Per-light bitmap resets on map load.

## Sequencing

**Phase 1 (sequential):** Task 1 — debug-line renderer is a prerequisite for any visualization.
**Phase 2 (sequential):** Task 2 — consumes Task 1's line API; introduces `ShDiagnosticsState` shape that Task 3 binds against.
**Phase 3 (sequential):** Task 3 — depends on the state struct and accessor surface from Task 2.

## Rough sketch

- New module: `crates/postretro/src/render/debug_lines.rs` — `DebugLineRenderer { vertex_buf, pipeline, segments: Vec<DebugLineVertex> }` with a fixed segment cap (tune during Task 1; 256 k segments ≈ 10 MB is a reasonable starting point). `push_line`, `push_aabb`, `push_marker` helpers. Scheduled in `Renderer::render_frame_indirect` after fog composite, before egui. Pipeline sample count matches the world render target.
- New module: `crates/postretro/src/render/sh_diagnostics.rs` — `ShDiagnosticsState` (panel-bound), `emit(state, sh: &ShVolumeResources, delta_vols: &[DeltaVolumeMeta], lines: &mut DebugLineRenderer)`. Guard: returns immediately when panel is hidden.
- Probe validity: `ShVolumeResources` gains `validity: Vec<u8>` populated from section 20 at load time.
- Cell cull color: each cell's center is point-tested into the BSP to find its leaf; the leaf's cull status from the per-leaf cull set determines green vs. cyan, same source as the world-wireframe path.
- `Renderer::sh_delta_volumes()` + `Renderer::has_sh_volume()` added to the renderer's public accessor surface.
- Panel: `egui::CollapsingHeader::new("SH Volumes")` block added to `draw_diagnostics_panel`.

## Open questions

- Cell-radius default: 8–16 m feels right for typical map scales; tune during Task 2.
