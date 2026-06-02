---
name: Incremental Bake Per Light
description: v1 — per-light contribution layers cached and composited at bake time, so editing one light re-bakes only its layer. Compiler-internal; runtime and PRL format untouched.
type: plan
---

# Incremental Bake Per Light

> Supersedes the per-element framing of the original stub. See `research.md` (§4d) for the
> grounded analysis that selected per-light over per-face / per-room grain. The blocking plan
> `build-stage-cache/` has shipped; this builds on its `StageCache`/`CacheKey` substrate.
> The folder name `incremental-bake-per-element/` is kept for link stability; the plan is per-light.

## Goal

Cut `prl-build` iteration time for the lighting loop. Decompose the static lightmap (direct) and SH irradiance (indirect) bakes into **per-light contribution layers**, cache each layer keyed on its own inputs, and composite the cached layers at bake time into the `Lightmap` (section 22) and `OctahedralShVolume` (section 34) sections the runtime already consumes. Editing one light re-bakes only that light's layer; the rest load from cache. Runtime and PRL format are untouched — this is purely a compiler caching strategy.

## Why per-light

Keyed on light *influence*, not map topology, so it holds up in large open arenas where one light floods most of the map (the case that breaks per-room/portal grain). Seam-free by construction: a light's contribution is exactly zero at `falloff_range`, so a layer fades to zero at its own support boundary — nothing to stitch. Linear and additive: both bakes sum per-light contributions with no per-light clamp or tone-map. Reuses the animated-light path (`sh_bake::bake_probe_indirect_rgb`, `affinity_grid`, `chunk_light_list_bake`), which already does single-light layer bakes for dynamic lights.

**Deliberate cold-for-warm trade.** Campaign-test's bake is SH-dominated, and per-light SH re-casts each probe's shared primary rays per light — so cold builds get slower by roughly the *average per-probe light-overlap depth* (bounded by influence culling, not total light count). This is an accepted v1 cost: cold/full rebakes are rare, the warm-edit win is the goal, and primary-ray-hit caching (deferred — see Out of scope / Open questions) is the named lever if cold builds regress past tolerance.

## Decomposition reference (read before the tasks)

The per-light pipeline becomes the **canonical** bake; the legacy monolithic lightmap/SH bake is retained only for the directional-light fallback (below). Two consequences the spec leans on throughout:

- **The composite is never cached.** Every build enumerates the current point/spot lights, fetches or bakes each light's layer, and sums them (plus the directional fallback). Deletion, addition, and reordering are handled by construction — there is no cached whole-section result to go stale.
- **Determinism is self-referential.** The lightmap composite (per-texel `irr += contribution·v` summed in light order) reproduces the legacy monolithic lightmap bit-for-bit, because the addition order is identical. The SH composite does **not**: the monolithic SH sums lights inside each ray then projects (`basis·Σrad`), while per-light layers project then sum (`Σ basis·rad`), and float math is non-associative. So the determinism gate compares the cached composite against the **same per-light pipeline run with `--no-cache`**, not against the legacy monolithic bake. Per-light SH is a new, equally-valid bake whose output differs from today's by float rounding.

## Scope

### In scope

- Per-light contribution layers for the **static lightmap** (direct) and **SH irradiance volume** (indirect) bakes, for **point and spot** lights.
- Bake-time compositing of cached + freshly-baked layers into the existing PRL sections.
- Per-layer cache entries on the existing `StageCache`, under **new per-light stage ids** (`"lightmap_layer"` and `"sh_layer"`, distinct from the existing `"lightmap"`/`"sh_volume"`) with their own version constants, keyed by content hash of `(light params, influence-bounded geometry slice, config, atlas/probe-grid layout descriptor)`. Layer entries live in the same `StageCache` directory (real default `<workspace>/.build-caches/prl-cache/`) keyed by these ids.
- Geometry-edit invalidation emergent from the per-layer geometry-slice hash (no separate overlap test): a layer whose influence-bounded geometry slice changed gets a new key and re-bakes; an untouched slice keeps its key and hits.
- Directional-light fallback: a directional-only full-stage bake of each affected section, summed with the per-light point/spot layers in the compositor.
- Corruption handling: a layer entry that fails length/hash validation or deserialization is treated as a miss (warn, re-bake), mirroring `StageCache::get`.
- Determinism: layers composite in a fixed light order so the cached composite is byte-identical to the uncached (`--no-cache`) per-light composite.
- A determinism gate test (self-referential, as above) and a storage-profiling spike (Task 1) gating the substrate before wiring lands.

