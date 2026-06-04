---
name: Incremental Bake Per Element — Research
description: Codebase + prior-art research consolidated to unblock drafting. The blocking plan (build-stage-cache) has shipped.
type: research
---

# Incremental Bake Per Element — Research Brief

> Prepared to feed `/draft-plan`. The stub (`index.md`) was blocked on `build-stage-cache/`
> landing; that plan is now in `plans/done/`, so the blocking questions can be answered against
> real code. Findings below are grounded in source with `file:line` citations. Sources: four
> research passes (build-stage-cache as-built, lightmap baker, SH baker, external prior art).

## TL;DR — the path forward

1. **The blocker is cleared.** `build-stage-cache` shipped. Its `CacheKey` / `STAGE_VERSION` /
   determinism substrate is solid and directly reusable. Its **storage** backend (flat
   one-file-per-hex-key, `sync_all` per put, nuke-to-evict) does **not** scale to per-element
   granularity and must be redesigned.
2. **SH and lightmap are at very different readiness.** SH is *nearly ready* — `affinity_grid.rs`
   already computes light→probe-block reach for the animated/delta path, and per-probe bake is
   already isolated, parallel, and deterministic. Lightmap has **two hard prerequisite blockers**
   (atlas packer is not reuse-stable; faces have no stable identity).
3. **Recommendation: split into two plans, SH first.** The stub already anticipated this. SH-per-probe
   is a clean, low-risk first slice that exercises the new storage substrate and the
   light-reach-inversion pattern. Lightmap-per-face is a second, heavier plan gated on a packer rewrite.
4. **Ship as two-mode, per universal prior art:** `--incremental` for the designer loop (dirty-set +
   guard band, may seam), full clean bake as the default for release/CI. Don't ship incremental patches.

---

## 1. Substrate as-built (`crates/level-compiler/src/cache.rs`)

**Reusable as-is:**
- `CacheKey::new(stage_id: &str, stage_version: u32, input_hash: &[u8])` → `blake3(stage_id || version_le || input_hash)` (`cache.rs:18-42`). The arbitrary `input_hash` seam means per-element keys drop in with zero change. A third stage (`sdf_atlas`) already proves stages extend trivially.
- Per-stage `pub const STAGE_VERSION: u32` folded into the key (`lightmap_bake.rs:54`, `sh_bake.rs:43`, `sdf_bake.rs:59`).
- Determinism floor is established and tested (byte-identical output invariant, `build_pipeline.md:186`).

**Must be redesigned for per-element:**
- Storage is **one flat file per entry**, named by 64-char hex, in a single directory (`cache.rs:154-156`). Millions of per-face/per-probe blobs = millions of inodes in one dir → directory-index degradation, slow `readdir`, brutal `rm -rf` (the documented "nuke to evict" workflow).
- **`sync_all()` per `put`** (`cache.rs:150`) + a rename per entry → N fsyncs for N elements. Untenable at per-element scale.
- Fixed 36-byte header per entry (`[len u32 | blake3(payload) 32B | payload]`, `cache.rs:8-13`) is large relative overhead for ~hundreds-of-bytes payloads; 4 KiB block rounding compounds it.
- API is blob-in/blob-out, no batch, no enumeration (`get`/`put`, `cache.rs:62,114`). No `put_many`, no "which entries reference light X" query.
- **Output is not postcard** — cached payload is the section's own `to_bytes()`/`from_bytes()` codec (`main.rs:294-298,366`). postcard is used only to fingerprint *inputs*. A per-element design needs a **per-element wire format** (per-face blob, per-probe tile) that does not exist yet — the format crate only has whole-section codecs.

**Storage options (from prior art, §storage):** SQLite or LMDB for a metadata/dependency index (enables "which elements reference light X" as an indexed query) + a packed mmap'd array for the dense probe grid + a chart-keyed blob store for variable-size faces. Avoid RocksDB (single-writer offline doesn't need its write-concurrency; tuning cost is a liability). Avoid one-file-per-element. Minimal interim fix if we stay file-based: hash-prefix subdir sharding (`ab/cdef…`) + drop per-entry fsync.

## 2. SH per-probe — nearly ready (`crates/level-compiler/src/sh_bake.rs`)

- **Per-probe is already the ideal unit:** independent `into_par_iter().map().collect()` over a flat probe index (`sh_bake.rs:185-211`); each probe = 256 Fibonacci-sphere rays, sequential float accumulation, index-derived seeds (no RNG). Caching one probe in isolation cannot perturb another. Determinism is tested and documented.
- **Light reach already exists for the delta path** — this is the key asset. `affinity_grid.rs` computes a light's influence AABB from `falloff_range + padding` (`affinity_grid.rs:258-267`), intersects it with a **portal flood-fill** of reachable leaves (`affinity_grid.rs:150-182`), and decomposes to 4×4×4 probe blocks (`AFFINITY_FACTOR = 4`, `affinity_grid.rs:35`; `decompose_affinity`, `:89-142`). Inverting this gives "which probes does light L reach" almost for free. Currently runs only for animated/delta lights, not the static bake.
- **Natural granularity:** single probe (finest) or the existing 4×4×4 **affinity cell** (amortizes per-element overhead, already aligned to the light-reach decomposition). Recommend the affinity cell as the cache unit — it reuses existing machinery.
- **Stable identity caveat:** probe linear index `x + y·nx + z·nx·ny` (`sh_bake.rs:341-349`) is only stable if grid origin/dims are unchanged, and those derive from the **world vertex AABB** (`sh_bake.rs:315-338`). A geometry edit that resizes the AABB shifts *all* indices. Key on `(world position)` or `(grid coord, origin, dims)`, not the raw index.
- **Ordered-light-set caveat:** the soft-visibility seed mixes `light_index` = position in the bake slice, and SH accumulation iterates `lights.iter()` in order (`sh_bake.rs:890`). A per-probe cache must key on and preserve the *exact ordered* light set the probe saw, or byte-identity breaks.
- BVH is rebuilt from geometry every run (uncached by design; pure function of geometry, `build-stage-cache/index.md:75`). No change needed.

