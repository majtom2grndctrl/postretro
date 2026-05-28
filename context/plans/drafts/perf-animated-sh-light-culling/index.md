# Perf — Animated SH Light Culling

## Goal

Cut the per-frame SH compose workload and the GPU storage footprint of baked animated-light delta volumes so a single bind group fits inside the WebGPU `max_storage_buffer_binding_size` spec floor (128 MiB) and the per-probe inner loop touches only the lights that actually reach the probe. Today, `campaign-test.prl` ships 21 dense per-light AABB grids that total ~185 MB after f16→f32 expansion, and every base SH probe loops over every animated light. The raised wgpu limit (512 MiB) just landed as a stopgap; this plan removes that load-bearing dependency.

## Scope

### In scope
- Sparse per-light delta storage on disk: do not bake or ship probes outside the light's portal-reachable region within its AABB.
- f16 probes stay f16 through to the shader (drop the f16→f32 expansion in `build_delta_buffers`).
- Coarse spatial index over the base SH volume that maps each base probe to a short list of overlapping animated lights.
- Compose shader iterates only the indexed light list per probe (replaces the current "loop all lights, AABB test").
- New PRL section version bump for the sparse + index payload (pre-release; no compat shim).
- `campaign-test.prl` rebakes end-to-end and renders without raising any wgpu limit past spec floors.

### Out of scope
- Octree / BVH spatial index. Coarse uniform grid is the M10 minimum; revisit only if the uniform grid's worst-case cell list overflows the per-thread budget.
- Dynamic re-binning when lights move. Animated lights are static-positioned in the bake; movement is intensity/color only.
- The 1,962,545-tile compute-dispatch issue on the animated-lightmap install path. Same dense-everything root cause but a different code path; tracked separately. Note in the implementation: if the affinity-cell decomposition introduced here is reusable for that pass, mention it in the commit message so the follow-up plan can pick it up.
- Lowering `REQUIRED_STORAGE_BUFFER_BINDING_SIZE`. Leave the raised limit in place; this plan makes it non-load-bearing, not removed.
- Tighter per-light AABBs. The reachable-region clip subsumes most of the win.

## Acceptance criteria

- [ ] `cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o /tmp/out.prl` succeeds; resulting `.prl` is smaller than the pre-change baseline for the `DeltaShVolumes` section.
- [ ] `cargo run -p postretro -- /tmp/out.prl` boots and reaches the first rendered frame with no wgpu validation errors.
- [ ] On `campaign-test.prl`, the combined GPU footprint of every storage binding in `SH Compose Bind Group` is below 128 MiB. Verify via a startup log line that reports per-binding sizes for that bind group.
- [ ] Compose shader's per-probe loop bound is the affinity-cell light-list length, not `delta_light_count`. Verifiable by reading `sh_compose.wgsl` post-change.
- [ ] Visual parity on `campaign-test.prl`: the SH base-only debug toggle (existing) and the new full-compose path produce equivalent total irradiance for any probe inside every animated light's reachable region. Tolerance: per-band ≤ 1e-3 relative.
- [ ] PRL section version reject path: loading a pre-bump `.prl` with the new engine logs a clear "recompile with current prl-build" error and exits, mirroring the existing `DELTA_SH_VOLUMES_VERSION` mismatch path.
- [ ] Renderer no longer requests `max_storage_buffer_binding_size` above the WebGPU spec floor (`134_217_728`) to load `campaign-test.prl`. (The raised limit may remain requested as headroom, but campaign-test must not require it.)

## Tasks

### Task 1: Affinity grid decomposition (bake)
Decide the affinity-cell shape and produce, per animated light, the set of affinity cells the light's contribution should be stored at. An affinity cell is a coarse cube of base SH probes (e.g. 4×4×4 base probes per affinity cell — exact factor is a tuning knob, picked once at bake time and stored). For each light, intersect the light's AABB with the BSP-portal-reachable region from the light's position. The result is a list of affinity cells per light, with the implied list of base-probe positions covered. This lives in `crates/level-compiler/` next to the existing delta SH bake.

### Task 2: Sparse delta probe bake
Rework the delta bake so each light's emitted probes correspond only to base-SH-grid positions inside its affinity-cell list (Task 1). The bake stops producing AABB-dense grids and instead emits `(base_probe_index, sh_f16_coeffs)` pairs (or a per-light run-length form keyed to affinity cells — pick at impl time). Probes outside the reachable region are not baked, not stored. Updates `DeltaShVolumesSection` to the new shape; bumps `DELTA_SH_VOLUMES_VERSION`.

### Task 3: Per-probe affinity index
Build the runtime lookup: for each base SH probe (or each affinity cell, if the per-probe table is too large), the list of animated-light indices whose sparse data covers it. Emit as a flat CSR-style pair: `affinity_offsets: Vec<u32>` (length affinity_cell_count + 1) and `affinity_lights: Vec<u32>` (flat indices). Built at compile time from Task 1's per-light affinity-cell lists. Stored either alongside the sparse probes in the bumped `DeltaShVolumesSection` or as a sibling section — decide based on whether the level loader can read both atomically. Loader uploads both as storage buffers in the same group as the existing delta bindings.

