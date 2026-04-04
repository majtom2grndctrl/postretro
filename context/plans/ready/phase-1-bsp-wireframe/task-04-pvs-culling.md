# Task 04: PVS Culling

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** task-01 (BSP data — visibility data, BSP tree, leaf-to-face mappings), task-03 (camera — provides camera position each frame).
> **Produces:** filtered draw set (only PVS-visible faces submitted to renderer). Consumed by task-05 (frustum culling refines this set further).

---

## Goal

Implement PVS-based visibility culling. Each frame, determine which BSP leaf the camera occupies, decompress the PVS for that leaf, and submit only faces belonging to visible leaves. This is the primary visibility optimization — it eliminates geometry behind walls and around corners.

---

## Implementation Guidance

### Point-in-leaf query

Walk the BSP node tree from the root. At each node, test camera position against the split plane. Descend into the front or back child. Repeat until reaching a leaf. This gives the camera's current BSP leaf index.

### PVS decompression

Look up the compressed PVS bitfield for the camera's leaf. Decompress it:

- Quake standard RLE format: a zero byte signals a run of invisible leaves. The next byte is the count of zero bytes to expand.
- Check whether qbsp 0.14 exposes decompressed PVS directly. If it does, use that API. If it only provides raw visdata bytes plus per-leaf PVS offsets, implement the RLE decompression.

The decompressed PVS is a bitfield where bit N indicates whether leaf N is visible from the camera's leaf.

### Build visible leaf set

Iterate the decompressed PVS. Collect the set of visible leaf indices.

### Filter draw set

Only submit faces belonging to visible leaves. Use the leaf-to-face mapping from task-01's per-face metadata.

Approach: each frame, iterate visible leaves, collect their face ranges, and issue draw calls (or build an indirect draw list) covering only those faces. The simplest initial approach is to rebuild a per-frame index buffer or use multiple draw calls — choose based on what produces clean code. Optimization can come later if needed.

### Missing PVS handling

When PVS data is absent (map compiled without `vis`), draw all faces. Log a warning once at load time, not per-frame.

---

## Key Decisions

| Item | Resolution |
|------|------------|
| qbsp PVS API | Check qbsp 0.14. If it exposes decompressed PVS, use it. If only raw visdata bytes, implement Quake-standard RLE decompression (zero byte = run of invisible leaves, next byte = count). |
| Draw set filtering | Rebuild per-frame visible face list from PVS. Start simple (multiple draw calls or per-frame index buffer rebuild). Optimize if profiling shows a bottleneck. |
| Missing PVS | Draw all faces. Log warning once at load time. |

---

## Acceptance Criteria

1. Navigate to a room not visible from another room. Logged draw counts drop compared to drawing everything.
2. Compile the test map without `vis`. Draw count rises to total face count, confirming PVS was doing work.
3. Missing PVS data triggers a single load-time warning, not per-frame log spam.
4. No crash or panic when PVS data is absent.
5. Point-in-leaf query returns correct leaf index (verifiable via task-06 diagnostics or debug logging).