## 3. Lightmap per-face — two hard blockers (`crates/level-compiler/src/lightmap_bake.rs`)

- **Blocker A — atlas packer is not reuse-stable.** `shelf_pack` sorts charts by height, derives atlas width from *total area* (next-pow-2), and lays out shelves sequentially (`lightmap_bake.rs:546-604`). Change one face's texel count → total area changes → atlas width can jump → **every** placement shifts. Worse: retry-on-overflow **halves global density** and re-bakes the whole atlas (`main.rs:318-363`). Unchanged faces do **not** keep their atlas slots. The baked *values* are position-independent and reusable; the *placement* is not. **Requires a stable-slot/guillotine allocator or per-face sub-atlas pages before per-face reuse is possible.**
- **Blocker B — no stable face identity.** Faces are keyed only by positional index into `face_index_ranges` (`bvh_build.rs:70-73`); the BVH `sort_key` keys on `index_offset` (geometry layout), which shifts on reorder. A per-element cache needs a content-hash face key synthesized from spatial inputs (vertex positions + normal + material + reaching-light params).
- **Good news:** the influence primitive exists — `falloff_range` zeroes a light's contribution beyond its radius for all three falloff models (`lightmap_bake.rs:849-869`), a true finite influence sphere. Faces already have AABBs (`bvh_build.rs:78`); an AABB-vs-light-sphere test gives the per-light→faces set cheaply. Bake is **fully serial today** (no rayon in `lightmap_bake.rs`) but embarrassingly parallel — adding `par_iter` over charts is easy and slots into incremental work distribution. Determinism is solid (`texel_seed` FNV, no RNG).

## 4. Prior art — what transfers, what doesn't

- **Reality check:** mainstream engines mostly do *progressive full refinement* (Unity Progressive Lightmapper) or *viewport-scoped* selection (Unreal "Bake What You See"), **not** light-scoped dependency invalidation. Even Unity kept incremental probe baking on the *roadmap*. Truly matching prior art is academic: **Luksch & Wimmer, "Incrementally Baked Global Illumination"** (many-light/VPL dependency tracking + priority re-convergence). Worth pulling the full PDF before hardening algorithm details.
- **Steal the per-texel-independence principle** (Unity PL): bake elements as independent units with no shared global irradiance cache a single light invalidates wholesale. We already have this (per-probe, per-chart).
- **Track dependencies, don't just guess radii.** Store per element: contributing light IDs + sampled-geometry AABBs + input hash. Dirty set = elements referencing the changed light ∪ elements whose sampled geometry moved. Influence-sphere/BVH query is the *fallback* for elements with no prior record.
- **Guard band for seams.** Denoise/dilation are neighborhood ops; rebaking a patch in isolation picks up stale neighbor texels. Rebake dirty set **+ a halo** wide enough to cover the filter kernel, then write back only the dirty core. Cross-object/atlas-island stitching is unsolved even in Unity — expect residual boundary seams in incremental mode; accept them as iteration-only.
- **Indirect ripple is the fundamental limit.** A light change propagates past its direct radius via bounces. Bound it: SH/lightmaps are *linear* in incoming radiance, so storing contribution **per-light** makes the *direct* term an exact subtract-add on a light edit. Propagate *indirect* via a small fixed number of Jacobi bounces gated by an energy threshold (bright/high-albedo surfaces ripple farther). Full multi-bounce only in release mode.
- **Two-mode CLI, explicit.** `--incremental` (dirty-set, may seam) for iteration; full clean bake as default for release/CI. Every shipping engine has this escape hatch.

## 4b. Room/cluster grouping (instead of per-face) — strongly recommended

Investigated after the question "could we leverage the BVH to group faces into rooms?". Verdict: **the grouping idea is right and changes the whole risk profile — but group by the engine's BSP leaf / cell, not the BVH.**

- **Don't use the BVH as the grouping structure.** The BVH is a SAH ray-acceleration tree (`bvh_build.rs:156`); its subtrees have no semantic meaning and reshuffle on any geometry edit. It *carries* the room id but doesn't define it.
- **The grouping key already exists for free.** Every face carries `FaceMeta.leaf_index` = the raw BSP leaf index (`geometry.rs:143-144`), which is exactly the BVH's `cell_id` (`bvh_build.rs:84`). Faces are already emitted in leaf order (`build_leaf_ordered_faces`, `geometry.rs:382-407`), and the baker already has this via `GeometryResult` — it just iterates positionally today (`lightmap_bake.rs:426,653`). **No new plumbing.** Probes likewise resolve to a leaf via `find_leaf_for_point` (`sh_bake.rs:360-369`). One unified room key for both bakers.
- **A "room" = a portal-connected component of empty leaves.** The BSP splits on every brush plane, so one designer room fragments into several convex leaves stitched by portals (`partition/brush_bsp.rs`, `portals.rs`). Leaf is the finest stable grain; coarsen to portal regions later if desired.
- **This makes the storage redesign UNNECESSARY (the big win).** Cell ids are hard-capped at `MAX_CELL_ID_EXCLUSIVE = 4096` (`bvh_build.rs:18,136-141`); realistic maps have hundreds of leaves. Per-room grain → hundreds-to-low-thousands of cache entries, which the existing flat-file `cache.rs` handles fine. The "redesign storage for millions of blobs" problem (§1) **only existed for per-face grain.** Room grain removes it.
- **The dirty-set mechanism already exists.** `affinity_grid.rs` does a portal flood-fill from a light's leaf — `reachable_leaves` / `cells_for_light` / `decompose_affinity` (`affinity_grid.rs:150-235`), with solid/exterior bypass handled, mirrored in `chunk_light_list_bake.rs:140-179`. Dirty rooms on a light edit = `reachable_leaves(old pos) ∪ reachable_leaves(new pos)`. Reuse directly.
- **Lower merge/seam risk — the other thing you hoped for.** Room boundaries fall on walls/portals, where **direct**-light seams are invisible (no shared lit surface across a solid wall). **Indirect** light bounces one portal hop, so add a **one-portal-hop guard band** for the SH/indirect stage (re-bake reached rooms ∪ their portal neighbors) — a single extra BFS layer on the same adjacency graph; the affinity grid's `AABB_PADDING_METERS` is the existing precedent for over-covering reach.

