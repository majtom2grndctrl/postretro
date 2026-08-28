# Large-Map Spatial Residency

> **Status:** Epic seed. Research-backed but not a draft or ready implementation
> plan. Do not orchestrate. A future planning session turns this into scoped
> specs after the listed measurements exist.
> **Supporting research:** `context/research/spatial-streaming.md`.

## Outcome

One authored level remains one logical PRL. Large levels can retain only the
nearby baked spatial resources in memory, while portals and designed seams hide
loading. Map scale is bounded by disk and residency budget rather than a
whole-level GPU upload.

## Scope boundary

This seed does not change the active adaptive-SH-coarsening work, the current
whole-PRL level-install loader, or renderer ownership of GPU resources.
Adaptive SH coarsening and residency solve different problems: coarsening
reduces data inside a resident chunk; residency selects the chunks retained.
Both remain useful.

## Existing substrate

- BSP empty-leaf cells are runtime `cell_id`s and portal traversal already
  produces visible cells.
- Cluster adjacent cells into bounded residency units. Do not create a parallel
  per-subsystem spatial query.
- Regional BVH is a culling-layout idea, not the residency authority. Its old
  plan was archived; only the cell-clustering lesson carries forward.
- Today PRL loads whole and renderer uploads sections whole. There is no
  partial texture streaming.

## Staged architecture

1. Compiler-only clustering and deterministic cluster directory: cluster IDs,
   bounds/cell membership, and per-spatial-section byte ranges or payloads.
2. Prove that directory without eviction. Keep the global bake view needed by
   SH/SDF baking.
3. Add visible-cell-driven prefetch/hysteresis and one resource's residency,
   likely SH/delta data or geometry. Keep lightmaps whole initially.
4. Move disk/decode work off-frame-path. Renderer atomically uploads, installs,
   and retires cluster generations under a budget.
5. Generalize the same cluster state to lightmap layers, SH base/deltas,
   geometry/BVH views, SDF, probes, fog, and acoustics.

## Constraints for planning

- Portal-adjacent clusters must prefetch/retain enough data to avoid visible
  geometry or lighting holes. Define cross-cluster SH ownership/halo rules.
- Cross-sector lights must preserve no-double-counting; lights cannot be
  assumed local to one cluster.
- A frame sees generation-matched dependent resources, never a mixed partial
  install. Define a conservative fallback or designed seam gate for misses.
- I/O and CPU preparation stay outside Input → Game logic → Audio → Render →
  Present. Renderer owns GPU upload, synchronization, and retirement.
- Co-op admission preserves one logical level/content identity. Residency is
  local resource state, not divergent gameplay/collision/visibility content.

## Decisions still open

- Cluster byte/primitive targets, deterministic partition rule, and authored
  seam/priority/always-resident hints.
- PRL directory shape and whether payloads stay one file with ranges or use
  sidecars.
- First streaming resource, miss behavior, prefetch horizon, hysteresis, and
  eviction policy.
- Cross-cluster light, probe, and texture-boundary ownership.
- Platform residency budgets and whether partial lightmap layers justify their
  packing-density cost.

## Pre-planning measurements

- Whole-level disk, CPU, and GPU residency by spatial section on representative
  production-shaped maps; include per-cluster distribution after a dry-run
  clustering pass.
- Portal visibility and predicted-next-cluster traces; quantify working-set,
  doorway churn, and prefetch lead time.
- Storage latency/decode/upload timing on target hardware; measure cold and
  warm cache behavior and the cost of atomic installation.
- Seam/miss prototypes for the first resource, including cross-sector lights
  and SH boundaries.
- Frame-time, draw-call, and memory effects against the current whole-level
  baseline. Do not infer a solution from Stress Warren alone.
