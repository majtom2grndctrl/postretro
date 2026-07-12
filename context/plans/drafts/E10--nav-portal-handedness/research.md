# Research — Nav Portal Handedness

Investigation record behind `index.md`. Symptom, root-cause verification, empirical
reproduction over real baked data, direction analysis against the playtest report,
and why prior fixes missed. Line numbers are as of this investigation (branch
`claude/epic-10-ai-nav-risks-8jl4yy`, 2026-07-07) — expect drift.

## Symptom (playtest)

On the movement-feel arena (8 `reference_enemy` ring around the player spawn),
enemies begin following the player, then a flock walks off toward **TrenchBroom
West**. The Alt+Shift+A agent overlay shows a charted path that runs **to a wall
at the far point of the arena and then back toward the player**. Multiple prior
fixes (stuck recovery, replan policy, steering feel, target selection) did not
resolve it.

## Root cause: baked vertical-portal handedness inversion

Two layers each hold a self-consistent portal-endpoint convention — and they
disagree. `NavPortal` (`crates/level-format/src/navmesh.rs:20-31`) documents no
handedness convention at all, so nothing forced them to agree.

**Runtime convention** (`crates/postretro/src/nav/path.rs`):

- `oriented_portals` (path.rs:204-225): stored `left`/`right` are taken as
  oriented for `region_a → region_b` traversal; crossing `region_b → region_a`
  swaps them. So stored `left` must sit on the agent's left when crossing
  `region_a → region_b`.
- The funnel's turn tests (`triangle_area_xz`, path.rs:232-238) encode that
  chirality: for travel direction `d` and stored offset `s = left - right`,
  correctness requires `d.z*s.x - d.x*s.z < 0`. Concretely: a constant-X portal
  crossed toward +X needs `left` at the **greater-Z** endpoint; a constant-Z
  portal crossed toward +Z needs `left` at the **lesser-X** endpoint.
- Hand-built test fixtures agree: L-corridor portal at x=4 crossed west→east
  stores `left = [4,0,8]` (z_hi) (path.rs:453-458); the reversed-traversal
  fixture stores `left = z_lo` for an east→west `region_a → region_b` crossing
  (path.rs:520-526). Straight corridor (+Z travel) stores `left = x_lo`
  (path.rs:361-373).

**Bake convention** (`crates/level-compiler/src/navmesh_bake.rs`):

- `shared_vertical_edge` (constant-X portals, navmesh_bake.rs:682-732) emits
  `left = [x, y, world_z0]`, `right = [x, y, world_z1]` — **z_lo first,
  unconditionally** (lines 726-731), regardless of which side `region_a` is on.
- `shared_horizontal_edge` (constant-Z portals, navmesh_bake.rs:744-783) emits
  `left = x_lo`, `right = x_hi` (lines 778-782).

**Which portals are wrong.** `region_a`/`region_b` come from the region sort
(z0-then-x0-then-…, navmesh_bake.rs:573-579) via the `a < b` pair loop
(navmesh_bake.rs:653-668).

- Horizontal portals: for regions abutting in Z, the south region always has the
  smaller z0, so `region_a` is always the south region; `region_a → region_b` is
  +Z travel; stored `left = x_lo` is **always correct**.
- Vertical portals: regions abutting in X overlap in Z, so either can sort
  first. When the **west** region is `region_a` (z0 equal → x0 decides → west
  first; or west z0 smaller), the `region_a → region_b` crossing is +X travel and
  the stored `left = z_lo` is **inverted**. When the east region happens to sort
  first (its z0 is smaller), the crossing is −X travel and `left = z_lo` is
  correct by accident. Real bakes carry a **mix** of inverted and correct
  vertical portals.

An inverted portal makes the funnel's left/right tightening track the
geometrically wrong sides, so the string-pull bends at the **far** jamb of the
portal (or clips through wall corners) instead of the near jamb. With the
room-spanning constant-X portals the greedy strip decomposition produces, the
far jamb is a far wall — "path to the far wall, then back to the player."

## Empirical reproduction (throwaway probe, not committed)

A temporary test module in `postretro-level-compiler` baked fixture floors with
the real `bake_navmesh` and ran the **real runtime pathfinding** over the result
— a verbatim `#[path]`-include copy of `crates/postretro/src/nav/{mod,path}.rs`
with only the `postretro_foundation::NavAgentParams` re-export replaced by an
identical local struct. Probe deleted after the runs; repo restored.

