# Dynamic Light PVS Culling

> **Status:** ready
> **Depends on:** `lighting-spot-shadows/` (shipped — `SpotShadowPool::rank_lights` already accepts a `_visible_cell_bitmask: &[bool]` slot). `lighting-dynamic-flag/` (shipped — `MapLight.is_dynamic` available).
> **Related:** `context/lib/build_pipeline.md` §Runtime visibility · `context/lib/rendering_pipeline.md` §4 · `context/plans/done/lighting-spot-shadows/index.md` Task A · `context/plans/drafts/perf-anti-penumbra-pvs/index.md` (compile-time PVS tightening — independent and complementary).

---

## Context

`SpotShadowPool::rank_lights` (`postretro/src/lighting/spot_shadow.rs:241`) was designed to accept a per-light visibility bitmask and use it as a pre-filter before the influence-volume frustum check and the heuristic ranking. The signature already names a `_visible_cell_bitmask: &[bool]` parameter, but it is prefixed `_` and never consulted. The render-loop call site (`postretro/src/render/mod.rs:1598`–`1601`) carries the placeholder:

```rust
// For now, we don't have per-frame visibility (visible_cell_bitmask).
// Use a dummy bitmask that considers all lights visible.
// This will be replaced with actual visibility data in Task B.
let dummy_visible = vec![true; self.level_lights.len()];
```

Meanwhile the engine already determines, per frame, the exact set of cells (BSP leaves) visible from the camera. `determine_visible_cells` (`postretro/src/visibility.rs:523`) returns a `VisibleCells::Culled(Vec<u32>)` derived either from runtime portal traversal (`postretro/src/portal_vis.rs::portal_traverse`, the primary path) or from the precomputed PVS bitset (`--pvs` builds), with frustum-cull fallbacks for solid-leaf / exterior / empty-world cases. Cell ids equal BSP leaf indices in the current compiler.

A dynamic spot light whose origin sits in a cell the camera cannot reach contributes nothing visible this frame, but the renderer:

1. Includes it in the heuristic ranking, possibly displacing a *visible* light from a shadow slot (`SHADOW_POOL_SIZE = 8`).
2. Renders a depth pass for it if it wins a slot (`render/mod.rs:1797`–`1828`).
3. Iterates it in the per-fragment dynamic light loop on every shaded pixel.

The shadow-slot displacement is the headline cost: an off-camera muzzle flash in another room can steal the slot from a flashlight pointing at the player. The depth pass and per-fragment iteration are secondary, but free once the bitmask is wired.

The piece that is missing is the bridge between `VisibleCells` (cell ids visible from the camera) and `&[bool]` indexed by **light** index that `rank_lights` expects. To build it, every light needs a known cell id. Today nothing in the runtime knows which leaf a light's origin sits in: `MapLight` (`postretro/src/prl.rs:127`) carries `origin: [f64; 3]` and no leaf assignment, and the AlphaLights wire format (`postretro-level-format/src/alpha_lights.rs::AlphaLightRecord`) is a flat 67-byte record with no leaf field.

The compiler already knows the BSP and has the lookup helper: `find_leaf_for_point(&BspTree, DVec3) -> usize` (`postretro-level-compiler/src/partition/bsp.rs:17`), used today by `sh_bake.rs` and `visibility/mod.rs`. Compile-time assignment is therefore the natural choice — the engine never has to walk the BSP for lights, and the assignment is stable across runs.

---

## Goal

Per-frame: shadow-slot ranking and the dynamic light loop see only lights whose origin cell is in the current visible-cell set. The dummy-bitmask placeholder is removed. With N dynamic spot lights placed in cells unreachable from the camera, exactly zero of them receive a shadow slot and zero depth passes are rendered for them. With the camera in their cell, slot allocation matches today's behavior.

---

## Approach

Three tasks. Task A bakes a per-light leaf index into the PRL at compile time and parses it at load time (one new field on `MapLight`). Task B exposes the per-frame visible-cell set to the renderer and wires it through `update_dynamic_light_slots` into `rank_lights` — B reads `MapLight.leaf_index`, so it depends on A landing first. Task C un-stubs `rank_lights` to consume the bitmask and adds tests, and depends on B.

```
A (compiler + format + loader) ─► B (renderer wiring) ─► C (rank_lights body + tests)
```

All three should land together — pre-release policy, no compat shims.

---

## Task A — Bake light cell index at compile time