### Task 4: Compose shader rewrite
Replace the per-probe `for li in 0..delta_light_count` loop in `sh_compose.wgsl` with: compute the probe's affinity-cell index → load `[start, end) = affinity_offsets[cell..cell+1]` → loop `for i in start..end` reading `affinity_lights[i]` as the light index. Each iteration looks up the light's sparse probe contribution directly by base-probe index (no AABB test, no descriptor-out-of-range path — descriptor validation now happens once at bake/load). f16 stays f16 in the storage buffer; sample via `unpack2x16float`. Existing dev-tools `base-only` override (write 0 to the light count uniform) still needs to work — gate via the affinity-offset length, not a separate light count.

### Task 5: Verify and tune
Run campaign-test end-to-end. Measure: bake time delta, `.prl` size delta, GPU storage footprint of the SH Compose Bind Group, per-frame compose dispatch time (with `POSTRETRO_GPU_TIMING=1`). Confirm AC items. If footprint is still over 128 MiB, tune the affinity-cell factor (Task 1 knob) and re-bake. Capture the chosen factor in a code constant with a short rationale comment.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the affinity-cell granularity all later tasks consume.
**Phase 2 (concurrent):** Task 2, Task 3 — both build on Task 1's per-light affinity-cell lists; one emits sparse probes, the other emits the index. They share the bumped `DeltaShVolumesSection` definition so coordinate the layout change in one commit, then split the file-emit and the index-emit edits.
**Phase 3 (sequential):** Task 4 — consumes the new section shape and bindings from Phase 2.
**Phase 4 (sequential):** Task 5 — observes the integrated system; only meaningful once Task 4 ships.

## Rough sketch

Naming: introduce `AffinityCell` as the spatial unit in both bake and runtime. An affinity grid covers the base SH volume's AABB at integer factor `AFFINITY_FACTOR` (initial value: 4, i.e. one affinity cell = 4×4×4 base probes ≈ 64 m³ at 1.0 m base probe spacing). Affinity-cell index = `floor(base_probe_index_xyz / AFFINITY_FACTOR)` linearized z-major.

Reachable-region clip (Task 1) does not require new portal/BSP plumbing — the compiler already produces portal connectivity for runtime visibility. Reuse the same per-leaf reachability with the light's containing leaf as the seed.

Sparse delta storage (Task 2): two competing forms — per-base-probe `(base_idx, coeffs[27])` pairs vs. per-affinity-cell dense sub-blocks of `AFFINITY_FACTOR³` probes plus a per-cell occupancy bitmap. The dense-sub-block form is shader-friendly (constant stride per cell) and likely smaller for clustered reachable regions; the pair form is smaller for sparse coverage. Pick at impl time based on a measurement on campaign-test.

Compose shader (Task 4) keeps the existing `@workgroup_size(4, 4, 4)` dispatch. With `AFFINITY_FACTOR = 4`, one workgroup maps 1:1 to one affinity cell — the inner light loop is read-once per workgroup, not per thread. Worth structuring around.

The Animated SH delta volumes paragraph in `context/lib/rendering_pipeline.md §4` documents the current dense bake. Update at promotion, not in this draft.

## Wire format

`DeltaShVolumesSection` bumps to version 2. Layout below; little-endian throughout; integer signedness and widths follow v1.

| Field | Width | Notes |
|---|---|---|
| `version` | `u8` | `= 2`. v1 path rejects with the existing version-mismatch error. |
| `affinity_factor` | `u8` | Power-of-two cell size in base probes per axis. Stored, not assumed. |
| `affinity_dims` | `3 × u32` | Affinity grid dimensions = `ceil(base_dims / affinity_factor)`. |
| `animated_light_count` | `u32` | As v1. |
| `animation_descriptor_indices` | `u32 × animated_light_count` | As v1. |
| `affinity_offsets` | `u32 × (affinity_cell_count + 1)` | CSR offsets into `affinity_lights`. Last entry = total list length. |
| `affinity_lights` | `u32 × affinity_offsets[-1]` | Flat light-index list. |
| Per-light sparse probe payload | variable | Shape decided in Task 2 (pair form or sub-block form). Per-light header documents which. |

Empty animated-light count: `affinity_offsets` is `[0; affinity_cell_count + 1]`, `affinity_lights` is empty. No probe payload.
Sentinel for "no animation descriptor": same `u32::MAX` convention as v1.

## Open questions

- Affinity-cell factor: 4 is a guess. Final value chosen by measurement in Task 5. If the worst affinity cell holds more lights than the per-thread loop budget tolerates, drop to factor 2 (finer cells, shorter lists).
- Sparse probe storage form: pair vs. sub-block. Pick at Task 2 impl with one measurement on campaign-test. Both fit the wire-format slot.
- Should the affinity index live inside `DeltaShVolumesSection` or as a new sibling PRL section? Same-section keeps load atomic; sibling makes the index reusable if a non-SH consumer (the animated-lightmap install dispatch issue) ends up wanting the same per-cell light list.
- The current code has a known correctness bug — `grid_dimensions` declared probe count can disagree with the written probe count, causing OOB reads ("likely flicker source" per the warning at `sh_compose.rs:343–356`). This plan removes the dense grid entirely, which dissolves that bug class. Confirm at Task 4 that the new shape has no equivalent.
