# portal-walk-bounded-regions

Brief · compact · reads: `context/lib/rendering_pipeline.md` §2, `context/lib/build_pipeline.md` §Runtime visibility, `context/lib/testing_guide.md` · read at 27ca6a6 · evidence: `research.md`, `prototype/`

## Problem

Developer-observed defect, confirmed by an offline sweep of the real traversal: on
stress-warren-hallway-inspection, a stair room of floating shaft platforms makes the
runtime portal walk spend milliseconds per frame and trip its step cap, and the
bounded fallback then draws several times the visible cells. Cause: the walk
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
  clipped against its source cell's rect, its screen bounds are unioned into the
  destination cell's rect, and a cell expands again only when its rect grows. This is
  how Descent, Source and Thief bound portal cost; the id Tech 4 walk we ported relies
  on hand-placed portals, which compiler cells lack.
- **Rect bounds snap outward to a fixed grid.** Snapping caps how often a cell can grow,
  makes the visible set independent of portal order, and keeps every opening at least
  one grid cell wide, so edge-on slivers stay conservative.
- **The visible set becomes a conservative superset of the exact portal set.** No
  consumer reads a visible cell as line of sight (consumer table in `research.md`).
  Fixed-capacity consumers (shadow slot ranking, the mesh instance cap) and
  non-evictable SH and lightmap demand, which also enlarges the settle set, pay in
  quality and cost, and are measured. The set keeps today's shape: valid cell ids, a
  drawable draw set, non-solid fog reach that is never empty on walk paths, and blocked
  portals block. This diverges from
  `plans/done/perf-visible-cell-candidate-cull` and `perf-dynamic-light-pvs-cull`,
  which describe the output as "exact"; output identity rests on spans matching
  visible cells, which a superset keeps, and `cell-visibility-relation`'s
  superset-only gate rule points the same way.
- **The camera-on-portal-plane bypass applies only near the polygon.** Today it tests
  the portal's infinite plane, so a distant portal coplanar with the camera skips
  clipping, the largest source of over-inclusion. The bypass still covers a camera on
  or beside the portal itself.
- **The step cap stays as a backstop and is lowered.** A trip takes the bounded
  frustum-set fallback. With the bound, the cap never trips on the swept map, so it
  guards only pathological content.
- **Rect, not octagon or hybrid.** The octagon saved a fraction of a cell per frame for
  more steps and CPU. Hybrids were tighter but costlier, and the keep-going hybrid's
  output depends on portal order. Measurements are in `research.md`.
- **The id-51 bake samples the new walk.** The change bumps `PORTAL_WALK_EPOCH`, which
  the id-51 cache key hashes, so one walk keeps one meaning. Rect over-inclusion depends
  on the projection, so runtime can draw cells outside a camera cell's baked set. Those
  cells are occluded, but their never-refused lightmap and SH demand can grow both
  pools, and a truly visible block that waits on a retiring pool generation misses
  visibly.
- **This brief owns the bake gap.** The gap is material when, at any of 100° 16:9,
  130° 16:9 or 130° 21:9, the rect walk exceeds the exact walk on pool growths, peak
  pool layers, or visible misses on the hallway walks at default levers. A material
  gap widens the bake's sampling projections in this change, until the rect walk is
  back within that line. Widening stays conditional because it enlarges every map's
  mandatory set, preload and settle set.
- **Placement:** runtime traversal in the visibility crate. Consumers and their inputs
  keep their shape; only the set's contract loosens.
- **Non-goals:**
  - Compiler detail brushes or cell merging. Out of scope because they change cell
    granularity for fog masks, SH clusters, lightmap blocks and draw indexing, and add
    an authoring surface. They remain a possible later authoring optimization.
  - A traversal-only clustering overlay (walk grouped slab cells, map results back to
    cells). Out because an exact walk over clusters is still unbounded. It could later
    complement the rect walk by cutting its constant.
  - Precomputed visibility: `rendering_pipeline.md` §2 rejects it.
  - Parallelizing the walk. This brief removes the cost premise behind it; the
    existing `portal_walk` CPU stage keeps measuring walk time.

