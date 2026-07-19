# E10 — Navmesh Capsule Clearance (Euclidean erosion + funnel corner-offset)

> **Conditional — do not build until a gate fires.** This is the pre-scoped "Option B" that `E10--enemy-stuck-recovery` (post-implementation escalation gate) and `E10--enemy-combat-positioning` (navmesh-clearance escalation, Open questions) both defer to. It exists so that when a gate fires, the decision is a promotion, not a research project. Entry gate: route-valid wedge failures or visible wall-hugging persist on the pillar/wedge fixture after tangent recovery lands, or combat positioning selects navmesh-reachable points the capsule cannot physically occupy.
>
> **Wave:** E10 enemy-AI follow-up. Runs after `E10--enemy-stuck-recovery` (its fixture and detection signal are this spec's validation instruments).

## Goal

Make routes physically traversable by construction: erode the walkable grid by the agent radius as a Euclidean disk instead of a Chebyshev box, and inset funnel waypoints off portal endpoints by the agent radius — so no waypoint sits within the capsule radius of static geometry and corner wedges stop occurring at the source. Stuck recovery remains as a backstop, not the primary defense.

## Background (the cause)

- **Erosion is a cell-quantized Chebyshev box.** `erode` (`crates/level-compiler/src/navmesh_bake.rs:406`) computes `radius_cells = (agent_radius / cell_size).ceil()` (line 407) and `near_boundary` (line 466) tests `dx < radius_cells && dz < radius_cells` (line 470) — an axis-aligned box, not a disk, quantized to whole cells. Clearance is anisotropic (diagonal vs axis) and cell-granular: a cell whose center clears the radius but whose area does not stays walkable, leaving sub-cell slivers at pillar corners where the capsule still contacts geometry.
- **Funnel waypoints are exactly portal endpoints.** `funnel` (`crates/postretro/src/nav/path.rs:244`) commits interior waypoints with `path.push(left)` / `path.push(right)` (lines 271, 289) — the raw `gate_left`/`gate_right` portal endpoints, which the bake aligns to the cell lattice (`navmesh_bake.rs:722-731`). A waypoint at a portal endpoint can sit flush against a wall corner; an agent steering to its center grazes the corner and `collide_and_slide` consumes the velocity — the wedge `E10--enemy-stuck-recovery` detects and recovers from.
- **Bake and runtime clearance.** `NavParams::agent_radius` flows through the serialized navmesh into runtime agent parameters. Offline erosion uses that physical radius. Runtime funnel clearance uses `agent_radius + SKIN_DISTANCE`, matching capsule sweeps that stop one skin width before contact. This runtime margin does not change bake erosion or serialized PRL data.

## Scope

### In scope

- **Euclidean erosion.** Replace the Chebyshev `near_boundary` test with a Euclidean-disk test at the same grid resolution, conservatively quantized: a cell is eroded when any point of the cell lies within `agent_radius` of a boundary column (equivalently, test cell-center distance against `agent_radius + half cell diagonal`). Boundary detection (`is_boundary_span`) is unchanged; the erosion floor-match in `near_boundary` is aligned to `step_height + STEP_EPS` to match `is_boundary_span`'s existing tolerance (previously a bare `step_height` compare, which could leave a wall-adjacent cell walkable at the float boundary).
- **Funnel corner-offset.** Pre-inset portal gates by the effective runtime clearance (`agent_radius + SKIN_DISTANCE`) in XZ before string-pulling. Bevel incident segments that would cut the endpoint clearance disk. A portal with horizontal width less than or equal to `2 × effective clearance` yields its midpoint. Start and goal are unchanged. Y remains interpolated along the original portal segment.
- **Stage-version bump.** `NAVMESH_STAGE_VERSION` 2 → 3 (`navmesh_bake.rs:15`), forcing a navmesh re-bake of all cached maps on next build. No PRL body-layout change: regions and portals serialize identically, so `NAVMESH_VERSION` (`crates/level-format/src/navmesh.rs:81`) stays 1 and previously compiled `.prl` files still parse.
- **Pre-split of the bake file.** `navmesh_bake.rs` is 1243 lines (tests in-file from ~line 785). Split before extending, per the split-before-extend rule.
- **Fixture validation.** Waypoint-clearance and no-stuck assertions on the concave-corner fixture built by `E10--enemy-stuck-recovery`, plus a `campaign-test` re-bake sanity pass.

### Out of scope

- Grid resolution / `cell_size` changes; sub-cell (distance-field) walkability.
- Per-archetype agent radii — the navmesh stays baked for the one canonical agent.
- Runtime dynamic clearance queries, corridor re-derivation, ORCA/RVO.
- Removing stuck recovery — tangent recovery stays as the backstop for cases clearance cannot prove (dynamic agents, movers).
- PRL format changes (`NAVMESH_VERSION` bump) — not needed; the serialized surface is unchanged.

## Acceptance criteria

- [ ] Every interior waypoint and path segment returned by `find_path` on the concave-corner fixture clears static collision geometry by at least the effective runtime clearance, within test epsilon. Parry-direct mesh queries verify the route. Midpoints from portals at or below the narrow threshold are exempt. The fixture navmesh is hand-built, so erosion clearance rides on AC3 plus the `campaign-test` re-bake pass.
- [ ] An agent following a funnel path around the pillar-corner fixture completes the route without stuck detection firing — goal-projected progress stays above the stuck epsilon for the whole traversal (runnable integration test reusing the stuck-recovery fixture and detection signal).
- [ ] Erosion clearance is isotropic to within one cell: measured eroded distance from a straight wall and from the same wall rotated 45° differ by at most one cell, and neither exceeds `agent_radius` by more than one cell (runnable unit test on synthetic grids — the upper bound rejects a coarser ceil'd-integer disk test that would pass isotropy alone while over-eroding).
- [ ] A portal whose XZ width is less than or equal to twice the effective runtime clearance yields its midpoint — no NaN or oscillating offset. Unequal endpoint Y does not affect width classification.
- [ ] Bumping the stage version invalidates only navmesh cache entries: the existing build-stage-cache tests remain green (each stage's cache key folds `NAVMESH_STAGE_VERSION`, so 2→3 invalidates navmesh while other stages keep their keys — the "re-bakes all maps" behavior is a construction/review gate, not a runnable pipeline test). The unchanged serialized surface is verified by the existing navmesh encode/decode round-trip tests under `NAVMESH_VERSION 1` (no committed `.prl` fixture exists to load).
- [ ] Existing A*, funnel, region-decomposition, and steering tests remain green, updated only where a route legitimately changes because clearance moved a waypoint.

## Tasks

### Task 1: Pre-split navmesh_bake.rs
Behavior-preserving split: move the `#[cfg(test)]` module (from ~line 785) to a sibling `navmesh_bake/tests.rs` (or extract the rasterize/erode pass into a submodule — whichever seam is cleaner), leaving production code under the ~800-line threshold before Task 2 extends it. No logic changes; all tests stay green.

### Task 2: Euclidean erosion + stage bump
In `erode` (`navmesh_bake.rs`), replace the Chebyshev `near_boundary` proximity test with the conservative Euclidean-disk test (cell eroded when any point of it is within `agent_radius` of a boundary column; floor-matching via the existing `step_height` neighbor test). `near_boundary` currently receives only the ceil'd `radius_cells: i64` and works in integer cell coords — thread the float `agent_radius` (or `grid.cell_size`) from `erode` into it so the disk test measures real distance, not the lossy integer (both private, same file; `erode` is the sole caller). Bump `NAVMESH_STAGE_VERSION` to 3. Add the isotropy unit test (AC3), asserting both isotropy and the over-erosion upper bound.

### Task 3: Funnel corner-offset
In `crates/postretro/src/nav/path.rs`, pre-inset gates by the effective runtime clearance. Run all funnel state on those gates; never mix a raw apex with an emitted inset point. Add portal-side bevels where incident chords would enter the endpoint clearance disk. Base width and direction on XZ, including the inclusive narrow-midpoint threshold. Preserve portal Y through interpolation. Add raw-apex, segment-clearance, narrow-threshold, and unequal-Y regressions.

### Task 4: Fixture validation
The fixture is `ConcaveCorner`, a private `#[cfg(test)]` struct in `crates/postretro/src/agent_steering/tests.rs`. Reuse its collision world and nav graph for waypoint, segment-clearance, and strict no-stuck tests over the original route. Query its collision mesh directly because `CollisionWorld` exposes no clearance method. Re-bake `campaign-test`; record build diagnostics and whether the environment permits visual inspection. Keep `NAVMESH_VERSION` at 1 and rely on the existing encode/decode round trip.

## Sequencing

**Phase 1 (sequential):** Task 1 — file split; only Task 2 touches the split result. (Task 3 edits `path.rs`/`nav/mod.rs` and could begin in parallel with Task 1.)
**Phase 2 (concurrent):** Task 2 (bake crate), Task 3 (runtime crate) — independent files; each carries its own unit tests.
**Phase 3 (sequential):** Task 4 — consumes both Task 2's re-baked grid and Task 3's offset waypoints.

## Open questions

- **Narrow corridors on existing maps.** Conservative Euclidean erosion erodes slightly more than the current under-eroding quantization in some spots; a corridor authored at marginal width could close. Mitigation is the `campaign-test` re-bake check in Task 4; if a corridor closes, the map is authored too tight for the canonical agent and should be widened — not special-cased in the bake.
- **Combat-positioning interaction.** Clearance-correct waypoints make combat-position candidates near walls honestly unreachable, which `E10--enemy-combat-positioning` treats as a filter outcome, not an error. No coupling work needed; noted so the positioning spec's escalation gate references this draft.

## Verification note

Isolated re-bake completed successfully:

`CARGO_TARGET_DIR=/private/tmp/postretro-e10-navmesh-clearance-task4-bake cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o content/dev/maps/campaign-test.prl --no-tui`

Output was a 244 MB gitignored PRL. Warnings covered light defaults, coplanar material conflicts, and watertightness diagnostics. The headless environment could not support manual visual inspection for wall-hugging; runnable concave-corner clearance and no-stuck tests cover that behavior here.
