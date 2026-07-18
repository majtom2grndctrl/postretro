# E10 — Navmesh Capsule Clearance (Euclidean erosion + funnel corner-offset)

> **Conditional — do not build until a gate fires.** This is the pre-scoped "Option B" that `E10--enemy-stuck-recovery` (post-implementation escalation gate) and `E10--enemy-combat-positioning` (navmesh-clearance escalation, Open questions) both defer to. It exists so that when a gate fires, the decision is a promotion, not a research project. Entry gate: route-valid wedge failures or visible wall-hugging persist on the pillar/wedge fixture after tangent recovery lands, or combat positioning selects navmesh-reachable points the capsule cannot physically occupy.
>
> **Wave:** E10 enemy-AI follow-up. Runs after `E10--enemy-stuck-recovery` (its fixture and detection signal are this spec's validation instruments).

## Goal

Make routes physically traversable by construction: erode the walkable grid by the agent radius as a Euclidean disk instead of a Chebyshev box, and inset funnel waypoints off portal endpoints by the agent radius — so no waypoint sits within the capsule radius of static geometry and corner wedges stop occurring at the source. Stuck recovery remains as a backstop, not the primary defense.

## Background (the cause)

- **Erosion is a cell-quantized Chebyshev box.** `erode` (`crates/level-compiler/src/navmesh_bake.rs:406`) computes `radius_cells = (agent_radius / cell_size).ceil()` (line 407) and `near_boundary` (line 466) tests `dx < radius_cells && dz < radius_cells` (line 470) — an axis-aligned box, not a disk, quantized to whole cells. Clearance is anisotropic (diagonal vs axis) and cell-granular: a cell whose center clears the radius but whose area does not stays walkable, leaving sub-cell slivers at pillar corners where the capsule still contacts geometry.
- **Funnel waypoints are exactly portal endpoints.** `funnel` (`crates/postretro/src/nav/path.rs:244`) commits interior waypoints with `path.push(left)` / `path.push(right)` (lines 271, 289) — the raw `gate_left`/`gate_right` portal endpoints, which the bake aligns to the cell lattice (`navmesh_bake.rs:722-731`). A waypoint at a portal endpoint can sit flush against a wall corner; an agent steering to its center grazes the corner and `collide_and_slide` consumes the velocity — the wedge `E10--enemy-stuck-recovery` detects and recovers from.
- **One radius, threaded end to end.** `NavParams::agent_radius` (worldspawn `nav_agent_radius` KVP, default 0.4 — `crates/level-compiler/src/map_data.rs:589,592,612`) → serialized `NavMeshSection::agent_radius` (`navmesh_bake.rs:138`) → `NavAgentParams::radius` (`crates/postretro/src/nav/mod.rs:87-90`) → `AgentComponent::radius` via `from_nav_params` (`crates/entities/src/components/agent.rs:147-149`). Bake-side erosion and runtime-side corner-offset therefore use the same value by construction — no second constant to reconcile.

## Scope

### In scope

- **Euclidean erosion.** Replace the Chebyshev `near_boundary` test with a Euclidean-disk test at the same grid resolution, conservatively quantized: a cell is eroded when any point of the cell lies within `agent_radius` of a boundary column (equivalently, test cell-center distance against `agent_radius + half cell diagonal`). Boundary detection (`is_boundary_span`, floor-matched within `step_height`) is unchanged.
- **Funnel corner-offset.** Inset each committed interior waypoint by the agent radius along its portal segment, away from the endpoint (toward the portal interior). Degenerate clamp: a portal narrower than `2 × radius` yields the portal midpoint. Start/goal points are not offset. Implementation site is either the two funnel commit points or a pre-inset of portal segments in `oriented_portals` — implementer's choice; behavior contract is the AC.
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

- [ ] Every interior waypoint returned by `find_path` on the concave-corner (pillar) fixture lies at least the baked agent radius (minus an epsilon no larger than `0.1 × cell_size`) from static collision geometry, verified by a parry-direct distance query (`parry3d`, e.g. `TriMesh::project_local_point`) against the fixture's collision `TriMesh` at each waypoint (runnable test — `CollisionWorld` exposes no clearance method). Waypoints from portals narrower than `2 × radius` are exempt — they take the portal midpoint per AC4. This runnable check validates the funnel offset (Task 3); the fixture navmesh is hand-built, not baked, so erosion clearance (Task 2) rides on AC3 plus the manual `campaign-test` re-bake pass in Task 4 (no in-test bake+load harness exists).
- [ ] An agent following a funnel path around the pillar-corner fixture completes the route without stuck detection firing — goal-projected progress stays above the stuck epsilon for the whole traversal (runnable integration test reusing the stuck-recovery fixture and detection signal).
- [ ] Erosion clearance is isotropic to within one cell: measured eroded distance from a straight wall and from the same wall rotated 45° differ by at most one cell, and neither exceeds `agent_radius` by more than one cell (runnable unit test on synthetic grids — the upper bound rejects a coarser ceil'd-integer disk test that would pass isotropy alone while over-eroding).
- [ ] A portal narrower than twice the agent radius yields its midpoint as the waypoint — no NaN, no oscillating offset (runnable unit test on the funnel or portal-inset helper).
- [ ] Bumping the stage version invalidates only navmesh cache entries: the existing build-stage-cache tests remain green (each stage's cache key folds `NAVMESH_STAGE_VERSION`, so 2→3 invalidates navmesh while other stages keep their keys — the "re-bakes all maps" behavior is a construction/review gate, not a runnable pipeline test). The unchanged serialized surface is verified by the existing navmesh encode/decode round-trip tests under `NAVMESH_VERSION 1` (no committed `.prl` fixture exists to load).
- [ ] Existing A*, funnel, region-decomposition, and steering tests remain green, updated only where a route legitimately changes because clearance moved a waypoint.

## Tasks

### Task 1: Pre-split navmesh_bake.rs
Behavior-preserving split: move the `#[cfg(test)]` module (from ~line 785) to a sibling `navmesh_bake/tests.rs` (or extract the rasterize/erode pass into a submodule — whichever seam is cleaner), leaving production code under the ~800-line threshold before Task 2 extends it. No logic changes; all tests stay green.

### Task 2: Euclidean erosion + stage bump
In `erode` (`navmesh_bake.rs`), replace the Chebyshev `near_boundary` proximity test with the conservative Euclidean-disk test (cell eroded when any point of it is within `agent_radius` of a boundary column; floor-matching via the existing `step_height` neighbor test). `near_boundary` currently receives only the ceil'd `radius_cells: i64` and works in integer cell coords — thread the float `agent_radius` (or `grid.cell_size`) from `erode` into it so the disk test measures real distance, not the lossy integer (both private, same file; `erode` is the sole caller). Bump `NAVMESH_STAGE_VERSION` to 3. Add the isotropy unit test (AC3), asserting both isotropy and the over-erosion upper bound.

### Task 3: Funnel corner-offset
In `crates/postretro/src/nav/path.rs`, inset committed interior waypoints by `NavAgentParams::radius` along the portal segment away from the chosen endpoint, with the midpoint clamp for portals narrower than `2 × radius`. `NavGraph` stores `agent: NavAgentParams` (`nav/mod.rs:69`), so `find_path` reads `graph.agent.radius` in-crate — no crate boundary crossed. Plumb it as a parameter to `funnel` (sole caller: `find_path`, `path.rs:60`) rather than a global. If offsetting at the funnel commit points, the paired endpoint (the inset direction) is `gates[left_index].1` at `path.push(left)` and `gates[right_index].0` at `path.push(right)`; the degenerate `(goal, goal)` gate is never committed through those paths, so start/goal stay un-offset by construction. (Pre-insetting the segments in `oriented_portals` avoids the index bookkeeping — implementer's choice.) Add the degeneracy unit test (AC4). Two existing assertions legitimately change and must be updated to expect the inset waypoint (per AC6): `find_path_bends_l_corridor_at_inner_corner_portal_endpoint` (`path.rs:464`) and `find_path_handles_reversed_portal_traversal_via_left_right_swap` (`path.rs:504`), which currently assert a waypoint exactly at the raw corner endpoint.

### Task 4: Fixture validation
The fixture is `ConcaveCorner`, a private `#[cfg(test)]` struct in `crates/postretro/src/agent_steering/tests.rs` (exposes `fixture()`, `collision_world()`, `nav_graph()`); the new tests live in that same file to reuse it. Add the waypoint-clearance test (AC1): run `find_path` over `ConcaveCorner::nav_graph()` and assert each interior waypoint's clearance with a parry-direct distance query (`parry3d`) on the fixture's collision `TriMesh` — `CollisionWorld` exposes no clearance method, so call parry on its `mesh`/`isometry` directly, as the existing tests already construct. Add the no-stuck integration test (AC2): drive `tick(registry, world, Some(&graph), …)` around the pillar and assert `stuck_ticks` never reaches `STUCK_TICKS_THRESHOLD` for the whole traversal. Then re-bake `campaign-test` and record a manual check that wall-hugging and wedge failures are gone (the runnable clearance assertion stays on the pillar fixture — no in-test bake+load harness exists). For AC5, assert the existing navmesh encode/decode round-trip under the unchanged `NAVMESH_VERSION 1` (there is no committed `.prl` fixture to parse). If failures persist, the output is a diagnosis note — not further bake work in this spec.

## Sequencing

**Phase 1 (sequential):** Task 1 — file split; only Task 2 touches the split result. (Task 3 edits `path.rs`/`nav/mod.rs` and could begin in parallel with Task 1.)
**Phase 2 (concurrent):** Task 2 (bake crate), Task 3 (runtime crate) — independent files; each carries its own unit tests.
**Phase 3 (sequential):** Task 4 — consumes both Task 2's re-baked grid and Task 3's offset waypoints.

## Open questions

- **Offset site (implementation note, not open).** Funnel commit points vs pre-inset in `oriented_portals` is the implementer's call; the AC contract (waypoint clearance + degenerate midpoint) is site-agnostic.
- **Narrow corridors on existing maps.** Conservative Euclidean erosion erodes slightly more than the current under-eroding quantization in some spots; a corridor authored at marginal width could close. Mitigation is the `campaign-test` re-bake check in Task 4; if a corridor closes, the map is authored too tight for the canonical agent and should be widened — not special-cased in the bake.
- **Combat-positioning interaction.** Clearance-correct waypoints make combat-position candidates near walls honestly unreachable, which `E10--enemy-combat-positioning` treats as a filter outcome, not an error. No coupling work needed; noted so the positioning spec's escalation gate references this draft.
