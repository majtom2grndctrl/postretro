# Large-Map Spatial Residency

> **Status:** Epic seed. SH is the first resource and ships through stages 1–4 below
> (`context/plans/done/sh-probe-streaming/`). Generalizing to further resources is
> unplanned. Not ready for `/build-spec`; a planning session turns stage 5 into scoped
> specs after the listed measurements exist.
> **Supporting research:** `context/research/spatial-streaming.md`. Shipped SH residency:
> `context/lib/rendering_pipeline.md` §Cluster SH residency.

## Outcome

One authored level remains one logical PRL. Large levels can retain only the
nearby baked spatial resources in memory, while portals and designed seams hide
loading. Map scale is bounded by disk and residency budget rather than a
whole-level GPU upload.

## Scope boundary

This seed does not change renderer ownership of GPU resources. Compression and
residency solve different problems: compression (adaptive SH coarsening, BC5 shadowmask
at rest) shrinks a resident resource; residency selects which parts stay resident. Both
remain useful, and compression lands first where it is ready, because it changes the
byte counts residency is designed around.

## Existing substrate

- BSP leaves are runtime `cell_id`s; portal traversal produces visible cells.
- Cluster directory (id 49) partitions every runtime cell into exactly one cluster by a
  deterministic greedy rule bounded by primitive and cell counts. The loader re-derives
  the partition and rejects a mismatch. Per-resource ranges, owner halos, and authored
  hints (pins, seam warm-up, priority) live there.
- Cluster SH payloads (id 50) stream through renderer-owned pools: warm-set prefetch from
  the camera cell, one ordered read issuer, a decode pool, a decoded-byte install budget
  per drain, journaled install, and generation retirement. Billboard scatter (ids 47/48)
  and every non-SH section still load and upload whole.
- Always-on streaming counters feed the log, the dev-tools Streaming tab, and capture.
- Regional BVH is a culling-layout idea, not the residency authority. Its old plan was
  archived; only the cell-clustering lesson carries forward.

## Staged architecture

1. Compiler-only clustering and deterministic cluster directory. **Shipped.**
2. Prove the directory without eviction; keep the global bake view SH/SDF baking needs.
   **Shipped.**
3. Visible-cell-driven prefetch, hysteresis, and one resource's residency. **Shipped for
   SH.** Prefetch follows the camera cell, not the view direction.
4. Disk and decode off the frame path; renderer installs and retires cluster generations
   under a budget. **Shipped for SH.**
5. Generalize the same cluster state to lightmap-shaped layers, geometry/BVH views, SDF,
   fog, and acoustics. Lightmap-shaped data is now the largest whole-resident class (see
   measurements), so it is the likely next resource.

## Lessons from SH residency

- **Most clusters are empty.** Solid leaves each become a one-cell cluster; exterior
  leaves group into a few clusters. About 90% of clusters carry no data, and every byte
  lives in playable clusters (26 of 482 on `stress-warren-mini`, 22 of 181 on
  `campaign-test`). The runtime never reaches them. Measured cost is negligible: under a
  second of bake, a few milliseconds of load-time validation, microseconds per frame, no
  GPU. Excluding them needs a directory version bump; bundle it with one, never alone.
- **Playable clusters are coarse and uneven.** Median extent is 26–33 m. Several SH
  clusters exceed the per-drain install budget alone (up to 19 MiB), and the budget admits
  one cluster regardless, so a single cluster sets the worst install. The partition bounds
  primitives and cells, never bytes. Each added resource raises bytes per cluster.
- **Warm-set reach is a function of cluster size.** A fixed count of eight clusters covers
  roughly a third of either test map. Finer clusters would shrink that reach.
- **Bounds are policy, owned once.** The drain boundary kept a retired per-count cap
  after the budget changed and failed the first drain on small-cluster maps. Boundaries
  validate identity and structure; the controller alone owns how much work a drain carries.
- **Ordered I/O holds on SATA.** One issuer in file-offset order with coalescing ran clean
  on a SATA SSD during owner walks.
- **Development hardware has no GPU timing.** Evidence must be bytes, counts, and CPU time.
  The dev panel reports no resident bytes per resource, draws, or CPU frame time.