**Two qualifications (room grouping is necessary, not sufficient):**

1. **Identity still needs a content hash, not the leaf index.** Leaf indices are a global DFS push-order counter (`brush_bsp.rs:417-418`) driven by globally-scored splitter selection — editing brushes in one room can renumber leaves everywhere. Leaf id *is* more stable than a face index (immune to face churn within *other* rooms, and to light-only edits), but not absolute. Key each room on `blake3` of its faces (positions/UVs/normals/material) + the static lights reaching it — reusing the existing `CacheKey` model (`cache.rs:27-36`), computed per room instead of per map. Leaf id is only a within-build handle / coarse bucket.
2. **The atlas packer still must change (blocker A is not dodged by grouping alone).** `shelf_pack` derives one global `atlas_w`/`atlas_h` from *total* chart area (`lightmap_bake.rs:557-562`), so room B's bytes shift when room A changes even with grouping. Fix: **fixed per-room sub-rectangles in the single atlas** (fixed dimensions + fixed per-room slots), so unchanged rooms stay byte-identical. NOT per-room pages/array-textures — the format and runtime are hard-wired to a single 2D atlas (`level-format/src/lightmap.rs:21-63`, `lighting/lightmap.rs:107-129`) and the animated atlas is coupled to one shared UV space (`animated_lightmap.rs:26-29`). This is a contained `prepare_atlas`/`shelf_pack` change that touches neither format nor runtime.

**Net effect on the plan:** room grouping (a) collapses the storage-redesign task, (b) unifies lightmap + SH under one leaf-based dirty-set, (c) reuses the existing portal flood-fill, and (d) lands seams on walls. It does not remove the need for (i) a content-hash room key and (ii) a fixed-layout atlas packer. This is a materially simpler and lower-risk shape than per-face caching, and it may collapse the two-plan split back toward a single plan built on the shared room/leaf substrate.

## 4c. Animated weight-map stage is now cached too — and atlas-coupled (merged from main)

Post-research, `main` landed PR #51 (`fc635a4` + `60caa43`) wiring the **animated-light weight-map** bake into `StageCache`. This changes the plan's scope and reinforces the room-grouping direction.

- **There are now four cached expensive stages, not two.** `CacheKey::new` is called for `"lightmap"`, `"sh_volume"`, `"animated_lm_weight_maps"`, and `"sdf_atlas"` (`main.rs:308,436,591,649`). The stub's framing ("per-face lightmap and per-probe SH") is **incomplete** — an incremental plan must also account for the animated weight-map stage (and SDF atlas).
- **The animated weight-map stage is atlas-coupled to the lightmap.** Its input hash folds `atlas_width`/`atlas_height`, `final_lightmap_density`, and `animated_light_chunks_section.to_bytes()` as a proxy for the (non-`Serialize`) charts + placements (`main.rs:~560-590`). It consumes the **same atlas charts/placements/UV space** as the static lightmap. **Implication:** the fixed-per-room-sub-rectangle atlas change (§4b qualification 2) forces this stage to move in lockstep — repartitioning the atlas changes its inputs and invalidates its cache too. **The lightmap and animated-weight-map stages must be planned together**, sharing one room/leaf grouping and one atlas-layout change. This strengthens the single-plan-on-shared-substrate recommendation (§5.1).
- **Useful precedent — hashing derived-output bytes as an input fingerprint.** Because charts/placements don't derive `Serialize`, the weight-map hash folds the serialized chunk-section bytes as a faithful fingerprint of those deterministically-derived inputs (`main.rs` comment at the `wm_input_hash` block). A per-room key can reuse this trick: hash a room's serialized chart/placement bytes rather than re-deriving `Serialize` for chart types.
- **Determinism subtlety to inherit.** The same PR fixed a cache-state-dependent non-determinism bug: a placeholder lightmap section stores a sentinel `texel_density` of 1.0, so re-preparing the atlas from a cache hit mis-sized it vs. the miss path (`resolve_cached_lightmap_density`, `main.rs:133-150`). Per-element/per-room caching multiplies these hit-vs-miss boundary hazards — every cached unit must reproduce the exact density/atlas-dim context it was baked under. Budget determinism-audit work at the finer grain.
- **Animated lights are already spatially chunked by the same portal flood-fill.** `chunk_light_list_bake.rs:140-179` mirrors `affinity_grid.rs`'s `reachable_leaves` BFS (noted in §4b). So the animated path *already* uses the leaf/portal reach machinery — the room-based dirty-set unifies all of lightmap + animated-weight-map + SH under one mechanism.

