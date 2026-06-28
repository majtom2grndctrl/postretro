# Spatial Streaming & Region-Weighted Baked Data — Design Exploration

**Date investigated:** 2026-06-28
**Status:** Pre-spec exploration. Not yet a draft plan. Captures the long-term
target — likely a whole roadmap epic — so smaller sprints (the multi-layer
lightmap atlas, future cell-clustering work) keep the right seams open and don't
commit to the wrong spatial substrate.

> **Read this when:** scoping any *residency* / *streaming* work for baked data
> (lightmaps, SH irradiance, world geometry/BVH, SDF occluders, reflection
> probes, fog, acoustics), or any feature that wants to load/evict baked content
> by area — and when deciding whether a new baked section should be spatially
> partitionable.
> **Key invariant:** there should be **one** spatial residency substrate that
> every baked subsystem subscribes to, and it should be **cells** (clustered),
> reusing the visibility signal the renderer already computes — not a parallel
> per-subsystem spatial query, and not a separate "regional BVH" structure.
> **Related:** `context/lib/build_pipeline.md` (cells, baked sections) ·
> `context/lib/rendering_pipeline.md` (portal visibility, visible-cell cull) ·
> `context/plans/in-progress/lightmap-array-atlas/` (multi-layer atlas — the
> first baked section nudged toward cell-clusterable residency) ·
> `context/plans/done/perf-per-region-bvh/` &
> `context/plans/done/perf-baked-visibility-region-masks/` (archived) ·
> `context/plans/done/perf-visible-cell-candidate-cull/`

---

## 1. Vision

Today the engine loads the entire compiled `.prl` and uploads every baked
section whole — one global lightmap atlas, one whole SH irradiance volume, the
full geometry + BVH, all at level load. That caps map size on VRAM and load
time. The long-term target is **spatial streaming**: keep resident only the
baked data near (or about to be near) the camera, and load/evict the rest as the
player moves — so map size scales with disk, not with VRAM.

The aesthetic ethos matters here: monster closets, scripted reveals, and
theatrical set-pieces are first-class. That means **streaming seams are a
gameplay/pacing decision as much as a technical one** — loads should hide behind
dramatically-chosen thresholds (a slow door, an elevator, a corridor), not pop
in the player's peripheral vision.

This note argues for the **substrate** (cells), the **granularity** (clustered
cells), and the **authoring model** (algorithm-default, author-weighted hints) —
so the eventual epic can be decomposed into independently-shippable slices.

---

## 2. What is streamable

Almost all *baked spatial* data shares one property — it's indexed by position,
so it can be made resident per-area. Candidates, roughly in payoff order:

| Data | Section | Why it streams | Notes |
|------|---------|----------------|-------|
| **SH irradiance volume** | `OctahedralShVolume` (id 34) | Probe grid is inherently spatial; only probes near the camera are sampled | Currently the heaviest baked artifact *and* the slowest bake stage — top target |
| **World geometry + BVH** | geometry + BVH arrays | Classic world-streaming: load/evict the vertex/index/BVH data for nearby cells | Biggest VRAM + load-time win on large maps |
| **Lightmap atlas layers** | `Lightmap` (id 22) | The multi-layer array atlas already carries a per-vertex layer index; cell-aligned layers are load/evictable as layer ranges | See §6 — this branch already half-set-up the structure |
| **SDF static-occluder atlas** | `SdfAtlas` | Spatial voxel/occluder data | Same shape of problem |
| **Reflection probes** | baked cubemaps | Stream by proximity | |
| **Fog volumes / acoustic-reverb zones** | fog + acoustic sections | Spatial by construction | Small, low priority |

**Off-axis (keep separate):** texture/material residency streams by *material +
mip*, not by region. Don't fold it into the spatial substrate — it has a
different key and a different eviction policy.

**Key observation:** every spatial item above wants the *same* answer to
"what's near the camera." So the design goal is one residency signal that all of
them subscribe to — not bespoke streaming logic per subsystem.

---

## 3. Substrate: cells (clustered), not "regional BVH"

The term "regional BVH" conflated two different ideas; for streaming, only one
matters.

