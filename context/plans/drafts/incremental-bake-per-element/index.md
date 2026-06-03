---
name: Incremental Bake — Per-Light Lightmap + Per-Group SH
description: v1 — per-light lightmap layers and per-probe-group SH cache entries, composited/assembled at bake time so editing one light refreshes only the affected entries. Compiler-internal; runtime and PRL format untouched; output bit-identical to the monolithic bake.
type: plan
---

# Incremental Bake — Per-Light Lightmap + Per-Group SH

> Supersedes the earlier per-light-for-both framing (and the original per-element stub). See
> `research.md` (§4d, and the §6 reversal below) for the grounded analysis. The blocking plan
> `build-stage-cache/` has shipped; this builds on its `StageCache`/`CacheKey` substrate.
> The folder name `incremental-bake-per-element/` is kept for link stability.

## Goal

Cut `prl-build` iteration time for the lighting loop. Cache the two static lighting bakes at a grain that lets a light edit refresh only the affected cache entries instead of rebaking the whole map, then composite/assemble the cached entries into the `Lightmap` (section 22) and `OctahedralShVolume` (section 34) sections the runtime already consumes. Two grains, one per channel — chosen to fit each bake's nature:

- **Lightmap (direct):** per-light contribution layers. Direct illumination is keyed on texel-to-light distance with a hard cutoff, so per-light is exact, cheap, and edit-one-light-instant.
- **SH (indirect):** per-probe-group cache entries. Each group is baked with the existing whole-probe algorithm over its probe subset, so it stays bit-identical to today and pays no cold-build penalty.

Runtime and PRL format are untouched — purely a compiler caching strategy — and the assembled output is **byte-identical to the current monolithic bake** for both sections.

## Why this shape

**Per channel, by its nature.** The lightmap's per-light contribution reaches *exactly* zero at and beyond `falloff_range` (`lightmap_bake.rs:887-907` — a clamped-to-zero linear ramp, or a hard `> range` cutoff for the inverse models), so a layer has bounded support and per-light layers sum back to the monolithic result bit-for-bit. SH is indirect, expensive, and bounce-coupled — decomposing it per light would force either re-casting each probe's primary rays per light (a cold-build regression) or culling probes by a bounding box (which drops genuine far-field bounces, because SH falloff is keyed on the *bounce hit point*, not the probe — `sh_bake.rs:579-597`). Chunking SH spatially instead keeps the "cast once, sum all in-range lights, project" structure intact per probe, so each group is the monolithic bake restricted to a probe subset.

