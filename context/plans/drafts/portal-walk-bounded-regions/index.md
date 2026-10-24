# portal-walk-bounded-regions

Brief · compact · reads: `context/lib/rendering_pipeline.md` §2, §4, `context/lib/build_pipeline.md` §Runtime visibility, `context/lib/testing_guide.md` · read at 27ca6a6 · evidence: `research.md`, `prototype/`

## Problem

Developer-observed defect, confirmed by an offline sweep of the real traversal: on
stress-warren-hallway-inspection, a stair room of floating shaft platforms makes the
portal walk spend milliseconds per frame and trip its step cap, and the bounded
fallback then draws several times the visible cells. Cause: the walk enumerates every
portal chain that survives narrowing and keeps no per-cell state, so its cost follows
the paths into each cell, and stacked slab cells multiply those paths. When this is
done, the walk `rendering_pipeline.md` §2 records as decided is built: cost is bounded
per visible cell, the visible set is a conservative superset with small measured
over-inclusion, and no ordinary pose reaches the step-cap fallback.

## Decisions

- **Build §2's bounded-region walk, with a rect as the region.** Descent bounds portal
  cost the same way, re-queuing a segment only when its region grows; Source unions a
  per-area rect. The id Tech 4 walk relies on hand-placed portals, which compiler cells
  lack.
- **The visible set follows §2's superset contract.** It diverges from
  `plans/done/perf-visible-cell-candidate-cull` and `perf-dynamic-light-pvs-cull`,
  which call the output exact; their output identity rests on spans matching visible
  cells, which a superset keeps.
- **The camera-on-portal-plane bypass applies only near the polygon.** Today it tests
  the infinite plane, so a distant coplanar portal skips clipping: the largest source
  of over-inclusion.
- **The id-51 bake samples the new walk.** The change bumps `PORTAL_WALK_EPOCH`, which
  the id-51 cache key hashes, so one walk keeps one meaning.
- **The bake gap is gated, not widened.** Runtime draws over-included cells outside
  the baked set, and their demand is never refused; widening the bake would enlarge the
  never-refused mandatory set instead (`research.md` §Bake-gap ruling). The gate counts
  lightmap blocks the pool deferred, never more than the pre-change build defers. It
  charges the whole change: deferrals from the rect-sampled bake's larger mandatory set
  count alongside runtime over-inclusion. Where the pre-change build defers none, the
  rect walk must defer none. A failed gate stops the build for an owner decision.
- **This brief lands after `sh-streaming--pool-growth-device-limit`** (in `ready/`).
  Today SH pool growth past a device limit ends the session; that brief makes it refuse
  a cluster instead, and sizes dense layers from the whole level, which bounds the
  superset's added demand.
- **Placement:** runtime traversal in the visibility crate. Consumers and their inputs
  keep their shape.
- **Non-goals:**
  - Compiler detail brushes or cell merging: they change cell granularity for fog
    masks, SH clusters, lightmap blocks and draw indexing, and add an authoring
    surface.
  - A traversal-only clustering overlay: an exact walk over clusters is still
    unbounded.
  - Precomputed visibility: §2 rejects it.
  - Parallelizing the walk: this brief removes its cost premise.
  - Refusable outside-set demand: it would also refuse the bake's genuine sampling
    misses, the error never-refuse exists to prevent.
  - Line-of-sight-aware shadow slot ranking and mesh instance cap: both rank inside the
    visible set with no line-of-sight term (the mesh cap orders by visibility tier, then
    first seen), so an over-included light or mesh can take a slot. Drops are reported,
    not fixed here.

## Acceptance

Pins are in `research.md` §Ordering pins.

### Automated

Superset guarantee, over randomized cameras on synthetic fixtures (a stacked-slab
shaft, an open 3D lattice, cyclic rings) and the named cases:
- [ ] Every cell the exact walk reaches is reached. Allowed exceptions: a cell the
  exact walk reaches only through a final clipped polygon wholly outside the view
  frustum, or through a chain that crosses a portal from the side away from the camera
  (a sign test, no area threshold). An unclassified exception fails.
- [ ] The randomized cameras come from a fixed seed and count. Any camera where the
  oracle hit its own cap is reported, not counted as full proof.
- [ ] Every existing traversal test that expects a cell visible still passes on its
  current fixture. (O1)
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
- [ ] A cell whose rect grows after its descendants expanded re-expands them, and the
  newly exposed cells become visible. (O2)