## Constraints for planning

- Portal-adjacent clusters must prefetch and retain enough data to avoid visible
  geometry or lighting holes. SH halo ownership is settled; each new resource defines its
  own boundary ownership.
- Cross-sector lights must preserve no-double-counting; lights cannot be
  assumed local to one cluster.
- A frame sees generation-matched dependent resources, never a mixed partial
  install. Each resource defines a conservative miss fallback; SH's is the ambient floor.
- I/O and CPU preparation stay outside Input → Game logic → Audio → Render →
  Present. Renderer owns GPU upload, synchronization, and retirement.
- Co-op admission preserves one logical level/content identity. Residency is
  local resource state, not divergent gameplay/collision/visibility content.
- Lightmap charts pack by cell (next-fit; a cell never spans layers), but cell-id order
  scatters a cluster's cells across layers. Layer size follows the largest cell, so one
  huge cell can force a single 4096² layer that residency cannot split. Cluster residency
  of lightmap-shaped data needs a packer that orders by cluster and caps the layer size.
- Lightmap (id 22) and shadowmask (id 42) share the fragment's layer index. They are
  resident together or not at all.
- The forward fragment stage has no free binding. Layer indirection folds into the
  existing binding-7 uniform, never a new fragment binding. The animated atlas resolves
  per-face blocks there in the fragment stage; static-layer indirection may use the
  vertex stage.

## Decisions still open

Leanings come from a read-only dry run (research below). None is decided.

- Byte-aware partition bounds, now that a second resource shares each cluster. Lean:
  bound by pre-pack chart texel area, and have the packer consume the partition. The
  partition would then run before the lightmap bake.
- First lightmap-shaped work. Lean, in order:
  1. Compact the animated atlas (done: `done/animated-lightmap-compact-atlas`). Per-face
     blocks resolved in the fragment stage from the binding-7 table replace the pooled
     animated-slot idea; pages are the load-and-evict unit for stage 5. No I/O, and it covers the largest byte class. A virtual-layer
     table for static layers remains open.
  2. Ids 22 and 42 as one layer-keyed unit through the issuer. Existing layer-major
     payloads give per-layer file ranges, so there is no new section.
  3. Id 25 weight maps by chunk range.
- Packing granularity. Earlier lean: soft, cluster-ordered packing with a capped layer
  size. Measured hard per-cluster boundaries cost 2.1–5.5× the texels. The per-cell
  mandatory dry run (below) tests it. Under cluster closure on the hallway map,
  cluster-ordered 2048² beats today's packing at every bounded D, but only by 7–33%.
  Cluster-ordered 1024² gives the lowest layer bytes at 64 and 128 m, but needs 344
  layers against the 256-layer runtime limit. At 64 m, whole-layer residency's worst cell
  needs 2.6–5.3× the texel-exact worst, depending on layout and granularity. Lean now:
  residency finer than a whole layer, such as pages or a virtual-layer table, matters
  more than a packer change alone.
- Reach bound. The portal-path distance whose cells are mandatory. It is the design's
  largest lever. On the hallway map under cluster closure, Low fits at 64 m but not at
  128 m at full resolution, even texel-exact. Open for the owner.
- Miss policy. Lean:
  - Visible-cluster layers are mandatory. The pool grows rather than refusing them.
  - A transient miss drops static direct light and keeps SH indirect.
  - A low-resolution fallback tile is still a candidate. It needs a seam prototype.
- Ownership. Texels belong to their receiver cell, so partitioning receivers cannot
  double-count a light. The shadowmask channel table and the animation descriptors stay
  global.
- Residency budgets. See the next section.
- Whether the next directory version drops solid and exterior cells from clustering. The
  lightmap layer-range table can ride the same version bump.

## Residency budget tiers

Goal: PostRetro runs impressively on laptop GPUs, as classic DOOM runs on a potato. One
player-facing tier covers every streamed and whole-resident resource, not SH alone. The
bullets below are leanings, not decisions.

- **Today.** SH alone has a budget: a hardcoded 256 MiB floor sized to a 6 GB GTX 1660,
  held twice (`DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` in `plan_initial_pool_floor`,
  `DEFAULT_GPU_FLOOR_BYTES` in `ShResidencyAccounting`). Pools reserve the smaller of the
  whole map and their share at install.
