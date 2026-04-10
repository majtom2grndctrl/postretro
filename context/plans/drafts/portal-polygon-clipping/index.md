# Portal Polygon Clipping in Runtime Visibility

> **Status:** draft
> **Depends on:** none. Engine-only change to runtime portal traversal.
> **Related:** `context/lib/build_pipeline.md` §Runtime visibility · `context/plans/done/portal-bsp-vis/`

---

## Goal

Make per-frame portal visibility a single-pass algorithm by clipping each portal polygon against the current view frustum before narrowing. The narrowed frustum becomes a strict geometric subset of the camera frustum, and the per-leaf AABB frustum pass that currently restores camera-cone enforcement can be removed.

This is the architectural endpoint described in `build_pipeline.md` §Runtime visibility — the id Tech 4 (Doom 3) form of runtime portal flood-fill.

---

## Motivation

Runtime PRL visibility today is two cooperating layers:

1. Portal flood-fill with frustum narrowing.
2. Per-leaf AABB cull against the unmodified camera frustum.

The second layer exists because the first is approximate. Frustum narrowing builds a new frustum from `[portal plane, portal-edge planes, original far plane]` and discards the camera's left/right/top/bottom planes. For a portal fully inside the camera cone this is correct — the portal-edge cone is geometrically inside the camera cone — but for a portal that straddles the camera frustum boundary, the constructed cone can extend outside it. Each subsequent narrowing inherits and compounds the looseness.

Clipping the portal polygon against the current frustum before narrowing removes the failure mode at its source. The clipped polygon lies entirely inside the current frustum by construction. Edge planes derived from the clipped polygon form a cone strictly inside the current cone. By induction, every narrowed frustum reachable through any portal chain is a strict subset of the camera frustum, and any leaf marked visible by the flood-fill is guaranteed to lie inside the camera cone.

Once that invariant holds, the per-leaf AABB pass is genuinely redundant.

This matches id Tech 4's runtime portal vis — Doom 3, Quake 4, Prey — where polygon-to-frustum clipping is the load-bearing step that lets per-frame portal visibility be a complete culling system on its own.

---

## Scope

### In scope

- Polygon-vs-frustum clipping in the engine's runtime visibility module. Sutherland-Hodgman against an arbitrary set of half-spaces.
- Use the clipped polygon as both the visibility test (replaces the current "is polygon outside any plane" separating-axis test) and the input to frustum narrowing.
- Remove the per-leaf AABB frustum cull from the PRL portal-traversal path, after the new invariant is verified.
- Update or rewrite the runtime portal-traversal tests to cover: clipped-portal-fully-inside, clipped-portal-fully-outside, clipped-portal-partial, multi-hop strict-subset preservation.
- Update `context/lib/build_pipeline.md` §Runtime visibility to reflect the single-pass form. Remove the "two cooperating layers" note and the "do not remove either layer in isolation" warning.

### Out of scope

- Compile-time portal generation. PRL file format unchanged. The Portals section payload is the same — convex polygons connecting empty leaves.
- The `--pvs` (LeafPvs) fallback path. Independent.
- The BSP legacy loader's visibility path. Independent.
- Performance optimization beyond what the algorithm naturally provides. Profile after the rewrite is correct.
- Changing the BFS structure. The graph traversal stays as-is — only the per-portal computation changes.

### Non-goals

- Matching id Tech 4's source code line-for-line. Adopt the algorithmic shape.
- Eliminating all approximation. Floating-point clipping has its own epsilons; these are unavoidable and documented at the clip routine.

---

## Approach

### Algorithm

For each portal visited during the BFS:

1. Clip the portal polygon against every plane of the current frustum using Sutherland-Hodgman. If the result is empty (fewer than 3 vertices after clipping), the portal is not visible — skip.
2. Build the narrowed frustum from the clipped polygon: portal plane as near, one edge plane per clipped edge through the camera position, far plane carried from the current frustum.
3. Mark the neighbor leaf visible. Enqueue with the narrowed frustum.

The current "AABB outside frustum" early-out and the "polygon outside frustum" separating-axis test both collapse into step 1: an empty clip output is the unified rejection signal.

### Invariant

Every narrowed frustum produced by this algorithm is a strict geometric subset of the frustum it was narrowed from. By induction from the camera's initial frustum, every narrowed frustum reachable through any portal chain is a strict subset of the camera frustum. Therefore every leaf marked visible by the flood-fill lies inside the camera cone.

### Removing the AABB pass

After the new traversal lands and is verified manually on representative maps, remove the per-leaf AABB frustum cull from the PRL portal-traversal path. The strict-subset invariant makes it redundant. Removal is the final step, not the first.

---

## Acceptance Criteria

1. The runtime portal traversal yields a narrowed frustum that is a geometric subset of the input frustum for every portal hop. Verified by a unit test that constructs a multi-hop chain and checks each narrowed frustum's planes against the previous.
2. Maps with previously-observed culling behavior (test-2, test-3, and any others used during the regression hunt) cull correctly when the camera enters a side room. Verified manually with the wireframe overlay.
3. Removing the per-leaf AABB frustum cull does not visibly change rendered output on any test map. If it does, the strict-subset invariant is broken and the polygon clipping is incomplete — fix that, do not restore the AABB pass.
4. No visible degradation through narrow portal apertures. The original symptom that motivated the (later-reverted) `narrow_frustum` rewrite was missing geometry through narrow gaps; verify that symptom does not return.
5. No change to the PRL file format or the compiler.

---

## Risks

- **Sutherland-Hodgman epsilon tuning.** Polygon clipping at floating-point precision can drop or duplicate vertices near plane boundaries. Use a small epsilon, document it, and prefer over-inclusion to under-inclusion at the boundary (a one-vertex slop in favor of "inside" cannot violate the strict-subset invariant once the next narrowing runs).
- **Degenerate clipped polygons.** A portal polygon that touches the frustum at a single point or edge can clip to fewer than 3 vertices. Treat as "not visible" and continue the BFS — the same rejection path the empty case takes.
- **Frustum plane count grows.** The current narrowed frustum has `portal_polygon.len() + 2` planes. Clipping against the previous frustum first means the clipped polygon may have more vertices than the original portal (one new vertex per crossed plane). Expected; no architectural concern.

---

## What Carries Forward

| Output | Consumed by |
|--------|-------------|
| Single-pass runtime portal visibility | All future runtime visibility work — dynamic occluders, area-portal state changes, anything that builds on portal flood-fill |
| Sutherland-Hodgman polygon-vs-frustum clip routine | Reusable for any future runtime culling that needs polygon-against-half-space-set clipping |
| Strict-subset invariant in frustum narrowing | Lets future visibility features (e.g., per-portal user clipping, mirrors) compose without re-deriving the invariant |

---

## Open Questions

None at draft time. Promote to ready when scheduled.