### Out of scope

- **Per-light layers for directional / sun lights.** Whole-map influence yields no sparsity and invalidates on nearly every edit. They go through a directional-only full-stage bake, summed into the composite.
- **Per-face lightmap atlas repartitioning** and **per-room / portal grouping** (`research.md` §4b). Atlas layout is unchanged: all layers share the one atlas the current packer produces.
- **Runtime or PRL format changes.** No new shipped section; the runtime never sees per-light layers.
- **SDF atlas stage** and the **animated weight-map stage** (already cached whole-stage). Untouched.
- **Bit-compatibility with today's SH output.** Per-light SH is a new bake (see Decomposition reference).
- **Primary-ray-hit caching for SH** (reusing per-probe geometry hits across light layers). A future cold-build optimization; v1 re-casts per light like the delta path.
- **Multi-bounce ripple convergence.** v1 keeps the existing single-pass indirect model.

## Acceptance criteria

- [ ] Building a map twice with no change: second build composites from cached layers and emits a `.prl` byte-identical to the first.
- [ ] Moving or retuning one point/spot light: only that light's lightmap and SH layers report cache miss; every other layer hits (verifiable in build progress logs).
- [ ] The cached composite is byte-identical to the same per-light pipeline run with `--no-cache`, for both sections, on every fixture in `content/dev/maps/`. (Determinism gate.)
- [ ] Editing geometry inside a light's influence region re-bakes that light's layer (its geometry-slice hash changed); a light whose influence-bounded geometry is unchanged hits cache.
- [ ] A map mixing point/spot and directional lights produces correct output with no double-counting: point/spot come only from per-light layers, directional only from the directional-only fallback bake.
- [ ] A map with only directional lights falls back entirely to the full-stage bake and produces correct output.
- [ ] A corrupt or missing layer entry is detected, discarded with a warning, and that layer re-bakes; the build succeeds.
- [ ] `--no-cache` bypasses per-light layer reads and writes; the build behaves as a cold per-light bake.
- [ ] Peak per-light intermediate memory and on-disk size are recorded (Task 1) for a heavily-lit open-arena fixture, and the chosen substrate is documented.

> Every "byte-identical" criterion above is within the per-light pipeline. The first per-light build's SH section will differ from pre-migration `.prl` files by float rounding — expected, per "Bit-compatibility … Out of scope".

## Tasks

### Task 1: Storage and cold-build profiling spike

Before wiring, quantify both costs. Per-light layers are `O(texels × overlapping-lights)` for the lightmap and `O(probes-in-influence × lights)` for SH. Build or pick a heavily-lit open-arena fixture (many overlapping point/spot lights) plus reuse campaign-test (SH-dominated), instrument a throwaway per-light bake, and record:

- **Storage:** peak intermediate memory and total on-disk layer bytes. Decide whether the existing flat-file `StageCache` (one file per entry, `sync_all` per put) holds at per-light entry counts, or a batched/packed store is needed. This plan assumes flat-file; a "batched store needed" outcome triggers re-planning before Phase 2 (no batched-store task exists here).
- **Cold-build inflation:** the measured per-light cold SH time vs. the current monolithic SH time, i.e. the realized average per-probe light-overlap depth. This is the number behind the accepted cold-for-warm trade — record it so "longer cold build" is a known multiplier, and flag if it exceeds a tolerance that would pull primary-ray-hit caching into v1 scope.
- **Warm-build validation:** confirm that re-baking a single light's layers (all others hitting cache) is the expected fraction of a full bake.

Document the decision and the three numbers in `research.md`. Output is a go/no-go, not production code. The three numbers are a named owner gate: the project owner reviews them and makes the go/no-go (flat-file vs batched store; whether the cold-build multiplier pulls primary-ray-hit caching into v1) — not an implementer-chosen threshold.

### Task 2: Per-light layer types and serialization

