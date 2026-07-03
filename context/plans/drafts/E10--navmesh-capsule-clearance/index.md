# E10 — Navmesh Capsule Clearance (Euclidean erosion + funnel corner-offset)

> **Conditional — do not build until a gate fires.** This is the pre-scoped "Option B" that `E10--enemy-stuck-recovery` (post-implementation escalation gate) and `E10--enemy-combat-positioning` (navmesh-clearance escalation, Open questions) both defer to. It exists so that when a gate fires, the decision is a promotion, not a research project. Entry gate: route-valid wedge failures or visible wall-hugging persist on the pillar/wedge fixture after tangent recovery lands, or combat positioning selects navmesh-reachable points the capsule cannot physically occupy.
>
> **Wave:** E10 enemy-AI follow-up. Runs after `E10--enemy-stuck-recovery` (its fixture and detection signal are this spec's validation instruments).

## Goal

Make routes physically traversable by construction: erode the walkable grid by the agent radius as a Euclidean disk instead of a Chebyshev box, and inset funnel waypoints off portal endpoints by the agent radius — so no waypoint sits within the capsule radius of static geometry and corner wedges stop occurring at the source. Stuck recovery remains as a backstop, not the primary defense.

## Background (the cause)

- **Erosion is a cell-quantized Chebyshev box.** `erode` (`crates/level-compiler/src/navmesh_bake.rs:406`) computes `radius_cells = (agent_radius / cell_size).ceil()` (line 407) and `near_boundary` (line 466) tests `dx < radius_cells && dz < radius_cells` (line 470) — an axis-aligned box, not a disk, quantized to whole cells. Clearance is anisotropic (diagonal vs axis) and cell-granular: a cell whose center clears the radius but whose area does not stays walkable, leaving sub-cell slivers at pillar corners where the capsule still contacts geometry.
- **Funnel waypoints are exactly portal endpoints.** `funnel` (`crates/postretro/src/nav/path.rs:244`) commits interior waypoints with `path.push(left)` / `path.push(right)` (lines 272, 289) — the raw `gate_left`/`gate_right` portal endpoints, which the bake aligns to the cell lattice (`navmesh_bake.rs:722-731`). A waypoint at a portal endpoint can sit flush against a wall corner; an agent steering to its center grazes the corner and `collide_and_slide` consumes the velocity — the wedge `E10--enemy-stuck-recovery` detects and recovers from.
- **One radius, threaded end to end.** `NavParams::agent_radius` (worldspawn `nav_agent_radius` KVP, default 0.4 — `crates/level-compiler/src/map_data.rs:499,519`) → serialized `NavMeshSection::agent_radius` (`navmesh_bake.rs:138`) → `NavAgentParams::radius` (`crates/postretro/src/nav/mod.rs:87-90`) → `AgentComponent::radius` via `from_nav_params` (`crates/entities/src/components/agent.rs:127`). Bake-side erosion and runtime-side corner-offset therefore use the same value by construction — no second constant to reconcile.

## Scope

### In scope

- **Euclidean erosion.** Replace the Chebyshev `near_boundary` test with a Euclidean-disk test at the same grid resolution, conservatively quantized: a cell is eroded when any point of the cell lies within `agent_radius` of a boundary column (equivalently, test cell-center distance against `agent_radius + half cell diagonal`). Boundary detection (`is_boundary_span`, floor-matched within `step_height`) is unchanged.
- **Funnel corner-offset.** Inset each committed interior waypoint by the agent radius along its portal segment, away from the endpoint (toward the portal interior). Degenerate clamp: a portal narrower than `2 × radius` yields the portal midpoint. Start/goal points are not offset. Implementation site is either the two funnel commit points or a pre-inset of portal segments in `oriented_portals` — implementer's choice; behavior contract is the AC.
- **Stage-version bump.** `NAVMESH_STAGE_VERSION` 2 → 3 (`navmesh_bake.rs:15`), forcing a navmesh re-bake of all cached maps on next build. No PRL body-layout change: regions and portals serialize identically, so `NAVMESH_VERSION` (`crates/level-format/src/navmesh.rs:81`) stays 1 and previously compiled `.prl` files still parse.
- **Pre-split of the bake file.** `navmesh_bake.rs` is 1226 lines (tests in-file from ~line 792). Split before extending, per the split-before-extend rule.
- **Fixture validation.** Waypoint-clearance and no-stuck assertions on the concave-corner fixture built by `E10--enemy-stuck-recovery`, plus a `campaign-test` re-bake sanity pass.

### Out of scope

- Grid resolution / `cell_size` changes; sub-cell (distance-field) walkability.
- Per-archetype agent radii — the navmesh stays baked for the one canonical agent.
- Runtime dynamic clearance queries, corridor re-derivation, ORCA/RVO.
- Removing stuck recovery — tangent recovery stays as the backstop for cases clearance cannot prove (dynamic agents, movers).
- PRL format changes (`NAVMESH_VERSION` bump) — not needed; the serialized surface is unchanged.

## Acceptance criteria

- [ ] Every interior waypoint returned by `find_path` on the concave-corner (pillar) fixture and on a re-baked `campaign-test` lies at least the baked agent radius (minus a small epsilon) from static collision geometry, verified by a capsule-overlap or distance query against the collision world at each waypoint (runnable test).
- [ ] An agent following a funnel path around the pillar-corner fixture completes the route without stuck detection firing — goal-projected progress stays above the stuck epsilon for the whole traversal (runnable integration test reusing the stuck-recovery fixture and detection signal).
- [ ] Erosion clearance is isotropic to within one cell: measured eroded distance from a straight wall and from the same wall rotated 45° differ by at most one cell (runnable unit test on synthetic grids).
- [ ] A portal narrower than twice the agent radius yields its midpoint as the waypoint — no NaN, no oscillating offset (runnable unit test on the funnel or portal-inset helper).
- [ ] Bumping the stage version invalidates only navmesh cache entries: next build re-bakes navmesh for all maps while other stages hit cache (existing build-stage-cache tests remain green); previously compiled `.prl` files still load (body layout unchanged).
- [ ] Existing A*, funnel, region-decomposition, and steering tests remain green, updated only where a route legitimately changes because clearance moved a waypoint.

## Tasks

### Task 1: Pre-split navmesh_bake.rs
Behavior-preserving split: move the `#[cfg(test)]` module (from ~line 792) to a sibling `navmesh_bake/tests.rs` (or extract the rasterize/erode pass into a submodule — whichever seam is cleaner), leaving production code under the ~800-line threshold before Task 2 extends it. No logic changes; all tests stay green.

### Task 2: Euclidean erosion + stage bump
In `erode` (`navmesh_bake.rs`), replace the Chebyshev `near_boundary` proximity test with the conservative Euclidean-disk test (cell eroded when any point of it is within `agent_radius` of a boundary column; floor-matching via the existing `step_height` neighbor test). Bump `NAVMESH_STAGE_VERSION` to 3. Add the isotropy unit test (AC3).

### Task 3: Funnel corner-offset
In `crates/postretro/src/nav/path.rs`, inset committed interior waypoints by `NavAgentParams::radius` along the portal segment away from the chosen endpoint, with the midpoint clamp for portals narrower than `2 × radius`. The radius reaches the funnel from the `NavGraph`'s stored agent params (`nav/mod.rs`) — plumb it as a parameter to `funnel` (callers: `find_path`) rather than a global. Add the degeneracy unit test (AC4).

### Task 4: Fixture validation
Add the waypoint-clearance test (AC1) and the no-stuck integration test (AC2) against the stuck-recovery concave-corner fixture; re-bake `campaign-test` and record a manual check that wall-hugging and wedge failures are gone. If failures persist, the output is a diagnosis note — not further bake work in this spec.

## Sequencing

**Phase 1 (sequential):** Task 1 — file split; everything else touches the split result.
**Phase 2 (concurrent):** Task 2 (bake crate), Task 3 (runtime crate) — independent files; each carries its own unit tests.
**Phase 3 (sequential):** Task 4 — consumes both Task 2's re-baked grid and Task 3's offset waypoints.

## Open questions

- **Offset site.** Funnel commit points vs pre-inset in `oriented_portals` — implementation choice; the AC contract (waypoint clearance + degenerate midpoint) is site-agnostic. Decide during implementation.
- **Narrow corridors on existing maps.** Conservative Euclidean erosion erodes slightly more than the current under-eroding quantization in some spots; a corridor authored at marginal width could close. Mitigation is the `campaign-test` re-bake check in Task 4; if a corridor closes, the map is authored too tight for the canonical agent and should be widened — not special-cased in the bake.
- **Combat-positioning interaction.** Clearance-correct waypoints make combat-position candidates near walls honestly unreachable, which `E10--enemy-combat-positioning` treats as a filter outcome, not an error. No coupling work needed; noted so the positioning spec's escalation gate references this draft.