## 4d. Per-light decomposition — the reliable, arena-proof grain (compiler-internal)

Investigated after the observation that modern boomer-shooter maps (e.g. Quake Map Jam arenas) have large open spaces where portal-reach (§4b) dirties most of the map — so topology-keyed grain fails exactly where it's needed. The grain must be keyed on **light influence**, not map structure. The reliable way to do that is to decompose by **light entity** rather than by output region. Two axes, and this is the input axis:

- **Per-face / per-probe-block** = *output-spatial* partition (where the result is stored). **Per-light-entity** = *contribution decomposition* (what feeds each result). Because lighting is linear, the per-light axis is the one that divides out the real cost factor: bake cost ≈ `texels × lights × shadow-samples`, and a single-light edit under per-light grain re-samples only that one light. Per-face/per-room still re-sample *every* light at each dirtied texel (the stored value is their sum) — they attack the wrong term.

- **It can be a pure compiler-internal cache — runtime and PRL format untouched.** The final `Lightmap` (22) and `OctahedralShVolume` (34) sections are pure additive sums with no per-light identity (`lightmap_bake.rs:758`, `sh_bake.rs:797`), no per-light clamp/tonemap. The compiler can cache per-light layers, composite them at bake time, and emit byte-identical sections. (Contrast the *animated* path, which deliberately *ships* its per-light decomposition because animation recombines per-frame.)

- **The machinery already exists — for animated lights.** This is essentially a generalization of the dynamic path to static lights, not a new invention:
  - `delta_sh_bake.rs:342,354` already bakes a single light's SH layer via `bake_probe_indirect_rgb` with a one-light slice `[light]`, sharing `sample_radiance_rgb` with the base bake — the exact "bake one light's layer" primitive.
  - `TexelLight` (`level-format/src/animated_light_weight_maps.rs:60`) is already a per-light-per-texel direct-layer representation; `bake_one_chunk` (`animated_light_weight_maps.rs:240-300`) is a working per-light lightmap-layer baker.
  - Influence + invalidation: `affinity_grid::{light_aabb, reachable_leaves, cells_for_light}` and `chunk_light_list_bake::overlaps_chunk` (which **already filters static lights** via `!is_dynamic`, `:80-84`). Seam-free support by construction: contribution is exactly zero at `falloff_range`.

- **Cold-build cost is roughly flat on rays.** The expensive work (`soft_visibility` shadow rays / BVH `segment_clear`) is *already* per-(texel×light); only the cheap shared raster setup loses amortization. Cost shifts to **storage of intermediates** (O(texels × overlapping-lights), same blow-up the animated weight pool already accepts, 16 MB cap at `chunk_light_list_bake.rs:27`) plus a cheap final composite pass.

**Three real risks (a draft must address):**
1. **Dominant-direction nonlinearity.** Irradiance composites exactly, but the static lightmap's bumped-Lambert *direction* channel is a normalized sum of per-light directions (`forward.wgsl:730`, `lightmap_bake.rs:764-774`); `normalize(Σ dir_i)` is not recombinable from independently-normalized per-light directions. The cache must store per-light **unnormalized weighted directions** and normalize the sum at composite. The value is already computed in-loop (`lightmap_bake.rs:765`) — bounded, but the one place "composite = full bake" needs care.
2. **Whole-map lights get no benefit.** Directional/sun lights have world-AABB influence (`affinity_grid.rs:260`) → a per-light layer covers every texel (no sparsity) and invalidates on essentially every geometry edit. Per-light wins on **point/spot** lights — which are exactly the ones designers tune for local mood and set-pieces. Sun is typically one light set once; acceptable.
3. **Determinism + non-local shadow invalidation.** Composite must sum in fixed light order to preserve byte-identical output (`build_pipeline.md:186`). Geometry edits invalidate any layer whose influence AABB overlaps the changed-geometry AABB (occlusion is non-local; conservative but sound — the AABB test already exists).

**Grain comparison (the three candidates):**

| Grain | Straightforward | Reliable | Arena-proof | Reach | Runtime change |
|---|---|---|---|---|---|
| Per-SH-probe-block | ✅ easiest (regular grid, no packing/identity) | high | yes (radius) | narrow — indirect SH only | none |
| **Per-light-entity** | medium (reuses animated path) | **highest** (linear, seam-free) | **yes** (influence) | **broad — direct + indirect** | **none (compiler-internal)** |
| Per-face | ❌ hardest (packer + identity) | low | partial | doesn't cut the `lights` factor | none |

Per-light **subsumes** the spatial schemes: "store light L's contribution over the probes/texels in its influence sphere" *is* per-light decomposition with influence-bounded spatial support. Per-SH-probe-block is the easy first slice; per-light is the destination. Per-room/portal (§4b) is demoted by the arena case; per-face is demoted as the wrong cost axis.

## 5. Open decisions for `/draft-plan`

