# Fog Spot-Light Pre-Cull

> **Status:** ready
> **Related:** `context/lib/rendering_pipeline.md` §4 Lighting, §7.5 Fog Volume Composite, §9 Boundary Rule

---

## Goal

Pre-cull dynamic spot lights against active fog volume AABBs before they reach the fog raymarch's per-step inner loop. Mirrors the existing point-light pre-cull in `FogVolumeBridge::update_points` (which takes light/brightness pairs and computes active volumes). Spots that cannot scatter into any fog volume currently waste shader cycles every march step (attenuation math + a shadow `textureLoad` per step per spot).

## Scope

**In scope.**
- CPU-side bounding-sphere ↔ fog-AABB test for each shadow-slotted spot before upload.
- Renderer-side plumbing so the spot collector can read the active fog volume AABBs.

**Out of scope.**
- GPU-side cone-exact culling (per-step or per-tile).
- Shadow-pool slot assignment changes — slots remain assigned by the existing scoring path; only the *fog upload* is culled, not the shadow render.
- Changes to `fog_volume.wgsl` inner loop (still iterates `spot_count`; the count just gets smaller).
- Tighter-than-sphere tests (cone-vs-AABB). Bounding sphere is already a tight conservative cull for the typical spot range.

## Non-goals

- Reducing `MAX_FOG_VOLUMES` or `SHADOW_POOL_SIZE` capacities.
- Refactoring `collect_fog_spot_lights` into the bridge wholesale — bridge owns AABBs, renderer owns slot assignment + `level_lights`. Plumbing is one-way (AABBs → renderer).

## Acceptance criteria

1. A shadow-slotted dynamic spot whose bounding sphere (center = position, radius = `falloff_range`) misses every active fog volume AABB does not appear in the uploaded `FogSpotLight` buffer for that frame.
2. A shadow-slotted dynamic spot whose bounding sphere intersects at least one active fog volume AABB is uploaded unchanged (same fields, same `slot`, same pre-multiplied color).
3. `FogParams.spot_count` matches the number of records actually uploaded; the shader's `for i in 0..spot_count` loop bound shrinks accordingly.
4. When no fog volumes are active, the fog pass is already skipped (`FogPass::active()` returns false) — culling code is not exercised; behavior unchanged.
5. In scenes with shadow-slotted spots present but NOT intersecting any active fog volumes, GPU time of the fog raymarch pass measurably decreases versus the baseline. In scenes where spots DO intersect fog volumes, correctness is unchanged (same pixel output, same per-step iteration count).
6. Unit test covers: spot inside an AABB passes; spot outside all AABBs is dropped; static spot already excluded by existing `LightType::Spot` + slot-assigned filter remains excluded.

## Rough sketch

The fog spot collector lives in the renderer — `crates/postretro/src/render/mod.rs`, function `collect_fog_spot_lights` (around line 1683). It walks `self.spot_shadow_pool.slot_assignment`, filters to shadow-slotted dynamic spots in `self.level_lights`, applies the brightness-suppression threshold, and packs `FogSpotLight` records.

Add one filter step inside the loop, between brightness suppression and the `out.push`:

```rust
// Proposed
let center = Vec3::new(light.origin[0] as f32, light.origin[1] as f32, light.origin[2] as f32);
if !sphere_intersects_any_fog_aabb(center, light.falloff_range, fog_aabbs) {
    continue;
}
```

`fog_aabbs` is a slice (or iterator) of `(min, max)` pairs sourced from `FogVolumeBridge`. The intersection test mirrors the helper in `fog_volume_bridge.rs` (`sphere_intersects_any_aabb`): clamp the sphere center into each AABB, compare squared distance to `radius²`. A spot that misses all volumes is dropped.

## Plumbing

`FogVolumeBridge` owns the per-level AABB cache (`aabbs: HashMap<EntityId, FogVolumeAabb>`, `entity_ids: Vec<EntityId>`). The renderer does not currently read it.

Two plumbing points:

1. **Bridge exposes active fog AABBs.** Add a method on `FogVolumeBridge` returning the AABBs whose owning entity has a non-zero `FogVolumeComponent.density` this frame (matches the gate already applied in `update_volumes`). Shape: a slice of `(Vec3, Vec3)` min/max pairs cached on the bridge alongside `volumes_bytes` so it stays aligned with the bytes the renderer uploaded. Populated as a side-effect of `update_volumes`.

2. **Main loop hands AABBs to the renderer.** `crates/postretro/src/main.rs` (where `update_volumes` → `upload_fog_volumes` runs) passes the active-AABB slice to the renderer alongside the volume bytes. Either: (a) extend `Renderer::upload_fog_volumes` to accept AABBs, or (b) add a sibling `Renderer::set_fog_aabbs` call. Renderer caches the slice in a `Vec<(Vec3, Vec3)>` field on the `Renderer` struct; `collect_fog_spot_lights` reads it.

Either option keeps the boundary clean: the renderer never sees `FogVolumeBridge` types. The AABBs arrive at the renderer in main.rs (same location as volumes_bytes upload, where `update_volumes` → `upload_fog_volumes` runs) and are cached on the Renderer struct for use in `collect_fog_spot_lights` during the render pass. This 1-frame latency is safe: AABB data is immutable compile-time geometry, not frame-varying behavior. Choose at implementation time based on which keeps `upload_fog_volumes`'s byte-only contract intact (option b is preferred for that reason). If `collect_fog_spot_lights` is called before the first AABB update (e.g. frame 0), the cache is empty and all spots pass the test — safe, since culling is a perf optimization, not a correctness gate.

## Tasks

### 1. Implement spot pre-cull

- Extend `FogVolumeBridge` to cache active AABBs alongside the existing per-frame volume packing. Active = same gate as `update_volumes` (component present, density > 0).
- Expose a read accessor on the bridge for active AABBs.
- When `update_volumes` produces no active volumes, pass an empty slice to the renderer's AABB cache (same call site in main.rs). Ensures no stale AABBs persist across volume deactivations.
- Plumb the AABB slice from `main.rs` into the renderer per frame (sibling call to `upload_fog_volumes`).
- In `Renderer::collect_fog_spot_lights`, filter each shadow-slotted spot by sphere-vs-any-AABB before pushing.
- Reuse the intersection math pattern from `sphere_intersects_any_aabb` in `fog_volume_bridge.rs`. Copy the logic inline to the renderer's filter if needed to maintain module boundaries; both files are small.
- Unit tests in `render/mod.rs` (or wherever the collector ends up testable): spot inside an AABB passes the cull and uploads; spot outside all AABBs is dropped before upload; static (non-shadow-slotted) spot is rejected by the existing slot filter before reaching the AABB test.

## AC ↔ task cross-check

- AC 1, 2, 3, 6 → Task 1 (filter + tests).
- AC 4 → Task 1 (no behavioral change when pass is inactive — confirmed by reading the existing `if self.fog.active()` gate at the call site).
- AC 5 → Task 1 (the cull *is* the perf win; verified manually with GPU timing, not as an automated assertion).
- Task 1 → covers all ACs.

## Open questions

None. The pattern (`update_points`), the data source (bridge AABB cache), and the call site (`collect_fog_spot_lights`) are all confirmed in source.
