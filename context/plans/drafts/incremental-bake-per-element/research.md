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

## Key files

- Substrate: `crates/level-compiler/src/cache.rs`, `main.rs` (cache wiring ~`:254-446`)
- SH: `crates/level-compiler/src/sh_bake.rs`, `affinity_grid.rs`, `delta_sh_bake.rs`; `crates/level-format/src/sh_volume.rs`, `octahedral.rs`
- Lightmap: `crates/level-compiler/src/lightmap_bake.rs`, `chart_raster.rs`
- Shared: `bvh_build.rs`, `geometry.rs`, `map_data.rs`
- Docs: `context/lib/build_pipeline.md` (cache §, determinism invariant `:186`), `context/plans/done/build-stage-cache/index.md`
