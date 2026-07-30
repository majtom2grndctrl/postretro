# E10 — Nav Portal Handedness (baked funnel-portal orientation)

> **Wave:** E10 enemy-AI follow-up. Surfaced in manual play-testing on the
> movement-feel arena: aggroed enemies chart a path to a wall at the far point of
> the arena and back, and a flock walks off toward TrenchBroom-West (engine +Z).
> Root cause verified empirically — see sibling `research.md`.
>
> **Adjacent specs:** `E10--enemy-stuck-recovery` (shipped; steering-side backstop
> that could not fix this — the path data itself is wrong) and
> `E10--navmesh-capsule-clearance` (shipped; touched the same bake
> functions for Euclidean erosion and funnel waypoint insetting, orthogonal to
> endpoint order — split the test file out of `navmesh_bake.rs` and bumped the
> stage version to 3).

## Goal

The bake emits constant-X (east-west doorway) nav portals with `left`/`right` in a
fixed order, while the runtime funnel requires them oriented for
`region_a → region_b` traversal — so the string-pull bends at the far doorway jamb
or clips wall corners, sending enemies to a far wall and back. Fix the bake's
vertical-portal orientation, pin the handedness convention in the format, close the
bake→runtime test hole that let two self-consistent layers disagree, and anchor A*
costs on real start/goal positions instead of region centroids.

## Scope

### In scope

- Orient baked constant-X portal endpoints by which region is `region_a`.
- Document the portal handedness convention on `NavPortal` in
  `postretro-level-format`; reject stale navmesh sections at load
  (`NAVMESH_VERSION` 1 → 2) via the existing warn-and-ignore degradation.
- Bump the navmesh bake stage version so cached bakes invalidate.
- Expose the navmesh bake through the level-compiler lib target so cross-crate
  tests can bake fixtures (bc5/texture_mips precedent).
- Bake→runtime contract tests: bake small fixture geometries, run `find_path`,
  assert funnel waypoints land at the near jamb.
- A* cost anchoring: portal-to-portal hop costs with true start/goal anchoring,
  replacing the centroid metric. Rides in this spec rather than splitting out: it
  is the same playtest report's secondary symptom, confined to the same query
  module, and verified by the same fixture style — a separate spec would restate
  this one's context for ~40 lines of change.
- Rebake of dev `.prl` content and an in-arena playtest verification.

### Out of scope

- Euclidean erosion and funnel waypoint inset off portal endpoints —
  `E10--navmesh-capsule-clearance` (shipped).
- Region decomposition changes (greedy rectangles stay; the contour tracer stays
  future work).
- Steering, replan policy, target selection — shipped; the behavior state machine
  has since replaced the engine-closed FSM with an authored state graph
  (`components.behavior` descriptor, `scripting/systems/ai/` module tree), but the
  `agent_steering` API surface (`set_destination`/`clear_destination`/`path_state`)
  consumed by this spec is unchanged.
- Off-mesh links, portal kinds, multi-agent bakes.

## Acceptance criteria

- [ ] **AC1 — near-jamb funnel over baked data.** For baked fixture floors
  (east-west doorway, north-south doorway, L-corridor in both chiralities), a
  path between two rooms whose straight start→goal line misses the doorway bends
  at the doorway jamb **nearer** that line — in **both** traversal directions —
  and every path leg stays inside the walkable floor. Verified by runnable tests
  that call the real bake, then the real `find_path`, over each fixture.
