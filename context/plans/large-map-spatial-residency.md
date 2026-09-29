# Large-Map Spatial Residency

> **Status:** Epic seed. SH is the first resource and ships through stages 1–4 below
> (`context/plans/done/sh-probe-streaming/`). Generalizing to further resources is
> unplanned. Stage 5 (lightmap-shaped data first) is the resumable problem brief
> `ready/spatial-residency--lightmap-cell-blocks/`, covering the full path: compiler cell blocks, a baked residency set, runtime pool and
> remap, streaming through the issuer, and a dev-panel meter. Its budget is a debug-tool
> pool-cap slider. The player-facing tier is a later spec: `experimental_spikes.md`
> forbids user-facing settings in a spike. Not ready for `/build-spec`.
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
- Lightmap charts pack by cell today (next-fit; a cell never spans layers), but cell-id
  order scatters a cluster's cells across layers, and layer size follows the largest cell.
  Whole layers are therefore the wrong residency unit. The unit is the cell block (see
  Packing granularity).
- Lightmap (id 22) and shadowmask (id 42) share the fragment's layer index and nothing
  else. The shadowmask is twice the lightmap width (two mask groups side by side,
  `sample_shadowmask_atlas`). They are resident together or not at all.
- The forward fragment stage has no free binding. Indirection never adds a fragment
  binding. The animated atlas resolves per-face blocks in the fragment stage from the
  binding-7 uniform. Static cell blocks resolve in the vertex stage from a table in free
  bind group 6.

## Decisions still open

Reach bound and packing granularity are decided (owner, 2026-09-28). The rest are
leanings from a read-only dry run (research below).

- Byte-aware partition bounds, now that a second resource shares each cluster. Lean:
  bound by pre-pack chart texel area, and have the packer consume the partition. The
  partition would then run before the lightmap bake.
- First lightmap-shaped work. Lean, in order:
  1. Compact the animated atlas (done: `done/animated-lightmap-compact-atlas`). Per-face
     blocks resolved in the fragment stage from the binding-7 table replace the pooled
     animated-slot idea; pages are the load-and-evict unit for stage 5. No I/O, and it covers the largest byte class. A virtual-layer
     table for static layers remains open.
  2. Ids 22 and 42 as one block-keyed unit through the issuer. The compiler's cell-block
     layout gives per-block file ranges.
  3. Id 25 weight maps by chunk range.
