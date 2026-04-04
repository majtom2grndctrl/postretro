# Task 05: Frustum Culling

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** task-04 (PVS culling — provides the PVS-visible leaf set to refine), task-03 (camera — provides the view-projection matrix for frustum plane extraction).
> **Produces:** further-refined draw set. Leaves that are PVS-visible but outside the view frustum are discarded.

---

## Goal

After PVS narrows the visible leaf set, discard leaves whose bounding box falls entirely outside the view frustum. This removes geometry that is PVS-visible (not occluded by walls) but behind or beside the camera.

---

## Implementation Guidance

### Frustum plane extraction

Extract six frustum planes from the combined view-projection matrix. Standard approach: each plane is derived from a row combination of the 4x4 matrix (left, right, top, bottom, near, far). Normalize each plane.

### AABB-frustum test

For each PVS-visible leaf, test its axis-aligned bounding box against the six frustum planes. Use the standard "positive vertex" test:

- For each plane, find the AABB vertex most in the direction of the plane normal (the "positive vertex").
- If the positive vertex is behind the plane, the entire AABB is outside — cull the leaf.
- If any plane culls the leaf, skip it.

This is conservative — some leaves partially outside the frustum will still be drawn. Per-face culling is not needed for Phase 1.

### Integration with PVS pipeline

The culling pipeline is: PVS visible set -> frustum cull -> draw set. Frustum culling operates on the output of task-04, not on the full leaf set.

---

## Acceptance Criteria

1. Face a wall in an enclosed room. Draw count drops further compared to PVS-only culling.
2. Turn around — different faces are drawn, draw counts shift.
3. Looking at a corner where few leaves are visible produces a low draw count.
4. Frustum culling never removes geometry that should be visible (no popping or disappearing faces in normal navigation).
5. Performance: frustum test adds negligible overhead per frame (it operates on the already-reduced PVS set, typically tens to low hundreds of leaves).
