# Animated Lightmap Compose — Clear Removal & Visibility Cull

## Goal

Cut animated lightmap compose-pass GPU cost on the target hardware (discrete Radeon Pro + Intel iGPU MacBook). Two independent wins: (1) delete a fully redundant clear pass, (2) skip dispatch tiles belonging to cells the portal trace just proved invisible. No behavior change to the sampled atlas for visible geometry.

## Scope

### In scope

- Delete the `clear_main` compute entry point, its pipeline, and its per-frame dispatch. (The compose pass shares the same `group(1)` layout, so no bind-group layout entries go away.) The atlas is zero-initialized by wgpu on creation and is never recreated mid-run. Compose writes every texel the forward pass samples; the residual lanes inside partially-covered 8×8 tiles (where `rect_x >= rect.width || rect_y >= rect.height` short-circuits compose) rely on the zero-init and are never sampled via any valid lightmap UV. The clear adds no behavior — it only re-zeroes memory that is either already zero or about to be overwritten.
- Per-frame CPU filter of `DispatchTile`s against the current `VisibleCells` bitmask. Re-upload the trimmed tile list and dispatch only the kept count.
- Build the chunk → cell index required for the filter. Source of truth is `BvhLeaf { cell_id, chunk_range_start, chunk_range_count }`; the animated lightmap load path already consumes `AnimatedLightChunks`, so derive a parallel `chunk_cell_ids: Vec<u32>` at load time.
- Handle `VisibleCells::DrawAll` (exterior / fallback) by dispatching the full unfiltered tile list — same behavior as today minus the clear.
- Update doc surfaces that describe the current "clear + compose" shape so they don't drift: `context/lib/rendering_pipeline.md` §7.1 (step 4 mentions the clear), the module header comment in `animated_lightmap.rs`, the `dispatch()` doc comment, and the compute-pass label string.
- Update the `compose_shader_parses_and_declares_debug_binding` test in `animated_lightmap.rs` — it currently asserts `has_clear` on the concatenated shader source and will fail on Task 1 landing. Invert the assertion (or drop the clear check) as part of Task 1.
- Guarantee `VisibleCells` remains the single source of truth for every draw that samples the animated lightmap atlas. The forward pass is the only sampler today; any future pass (reflection probes, alternate cameras) that draws animated-lit geometry must either share the same `VisibleCells` or skip animated-lit chunks entirely. Call this out in the module header comment so the invariant is visible where the cull is implemented.

### Out of scope

- CPU-side curve evaluation / baking curves into descriptors. Inner-loop work is not the dominant cost once invisible tiles are skipped, and this architecture change deserves its own plan if measurements justify it.
- Indirect dispatch for the compose pass. Direct dispatch with a trimmed count is simpler and sufficient.
- Per-texel or per-chunk early-out inside the compose shader. Cell-level cull is a coarser and cheaper win.
- Changes to the weight-map bake, PRL section layout, or forward shader sampling path.
- Behavior change to debug modes (`POSTRETRO_ANIMATED_LM_DEBUG`). Modes 1 and 2 also short-circuit on the `rect_x >= rect.width` guard and thus also depend on zero-init for uncovered lanes; that is unchanged. Post-cull, tiles belonging to invisible cells will read zero in the heatmap/isolation output — intentional, the debug views track what the GPU is actually composing this frame, not potential coverage. The env var is parsed once at init, so no runtime mode-switching path exists.

## Acceptance criteria

- [ ] `clear_main` entry point and its pipeline are gone; no pass clears the animated lightmap atlas at runtime.
- [ ] On a map with two animated-light rooms separated by a closed portal, a `log::debug!` counter logged from `animated_lightmap.dispatch()` shows `kept < total` tiles when the camera is in one room with the other fully occluded, and `kept == total` when both rooms are visible.
- [ ] Rendered output for visible animated-lit surfaces is unchanged — lightmap sampling in the forward pass produces the same RGB as before for any camera position that can see the surface.
- [ ] `VisibleCells::DrawAll` path renders animated lighting correctly in exterior-camera and missing-visibility fallback cases.
- [ ] `context/lib/rendering_pipeline.md` §7.1 no longer describes a clear step for the animated lightmap compose pass.
- [ ] No validation errors from wgpu on startup or during frame encoding with a PRL containing animated lights.
- [ ] `cargo test -p postretro` and `cargo test -p postretro-level-compiler` pass.

