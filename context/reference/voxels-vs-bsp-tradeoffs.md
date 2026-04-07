# Voxel Ray-Cast vs BSP Portals: Trade-offs

> **Read this when:** choosing or evaluating the visibility pipeline.
> **Key invariant:** both approaches produce correct PVS from any solid geometry — the differences are compile cost, precision, and runtime flexibility.

## How BSP portals work

BSP compiler splits space using planes from brush faces. Every leaf is a convex volume classified solid or empty. A portal is the polygon where two empty leaves share a boundary — the splitting plane clipped to leaf bounds. `vis` traces anti-penumbra through chains of portals to determine leaf-to-leaf visibility.

Portals form between every pair of adjacent empty leaves, not just at doorways. Any wall, pillar, or solid brush creates BSP splits, separate leaves, and portals. Visibility is exact — if geometry blocks a sightline, `vis` knows.

## How voxel ray-casting works

Voxelizer stamps brushes into a 3D solid/empty grid. Spatial grid groups voxels into clusters. Ray marcher (3D-DDA) fires rays between cluster sample points through the voxel grid. Cluster pairs with sufficient ray connectivity are mutually visible.

Visibility is approximate — sampled, not proven. Thresholds filter noise. Resolution determines minimum occluder size.

## Where each approach has advantages

| | Voxel ray-cast | BSP portal |
|---|---|---|
| **Correctness** | Approximate (sampling, thresholds) | Exact (geometric proof) |
| **Tuning** | Voxel resolution, sample count, visibility threshold | None — but BSP split heuristics affect tree quality |
| **Compile cost scaling** | Resolution³ for grid, clusters² × samples for PVS. Topology-independent but brute-force. | Portal count × anti-penumbra complexity. Fast when portals are small (indoor), slower when large (open areas). |
| **Culling selectivity — indoor** | Good with sufficient resolution | Excellent — tight portals reject aggressively |
| **Culling selectivity — open areas** | Same as indoor | Low — most leaves see most other leaves |
| **Dynamic visibility** | Rebake required | Enable/disable portal (e.g. door open/close) |
| **Volumetric queries** | Natural — "how much solid between A and B?" | Not designed for this |
| **Leaf shape** | Grid-aligned cells | Convex regions conforming to geometry |

## Common misconception: "portals need chokepoints"

BSP portals don't need chokepoints to be *correct*. A city street with buildings on both sides produces valid portals and correct occlusion. Buildings block cross-block sightlines through portal anti-penumbra, same as doorway walls.

Chokepoints help *culling selectivity*. Small portals (doorways) reject most leaf pairs. Large portals (open plazas) reject few leaf pairs. The system still works — it just saves less at runtime.

## Why we chose voxel ray-casting

The original face-based BSP partitioned faces, not volumes. Portal generation leaked through walls. Rather than build a brush-based solid/empty BSP (which Quake's qbsp does), we built a voxel grid to recover solid/empty classification and used ray-casting for visibility.

A brush-based BSP was the road not taken. We have the brush volumes and could still build one.

## Compile time: what we know

Early benchmarks (2026-04) show ericw-tools (qbsp+vis) compiling test maps faster than our voxel compiler. This is unsurprising — ericw-tools is 25+ years of optimized C with early-out pruning (portal flow stops when blocked). Our compiler is young, and voxel ray-casting is brute-force (tests every cluster pair).

Whether voxel compile time scales better on large open maps is plausible but unproven. No benchmarks exist for this yet.

## Relevance to product goals

Cyberpunk city aesthetic — mixed indoor/outdoor, multi-level structures, dense urban geometry. Both approaches produce correct results for this content. The voxel approach was chosen because it avoided implementing a full brush-based BSP compiler, not because of proven compile-time advantages. BSP portals remain a viable alternative.
