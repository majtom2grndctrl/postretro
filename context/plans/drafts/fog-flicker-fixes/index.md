# Fog Flicker Fixes

## Goal

Eliminate visible fog flickering in `campaign-test.prl` (and any map with fog volumes whose AABBs straddle zero-face empty leaves or portal boundaries). The dominant cause is that fog activation is gated on the *drawable* leaf set, which strips zero-face leaves from the fog-cell-mask OR — so visible volumes pop in and out as the camera moves. Secondary causes are silhouette-aliased low-res depth taps, near-surface march collapse, AABB-face float jitter, BSP camera-leaf bias, and f32 precision loss in fog animation timing.

## Scope

### In scope

- Separate "drawable leaves" from "fog/light-reachable leaves" so fog and dynamic-light gating use the wider, portal-reachable set without the `face_count > 0` filter.
- Unconditionally union the camera leaf's fog mask into the active set on the `Culled` visibility path.
- Wire `light_influences` into `update_dynamic_light_slots` (currently passed as `&[]`) so spot lights in zero-face leaves participate in fog beam scattering.
- Validate `fog_cell_masks` length against `leaves.len()` at PRL load; log and degrade to "all slots active" on mismatch.
- WGSL: replace single nearest depth tap with a min-over-block (`pixel_scale × pixel_scale`) in the fog raymarch.
- WGSL: drop the `step * 0.5` floor on outer `start_t` — sub-interval entry already half-step-offsets.
- Runtime AABB epsilon inflation in `FogPass::set_canonical_volumes` (1 mm) to stabilize strict-`<` slab clip at exact AABB faces.
- BSP camera-leaf hysteresis (dead band of ~1 mm) with a `with_hint` variant; fog code calls the hinted form.
- Widen fog-animation timing to f64 in the bridge, narrow to f32 only after differencing.
- Sticky-frame fog activation in `FogPass::repack_active`: keep a volume active for N frames after its leaf exits the active set, so transient narrowed-portal frames don't deactivate it.

### Out of scope

- Temporal jitter + composite upscale filter (Fix O in the source report). Deferred — only revisit if banding crawl remains the dominant artifact after the in-scope changes ship.
- Adding script-spawned dynamic spots into `collect_fog_spot_lights` (Fix I in the report). Missing feature, not flicker — separate plan.
- Any PRL bake change. The baker is correct; the runtime filter is the bug.
- Reverting nearest-upscale composite blit. The pixelated aesthetic stays.

## Acceptance criteria

- [ ] Running `cargo run -p postretro -- content/dev/maps/campaign-test.prl` and walking around the existing `fog_volume` and `fog_lamp` shows fog density that does not pop on/off as the camera moves across portal seams or near zero-face leaves. (Manual visual check.)
- [ ] Spot lights whose host leaf has zero faces still scatter visible beams in fog as the camera approaches; beams do not blink as adjacent leaves enter/exit the drawable set.
- [ ] Walking the camera into a wall inside a fog volume leaves the fog visible at the near plane — fog does not punch a hole around the camera.
- [ ] A fog-volume AABB whose face lies on a leaf split plane renders without per-frame inclusion/exclusion as the camera grazes the plane.
- [ ] Loading a PRL whose `FogCellMasks` section is shorter than `leaves.len()` does not crash; renderer logs a warning and falls back to "all slots active" until the next level load.
- [ ] After ~30 minutes of uptime, fog-volume animation curves continue to interpolate smoothly with no visible sub-second density steps.
- [ ] Existing fog tests (`render/mod.rs` `compute_fog_cell_mask` tests, `level-format/src/fog_cell_masks.rs` `union_active_mask` tests) pass with the new dual-set plumbing; new tests cover camera-leaf union and length-mismatch fallback.
- [ ] No new `cargo clippy` warnings; `cargo test --workspace` passes.

## Tasks

### Task 1: Dual leaf-set in visibility