## Acceptance

Pins (O1–O12) are in `research.md` §Ordering pins.

### Automated

Superset guarantee, over randomized cameras on synthetic fixtures (a stacked-slab
shaft, an open 3D lattice, cyclic rings) and the named cases:
- [ ] Every cell the exact walk reaches is reached. The exact walk has one allowed
  exception: a cell it reaches only through a final clipped polygon wholly outside the
  view frustum, or through a chain that crosses a portal from the side away from the
  camera. A ray crosses a portal plane once, so the second class is a sign test with no
  area threshold. Each exception is classified by that check; an unclassified one
  fails.
- [ ] The randomized cameras come from a fixed seed and count, so a failure replays. Any
  camera where the oracle hit its own cap is reported, not counted as full proof.
- [ ] Every existing traversal test that expects a cell visible still passes on its
  current fixture. Portal sides come from data those fixtures already carry. (O1)
- [ ] A cell reachable by two paths with different openings is visible through the
  wider one, under both portal orders.
- [ ] A portal crossing the camera plane, with some vertices behind the eye, includes
  its neighbour.
- [ ] A camera on a portal polygon, or just inside a portal's wall, sees through it.
- [ ] A camera on a portal's plane, just outside its edge but within the bypass
  distance, sees through it in both directions, whichever side cell the locator
  reports. Just beyond that distance, the portal is clipped normally. A camera on a
  plane shared by several stacked-slab portals sees the cells above and below it and
  their lateral neighbours. (O8, O10)
- [ ] A camera within the render near distance of two consecutive portals sees the cell
  past the second one. (O9)
- [ ] A cell whose rect grows after its descendants expanded re-expands them. The
  growth can arrive through a longer path or through a cycle. The test confirms the
  cell expanded more than once, and the newly exposed cells become visible. (O2)
- [ ] A contribution that grows a rect by one grid step expands the cell again. A
  contribution that adds nothing does not. (O3)
- [ ] A portal tangent to a rect edge or a snap boundary is included.
- [ ] The camera cell is always visible.

Set shape and state:
- [ ] At every camera in the superset sweep, the walk returns only in-range cell ids.
  The draw set holds only drawable cells. Fog reach holds only non-solid cells and
  always the camera cell, so it is never empty. The draw set is exactly the drawable
  part of fog reach. (O4, O12)
- [ ] Back-to-back walks from one caller each match a walk from fresh state. That holds
  when a door closes or opens between them, and when the camera crosses into the next
  cell. Walks run at once on several threads match too. (O7)

Must still exclude:
- [ ] Existing occlusion cases still hide: the far end of an L-shaped corridor, and an
  unreachable side branch behind a straddling portal.
- [ ] A cell inside the camera frustum, reachable only through a small opening whose
  rect misses its portal, stays hidden.
- [ ] A portal wholly behind the camera is rejected.
- [ ] A blocked portal blocks in both directions, including a closed door that seals a
  closet: the interior stays out of both the draw set and fog reach.
- [ ] A distant portal coplanar with the camera but far from it is clipped normally; its
  cell is hidden when the opening lies outside the source rect.

Bounded cost:
- [ ] On the stacked-slab fixture, the exact walk exceeds the step cap in force before
  this change. The new walk makes at most k times the summed degree of reached cells in
  portal tests, and never trips. k is a fixed number in the test.
- [ ] On every fixture, no cell expands more than 32 times, including the
  highest-degree cell.
- [ ] On the open lattice, the new walk finishes under the cap when run on a thread with
  a small fixed stack.
- [ ] Every outbound portal the walk looks at counts once toward the cap and the
  portal-test count, including portals that face away, are blocked, or lead to solid
  cells. (O11)