- [ ] A contribution that grows a rect by one grid step expands the cell again. One that
  adds nothing does not. (O3)
- [ ] A portal tangent to a rect edge or a snap boundary is included.
- [ ] The camera cell is always visible.

Set shape and state:
- [ ] At every camera in the superset sweep, cell ids are in range, the draw set holds
  only drawable cells, fog reach holds only non-solid cells and always the camera cell,
  and the draw set is exactly the drawable part of fog reach. (O4, O12)
- [ ] Back-to-back walks from one caller each match a walk from fresh state, across a
  door toggle and a camera cell change. Concurrent walks on several threads match too.
  (O7)

Must still exclude:
- [ ] The far end of an L-shaped corridor, and an unreachable side branch behind a
  straddling portal, stay hidden.
- [ ] A cell inside the frustum, reachable only through a small opening whose rect
  misses its portal, stays hidden.
- [ ] A portal wholly behind the camera is rejected.
- [ ] A blocked portal blocks in both directions; a closed door keeps a closet out of
  both the draw set and fog reach.
- [ ] A distant portal coplanar with the camera is clipped normally; its cell is hidden
  when the opening lies outside the source rect.

Bounded cost:
- [ ] On the stacked-slab fixture, the exact walk exceeds the step cap in force before
  this change. The new walk makes at most k times the summed degree of reached cells in
  portal tests, and never trips. k is a fixed number in the test.
- [ ] On every fixture, no cell expands more than 32 times.
- [ ] On the open lattice, the new walk finishes under the cap on a thread with a small
  fixed stack.
- [ ] Every outbound portal the walk looks at counts once toward the cap and the
  portal-test count, including facing-rejected, blocked and solid-leading ones. (O11)
- [ ] The step cap is below its value before this change.
- [ ] Shuffling each cell's portal list yields an identical visible set.
- [ ] A forced cap trip takes the bounded frustum-set fallback, never the draw-all set.
- [ ] A walk the cap stops returns only cells the uncapped walk reaches, and always the
  camera cell. (O5)
- [ ] The cap-trip fallback's fog reach contains everything the uncapped walk reaches
  from the same camera, including a camera beside a portal looking away from it. (O6)
- [ ] The portal-walk epoch differs from its value before this change.
- [ ] Under the level compiler's cell-residency bake and lightmap residency dry run, no
  comment, report string or test assertion matches "lower bound" or "any (runtime )?FOV".
  This is a grep gate over those two module trees.
- [ ] The tests that make a drawn cell outside the baked set never-refused visible demand
  pass unchanged. The bake's sampling projections and the cell-residency stage version
  match their values before this change; only the walk epoch moves its cache key.

### Manual

Name the machine. "Pre-change" is the last commit before the walk change, carrying this
brief's measurement modes but the old walk and epoch, on the base commit's bake. (O14, O19)
Both builds share a base containing the SH pool device-limit fix; a row run before it
landed is rerun. (O20)
- [ ] Sweep: run the real traversal over the hallway poses and headings in
  `research.md`, single-threaded release, plus straight up and down at the shaft poses.
  Zero cap trips and zero unclassified misses. Report portal-test counts, CPU µs (p50,
  p99, p99.9, max), and extra cells vs exact (p50, p95, max) overall, for the shafts
  and for the arena, against the prototype numbers. At the shaft and arena poses,
  report the change in visible SH clusters and shadow-eligible lights.
  The id-51 bake reports no step-limit walks on the hallway.
- [ ] Residency gate: run the lightmap walk measurement on the hallway random and tour
  walks at default levers, at 100° 16:9, 130° 16:9 and 130° 4:3, at pitch 0 and ±45°,
  with reads completed before each frame draws. At every projection and pitch, the rect
  walk's pool-deferred block-frames (behind a retiring generation or at the device
  limit) do not exceed the pre-change build's, and it reports zero step-limit frames. For both builds, report outside-set
  block-frames, pool growths, peak occupied layers, bytes read and physical reads
  (O15), the baked mandatory set per camera cell (p50, p95, max blocks), and the id-51
  stage time. Spawn frames are reported apart. Each run uses one drain per frame for
  retirement, and the pre-change build runs the same synchronous mode. For both builds,
  report non-resident block-frames by cause: left behind by admission, deferred behind a
  retiring generation, deferred at the device limit. A drawn block still non-resident
  after the frame's last drain counts as pool-deferred when that drain carried and
  deferred it; the drain's device-limit flag gives the cause. Any other drawn
  non-resident block counts as left behind by admission. Each block counts once per
  frame. The walk measurement models no device layer limit, so that bucket reads zero
  by construction. (O17, O18) Repeat on movement-feel,
  whose cells span several blocks, and report outside-set blocks per drawn cell outside
  the baked set, for both builds.
