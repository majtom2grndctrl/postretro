# Large-Map Spatial Residency

> **Status:** Epic seed. SH is the first resource and ships through stages 1–4 below
> (`context/plans/in-progress/sh-probe-streaming/`). Generalizing to further resources is
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
- Lightmap charts pack across atlas layers without regard to clusters. Cluster residency
  of lightmap-shaped data needs a chart-to-cluster mapping, or a packer that respects it.
- Whole-atlas animated-lightmap compute already exceeds wgpu's per-dimension dispatch
  limit on `campaign-test`. Per-cluster dispatch must not inherit that shape.

## Decisions still open

- Byte-aware partition bounds, now that a second resource shares each cluster.
- Which lightmap-shaped section streams first (shadowmask id 42, lightmap id 22, animated
  weight maps id 25), and whether partial atlas layers justify their packing-density cost.
- Cross-cluster light, texture-boundary, and chart ownership for lightmap-shaped data.
- Platform residency budgets per resource.
- Whether the next directory version drops solid and exterior cells from clustering.

## Pre-planning measurements

Measured whole-resident section sizes (SH streams and is excluded):

| Section | `campaign-test` | `stress-warren-mini` (lightmap density 0.8) |
|---|---|---|
| Shadowmask atlas (id 42) | 64 MiB | 1.7 MiB |
| Animated light weight maps (id 25) | 60 MiB | 1.4 MiB |
| Lightmap (id 22) | 24 MiB | 0.6 MiB |

`stress-warren-hallway-inspection` measured id 42 at 88 MB, projected to 1.29 GB at the
default density (`context/plans/ready/shadowmask-atlas-compress-at-rest/`).

Still needed:

- Resident bytes per GPU resource, CPU frame time, and draw counts in the dev panel,
  before any stage 5 plan is judged.
- Per-cluster attribution of lightmap-shaped bytes from a dry-run chart-to-cluster pass.
- Post-compression sizes once the shadowmask at-rest brief lands.
- A production-shaped map beyond the Stress Warren family and `campaign-test`.
- Seam and miss prototypes for the first lightmap-shaped resource, including
  cross-sector lights.