Split `determine_visible_cells` output so callers get both the existing drawable list and a wider fog/light-reachable list. Portal traversal (`portal_vis::portal_traverse`) already records reachability without consulting `face_count`; the `face_count > 0` predicate currently lives only in the post-traversal filter (`visibility.rs` portal-path and `visible_leaves_frustum_all`). Introduce a `fog_reachable_leaves: Vec<u32>` alongside `VisibleCells`, produced from the same traversal sweep with the `face_count` predicate dropped (the `!is_solid` and `portal_visible[idx]` predicates stay). Each visibility-mode arm (`Portal`, `SolidLeaf`, `ExteriorCamera`, `NoPortals`) returns the appropriate fog-reachable list — solid-leaf and exterior-camera paths return every non-solid leaf (matching today's `DrawAll` semantics for fog). Thread the new list from `main.rs` through `Renderer::render_frame_indirect` into `compute_fog_cell_mask` *and* into `update_dynamic_light_slots`. Update existing callers that don't need the wider set to ignore it.

### Task 2: Camera leaf union in fog mask

In `compute_fog_cell_mask`, accept the camera-leaf index (already on `VisibilityStats.camera_leaf`) and OR `fog_cell_masks.get(camera_leaf)` into the result on the `Culled` path. Idempotent if the leaf is already in the fog-reachable list. Plumb `camera_leaf` from `main.rs` through `render_frame_indirect`. Update the existing `compute_fog_cell_mask` unit tests to pass a camera leaf and add one new case covering a camera-leaf with bits the visible-leaf union misses.

### Task 3: Light slots consume fog-reachable mask + wire light_influences

In `update_dynamic_light_slots` (`render/mod.rs`), build `visible_leaf_mask` from the fog-reachable set produced by Task 1, not from `VisibleCells::Culled`. At the existing call site, replace the `&[]` argument with the real `LightInfluence` slice the renderer already owns (audit which Renderer field carries it — the parameter exists on the signature but no caller supplies it). Confirm `SpotShadowPool::rank_lights` continues to gracefully evict overflow lights when more lights enter the slot competition.

### Task 4: PRL fog_cell_masks length validation

In `prl.rs` where `FogCellMasks` parses (around line 697), compare `masks.len()` against `leaves.len()` (already loaded earlier in the same function). On mismatch, log at `warn!`, set `level_world.fog_cell_masks = None`, continue load. The renderer's existing "masks absent → all slots active" fallback (see `rendering_pipeline.md` §7.5) handles the degraded case. Add a unit test that loads a synthetic PRL with truncated masks.

### Task 5: Min-over-block depth tap

In `fog_volume.wgsl` (the `cs_main` depth-load block around line 348), replace the single `textureLoad` with a `min`-reduce over the `pixel_scale × pixel_scale` block of full-res depth samples covered by the scatter texel. The scale is derivable from `depth_dims / out_dims`. Min selects the closest hit, so fog never bleeds through silhouettes. Bound the loop with constant comparisons against the static-known max `pixel_scale = 8` (per `worldspawn.fog_pixel_scale` range) to keep WGSL happy.

### Task 6: AABB epsilon inflation on upload

In `FogPass::set_canonical_volumes` (`render/fog_pass.rs:544`), expand each canonical volume's `min`/`max` by `1.0e-3` (1 mm) in each axis before storing on `FogPass::canonical_volumes`. Document the epsilon next to the field. Plane-bounded clip planes inside the AABB are unaffected — they live in their own buffer and clip independently.

### Task 7: Drop start_t step-floor

In `fog_volume.wgsl` (line 355–356), change `let start_t = max(fog.near_clip, step * 0.5);` to `let start_t = fog.near_clip;`. Per-sub-interval half-step alignment at line 498 (`var t = sub_enter + step * 0.5;`) is unchanged and still keeps samples off the entry plane. Walk-into-wall must keep fog visible at the near plane.

### Task 8: BSP camera-leaf hysteresis

Add `LevelWorld::find_leaf_with_hint(position: Vec3, prev_leaf: Option<usize>) -> usize` in `prl.rs`, mirroring `find_leaf` but applying a ~1 mm dead band around each split plane: when `|side| < EPS`, prefer the side that matches `prev_leaf`. Store `last_camera_leaf: Option<usize>` on `Renderer` alongside the existing `last_camera_position`; use the hinted form for fog-mask camera-leaf lookups. Gameplay code keeps the unbiased `find_leaf`.

### Task 9: f64 fog animation timing

In `fog_volume_bridge.rs::tick` and `sample_*_curve_at` helpers, accept and operate on `time_seconds: f64` (or compute `now_ms` as f64 internally). Compute `(now_ms - start_ms)` and `% period_ms` in f64; narrow to f32 only at the leaf where the curve is sampled. Caller chain: `main.rs` script-time source likely already in seconds — widen at the bridge boundary so other consumers don't change.

### Task 10: Sticky-frame fog activation

Add `last_active_frame: Vec<u32>` on `FogPass`, sized to `canonical_volumes.len()`, tracking the most recent frame index each volume was in `cell_mask & live_mask`. In `repack_active`, OR in any volume whose `last_active_frame >= current_frame - HYSTERESIS` (e.g. `HYSTERESIS = 8`). The slab-clip prologue in WGSL early-outs cheaply for volumes the ray doesn't intersect, so the cost is bounded. Reset on level load.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the new fog/light-reachable list; downstream tasks consume it.

**Phase 2 (concurrent):** Task 2, Task 3 — both consume the new list; touch different functions in `render/mod.rs`. Land sequentially if file-merge conflicts surface, otherwise parallel.

**Phase 3 (concurrent):** Tasks 4, 5, 6, 7, 8, 9, 10 — independent files / independent functions. Fully parallel.

## Rough sketch

**Visibility split (Task 1).** Today `visibility.rs:240-251` (`visible_leaves_frustum_all`) and `:329-339` (portal-path) both filter on `!is_solid && face_count > 0 && portal_visible[idx]`. The fog-reachable list drops the `face_count > 0` clause. Cleanest shape: return a struct from `determine_visible_cells`:

```rust
// Proposed
pub struct VisibilityResult {
    pub visible_cells: VisibleCells,         // drawable — face_count > 0
    pub fog_reachable: Vec<u32>,             // wider — no face_count filter
    pub stats: VisibilityStats,
}
```

`render_frame_indirect` takes the result; `compute_fog_cell_mask` (definition at `render/mod.rs:53`, call at `:2867`) takes `fog_reachable` instead of `visible_cells`.

**Camera leaf (Task 2).** `compute_fog_cell_mask` currently signature `(&VisibleCells, &[u32], u32) -> u32`. Add `camera_leaf: Option<u32>`; after the OR-loop, `if let Some(cl) = camera_leaf { active |= masks.get(cl as usize).copied().unwrap_or(0); }`.

**Light influences (Task 3).** `update_dynamic_light_slots` (signature at `render/mod.rs:2456`) already accepts `light_influences: &[LightInfluence]` but the call at `:2696` passes `&[]`. Find the renderer field that owns `LightInfluence` data (look for the producer of the `light_influences` storage buffer in the lighting pass) and pass that slice through. If the data is computed inside `render_frame_indirect`, hoist it above the call.

**Depth min-over-block (Task 5).** WGSL pseudocode:

```wgsl
let ps_x = depth_dims.x / out_dims.x;
let ps_y = depth_dims.y / out_dims.y;
let base = vec2<u32>(gid.x * ps_x, gid.y * ps_y);
var depth_ndc: f32 = 1.0;
for (var dy: u32 = 0u; dy < 8u; dy = dy + 1u) {
    if dy >= ps_y { break; }
    for (var dx: u32 = 0u; dx < 8u; dx = dx + 1u) {
        if dx >= ps_x { break; }
        depth_ndc = min(depth_ndc, textureLoad(depth_texture,
            vec2<i32>(vec2<u32>(base.x + dx, base.y + dy)), 0));
    }
}
```

Upper bound `8` matches the `fog_pixel_scale` max (1–8 per `build_pipeline.md` table).

**Sticky activation (Task 10).** `repack_active` runs once per frame in `render_frame_indirect`. Pass the renderer's existing frame counter (or read from `FogParams.frame_index` if exposed) into `repack_active`. Hysteresis tunable as a const; 8 frames at 60 Hz ≈ 130 ms — long enough to mask single-frame portal narrowings, short enough that a genuinely-occluded volume drops out before the user notices.

**Affected files (summary):**
- `crates/postretro/src/visibility.rs` — split return shape (Task 1)
- `crates/postretro/src/main.rs` — visibility callsite + thread new list (Task 1)
- `crates/postretro/src/render/mod.rs` — `compute_fog_cell_mask` signature, `update_dynamic_light_slots` mask + influences, `render_frame_indirect` plumbing (Tasks 1, 2, 3, 10)
- `crates/postretro/src/render/fog_pass.rs` — `set_canonical_volumes` epsilon, `repack_active` hysteresis (Tasks 6, 10)
- `crates/postretro/src/shaders/fog_volume.wgsl` — depth min-block, start_t (Tasks 5, 7)
- `crates/postretro/src/prl.rs` — mask-length validation, `find_leaf_with_hint` (Tasks 4, 8)
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — f64 timing (Task 9)

## Open questions

- **Light-influence source.** Task 3 assumes the renderer already owns a `LightInfluence` slice; needs a quick audit to confirm where it lives and whether hoisting it to the `update_dynamic_light_slots` callsite is straightforward. If the data isn't readily available pre-call, this task expands to plumb it through.
- **Hysteresis tuning (Task 10).** `HYSTERESIS = 8` frames is a starting guess. May need to bump if testers see single-frame drops on faster cameras, or shrink if stale volumes cause visible "ghost fog" past portal closures. Decide during implementation review.
- **Per-volume vs global epsilon (Task 6).** 1 mm is a uniform inflation. If any author-facing fog volume is authored at sub-mm precision this is fine; verify against the FGD's documented density/radius scales in `build_pipeline.md`.
- **Hinted leaf lookup applicability (Task 8).** Currently scoped to fog-mask camera-leaf lookups. Should reverb-zone or fog-density-at-camera queries also use the hinted form? Decide during implementation; default is "no" unless visible artifacts appear.
