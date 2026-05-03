# Portal-Based Fog Volume Culling

> **Status:** ready
> **Builds on:** `fog-volumes` plan (already shipped) — specifically `FogVolume` GPU struct, `FogVolumeComponent`, and the `FogPass` compute pipeline. Also assumes per-frame `VisibleCells` bitmask from portal traversal (`context/lib/rendering_pipeline.md` §2, §7.1).
> **Related:** `context/lib/rendering_pipeline.md` §2 (portal traversal), §7 (visibility prepasses), §7.5 (fog composite) · `context/lib/build_pipeline.md` (PRL section IDs) · `context/plans/drafts/fog-volumes/index.md` · `crates/postretro/src/visibility.rs` (VisibleCells) · `crates/postretro/src/render/fog_pass.rs` (raymarch compute)

---

## Goal

Cull the active fog volume list to only volumes reachable via portals from the current camera cell before dispatching the fog raymarch compute pass. Per-cell "which fog volumes overlap this cell" bitmasks are baked at compile time. Runtime unions the bitmasks of visible cells to produce an active set, then repacks the GPU fog buffer to only those volumes. The raymarch shader's per-sample AABB-test loop shrinks proportionally to the culled count; when the active count is zero the fog pass is skipped entirely. The benefit scales with the design intent: many small theatrical fog zones across interconnected spaces, not one or two volumes blanketing the level.

---

## Approach

### Compile time

After BSP construction (step 2 of the prl-build pipeline), collect the world-space AABBs already resolved by prl-build's parse step. With the AABB list in hand, walk every BSP leaf and test each fog AABB against the leaf's bounding box (`BspLeaf.bounds`). AABB-vs-AABB overlap is conservative — no leaf-boundary pop artifacts. Set bit `i` in leaf `L`'s mask when volume `i` overlaps leaf `L`. Write `0` for solid leaves without testing — they never appear in `VisibleCells::Culled`. The result is a `u32` per BSP leaf (full index); `MAX_FOG_VOLUMES = 16` keeps every active bit in the low half of the word. Bits 16–31 are reserved (zero on write, ignored on read) so the section can grow without a format break. Emit the array as PRL section 31 (`FogCellMasks`) when at least one `env_fog_volume` exists; omit the section otherwise.

### Runtime

After portal traversal produces `VisibleCells`, build `active_mask: u32`. (`volume_count` = number of `FogVolume`s loaded from PRL section 30 — canonical, immutable per level-load.)

- `Culled(ref leaves)` + `FogCellMasks` present: iterate the leaf indices, OR each leaf's `u32` mask from the loaded `FogCellMasks` array, then AND against `all_slots_mask = (1u32 << volume_count) - 1` so reserved bits 16..32 cannot light up a phantom slot.
- `Culled(ref leaves)` + `FogCellMasks` absent: legacy-PRL fallback. Set `active_mask = all_slots_mask` so a level baked before section 31 existed still renders all canonical fog volumes; `live_mask` continues to gate density-zero slots. Section absence does **not** imply `volume_count == 0` — a legacy PRL can carry section 30 (FogVolumes) without section 31.
- `DrawAll`: portal traversal fallback (solid-leaf camera, exterior, or no-portals map). Set `active_mask = all_slots_mask` so every canonical volume stays active.

Repack the GPU fog buffer densely: write only volumes whose bit is set in `active_mask`, in ascending source-index order. Volume indices in the GPU buffer are not stable across frames — no other system may reference them. Upload `active_count = active_mask.count_ones()` to the FogPass uniform; the shader loops `0..active_count` as before. When `active_count == 0`, skip the fog pass entirely; this matches the existing `FogPass::active()` guard.

---

## Wire Format

PRL section 31 (`FogCellMasks`). Optional. Present when at least one `env_fog_volume` brush entity exists in the source map.

| Field | Type | Description |
|-------|------|-------------|
| `cell_count` | `u32` | Total BSP leaf count (solid + empty). Matches the leaf-array length in the `BspLeaves` section. |
| `masks` | `[u32; cell_count]` | Index `i` is leaf `i`'s fog-volume bitmask. Bits `0..MAX_FOG_VOLUMES` (`MAX_FOG_VOLUMES = 16`): volume present in this cell. Bits `16..31`: reserved, written as `0`, ignored on read. Solid leaves are written as `0`. |

Section absent ⇒ no fog volumes in the map (`volume_count == 0`). `FogPass::active()` returns false and the pass is skipped before any mask lookup.

