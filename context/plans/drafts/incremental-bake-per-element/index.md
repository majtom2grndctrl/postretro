---
name: Incremental Bake Per Light
description: v1 — per-light contribution layers cached and composited at bake time, so editing one light re-bakes only its layer. Compiler-internal; runtime and PRL format untouched.
type: plan
---

# Incremental Bake Per Light

> Supersedes the per-element framing of the original stub. See `research.md` (§4d) for the
> grounded analysis that selected per-light over per-face / per-room grain. The blocking plan
> `build-stage-cache/` has shipped; this builds on its `StageCache`/`CacheKey` substrate.

## Goal

Cut `prl-build` iteration time for the lighting loop. Decompose the static lightmap (direct) and SH irradiance (indirect) bakes into **per-light contribution layers**, cache each layer keyed on its own inputs, and composite the cached layers at bake time into byte-identical `Lightmap` (section 22) and `OctahedralShVolume` (section 34) output. Editing one light re-bakes only that light's layer; the rest load from cache. Runtime and PRL format are untouched — this is purely a compiler caching strategy.

## Why per-light

Keyed on light *influence*, not map topology, so it holds up in large open arenas where one light floods most of the map (the case that breaks per-room/portal grain). Seam-free by construction: a light's contribution is exactly zero at `falloff_range`, so a layer fades to zero at its own support boundary — nothing to stitch. Exact under linearity: both output sections are pure additive sums of per-light contributions with no per-light clamp or tone-map. Reuses the animated-light path (`delta_sh_bake`, `affinity_grid`, `chunk_light_list_bake`), which already does single-light layer bakes for dynamic lights.

## Scope

### In scope

- Per-light contribution layers for the **static lightmap** (direct) and **SH irradiance volume** (indirect) bakes, for **point and spot** lights.
- Bake-time compositing of cached layers into byte-identical existing PRL sections.
- Per-layer cache entries on the existing `StageCache`, keyed by content hash of `(light params, influence-bounded geometry, density/spacing config, atlas/probe-grid layout)`.
- Conservative geometry-edit invalidation: a layer is invalid if its influence AABB overlaps changed geometry. Reuses `affinity_grid::light_aabb`.
- Always-correct fallback: any uncertainty (new light, directional light, layout change, corrupt/missing entry) falls back to a full per-light bake of the affected layer or, where decomposition does not apply, the existing full-stage bake.
- Determinism: layers composite in a fixed light order so output stays byte-identical to a cold full bake.
- A determinism gate test: composited output equals a cold full-stage bake, byte-for-byte, on every `content/dev/maps/` fixture.
- A storage-profiling spike (Task 1) gating the substrate decision before layer wiring lands.

### Out of scope

- **Directional / sun lights.** Whole-map influence yields no sparsity and invalidates on nearly every edit. They stay on the existing full-stage bake path. The output composite still sums them in.
- **Per-face lightmap atlas repartitioning** and **per-room / portal grouping** (`research.md` §4b). The atlas layout is unchanged: all layers share the one atlas the current packer produces.
- **Runtime or PRL format changes.** No new shipped section; the runtime never sees per-light layers.
- **SDF atlas stage** and the **animated weight-map stage** (already cached whole-stage). No change to either.
- **Primary-ray-hit caching for SH** (reusing per-probe geometry hits across light layers). A future cold-build optimization; v1 re-casts per light like the delta path.
- **Multi-bounce ripple convergence.** v1 keeps the existing single-pass indirect model; per-light layers carry whatever bounce depth the current base bake produces.

## Acceptance criteria