1. A *BVH-structure* optimization (per-region sub-BVHs). This was a **culling**
   idea — `perf-per-region-bvh` and `perf-baked-visibility-region-masks` were
   both **archived**, superseded by `perf-visible-cell-candidate-cull`.
   Streaming does **not** need it. **Retire this framing for streaming.**
2. A *spatial residency unit*. Cells **already are** this.

### Why cells are the right substrate (grounded)

- **They already exist.** A cell is one BSP empty-leaf; the compiler's BSP leaf
  index is preserved one-to-one as the runtime `cell_id`, and every baked
  primitive already carries it. (`crates/level-compiler/src/geometry.rs` —
  "compiler BSP leaf id is preserved as the runtime cell id"; `bvh_build.rs` —
  `cell_id: face.leaf_index`.)
- **They are already the visibility unit.** Portal traversal is the sole
  visibility path (`index.md` §2), and the runtime already computes a **per-frame
  visible-cell bitmask** to drive culling. Streaming wants almost exactly that
  signal — "what's potentially visible / about to be" — so residency can **ride
  the cull system that already exists** instead of inventing a parallel spatial
  query. This is the single biggest reason to pick cells.
- **One substrate, all subsystems.** Key residency on cells and lightmap layers,
  SH probes, geometry, SDF, probes, and fog all stream on the same trigger. A
  "regional BVH" would not give this unification.

### Granularity caveat: cluster cells

A cell = one BSP leaf is almost certainly **too fine** for I/O chunks (thousands
of tiny cells → tiny reads, per-cell bookkeeping overhead). The streaming unit
should be a **cluster of adjacent cells** up to a primitive/byte budget. This
*cell clustering* pass is exactly the piece both archived region plans punted on
("cluster adjacent cells into groups of ~1k primitives" — listed as future scope
in `perf-per-region-bvh`). It is the foundational sub-feature of any streaming
epic.

---

## 4. Authoring: algorithm-default, author-weighted

Don't make authors hand-partition the map (tedious, fragile; an algorithm does
balanced clustering better). Don't make it purely algorithmic either — that
misses what only the author knows. **Hybrid:**

- **Algorithm owns the default.** Cluster cells into balanced streaming groups
  automatically. Zero authoring still yields a sane baseline.
- **Authors weight/override at the seams the algorithm can't see:**
  - **Streaming seams** — mark which portals/doors are load-hide points. This is
    a *pacing* decision: hide the load behind the elevator, the slow door, the
    corridor. Fits the theatrical set-piece ethos directly.
  - **Priority / budget** — tag a hero area "keep resident / high budget," a back
    hallway "low."
  - **Always-resident** — a hub or skybox that should never evict.

**Shape of "authored regions," if they return:** express them as **streaming
hints layered over cell clusters** (cluster seeds, priority tags,
always-resident flags, portal seam markers) — *not* as a separate spatial
structure and *not* as a hand-drawn partition. The algorithm guarantees the
baseline; the hints buy theatrical control where it matters. (No author-region
FGD entity exists today; regions are compiler-derived from BSP — so this is net-new
authoring surface, and it should be additive hints, not a mandatory partition.)

---

## 5. Region-weighted lightmap atlas splitting (the cheap middle slice)

Separable from streaming, and the slice that most directly leverages the
multi-layer atlas this branch shipped. The packer (`pack_layers` in
`lightmap_bake.rs`) is **already a group-by-spatial-key multi-bin packer** — it
groups charts by BSP leaf (`group_charts_by_leaf`), keeps each leaf cohesive on
one layer, and opens new layers on overflow. "Region-weighting" is mostly:

- change/coarsen the grouping key from leaf → `(region/cluster, leaf)`; and
- decide **hard vs soft** boundaries:
  - *Hard:* a region owns a contiguous block of layers → enables per-region
    residency and budgets, but **costs packing density** (more partial layers →
    more VRAM) and eats the `MAX_ATLAS_LAYERS = 256` / device
    `max_texture_array_layers` budget faster.
  - *Soft:* the packer merely *prefers* to co-locate a region's leaves on few
    layers → better locality, keeps density high, no hard guarantees.

**Resolution correction worth recording:** a `texture_2d_array` has a **single
per-layer dimension shared by all layers**, so "higher-res for this region"
*cannot* mean different layer sizes. Per-region resolution must come from
**texel-area budget** (more/denser layers for that region) or **per-chart
density coarsening** — i.e. the deferred *Strategy C (Tier-1 per-chart density
coarsening)* from the lightmap-array-atlas plan. So per-region resolution ≈
regional density + Strategy C, not per-layer dimensions.

