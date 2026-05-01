# Portal-Based Fog Volume Culling

> **Status:** stub draft
> **Depends on:** `fog-volumes` plan (shipping first) — specifically `FogVolume` GPU struct, `FogVolumeComponent`, and the `FogPass` compute pipeline. Also assumes per-frame `VisibleCells` bitmask from portal traversal (`context/lib/rendering_pipeline.md` §2, §7.1).
> **Related:** `context/lib/rendering_pipeline.md` §2 (portal traversal), §7 (visibility prepasses), §7.5 (fog composite) · `context/plans/drafts/fog-volumes/index.md` · `postretro/src/visibility.rs` (VisibleCells) · `postretro/src/render/fog_pass.rs` (raymarch compute)

---

## Goal

Cull the active fog volume list to only volumes reachable via portals from the current camera cell before dispatching the fog raymarch compute pass. The engine already bakes per-cell "which fog volumes are potentially visible from this cell" masks at level compile time; runtime uses the frame's `VisibleCells` bitmask to filter the GPU upload, eliminating GPU work for volumes entirely behind walls. On maps with many distinct fog zones (the intended use case — theatrical, moody, visually ambitious fog across interconnected spaces), this becomes meaningful — especially when design goals include many small localized volumes with different character rather than 1–2 blanketing the whole level.

---

## Approach

**Compile time:** For each empty BSP leaf (cell), determine which fog volumes' AABBs intersect the cell's geometry via a sweep over the BSP leaf's polygon set. Emit a per-cell fog-volume bitmask (`u32` array, one entry per cell; max 16 volumes → 16 bits per entry, fits). Store in PRL as a new optional section (alongside existing baked data).

**Runtime:** After portal traversal produces `VisibleCells` bitmask, union the fog-visibility masks for all visible cells to produce an active-volumes set. Cull the fog-volume GPU buffer before the raymarch dispatch, uploading only reachable volumes. If no volumes are reachable, skip the fog pass entirely (matches existing `FogPass::active()` guard).

**Result:** Raymarch compute avoids reading AABB and sampling for volumes that are geometrically unreachable. On a complex map (many rooms, multiple fog zones separated by walls), work drops proportionally to the culled volume count — especially significant when the design intent is theatrical, localized haze rather than global atmospheric fog.

---

## Dependencies

- **fog-volumes plan must ship first.** Depends on `FogVolumeComponent`, the `FogVolume` GPU struct (48 bytes: min/max AABB, density, color, scatter, falloff), and the `FogPass` compute pipeline including raymarch and composite passes. No changes to fog-volumes' design; this is a pure performance optimization layered on top.
- **Portal traversal and VisibleCells bitmask.** Already exist; `rendering_pipeline.md` §2 describes the per-frame flood-fill that produces the visible-cell set.

---

## Open Questions

1. **AABB intersection test at compile time.** When does a fog volume "see" a cell? Option A: AABB center point-in-cell test (BSP point classification, fast). Option B: AABB fully/partially overlaps cell's convex hull (more conservative, catches edge cases). Choose based on false-negative tolerance — aggressive culling risks popping; conservative culling keeps overhead high. Likely A (point-in-cell) with a note that volumes at cell boundaries may render even when not strictly reachable.

2. **Bitmask vs. index list representation.** Per-cell mask is `u32` (16 volumes max, matches `MAX_FOG_VOLUMES = 16` from fog-volumes); bitmask fits cleanly. Index list would be variable-length, complicating the PRL section format. Bitmask is simpler and matches the existing `VisibleCells` pattern (128-word bitmask). Decide at implementation time if a large map with many volumes needs a different representation.

3. **Gate condition.** This plan has no pre-work gate (unlike perf-per-region-bvh). Fog raymarch is already a small shader (only ~50 visible pixels when `fog_pixel_scale=4`, even on dense maps). Profile before promotion to determine if the culling overhead (CPU bitmask union per frame) justifies itself. Likely yes on dense, multi-zone maps; possibly not on simple single-zone layouts.

---

## Non-Goals

- Changing fog-volume visibility semantics or the AABB representation.
- Per-region fog rendering (portal regions to fog regions is a separate design question).
- Dynamic volume addition/removal affecting precomputed masks — baked at compile time only.
- Visibility from non-portal pathways (e.g., tall rooms or massive outdoor spaces). Portal traversal already bounds these via frustum culling; fog culling is strictly a portal-assisted optimization.

---

## Stub Content

**Why stub, not full spec?**

The core technique is straightforward: bake per-cell bitmasks, union visible cells' bitmasks, cull GPU uploads. The benefit is clear for the intended design use case (many small theatrical fog zones). The implementation is additive to fog-volumes with no architectural risk — PRL section is new but optional, runtime cull is a loop and a bitmask union. A full spec would prescribe wire formats and task breakdowns; stubs capture the concept and design intent, leaving the detailed work for a ready-stage planning pass.

**What would promote to ready:**

- Proof-of-concept measurements (fog raymarch time on a map with many volumes, with and without culling).
- PRL section wire format (how per-cell bitmasks are stored and loaded).
- Task breakdown (prl-build pass, PRL section loading, CPU union logic, GPU shader filter).
- Acceptance criteria (no volume pops, correct culling on the test map, no perf regression on maps with <2 fog zones).