**Fixture 1 — east-west doorway** (H-shaped floor: 4×8 m rooms at x∈[0,4] and
x∈[5,9], 1×1 m neck at z∈[3.5,4.5]; `cell_size` 0.25, no erosion). Baked
regions: west room (index 0), east room (1), neck (2). Baked portals:
`0↔2 left [4,0,3.5] right [4,0,4.5]`, `1↔2 left [5,0,3.5] right [5,0,4.5]`.
Portal 0↔2 is inverted: region_a is the west room, so the `region_a → region_b`
crossing is +X and the stored `left = z_lo` violates the runtime convention.
Portal 1↔2 is correct by accident: it pairs the east room (index 1, z0 = 0)
with the neck (index 2, z0 = 14), so region_a is the **east** room and the
`region_a → region_b` crossing is −X, where `left = z_lo` happens to be right.

Paths (`find_path`, start/goal Y = 0.1):

| Case | Near jamb | Waypoints emitted |
|---|---|---|
| west→east at z=7 | z=4.5 | start, **(4.0, 0, 3.5)**, goal — far jamb |
| east→west at z=7 | z=4.5 | start, **(4.0, 0, 3.5)**, goal — far jamb; the leg to it crosses x=5 at z≈5.9, **through the wall** |
| west→east at z=1 | z=3.5 | start, **(4.0, 0, 4.5)**, goal — far jamb |
| east→west at z=1 | z=3.5 | start, (5.0, 0, 3.5), **(4.0, 0, 4.5)**, goal — near jamb of the correct portal, then far jamb of the inverted one |

**Fixture 2 — north-south doorway** (same shape rotated: rooms at z∈[0,4] and
z∈[5,9], neck at x∈[3.5,4.5]). The decomposition grew the neck seed north
through the middle of the north room, producing one horizontal portal (0↔1 at
z=4, doorway width — correct) and two **room-depth vertical portals** at x=3.5
and x=4.5 spanning z∈[5,9]. Results:

| Case | Waypoints emitted |
|---|---|
| south→north east of doorway | start, (4.5, 0, 4), **(4.5, 0, 9)**, goal — walks to the z=9 **far wall**, then back to the goal at z=8 |
| north→south east of doorway | start, **(4.5, 0, 9)**, (4.5, 0, 4), goal — same wall touch |
| south→north west of doorway | start, (3.5, 0, 4), (3.5, 0, 5), goal — **correct** near jambs (that vertical portal is the correct-by-accident case) |
| north→south west of doorway | correct near jambs |

Fixture 2's east cases are the playtest symptom verbatim: a waypoint at the far
end of a room-spanning vertical portal — the far wall — then back toward the
target. The horizontal (constant-Z) portal bent correctly at the near jamb in
all four cases, confirming the horizontal emitter is sound.

## Direction check: does "TrenchBroom West" fit?

Map-space transform (`quake_to_engine`, `crates/level-compiler/src/parse.rs:45`):
`engine = (-q.y, q.z, -q.x)` plus 0.0254 unit scale. Swizzle tests
(parse.rs:1412-1437): Quake +X (TB East) → engine −Z; Quake +Y (TB North) →
engine −X; Quake +Z → engine +Y. So **TrenchBroom West = engine +Z**.

Inverted portals are constant-**X** portals; their segments run along engine Z,
so the wrong-jamb waypoint displacement is **along engine Z — exactly the
TB East–West axis**. The reported drift direction (TB West = engine +Z) lies on
precisely the axis this bug displaces waypoints along, and the "far wall then
back toward the player" shape matches the room-spanning-portal far jamb.

The specific **sign** (West rather than East) is geometry-dependent: the far
jamb is at +Z or −Z depending on where the straight agent→target line sits
along the portal run (fixture 1 produced both signs). A *flock* drifting one
way is expected — an aggro group sharing one region corridor shares the same
inverted portal and the same far jamb, so all members bend the same way. The
handedness bug stands on its own evidence; the West sign is consistent with it
but was not independently re-derived from a full movement-feel bake.

## Why prior fixes missed

`git log` fix history (all shipped, none touched the bake or funnel data):

