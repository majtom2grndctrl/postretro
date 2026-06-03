---
name: Incremental Bake — Per-Light Lightmap + Per-Group SH
description: v1 — per-light lightmap layers and per-probe-group SH cache entries, composited/assembled at bake time so editing one light refreshes only the affected entries. Compiler-internal; runtime and PRL format untouched. Lightmap is exact (warm == cold); SH is a bounded-reach warm approximation for fast iteration, with the cold `--no-cache` build as the exact ship source of truth.
type: plan
---

# Incremental Bake — Per-Light Lightmap + Per-Group SH

> Supersedes the earlier per-light-for-both framing (and the original per-element stub). See
> `research.md` (§4d, and the §6 reversal below) for the grounded analysis. The blocking plan
> `build-stage-cache/` has shipped; this builds on its `StageCache`/`CacheKey` substrate.
> The folder name `incremental-bake-per-element/` is kept for link stability.

## Goal

Cut `prl-build` iteration time for the lighting loop. Cache the two static lighting bakes at a grain that lets a light edit refresh only the affected cache entries instead of rebaking the whole map, then composite/assemble the cached entries into the `Lightmap` (section 22) and `OctahedralShVolume` (section 34) sections the runtime already consumes. Two grains, one per channel — chosen to fit each bake's nature:

- **Lightmap (direct):** per-light contribution layers. Direct illumination reaches exactly zero at `falloff_range`, so per-light is exact, cheap, and edit-one-light-instant — warm output equals the cold bake bit-for-bit.
- **SH (indirect):** per-probe-group cache entries with a **bounded reach**. Each group is baked over its probe subset with a finite ray/light-reach cutoff (Task 1), so a light edit invalidates only nearby groups. This makes warm SH a deliberate approximation — contributions past the cutoff are dropped — so warm SH is a touch dim in far-bounce regions, never miscolored locally. The cold `--no-cache` build runs the existing exact whole-volume bake and is the ship source of truth.

Runtime and PRL format are untouched — purely a compiler caching strategy. **Warm builds are an iteration aid, not a ship artifact.** The lightmap is exact (warm output byte-identical to the monolithic bake). SH is a bounded-reach approximation in warm builds; the shippable artifact is always a cold `--no-cache` build, whose SH equals the current monolithic `bake_sh_volume` exactly.

## Why this shape

**Per channel, by its nature.** The lightmap's per-light contribution reaches *exactly* zero at and beyond `falloff_range` (`lightmap_bake.rs:887-907` — a clamped-to-zero linear ramp, or a hard `> range` cutoff for the inverse models), so a layer has bounded support and per-light layers sum back to the monolithic result bit-for-bit. SH is indirect, expensive, and bounce-coupled — decomposing it per light would force either re-casting each probe's primary rays per light (a cold-build regression) or culling probes by a bounding box (which drops genuine far-field bounces, because SH falloff is keyed on the *bounce hit point*, not the probe — `sh_bake.rs:579-597`). Chunking SH spatially instead keeps the "cast once, sum in-range lights, project" structure intact per probe; bounding each group's reach is what makes a light edit local.

**Lightmap: bit-identical. SH: warm-approximate, cold-exact.** For the lightmap, per-light layers are independent and out-of-range lights contribute exactly `0.0`, so summing the layers (in global `static_lights` order) and normalizing reproduces `bake_face_chart` byte-for-byte — a pure memoization. SH is different: probe primary rays trace to `f32::INFINITY` (`sh_bake.rs:887`), so a bounce hit — and the light that lit it — can sit anywhere a probe can see. An *exact* per-group reach bound is therefore the whole map (every light reaches every group), which would localize nothing. So per-group SH deliberately bounds the **light set** to a finite reach cutoff (Task 1): rays still trace the full geometry, but lights past the cutoff are dropped from each probe's sum, so the warm bake is **not** byte-identical to the monolithic bake. Because dropping a light only removes a nonnegative term — no geometry is removed, so there is no sky-leak — warm SH is a strict, benign underestimate (dimmer-or-equal, never wrong-colored locally), and the cold `--no-cache` build restores the exact result for shipping.

**No directional fallback, no compositor-for-SH, no hit cache.** Directional lights are just non-sparse lightmap layers and ordinary members of every SH group's light set. SH groups are disjoint probe partitions written straight into the volume (no summation). Nothing here needs the per-light-SH machinery the prior draft carried.