Define the per-light layer payloads and their deterministic (de)serialization for the cache. Lightmap layer: per-atlas-texel linear irradiance plus the **unnormalized** weighted direction and coverage — the values accumulated in `bake_face_chart` *before* the per-texel `weighted_dir.normalize()`. SH layer: the per-probe SH coefficient set (`[f32; 27]`) over the light's influence region, plus the probe indices it covers. Both are compiler-internal — never shipped — so the encoding only needs to round-trip exactly and hash deterministically (constraint: fixed, deterministic byte encoding; exact layout is the implementer's choice). Layer-format changes are gated by the per-light stage version constant (Task 6). On deserialize or hash-mismatch failure, the layer is a cache miss (warn, re-bake). Depth moments and probe validity are geometry-only (not per-light) and stay with the existing per-probe geometry pass. Precision is lossless: store full-precision `f32` irradiance, `f32` unnormalized weighted-direction, and coverage (never `f16`); the compositor must sum in the same light order as the monolithic `for light in static_lights` loop. Both are load-bearing for the lightmap's bit-exact reproduction.

### Task 3: Single-light bake entry points

Factor the lightmap and SH bakes so a single light's layer can be produced in isolation. SH already has the primitive: `sh_bake::bake_probe_indirect_rgb` (`pub(crate)`, param `&[&MapLight]`) — call it with a one-element slice. For the lightmap, hoist the per-light body of `bake_face_chart` so one light's irradiance + unnormalized weighted-direction layer can be baked across the shared atlas. **Atlas plumbing:** call `prepare_atlas` **once** with the full static-light set (its empty-light branch is a placeholder path, so it must not be called per single-light layer); thread the resulting `charts`/`placements`/`atlas_width`/`atlas_height` into every single-light bake. Each entry point's inputs: one `MapLight`, the shared atlas (charts/placements/dims), the raytracing context (`ShBakeCtx` for SH; the lightmap's equivalent ctx) that wraps the BVH + occluders, and the relevant config; output is a layer (Task 2).

### Task 4: Influence + invalidation

Compute each light's influence region and dependency set. Reuse `affinity_grid::light_aabb` — currently module-private `fn light_aabb`; mark it (and `AABB_PADDING_METERS` if read directly) `pub(crate)` so the layer code can call it. (A second identical `fn light_aabb` exists at `delta_sh_bake.rs:416`; promote `affinity_grid`'s as the canonical one and fold the delta copy into it, so the layer influence AABB cannot drift from the delta path's.) Point/spot → `falloff_range + AABB_PADDING_METERS` sphere AABB; directional → world AABB (the fallback signal). The layer's cache key folds the light's params (the relevant `MapLight` fields under a fixed `postcard` encoding — the same serialization discipline the whole-stage key uses), the geometry slice intersecting its influence AABB, the relevant config (`lightmap_density` + `area_sample_count` for lightmap, `probe_spacing` for SH), and the atlas/probe-grid layout descriptor (lightmap: atlas dims + per-chart placements; SH: probe-grid origin/cell-size/dims plus tile layout — folded into every layer key so an atlas repack or grid change invalidates all layers).

**Geometry slice — definition (resolved).** Hash *face content*, not BVH topology. The slice is the set of `BvhPrimitive`s (one per face — `bvh_build.rs:32-44`) whose face AABB overlaps the influence AABB, gathered by querying the existing BVH (accelerator only). For the key, take a view of those primitives sorted by the content-derived `BvhPrimitive::sort_key` field (`bvh_build.rs:41`, populated by `primitive_sort_key(material_bucket_id, cell_id, index_offset)` at `bvh_build.rs:96-103`) — *not* the post-`Bvh::build` in-place permutation — and hash each face's geometry (its `index_offset..index_offset+index_count` slice — read from the parallel `GeometryResult.face_index_ranges[face_idx]` (`FaceIndexRange`, `geometry.rs:19-23`), not a field on the face — into `GeometryResult.geometry.indices` / `.vertices`) the same way the whole-stage key already does. Concretely the slice hash is the existing whole-stage `GeometryResult` content hash (postcard + blake3, `main.rs:299-308`) restricted to the influence-overlapping faces in canonical `sort_key` order — a narrowing of a mechanism that already ships, not a new one. This keys invalidation on stable geometry content + deterministic face order, decoupled from BVH build determinism and tree topology (chosen over hashing BVH leaf indices, which would couple the key to tree structure, over-select at leaf granularity, and drift across builds). The pre-permutation canonical order is always recoverable by re-sorting any `&[BvhPrimitive]` on `sort_key`.

