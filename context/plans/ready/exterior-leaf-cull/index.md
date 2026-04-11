# Exterior Leaf Culling

> **Status:** ready
> **Depends on:** none. Operates entirely within the level compiler pipeline; no engine changes required.
> **Related:** `context/lib/build_pipeline.md` §Compiler pipeline · `postretro-level-compiler/src/main.rs` · `postretro-level-compiler/src/geometry.rs` · `postretro-level-compiler/src/visibility/mod.rs`

---

## Context

When flying outside a test map, brush exterior surfaces are visible. They were previously invisible. The cause: the compiler packs geometry for all empty BSP leaves, including those that are outside the map boundary (the "void"). Doom 3's dmap compiler eliminates this geometry at compile time via a flood-fill from the void — leaves reachable from outside the map are marked exterior, and their geometry is excluded from the packed output.

This plan re-adds that step.

---

## Goal

After compilation, any BSP leaf reachable from outside the map's sealed geometry produces no rendered faces. The player, when standing inside the map and looking at a wall, never sees brush exterior surfaces. If the player escapes the map entirely (free-fly camera), no geometry renders — the scene is empty.

---

## Approach

### Pipeline reorder

The current `main.rs` pipeline extracts geometry before generating portals. Exterior culling requires the portal adjacency graph, and `BspLeavesSection` encoding must happen after the exterior set is known (it reads face counts from the BSP tree). The reorder separates portal generation and BSP encoding so the exterior flood-fill can run between them:

```
Before: parse → CSG → partition → geometry → [portal gen + BSP encode] → pack
After:  parse → CSG → partition → portal gen → exterior flood-fill → [BSP encode + geometry] → pack
```

Portal generation only reads the BSP tree, so moving it earlier has no dependencies. BSP/leaf encoding moves after the flood-fill so `encode_leaves_and_pvs` receives the exterior set and can emit zero face counts for exterior leaves.

### Step 1 — find_leaf_for_point

`find_leaf_for_point(tree: &BspTree, point: DVec3) -> usize` already exists as a test helper at `partition/bsp.rs:770`. Promote it to a `pub` function in the main module body. No logic changes needed.

The sign convention: `dot(point, node.plane_normal) - node.plane_distance >= 0.0` → front child; negative → back child. (Zero goes to front, matching the existing helper.)

### Step 2 — find_exterior_leaves

Add `find_exterior_leaves(tree: &BspTree, portals: &[Portal]) -> HashSet<usize>` in `visibility/mod.rs`.

1. Compute the AABB over all leaf bounds in the tree.
2. Use a point just outside that AABB (`max + DVec3::splat(1.0)`) as the void probe point.
3. Call `find_leaf_for_point` to get the void seed leaf.
4. **Solid seed guard:** if the seed leaf is solid, the probe landed inside a brush at the map boundary. Log `[Compiler] WARNING: void probe landed in a solid leaf — exterior leaf culling skipped` and return an empty set. Culling is a no-op; geometry is unchanged.
5. Build a portal adjacency list: `adjacency[leaf_idx]` = list of neighbor leaf indices from `portal.front_leaf` / `portal.back_leaf`.
6. BFS flood-fill from the seed through the adjacency list. Any leaf reachable from the seed is exterior. Return the set.

Log `[Compiler] Exterior flood-fill: {} exterior leaves, {} interior empty leaves`.

**Leak handling.** If `exterior_count > 0` and `interior_empty_count == 0`, all empty leaves were classified exterior — the map has no sealed interior or has a full leak. Log `[Compiler] WARNING: no interior empty leaves remain after exterior culling — map may be unsealed or have a leak`. No hard error; the compiler produces a valid (if geometry-free) .prl. Leak detection and repair is the author's responsibility.

**Zero-portal edge case.** If `portals` is empty (single-leaf map, all-solid map), the BFS cannot spread past the seed. If the seed is an empty leaf, it is the only leaf classified exterior. The plan does not special-case this further; for any real map that contains navigable space and portals this path is unreachable.

### Step 3 — split build_portal_pvs and pass exterior set to encoding

`build_portal_pvs` currently generates portals and immediately encodes the `BspLeavesSection` (via `encode_leaves_and_pvs`), which reads `leaf.face_indices.len()` from the BSP tree. Encoding must move after the exterior set is known so exterior leaves get `face_count = 0`.

Split the function (or add a new entry point) so that:
- **Portal generation** produces the raw portals and any PVS data.
- **BSP/leaf encoding** accepts `&HashSet<usize>` of exterior leaves and zeroes out `face_count` for any leaf in that set.