- [ ] Building a map twice with no change: second build composites from cached layers and emits a `.prl` byte-identical to the first.
- [ ] Moving or retuning one point/spot light: second build re-bakes only that light's lightmap and SH layers (verifiable in build progress logs); all other layers report cache hits; output differs only where that light reaches.
- [ ] Composited lightmap and SH sections are byte-identical to a cold full-stage bake (no caching) on every fixture in `content/dev/maps/`. (Determinism gate.)
- [ ] Editing geometry invalidates exactly the light layers whose influence AABB overlaps the changed geometry's AABB; non-overlapping layers report cache hits.
- [ ] Adding a directional/sun light, or any map with only directional lights, falls back to the full-stage bake and still produces correct output.
- [ ] A corrupt or missing layer entry is detected, discarded with a warning, and that layer re-bakes; the build succeeds.
- [ ] `--no-cache` bypasses per-light layers entirely; the build behaves as a cold full bake.
- [ ] Peak per-light intermediate memory and on-disk size are recorded (Task 1) for a heavily-lit open-arena fixture, and the chosen substrate is documented.

## Tasks

### Task 1: Storage-profiling spike

Before wiring, quantify the cost. Per-light layers are `O(texels × overlapping-lights)` for the lightmap and `O(probes-in-influence × lights)` for SH. Build or pick a heavily-lit open-arena fixture (many overlapping point/spot lights), instrument a throwaway per-light bake, and record peak intermediate memory and total on-disk layer bytes. Decide: does the existing flat-file `StageCache` (one file per entry, `sync_all` per put) hold at per-light entry counts, or is a batched/packed store needed? Document the decision in `research.md`. Output is a go/no-go on the substrate, not production code.

### Task 2: Per-light layer types and serialization

Define the per-light layer payloads and their deterministic (de)serialization for the cache. Lightmap layer: per-atlas-texel linear irradiance plus the **unnormalized** weighted direction and coverage — the values accumulated in `bake_face_chart` *before* the per-texel `weighted_dir.normalize()`. SH layer: the per-probe SH coefficient set (the `[f32; 27]` the base bake accumulates) over the light's influence region, plus the probe indices it covers. Both are compiler-internal — never shipped — so the format only needs to round-trip exactly and hash deterministically; reuse the project's existing deterministic serialization discipline. Depth moments and probe validity are geometry-only (not per-light) and stay with the existing per-probe geometry pass.

### Task 3: Single-light bake entry points

Factor the lightmap and SH bakes so a single light's layer can be produced in isolation. SH already has the primitive: `delta_sh_bake::bake_probe_indirect_rgb` takes a one-light slice. For the lightmap, hoist the per-light body of `bake_face_chart` so one light's irradiance + unnormalized weighted-direction layer can be baked across the shared atlas (charts/placements from `prepare_atlas`, which is deterministic from geometry + density). Each entry point takes one `MapLight` plus the shared bake context and returns a layer (Task 2).

### Task 4: Influence + invalidation

Compute each light's influence region and dependency set. Reuse `affinity_grid::light_aabb` for the AABB (point/spot → `falloff_range + padding` sphere; directional → world AABB → routed to fallback). The layer's cache key folds the light's params, the geometry intersecting its influence AABB, the relevant config (density/`area_sample_count` for lightmap, `probe_spacing` for SH), and the atlas/probe-grid layout descriptor. Geometry-edit invalidation falls out of the key: changed geometry inside the influence AABB changes the hashed geometry slice, missing it leaves the key stable. Occlusion is local to the influence sphere (occluders lie between light and lit texels, both within `falloff_range`), so AABB-bounded geometry is a sound conservative dependency set.

### Task 5: Compositor

Assemble cached + freshly-baked layers into the final sections. Lightmap: element-wise sum of per-light irradiance across layers, element-wise sum of unnormalized weighted directions, then a single `normalize` per texel — reproducing `bake_face_chart`'s output. SH: element-wise sum of per-light coefficient layers onto the geometry-only base (depth moments / validity), then pack the existing octahedral atlas. Directional/sun lights composite in via the existing full-stage path. Summation order is fixed (stable light ordering) to preserve byte-identical output.

### Task 6: Pipeline wiring + CLI

Wire Tasks 3–5 into `main.rs` in place of the current whole-stage lightmap and SH cache get/insert. Per static point/spot light: derive the layer key (Task 4), `get` from `StageCache`, on miss bake (Task 3) and `put`. Composite (Task 5) into the section the rest of the pipeline already consumes. Surface per-layer hit/miss in progress logs. Respect the existing `--no-cache` / `--cache-dir` flags. Bump the relevant `STAGE_VERSION` constants since the cache entry shape changes.