Index `i` in `masks` is the full BSP leaf index, matching the leaf indices carried by `VisibleCells::Culled`. Solid leaves are `0` and never appear in `VisibleCells::Culled`, so they never contribute to `active_mask`.

---

## Dependencies

- **fog-volumes plan (already shipped).** Uses `FogVolumeComponent`, the 96-byte `FogVolume` GPU struct, and the `FogPass` compute pipeline (raymarch + composite). No changes to fog-volumes' design.
- **Portal traversal and `VisibleCells`.** Per-frame flood-fill from the camera leaf already produces `VisibleCells::Culled(Vec<u32>)` or `VisibleCells::DrawAll`. See `rendering_pipeline.md` §2.
- **`env_fog_volume` AABB resolution.** prl-build's parse step already resolves each `env_fog_volume` brush to a world-space AABB. Task 1 consumes that existing output — no duplication needed.

---

## Tasks

### Task 1 — prl-build: fog AABB resolve and bitmask bake

- Collect the world-space AABBs already resolved by prl-build's parse step — no new derivation needed.
- For each BSP leaf, test each fog AABB against the leaf's bounding box (`BspLeaf.bounds`). Set bit `i` in the leaf's mask if the AABBs overlap. Write `0` for solid leaves without testing.
- Emit the masks array as PRL section 31 (`FogCellMasks`) when any `env_fog_volume` exists. Omit the section otherwise.

### Task 2 — level-format: FogCellMasks section parser

- Add section ID 31 (`FogCellMasks`) to `postretro-level-format`.
- Parse: read `cell_count: u32`, then `cell_count` × `u32` masks.
- Expose as `Option<Vec<u32>>` — `None` when the section is absent.

### Task 3 — runtime loader: store fog cell masks

- Load `FogCellMasks` from the PRL and store it on the level data accessible to the renderer.
- `None` is the all-zero / `DrawAll`-equivalent case; runtime falls back to the full fog-volume set.

### Task 4 — FogPass: CPU union, dense repack, upload filter

- After portal traversal:
  - `Culled(ref leaves)`: OR the per-leaf masks → `active_mask: u32`.
  - `DrawAll`: `active_mask = (1u32 << volume_count) - 1`.
- CPU-side: maintain the canonical `Vec<FogVolume>` in source order. Each frame, write the dense subset (volumes whose bit is set in `active_mask`) into the existing fog SSBO via `queue.write_buffer`. Upload unconditionally when `active_count > 0`.
- Upload `active_count = active_mask.count_ones()` by repurposing the existing `volume_count` field in the FogPass uniform. The WGSL raymarch loop bound is already fed from this field — no shader change required beyond the rename.
- Update `FogPass::active()` to gate on the per-frame `active_count` rather than the static loaded count.
- When `active_count == 0`, skip the fog pass via the `FogPass::active()` guard.
- Add a criterion microbenchmark for the bitmask-OR loop over a synthetic 200-leaf input, asserting < 10 µs.

---

## Acceptance Criteria

- **Correctness — overlap.** A fog volume whose AABB straddles a portal boundary is visible from every cell whose convex hull it overlaps. No volume pops at cell crossings.
- **Correctness — full skip.** A map where every fog volume sits behind walls relative to the camera produces `active_count == 0` and the fog pass does not run; `FogPass::active()` returns false.
- **Regression — single zone.** A map with one fog volume covering the whole playable area renders identically with and without culling. Bit 0 stays set for every reachable cell; no frame-to-frame flicker.
- **Regression — DrawAll path.** When `VisibleCells::DrawAll` is produced (solid-leaf camera, exterior, no-portals map), every fog volume uploads and the pass runs as it does today.
- **Performance gate.** On a test map with 8 or more fog volumes in separate rooms, fog-pass GPU time (measured via `POSTRETRO_GPU_TIMING=1`) with culling enabled is lower than without culling whenever the camera occupies a room containing 2 or fewer fog volumes. CPU union cost (the bitmask OR loop over visible leaves) stays under 10 µs on a 200-cell map, verified by a microbenchmark.

---

## Non-Goals

- Changing fog-volume visibility semantics or the AABB representation.
- Per-region fog rendering (portal regions to fog regions is a separate design question).
- Dynamic volume addition/removal affecting precomputed masks — baked at compile time only.
- Frustum-only culling for fog volumes. The `DrawAll` fallback uploads all volumes; that is the accepted behavior for solid-leaf, exterior, and no-portals cases.
- Volume index stability — volume indices in the GPU buffer are not stable across frames; no external system may reference them.