0. **Stage scope.** The incremental plan now spans **four** cached stages, not two: `lightmap`, `animated_lm_weight_maps` (atlas-coupled to the lightmap — must move together, §4c), `sh_volume`, and `sdf_atlas`. Decide which are in scope. Recommend: lightmap + animated-weight-map (coupled pair) + SH as the core; SDF atlas assessed separately.
1. **Split or single plan?** Two paths: **(a)** two plans (SH-per-probe first, lightmap-per-face second, gated on packer rewrite); or **(b)** if adopting room/leaf grouping (§4b), a single plan on the shared room-based dirty-set substrate, since grouping unifies all bakers and removes the storage-redesign asymmetry. Lean toward (b) — grouping is the lower-risk shape, and the lightmap/animated-weight-map atlas coupling (§4c) means they cannot be cleanly separated anyway.
2. **Cache grain — superseded by §4d.** Earlier this leaned per-room (BSP leaf / portal). The arena case (a light flooding a large open Map-Jam-style space) breaks topology-keyed grain. Revised recommendation: **per-light-entity decomposition as a compiler-internal cache** (§4d) — keyed on light influence (arena-proof), seam-free by construction, runtime untouched, reusing the animated-light machinery. **Per-SH-probe-block** is the low-risk first slice (SH only); **per-light** is the destination (direct + indirect). Per-face and per-room are both demoted.
3. **Storage backend.** Per-light intermediates are O(texels × overlapping-lights) — heavier than per-room but the animated path already accepts this shape (16 MB cap, `chunk_light_list_bake.rs:27`). Still likely viable on the existing flat-file substrate at per-light (not per-face) grain; confirm against worst-case overlap on a large arena fixture before committing. SQLite/LMDB only if storage profiling demands it.
4. **Stable identity scheme.** World-position-based keys (survive AABB/index shifts) vs. content-hash keys. Needed for both bakers.
5. **Indirect-ripple envelope.** Direct-only incremental + full-bake-for-release, vs. K-bounce energy-thresholded propagation. Defines the "acceptable seams" contract the stub asked for.
6. **Packer rewrite scope (lightmap only).** Stable-slot/guillotine allocator vs. per-face sub-atlas pages — a prerequisite task, possibly its own plan.

## 6. Grain reversal — per-channel hybrid (per-light lightmap + per-group SH)

Settled during drafting/review, this supersedes §4d's "per-light for both" for the SH channel. The earlier analysis demoted per-room/per-probe-block grain on the arena flood-light case *while assuming per-light SH was cheap*. Exposing the true cost of per-light SH — and considering a per-channel hybrid §4d never evaluated — flips the SH decision.

**Per-light SH is not cheap.** Decomposing the SH bake per light forces one of three bad choices:
1. **Re-cast each probe's primary rays per light** → cold-build regression ≈ ×(average per-probe light-overlap depth), worst exactly in the open-arena flood-light case the grain is meant to serve.
2. **Cache the primary-ray hits** (foundation-first) and re-shade per light → correct and no cold regression, but a ~100–200 MB *persisted* intermediate hit cache (millions of probe×ray records), a two-phase emit/shade refactor of the most numerically sensitive code, and whole-hit-cache invalidation on *any* geometry edit.
3. **Cull probes by the light's influence AABB** (the delta path's approach) → **incorrect**: SH falloff is keyed on the *bounce hit point*, not the probe (`sh_bake.rs:524-597`), so a probe outside a light's AABB can still receive a real far-field bounce off a wall inside the light's range. Probe-AABB culling drops it — a visible GI change, not rounding.

**Per-probe-group SH avoids all three.** Partition the probe grid into spatial groups; bake each group with the existing per-probe algorithm over its probe subset and a conservative reaching-light set. Because probes are independent in the single-pass indirect bake, out-of-range lights contribute exactly `0.0`, and adding `0.0` does not perturb a float sum, a group bake is **byte-identical** to the monolithic bake for those probes — bit-compatibility with today's `.prl` is preserved. No re-casting (no cold regression), no hit cache, no compositor, no directional fallback (directionals are ordinary members of every group's light set). Geometry-edit invalidation is local to groups whose geometry-in-reach changed.

**Cost accepted:** SH invalidation locality is bounded by each light's spatial reach (`falloff_range + SH-ray-reach`) and occlusion, so a light flooding many groups — and any directional — invalidates broadly. This is the per-region weakness §4b/§4d flagged, but it is far more tolerable here because **the lightmap stays per-light**: editing any light (flood lights included) updates its *direct* contribution instantly, and direct is the dominant visual channel; the soft indirect SH catches up at group grain. Task 1 measures the realized single-light group-invalidation fraction.

**Lightmap stays per-light (§4d holds).** Direct illumination has a hard `falloff_range` cutoff (`lightmap_bake.rs:849-869`), so per-light layers have bounded support, sum back to the monolithic result bit-for-bit (storing unnormalized weighted directions, normalizing once — risk 1 of §4d), and give instant single-light edits.

**Net:** a per-channel hybrid — per-light lightmap layers + per-group SH cache entries — dominates uniform per-light on correctness (bit-identical), cold-build cost (no regression), simplicity (no hit cache / compositor / directional fallback), and cache size, losing only flood-light SH edit latency, which the per-light direct channel masks. Per-face and uniform per-light SH are both demoted.

## Task 1 — Spike results

> Owner go/no-go gate for the SH half. The lightmap half is unconditional and not gated here.
> Numbers labelled **(measured)** come from real `prl-build` bakes of the two fixtures and a
> direct warm-vs-cold per-probe SH experiment; **(derived)** numbers are computed analytically
> from measured atlas/probe counts and the committed wire layouts; **(estimated)** numbers are
> reasoned bounds. Measured on a debug build, 2026-06-04, default `--probe-spacing 1.0`.

### Fixtures profiled

- **campaign-test** (heavily lit): 13 static lights (11 point + 2 spot) + dynamic/animated. **(measured)**
- **occlusion-test** (second heavily-lit fixture): 6 static lights (point + spot after translation). **(measured)**