- [ ] The step cap is below its old value of 20,000.
- [ ] Shuffling each cell's portal list yields an identical visible set.
- [ ] A forced cap trip takes the bounded frustum-set fallback, never the draw-all set.
- [ ] A walk the cap stops returns only cells the uncapped walk reaches, and always the
  camera cell. (O5)
- [ ] For the same camera, the cap-trip fallback's fog reach contains everything the
  uncapped walk reaches. This includes a camera beside a portal, looking away from it.
  (O6)
- [ ] No comment or report text calls the bake's sampled set a lower bound on visibility
  or says it holds for any FOV. The dry run's report states the projections the bake
  actually samples. This is a grep gate.
- [ ] The portal-walk epoch differs from its value before this change. If the bake
  widens, the cell-residency stage version differs too.

### Manual

- [ ] Measurement: sweep stress-warren-hallway-inspection with the real traversal, at
  the poses and headings in `research.md`, single-threaded release. Name the machine,
  and compare against a pre-change build on the same machine. Expect zero cap trips and
  zero unclassified misses. Report portal-test counts and CPU µs (p50, p99, p99.9,
  max). Report extra cells vs exact (p50, p95, max), overall and for the shafts and the
  arena, against the prototype numbers in `research.md`. At the shaft and arena poses,
  also report the change in visible SH clusters, shadow-eligible lights and collected
  mesh instances vs the exact walk.
- [ ] Pitch coverage: at the shaft poses, looking straight up and straight down, the new
  walk never trips the cap. The id-51 bake with the new walk reports no step-limit
  walks on the hallway.
- [ ] Materiality: run the lightmap walk measurement for each walk on the hallway
  random and tour walks at default levers, at 100° 16:9, 130° 16:9 and 130° 21:9.
  Report outside-set block-frames, pool growths, peak layers, and visible misses:
  blocks of cells the exact walk reaches, drawn while not resident. State the verdict.
  If material, rerun the table with the widened bake: the rect walk is within the
  line. Report the widened mandatory set per camera cell (p50, p95, max blocks) and
  the id-51 stage time.
- [ ] Bake gap: bake id-51 with each walk (exact, then rect). Over the sweep poses at the
  default FOV and at 130° with 16:9 and 21:9 aspect, report runtime-visible cells
  outside the camera cell's baked set (p50, p95, max) for each walk. Also report the
  baked mandatory set per camera cell (p50, p95, max cells and lightmap blocks) for
  each walk: over-inclusion accumulates across the bake's samples, and those blocks
  are never refused.
- [ ] Lightmap cost: along the churn route, report for each walk the visible lightmap
  blocks and bytes, pool growth events, peak pool layers, and misses from blocks
  waiting on a retiring generation. Report the id-51 stage time before and after on
  the hallway.
- [ ] Repeat the lightmap-cost row on movement-feel, whose cells span several blocks, and
  report blocks per over-included cell.
- [ ] At every entry pose and projection the Settle row measures, the settle frames take
  the walk path, never the step-limit fallback. (O13)
- [ ] Settle: entering the hallway, and teleporting to the shaft and arena poses, report
  for each walk the settle set (lightmap blocks and SH targets) and the time to reveal.
- [ ] Along the churn route, report lightmap bytes read and physical reads for each
  walk. (O15)
- [ ] At 130° with 16:9 and 21:9 aspect, the rect walk's measurement runs report zero
  step-limit frames.
- [ ] SH reach: rerun the SH reach measurement on the hallway and campaign-test with the
  rect bake, and with the widened bake if the bake widens. Report worst and p95 SH
  mandatory bytes per camera cell against the exact bake. The worst cell stays under
  the SH floor.
- [ ] Churn: along a walking route through the shafts and the arena, recorded in
  `research.md` and replayed for each walk,
  report per-frame cells entering and leaving the visible set (p50, p95, max). Snapped
  over-inclusion can flicker cells that drive non-evictable SH and lightmap demand.