- **Shape.** Lean: Low, Medium, High, and an auto default. One planner splits the tier into
  per-resource GPU caps and per-drain budgets, replacing both constants. Resource-neutral
  settings key. Options slot, menu control ("Applies after reload") and SDK type follow
  `ShadowQuality`. Applies at the next level install, as `configure_player_shadow_quality`
  does.
- **Auto default.** wgpu 29 reports no portable VRAM size or budget. `AdapterInfo` has
  name, vendor and device ids, device type, driver and backend. `Limits` are capability
  caps. `MemoryHints` only tunes allocator block sizes. `MemoryBudgetThresholds` makes D3D12
  and Vulkan fail allocations at a percent of the native budget but never returns it.
  `Device::generate_allocator_report` sums our own allocations, on D3D12 and Vulkan only.
  Candidates:
  - Fixed Medium; players step down. Portable, no guessing.
  - Device type. Metal reports `IntegratedGpu` for every unified-memory Mac, so Apple
    Silicon lands on Low without a Metal exception.
  - A vendor and device-id table. Catches known laptop parts; needs upkeep.
  - Native queries (DXGI, `VK_EXT_memory_budget`, Metal `recommendedMaxWorkingSetSize`)
    through raw backend handles. Three backend paths, and `unsafe` needs approval.
- **Owner decisions (2026-09-28).**
  - Target laptops running other apps.
  - A player-facing quality tier sets the budget. Low is 256 MiB for lightmap-shaped
    data, and higher tiers are opt-in for hardware that can hold them.
  - No pop-in on interior maps, including large arenas. A tier is valid for a map only
    when every camera cell's mandatory set fits it, so a miss fallback cannot stand in
    for residency.
  - Draw distance is limited: shorter than open-world games, longer than classic Quake.
  - For scale, streamed SH has rarely exceeded about 40 MB in owner walks on Windows.
- **Mandatory overshoot.** Visible, pinned and owner-closure data is never evicted, so a
  tier below a map's mandatory set is mostly overshoot. On `stress-warren-mini` the owner
  closure covers most clusters. For ids 22 and 42, offline mandatory bytes per camera
  cell are now measured (see Pre-planning measurements). The runtime figures are still
  unmeasured.
- **Whole-resident resources.** They count against the tier but cannot yield. Until stage
  5, a tier shrinks only the SH pools. On `campaign-test`, lightmap-shaped data alone is
  about 235 MiB, near the whole SH floor. A pooled animated atlas, stage 5's first step, is
  the first change that lets a low tier bite there.
- **Hardware floor.** `rendering_pipeline.md` §10 sets discrete-GPU floors; only adaptive
  base-probe spacing names a laptop iGPU floor. Tiers extend that divergence; record it.
- **Open for the owner.** Tier byte values above Low, and whether they scale per map. The auto
  heuristic. Whether a tier below mandatory warns, clamps, or stays silent. Whether tiers
  also drive load-time quality cuts, or residency alone.

## Pre-planning measurements

Whole-resident lightmap-shaped GPU bytes, parsed from compiled PRLs on 2026-09-27. Ids 22
and 42 are single-mip, so GPU bytes equal disk bytes. The animated atlas is derived and
never on disk.

| Resource | `campaign-test` | `stress-warren-mini` | `stress-warren-hallway-inspection` (fresh, 2026-09-28) |
|---|---|---|---|
| Animated lightmap atlas (irradiance + direction) | 144 MiB (3 slots × 2048²) | 5.1 MiB | 144 MiB (3 slots × 2048²) |
| Animated light weight maps (id 25) | 35.4 MiB | 1.4 MiB | 35.4 MiB |
| Shadowmask atlas (id 42, BC5) | 32 MiB | ≈0.8 MiB (est.; file predates BC5) | 584 MiB (73 layers, 338 selected lights) |
| Lightmap (id 22, BC6H + direction) | 24 MiB (4 × 2048²) | 0.6 MiB (27 × 128²) | 438 MiB (73 × 2048²; 292 irradiance + 146 direction) |