Invalidation is emergent: a geometry edit that changes a light's influence-bounded slice changes that layer's key (miss); a slice unchanged in the current build keeps its key (hit). Occlusion is local to the influence sphere — any occluder on a light→texel segment is nearer the light than the lit texel, hence inside `falloff_range` — so the AABB-bounded slice (point and spot alike, both using the falloff-sphere AABB) is a sound conservative dependency set.

### Task 5: Compositor

Assemble cached + freshly-baked layers into the final sections. Lightmap: element-wise sum of per-light irradiance across layers, element-wise sum of unnormalized weighted directions, then a single `normalize` per texel — reproducing `bake_face_chart`'s output. SH: per probe, sum the per-light `[f32; 27]` coefficient layers in the canonical light order (Task 4), then project once via `pack_octahedral_irradiance_tile` (`pub(crate)`, `sh_bake.rs:697`) and write the tiles into the `OctahedralShVolume` atlas alongside the geometry-only depth-moment / validity base (computed once by the geometry pass, not per light). Octahedral projection is linear in the coefficients, so summing coeffs then projecting equals per-light project-then-sum — exact — and any nonlinear encoding inside the pack runs once, on the summed result. **Directional lights** are baked by a directional-only invocation of the existing full-stage path (the full-stage bake must accept a filtered light set so point/spot are excluded and not double-counted — construct the whole-stage `ShBakeCtx` / `LightmapInputs` with only directional lights; the existing input hash then captures the filtered set, so the fallback's whole-stage cache key differs naturally from a full-light key) and summed in. Summation order is the canonical light order defined in Task 4, so the cached composite equals the uncached composite byte-for-byte. When the map has no point/spot lights, the per-light compositor is skipped and the section comes entirely from the directional-only full-stage path (placeholder atlas when there are no static lights).

### Task 6: Pipeline wiring + CLI

Wire Tasks 3–5 into `main.rs`, replacing the current whole-stage point/spot contribution of the lightmap and SH cache get/insert. Partition lights by `MapLight.light_type` (`LightType::{Point, Spot}` → per-light layers; `LightType::Directional` → fallback, so a directional light is never enumerated as a layer). Per static point/spot light: derive the layer key (Task 4), `get` from `StageCache`, on miss bake (Task 3) and `put`. Run the directional-only fallback bake (Task 5) for directional lights. Composite (Task 5) into the section the rest of the pipeline consumes. Per-light layers use **new** stage ids — `"lightmap_layer"` and `"sh_layer"`, distinct from the existing `"lightmap"` / `"sh_volume"` (main.rs:308,436) — each with its own version constant manually bumped when the layer algorithm or format changes — the existing whole-stage `STAGE_VERSION` constants stay as-is, still gating the directional fallback. Surface per-layer hit/miss in progress logs. Respect the existing `--no-cache` / `--cache-dir` flags.

### Task 7: Tests + determinism gate