- [ ] **AC2 — bake orientation invariant.** For a baked pair of rooms abutting on
  a constant-X edge, the emitted portal satisfies the documented convention
  (crossing `region_a → region_b`, stored `left` is on the agent's left)
  regardless of which side sorts as `region_a`. Verified by bake unit tests with
  fixtures that force `region_a` onto each side (regression: the old bake emitted
  z-ascending order unconditionally).
- [ ] **AC3 — straight corridors stay straight.** Baked fixtures whose start→goal
  line passes through every doorway produce exactly `[start, goal]`.
- [ ] **AC4 — stale sections rejected loudly.** Loading a `.prl` whose navmesh
  section carries the previous section version logs a warning and disables
  navigation for that map (no section, AI idle) instead of pathing on
  wrong-handed portals; recompiling the map restores navigation.
- [ ] **AC5 — cache invalidation.** The first `prl-build` after the fix misses the
  navmesh stage cache and re-bakes (stage version participates in the key; the
  existing key test covers the mechanism), so a warm rebuild cannot resurrect
  inverted portals.
- [ ] **AC6 — start-anchored routing.** On a baked fixture with one large region
  offering two doorways toward the goal, an agent starting beside doorway A exits
  through doorway A and one starting beside doorway B exits through B — the
  centroid metric would send one of them across the room. Existing two-doorway
  and reversed-traversal path tests keep passing.
- [ ] **AC7 — arena playtest.** On the movement-feel arena (dev-tools build,
  Alt+Shift+A overlay), aggroed enemies' drawn paths contain no waypoint at a
  portal endpoint farther from the straight agent→target line than that portal's
  other endpoint, and no group walks to a far wall and doubles back while chasing
  a visible player. Requires recompiling the map first (AC4/AC5 force this).
- [ ] **AC8 — no collateral.** `cargo test -p postretro --lib` and
  `cargo test -p postretro-level-compiler --lib` pass; `prl-build` output for an
  unchanged map differs only in the navmesh section.

## Tasks

### Task 1: Orient vertical portals in the bake; pin the convention; bump versions

In `crates/level-compiler/src/navmesh_bake.rs`, make `shared_vertical_edge` emit
endpoints oriented for `region_a → region_b` traversal. The function already
resolves which side is geometrically west (`left_region`/`right_region` via the
`ra.x1 == rb.x0` / `rb.x1 == ra.x0` arms) and receives `ra = regions[a]` where
`a`/`b` are the sorted indices stored as `region_a`/`region_b`: when `ra` is the
west region the crossing is +X and the portal must store `left = z_hi endpoint`,
`right = z_lo`; when `ra` is the east region the crossing is −X and the current
`left = z_lo`, `right = z_hi` order is already correct. `shared_horizontal_edge`
is provably always correct (region_a is always the south region under the z0-first
region sort) — leave it, but say why in a comment. The portal sort invariant
(sort by `(region_a, region_b)` then stored `left` under f32 total order) reads
the stored values and needs no change. Document the convention on `NavPortal` in
`crates/level-format/src/navmesh.rs`: stored `left`/`right` are oriented for
`region_a → region_b` traversal — the agent crossing that way has `left` on its
left in the XZ funnel projection; equivalently, constant-X portal crossed toward
+X → `left` is the greater-Z endpoint; constant-Z portal crossed toward +Z →
`left` is the lesser-X endpoint (matches the runtime `oriented_portals` swap and
the existing hand-built path fixtures). Because previously-baked sections violate
the now-pinned convention with no way to detect it, bump `NAVMESH_VERSION` 1 → 2
and make `NavMeshSection::from_bytes` reject any other version (today it reads
the version field without validating); the runtime loader already turns a
rejected section into warn-and-ignore → no navigation, which is the intended
loud degradation. Bump `NAVMESH_STAGE_VERSION` by one (currently 3 after the capsule-clearance bump; becomes 4) so the `"navmesh"` build
cache stage invalidates. Add bake unit tests asserting the emitted endpoint
order for a constant-X portal with `region_a` forced onto the west side and onto
the east side (stack the second fixture's regions so the east region's z0 sorts
first), and a `from_bytes` version-rejection test. Bake unit tests now live in
`navmesh_bake/tests.rs` (tests were split to a sibling file by capsule-clearance).

### Task 2: Expose the bake to cross-crate tests (plumbing)

The bake modules are declared only in the level-compiler's `main.rs`, so no other
crate can call `bake_navmesh` — and `fixture_pipeline.rs` documents that pain.
Following the `bc5`/`texture_mips` precedent (modules declared in both targets),
declare the dependency closure in `crates/level-compiler/src/lib.rs`:
`navmesh_bake`, `geometry`, `map_data`, `map_format`, `partition`, `cache`
— note that `navmesh_bake` is now a directory module (`navmesh_bake/mod.rs` +
`navmesh_bake/tests.rs`) after the capsule-clearance split; the
`pub mod navmesh_bake;` declaration covers both.
(`navmesh_bake`'s own test module uses `cache::CacheKey`; `geometry` pulls
`map_data`, `map_format`, `partition`; none reach further). `main.rs` keeps its
declarations unchanged. Then add `postretro-level-compiler.workspace = true` under
the `postretro` crate's `[dev-dependencies]` — the workspace dependency entry
already exists in the root `Cargo.toml`. `postretro` is bin-only, but
`#[cfg(test)]` modules under `src/` see dev-dependencies. Pure plumbing: no behavior change, no new public API beyond
the module declarations; the level-compiler's existing `--lib` test target will
now also compile those modules' co-located tests, which is acceptable (bc5 already
double-compiles).

### Task 3: Bake→runtime funnel contract tests