**Bit-identical, not merely equivalent.** Probes are independent in the single-pass indirect bake, out-of-range lights contribute exactly `0.0`, and adding `0.0` does not perturb a float sum. So baking a probe inside a group (with the group's light subset in global order) yields byte-for-byte what the whole-map bake yields. The lightmap composite (`normalize(Σ unnormalized_dir)`, `Σ irr`) likewise reproduces `bake_face_chart` exactly. No reassociation, no rounding caveat, no "new bake" — the cache is a pure memoization of today's output.

**No directional fallback, no compositor-for-SH, no hit cache.** Directional lights are just non-sparse lightmap layers and ordinary members of every SH group's light set. SH groups are disjoint probe partitions written straight into the volume (no summation). Nothing here needs the per-light-SH machinery the prior draft carried.

The cost this design accepts is named under Scope and §6: SH invalidation locality is bounded by each light's *spatial reach*, so a light flooding many groups (and any directional light) invalidates broadly. The per-light **direct** lightmap still updates that light instantly, so the dominant visual channel always responds immediately while indirect catches up at group grain.

## Key facts (read before the tasks)

Grounded against source, signatures confirmed:

- **Lightmap.** `bake_face_chart` (`lightmap_bake.rs:733`) loops `for light in static_lights` (`:772`), accumulating `irr += contribution * v` (`:796`) and `weighted_dir += to_light * lum` (`:803`), then storing `direction[idx]` from `weighted_dir.normalize()` (normalize `:812`, store `:817`) and marking `coverage[idx] = true` (`:818`). Atlas from `prepare_atlas` (`:194`; empty-light branch `:208` still plans charts and shelf-packs but skips vertex splitting + UV assignment — call once with the full static set). `LightmapInputs`/`LightmapConfig` (`:133`/`:144`; `lightmap_density :145`, `area_sample_count :149`).
- **SH.** Whole-volume entry `bake_sh_volume(&ShBakeCtx, &ShConfig)` (`sh_bake.rs:127`) builds one flat `static_lights` slice (`:152-157`) and feeds it to every probe (`:197`) via `bake_probe_rgb_with_moments` (`:811`) → `sample_radiance_rgb` (`:877`), which loops all lights per bounce hit (`:890`). Per-light contribution uses `falloff` (`:579-597`), a **hard cutoff** keyed on bounce-hit-to-light distance: Linear is `0.0` at `range`; inverse models `return 0.0` for `distance > range`. Octahedral pack `pack_octahedral_irradiance_tile` (`pub(crate)`, `:697`); `evaluate_sh_rgb` (`:645-655`) is a linear basis·coeff dot, only nonlinearity is the final clamp + f16 encode. `ShBakeCtx :66`, `ShConfig :121` (`probe_spacing :122`), `ShInputs :109` (hash-only). Probe rays run to a far sentinel (`:197`) — that reach is the dilation distance for group dependency bounds.
- **Influence.** `affinity_grid::light_aabb` (`affinity_grid.rs:258`, private; promote to `pub(crate)` for reuse): point/spot → `falloff_range + AABB_PADDING_METERS` (`:43`) AABB; directional → world AABB. A second identical copy exists at `delta_sh_bake.rs:416` — leave it (it differs only by f32-vs-f64 add ordering and feeds the shipped delta path; folding would shift its bytes).
- **Lights.** `MapLight.light_type: LightType` (`map_data.rs:221`); `LightType` is `Point`/`Spot`/`Directional` (`:134`); `falloff_range` (`:234`).
- **Geometry / BVH.** `BvhPrimitive` is one-per-face (`bvh_build.rs:32-44`) with content-derived `sort_key` field (`:41`, populated by `primitive_sort_key(material_bucket_id, cell_id, index_offset)` `:96-103`). `Bvh::build` permutes the slice in place (`:156`). A face's triangles are `index_offset..index_offset+index_count` (from the parallel `GeometryResult.face_index_ranges[face_idx]`, `FaceIndexRange` `geometry.rs:20-23`) into `GeometryResult.geometry.indices`/`.vertices`.
- **Cache.** `cache::CacheKey::new(stage_id, stage_version, input_hash)` (`cache.rs:27`); `StageCache::{get,put}` (`:62`/`:114`); `get` validates length+hash and returns a miss on failure. Existing keys `"lightmap"`/`"sh_volume"` (`main.rs:326`/`:455`) hash `postcard::to_allocvec(inputs)` + blake3 (`:317-326`); default dir `<workspace>/.build-caches/prl-cache/` (`:200`). Section ids `Lightmap = 22`, `OctahedralShVolume = 34` (`level-format/src/lib.rs:114,191`).

## Scope

### In scope

- **Lightmap:** per-light contribution layers for all light types (point/spot are sparse; directional is one non-sparse full-atlas layer), cached per light and composited.
- **SH:** per-probe-group cache entries — the probe grid partitioned into spatial groups, each baked over its probe subset with the existing per-probe algorithm and a conservative reaching-light set, then assembled into the volume.
- New stage ids on the existing `StageCache` (`"lightmap_layer"`, `"sh_group"`), each with its own version constant, in the same cache dir (`.build-caches/prl-cache/`).
- Corruption handling: a cache entry failing length/hash validation or deserialization is a miss (warn, re-bake), mirroring `StageCache::get`.
- Determinism: lightmap composite and SH assembly both byte-identical to the current monolithic bakes.
- A determinism gate test and a Task 1 spike (storage + realized single-light invalidation locality + group-size choice) gating before wiring lands.

### Out of scope

- **Per-face lightmap atlas repartitioning.** Atlas layout unchanged: all layers share the one atlas the current packer produces.
- **Per-light SH decomposition / primary-ray-hit caching.** Considered and rejected for v1 (see §6); per-group keeps the bake structure intact instead.
- **Incremental geometry invalidation finer than the group/layer dependency slice.** A geometry edit re-bakes the layers/groups whose dependency slice changed; tightening that is future work.
- **Runtime or PRL format changes.** No new shipped section; the runtime never sees layers or groups.
- **SDF atlas stage** and the **animated weight-map stage** (already cached whole-stage). Untouched.
- **Multi-bounce ripple convergence.** v1 keeps the existing single-pass indirect model.

## Acceptance criteria

- [ ] Building a map twice with no change: the second build serves both sections from cache and emits a `.prl` byte-identical to the first.
- [ ] The lightmap composite is byte-identical to the monolithic `bake_face_chart` output, and the SH per-group assembly is byte-identical to the monolithic `bake_sh_volume` output, on every fixture in `content/dev/maps/`. (Determinism gate — bit-compatibility with the pre-cache `.prl` is preserved.)
- [ ] Editing one point/spot light: its lightmap layer reports a miss and re-bakes; the SH groups within that light's reach report misses and re-bake; every other lightmap layer and SH group hits (verifiable in build progress logs).
- [ ] Editing a directional light: its lightmap layer re-bakes and all SH groups re-bake (whole-map reach) — correct, with no other lightmap layer affected.
- [ ] Editing geometry: only the lightmap layers whose influence-bounded face slice changed, and only the SH groups whose geometry-in-reach slice changed, re-bake; the rest hit.
- [ ] A corrupt or missing cache entry (layer or group) is detected, discarded with a warning, and re-baked; the build succeeds.
- [ ] `--no-cache` bypasses all layer/group reads and writes; the build behaves as a cold bake and matches the monolithic output.
- [ ] Task 1 records, for a heavily-lit fixture and campaign-test: lightmap-layer + SH-group on-disk and peak-memory sizes, the realized fraction of SH groups invalidated by a single point/spot light edit, and the chosen group size — and the owner signs off the go/no-go.

## Tasks

### Task 1: Storage and locality profiling spike

Before wiring, measure the two things that decide the design's payoff and its substrate, on a heavily-lit fixture plus campaign-test:

- **Storage:** lightmap-layer count/bytes and SH per-group count/bytes (on-disk and peak intermediate memory). The SH group cache stores output-sized tiles+moments, so it should be modest; confirm. Decide whether the flat-file `StageCache` holds at these entry counts or a packed store is warranted (this plan assumes flat-file; "packed store needed" triggers re-planning before wiring).
- **SH invalidation locality (the load-bearing number):** for a representative point/spot light, what fraction of SH groups fall in its conservative reach and would re-bake on an edit? This is bounded by `falloff_range + SH-ray-reach` and by map occlusion; if it is large (open maps / long rays), per-group SH degrades toward full rebuild for that light. Record it.
- **Group size:** pick the probe-group dimensions that balance cache-entry count against invalidation granularity; record the choice and its sensitivity.

Document the numbers in `research.md`. Output is a go/no-go owner gate (the project owner reviews the numbers and decides), not production code.

### Task 2: Lightmap layer types and serialization

Define the per-light lightmap layer payload and its deterministic (de)serialization. Per-atlas-texel: linear irradiance, the **unnormalized** weighted direction, and coverage — the values accumulated in `bake_face_chart` *before* `weighted_dir.normalize()`. Precision is lossless: full-precision `f32` for irradiance, weighted-direction, and coverage (never `f16`). Compiler-internal, never shipped — the encoding only needs to round-trip exactly and hash deterministically (fixed byte layout; exact form is the implementer's choice). Layer-format changes are gated by the `"lightmap_layer"` version constant (Task 6). On deserialize/hash-mismatch failure, the layer is a miss (warn, re-bake).

### Task 3: Lightmap single-light bake entry point

Hoist the per-light body of `bake_face_chart` so one light's irradiance + unnormalized-weighted-direction layer can be baked across the shared atlas. Call `prepare_atlas` **once** with the full static set (its empty-light branch is a placeholder path; do not call it per single-light layer) and thread the resulting `charts`/`placements`/`atlas_width`/`atlas_height` into every single-light bake. Entry-point inputs: one `MapLight`, the shared atlas (charts/placements/dims), the BVH + primitives + `GeometryResult` (the `&Bvh<f32, 3>`, `&[BvhPrimitive]`, `&GeometryResult` `bake_face_chart` already takes), and `LightmapConfig`; output is a layer (Task 2). Directional lights use the same entry point, producing a full-atlas (non-sparse) layer.

### Task 4: Lightmap influence + invalidation

Per-light dependency set and cache key. Reuse `affinity_grid::light_aabb` — promote it (and `AABB_PADDING_METERS` if read directly) to `pub(crate)`; leave the `delta_sh_bake.rs:416` copy untouched. Point/spot → `falloff_range + AABB_PADDING_METERS` AABB; directional → world AABB. The layer's key folds: the light's params (relevant `MapLight` fields under a fixed `postcard` encoding, the discipline the whole-stage key uses), the influence-bounded **geometry slice**, `lightmap_density` + `area_sample_count`, and the atlas layout descriptor (atlas dims + per-chart placements — so an atlas repack invalidates all layers). The promoted `affinity_grid::light_aabb` (the f32 copy) is authoritative — its exact output feeds the key, independent of the `delta_sh_bake.rs` copy (no cross-matching). Charts/placements don't derive `Serialize`; fold them via the same deterministically-derived proxy-bytes fingerprint the animated-weight-map stage already uses (`research.md` §4c), not a direct hash. The atlas overflow/density-halving retry repacks the whole atlas, changing this descriptor and so invalidating every lightmap layer by design (SH is unaffected); Task 1 should note whether the profiled fixtures hit that retry path.

**Geometry slice (shared with Task 6).** Hash *face content*, not BVH topology. The slice is the `BvhPrimitive`s (one per face) whose face AABB overlaps the influence AABB, gathered via the BVH (accelerator only), taken in canonical `sort_key`-field order (`bvh_build.rs:41`, populated by `primitive_sort_key` `:96-103`) — not the post-`Bvh::build` permutation — and hashed by each face's geometry (`index_offset..index_offset+index_count` from `GeometryResult.face_index_ranges[face_idx]` into `GeometryResult.geometry.indices`/`.vertices`). Concretely it is the existing whole-stage `GeometryResult` content hash (postcard + blake3) restricted to the influence-overlapping faces — a narrowing of a shipping mechanism. Decoupled from BVH build determinism; chosen over leaf-index hashing (which would couple to tree topology and drift across builds). Invalidation is emergent: a slice that changed → new key → re-bake; unchanged → hit. Occlusion is local to the influence sphere (any occluder on a light→texel segment is nearer than the texel, hence inside `falloff_range`), so the falloff-AABB slice is a sound conservative dependency set.

### Task 5: Lightmap compositor

Element-wise sum per-light irradiance across layers, element-wise sum the unnormalized weighted directions, then a single `normalize` per texel — reproducing `bake_face_chart`'s output bit-for-bit (same addition order = global `static_lights` order). Per-texel `coverage` is the logical OR of the layers' coverage flags. Out-of-influence texels in a sparse layer contribute exactly `0.0`; the compositor sums each layer into a zero-initialized full atlas, so the on-disk layer encoding (dense-with-zeros or explicit texel-index list) stays the implementer's choice. Write the `Lightmap` section. When a map has no static lights, the existing placeholder-atlas path applies.

### Task 6: SH per-group bake + cache

Partition the probe grid into spatial groups (size from Task 1). For each group:

- **Reaching-light set (conservative):** lights whose contribution can reach any group probe — i.e. whose `falloff_range` region, dilated by the SH ray reach, overlaps the group AABB. Directional lights reach every group. Take the set in global `static_lights` order so the per-hit sum order matches the monolithic bake (out-of-set lights contribute exactly `0.0`, so including only the reaching set is bit-identical).
- **Geometry-in-reach slice:** the face-content slice (same mechanism as Task 4) for faces within the group AABB dilated by the SH ray reach — the geometry the group's rays can hit.
- **Key:** `"sh_group"` + version, hashing the conservative reaching-light params (fixed postcard encoding), the geometry-in-reach slice, `probe_spacing`, and the probe-grid layout descriptor (origin/cell-size/dims + group bounds).
- **Bake:** run the existing per-probe algorithm (`bake_probe_rgb_with_moments` → `pack_octahedral_irradiance_tile`) over the group's probe indices with the reaching-light set; store the group's octahedral tiles + depth moments + validity. Payload format (mirroring Task 2): per probe, the post-`pack_octahedral_irradiance_tile` f16 octahedral tile, the f16 depth moments (`E[d]`, `E[d²]`), and the validity byte, in fixed byte layout with lossless round-trip; assembly is a byte-copy into the section's tile/record offsets (no re-pack), which is what makes byte-identity hold. Corruption handling mirrors Task 2: a length/hash failure (via `StageCache`) or a deserialize failure on the group's own codec is a miss (warn, re-bake). `get`/`put` against `StageCache`.

Assembly is placement, not compositing: each group writes its probes' tiles into their offsets in the `OctahedralShVolume` section. The result equals the monolithic `bake_sh_volume` output byte-for-byte.

### Task 7: Pipeline wiring + CLI

Wire Tasks 3–6 into `main.rs`, replacing the current whole-stage `"lightmap"`/`"sh_volume"` get/insert. The animated-weight-map and SDF-atlas stages stay whole-stage cached and are not rewired; atlas layout is unchanged, so their keys are unaffected. Lightmap: per static light (all `LightType`s), derive the layer key (Task 4), `get`; on miss bake (Task 3) and `put`; composite (Task 5). SH: per group, derive the key (Task 6), `get`; on miss bake and `put`; assemble. New stage ids `"lightmap_layer"`/`"sh_group"`, each with its own version constant, manually bumped when that algorithm or format changes; the existing whole-stage `STAGE_VERSION` constants are retired from the hot path (kept only if other call sites use them). Surface per-entry hit/miss in progress logs. Respect `--no-cache` / `--cache-dir`.

### Task 8: Tests + determinism gate

Cover: round-trip skip (build twice → all entries hit); single point/spot light edit (its lightmap layer + in-reach SH groups miss, rest hit); directional light edit (its lightmap layer + all SH groups miss); geometry edit (only dependency-overlapping layers/groups miss); corruption recovery; `--no-cache` bypass; `--cache-dir` redirect (entries read/written under the override). **Determinism gate:** the cached lightmap composite is byte-identical to monolithic `bake_face_chart`, and the cached SH assembly byte-identical to monolithic `bake_sh_volume`, across every `content/dev/maps/` fixture. (Both gate against the legacy whole-map bakes — bit-compatibility is preserved, so no self-referential `--no-cache` comparison is needed.)

### Task 9: Documentation

Update `build_pipeline.md` §Build Cache: add the `"lightmap_layer"` and `"sh_group"` stage ids and their independent version-bump discipline; note the per-light-lightmap / per-group-SH grain and that assembled output is bit-identical to the monolithic bake. No `rendering_pipeline.md` change (runtime and PRL format untouched). Record the §4d → §6 grain reversal in `research.md`.

## Sequencing

**Phase 1 (sequential):** Task 1 — spike gates substrate, group size, and the locality go/no-go.
**Phase 2 (concurrent):** the lightmap chain (Task 2 → Tasks 3, 4 → Task 5) and the SH track (Task 6) are independent subsystems and proceed in parallel; both reuse the Task 4 geometry-slice definition, which must land before Task 6 can define its geometry-in-reach slice (the rest of the lightmap chain still runs parallel to the SH track).
**Phase 3 (sequential):** Task 7 — wiring consumes both tracks.
**Phase 4 (sequential):** Task 8 — tests and the determinism gate validate the wired pipeline.
**Phase 5 (sequential):** Task 9 — docs, once validated.

## Open questions

- **Substrate (resolved by Task 1).** Flat-file `StageCache` vs. a packed store, decided by lightmap-layer and SH-group entry counts/bytes. Flat-file is the expected answer (group count is modest), pending the numbers.
- **SH invalidation locality (resolved by Task 1).** How local is a point/spot light's group-reach in practice — bounded by `falloff_range + SH-ray-reach` and occlusion. If open maps / long rays make it broad, reconsider group size or accept that flood/long-reach lights re-bake many groups (the per-light direct lightmap still updates instantly). This is the design's one accepted weakness; Task 1 quantifies it.
- **Group size.** The file-count vs. invalidation-granularity dial; Task 1 picks it. A future refinement could size groups adaptively to light density.