- **Packing granularity. Decided: cell blocks.**
  - Each cell's charts pack into one BC-aligned block. The runtime pool of 2048² layers
    allocates blocks with a freeing allocator. A dense block id rides in the vertex's
    `lightmap_layer` u16, and the vertex stage resolves it to (layer, offset) from a table
    in free bind group 6. UVs become block-local.
  - Rejected: whole layers (over Low from L = 16 m, and over it at L = 0 for
    cluster-ordered layouts); fixed P² tiles (92% of hallway texels are in charts larger
    than 128², so they'd need chart splitting); `first_instance` for cell identity (the
    device feature isn't requested, and DX12 reads it as 0, gfx-rs/wgpu#2471).
  - Half resolution is dropped from stage 5. Full-res cell blocks fit Low with room to
    spare. It stays a later lever for maps whose worst cell outgrows Low.
- **Reach bound. Decided: visibility-based.** The engine has no draw distance: `FAR = 4096`
  in `camera.rs`, the portal walk has no distance term, and the frustum-all fallback can
  draw the whole map. A portal-path distance D never bounded what is seen. Mandatory is
  everything visible from anywhere in the cells reachable within a movement lead L, plus
  those cells: M(c) = cluster(c) ∪ pinned ∪ ⋃_{c' within L} ({c'} ∪ PVS(c')). PVS is
  sampled offline and is a lower bound. Movement is about 11–15 m/s sustained and about
  50 m/s in dash bursts of 200 ms, so L is about 16–32 m.
- Miss policy. Lean:
  - Mandatory blocks are never refused. The pool grows rather than refusing them.
  - A transient miss drops static direct light and keeps SH indirect.
  - A low-resolution fallback tile is still a candidate. It needs a seam prototype.
- Ownership. Texels belong to their receiver cell, so partitioning receivers cannot
  double-count a light. The shadowmask channel table and the animation descriptors stay
  global.
- Residency budgets. See the next section.
- Whether the next directory version drops solid and exterior cells from clustering. The
  baked residency set can ride the same version bump.

## Residency budget tiers

Goal: PostRetro runs impressively on laptop GPUs, as classic DOOM runs on a potato. One
player-facing tier covers every streamed and whole-resident resource, not SH alone. The
bullets below are leanings, not decisions.

- **Today.** SH alone has a budget: a hardcoded 256 MiB floor sized to a 6 GB GTX 1660,
  held twice (`DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` in `plan_initial_pool_floor`,
  `DEFAULT_GPU_FLOOR_BYTES` in `ShResidencyAccounting`). Pools reserve the smaller of the
  whole map and their share at install.
- **Stage 5 budget.** A debug-tool pool-cap slider. The player-facing tier below is a
  later spec.
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
  - Mandatory is M(c) from Reach bound. PVS comes from runtime portal walks from 27
    sampled eye points per cell (3×3×3 inset lattice, six 94° cube faces). L uses the
    untruncated distance recompute.
  - Cluster closure admits the whole cluster of each reached cell. That is the
    cluster-keyed design; the cell-granular figure is its lower bound.
  - All 2,082 non-solid, non-exterior cells are camera cells.
  - Self-checks: attribution matches each payload exactly, charts don't overlap, a
    stored-order repack reproduces every placement, and the distance recompute matches
    all 46,564 stored id 46 pairs.
- **Visibility.** Every figure is a lower bound.
  - Sightline (farthest sampled-visible cell): p50 71.6 m, p95 127.1 m, max 237.5 m.
  - Convergence: 9, 27 and 125 eye points. Worst-case bytes move at most 5% from 9 to
    125; p95 moves 6.6% from 9 to 27 and up to 10.3% from 9 to 125.
  - 0.28% of portal walks (941 of 335,898) overflow `MAX_PORTAL_WALK_STEPS`. The dry run
    keeps the truncated walk.

Visible-set table. Worst camera cell in MiB, texel-exact and whole layers in today's
packing (2048²); p95 after the slash; brackets count cells over Low's 256 MiB.

| L | Cell-granular, texel-exact | Cell-granular, whole layers | Cluster closure, texel-exact | Cluster closure, whole layers |
|---|---|---|---|---|
| 0 m | 72.2 / 28.6 | 252 / 140 | 136.7 / 64.3 | 392 [93] / 252 |
| 16 m | 84.3 / 50.8 | 280 [7] / 210 | 143.7 / 98.2 | 420 [512] / 350 |
| 32 m | 96.4 / 75.0 | 336 [180] / 280 | 165.4 / 132.1 | 518 [1,124] / 420 |

Fixed-tile table. P×P tiles, one owner unit each, L = 32 m. Tile/exact is tile texels over
chart texels for the whole map.

| Unit | P | Worst / p95 | Tile/exact |
|---|---|---|---|
| Cell | 128 | 153.6 / 127.4 | 1.72× |
| Cell | 256 | 221.8 / 181.3 | 2.46× |
| Cell | 512 | 438.4 [513] / 342.1 | 4.10× |
| Cluster | 128 | 266.4 [6] / 219.5 | 1.69× |
| Cluster | 256 | 336.9 [163] / 270.6 | 2.08× |
| Cluster | 512 | 490.0 [770] / 396.4 | 2.58× |

Charts larger than a tile need their own tiles: 91.7% of chart texels at P = 128, 72.4%
at 256, 31.7% at 512. Tiles would force chart splitting.

Cell-block table. Each cell's charts pack into one block (bake MaxRects, min area over up
to 10 four-aligned widths). Cell-granular M(c); static layers pack M(c)'s blocks from
scratch into 2048² layers, the no-fragmentation lower bound.

| L | Worst / p95 MiB | Static layers, worst / p95 |
|---|---|---|
| 0 m | 97.0 / 35.3 | 8 / 3 |
| 16 m | 111.7 / 64.2 | 9 / 6 |
| 32 m | 125.7 / 94.1 | 10 / 8 |

- Blocks cost 1.23× chart texels (per-cell p50 1.15×, p95 2.29×). Block bytes are 1.30–1.34×
  texel-exact. No block exceeds 2048 in either dimension; the largest is 1144×1844.
- **Pool walks.** L = 16 m, shelf allocator with free and merge, 20,000-step random walk
  and far-point tour. A 12-layer pool (125% of the static worst) repacks on 0–0.3% of
  steps with immediate free. LRU eviction repacks more often than immediate free: up to
  1.33% at 12 layers, up to 5.2% at 9. Hard fails: none at 9 or 12 layers.
- `campaign-test` blocks: worst 29.4 / 37.1 / 43.4 MiB at L = 0 / 16 / 32, static pool 3 / 3
  / 4 layers. Its walk needs 1 layer of slack over the static worst for the shelf
  allocator: a 3-layer pool hard-fails, a 4-layer pool does not.

Findings:
- **Cell granularity is what makes Low fit.** Cell blocks fit Low at every measured L with
  a worst case near half of it. Cluster-keyed whole layers do not fit, and neither do
  tiles at cluster granularity.
- **Id 46 caps each source cell at its 32 nearest partners**
  (`CELL_VISIBILITY_FANOUT_K`).
  - A cell's stored set counts pairs kept from either end. Measured against a
    portal-path D of 128 m, it averages 42 cells, against 305 recomputed.
  - The cap already binds at 16 m, where 178 cells hit it, and the stored and recomputed
    sets diverge from 32 m.
  - Any consumer that reads id 46 as "everything within D" undercounts: stored pairs
    would put today's packing at 280 MiB worst at 128 m, against 770 MiB recomputed.
  - The SH warm set does not read it that way. `WarmSource::from_cell_visibility` runs a
    multi-hop Dijkstra over the stored pairs, capped at 8 clusters, so the ~42 vs ~305
    one-hop gap doesn't cap it. What remains is inferred, not observed: a dropped hop can
    inflate a composed distance and reorder the top 8, and the kept-pairs graph could
    split. A missed cluster loads late as Visible demand with the ambient floor. It is
    never lost. Smallest fix if ever needed: always keep direct portal-neighbour pairs
    (about 16 B per portal, a stage-version bump).
- **`campaign-test` never nears the limit.** Its whole lightmap plus shadowmask is 56 MiB.

Still needed:

- Resident and mandatory bytes per GPU resource, CPU frame time, and draw counts in the
  dev panel, before any stage 5 plan or tier value is judged.
- A guillotine or merging pool allocator is unmeasured; the walks used a shelf allocator.
- The portal-walk step-limit overflow on the hallway map (0.28% of walks) may warrant its
  own look.
- A rebuild of `stress-warren-mini`, whose PRL predates BC5.
- A production-shaped map beyond the Stress Warren family and `campaign-test`.
- Seam and miss prototypes for the first lightmap-shaped resource, including
  cross-sector lights.