For scale, streamed SH payloads (id 50) are 26 MiB on `campaign-test` and 246 MiB on the hallway map. On the hallway map, whole-resident lightmap-shaped data is about 1.2 GiB, and the static lightmap and shadowmask are over 1 GiB of it. Those two are where stage 5 pays off. The hallway map has 336 playable clusters of 3,385. Its largest cluster payload is 17 MiB, over twice the per-drain install budget.

The animated atlas allocates a full layer per slot, while its chunks cover about a fifth
of those texels. It is the largest lightmap-shaped consumer and needs no I/O to shrink.

Dry-run chart-to-cluster attribution on `campaign-test` (area estimated from UV bounds):
- Only a minority of clusters carry charts.
- No cell spans two layers.
- An 8-cluster warm set touches most layers on average and all of them at worst.
- Soft cluster-ordered packing at 1024² layers roughly halves the average touched share,
  for about 25% more texels.

Per-cell mandatory bytes, ids 22 and 42, on the hallway map (2026-09-28). The test is
`lightmap_residency_dry_run_from_prl`, an ignored `prl-build` test; its doc comment has the
run command.
- **Method.**
  - Mandatory is the camera's cluster, plus pinned clusters, plus cells within a
    portal-path distance D.
  - Cluster closure admits the whole cluster of each reached cell. That is the
    cluster-keyed design; the cell-granular figure is its lower bound.
  - All 2,082 non-solid, non-exterior cells are camera cells.
  - Self-checks: attribution matches each payload exactly, charts don't overlap, a
    stored-order repack reproduces every placement, and the untruncated distance
    recompute matches all 46,564 stored id 46 pairs.

Worst camera cell, in MiB, under cluster closure and the untruncated distance recompute.
Brackets count cells over Low's 256 MiB. Texel-exact counts the needed chart texels
alone. The layout columns count whole layers.

| D | Texel-exact | Half-res | Today's packing (2048²) | Cluster-ordered 1024² | Cluster-ordered 2048² |
|---|---|---|---|---|---|
| 16 m | 50 | 13 | 168 | 175 | 112 |
| 32 m | 62 | 16 | 224 | 200 | 168 |
| 64 m | 112 | 29 | 378 [261] | 287 [10] | 336 [41] |
| 128 m | 321 [144] | 84 | 770 [1,849] | 578 [1,487] | 714 [1,801] |
| Reach | 820 [2,081] | 215 | 1,022 [2,081] | 1,372 [2,081] | 1,036 [2,081] |

Findings:
- **Cell-granular sets are smaller.** They fit Low at 128 m even texel-exact: 227 MiB
  worst, 187 MiB p95.
- **Under cluster closure at full resolution, whole layers fit Low through 32 m.** At
  64 m only sub-layer residency fits, and at 128 m only half resolution fits.
- **Cluster-ordered 1024² exceeds the runtime limit.** It needs 344 layers plus 12
  oversize cells, 1.34× the texels. Cluster-ordered 2048² needs 74 layers, 1.01×.
- **Id 46 caps each source cell at its 32 nearest partners**
  (`CELL_VISIBILITY_FANOUT_K`).
  - A cell's stored set counts pairs kept from either end, so at 128 m it averages 42
    cells, against 305 recomputed.
  - The cap already binds at 16 m, where 178 cells hit it, and the stored and recomputed
    sets diverge from 32 m.
  - The error is large. At 128 m, stored pairs would put today's packing at 280 MiB,
    with 8 cells over; the recompute gives 770 MiB, with 1,849 over.
  - Any consumer that reads id 46 as "everything within D" undercounts. The SH warm set's
    use of id 46 is unchecked.
- **`campaign-test` never nears the limit.** Its whole lightmap plus shadowmask is 56 MiB.

Still needed:

- Resident and mandatory bytes per GPU resource, CPU frame time, and draw counts in the
  dev panel, before any stage 5 plan or tier value is judged.
- A rebuild of `stress-warren-mini`, whose PRL predates BC5.
- A production-shaped map beyond the Stress Warren family and `campaign-test`.
- Seam and miss prototypes for the first lightmap-shaped resource, including
  cross-sector lights.
