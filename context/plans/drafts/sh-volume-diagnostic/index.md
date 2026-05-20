# SH Volume Diagnostic

## Goal

Add an in-engine visual diagnostic for baked SH irradiance volumes: wireframe AABBs for the base volume and each animated-light delta volume, plus per-probe markers colored by validity. Controlled from the existing egui debug panel. Lets a level author see at a glance whether probe coverage spans playable space and whether delta volumes are placed where expected.

## Scope

### In scope

- Renderer-side debug-line API for drawing axis-aligned wireframe boxes and small probe markers on top of the world.
- SH diagnostics widget in the existing debug panel (`render/debug_ui/`):
  - Toggle: show base volume AABB.
  - Toggle: show base-grid cell wireframe (every cell, not just the outer AABB).
  - Toggle: show per-probe markers.
  - Marker mode: validity (valid = green, invalid = red), or uniform color.
  - Marker scale slider.
  - Per-animated-light list: toggle each delta volume's AABB independently; "all on" / "all off" buttons.
- Visualizations sample the same `ShVolumeResources` state the renderer already holds; no duplicate CPU mirror of probe data.
- Tool is gated behind the existing `dev-tools` feature flag (same as the rest of the debug UI).

### Out of scope

- New PRL sections or new fields in `ShVolume` / `DeltaShVolumes` — base grid + delta grids already carry origin, cell_size, and grid_dimensions, which is all the diagnostic needs to render. (Spec re-evaluates this only if a probe-by-probe inspection view is added later.)
- Per-probe SH coefficient inspection (numeric readout, dominant-direction arrows, sphere shading by SH).
- Picking / clicking individual probes in the 3D view.
- Reordering or restructuring the existing world-wireframe pipeline (`Alt+Shift+Backslash`); the new debug-line API is additive.
- A separate debug build target or CLI dump from `prl-build`.

## Acceptance criteria

- [ ] With the debug panel open and "Show base volume AABB" enabled, a wireframe box bounding the entire SH grid is visible, depth-tested against the world but drawn after world geometry so it shows through transparent walls only where the world allows.
- [ ] With "Show base-grid cells" enabled on a small test map, each cell of the grid is drawn as a wireframe box; toggling off restores the bare AABB or hides geometry entirely depending on the other toggles.
- [ ] With "Show probe markers" enabled, every probe position has a small marker; in validity mode, markers in solid leaves render red and markers in playable leaves render green.
- [ ] Marker scale slider visibly changes marker size in the viewport in real time.
- [ ] Per animated-light delta volumes, the debug panel lists one row per light; toggling a row's checkbox shows/hides that delta volume's wireframe AABB. "All on" / "all off" affect every row.
- [ ] Loading a map with no SH volume section shows the SH diagnostics widget in a disabled state with an explanatory label ("No SH volume baked"); toggles do nothing and no debug geometry is drawn.
- [ ] Loading a map with an SH volume but zero animated lights shows an empty "Animated light delta volumes" list with an explanatory label; base-volume controls still work.
- [ ] Disabling the debug panel (Alt+Shift+Backquote) hides all SH diagnostic geometry, regardless of which toggles were on.
- [ ] All controls persist their state across panel open/close within a single session; defaults on first open are: base AABB on, cells off, markers off.

## Tasks

### Task 1: Debug-line renderer

Add an additive immediate-mode debug-line API to the renderer module: a per-frame CPU buffer of `(start, end, color_rgba)` line segments uploaded to a small vertex buffer and drawn with `LineList` topology after the world pass and before egui. Pipeline reuses the swapchain color format and the depth buffer (depth test on, depth write off) so lines occlude correctly against world geometry without polluting depth for later passes. Buffer is cleared each frame; the SH diagnostics widget is the first consumer. Existing `wireframe_pipeline` is untouched — that pipeline draws world triangles as lines and serves a different purpose.

### Task 2: SH diagnostic geometry emission

Add an `sh_diagnostics` submodule under `render/` that, each frame, reads `ShVolumeResources` (base grid origin/cell_size/dimensions, per-probe validity, animated-light delta grids) plus a small `ShDiagnosticsState` struct (panel-controlled toggles, marker mode, marker scale, per-light visibility bitmap) and emits line segments into the debug-line renderer. Helpers: AABB → 12 edges, grid → cell edges, probe → small marker (octahedron or 3-axis cross — implementer's call; pick one shape and stick with it). Probe validity is read from the CPU-side copy of the SH section if retained at load time; if not retained, this task includes keeping it. Animated-light delta grids come from the same place `sh_compose.rs` reads them — get access via a renderer accessor rather than re-loading from PRL.

### Task 3: Debug panel UI

Extend `draw_diagnostics_panel` (or add a sibling panel — implementer's call) with an "SH Volumes" collapsing section containing the controls listed in scope. Wire each control to a field on `ShDiagnosticsState`, which lives next to `DiagnosticsState` on `DebugUi`. Per-animated-light rows: pull light names/indices from the runtime delta-volume list; if names are unavailable, label rows by index (`Delta light #N`). Seed control state once on first open, mirroring the existing `DiagnosticsState::seeded` pattern.

## Sequencing

**Phase 1 (sequential):** Task 1 — debug-line renderer is a prerequisite for any visualization.
**Phase 2 (sequential):** Task 2 — consumes the line API from Task 1; introduces `ShDiagnosticsState` shape that Task 3 binds against.
**Phase 3 (sequential):** Task 3 — depends on the state struct and accessor surface from Task 2.

## Rough sketch

- New module: `crates/postretro/src/render/debug_lines.rs` — `DebugLineRenderer { vertex_buf, pipeline, segments: Vec<DebugLineVertex> }`. `push_line`, `push_aabb`, `push_marker` helpers. Drawn from a new render pass scheduled in `Renderer::render_frame` between the world overlay pass and egui.
- New module: `crates/postretro/src/render/sh_diagnostics.rs` — `ShDiagnosticsState` (panel-bound), `emit(state, sh: &ShVolumeResources, lines: &mut DebugLineRenderer)`.
- Probe validity access: `ShVolumeResources` today holds GPU textures only; add a CPU-side `Vec<u8>` (one byte per probe) populated at load time. Cheap — for a typical map this is a few thousand bytes.
- Delta volume metadata access: add `Renderer::sh_delta_volumes(&self) -> &[DeltaVolumeMeta]` returning origin / cell_size / grid_dimensions / display name per animated light, sourced from the same data already loaded by the SH compose path.
- Panel: add a collapsing `egui::CollapsingHeader::new("SH Volumes")` block to `draw_diagnostics_panel`.

## Open questions

- Probe marker geometry: octahedron (6 line segments per probe) vs. 3-axis cross (3 segments). Octahedron reads better at distance; cross is half the line count. Default to cross unless a small test shows it's unreadable.
- Should "show cells" cap probe-count to avoid frame-rate cliffs on huge maps? A 64×32×64 grid is ~131k cells × 12 edges = 1.5M line vertices per frame, which is fine for a debug tool but worth a sanity check. If it's a problem, add a "cells max" guard that downsamples (e.g., draw every Nth cell) and surface N as a slider.
- Per-light row labels: do delta volumes carry the originating light entity's `targetname` through the bake? If not, "Delta light #N" is the v1 label. Future: thread `targetname` through `delta_sh_bake.rs` → PRL → runtime — out of scope for this plan unless trivially cheap.
- Should the diagnostic also visualize the **interpolation cell** the camera is currently inside (highlight that one cell in a distinct color)? Useful for "why does indirect look wrong here?" debugging. Probably yes, but adds a fourth toggle — flagging for user decision before implementation.
