# Animated Lightmap Compose — Clear Removal & Visibility Cull

## Goal

Cut animated lightmap compose-pass GPU cost on the target hardware (discrete Radeon Pro + Intel iGPU MacBook). Two independent wins: (1) delete a fully redundant clear pass, (2) skip dispatch tiles belonging to cells the portal trace just proved invisible. No behavior change to the sampled atlas for visible geometry.

## Scope

### In scope

- Delete the `clear_main` compute entry point, its pipeline, bind-group layout entry, and per-frame dispatch. The atlas is zero-initialized by wgpu on creation, and compose always writes `vec4(accum, 1.0)` to every texel it targets — the clear is redundant.
- Per-frame CPU filter of `DispatchTile`s against the current `VisibleCells` bitmask. Re-upload the trimmed tile list and dispatch only the kept count.
- Build the chunk → cell index required for the filter. Source of truth is `BvhLeaf { cell_id, chunk_range_start, chunk_range_count }`; the animated lightmap load path already consumes `AnimatedLightChunks`, so derive a parallel `chunk_cell_ids: Vec<u32>` at load time.
- Handle `VisibleCells::DrawAll` (exterior / fallback) by dispatching the full unfiltered tile list — same behavior as today minus the clear.

### Out of scope

- CPU-side curve evaluation / baking curves into descriptors. Inner-loop work is not the dominant cost once invisible tiles are skipped, and this architecture change deserves its own plan if measurements justify it.
- Indirect dispatch for the compose pass. Direct dispatch with a trimmed count is simpler and sufficient.
- Per-texel or per-chunk early-out inside the compose shader. Cell-level cull is a coarser and cheaper win.
- Changes to the weight-map bake, PRL section layout, or forward shader sampling path.

## Acceptance criteria

- [ ] `clear_main` entry point and its pipeline are gone; no pass clears the animated lightmap atlas at runtime.
- [ ] On a map with two animated-light rooms separated by a closed portal, standing in one room dispatches strictly fewer compose tiles than standing where both rooms are visible. (Verify via a counter logged under `RUST_LOG=debug` or a `POSTRETRO_GPU_TIMING=1` pass-time drop.)
- [ ] Rendered output for visible animated-lit surfaces is unchanged — lightmap sampling in the forward pass produces the same RGB as before for any camera position that can see the surface.
- [ ] `VisibleCells::DrawAll` path renders animated lighting correctly in exterior-camera and missing-visibility fallback cases.
- [ ] No validation errors from wgpu on startup or during frame encoding with a PRL containing animated lights.
- [ ] `cargo test -p postretro` and `cargo test -p postretro-level-compiler` pass.

## Tasks

### Task 1: Remove the clear pass

Delete `clear_main` from `animated_lightmap_compose.wgsl`. In `animated_lightmap.rs`, drop `clear_pipeline` from `DispatchState`, remove its pipeline creation, and remove the clear `dispatch_workgroups` call at the top of `dispatch()`. Verify the compose pipeline still binds the atlas as `texture_storage_2d<rgba16float, write>` — nothing else has to change. Update the shader's leading comment block to reflect that the atlas relies on zero-init plus full-coverage writes from compose.

### Task 2: Per-frame visibility cull of dispatch tiles

At load time, build `chunk_cell_ids: Vec<u32>` with one entry per animated chunk, populated by iterating `BvhLeaf.chunk_range_start..start+count` and stamping the leaf's `cell_id`. Store it on `AnimatedLightmapResources` alongside the master (uncut) tile list.

Change `dispatch_tiles_buffer` to `STORAGE | COPY_DST` and size it to the master tile count. Each frame, before encoding the compose dispatch: given `&VisibleCells`, walk the master tile list and emit indices into a scratch `Vec<DispatchTile>` for tiles whose `chunk_cell_ids[tile.chunk_idx]` is set in the visible bitmask (or keep all tiles for `DrawAll`). `queue.write_buffer` the trimmed slice and dispatch `trimmed_len` workgroups.

Plumb `&VisibleCells` into `animated_lightmap.dispatch()` from the call site in `render/mod.rs` (same place the BVH cull already consumes it).

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent edits to the shader module, the Rust module, and (for Task 2) the caller. No shared lines; both land clean together.

## Rough sketch

- Shader: `postretro/src/shaders/animated_lightmap_compose.wgsl` — delete lines 93–100 (`clear_main`).
- Rust: `postretro/src/render/animated_lightmap.rs` — `DispatchState` loses `clear_pipeline`; `dispatch()` collapses to a single compute pass with one pipeline set and one `dispatch_workgroups` call driven by the trimmed count.
- Load path: wherever `AnimatedLightChunks` + BVH leaves are both in hand (same load step that already builds `ChunkAtlasRect`s), fill `chunk_cell_ids` in one pass over `BvhLeaves`.
- Caller: `postretro/src/render/mod.rs` around `encode_frame`'s animated lightmap dispatch — pass the already-computed `VisibleCells` reference alongside the existing arguments.
- Diagnostic: a `log::debug!("animated_lm tiles: {}/{} visible", kept, total)` once per second (rate-limited) is enough to observe cull ratios without spamming.

## Open questions

- Is there a maximum tile count we should budget for the scratch buffer, or is `Vec::with_capacity(master.len())` fine? (Current cap is 65,535 — trivial heap cost.)
- Do any existing tests cover the animated lightmap dispatch shape that would need their expected tile count updated? (Likely no — compose is GPU-only — but worth checking.)
- `VisibleCells::DrawAll` on a very dense map still dispatches the full tile set; if that's a regression risk vs. the current code path it isn't — we still win by removing the clear. No action, just noting.