Both required the `scripts-build` sidecar (campaign-test's worldspawn `data_script`); built it and
placed it beside `prl-build` in `target/debug/` to bake. No bearing on the cache design.

### 1. Storage

**Measured section sizes (cold whole-map bake):**

| Fixture | SH probes | SH section | Lightmap atlas | Lightmap section | Affinity grid (4³ cells) |
|---|---|---|---|---|---|
| campaign-test | 194,028 | 57,436,896 B (~57.4 MB) | 4096×4096 | 83,886,108 B (~80 MB) | 11×4×16 = 704 |
| occlusion-test | 57,510 | 17,049,948 B (~17.0 MB) | 4096×4096 | 83,886,108 B (~80 MB) | 10×3×8 = 240 |

Per-probe SH output is **296 B** (measured: 57,436,896 / 194,028 = 296): an 8-byte `OctahedralShProbe`
record (`validity` + f16 `E[d]` + f16 `E[d²]`, `OCTAHEDRAL_PROBE_STRIDE = 8`) plus a 6×6 = 36-texel
octahedral tile at 8 B/texel (`OctahedralAtlasTexel = [u16;4]`) = 288 B. The per-group cache payload
(Task 6: f16 tile + f16 moments + validity byte per probe) is exactly this output, so **296 B/probe**
is the SH-group storage unit. **(measured + derived)**

**SH per-group entry counts and bytes (derived from probe counts):**

| Group size | Probes/group | Bytes/group | campaign groups | campaign total | occlusion groups |
|---|---|---|---|---|---|
| 4³ probes | 64 | ~18.9 KB | ~3,032 | ~57.4 MB | ~900 |
| 8³ probes | 512 | ~151 KB | ~380 | ~57.4 MB | ~115 |

Group **count** is at most the affinity-cell count scale (hundreds to low thousands), not millions —
the per-group SH cache total equals the SH section size (~57 MB worst fixture) plus the flat-file
36-byte-per-entry header overhead, which is negligible (≤ ~110 KB across 3,032 entries). Peak
intermediate memory is bounded by the existing whole-volume bake's working set (the `Vec<BakedProbe>`
of 27 f32 coeffs + metadata per probe ≈ 116 B/probe ≈ 22 MB for campaign-test) — per-group baking only
ever holds one group's probes plus the assembled output, so peak does **not** exceed the current
monolithic bake. **(derived / estimated)**

**Lightmap-layer storage (derived).** Atlas is 4096×4096 = 16,777,216 texels on both fixtures. Per
Task 2 the per-light layer stores, per texel: irradiance (the bake's `irradiance[idx*4..]` rgba =
4×f32 = 16 B), the unnormalized weighted direction (`Vec3` = 3×f32 = 12 B), and coverage (1 B) ≈
**29 B/texel** dense. A **directional** (non-sparse) layer is therefore ~487 MB full-precision — large,
but on-disk only and one such layer per directional light (typically one sun, often zero). **Point/spot
layers are sparse**: only covered texels are stored. campaign-test packs to a full 4096² atlas with
13 static lights, but each point/spot lights a bounded falloff sphere, so the union of covered texels
per light is a small fraction of the atlas; a sparse encoding (explicit `(texel_index, payload)` list)
keeps each point/spot layer in the single-digit-to-low-tens-of-MB range. Total lightmap-layer on-disk
storage is dominated by any directional layers; with the typical 0–1 directional lights, total stays
comfortably in the low-hundreds-of-MB worst case, hundreds of KB–tens of MB for the common
point/spot-only case. Peak intermediate memory: one full-precision atlas (~487 MB at 29 B/texel for
4096²) held during a single layer's bake + composite — the same order as the existing baker's atlas
buffers. **(derived)**

**Atlas overflow / density-halving:** neither fixture hit the retry path (no "Lightmap atlas overflow"
warning fired). Both pack to 4096×4096 (the natural `MAX_ATLAS_DIMENSION`) without escalation. So the
"repack invalidates all lightmap layers" hazard (Task 4) is latent but not exercised by these
fixtures. **(measured)**

**VERDICT — substrate: flat-file `StageCache` holds.** SH-group entry counts are hundreds to ~3,000
(group-size dependent), each ~19–151 KB — exactly the "modest" regime the existing one-file-per-hex-key
store handles (the §1/§4b "millions of blobs" failure mode was a per-face concern that per-group SH and
per-light lightmap both avoid). Lightmap-layer count = static-light count (≤ ~13 here, tens in practice),
sparse for point/spot. **No packed store needed; the plan's flat-file assumption is confirmed.** The
only flat-file cost notes: per-entry `sync_all` (`cache.rs:150`) at a few thousand puts is a one-time
cold-bake tax (not the warm hot path the plan optimizes), and a directional lightmap layer's ~487 MB
exceeds nothing structurally but is the largest single entry — fine for flat-file.

### 2. SH reach cutoff (the load-bearing choice)

**Experiment (measured).** Temporary instrumentation (since removed) ran the real per-probe bake
(`bake_probe_rgb_with_moments`, full geometry traced) on an adversarial 48 m corridor with 6 point
lights (Linear falloff, range 8 m, spaced 8 m apart so adjacent falloff spheres only just touch and
distant lights do not reach — a near-worst case for far-bounce loss). For every valid probe it compared
the **cold** result (all 6 lights) against a **warm** result using only the group's bounded reaching-light
set (lights whose `falloff_range + cutoff` sphere overlaps the group AABB), sweeping cutoff ∈ {0, 2, 4,
8, 16, 32} m and group size ∈ {4³, 8³}.