### Task 7: Tests + determinism gate

Cover: round-trip skip (build twice → all layers hit); single-light edit (only that light's layers miss); geometry-edit invalidation (only influence-overlapping layers miss); directional-only fallback; corruption recovery; `--no-cache` bypass. The determinism gate: composited output byte-identical to a cold full-stage bake across every `content/dev/maps/` fixture, for both sections.

## Sequencing

**Phase 1 (sequential):** Task 1 — storage spike gates the substrate before any wiring.
**Phase 2 (sequential):** Task 2 — layer types/serialization; Tasks 3–5 all depend on them.
**Phase 3 (concurrent):** Task 3, Task 4 — single-light bakes and influence/key derivation are independent.
**Phase 4 (sequential):** Task 5 — compositor consumes Task 3 layers and Task 4 ordering.
**Phase 5 (sequential):** Task 6 — pipeline wiring consumes Tasks 3–5.
**Phase 6 (sequential):** Task 7 — tests and the determinism gate validate the wired pipeline.

## Rough sketch

Grounded against current source (signatures confirmed):

- **Lightmap.** `bake_face_chart` (`lightmap_bake.rs:694`) accumulates `irr += contribution * v` and `weighted_dir += to_light * lum` over `for light in static_lights`, then stores `direction[idx] = weighted_dir.normalize()`. The per-light layer captures `irr` and the **pre-normalize** `weighted_dir` per texel; the compositor sums then normalizes. The atlas (`charts`/`placements`/`atlas_width`/`atlas_height`) comes from `prepare_atlas` (`lightmap_bake.rs:172`), a deterministic function of geometry + density that all layers share. `LightmapInputs`/`LightmapConfig` (`lightmap_bake.rs:119,130`) define the current whole-stage inputs the per-light key narrows.
- **SH.** Base bake `bake_probe_rgb_with_moments` (`sh_bake.rs:197`) sums all static lights per probe and also yields geometry-only depth moments. The per-light layer reuses the delta primitive `bake_probe_indirect_rgb(&ctx, pos, &[light], probe_index)` (`delta_sh_bake.rs:354`). `ShInputs`/`ShConfig` (`sh_bake.rs:109,119`) define the whole-stage inputs the per-light key narrows. Note: SH re-casts the 256 shared primary rays per light on cold build (matching the delta path); primary-hit caching is a deferred optimization.
- **Influence.** `affinity_grid::light_aabb(light, world_aabb)` (`affinity_grid.rs:258`): point/spot → `falloff_range + AABB_PADDING_METERS`; directional → world AABB (the fallback signal).
- **Cache.** `cache::CacheKey::new(stage_id, stage_version, input_hash)` (`cache.rs:27`) and `StageCache::{get,put}`. Per-light stage ids namespace layer entries from the existing whole-stage entries.

The one correctness-critical subtlety: the lightmap dominant-direction channel is nonlinear (`normalize(Σ dir_i) ≠ recombine(normalize(dir_i))`). Storing the **unnormalized** weighted direction per layer and normalizing only the composite sum is what keeps "composite == full bake" exact.

## Open questions

- **Substrate (resolved by Task 1).** Flat-file `StageCache` vs. a batched/packed store, decided by the arena-fixture profile. Entry counts are bounded by `point/spot-light count`, far below the per-face "millions" that motivated a redesign — flat-file is the expected answer, pending the number.
- **SH cold-build inflation.** Re-casting shared primary rays per light raises cold-build cost with light count. Acceptable for v1 (the delta path already pays it); primary-hit caching is the future optimization if cold builds regress unacceptably.
- **Layer-set membership in the key.** A texel/probe's composited value must sum the same light set in the same order across builds. The key must pin not just each light's inputs but the ordered membership of contributing lights, so a hit cannot silently composite a stale set. Confirm the fixed light ordering source during Task 4.