**Crates:** `postretro-level-format`, `postretro-level-compiler` · **Files:** `postretro-level-format/src/alpha_lights.rs`, `postretro-level-compiler/src/pack.rs`, optionally `postretro-level-compiler/src/main.rs` for log lines.

1. **Format change.** Extend `AlphaLightRecord` (`postretro-level-format/src/alpha_lights.rs`) with a `leaf_index: u32` field. Append it to the on-disk record after `cast_shadows`. Update `ALPHA_LIGHT_RECORD_SIZE` from 67 to 71 bytes. Update `to_bytes` / `from_bytes` and the offset arithmetic in `from_bytes` (read at `o + 67..o + 71`). Update the round-trip tests at lines 222–322 to round-trip the new field. Pre-release policy: bump nothing — older `.prl` files are simply re-baked.

   **Sentinel.** Reserve `u32::MAX` as "unassigned / cannot determine leaf". The compiler emits this for lights whose origin lands in a solid leaf — a map authoring error. Runtime culls these lights and emits a `warn!` at load time naming the light index and origin. Solid-leaf lights cannot illuminate visible geometry; hiding the error with an always-visible fallback lets bad maps ship.

2. **Compiler bake.** In `postretro-level-compiler/src/pack.rs::encode_alpha_lights`, accept the BSP tree as a second parameter. The existing `.filter(|l| !l.bake_only).map(...)` chain determines which lights get emitted — record index `i` in the output aligns with the filtered subset, not with `map_data.lights[i]`. The leaf lookup must happen inside this same chain. For each non-bake-only light: call `partition::find_leaf_for_point(tree, light.origin)` to get a `usize` index `idx`. Check `tree.leaves[idx].is_solid`; if true, emit `leaf_index = u32::MAX` and a `warn!` naming the light's origin and index in the array (no entity name is plumbed today). Otherwise emit `leaf_index = idx as u32`.

3. **Threading.** `encode_alpha_lights` is called once in `main.rs` (line 252); the resulting section is passed by reference to whichever of `pack_and_write_pvs` / `pack_and_write_portals` runs. Add `tree: &BspTree` as a second argument to `encode_alpha_lights`. The two pack helpers receive the already-encoded section and don't need to change. There are also two test call sites in `pack.rs` (lines 779 and 854) that call `encode_alpha_lights` directly — update both to pass a `BspTree` fixture (an empty tree suffices: returns leaf 0, which is non-solid).

4. **Logging.** Extend the existing `AlphaLights: N bytes (M lights)` log line in `pack.rs` (lines 216–218 and 361–363) with `, K assigned to leaves, J unassigned`. This is the primary diagnostic for noticing when a map has lights stuck in solid geometry.

### Task A acceptance gates

- A `.prl` produced by the new compiler round-trips through `AlphaLightsSection::to_bytes` / `from_bytes` preserving `leaf_index` for every light. Verified by an updated `round_trip_multiple_records` test.
- On `assets/maps/test.prl` and `assets/maps/occlusion-test.map` rebuild, the bake-time log line reports zero unassigned lights (any non-zero unassigned count names a real authoring bug to investigate before landing).
- A synthetic test light placed inside a brush volume produces `leaf_index = u32::MAX` and a `warn!`.

---

## Task B — Expose visible-cell set to the renderer and wire it through

**Crate:** `postretro` · **Files:** `postretro/src/prl.rs`, `postretro/src/render/mod.rs`, `postretro/src/main.rs`.

1. **Runtime field.** Add `pub leaf_index: u32` to `MapLight` (`postretro/src/prl.rs:127`). Populate it in `convert_alpha_lights` (`prl.rs:265`) from the new wire field. The compiler-side `MapLight` (`postretro-level-compiler/src/map_data.rs:185`) does **not** need the field — the assignment happens at pack time, not in the canonical map representation. Same is true of the chunk-light-list bake and SH bake, which both already use `find_leaf_for_point` directly when they need a leaf.

2. **Plumb the visible-cell set into the renderer.** `main.rs::redraw` already holds the `VisibleCells` result from `determine_visible_cells`. Convert it to a `&[bool]` indexed by leaf there, before calling `render_frame_indirect`. Pass that slice into `update_dynamic_light_slots` — the renderer never needs to know about `VisibleCells` as a type.

   Conversion in `main.rs::redraw`:
   - `VisibleCells::DrawAll`: pass `&[]` as the leaf bitmask — the empty slice is the explicit DrawAll sentinel recognized by `update_dynamic_light_slots`.
   - `VisibleCells::Culled(cell_ids)`: allocate a `Vec<bool>` of length `self.leaf_count`, seed `false`, set each `cell_ids[i]` to `true`.

   Inside `update_dynamic_light_slots`, build the per-light `Vec<bool>` of length `self.level_lights.len()`:
   - **If the leaf-bool slice is empty** (DrawAll sentinel): set `true` for every light whose `leaf_index != u32::MAX`, `false` for solid-leaf lights. DrawAll means "don't cull by visibility" — but solid-leaf lights stay culled on all paths.
   - **Otherwise** (leaf bitmask populated): for each light, `true` if `leaf_index < slice.len() && slice[leaf_index as usize]`; `false` if `leaf_index == u32::MAX`; `false` otherwise.

