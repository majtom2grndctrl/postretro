# Spec Light Portal Culling

## Goal

Static spec lights bleed specular highlights through solid geometry because chunk spec light list assignment uses spatial overlap only. Filter assignments by portal reachability: a spec light is only assigned to chunks whose BSP leaf is connected to the light's source leaf via the portal graph. No geometry in a different room — separated by solid walls with no portal connection — receives specular from that light.

## Scope

### In scope

- `prl-build`: extend `ChunkLightListInputs` with BSP tree and portal list (both already in scope at the call site — not yet threaded)
- `prl-build`: derive a leaf adjacency map from the portal list inside the chunk bake
- `prl-build`: for each spec light, locate its source leaf via point-location in the BSP tree
- `prl-build`: BFS through the portal adjacency graph from the source leaf to build a reachable-leaf set; gate chunk assignment on membership
- Fallback: lights whose origin lands in a solid or exterior leaf skip the portal filter and use the existing spatial + BVH path
- Fallback: chunks whose centroid lands in a solid or exterior leaf (wall bisects the chunk) skip the portal filter for that chunk and use the existing spatial + BVH path
- `forward.wgsl`: add an `NdotL > 0` back-face guard on the static spec loop. `blinn_phong` uses `NdH` not `NdotL`, so geometrically back-facing lights can still produce highlights without this guard. Cheap and complementary to the portal cull.

### Out of scope

- Other runtime shader changes — `forward.wgsl` only gains the targeted NdotL guard; no other modifications
- Dynamic light specular — dynamic lights use shadow maps (spots) or influence spheres; unrelated path
- PVS — not used; runtime uses portal traversal
- Range-capped BFS — the spatial range check already gates the final per-light assignment; unconstrained portal BFS is correct

## Acceptance criteria

- [ ] A spec light in one room produces no specular on geometry in a non-adjacent room separated by solid geometry with no portal connection
- [ ] A spec light visible through an open portal still contributes specular to geometry in the adjacent cell
- [ ] Lights whose origin is in a solid or exterior leaf compile without error; their assignment falls back to the existing spatial + BVH path (no regression, no panic)
- [ ] `prl-build` on `assets/maps/test.map` exits zero (regression)
- [ ] `prl-build` on `assets/maps/test-3.map` exits zero
- [ ] `cargo check -p postretro-level-compiler` passes clean

## Tasks

### Task A: Thread BSP tree, portal list, and exterior leaf set into the chunk bake

Add `tree: &BspTree`, `portals: &[Portal]`, and `exterior_leaves: &ExteriorLeaves` to `ChunkLightListInputs`. Wire them at the call site in `main.rs` — all three are already bound there (`result.tree`, `generated_portals`, `exterior_leaves`). No logic changes yet; this task only extends the struct and the call site.

### Task B: Portal-reachability filter in the chunk bake

Inside `bake_chunk_light_list()`:

1. **Build adjacency map.** For each portal, record both directed edges (`front_leaf → back_leaf` and `back_leaf → front_leaf`). Result: `HashMap<usize, Vec<usize>>`.
2. **Locate each light's source leaf.** Call `find_leaf_for_point(tree, light.origin)` (re-exported from `partition`). If the returned leaf is solid or exterior, mark the light as unlocated → skip portal filter for that light.
3. **BFS per light.** From the source leaf, flood-fill through the adjacency map. Skip expansion into exterior leaves. Collect all reachable non-exterior leaf indices into a `HashSet<usize>`.
4. **Gate chunk assignment.** For each candidate chunk, compute its center point and call `find_leaf_for_point` to determine its leaf. If the leaf is not in the reachable set, skip the chunk. Existing spatial (range sphere) and BVH occlusion checks follow for accepted candidates.

## Sequencing

**Phase 1 (sequential):** Task A — threading is a prerequisite for Task B.
**Phase 2 (sequential):** Task B — consumes the inputs threaded in Task A.

## Rough sketch

Adjacency extraction (O(portal count)):

```rust
// Proposed design — remove after implementation
let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
for p in inputs.portals {
    adjacency.entry(p.front_leaf).or_default().push(p.back_leaf);
    adjacency.entry(p.back_leaf).or_default().push(p.front_leaf);
}
```

BFS per light:

```rust
// Proposed design — remove after implementation
let source = find_leaf_for_point(inputs.tree, light.origin);
let mut reachable = HashSet::from([source]);
let mut queue = VecDeque::from([source]);
while let Some(leaf) = queue.pop_front() {
    for &neighbor in adjacency.get(&leaf).into_iter().flatten() {
        if reachable.insert(neighbor) {
            queue.push_back(neighbor);
        }
    }
}
```

Chunk center leaf check:

```rust
// Proposed design — remove after implementation
let center = chunk_aabb_center(cell_x, cell_y, cell_z, &grid_params);
let chunk_leaf = find_leaf_for_point(inputs.tree, center);
if !reachable.contains(&chunk_leaf) {
    continue; // portal-culled
}
// existing range-sphere and BVH checks follow
```

The `find_leaf_for_point` call for each chunk center is the inner loop. For typical cell sizes (1–2 m) and moderate map extents, BSP depth is ≤ ~30 levels — cheap.

## Open questions

(none)