**Committed error metric:** `max per-probe per-channel relative irradiance error, post-f16-encode` —
both warm and cold octahedral-tile irradiance values rounded through f16 first, then per RGB channel
`|warm − cold| / max(cold, FLOOR)`, maxed over a 14-direction lobe sample and over all probes. This is
the exact metric Task 8 gate (3) asserts.

Measured (group_dim 4³, the recommended size):

| cutoff | max rel err (raw) | abs err at that probe | cold irr there | **max-abs err** | max rel err where cold ≥ 0.02 | mean rel err | single-light invalidation |
|---|---|---|---|---|---|---|---|
| 0 m | 1.000 | 0.0047 | 0.0047 | 0.0442 | 0.411 | 0.210 | 13/52 = 0.25 |
| 4 m | 0.895 | 0.0035 | 0.0039 | 0.0127 | 0.218 | 0.111 | 17/52 = 0.33 |
| 8 m | 0.513 | 0.0025 | 0.0049 | 0.0065 | 0.120 | 0.070 | 21/52 = 0.40 |
| **16 m** | **0.412** | 0.0020 | 0.0048 | **0.0038** | **0.074** | 0.025 | 29/52 = 0.56 |
| 32 m | 0.070 | 0.0003 | 0.0050 | 0.0005 | 0.010 | 0.0001 | 45/52 = 0.87 |

**Key reading of the data.** The *raw* max relative error is high (0.4–1.0) at every practical cutoff,
but it is **dominated by near-black probes**: at cutoff 16 m the single worst offender has cold
irradiance 0.0048 (effectively black) and an absolute error of 0.0020 — an invisible defect reported as
41% relative. The honest signal is the **max absolute error** (0.0038 at 16 m, falling with cutoff) and
the **relative error restricted to probes that actually carry visible indirect light** (cold ≥ 0.02,
~2% of a unit-albedo bounce): **0.074 at cutoff 16 m, 0.12 at 8 m**. This corridor is adversarial; real
rooms have far more light-sphere overlap, so visible-probe error at a given cutoff is strictly lower in
practice.

**Chosen cutoff: `falloff_range + 16 m` dilation** (i.e. a per-group reaching-light set = lights whose
`falloff_range + 16.0` AABB overlaps the group). 16 m is ~1–2× a typical mood-light range (campaign-test
brightnesses 150–500 translate to ranges on that order) and is where, even in the adversarial corridor,
visible-probe relative error sits at ~0.07 and absolute error at ~0.004 — comfortably below perceptual
threshold for a low-frequency bounce channel, while invalidation (~56% of groups for a flood light) is
still meaningfully sub-whole-map.

**Committed warm-SH tolerance constant (the Task 8 gate (3) assertion):**

```
WARM_SH_MAX_REL_IRRADIANCE_ERROR = 0.15   // max per-probe per-channel relative irradiance
                                          // error, post-f16-encode, evaluated only at probes
                                          // whose cold per-channel irradiance ≥ 0.02 (the
                                          // visibility floor); probes below the floor are
                                          // exempt (their absolute error is imperceptible).
```

The floored form is **load-bearing**: without the `cold ≥ 0.02` visibility floor the raw metric is
0.4–1.0 at every usable cutoff (near-black probes), and no finite cutoff would pass — the gate would be
meaningless. With the floor, cutoff `+16 m` clears 0.15 with margin (measured 0.074 worst-case
adversarial; lower in real rooms). Task 8 must implement the metric with this floor and assert ≤ 0.15.

**Realized single-light SH-group invalidation fraction (measured):** at cutoff `+16 m`, group_dim 4³,
a single point/spot light invalidates **~56%** of groups in the adversarial corridor (29/52). In a real
map a light only reaches groups near it, so this is an upper bound for a light whose dilated reach spans
much of a small fixture; on a larger map the fraction falls (the dilated sphere covers a smaller share
of total groups). A directional light invalidates **100%** of groups by design (world-AABB reach) — and
that is correct and unavoidable.

### 3. Group size

**Chosen: 4³ probes per group** (64 probes, ~18.9 KB/entry). Rationale and sensitivity (measured):

- **Entry count:** 4³ → ~3,032 groups (campaign), ~900 (occlusion) — well within flat-file range.
  Doubling to 8³ cuts entry count ~8× (~380 / ~115 groups, ~151 KB each); halving to 2³ would multiply
  it ~8× (~24,000 groups) — still flat-file-viable but approaching the regime where per-entry `sync_all`
  and inode pressure start to matter, with diminishing locality gains.
- **Invalidation granularity:** finer groups localize a light edit better. At cutoff `+16 m`, 4³ gave
  29/52 = 0.56 invalidation vs 8³'s 4/7 = 0.57 — comparable *fraction*, but 4³ re-bakes 64-probe units
  vs 8³'s 512-probe units, so the *absolute work* saved per edit is finer at 4³.
- **Alignment bonus:** 4³ equals the existing `AFFINITY_FACTOR = 4` affinity-cell decomposition
  (`affinity_grid.rs:35`), so the SH-group partition can reuse the affinity-grid machinery and probe-block
  geometry the animated/delta path already computes — no new spatial-partition code, and the group AABBs
  line up with structures the codebase already trusts.

4³ is the balance point: small enough that a light edit re-bakes a tight set of probe blocks, large
enough to keep entry count in the low thousands, and free-aligned to existing affinity cells.

### Go/no-go RECOMMENDATION for the SH half: **GO (per-group SH)**