The cost this design accepts: warm SH diverges from the exact bake by the dropped past-cutoff far bounces, and the shippable build must be cold. The per-light **direct** lightmap stays exact and updates instantly, so the dominant visual channel is always faithful; indirect catches up approximately during iteration and exactly at ship.

## Key facts (read before the tasks)

Grounded against source, signatures confirmed:

- **Lightmap.** `bake_face_chart` (`lightmap_bake.rs:733`) loops `for light in static_lights` (`:772`), accumulating `irr += contribution * v` (`:796`) and `weighted_dir += to_light * lum` (`:803`), then storing `direction[idx]` from `weighted_dir.normalize()` (normalize `:812`, store `:817`) and marking `coverage[idx] = true` (`:818`). Atlas from `prepare_atlas` (`:194`; empty-light branch `:208` still plans charts and shelf-packs but skips vertex splitting + UV assignment — call once with the full static set). `LightmapInputs`/`LightmapConfig` (`:133`/`:144`; `lightmap_density :145`, `area_sample_count :149`).
- **SH.** Whole-volume entry `bake_sh_volume(&ShBakeCtx, &ShConfig)` (`sh_bake.rs:127`) builds one flat `static_lights` slice (`:152-157`) and feeds it to every probe (`:197`) via `bake_probe_rgb_with_moments` (`:811`) → `sample_radiance_rgb` (`:877`), which loops all lights per bounce hit (`:890`). Per-light contribution uses `falloff` (`:579-597`), a **hard cutoff** keyed on bounce-hit-to-light distance: Linear is `0.0` at `range`; inverse models `return 0.0` for `distance > range`. Octahedral pack `pack_octahedral_irradiance_tile` (`pub(crate)`, `:697`); `evaluate_sh_rgb` (`:645-655`) is a linear basis·coeff dot, only nonlinearity is the final clamp + f16 encode. `ShBakeCtx :66`, `ShConfig :121` (`probe_spacing :122`), `ShInputs :109` (hash-only). Probe primary rays trace to `f32::INFINITY` and return the `far_sentinel`/`SKY_COLOR` only on a miss (`closest_hit(..., f32::INFINITY)`, `sh_bake.rs:887`), so a bounce hit can be anywhere a probe sees — the *exact* reach is unbounded (whole-map). The per-group dilation distance is therefore a **chosen finite cutoff** (Task 1), not a source constant.
- **Influence.** `affinity_grid::light_aabb` (`affinity_grid.rs:258`, private; promote to `pub(crate)` for reuse): point/spot → `falloff_range + AABB_PADDING_METERS` (`:43`) AABB; directional → world AABB. A second identical copy exists at `delta_sh_bake.rs:416` — leave it (it differs only by f32-vs-f64 add ordering and feeds the shipped delta path; folding would shift its bytes).
- **Lights.** `MapLight.light_type: LightType` (`map_data.rs:221`); `LightType` is `Point`/`Spot`/`Directional` (`:134`); `falloff_range` (`:234`).
- **Geometry / BVH.** `BvhPrimitive` is one-per-face (`bvh_build.rs:32-44`) with content-derived `sort_key` field (`:41`, populated by `primitive_sort_key(material_bucket_id, cell_id, index_offset)` `:96-103`). `Bvh::build` permutes the slice in place (`:156`). A face's triangles are `index_offset..index_offset+index_count` (from the parallel `GeometryResult.face_index_ranges[face_idx]`, `FaceIndexRange` `geometry.rs:20-23`) into `GeometryResult.geometry.indices`/`.vertices`.
- **Cache.** `cache::CacheKey::new(stage_id, stage_version, input_hash)` (`cache.rs:27`); `StageCache::{get,put}` (`:62`/`:114`); `get` validates length+hash and returns a miss on failure. Existing keys `"lightmap"`/`"sh_volume"` (`main.rs:326`/`:455`) hash `postcard::to_allocvec(inputs)` + blake3 (`:317-326`); default dir `<workspace>/.build-caches/prl-cache/` (`:200`). Section ids `Lightmap = 22`, `OctahedralShVolume = 34` (`level-format/src/lib.rs:114,191`).

## Scope

### In scope