---

## 6. Why the multi-layer lightmap work already points here

The `lightmap-array-atlas` plan made the lightmap atlas a `texture_2d_array` with
a **flat, arbitrary per-vertex `lightmap_layer` index**. Two consequences for
streaming:

- More / region-aligned layers cost **nothing** at runtime (the index is
  per-vertex flat; no per-face layer switch), up to the layer cap.
- Cell-aligned layers would be the **natural residency granularity** for lightmap
  streaming — load/evict a cluster's layer range. The structure is already
  there; what's missing is a per-cluster → layer-range map in the PRL and a
  loader that uploads/evicts layer ranges instead of the whole atlas.

That a *different* feature (the atlas) independently landed on cells/per-vertex
layers as the right axis is a good tell that cells are the unifying substrate.

---

## 7. Current reality (what exists vs. what's missing)

**Implemented:**
- Cells (= BSP leaves = `cell_id`), one-to-one with drawable BVH leaves
  (per material bucket). Portal traversal + per-frame visible-cell bitmask.
- Every baked primitive carries `cell_id` / `leaf_index`; the lightmap packer
  already groups by it.

**Absent / loaded whole today:**
- No per-region/per-cell residency. `prl_loader` reads the whole `.prl` and
  uploads every section (lightmap, SH, geometry/BVH) whole at load.
- No cell *clustering* pass (cells are raw BSP leaves).
- No author-specified region entity (regions are compiler-derived).
- No per-region → layer-range / data-range map in the PRL.

**Archived (don't resurrect as-is):**
- `perf-per-region-bvh`, `perf-baked-visibility-region-masks` — culling-structure
  ideas, superseded. Mine them for the *clustering* sub-idea, not the structure.

**Deferred (a dependency for §5's resolution story):**
- Per-chart density coarsening (Strategy C) from the lightmap-array-atlas plan.

---

## 8. Suggested epic decomposition (independently shippable)

1. **Cell clustering** — the foundational pass: group cells into balanced
   streaming groups with a primitive/byte budget. Pure compiler-side; nothing
   streams yet, but it's the substrate everything else keys on.
2. **Region-keyed lightmap packing (+ optional per-region density via Strategy
   C)** — the cheap, high-value middle slice from §5. Leverages the array atlas
   directly; soft-boundary first.
3. **Authored streaming hints** — additive FGD/volume surface over clusters:
   seams (portal/door load-hide points), priority/budget, always-resident.
4. **Per-cluster residency for one subsystem (SH volume or geometry first)** —
   the first actual load/evict, riding the visible-cell signal. SH is the
   heaviest artifact; geometry is the biggest VRAM win. Prove the loader +
   PRL-layout changes on one before generalizing.
5. **Generalize residency** to the remaining spatial sections (lightmap layer
   ranges, SDF, probes, fog) on the same trigger.

Slices 1–2 are compiler-only and low-risk; 3 is authoring surface; 4–5 are the
cross-cutting runtime lift (loader, PRL section layout, partial GPU uploads).

---

## 9. Open questions / seams to keep open

- **Eviction policy & hysteresis** — visible-cell set churns per frame; residency
  must not thrash on a doorway. Needs a "potentially-visible-soon" set wider than
  the strict per-frame visible set, plus budget-based LRU.
- **Load latency vs. pop** — async load on a background thread (the event loop
  must never block; `development_guide.md` §4.2). What's the fallback while a
  cluster loads — placeholder lightmap/SH, or a hard gate at the seam?
- **PRL layout** — sections become per-cluster-addressable (offset/length table
  per cluster per section). Affects the `.prl` format and the loader.
- **Bake-time cost** — clustering and per-cluster section emission add compiler
  work; weigh against the warm/cold cache contract.
- **Cache-on-disk size** — orthogonal but related: stress-warren-scale caches
  already strain disk; cache compression (no compression today; single choke
  point at `cache.rs` `write_entry`/`get`; `flate2` already in-tree, `zstd` would
  compress float data better) is a cheap parallel win and may belong in the same
  epic's hygiene track.