3. **Renderer needs `leaf_count`.** `leaf_count` is stable per loaded level — same kind of per-level state as `level_lights`. Add `leaf_count: u32` to `Renderer`, set at construction via `LevelGeometry` (extend `LevelGeometry` with `leaf_count: u32`, populated from `world.leaves.len()` at the `main.rs` call site). Do not pass it as a per-frame function argument.

4. **Eliminate the dummy bitmask comment block** at `render/mod.rs:1598`–`1601`. Replace with the construction described in step 2.

### Task B acceptance gates

- `Renderer::update_dynamic_light_slots` no longer constructs a "dummy" bitmask; the placeholder comment is gone. Verified by `grep -r 'dummy_visible' postretro/src/` returning no hits.
- On a level with lights spread across multiple cells, with `RUST_LOG=debug`, the existing `[ShadowPool] light I → slot S` log lines (already present in `rank_lights`) only mention lights whose `leaf_index` is in the current frame's visible cell set. Verified by walking the camera into and out of side rooms with dynamic spots in them.
- `VisibleCells::DrawAll` (empty world or fallback) keeps every light eligible — slot allocation matches the dummy-bitmask behavior on those paths exactly. `leaf_index == u32::MAX` lights are culled on all paths (confirmed by placing a light inside a brush and verifying no slot is assigned).

---

## Task C — Un-stub `rank_lights` and lock the behavior in tests

**Crate:** `postretro` · **Files:** `postretro/src/lighting/spot_shadow.rs`.

1. **Body change.** In `SpotShadowPool::rank_lights` (`spot_shadow.rs:241`), drop the `_` prefix on `visible_cell_bitmask` and add an early-out at the top of the `filter_map`'s closure, immediately after the `is_dynamic && Spot` test:

   ```rust
   if idx < visible_cell_bitmask.len() && !visible_cell_bitmask[idx] {
       return None;
   }
   ```

   When the bitmask is shorter than the light list (defensive: covers the path-not-yet-loaded case during init), treat the light as visible. The renderer always passes a full-length bitmask in steady state.

2. **Remove `#[allow(dead_code)]`** above `rank_lights` (line 240) — the parameter is now live.