- [ ] SH reach: rerun the SH reach measurement on the hallway and campaign-test with
  each build's bake. Report worst and p95 SH mandatory bytes per camera cell. The worst
  cell stays under the SH floor.
- [ ] Settle: enter the hallway at its spawn, and at the shaft and arena poses through the
  start-pose override, at pitch 0 and ±45° at the shaft poses, each a fresh launch.
  Settling logs, per entry, any frame whose walk took the step-limit fallback and the
  settle set's peak size in lightmap blocks and SH targets; this row reads that log.
  Settle frames take the walk path, never the step-limit fallback, and no entry hits the
  settle timeout or logs its warning. For each
  build, report the settle set (lightmap blocks and SH targets), the settle hold time,
  and misses counted on the reveal frame. (O13, O16, O19, O21)
- [ ] Live: at the shaft A stair room and in the arena, with CPU timing on, the log shows
  no step-limit marker. Report frame time and collected mesh instances against the
  pre-change build at the same poses.
- [ ] Visual: walk the shaft spirals, turn at portal boundaries, and stand in doorways
  and on portal planes. No geometry pops out or goes missing. Report any in-view
  dynamic shadow that drops compared with the pre-change build.

## Path

- Seams: `flood`, `portal_traverse_detailed`, `clip_polygon_to_frustum`,
  `narrow_frustum`, `camera_on_polygon_plane` (`APEX_ON_PORTAL_PLANE_EPSILON`),
  `MAX_PORTAL_WALK_STEPS`, `VisibilityPath::PortalStepLimitFallback`. Walk counters
  leave through `cpu_stages::record_walk`; the title bar reads `VisibilityStats`.
- Later caller: `planar-water-reflections` lands after this brief and floods a
  mirror view from seed cells with no apex cell (spike: `portal_traverse_seeded`,
  `determine_mirror_visible_cells`). A seeded entry here saves it a second walk.
- `crates/visibility/src/portal_vis.rs` is past the split threshold, mostly tests.
  Split it first, behavior-preserving, in its own commit.
- Reference implementation: `prototype/src/alt_vis.rs`, the `rect256` variant. Read
  it, don't merge it. Its measured choices: front-to-back worklist by nearest clipped
  vertex, cross a portal only from the camera's side, clip camera-plane-crossing
  portals in 3D before projecting.
- Rival shape: a per-cell octagon, tighter under pitch for more steps and CPU. Keep the
  region behind one type so the swap stays local.
- Keep the exact chain walk as a test-only oracle, with the same bypass tightening.
- `walk_measurement` fixes FOV, aspect and pitch as constants and needs a mode that
  completes reads before the frame draws. Its note that SH does not stream is stale.
- `cell_residency_bake::pvs_sampling` and the dry run's `visible_set` call the sampled
  set a lower bound for any FOV, and a dry-run test pins that report string. Restate
  them. The bake's step-limit branch should not fire; leave it.
- After the epoch bump, rebake the measured maps. The hallway `.prl` is gitignored, so
  its sweep stays a measurement; fixtures carry the automated proof.
- Exact-set tests stay as regression guards: the closet-reveal door tests and the SH
  controller doorway test.
- At landing, §2 drops "decided, not yet built" and its description of the chain walk.
- First slice: the rect walk behind the existing entry point, plus the superset oracle
  on the stacked-slab fixture. It tests conservativeness at the near plane and under
  floating-point slop.

## Open questions
- Snap grid resolution and the lowered cap value — **delegated** (`research.md`
  §Variant results).
- "Near the polygon" distance for the bypass tightening — **delegated**.
- Where the sweep instrument lives and how it reaches the tightened oracle —
  **delegated**.
- The portal-test constant k — **delegated**: the prototype's worst case with at most
  2× headroom.
