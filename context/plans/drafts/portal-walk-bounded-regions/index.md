# portal-walk-bounded-regions

Brief · compact · reads: `context/lib/rendering_pipeline.md` §2, `context/lib/build_pipeline.md` §Runtime visibility, `context/lib/testing_guide.md` · read at b99101534 · evidence: `research.md`, `prototype/`

## Coordination with lightmap cell blocks

`spatial-residency--lightmap-cell-blocks` (in progress on `feat/lightmap-cell-blocks`;
`PORTAL_WALK_EPOCH` lands with it) consumes this walk in two ways.

- **Baked offline.** The compiler's CellResidencySet stage (id 51) samples
  `determine_visible_cells` and `portal_traverse` from eye points in every cell, to
  bake each camera cell's lightmap residency set. Its cache key hashes
  `postretro_visibility::PORTAL_WALK_EPOCH`. This brief changes which cells a pose
  reaches, so it bumps that epoch. Otherwise warm builds keep id-51 sets from the old
  walk, and the runtime counts the difference as visible misses. Rebake the measured
  maps after landing.
- **Superset consumer.** Add lightmap residency to the consumer list under "The
  visible set becomes a conservative superset". Visible cells drive lightmap block
  demand that is never refused, so extra cells mean more resident blocks and possible
  pool growth. At the shaft and arena poses, also report the change in visible
  lightmap blocks and bytes vs the exact walk.

It also helps that brief. On the hallway's shaft rooms the exact walk trips its step
cap during id-51 sampling. The bounded walk should cut that stage's bake time. Report
the stage time before and after.

## Problem

Developer-observed defect, confirmed by an offline sweep of the real traversal: on
stress-warren-hallway-inspection, a stair room of floating shaft platforms makes the
runtime portal walk trip its step cap, and frame rate collapses. Cause: the walk
enumerates every portal chain that survives frustum narrowing and keeps no per-cell
state, so cost follows the number of distinct paths into each cell, not the number of
visible cells. Compiler cells split on every brush plane, so stacked geometry yields
thin slab cells whose boundaries multiply surviving chains. When this is done, walk
cost is bounded by visible cells × portal degree × a snap-grid growth constant, and the
lowered step cap bounds anything past that. The visible set is a conservative superset
of the exact set, with small measured over-inclusion. No ordinary pose reaches the
step-cap fallback.

## Decisions

- **One grow-only screen-space rect per cell replaces chain enumeration.** A portal is
  clipped against its source cell's rect, and the result's screen bounds are unioned
  into the destination cell's rect. A cell expands again only when its rect grows.
  Why: this is how Descent, Source and Thief bound portal cost. The id Tech 4 walk we
  ported relies on hand-placed portals, which compiler cells lack. `rendering_pipeline.md`
  §2's "per-frame portal traversal is cheap at modern cell counts" holds only with this
  bound.
- **Rect bounds snap outward to a fixed grid.** Why: snapping caps how often a cell can
  grow. It also makes the visible set independent of portal order, and it keeps every
  opening at least one grid cell wide, so edge-on slivers stay conservative.
- **The visible set becomes a conservative superset of the exact portal set.** Every
  consumer is correct on a superset: camera cull, fog reach, shadow-light eligibility
  and slot ranking, SH streaming targets and residency, the SH sampled-row gate, and
  the particle, mesh and mover culls. Extra cells raise non-yielding SH demand and
  shadow-slot competition, so both are measured. This diverges from
  `plans/done/perf-visible-cell-candidate-cull` and `perf-dynamic-light-pvs-cull`,
  which call traversal output "exact". That was a description, not a requirement:
  output identity rests on spans matching visible cells, and a superset keeps that.
  `cell-visibility-relation`'s superset-only gate rule points the same way.
  `context/lib/` records "conservative" at promotion.
- **The camera-on-portal-plane bypass applies only near the polygon.** Today it tests
  the portal's infinite plane, so any distant portal coplanar with the camera skips
  clipping. It fires on a few percent of poses and is the largest source of
  over-inclusion. The bypass still covers a camera standing on or beside the portal
  itself. This tightens the exact test oracle and the new walk alike.
- **The step cap stays as a backstop and is lowered.** A trip takes the bounded
  frustum-set fallback landed by `perf/hallway-inspection-followups`. Why: with the
  bound, the cap never trips on the swept map, so the cap only guards pathological
  content.
- **Rect, not octagon or hybrid.** The octagon cut extra cells by a fraction of a cell
  per frame, mostly from camera pitch rather than wall angle, and cost more steps and
  CPU. A hybrid (exact to a budget, then rect) was tightest but costlier, and its
  output depended on portal order. Keep the region behind one type, so swapping in the
  octagon stays local if a future map measures looser.
- **Placement:** runtime traversal in the visibility crate. Consumers and their inputs
  keep their shape; only the set's contract loosens.