- `E10--enemy-stuck-recovery` (287247e/260d35d wave): stuck detection + tangent
  slide in `agent_steering.rs`. Explicitly scoped bake work out ("Option B …
  touches the bake algorithm … Deferred").
- `E10--enemy-steering-feel` (6358e70): accel/turn-rate/arrival easing — "the
  path itself is already string-pulled by the funnel; this is about how the
  agent moves along it." Assumed the path was right.
- `E10--enemy-mp-target-selection` + replan-policy rework
  (`REPLAN_DEST_THRESHOLD`, agent_steering.rs:86-104): which target and when to
  replan — replans re-run the same funnel over the same inverted portals.
- `E10--navmesh-capsule-clearance` (draft, conditional): erosion shape and
  waypoint inset — adjacent bake work, but orthogonal to endpoint order.

All patched layers **downstream of the baked data**. Runtime path tests
hand-build portals in the correct convention (path.rs:361-373, 444-460,
517-535), so they pass; bake tests assert portal *presence*, Y, and the shared
line (`climbable_step_yields_a_portal`, navmesh_bake.rs:1033-1063 — a
horizontal portal; no test asserts a **vertical** portal's endpoint order).
**No test bakes geometry and runs `find_path` over the result** — the two
self-consistent layers never met in CI.

## Crate-structure findings (test placement)

- `postretro-level-compiler` is a bin crate for everything nav: `navmesh_bake`,
  `geometry`, `map_data` are declared in `main.rs` only. `lib.rs` exposes just
  `bc5` and `texture_mips` ("shared helpers used by narrow development tools").
  `fixture_pipeline.rs` documents the constraint: cross-module integration
  tests must co-locate because there is no lib target for those modules.
- `postretro` is also bin-only (no `[lib]`), but unit tests inside `src/` can
  use `[dev-dependencies]` — so exposing the bake via the level-compiler lib
  target (bc5 precedent: declared in both `lib.rs` and `main.rs`) and adding a
  dev-dependency lets bake→runtime contract tests co-locate with `nav/`.
- Module closure needed on the lib target: `navmesh_bake`, `geometry`,
  `map_data`, `map_format`, `partition` (via `geometry`), `cache` (used by
  navmesh_bake's own test module). None import further crate-internal modules.
- `navmesh_bake.rs` is 1226 lines with tests from ~line 792 — feature code
  ~790 lines, under the ~800 split threshold; the orientation fix is a small
  edit, not an extension, so no pre-split task. (The capsule-clearance draft
  plans a pre-split for its larger change; if it lands first, fine.)

## Secondary: centroid-anchored A* costs

Verified in `astar_corridor` (path.rs:119-181): heuristic = XZ distance between
**region centroids** (:124-125); edge cost = centroid → portal midpoint →
centroid (:162-167). Neither anchors on the true start/goal positions, so in
large regions (the arena floor is one or few big rectangles) the first-hop cost
is charged from the region centroid regardless of where the agent stands —
mis-picking doorways/corridors when alternatives exist. Milder than the
handedness bug and masked by it; visible after (1) is fixed. Fix: A* over
portal nodes anchored at the true start/goal (see spec Task 4).

## Version / invalidation findings

- `NAVMESH_STAGE_VERSION = 2` (navmesh_bake.rs:15) keys the `"navmesh"` build
  cache stage; bumping it invalidates cached bakes
  (`cache_key_changes_with_each_nav_param`, navmesh_bake.rs:1173-1230).
- No `.prl` files are committed (`git ls-files content/dev/maps | grep prl` →
  0); compiled maps are dev-local. A stage bump alone fixes the *cache* but a
  stale local `.prl` still carries inverted portals until recompiled.
- `NavMeshSection::from_bytes` (level-format navmesh.rs:140-165) reads the
  section version but **does not validate it**. The loader treats a malformed
  navmesh section as warn-and-ignore → `navmesh: None` → AI disabled
  (`prl_loader.rs:1958-1978`) — an existing graceful degradation path the spec
  reuses for loud stale-section rejection (`NAVMESH_VERSION` 1 → 2).
- Portal endpoint consumers: the funnel (orientation-sensitive) and the
  Alt+Shift+N navmesh overlay (`render/nav_diagnostics.rs:53-54`, draws the
  segment — orientation-agnostic). Nothing else reads `left`/`right`.
- Alt+Shift+A is the all-agent movement overlay
  (`renderer/src/render/renderer_diagnostics.rs:443`), the instrument for the
  playtest AC.