New `#[cfg(test)]` sibling module under `crates/postretro/src/nav/` (declared
from `nav/mod.rs`; keep it out of `path.rs` so Task 4 can run concurrently).
Each test builds fixture floor triangles (a ~40-line local floor-quad →
`GeometryResult` builder mirroring the bake's own test helpers — those are
`#[cfg(test)]`-private to the compiler crate, so replicate, don't import), bakes
with `postretro_level_compiler::navmesh_bake::bake_navmesh` at `cell_size` 0.25
and zero erosion, wraps the section in `NavGraph::from_section`, and runs
`find_path` (which now returns `NavPath` — a struct with `points: Vec<Vec3>` and
`mandatory_waypoints: Vec<bool>` — rather than bare `Option<Vec<Vec3>>`).
Fixtures, each traversed in both directions: (1) east-west doorway —
two rooms abutting in X joined by a 1 m neck, start/goal offset so the straight
line misses the neck; assert the interior waypoints sit at the near jamb (both
portal endpoints on the jamb side nearer the straight line, within epsilon;
waypoints will land at the **inset** near-jamb point — offset by
`agent_radius + SKIN_DISTANCE` from the raw portal endpoint — since the
capsule-clearance insetting pipeline is now active; assertions should compare
against the inset position, not the raw endpoint) and
no waypoint at the far jamb; (2) north-south doorway — the same shape rotated,
covering the constant-Z emitter and the room-depth vertical portals its
decomposition produces (the empirical far-wall case in `research.md`); (3)
L-corridor in both chiralities — assert the bend lands at the inner-corner portal
endpoint, mirroring the existing hand-built L tests but over baked data; (4) a
straight two-room corridor asserting the `[start, goal]` collapse (AC3). Include
the one-line regression comment naming this bug per the testing guide. These
tests close the AC1/AC3 contract: the bake and the funnel meet in CI for the
first time. Note: the funnel now operates on `FunnelGate`/`FunnelEndpoint`
types rather than raw `(Vec3, Vec3)` tuples — this does not affect the contract
test API (they call `find_path` which handles the internal types), but fixture
authors should be aware in case internal assertions need updating.

### Task 4: Anchor A* costs on portals and true endpoints

In `crates/postretro/src/nav/path.rs`, replace the centroid-anchored corridor
search: today the heuristic is centroid-to-centroid and each edge costs
centroid → portal-midpoint → centroid, so a start far from its region's centroid
is mis-charged and large-region routes pick wrong doorways. Search over portal
crossings instead: the start expands to every portal of the start region at cost
`distance_xz(start, portal_mid)`; a portal expands to the portals of its far-side
region at mid-to-mid cost; reaching any portal of the goal region closes with
`distance_xz(portal_mid, goal)`; heuristic = `distance_xz(portal_mid, goal)`
(admissible — straight-line). Keep the output contract identical: the corridor of
`CorridorHop { portal_index, from_region }` values consumed by
`oriented_portals`, including the exact-portal guarantee when two portals join
the same region pair. Same-region start/goal short-circuit is unchanged. Callers
are unchanged (`find_path` is the primary entry; `agent_steering` and
`combat_positioning` consume waypoints). Add tests: a large single region with two doorways to the goal region
where each start position must exit through its adjacent doorway (AC6), and keep
`find_path_follows_cheaper_of_two_doorways_between_same_region_pair` plus the
reversed-traversal test green.

### Task 5: Rebake and arena verification

Recompile the dev maps used for AI testing (at minimum `movement-feel` and
`campaign-test`) with `prl-build`; confirm the stale-section warning fires when
running an old `.prl` against the new engine and disappears after recompiling
(AC4). Then run the movement-feel arena with dev-tools, aggro the enemy ring, and
verify AC7 on the Alt+Shift+A overlay: paths bend at near jambs, no far-wall
double-backs, chase looks direct. Record the observation (pass/fail plus any
residual detour behavior) in the PR description; residual milder detours under
crowding are steering-side and out of scope here.

## Sequencing

**Phase 1 (concurrent):** Task 1 (bake fix + format convention), Task 2 (test
plumbing) — independent files.
**Phase 2 (concurrent):** Task 3 (consumes Task 1's orientation and Task 2's lib
surface), Task 4 (path.rs only; no file overlap with Task 3's sibling module).
**Phase 3 (sequential):** Task 5 — consumes everything; manual verification gate.

## Rough sketch

- Convention predicate, for tests and comments: with travel direction `d`
  (region_a → region_b, XZ) and `s = left − right`, require
  `d.z * s.x − d.x * s.z < 0` — the same chirality `triangle_area_xz`
  (path.rs) encodes and all hand-built runtime fixtures satisfy.
- `shared_vertical_edge` fix is a two-arm emit at the existing single emit site:
  `ra` is `left_region` ⇒ `left = [x, y, z_hi]`, else the current order.
- Portal-node A*: nodes are portal indices plus a virtual start; `came_from`
  maps portal → (previous portal | start), reconstructed into `CorridorHop`s by
  walking far-side regions. Region-level `g_score` dedup is not sufficient —
  score per portal.
- `NAVMESH_VERSION` bump forces no PRL container change; only the navmesh
  section body's version field value changes. `from_bytes` gains one equality
  check returning the existing `invalid(...)` error shape.
- `navmesh_bake/mod.rs` feature code is ~790 lines (tests split to
  `navmesh_bake/tests.rs`, 617 lines, after the capsule-clearance pre-split) —
  Task 1 is an edit-in-place. The capsule-clearance spec completed this split.

## Open questions

- Capsule clearance has shipped. Task 3's near-jamb assertions must compare
  against the inset point (portal endpoint offset inward by
  `agent_radius + SKIN_DISTANCE`), not the raw endpoint. The `inset_portals`
  pipeline and `ensure_endpoint_clearance` repair pass are active in `find_path`.