- [ ] Live: at the shaft A stair room and in the arena, with CPU timing on, the log shows
  no step-limit marker. Report frame time relative to the pre-change build at the same
  poses.
- [ ] Visual: walk the shaft spirals, turn at portal boundaries, and stand in doorways
  and on portal planes. No geometry pops out or goes missing. At the same poses, no
  in-view dynamic shadow drops compared with the pre-change build.

## Path

- Seams: `flood`, `portal_traverse_detailed`, `clip_polygon_to_frustum`,
  `narrow_frustum`, `camera_on_polygon_plane` (and `APEX_ON_PORTAL_PLANE_EPSILON`),
  `MAX_PORTAL_WALK_STEPS`, `VisibilityPath::PortalStepLimitFallback`.
  `PortalTraversalStats` is crate-private; its counters leave through
  `cpu_stages::record_walk` as `VisibilityStage` counts. The title bar reads
  `VisibilityStats` (path label, `walk_reach`).
- CPU µs: `POSTRETRO_CPU_TIMING=1` logs `portal_walk` and `walk_considered` per
  120-frame window. The offline sweep times the walk directly.
- `crates/visibility/src/portal_vis.rs` is past the split threshold, mostly its tests
  module.
  Split it first, behavior-preserving, in its own commit.
- Reference implementation: `prototype/src/alt_vis.rs`, the `rect256` variant. It is a
  scratch harness with absolute path dependencies; read it, don't merge it.
- Measured choices the prototype supports:
  - Worklist front-to-back by distance to the nearest clipped portal vertex (FIFO
    re-processed more; see the variant table).
  - Cross a portal only from the camera's side, using the camera cell locator.
  - Clip portals crossing the camera plane in 3D, then project. Handing the child the
    parent's region made looseness far worse.
- Keep the exact chain walk as a test-only oracle for the superset property. It takes
  the same near-polygon bypass tightening as the new walk.
- Keep the region behind one type, so swapping in the octagon stays local if a future
  map measures looser.
- After the epoch bump, rebake the measured maps.
- Tests that assert exact sets stay as regression guards: the closet-reveal door tests
  in the engine and compiler, and the SH controller doorway test.
- `cell_residency_bake::pvs_sampling` and the dry run's `visible_set` and report text
  call the sampled set a lower bound on visibility that holds for any FOV; a dry-run
  test pins the report string. The rect walk falsifies both claims; restate them.
- The id-51 bake's step-limit branch (`walk_cube_faces`) re-runs the walk for a
  truncated result. With the bound it should not fire; leave it.
- `walk_cube_faces` builds one constant square projection over `CUBE_FACES`. Widening
  turns it into a list, bumps `CELL_RESIDENCY_SET_STAGE_VERSION`, and updates the dry
  run's "6 cube faces" text.
- `walk_measurement` fixes FOV and aspect as constants and measures lightmaps only; its
  note that SH does not stream is stale. Thread FOV and aspect through its configs.
  SH settle cost is measured in-engine.
- The swept map's `.prl` is gitignored, so the map sweep stays a measurement, not CI.
  The fixtures carry the automated proof.
- First slice: the rect walk behind the existing entry point, plus the superset oracle
  on the stacked-slab fixture. It falsifies the riskiest assumption: conservativeness at
  the near plane and under floating-point slop.

## Open questions
- Snap grid resolution and the lowered cap value — **delegated** (measured: a grid of
  256 cells per NDC unit; a 5,000-step cap never trips on the swept map).
- "Near the polygon" distance for the plane-bypass tightening — **delegated**.
- Where the sweep instrument lives, how it reaches the tightened exact oracle, and how
  it counts SH clusters, shadow-eligible lights and mesh instances — **delegated**. The
  prototype's oracle keeps the old bypass and depends on Windows paths.
- The portal-test constant k — **delegated**: the prototype's worst case with at most
  2× headroom.
- Which projections and view directions the widened bake adds, if the gap is material
  — **delegated**.
