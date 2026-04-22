# Animated Light Weight Maps

## Goal

Bake per-animated-light contribution weight maps into each chunk from Spec 1, then compose them at runtime into an animated lightmap contribution atlas that adds to the static baked atlas in the forward shader. Animated lights get smooth, per-texel, shadowed direct contribution at runtime with memory scaling by local overlap density rather than total light count.

Consumes Spec 1's `AnimatedLightChunks` section. Removes animated lights from the static baked composite in `lightmap_bake`. Reuses the `AnimationDescriptor` already driving SH volume animated layers.

See `research.md` for prior-art survey.

## Scope

### In scope

- New PRL section `AnimatedLightWeightMaps` (SectionId = 25). Per-chunk atlas rectangle; within each rectangle, one per-covered-texel `(offset, count)` into a flat `(light_index, weight)` pool. `offset_counts.len()` equals the sum of `rect.width × rect.height` across chunks — uncovered atlas texels have no entry. Scalar monochrome weight per (texel, light); color comes from the descriptor. `light_index` references the animated-light subset (same ordering Spec 1's chunk lists use), not the global `MapLight` array.
- Compile-time baker: for every chunk from Spec 1, resolve the chunk's atlas-texel rectangle from the owning face's chart placement (emitted by the existing lightmap bake) and the chunk's face-local UV sub-region. For each texel in that rectangle, for each animated light in the chunk's light list, compute unshadowed Lambert × distance-falloff × cone (same math as the static bake), run a shadow ray against the shared BVH, and emit `(light_index, weight)` when visible and non-zero. `light_index` is remapped from Spec 1's filtered-list ordering to the animated-subset index matching the `AnimationDescriptor` buffer (relative order preserved; only animated lights are included). The weight is pre-shaded irradiance: the runtime composition sums pre-shaded animated irradiance with the already-directional-shaded static lightmap result.
- Removal of animated-light contribution from the static `lightmap_bake`. Static atlas carries only non-animated static lights after this spec.
- Runtime compute pre-pass that composes the animated-lightmap-contribution atlas. Inputs: the three weight-map storage buffers, a CPU-built `dispatch_tiles` storage buffer (see below), the animation-descriptor storage buffer, the frame-time uniform (from the per-frame uniform bound at group 0). Output: an `Rgba16Float` storage texture (created with `STORAGE_BINDING | TEXTURE_BINDING`) matching the lightmap atlas dimensions. Compose uses a dedicated compute pipeline bind group layout: group 0 is the full frame `Uniforms` buffer (same struct the forward pipeline binds; `uniforms.time` is the cycle-time source); a compute-exclusive group carries the four weight-map / dispatch storage buffers, the descriptor buffer, the shared `anim_samples` buffer that `curve_eval.wgsl` reads, and the storage-texture write view for the animated atlas. The forward pass binds the same animated-atlas texture as a sampled view on bind group 4 alongside the static lightmap. Adapter feature check at init matches the SH volume path. Dispatch is per 8×8 tile: at map-load time the engine expands every `chunk_rect` into one or more `DispatchTile { chunk_idx, tile_origin_x, tile_origin_y, _pad }` entries covering the rect; dispatch count is `dispatch_tiles.len()`, workgroup size is `@workgroup_size(8, 8, 1)`. Threads outside the chunk rect early-out. Chunks smaller than 8×8 pay one workgroup; larger chunks scale linearly; uncovered atlas area pays no thread cost. A compute pre-clear pass (writes vec4(0.0) to every texel via compute dispatch, no adapter feature required) precedes compose each frame; uncovered texels stay zero.
- Forward shader samples the animated-contribution atlas alongside the static directional lightmap and adds the animated term to the static-shaded irradiance. Directional information is baked into the animated weights via Lambert; no direction texture is needed for the animated atlas.
- Descriptor gains a `u32 active` flag in the existing tail padding; `ANIMATION_DESCRIPTOR_SIZE` stays at 48. Scripting toggles `active` each frame via the CPU-side mirror buffer.
- `AnimationDescriptor.brightness` and `.color` evaluation on the GPU uses **Catmull-Rom** interpolation. Descriptor storage unchanged; the sampling function is the prerequisite. Sibling plan `context/plans/ready/animated-curve-eval/` owns the evaluator and the shared WGSL helper used by both the SH animation pass and this compose pass. This spec depends on it.
- Compile-time log: weight-map byte size, texel count, mean lights per covered texel, peak per-chunk texels.
- Unit tests (compiler): single chunk / single light → every covered texel carries exactly one light with Lambert-shaped weight; occlusion (parallel-plate blocker) → zero weight on shadowed texels; determinism (two builds → byte-identical section).
- Unit tests (runtime, CPU-side where possible): descriptor sampling (Catmull-Rom round-trip), buffer packing symmetry, compose-pass output dimensions.
- Debug visualization: env-var-gated shader path that outputs per-texel animated-light count or single-light isolation. Scope-contingent — drop first if time runs short.

### Out of scope

- Non-linear `LightAnimation` evaluator. Plan 2 owns it. This spec depends on Catmull-Rom landing upstream; it does not absorb the work.
- Runtime BVH traversal as a light-gather mechanism. Chunks are addressed by face via the baked atlas UV; no per-frame spatial queries.
- Luxel-space baking. Texels only. UV density comes from the lightmap packer.
- Dynamic resolution, atlas resize, or chunk re-bake at runtime.
- FGD entity wiring for animated lights. Scripts spawn them; the level compiler does not.
- Particle additive blending changes. Weight-map composition is a sum; particle pass (plan 3) is unaffected.
- Any change to the BVH section or BVH traversal shader.
- Specular from animated lights. Weight maps drive diffuse irradiance only; specular for runtime-active lights already runs through the dynamic-direct loop when a scripted light is in-scene.

## Acceptance criteria

- [ ] `AnimatedLightWeightMaps` section round-trips via `to_bytes` / `from_bytes` and is emitted by `prl-build` for every map that has at least one animated light.
- [ ] After compile, the static lightmap atlas shows no contribution from animated lights: on a synthetic one-face map lit only by an animated light, the static atlas byte-matches the baseline produced by compiling the same map with zero lights.
- [ ] On that same map, the runtime compose pass produces a non-zero animated-contribution atlas whose magnitude tracks the descriptor's brightness curve over time.
- [ ] Uncovered atlas texels read as zero in the forward pass: verified by a test map where `chunk_rect` covers only a small region and a fragment shader sample at an uncovered texel returns (0, 0, 0).
- [ ] Summing weight-map byte size across the bundled test-maps stays under 8 MB per map. The compile log reports byte size, covered-texel count, and mean animated lights per covered texel. On the bundled test-maps, mean lights per covered texel stays ≤ 2.5. (Worst-case at cap=4 and full atlas coverage is 40 MB; the mean-lights metric guards against approaching that ceiling.)
- [ ] Every covered texel's light list has length ≤ `MAX_ANIMATED_LIGHTS_PER_CHUNK` (sourced from the format crate, defined by Spec 1). Every light index in a texel list appears in the parent chunk's light index list, using the same animated-subset ordering.
- [ ] Every `light_index` value in `texel_lights` is < the animated-light count implied by `AnimatedLightChunks` (equivalently, < the sized length of the runtime `AnimationDescriptor` buffer). The engine's post-load cross-section validator asserts this before the first compose dispatch and refuses to load maps that fail.
- [ ] `chunk_rects.len()` equals the emitted `AnimatedLightChunks.chunks.len()`, and `chunk_rects[i]` corresponds to `chunks[i]`.
- [ ] `chunk_rects[i].texel_offset == Σ_{j<i} (chunk_rects[j].width × chunk_rects[j].height)` for all i; `offset_counts.len() == Σ_i (chunk_rects[i].width × chunk_rects[i].height)`. Both invariants verified by a unit test.
- [ ] No two entries in `chunk_rects` overlap in atlas coordinate space. The baker asserts this at compile time.
- [ ] A parallel-plate occlusion test (light above, blocker between light and surface) produces zero weight on shadowed texels and non-zero on lit texels.
- [ ] Two compiler runs on the same input produce byte-identical `AnimatedLightWeightMaps` sections.
- [ ] Toggling a descriptor's `active` flag from the CPU side zeros that light's contribution in both the next composed animated atlas and the SH volume animated layer, on the same frame, without recompiling the map. Integration test: load a one-light fixture map, render frame 1 with `active=1` (assert non-zero animated-atlas texels at the chunk), set `active=0`, render frame 2 (assert those texels return to zero).
- [ ] Descriptor evaluator (Catmull-Rom) reconstructs authored keyframes exactly at sample points and produces continuous first derivatives between samples. Unit test asserts both.
- [ ] Both `forward.wgsl` (eval_animated_brightness / eval_animated_color path) and `animated_lightmap_compose.wgsl` import and call the same `sample_curve_catmull_rom` symbol; no duplicate evaluator remains in the codebase.
- [ ] Frame ordering unchanged: the compose pass runs inside Render, after the BVH cull pass and before the depth prepass. `POSTRETRO_GPU_TIMING=1` lists it as a distinct pass.
- [ ] When `AnimatedLightWeightMaps` is absent (maps with zero animated lights), the engine loads and renders without error; the forward pass receives a 1×1 zero `Rgba16Float` texture on the animated-atlas binding and produces no animated contribution.
- [ ] `cargo check -p postretro -p postretro-level-compiler -p postretro-level-format` clean. Existing lightmap / SH tests still pass.

## Tasks

### Task 1: Section definition

Add `AnimatedLightWeightMaps = 25` to `SectionId`. Define a section carrying: per-chunk `chunk_rects: Vec<ChunkAtlasRect>` (field layout in Rough sketch), per-covered-texel `offset_counts: Vec<(u32 offset, u32 count)>`, flat `texel_lights: Vec<(u32 light_index, f32 weight)>`. Symmetric `to_bytes` / `from_bytes`. Follow the existing `ChunkLightListSection` indirect-encoding shape — already proven for similar per-chunk variable-length data.

### Task 2: Remove animated lights from the static lightmap bake

In `lightmap_bake`, filter the `static_lights` pass to `!light.is_dynamic && light.animation.is_none()`. All animated non-dynamic lights — including `bake_only` animated lights — are removed from the static bake; their contribution moves to the compose path.

Also update Spec 1's chunk-list filter in `animated_light_chunks.rs` from `!is_dynamic && !bake_only && animation.is_some()` to `!is_dynamic && animation.is_some()` so `bake_only` animated lights enter the chunk list and receive weight-map entries. This is a retroactive change to shipped Spec 1 code.

Update the existing test `static_flag_bakes_but_dynamic_does_not` to cover the animated and `bake_only` animated cases.

### Task 3: Weight-map baker

New module under `postretro-level-compiler`. Consumes: `AnimatedLightChunks` section, `LightInfluence` records, `MapLight` animation-bearing lights, BVH + primitives for shadow rays, the lightmap atlas geometry (width, height, per-face placements). For every chunk:

1. Resolve the chunk's atlas-texel rectangle from the face's chart placement and the chunk's UV sub-region. Round outward to integer texel boundaries (floor min, ceil max). Assert that no two chunks on the same face produce overlapping atlas rectangles after rounding — guaranteed by Spec 1's non-overlapping UV sub-regions plus any sub-texel gap between adjacent chunks.
2. For every texel in that rectangle, for every animated light in the chunk's light list: recompute the per-texel world position and normal using the same chart-rasterization math as `lightmap_bake` — these values are not persisted and must be derived fresh from the chart placement and UV coordinates. Compute unshadowed Lambert contribution (distance falloff × N·L × spotlight cone — identical to `lightmap_bake`). Run a shadow ray against the shared BVH using the same epsilon/bias as `lightmap_bake` to avoid self-intersection. Emit `(remapped_light_index, weight)` when visible and non-zero. Spec 1's chunk-list filter and the `AnimationDescriptor` buffer use the same filter (`!is_dynamic && animation.is_some()`), so chunk-list light indices directly reference descriptor buffer slots — no remap needed (see Settled decisions). Emit a zero-count entry for texels where no light contributes so `offset_counts.len()` matches the total rect area invariant.
3. Pack into the section's indirect format.

Parallelize chunks with `rayon`. Determinism: iterate chunks in section order, lights in chunk-list order, write output slots at known offsets.

### Task 4: Descriptor active-flag + buffer plumbing

Extend the **GPU-side** `AnimationDescriptor` (in `postretro/src/render/sh_volume.rs`, 48-byte stride) with a `u32 active` flag. The on-disk format-crate `AnimationDescriptor` (`postretro-level-format/src/sh_volume.rs`) is unchanged — `active` is a runtime-only flag set by scripts, not persisted in PRL. `write_descriptor_bytes` is updated to emit the new field; `ANIMATION_DESCRIPTOR_SIZE` stays at 48. Exact field placement is determined by the implementor given the descriptor's state when this task lands — Plan 2 Sub-plan 1's direction channel also uses tail padding, so coordinate placement with that spec before committing the layout. Proposed tail layout (must be confirmed with scripting-foundation Plan 2 Sub-plan 1 before committing): bytes 36–39 = `active: u32`; bytes 40–43 = `direction_offset: u32` (Plan 2 Sub-plan 1); bytes 44–47 = `direction_count: u32` (Plan 2 Sub-plan 1). All three consumers must land their changes in a coordinated commit or leave the unowned fields as `_pad` until their spec lands. Note: the existing GPU-side `AnimationDescriptor` already carries `period`, `phase`, `brightness_offset`, `brightness_count`, `base_color`, `color_offset`, `color_count`, and a trailing `_padding: vec2<f32>` that absorbs the remaining 12 bytes (the last 4 of which are an implicit alignment gap after `color_count`). The compose shader in Task 5 consumes all of `period`, `phase`, `brightness_offset`/`brightness_count`, `color_offset`/`color_count`, and `base_color` — this task only adds `active` into the existing tail padding. Engine maintains a CPU-side mirror of the descriptor buffer, writes it every frame via `queue.write_buffer` before the compose pass. Scripting sets the flag on light spawn / despawn / toggle. Also update the SH-animation evaluation path in `forward.wgsl` (`eval_animated_brightness` / `eval_animated_color`) to multiply by `desc.active` so inactive animated lights contribute nothing to the SH volume either. This is part of Task 4, not Task 6.

### Task 5: Compose compute pass

New WGSL compute shader `animated_lightmap_compose.wgsl` with `@workgroup_size(8, 8, 1)`. The shader declares `struct Uniforms` identically to `forward.wgsl` and binds it at group 0 binding 0 (not a narrower `FrameTime` substruct — the binding must match the full buffer layout); cycle time is read as `uniforms.time`. Compute-stage bindings include `anim_samples: array<f32>` alongside `chunk_rects`, `offset_counts`, `texel_lights`, `dispatch_tiles`, `descriptors`, and the storage-texture write view. At map-load time a CPU pass in `animated_lightmap.rs` expands `chunk_rects` into a flat `Vec<DispatchTile>` where `DispatchTile = { chunk_idx: u32, tile_origin_x: u32, tile_origin_y: u32, _pad: u32 }` — one entry per 8×8 tile needed to cover each chunk rect (`ceil(rect.width / 8) × ceil(rect.height / 8)` tiles per chunk). Dispatch count = `dispatch_tiles.len()`. Per workgroup: read `tile = dispatch_tiles[workgroup_id.x]`, then `rect = chunk_rects[tile.chunk_idx]`. Per thread: `rect_x = tile.tile_origin_x + local_invocation_id.x`, `rect_y = tile.tile_origin_y + local_invocation_id.y`; early-out if `rect_x >= rect.width || rect_y >= rect.height`; else `texel_idx = rect.texel_offset + rect_y * rect.width + rect_x`; iterate the texel's light list; sample each descriptor's brightness and color via the shared Catmull-Rom helpers at the current cycle time (using the same `fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase)` formula as `forward.wgsl`); multiply by weight and `active`; accumulate and `textureStore` once at `(rect.atlas_x + rect_x, rect.atlas_y + rect_y)`.

If `dispatch_tiles.len()` exceeds `max_compute_workgroups_per_dimension` (65535 by default), dispatch as `(ceil(N / K), K, 1)` for a K ≤ 65535 chosen at dispatch time and compute the flat tile index in the shader as `workgroup_id.x * K + workgroup_id.y`. Alternatively, assert the tile count at map-load and refuse maps that exceed the limit — acceptable for the initial implementation. Implementer picks the approach; the constraint is documented here.

When the PRL file has no `AnimatedLightWeightMaps` section (maps with zero animated lights), skip the compose pipeline and atlas allocation entirely and bind a dummy 1×1 `Rgba16Float` zero texture on forward group 4 in the animated-atlas slot. Follows the SH volume dummy pattern (`dummy_storage_buffer` / `dummy_descriptor_buffer` in `sh_volume.rs`, which supplies minimum-size bindings when no SH section is present).

Forward fragment sampling uses the existing `lightmap_sampler` (a `NonFiltering` sampler): `Rgba16Float` is non-filterable at wgpu default limits, so a linear/filtering sampler would fail validation.

Each frame the encoder dispatches a compute pre-clear pass over the animated atlas before dispatching compose (writes `vec4(0.0)` to every texel via compute — no adapter feature required); uncovered atlas texels stay at zero. Implementer may combine the clear and compose into one shader or use a separate clear shader.

At map load time in `animated_lightmap.rs::new`, run a cross-section validator: assert `chunk_rects.len() == AnimatedLightChunks.chunks.len()`; assert every `light_index` in `texel_lights` is < the animated-light count implied by `AnimatedLightChunks`; assert the `texel_offset` prefix-sum invariant. Log a clear error and refuse to load maps that fail validation.

In `animated_lightmap.rs`, when building the compute pipeline, concatenate `curve_eval.wgsl` with `animated_lightmap_compose.wgsl` before passing to `device.create_shader_module`, following the same pattern as the forward-pass shader build.

Renderer owns this pass. Runs inside Render, after the BVH cull pass and before the depth prepass. wgpu infers the storage→sampled barrier from the bind-group usage change (compute write → fragment sample), so no explicit pipeline barrier is needed. The animated-lightmap storage texture is created with `STORAGE_BINDING | TEXTURE_BINDING` (no `COPY_DST` required — the clear is done via compute dispatch). Two views of the same texture: the compose pass binds the write view as a storage texture; the forward pass binds the read view as `texture_2d<f32>` on bind group 4 alongside the static atlas (same pattern as the SH volume pass). Forward shader composes `static_directional_shaded + animated_sample` at the same UV; the animated term is already pre-shaded irradiance (Task 3), so no N·L is applied at runtime.

GPU timing: follow the existing `TIMING_PAIR_*` pattern in `postretro/src/render/mod.rs`. Add `const TIMING_PAIR_ANIMATED_LM_COMPOSE: usize = …;` (inserted in frame order, before `TIMING_PAIR_DEPTH_PREPASS`) and bump `TIMING_PAIR_COUNT`. Extend the `pass_labels` vec with `"animated_lm_compose"` at the new index. Wire the compose compute pass via `timing.as_ref().map(|t| t.compute_pass_writes(TIMING_PAIR_ANIMATED_LM_COMPOSE))` on its descriptor, mirroring the cull pass.

### Task 6: Shared Catmull-Rom WGSL helper

Consume the `sample_curve_catmull_rom` (scalar brightness) and `sample_color_catmull_rom` (RGB color) helpers landed by `animated-curve-eval`. Wire `animated_lightmap_compose.wgsl` to include/concatenate `curve_eval.wgsl` (per `animated-curve-eval`'s shader-source conventions). The `forward.wgsl` SH animation path refactor is owned by `animated-curve-eval` Task 3 — no duplicate work here.

### Task 7: Debug visualization (scope-contingent)

Env var `POSTRETRO_ANIMATED_LM_DEBUG` selects a shader variant that visualizes per-texel light count or isolates one light. No UI, no persistence.

### Task 8: Tests

Compiler tests: single-chunk single-light weight shape, occluded texel zeroed, determinism, byte-size budget on a synthetic map. Runtime tests: descriptor round-trip through the pack/unpack path, compose-pass output dimensions match the static atlas, active-flag masking. An end-to-end fixture map with one animated light, compiled and loaded, feeds a one-frame render and asserts the composed atlas is non-zero at the expected chunk's texels.

Task author writes the fixture `.map` files under `assets/maps/` (e.g. `test_animated_weight_maps_single.map`, `test_animated_weight_maps_occluded.map`, `test_animated_weight_maps_cap.map`) covering: (a) one animated light lighting one face, (b) a parallel-plate occluder between light and surface, (c) multiple animated lights overlapping at `MAX_ANIMATED_LIGHTS_PER_CHUNK`.

## Sequencing

**Phase 0 (prerequisite, out of spec):** `animated-curve-eval` lands the shared Catmull-Rom helper and the `LightAnimation` evaluator.
**Phase 1 (sequential):** Task 1 — section type blocks the baker and runtime loader.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — bake removal, weight baker, descriptor `active` flag. Independent code paths.
**Phase 3 (sequential):** Task 5 — compose pass consumes the section and the descriptor flag. Folds in Task 6 integration (single commit) so the SH animation pass and compose pass share one WGSL helper.
**Phase 4 (concurrent):** Task 7, Task 8 — debug shader and tests.

## Rough sketch

Section layout (illustrative — field order may shuffle to pack cleanly):

```rust
#[repr(C)]
pub struct ChunkAtlasRect {
    atlas_x: u32,
    atlas_y: u32,
    width: u32,
    height: u32,
    texel_offset: u32, // index into per-texel offset_counts
}

pub struct AnimatedLightWeightMapsSection {
    pub chunk_rects: Vec<ChunkAtlasRect>,
    pub offset_counts: Vec<(u32, u32)>, // (offset, count) into texel_lights
    pub texel_lights: Vec<(u32, f32)>,  // (light_index, weight)
}
```

GPU binding shape (two layouts): **Compose compute pipeline** — group 0: the full frame `Uniforms` buffer (same struct `forward.wgsl` binds; the compose shader declares `struct Uniforms` identically and reads `uniforms.time`); compute-exclusive group: `chunk_rects` storage buffer, `offset_counts` storage buffer, `texel_lights` storage buffer, `dispatch_tiles` storage buffer, `descriptors` storage buffer, `anim_samples: array<f32>` storage buffer (the flat brightness/color sample pool the `curve_eval.wgsl` helper reads by lexical name; same buffer the SH animation path binds, declared here at the compose pipeline's `(group, binding)`), `animated_lm_atlas: texture_storage_2d<rgba16float, write>`. That is six storage buffers plus one storage texture on the compute stage, within wgpu's default `max_storage_buffers_per_shader_stage` of 8. **Forward fragment pipeline** — group 4 gains one additional binding: `animated_lm_atlas` as `texture_2d<f32>` sampled view (two views of the same texture, same as the SH volume pattern). The forward pass samples it through the existing `lightmap_sampler` — a `NonFiltering` sampler, required because `Rgba16Float` is non-filterable at wgpu default limits.

Compose loop (WGSL pseudocode, `@workgroup_size(8, 8, 1)` — `workgroup_id.x` selects an 8×8 tile of some chunk):

```
let tile = dispatch_tiles[workgroup_id.x];
let rect = chunk_rects[tile.chunk_idx];
let rect_x = tile.tile_origin_x + local_invocation_id.x;
let rect_y = tile.tile_origin_y + local_invocation_id.y;
if (rect_x >= rect.width || rect_y >= rect.height) { return; }
let texel_idx = rect.texel_offset + rect_y * rect.width + rect_x;
let oc = offset_counts[texel_idx];
var accum = vec3<f32>(0.0);
for (var i = 0u; i < oc.count; i++) {
    let entry = texel_lights[oc.offset + i];
    let desc = descriptors[entry.light_index];
    if (desc.active == 0u) { continue; }
    let t = fract(uniforms.time / max(desc.period, 1.0e-6) + desc.phase);
    let b = sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t);
    let c = sample_color_catmull_rom(desc.color_offset, desc.color_count, t, desc.base_color);
    accum += c * b * entry.weight;
}
textureStore(animated_lm_atlas, vec2<i32>(rect.atlas_x + rect_x, rect.atlas_y + rect_y), vec4(accum, 1.0));
```

Key files touched:
- `postretro-level-format/src/lib.rs` — `SectionId::AnimatedLightWeightMaps`.
- `postretro-level-format/src/animated_light_weight_maps.rs` — new.
- `postretro/src/render/sh_volume.rs` — GPU `AnimationDescriptor` gains `active`; `write_descriptor_bytes` updated.
- `postretro-level-compiler/src/lightmap_bake.rs` — filter out animated lights.
- `postretro-level-compiler/src/animated_light_chunks.rs` — remove `!bake_only` from the chunk-list filter (retroactive Spec 1 change).
- `postretro-level-compiler/src/animated_light_weight_maps.rs` — new baker.
- `postretro-level-compiler/src/main.rs` — emit call.
- `postretro/src/render/animated_lightmap.rs` — new compose-pass module; handles the compute pre-clear and compose dispatches and concatenates `curve_eval.wgsl` with the compose shader source at pipeline-build time.
- `postretro/src/shaders/animated_lightmap_compose.wgsl` — new. Handles both the clear and compose passes (implementer may combine into one shader or use a separate clear shader).
- `postretro/src/shaders/forward.wgsl` — add animated-contribution sample; `eval_animated_brightness` / `eval_animated_color` gain the `desc.active` gate so inactive lights contribute nothing to the SH volume.
- `postretro/src/render/mod.rs` (or wherever `POSTRETRO_GPU_TIMING` pass entries are registered) — add compose pass entry.

## Settled decisions

- **Indirect per-texel encoding, offset + count into flat list.** Follows `ChunkLightListSection` precedent. Matches the 4-8 MB memory target because per-texel lists scale with local overlap density (1-3 lights typical), not total animated-light count. Inline fixed-cap-4 rejected — wastes 16 B per covered texel on the common case.
- **`offset_counts` covers covered texels only, not the full atlas.** Sized to `Σ rect.width × rect.height`. Sizing to the atlas (1M entries for 1024²) would consume the memory budget by itself.
- **Animated-atlas resolution matches the static lightmap (1024² today).** One UV, one fetch per fragment; no sampling-path branching. Resolution will be evaluated by manual testing and iteration after implementation — halving to 512² remains an option if compose proves budget-dominant or authored flicker reads too crisp against low-res base textures.
- **`chunk_rects` stored as a storage buffer.** One `ChunkAtlasRect` is 20 B; at 10k chunks the upload is 200 KB — fine. Switching to a 1D texture past ~50k chunks is an implementer call at that threshold; no pre-implementation decision needed.
- **Animated term is pre-shaded Lambert irradiance, summed with the static-shaded irradiance at fragment time.** Directional information is baked into the weight; the animated atlas has no direction channel. Forward shader does the directional evaluation on the static lightmap as today, then adds the animated sample — no runtime N·L for animated lights.
- **Catmull-Rom for `AnimationDescriptor` curve evaluation.** Author-friendly (keyframes only, no explicit tangents), cheap (4-tap inline), shape-preserving for smooth pulses. Overshoot on hard strobe transitions is on-brand for the retro aesthetic. Plan 2 owns the implementation; Spec 2 depends on it.
- **Compose runs as a compute pre-pass inside Render**, before the depth prepass. Produces a storage-texture atlas matching the static lightmap dimensions. Forward shader adds `static + animated` with no sampling-path branching. Frame ordering unchanged.
- **Compose dispatches one workgroup per 8×8 tile, not one per chunk.** A CPU load-time pass expands `chunk_rects` into a flat `dispatch_tiles` buffer covering every rect with `ceil(w/8) × ceil(h/8)` tiles per chunk. Workgroup size `@workgroup_size(8, 8, 1)` (64 threads) stays under every WebGPU adapter limit regardless of chunk dimensions. Per-chunk dispatch was rejected: it forces the workgroup to size to the largest possible rect, bloats thread count, and risks exceeding the 1024-thread adapter limit at high texel density.
- **Descriptor gains an `active: u32` flag.** Scripts toggle per frame; shader multiplies contribution by `active`. No level reload required for light despawn.
- **`texel_offset` is a prefix sum over prior chunk texel counts.** `chunk_rects[i].texel_offset == Σ_{j<i} (chunk_rects[j].width × chunk_rects[j].height)`. `offset_counts` is laid out in `chunk_rects` index order. This invariant is what makes the WGSL `texel_idx` calculation correct; the baker asserts it at emit time.
- **`light_index` is in `AnimationDescriptor` buffer ordering.** Spec 1's chunk-list filter (`!is_dynamic && animation.is_some()`) and the descriptor buffer use the same filter and iteration order, so chunk-list indices directly reference descriptor slots. No remap at bake time.
- **`bake_only` animated lights participate in weight-map compose.** `bake_only` means excluded from `AlphaLights` and `LightInfluence` — no runtime specular, no runtime direct presence. It does not mean excluded from diffuse animation. Animated `bake_only` lights enter Spec 1's chunk list and receive weight-map entries like any other animated non-dynamic light. Requires the retroactive Spec 1 filter change in Task 2.
- **Atlas-rect rounding is outward.** When mapping a chunk's face-local UV sub-region to integer atlas texels, round outward (floor min, ceil max). Chunks on the same face must not produce overlapping `chunk_rects`; the baker asserts this. Guaranteed by Spec 1's non-overlapping UV sub-regions.
- **`active` defaults to 1 on first buffer upload.** The CPU-side descriptor mirror initializes `active = 1u32` for every descriptor when the map loads. Scripts must explicitly set it to 0 to disable a light; a zero-initialized buffer would silently darken all animated lights.
- **Same `AnimationDescriptor` drives both SH animated layers and the weight-map compose.** One descriptor buffer, two consumers. Adding `active` is a pure extension; stride stays at 48 bytes (fills existing padding).
- **Spec 1's per-chunk cap `MAX_ANIMATED_LIGHTS_PER_CHUNK` bounds the shader-side texel loop.** Unrolled at the cap (proposed 4). Texel lists never exceed the cap because baked weights only exist for lights in the parent chunk's list.
- **Weight storage is `f32`, not `f16`.** Monochrome scalar irradiance values benefit from full precision; quantization at bake time would compound with Catmull-Rom interpolation error. `f32` = 8 bytes per entry; at the 4–8 MB memory target, this is tractable.