Cover: round-trip skip (build twice → all layers hit); single-light edit (only that light's layers miss); geometry-edit invalidation (only influence-overlapping layers miss); mixed point/spot + directional (no double-count); directional-only fallback; corruption recovery; `--no-cache` bypass. The determinism gate: cached composite byte-identical to the `--no-cache` per-light composite across every `content/dev/maps/` fixture, for both sections. Note the cold `--no-cache` per-light SH bake is slow on SH-dominated fixtures (campaign-test) and slowest on the arena stress fixture — if either is pathologically slow for CI, the gate may time-bound or exclude it; record which, and keep at least one SH-heavy fixture in the gate so the property is actually exercised.

### Task 8: Documentation

Update `build_pipeline.md` §Build Cache: add the two new stage ids (`lightmap_layer`, `sh_layer`) and their independent version-bump discipline; note the per-light pipeline is the canonical bake (the monolithic bake is retained only for the directional fallback); and record the SH caveat — per-light SH is a new bake whose section differs from pre-migration `.prl` by float rounding. No `rendering_pipeline.md` change (runtime and PRL format are untouched).

## Sequencing

**Phase 1 (sequential):** Task 1 — storage spike gates the substrate before any wiring.
**Phase 2 (sequential):** Task 2 — layer types/serialization; Tasks 3–5 all depend on them.
**Phase 3 (concurrent):** Task 3, Task 4 — single-light bakes and influence/key derivation are independent.
**Phase 4 (sequential):** Task 5 — compositor consumes Task 3 layers and Task 4 ordering.
**Phase 5 (sequential):** Task 6 — pipeline wiring consumes Tasks 3–5.
**Phase 6 (sequential):** Task 7 — tests and the determinism gate validate the wired pipeline.
**Phase 7 (sequential):** Task 8 — docs, once the pipeline is wired and validated.

## Rough sketch

Grounded against current source (signatures confirmed):

- **Lightmap.** `bake_face_chart` (`lightmap_bake.rs:695`) accumulates `irr += contribution * v` (`:758`) and `weighted_dir += to_light * lum` (`:765`, where `lum` already folds in `v`) over `for light in static_lights`, then stores `direction[idx] = weighted_dir.normalize()` (`weighted_dir.normalize()` at `:774`, store at `:779`). The per-light layer captures `irr` and the **pre-normalize** `weighted_dir` per texel; the compositor sums then normalizes. The atlas comes from `prepare_atlas` (`lightmap_bake.rs:172`), called once with the full static set — note its empty-light branch (`:186`) skips UV writes, so it is deterministic from geometry + density only given a non-empty light set. `LightmapInputs`/`LightmapConfig` (`lightmap_bake.rs:119,130`) define the whole-stage inputs the per-light key narrows.
- **SH.** Base bake `bake_probe_rgb_with_moments` (defined `sh_bake.rs:811`) sums all static lights per probe and yields geometry-only depth moments. The per-light layer reuses `sh_bake::bake_probe_indirect_rgb(ctx, pos, &[light], probe_index) -> [f32; 27]` (`pub(crate)`, defined `sh_bake.rs:775`; the delta baker calls it with a 1-element slice at `delta_sh_bake.rs:354`). Indirect-only (`BOUNCE_ALBEDO`, `sh_bake.rs:48`); direct is the lightmap's job. `ShConfig` (`sh_bake.rs:121`) plus the `ShBakeCtx` raytracing context (`sh_bake.rs:66`) are the whole-stage `bake_sh_volume(&ShBakeCtx, &ShConfig)` (`sh_bake.rs:127`) inputs; `ShInputs` (`:109`) is the hash-only struct.
- **Influence.** `affinity_grid::light_aabb(light, world_aabb)` (`affinity_grid.rs:258`, currently private): point/spot → `falloff_range + AABB_PADDING_METERS` (`:43`); directional → world AABB.
- **Cache.** `cache::CacheKey::new(stage_id, stage_version, input_hash)` (`cache.rs:27`); `StageCache::{get,put}` (`cache.rs:62,114`). Section ids `Lightmap = 22`, `OctahedralShVolume = 34` (`level-format/src/lib.rs`).

The one correctness-critical subtlety: the lightmap dominant-direction channel is nonlinear (`normalize(Σ dir_i) ≠ recombine(normalize(dir_i))`). Storing the **unnormalized** weighted direction per layer and normalizing only the composite sum keeps the lightmap composite exact.

## Open questions

- **Substrate (resolved by Task 1).** Flat-file `StageCache` vs. a batched/packed store, decided by the arena-fixture profile. Entry counts are bounded by point/spot-light count, far below the per-face "millions" — flat-file is the expected answer, pending the number.
- **SH cold-build inflation (decided: accept for v1).** Campaign-test is SH-dominated, and re-casting shared primary rays per light raises cold cost by ~the average per-probe light-overlap depth (bounded by influence culling, not total light count). Accepted as a deliberate cold-for-warm trade; Task 1 measures the realized multiplier. Primary-ray-hit caching (cache the light-independent per-probe geometry hits, reuse across layers) is the named lever that collapses the inflation back toward 1× — deferred unless Task 1's number, or real cold/CI build times, exceed tolerance.