## Tasks

### Task 1: Remove the clear pass

Delete `clear_main` from `animated_lightmap_compose.wgsl`. In `animated_lightmap.rs`, drop `clear_pipeline` from `DispatchState`, remove its pipeline creation, remove the clear `dispatch_workgroups` call at the top of `dispatch()`, and delete the now-dead `atlas_size` field (only used to compute `clear_groups`). Rename the compute-pass label from `"Animated LM Clear+Compose"` to something like `"Animated LM Compose"`. Update the module header doc comment, the `dispatch()` doc comment, and the shader's leading comment block to describe the new shape (compose-only; zero-init + full-coverage writes for sampled texels). Update the `compose_shader_parses_and_declares_debug_binding` test to drop the `has_clear` assertion. Update `context/lib/rendering_pipeline.md` §7.1 step 4 to remove the "clears the animated-lightmap atlas, then composites" wording.

### Task 2: Per-frame visibility cull of dispatch tiles

At load time, build `chunk_cell_ids: Vec<u32>` with one entry per animated chunk, populated by iterating `BvhLeaf.chunk_range_start..start+count` and stamping the leaf's `cell_id`. Store it on `AnimatedLightmapResources` alongside the master (uncut) tile list.

Change `dispatch_tiles_buffer` to `STORAGE | COPY_DST` and size it to the master tile count. Add persistent scratch buffers to `DispatchState` (`scratch_tiles: Vec<DispatchTile>` and `scratch_bytes: Vec<u8>`) so nothing allocates per frame — `clear() + extend()` each frame. Each frame, before encoding the compose dispatch: given `&VisibleCells`, walk the master tile list and push tiles whose `chunk_cell_ids[tile.chunk_idx]` is set in the visible bitmask (or push all tiles for `DrawAll`). Repack into `scratch_bytes` (reusing the existing `pack_dispatch_tiles` byte layout), `queue.write_buffer` the trimmed slice, and dispatch `trimmed_len` workgroups.

When `trimmed_len == 0` (e.g., every cell with animated chunks is off-screen), skip `write_buffer` and the entire compute-pass encoding — don't begin a compute pass that does nothing. The atlas keeps its prior-frame contents, which is fine because the forward pass will not sample any tile from an invisible cell.

Log `kept/total` at `debug` level using the prev-value dedup pattern already used in `render/mod.rs` around the cull logger (log on `kept != prev_kept` rather than on a timer — no wall-clock helper exists in this module). Reset `prev_kept` on map load.

Plumb `&VisibleCells` into `animated_lightmap.dispatch()` from the call site in `render/mod.rs` (same place the BVH cull already consumes it).

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2. Both edit `DispatchState` and the body of `dispatch()`; realistic merge path is one PR. Task 1 lands first because it simplifies the shape Task 2 restructures.

## Rough sketch

- Shader: `postretro/src/shaders/animated_lightmap_compose.wgsl` — delete `clear_main` (lines 93–100).
- Rust: `postretro/src/render/animated_lightmap.rs` — `DispatchState` loses `clear_pipeline` and `atlas_size`; `dispatch()` collapses to a single compute pass with one pipeline set and one `dispatch_workgroups` call driven by the trimmed count.
- Load path: wherever `AnimatedLightChunks` + BVH leaves are both in hand (same load step that already builds `ChunkAtlasRect`s), fill `chunk_cell_ids` in one pass over `BvhLeaves`.
- Caller: `postretro/src/render/mod.rs` around line 1938 — the `visible: &VisibleCells` reference used for BVH cull logging is already in scope; pass it into `dispatch()`.
- Diagnostic: `log::debug!("animated_lm tiles: {}/{} visible", kept, total)` gated by `kept != prev_kept` — matches the dedup pattern used by the BVH cull logger in `render/mod.rs` and needs no new rate-limit helper.

## Open questions

- `VisibleCells::DrawAll` on a very dense map still dispatches the full tile set. Not a regression vs. today (we still win from clear removal), and `DrawAll` only fires on exterior-camera / missing-visibility fallbacks. No action, flagging only.