- **Lightmap:** per-light contribution layers for all light types (point/spot are sparse; directional is one non-sparse full-atlas layer), cached per light and composited.
- **SH:** per-probe-group cache entries — the probe grid partitioned into spatial groups, each baked over its probe subset with the existing per-probe algorithm and a **bounded reaching-light set** (finite reach cutoff from Task 1), then assembled into the volume. Warm-only approximation; the cold `--no-cache` path runs the exact whole-volume bake.
- **Warm/cold contract:** warm builds are iteration-only; the shippable artifact is a cold `--no-cache` build. Warm SH bakes emit a one-line warning that indirect lighting is approximate; `--no-cache` selects the exact whole-volume SH bake.
- New stage ids on the existing `StageCache` (`"lightmap_layer"`, `"sh_group"`), each with its own version constant, in the same cache dir (`.build-caches/prl-cache/`).
- Corruption handling: a cache entry failing length/hash validation or deserialization is a miss (warn, re-bake), mirroring `StageCache::get`.
- Determinism: the lightmap composite is byte-identical to the monolithic `bake_face_chart`. Warm SH is a bounded-reach approximation (not byte-identical to the monolithic bake); the cold `--no-cache` SH equals the monolithic `bake_sh_volume` exactly. Both bakes are self-consistent (same inputs → same bytes).
- A gate test (lightmap exactness, cold-SH exactness, warm-SH tolerance) and a Task 1 spike (storage + SH reach-cutoff choice + realized single-light invalidation locality + group size) gating before wiring lands.

### Out of scope

- **Per-face lightmap atlas repartitioning.** Atlas layout unchanged: all layers share the one atlas the current packer produces.
- **Per-light SH decomposition / primary-ray-hit caching.** Considered and rejected for v1 (see §6); per-group keeps the bake structure intact instead.
- **Incremental geometry invalidation finer than the group/layer dependency slice.** A geometry edit re-bakes the layers/groups whose dependency slice changed; tightening that is future work.
- **Runtime or PRL format changes.** No new shipped section; the runtime never sees layers or groups.
- **SDF atlas stage** and the **animated weight-map stage** (already cached whole-stage). Untouched.
- **Multi-bounce ripple convergence.** v1 keeps the existing single-pass indirect model.

## Acceptance criteria