The substrate holds (flat-file, ~3,000 modest entries — no re-plan trigger), and a defensible cutoff
exists: `falloff_range + 16 m` dilation keeps warm-vs-cold error within the committed tolerance
(`WARM_SH_MAX_REL_IRRADIANCE_ERROR = 0.15`, floored at cold irradiance 0.02) while a single point/spot
light invalidates only a bounded sub-whole-map share of groups (~56% in an adversarial small corridor,
less on real maps). The one honesty caveat the owner must accept: the *raw* unfloored max-relative metric
is large at every usable cutoff because near-black probes dominate it, so the gate **must** use the
visibility-floored form — without the floor there is no go. With the floor, the absolute error at the
recommended cutoff is ~0.004 (adversarial worst case) on a low-frequency channel that the runtime samples
trilinearly, which is below perceptual relevance; and the per-light *direct* lightmap channel — the
dominant visual term — stays exact and instant regardless. The cold `--no-cache` build remains the exact
ship source of truth. Recommend proceeding with Task 6 as specced, with cutoff dilation 16 m, group size
4³, and the floored tolerance gate.

### Gate 3 follow-up — the metric was wrong, not the cutoff (corrects the committed value above)

When Task 8's `warm_sh_within_tolerance_on_fixtures` first ran the floored metric against **real** fixtures
(not the spike's tiny synthetic corridor), the committed `max` form failed: campaign-test reported
`max = 0.356` vs the 0.15 tolerance. Diagnosing the full distribution (1,234,998 floored samples) showed
this is a **`max`-metric artifact, not a quality problem**:

| stat | campaign-test | occlusion-test | small fixtures |
|---|---|---|---|
| mean | 0.0019 | 0.0007 | 0.000 |
| p99 | 0.043 | 0.013 | 0.000 |
| p99.9 | **0.090** | 0.029 | 0.000 |
| p99.99 | 0.133 | 0.045 | — |
| max | **0.356** | 0.065 | 0.000 |
| samples > 0.15 | 80 (0.0065%) | 0 | 0 |
| samples > 0.35 | 1 | 0 | 0 |

The channel is overwhelmingly faithful (mean 0.19%, p99.9 = 9%) and strictly dimmer-or-equal; the 0.356
was a **single floor-boundary probe** out of 1.23M (one barely above the 0.02 floor that lost most of its
light to the cutoff, so a tiny absolute miss reads as a large relative one). The spike's `max` measured
clean only because it ran over a handful of probes — `max` over millions catches the rare outlier.

**Resolution (owner-approved "diagnose + fix metric"):** the gate metric is changed from `max` to the
**99.9th percentile** of the floored relative error, and the constant renamed
`WARM_SH_MAX_REL_IRRADIANCE_ERROR` → **`WARM_SH_P999_REL_IRRADIANCE_ERROR`**, value kept at **0.15**
(~1.7× headroom over the observed p99.9 of 0.090). The **cutoff (16 m), group size (4³), and all bake
behavior are unchanged** — only how the gate *judges* the (already-correct, benign-underestimate) warm
output. p99.9 bounds the body of the distribution and is robust to the floor-boundary outlier the `max`
form over-weighted. Gate now passes on every fixture (campaign 0.090, occlusion 0.029, small ≤ 0.000).

### Follow-up — warm-build group bake is serial (parallelism regression)

`bake_sh_volume_grouped` (`sh_group.rs`) bakes groups in a serial `for group in &groups` loop
(`bake_group` itself uses `.iter().map()`), while the monolithic `bake_sh_volume` bakes probes with
`into_par_iter()`. So a cold-cache warm build #1 (every group a miss) runs materially slower than a
`--no-cache` build of the same map — measured on occlusion-test: warm#1 **677 s** vs cold **228 s**
(~3×). The deterministic fix is **rayon-over-groups**: each group's per-probe soft-visibility seed is
its global probe index and there is no cross-group state, so a parallel group bake is byte-identical to
the serial one. The only shared resource is the `StageCache` get/put — partition it per group or
collect bakes then write. Tracked as a `// follow-up:` comment at the group loop.

### Gate 3 follow-up (2) — global-index soft-visibility seed restores strict dimmer-or-equal

The §2 "ordered-light-set caveat" (the soft-visibility seed mixing the light's *bake-slice* position)
had a sharper consequence than just byte-identity: because the warm grouped path passes the **bounded**
reaching set, a kept light sitting after a *dropped* light got a different sample-lattice rotation than
in the cold (full-set) bake — so warm could come out slightly **brighter** in spots, contradicting the
plan's strict "dimmer-or-equal, never brighter" claim. **Resolved** by threading each light's **global
`static_lights` index** into `soft_visibility_seed` (via `sample_radiance_rgb`'s `light_global_indices`,
plumbed through `bake_probe`/`bake_group`): a kept light now gets the SAME lattice rotation whether the
bake sees the full set or the bounded set. The monolithic/cold path passes `None` (slice position ==
global index), so its bytes are unchanged — `full_light_set_grouped_equals_monolithic` and the cold
byte-identity gate still pass. The warm-SH p99.9 agreement tightens, and the dimmer-or-equal contract is
now literally true.

## Key files

- Substrate: `crates/level-compiler/src/cache.rs`, `main.rs` (cache wiring ~`:254-446`)
- SH: `crates/level-compiler/src/sh_bake.rs`, `affinity_grid.rs`, `delta_sh_bake.rs`; `crates/level-format/src/sh_volume.rs`, `octahedral.rs`
- Lightmap: `crates/level-compiler/src/lightmap_bake.rs`, `chart_raster.rs`
- Shared: `bvh_build.rs`, `geometry.rs`, `map_data.rs`
- Docs: `context/lib/build_pipeline.md` (cache §, determinism invariant `:186`), `context/plans/done/build-stage-cache/index.md`
