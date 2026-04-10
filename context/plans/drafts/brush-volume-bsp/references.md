# References — Doom 3 dmap source

Pointers into the canonical id Tech 4 / Doom 3 map compiler (`dmap`), used as the algorithmic reference for this refactor. The implementing agent should consult these directly when in doubt; the plan summarizes their behavior but the source is authoritative.

Two repos. Both verified for the algorithms cited in the plan. RBDOOM-3-BFG matches the 2011 GPL release on every algorithmic point relevant to this plan; consult RBDOOM only for the modernization notes at the end.

## Doom 3 GPL release (2011)

Repo: `https://github.com/id-Software/DOOM-3`
Path prefix: `neo/tools/compilers/dmap/`

| Topic | File | Function | What to read it for |
|---|---|---|---|
| Brush face list construction | `facebsp.cpp` | `MakeStructuralBspFaceList` | How structural brushes are flattened into a face list at BSP build time. Confirms there is no coplanar tiebreaker. |
| Splitter plane selection | `facebsp.cpp` | `SelectSplitPlaneNum` | The balance + split-count scoring heuristic. Source for Q1 (splitter candidate pool) resolution. |
| Tree construction recursion | `facebsp.cpp` | `BuildFaceTree_r` | Reference for how the recursion descends and terminates. |
| Brush filtering into the tree | `tritjunction.cpp` (or csg.cpp) | `FilterBrushesIntoTree` | How brush solidity is propagated to leaves during/after construction. The `node->opaque` flag set here is what Phase B's leaf check tests against. |
| **Phase B Pass 1 — visible hull build** | `usurface.cpp` | `ClipSideByTree_r` | **The most important reference for this plan.** Plane-equality routing (when the side's `planenum` equals the node's `planenum`, route to front child only) and convex-hull accumulation at non-opaque leaves. |
| **Phase B Pass 2 — area distribution** | `usurface.cpp` | `PutWindingIntoAreas_r` | The second pass: walks each side's `visibleHull` through the tree and emits triangles into empty leaves with valid area assignments. The leaf check is `node->area >= 0 && !node->opaque` — no further geometric test. |
| Convex hull union | `usurface.cpp` | `AddToConvexHull` (called from `ClipSideByTree_r`) | The hull-union primitive Pass 1 uses to accumulate fragments. Takes the side's plane normal as the projection axis. |

## RBDOOM-3-BFG (active modernization fork)

Repo: `https://github.com/RobertBeckebans/RBDOOM-3-BFG`
Path prefix: equivalent under `neo/tools/compilers/dmap/`

For every algorithm relevant to this plan, RBDOOM matches the 2011 GPL release verbatim (only formatting differences — Allman braces vs. K&R). Consult RBDOOM only for these orthogonal modernizations:

| Topic | What it changes | Postretro stance |
|---|---|---|
| Per-axis configurable block size | `BLOCK_SIZE` becomes per-axis floats; can disable forced block-boundary splits per axis | We don't have block-boundary splits today; not relevant |
| Alternate splitter scoring | Surface-area bias, plane reuse counters, near-edge penalty, imbalance penalty | Gated behind `#if 0` in RBDOOM (experimental, not active). Do not adopt; keep the simple heuristic |
| Polygon mesh primitives (`polyTris`) | New code path for Blender-exported mesh geometry as BSP splitters | Out of scope; brushes only |
| Subtractive brushes (`b->substractive`) | Reverses winding direction for subtractive boolean brushes | Out of scope |
| Valve 220 texture projection (`texValve220` in `side_t`) | Adds the TrenchBroom-output projection format to the side struct | **We already handle this** via shambler at the parse boundary — no source-level changes needed |
| Removed `gldraw.cpp` | OpenGL debug visualizer dropped | We never had one; not relevant |

## Agent verification

The dmap research that informed these references was performed by a Sonnet research agent on 2026-04-10. The agent's task was: read both repos via WebFetch, find the canonical answer to (a) the coplanar tiebreaker rule and (b) the brush-side face extraction algorithm, and quote the relevant source. The agent reported high confidence on both questions, with verbatim source quotes from `facebsp.cpp:MakeStructuralBspFaceList` and `usurface.cpp:ClipSideByTree_r` / `PutWindingIntoAreas_r`.

If reading the source independently to verify or extend, start with `usurface.cpp` — Phase B is the highest-stakes algorithmic choice in this plan and the source there is the definitive spec.