3. **Tests.** Add to the `tests` module (`spot_shadow.rs:314`):
   - `lights_in_invisible_cells_are_culled`: 3 dynamic spots, bitmask `[true, false, true]`, assert the middle light gets `NO_SHADOW_SLOT` and the other two each get a slot.
   - `nine_lights_with_eight_visible_assigns_eight`: 9 lights, bitmask with one light marked invisible, assert all 8 visible lights get slots and the invisible one is `NO_SHADOW_SLOT` regardless of heuristic score (specifically: place the invisible light closer than all others so it would otherwise rank #1).
   - `empty_bitmask_treated_as_all_visible`: pass `&[]` and confirm behavior matches today's tests.

   Update the existing `nine_lights_eight_assigned_one_unshadowed` and other tests that pass `&[]` for the bitmask — they continue to use `&[]` and the empty-bitmask defensive-fallback path keeps them green.

### Task C acceptance gates

- `cargo test -p postretro lighting::spot_shadow` passes including the three new tests.
- `cargo clippy --workspace -- -D warnings` clean (no `dead_code` allow needed).

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro-level-format/src/alpha_lights.rs` | A | Add `leaf_index: u32` to `AlphaLightRecord`; bump `ALPHA_LIGHT_RECORD_SIZE` to 71; update read/write/round-trip tests (lines 222–322) |
| `postretro-level-compiler/src/pack.rs` | A | `encode_alpha_lights` takes `tree: &BspTree`; calls `find_leaf_for_point` + checks `is_solid` per light inside the existing filter-map chain; emits `u32::MAX` + warn for solid-leaf origins; extend log line; update two test call sites (lines 779, 854) to pass an empty `BspTree` fixture |
| `postretro-level-compiler/src/main.rs` | A | Pass `&result.tree` into `encode_alpha_lights` at line 252 |
| `postretro/src/prl.rs` | B | Add `leaf_index: u32` to `MapLight`; populate in `convert_alpha_lights`; update round-trip tests (lines 1223–1250) for the new field |
| `postretro/src/render/mod.rs` | B | Change `update_dynamic_light_slots` signature to take `&[bool]` (pre-built per-light bitmask); build per-light bitmask from `level_lights[i].leaf_index`; remove `dummy_visible` block; add `leaf_count: u32` field to `Renderer` and to `LevelGeometry` |
| `postretro/src/main.rs` | B | Convert `VisibleCells` to `Vec<bool>` after `determine_visible_cells`, before `render_frame_indirect`; pass the slice into `update_dynamic_light_slots`; extend `LevelGeometry` construction (line 285) with `leaf_count` from `world.leaves.len()` |
| `postretro/src/lighting/spot_shadow.rs` | C | Un-stub `visible_cell_bitmask` (drop `_`, drop `#[allow(dead_code)]`, add early-out in `filter_map`); add three tests |

---

## Acceptance Criteria

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A, B, and C acceptance gates above.
5. **Behavioral gate** — manual: on a test map with at least two dynamic spot lights placed in cells separated from the camera by closed portals (no sightline), no `[ShadowPool] light I → slot S` log line is emitted for those lights when the camera is in the player room. Walking the camera through the portal so the side rooms become visible causes the slot allocation to update on the next frame. Quote the exact log lines in the PR description.
6. **Format-bump confirmation:** an attempt to load a `.prl` produced by the pre-A compiler against the post-A engine fails fast at the `AlphaLights` parse step with an unambiguous error (record-size mismatch surfaces as `truncated` or short-record error from the existing length checks). Re-baking with the new compiler resolves it. Pre-release policy — no compat shim, all consumers fixed in the same pass. Verified by rebuilding `assets/maps/test.prl` and `assets/maps/occlusion-test.map` in the same change.
7. The `_visible_cell_bitmask` parameter no longer carries the `_` prefix in `postretro/src/`; verified by `grep -r '_visible_cell_bitmask' postretro/src/` returning no hits.

---

## Out of scope

- Any compile-time PVS tightening. `perf-anti-penumbra-pvs/` is a separate draft and a different stage of the pipeline. This plan consumes whatever cell visibility the runtime already produces; it does not change what cells are considered visible.
- Per-light **influence-bound** culling against the visible set. A light whose origin lies in a non-visible cell but whose influence sphere reaches into a visible cell is still culled here — the cone or sphere may still legitimately illuminate visible geometry through a thin wall or near-miss portal. This is acceptable for v1 (dynamic lights are short-range and authored carefully). A follow-up plan can extend the per-light bitmask to "any cell touched by the light's influence sphere is visible" using a baked per-light cell list (mirrors `chunk_light_list_bake.rs` shape). Not done here.
- Per-light **influence-cell list** baking. The simpler "origin cell" assignment is enough to fix the headline shadow-slot displacement bug. Influence-cell lists are a strict generalization and should be a separate plan.
- Cell tracking for **moving** dynamic lights (Milestone 6+ entity system). Today all lights come from FGD entities at fixed positions; `leaf_index` is baked at compile time. When dynamic entities can move, runtime cell re-lookup will be needed — that infrastructure is part of the entity system, not this plan.
- The dynamic light **per-fragment loop** culling (chunk-light-list-style). Today's chunk-light-list spec buffer covers static lights only; dynamic lights are iterated in full each frame. Bringing dynamic lights into the chunk grid is a separate optimization.
- Removing the BVH-based per-shadow-slot draw-list culling. Each shadow slot still draws all static geometry; per-slot culling is mentioned as a future optimization in `lighting-spot-shadows/` Task B and stays there.

---

## Open Questions

None. All resolved:

- **`leaf_index = u32::MAX` — always visible or always culled?** Cull it. A light in solid geometry is a map error; the load-time `warn!` is the feedback. Silently rendering it lets bad maps ship.
- **`leaf_count` — `Renderer` field vs. function arg?** `Renderer` field. Leaf count is stable per loaded level, same as `level_lights`. Per-frame function args are for per-frame data.
- **`&VisibleCells` vs. pre-built `&[bool]`?** Pre-built `&[bool]` in `main.rs`. Convert once at the call site; pass simple data to the renderer. Renderer has no reason to know about `VisibleCells`.