- **Non-goals:**
  - Compiler detail brushes or cell merging. Out of scope because they change cell
    granularity for fog masks, SH clusters and draw indexing, and add an authoring
    surface. They remain a possible later authoring optimization.
  - A traversal-only clustering overlay (walk grouped slab cells, map results back to
    cells). Out because an exact walk over clusters is still unbounded. It could later
    complement the rect walk by cutting its constant.
  - Precomputed visibility: `rendering_pipeline.md` §2 rejects it.
  - Multi-rect or coverage-bitmask regions. The next step if tightness becomes the
    problem.
  - Parallelizing the walk. This brief removes the cost premise behind the
    parallelization gate in `drafts/portal-walk-cpu-instrumentation`; that draft still
    owns walk timing.

## Acceptance

### Automated

Superset guarantee, over randomized cameras on synthetic fixtures (a stacked-slab
shaft, an open 3D lattice, cyclic rings) and the named cases:
- [ ] Every cell the exact walk reaches is reached. The exact walk has one allowed
  exception: a cell it reaches only through a final clipped polygon wholly outside the
  view frustum, or through a near-zero sliver crossing a portal backward. Each exception
  is classified by that check; an unclassified one fails.
- [ ] A cell reachable by two paths with different openings is visible through the
  wider one, under both portal orders.
- [ ] A portal crossing the camera plane, with some vertices behind the eye, includes
  its neighbour.
- [ ] A camera on a portal polygon, or just inside a portal's wall, sees through it.
- [ ] A rect that grows through a cycle after its descendants expanded re-expands them;
  newly exposed cells become visible.
- [ ] A portal tangent to a rect edge or a snap boundary is included.
- [ ] The camera cell is always visible.

Must still exclude:
- [ ] Existing occlusion cases still hide: the far end of an L-shaped corridor, and an
  unreachable side branch behind a straddling portal.
- [ ] A portal wholly behind the camera is rejected.
- [ ] A blocked portal blocks in both directions.
- [ ] A distant portal coplanar with the camera but far from it is clipped normally; its
  cell is hidden when the opening lies outside the source rect.

Bounded cost:
- [ ] On the stacked-slab fixture, the exact walk exceeds the cap. The new walk makes at
  most a small constant times the summed degree of reached cells in portal tests, and
  never trips.
- [ ] Every cell's expansions stay under a constant set by the snap grid, including the
  highest-degree cell.
- [ ] The open lattice terminates with bounded stack depth.
- [ ] Shuffling each cell's portal list yields an identical visible set.
- [ ] A forced cap trip takes the bounded frustum-set fallback, never the draw-all set.

### Manual

- [ ] Measurement: sweep stress-warren-hallway-inspection with the real traversal, at
  the poses and headings in `research.md`. Expect zero cap trips and zero unclassified
  misses. Report portal-test counts and CPU µs (p50, p99, p99.9, max). Report extra cells
  vs exact (p50, p95, max), overall and for the shafts and the arena, against the
  prototype numbers in `research.md`. At the shaft and arena poses, also report the
  change in visible SH clusters and in shadow-eligible lights vs the exact walk.
- [ ] Live: at the shaft A stair room and in the arena, the title bar never shows the
  step-limit path. Report frame time relative to the pre-change build at the same poses.
- [ ] Visual: walk the shaft spirals, turn at portal boundaries, and stand in doorways
  and on portal planes. No geometry pops out or goes missing.

## Path

- Seams: `flood`, `portal_traverse_detailed`, `clip_polygon_to_frustum`,
  `narrow_frustum`, `camera_on_polygon_plane` (and `APEX_ON_PORTAL_PLANE_EPSILON`),
  `MAX_PORTAL_WALK_STEPS`, `PortalTraversalStats` (considered and accepted, read by the
  title bar and diagnostics), `VisibilityPath::PortalStepLimitFallback`.
- `crates/visibility/src/portal_vis.rs` is past 2,000 lines. Split it first,
  behavior-preserving, in its own commit.
- Reference implementation: `prototype/src/alt_vis.rs`, the `rect256` variant. It is a
  scratch harness with absolute path dependencies; read it, don't merge it.
- Measured choices the prototype supports:
  - Worklist front-to-back by distance to the nearest clipped portal vertex (FIFO
    re-processed about 3× more).
  - Cross a portal only from the camera's side, using the camera cell locator.
  - Clip portals crossing the camera plane in 3D, then project. Handing the child the
    parent's region made looseness far worse.
- Keep the exact chain walk as a test-only oracle for the superset property.
- The swept map's `.prl` is gitignored, so the map sweep stays a measurement, not CI.
  The fixtures carry the automated proof.
- First slice: the rect walk behind the existing entry point, plus the superset oracle
  on the stacked-slab fixture. It falsifies the riskiest assumption: conservativeness at
  the near plane and under floating-point slop.

## Open questions

- Snap grid resolution and the lowered cap value — **delegated** (measured: 256 steps
  per NDC unit; a 5,000-step cap never trips on the swept map).
- "Near the polygon" distance for the plane-bypass tightening — **delegated**.
- Where the sweep instrument lives (checked-in example or bench vs the plan's
  `prototype/`) — **delegated**.