- [ ] Building a map twice with no change: the second build serves both sections from cache, rebakes nothing, and emits a `.prl` byte-identical to the first.
- [ ] The warm lightmap composite is byte-identical to the monolithic `bake_face_chart` output on every fixture in `content/dev/maps/` (the composited atlas before BC6H encode; the lossy encode is shared and unchanged).
- [ ] A cold `--no-cache` build's SH is byte-identical to the monolithic `bake_sh_volume` output on every fixture (ship-path regression guard). Warm SH stays within the agreed tolerance of the cold bake (it only drops past-cutoff far bounces).
- [ ] Editing one point/spot light: its lightmap layer reports a miss and re-bakes; the SH groups within that light's bounded reach report misses and re-bake; every other lightmap layer and SH group hits (verifiable in build progress logs).
- [ ] Editing a directional light: its lightmap layer re-bakes and all SH groups re-bake (directional reaches every group) — correct, with no other lightmap layer affected.
- [ ] Editing geometry: only the lightmap layers whose influence-bounded face slice changed re-bake (the rest hit); all SH groups re-bake (SH rays trace full geometry, so the indirect channel's geometry dependency is whole-map — light-edit locality is unaffected).
- [ ] A corrupt or missing cache entry (layer or group) is detected, discarded with a warning, and re-baked; the build succeeds.
- [ ] `--no-cache` bypasses all layer/group reads and writes and selects the exact whole-volume SH bake (and the exact lightmap bake); output matches the monolithic bakes. This is the shippable path.
- [ ] Warm SH builds emit a one-line warning that indirect lighting is approximate and a clean (`--no-cache`) bake is required for a final/shipped map.
- [ ] Task 1 records, for a heavily-lit fixture and campaign-test: lightmap-layer + SH-group on-disk and peak-memory sizes, the chosen SH reach cutoff and the warm-vs-cold SH error it implies, the realized fraction of SH groups invalidated by a single point/spot light edit at that cutoff, and the chosen group size — and the owner signs off the go/no-go.

## Tasks

### Task 1: Storage and locality profiling spike

Before wiring, measure the two things that decide the design's payoff and its substrate, on a heavily-lit fixture plus campaign-test:

- **Storage:** lightmap-layer count/bytes and SH per-group count/bytes (on-disk and peak intermediate memory). The SH group cache stores output-sized tiles+moments, so it should be modest; confirm. Decide whether the flat-file `StageCache` holds at these entry counts or a packed store is warranted (this plan assumes flat-file; "packed store needed" triggers re-planning before wiring).
- **SH reach cutoff (the load-bearing choice):** because probe rays are unbounded (`f32::INFINITY`), the *exact* per-group reach is whole-map — so the design hinges on a finite cutoff that trades warm fidelity for iteration speed. Pick a candidate cutoff (a cap on bounce distance and/or `falloff_range + dilation`) and measure two things: the warm-vs-cold SH error it produces (within iteration tolerance for this low-frequency channel?), and the realized fraction of SH groups a single point/spot light edit invalidates at that cutoff. If no cutoff gives both acceptable error and useful locality, the SH half is a no-go (fall back to whole-stage SH caching) — record that outcome too.
- **Group size:** pick the probe-group dimensions that balance cache-entry count against invalidation granularity; record the choice and its sensitivity.

Document the numbers in `research.md`. Output is a go/no-go owner gate (the project owner reviews the numbers and decides), not production code. The lightmap half is unconditional; this gate decides the SH half.

### Task 2: Lightmap layer types and serialization

Define the per-light lightmap layer payload and its deterministic (de)serialization. Per-atlas-texel: linear irradiance, the **unnormalized** weighted direction, and coverage — the values accumulated in `bake_face_chart` *before* `weighted_dir.normalize()`. Precision is lossless: full-precision `f32` for irradiance, weighted-direction, and coverage (never `f16`). Compiler-internal, never shipped — the encoding only needs to round-trip exactly and hash deterministically (fixed byte layout; exact form is the implementer's choice). Layer-format changes are gated by the `"lightmap_layer"` version constant (Task 6). On deserialize/hash-mismatch failure, the layer is a miss (warn, re-bake).

### Task 3: Lightmap single-light bake entry point

Hoist the per-light body of `bake_face_chart` so one light's irradiance + unnormalized-weighted-direction layer can be baked across the shared atlas. Call `prepare_atlas` **once** with the full static set (its empty-light branch is a placeholder path; do not call it per single-light layer) and thread the resulting `charts`/`placements`/`atlas_width`/`atlas_height` into every single-light bake. Entry-point inputs: one `MapLight`, the shared atlas (charts/placements/dims), the BVH + primitives + `GeometryResult` (the `&Bvh<f32, 3>`, `&[BvhPrimitive]`, `&GeometryResult` `bake_face_chart` already takes), and `LightmapConfig`; output is a layer (Task 2). Directional lights use the same entry point, producing a full-atlas (non-sparse) layer.

### Task 4: Lightmap influence + invalidation

Per-light dependency set and cache key. Reuse `affinity_grid::light_aabb` — promote it (and `AABB_PADDING_METERS` if read directly) to `pub(crate)`; leave the `delta_sh_bake.rs:416` copy untouched. Point/spot → `falloff_range + AABB_PADDING_METERS` AABB; directional → world AABB. The layer's key folds: the light's params (relevant `MapLight` fields under a fixed `postcard` encoding, the discipline the whole-stage key uses), the influence-bounded **geometry slice**, `lightmap_density` + `area_sample_count`, and the atlas layout descriptor (atlas dims + per-chart placements — so an atlas repack invalidates all layers). The promoted `affinity_grid::light_aabb` (the f32 copy) is authoritative — its exact output feeds the key, independent of the `delta_sh_bake.rs` copy (no cross-matching). Charts/placements don't derive `Serialize`; fold them via the same deterministically-derived proxy-bytes fingerprint the animated-weight-map stage already uses (`research.md` §4c), not a direct hash. The atlas overflow/density-halving retry repacks the whole atlas, changing this descriptor and so invalidating every lightmap layer by design (SH is unaffected); Task 1 should note whether the profiled fixtures hit that retry path.

**Geometry slice (mechanism shared with Task 6).** Hash *face content*, not BVH topology. The slice is the `BvhPrimitive`s (one per face) whose face AABB overlaps the influence AABB, gathered via the BVH (accelerator only), taken in canonical `sort_key`-field order (`bvh_build.rs:41`, populated by `primitive_sort_key` `:96-103`) — not the post-`Bvh::build` permutation — and hashed by each face's geometry (`index_offset..index_offset+index_count` from `GeometryResult.face_index_ranges[face_idx]` into `GeometryResult.geometry.indices`/`.vertices`). Concretely it is the existing whole-stage `GeometryResult` content hash (postcard + blake3) restricted to the influence-overlapping faces — a narrowing of a shipping mechanism. Decoupled from BVH build determinism; chosen over leaf-index hashing (which would couple to tree topology and drift across builds). Invalidation is emergent: a slice that changed → new key → re-bake; unchanged → hit. Occlusion is local to the influence sphere (any occluder on a light→texel segment is nearer than the texel, hence inside `falloff_range`), so the falloff-AABB slice is a sound conservative dependency set. This local-occlusion bound is what lets the *lightmap* restrict its geometry slice; SH has no such bound (rays are unbounded), so Task 6 hashes the same `GeometryResult` content whole-map instead.

### Task 5: Lightmap compositor

Element-wise sum per-light irradiance across layers, element-wise sum the unnormalized weighted directions, then a single `normalize` per texel — reproducing `bake_face_chart`'s output bit-for-bit (same addition order = global `static_lights` order). Per-texel `coverage` is the logical OR of the layers' coverage flags. Out-of-influence texels in a sparse layer contribute exactly `0.0`; the compositor sums each layer into a zero-initialized full atlas, so the on-disk layer encoding (dense-with-zeros or explicit texel-index list) stays the implementer's choice. Write the `Lightmap` section. When a map has no static lights, the existing placeholder-atlas path applies.

### Task 6: SH per-group bake + cache

Partition the probe grid into spatial groups (size from Task 1). For each group:

- **Reaching-light set (bounded):** lights whose `falloff_range` region, dilated by the **chosen finite reach cutoff** (Task 1), overlaps the group AABB. Directional lights reach every group. Take the set in global `static_lights` order so the per-hit sum order matches the bake. Out-of-set lights are *dropped* — this is the deliberate warm approximation (an exact set would be whole-map, since probe rays are unbounded). Rays still trace against the full geometry — only the light sum is bounded, so each dropped light removes a nonnegative term (dimmer, never a sky-leak): the warm result is a strict, benign underestimate, not bit-identical to the monolithic bake.
- **Geometry dependency (whole-map):** rays trace the full geometry (unbounded), so a group's output depends on all geometry. The key folds the whole-map `GeometryResult` content hash (the Task 4 mechanism, unrestricted — shared across all groups), so any geometry edit re-bakes every SH group. Only the light set is bounded; that is what makes a *light* edit local. Tightening SH geometry-edit locality would need occlusion/PVS analysis — future work.
- **Key:** `"sh_group"` + version, hashing the reach cutoff, the bounded reaching-light params (fixed postcard encoding), the whole-map `GeometryResult` content hash, `probe_spacing`, and the probe-grid layout descriptor (origin/cell-size/dims + group bounds). Changing the cutoff, or any geometry, therefore invalidates every group.
- **Bake (warm path):** run the per-probe algorithm (`bake_probe_rgb_with_moments` → `pack_octahedral_irradiance_tile`) over the group's probe indices with the bounded reaching-light set (rays trace full geometry; no geometry cutoff); store the group's octahedral tiles + depth moments + validity. Payload format (mirroring Task 2): per probe, the post-`pack_octahedral_irradiance_tile` f16 octahedral tile, the f16 depth moments (`E[d]`, `E[d²]`), and the validity byte, in fixed byte layout with lossless round-trip; assembly is a byte-copy into the section's tile/record offsets (no re-pack), so the assembled warm volume reproduces the per-group bakes exactly. Corruption handling mirrors Task 2: a length/hash failure (via `StageCache`) or a deserialize failure on the group's own codec is a miss (warn, re-bake). `get`/`put` against `StageCache`.

Assembly is placement, not compositing: each group writes its probes' tiles into their offsets in the `OctahedralShVolume` section. The assembled warm volume equals the bounded-reach per-group bakes exactly; it *approximates* the monolithic `bake_sh_volume` (dropping past-cutoff far bounces). The cold `--no-cache` path skips groups entirely and runs the existing exact whole-volume `bake_sh_volume` — the unchanged ship algorithm.

### Task 7: Pipeline wiring + CLI

Wire Tasks 3–6 into `main.rs`, replacing the current whole-stage `"lightmap"`/`"sh_volume"` get/insert. The animated-weight-map and SDF-atlas stages stay whole-stage cached and are not rewired; atlas layout is unchanged, so their keys are unaffected. Lightmap: per static light (all `LightType`s), derive the layer key (Task 4), `get`; on miss bake (Task 3) and `put`; composite (Task 5). SH: per group, derive the key (Task 6), `get`; on miss bake and `put`; assemble. New stage ids `"lightmap_layer"`/`"sh_group"`, each with its own version constant, manually bumped when that algorithm or format changes; the existing whole-stage `STAGE_VERSION` constants are retired from the hot path (kept only if other call sites use them). Surface per-entry hit/miss in progress logs. Respect `--no-cache` / `--cache-dir`. `--no-cache` additionally selects the exact whole-volume `bake_sh_volume` (the ship path), not a cache-bypassed per-group bake; the warm per-group SH path emits a one-line warning that indirect lighting is approximate and a clean bake is required for a final map.

### Task 8: Tests + determinism gate

Cover: round-trip skip (build twice → all entries hit, rebakes nothing); single point/spot light edit (its lightmap layer + in-reach SH groups miss, rest hit); directional light edit (its lightmap layer + all SH groups miss); geometry edit (only dependency-overlapping lightmap layers miss; all SH groups miss); corruption recovery; `--no-cache` bypass; `--cache-dir` redirect (entries read/written under the override); warm-SH approximation warning fires. **Gates:** (1) the warm lightmap composite is byte-identical to monolithic `bake_face_chart` (composited atlas, pre-BC6H) across every `content/dev/maps/` fixture — the lightmap is exact. (2) A cold `--no-cache` build's SH is byte-identical to monolithic `bake_sh_volume` across every fixture — the ship-path regression guard. (3) Warm SH stays within the agreed tolerance of the cold bake (per-probe error bound from Task 1). The lightmap gates against the legacy bake exactly; SH gates the cold path exactly and the warm path within tolerance.

### Task 9: Documentation

Update `build_pipeline.md` §Build Cache: add the `"lightmap_layer"` and `"sh_group"` stage ids and their independent version-bump discipline; note the per-light-lightmap (exact, warm == cold) / per-group-SH (bounded-reach warm approximation, cold-exact) grain; and document the **warm/cold-build contract** — warm builds are iteration-only, the lightmap is exact, warm SH is approximate, and a clean `--no-cache` build is required for a final/shipped map (with the exact whole-volume SH). Amend §Determinism invariant too: it currently requires byte-identical output from both cached stages and that new `sh_bake.rs` code preserve it — carve out the warm per-group SH path as an explicit exception (the byte-identical guarantee now covers the lightmap composite and the cold whole-volume SH only). No `rendering_pipeline.md` change (runtime and PRL format untouched). Record in `research.md` both the §4d → §6 grain reversal and the further reframing from "SH bit-identical" to "SH warm-approximate / cold-exact" (the unbounded-ray finding that forced it).

## Sequencing

**Phase 1 (sequential):** Task 1 — spike gates substrate, the SH reach cutoff, group size, and the SH-half go/no-go (the lightmap half is unconditional).
**Phase 2 (concurrent):** the lightmap chain (Task 2 → Tasks 3, 4 → Task 5) and the SH track (Task 6) are independent subsystems and proceed in parallel; both use the `GeometryResult` content-hash mechanism (Task 4 restricts it to the influence slice; Task 6 uses it whole-map, which is just the existing whole-stage hash), so the SH track has no hard dependency on Task 4 and runs fully parallel.
**Phase 3 (sequential):** Task 7 — wiring consumes both tracks.
**Phase 4 (sequential):** Task 8 — tests and the determinism gate validate the wired pipeline.
**Phase 5 (sequential):** Task 9 — docs, once validated.

## Open questions

- **Substrate (resolved by Task 1).** Flat-file `StageCache` vs. a packed store, decided by lightmap-layer and SH-group entry counts/bytes. Flat-file is the expected answer (group count is modest), pending the numbers.
- **SH reach cutoff & warm/cold divergence (resolved by Task 1).** Probe rays are unbounded, so the cutoff is a chosen approximation traded against warm fidelity; Task 1 picks it and quantifies the warm-vs-cold error and the realized invalidation fraction. The accepted risk: a designer judging indirect lighting on a warm build — mitigated by the approximation warning and the cold ship build. If no cutoff balances error and locality, the SH half falls back to whole-stage caching (the lightmap half still ships).
- **Cold-build enforcement.** Production/CI must run `--no-cache` (or a release-bake flag) so an approximate warm `.prl` never ships. Whether to add an explicit `--release-bake` flag or rely on the `--no-cache` convention (plus the warm warning) is open — leaning on convention for v1.
- **Group size.** The file-count vs. invalidation-granularity dial; Task 1 picks it. A future refinement could size groups adaptively to light density.