The exact split is an implementation judgment call; the invariant is that `BspLeavesSection` face counts and `GeometrySectionV2` face ranges agree — both must reflect the exterior set.

### Step 4 — filter geometry

Modify `geometry::build_leaf_ordered_faces` to accept `&HashSet<usize>`. When iterating leaves, skip adding faces for exterior leaves but still increment the sequential empty-leaf counter. This preserves sequential leaf indices (BspLeaf section references remain valid) while producing empty face ranges for exterior leaves.

Update `geometry::extract_geometry` signature to accept `&HashSet<usize>` and thread it through to `build_leaf_ordered_faces`.

**Existing call sites.** All existing callers of `extract_geometry` (in `portals.rs`, `csg.rs`, `pack.rs`, and `geometry.rs` tests) pass `&HashSet::new()` to preserve current behavior.

### Step 5 — wire up in main.rs

```rust
// Target state after all changes:
let generated_portals = visibility::generate_portals(&result.tree);
let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);
let vis_result = visibility::encode_vis(&result.tree, &generated_portals, &exterior_leaves);
let geo_result = geometry::extract_geometry(&result.faces, &result.tree, &exterior_leaves);
```

The exact function names are an implementation detail; the invariant is that encoding and geometry extraction both receive the same exterior set.

---

## Scope

### In scope

- Promote `find_leaf_for_point` to `pub` in `partition/bsp.rs`
- `find_exterior_leaves` in `visibility/mod.rs`
- Split `build_portal_pvs` so BSP/leaf encoding accepts the exterior set
- Modified `build_leaf_ordered_faces` and `extract_geometry` signatures in `geometry.rs`
- Updated call sites in `portals.rs`, `csg.rs`, `pack.rs`, and `geometry.rs` tests (pass `&HashSet::new()`)
- Pipeline reorder and wiring in `main.rs`
- Log lines: exterior leaf count, solid-seed warning, zero-interior warning
- Unit tests: `find_leaf_for_point` on a trivial tree; `find_exterior_leaves` covering normal sealed map, solid seed guard, and zero-portal edge case

### Out of scope

- Engine changes — culling is compile-time only
- A `--no-exterior-cull` flag (useful future escape hatch; not needed for correctness)
- Leak-point output (the `.pts` file Quake tools emit to show authors where the leak is)
- Changing PRL section IDs or adding new sections
- Removing exterior leaves from the BspLeaves section entirely (remapping BSP tree child indices is more invasive than warranted; empty face ranges are sufficient)

---

## Files to modify

| File | Change |
|------|--------|
| `postretro-level-compiler/src/partition/bsp.rs` | Promote `find_leaf_for_point` from test helper to `pub` |
| `postretro-level-compiler/src/visibility/mod.rs` | Add `find_exterior_leaves`; split `build_portal_pvs` to separate encoding |
| `postretro-level-compiler/src/geometry.rs` | Filter exterior leaves in `build_leaf_ordered_faces` / `extract_geometry` |
| `postretro-level-compiler/src/main.rs` | Reorder pipeline, wire exterior set |
| `postretro-level-compiler/src/portals.rs` | Update `extract_geometry` call site |
| `postretro-level-compiler/src/csg.rs` | Update `extract_geometry` call site |
| `postretro-level-compiler/src/pack.rs` | Update `extract_geometry` call sites |

---

## Acceptance Criteria

1. Recompile `assets/maps/test.map` (or any test map). When the engine loads the resulting .prl and the free-fly camera exits the map, no geometry is visible.
2. From inside the map, all previously-visible interior geometry still renders correctly.
3. `cargo test -p postretro-level-compiler` passes. New tests cover `find_leaf_for_point` and `find_exterior_leaves` (including solid-seed guard and zero-portal edge cases).
4. `cargo test -p postretro` passes (engine-side tests unaffected).
5. Compiler log shows the exterior flood-fill line with a non-zero exterior leaf count for any real test map.
6. For each exterior leaf in the packed output, `BspLeafRecord.face_count == 0`. Verify in a compiler-side unit test by inspecting the encoded `BspLeavesSection` directly.
7. `BspLeavesSection` face ranges and `GeometrySectionV2` face ranges agree — no offset misalignment. Verify by loading the compiled .prl in the engine without crash or rendering artifact.
8. No `unsafe` blocks introduced.

---

## Open Questions

None. Ready for implementation.
