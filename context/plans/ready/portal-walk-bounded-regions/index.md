# portal-walk-bounded-regions

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §2, §4, §7.3a, `context/lib/build_pipeline.md` §Runtime visibility, `context/lib/development_guide.md` §1.4, `context/lib/testing_guide.md` · read at 800bbe186 · evidence: `research.md`, `assessment-2026-10-25.md`, `prototype/`

## Problem

Developer-observed defect, confirmed by offline sweeps of the real traversal and a live
run: the portal walk trips its step cap on ordinary poses. On
stress-warren-hallway-inspection, a stair room of floating shaft platforms makes the walk
spend milliseconds and trip, and the bounded fallback then draws several times the visible
cells. On reflection-lake-test, the map planar water reflections build on, it trips on
every frame at spawn and the walk alone takes about a third of a dev build's frame CPU;
occlusion-test trips too (`research.md` §Re-grounding at 800bbe186). Cause: the walk
enumerates every portal chain that survives narrowing and keeps no per-cell state, so its
cost follows the paths into each cell, and compiler-split cells multiply those paths. When
this is done, the walk `rendering_pipeline.md` §2 records as decided is built, for the
camera and for the planar mirror's seed cells: cost is bounded per visible cell, the
visible set is a conservative superset whose over-inclusion is measured and reported per
map, and no ordinary pose reaches the step-cap fallback.

## Decisions

- **Build §2's bounded-region walk, with a rect as the region.** Descent's growth-gated
  re-queue is the cost precedent; compiler cells lack the hand-placed portals id Tech 4's
  chain walk relies on (`research.md` §Algorithm survey).
- **One walk, two entries.** The camera entry starts from the located camera cell with the
  near plane slid to the eye. The seeded entry, for `planar-water-reflections` (§2, §7.3a),
  starts from caller-given seed cells, never calls the locator, and takes the caller's eye,
  view-projection and near plane as given; the bypass never overrides that near plane.
  Seeds share one per-cell region state and one step cap, every usable seed is queued with
  the caller's full screen before any cell expands, and a cap trip is returned to the
  caller, which picks its own fallback. The walk skips seeds that are out of range, solid,
  exterior or wholly off screen; filtering the result into draw and fog sets, and wiring
  the mirror, stay with that brief.
- **The visible set follows §2's superset contract.** It diverges from
  `plans/done/perf-visible-cell-candidate-cull` and `perf-dynamic-light-pvs-cull`, which
  call the output exact; both guarantees survive a superset (`research.md` §Visible-set
  consumers).
- **The camera-on-portal-plane bypass applies only near the polygon,** not on its infinite
  plane, where a distant coplanar portal skips clipping: the sweep's worst over-inclusion
  (`research.md` §Camera-on-plane bypass). A bypassed neighbour receives the source cell's
  rect, never the full screen, and a blocked portal blocks before the bypass is tested.
- **The step-limit fallback's fog reach also holds every cell whose bounds come within the
  bypass distance of the eye;** its draw set keeps the camera frustum. That keeps fallback
  fog reach a superset of the walk's, bypassed neighbours behind or beside the eye
  included.
- **The chain walk stays as the test-only oracle,** with the tightened bypass, and never
  floods outside its parent frustum through a bypassed portal. Acceptance's "exact walk"
  means this oracle.
- **The id-51 bake samples the new walk.** The change bumps `PORTAL_WALK_EPOCH`, which the
  id-51 cache key hashes, so one walk keeps one meaning.
- **The bake gap is gated, not widened** (`research.md` §Bake-gap ruling). The gate charges
  the whole change, bake and runtime, against the pre-change build: lightmap block-frames
  deferred behind a retiring generation never exceed its count, and zero stays zero.
  Frames where the pre-change walk trips demand only the camera cell's baked set in that
  build, so they, and the frames after them until a pool growth begun on them has retired,
  leave the gate and are reported apart (O28). A failed gate stops the build for an owner
  decision.
- **Placement:** runtime traversal in the visibility crate. Callers of
  `determine_visible_cells` keep its inputs and outputs; the scratch they pass may become a
  caller-owned walk scratch.
- **Non-goals:**
  - Compiler detail brushes or cell merging: they change cell granularity for fog
    masks, SH clusters, lightmap blocks and draw indexing, and add an authoring
    surface.
  - A traversal-only clustering overlay: an exact walk over clusters is still
    unbounded.
  - Precomputed visibility: §2 rejects it.
  - Refusable outside-set demand: it would also refuse the bake's genuine sampling
    misses (`research.md` §Owner decisions).
  - Line-of-sight-aware shadow slot ranking and mesh instance cap: neither ranks by line
    of sight (`research.md` §Owner rulings), so an over-included light or mesh can take a
    slot. Drops are reported, not fixed here.

## Acceptance

Pins are in `research.md` §Ordering pins; report sets R1–R4 in §Report sets. Rows marked
*guard* pass on today's walk and must still pass.

### Automated

Superset guarantee, over randomized cameras on synthetic fixtures (a stacked-slab
shaft, an open 3D lattice, cyclic rings) and the named cases. Each camera's cell is the
fixture cell that contains it, from a locator built for the fixture or passed in
directly, never a one-leaf locator's fixed answer.
- [ ] Every cell the exact walk reaches is reached. Allowed exceptions: a cell the
  exact walk reaches only through a final clipped polygon wholly outside the view
  frustum, or through a chain that crosses a portal from the side away from the camera
  (a sign test, no area threshold). An unclassified exception fails.
- [ ] The randomized cameras come from a fixed seed and count. The oracle's own cap is
  far above today's cap, and at least one stacked-slab camera whose exact walk exceeds
  today's cap completes under the oracle. Any camera where the oracle hits its cap or its
  chain-depth limit is reported, not counted as full proof.
- [ ] *Guard:* every existing traversal test that expects a cell visible still passes on
  its current fixture. (O1)
- [ ] *Guard:* a cell reachable by two paths with different openings is visible through
  the wider one, under both portal orders.
- [ ] A portal crossing the camera plane, with some vertices behind the eye, includes
  its neighbour.
- [ ] *Guard:* a camera on a portal polygon, or just inside a portal's wall, sees through
  it.
- [ ] A camera on a portal's plane, just outside its edge but within the bypass
  distance, sees through it in both directions, whichever side cell the locator
  reports. Just beyond that distance, the portal is clipped normally. A camera on a
  plane shared by several stacked-slab portals sees the cells above and below it and
  their lateral neighbours. (O8, O10)
- [ ] A camera exactly on a distant portal's plane: any cell the exact walk reaches
  through that portal and the new walk misses is a classified exception. (O36)
- [ ] A camera within the render near distance of two consecutive portals sees the cell
  past the second one. (O9)
- [ ] A camera within the bypass distance of a portal between two cells, neither of them
  the camera cell: the bypass hands the far cell no more than the near cell's rect, and
  neither cell re-expands from crossing back. (O23)
- [ ] A cell whose rect grows after its descendants expanded re-expands them, and the
  newly exposed cells become visible. (O2)
- [ ] A contribution that grows a rect by one grid step expands the cell again. One that
  adds nothing does not. (O3)
- [ ] A portal tangent to a rect edge or a snap boundary is included.
- [ ] *Guard:* on the camera entry, the camera cell is always visible.

Seeded entry:
- [ ] A walk seeded from several cells, none holding the eye, under a caller's oblique
  near plane, reaches every cell the oracle seeded from the same cells reaches, with the
  superset check's exceptions. The fixture's locator answers a cell no seed reaches, and
  that cell stays hidden. Shuffling the seeds changes neither the set nor the
  portal-test count. (O27, O33)
- [ ] A cell reachable only through portals wholly behind the caller's near plane stays
  hidden, in the walk and the oracle, even when the eye lies within the bypass distance
  of one of those portals. (O31)
- [ ] A seeded walk from the camera cell alone, under the slid frustum, equals the camera
  entry. (O27)
- [ ] A seeded walk given no seeds, or only unusable ones, returns an empty set without a
  trip. Repeating a seed changes neither the set nor the portal-test count. An
  out-of-range, solid, exterior or wholly off-screen seed is skipped. (O32)
- [ ] A seeded walk forced past its cap reports the trip to its caller, distinct from an
  empty or completed walk, and returns no fallback set of its own. (O34)

Set shape and state:
- [ ] *Guard:* at every camera in the superset sweep, cell ids are in range, the draw set
  holds only drawable cells, and fog reach holds only non-solid cells and always the
  camera cell. On a walk-path frame the draw set is exactly the drawable part of fog
  reach. (O4, O12)
- [ ] Back-to-back walks from one caller each match a walk from fresh state, across a
  door toggle, a camera cell change, a switch to a world with more cells, and a seeded
  walk run between two camera walks, which leaves the first camera result and its
  counters unchanged. When the walk keeps a generation counter, a walk across its wrap
  also matches. Concurrent walks on several threads match too. (O7, O25, O35)

Must still exclude:
- [ ] *Guard:* the far end of an L-shaped corridor, and an unreachable side branch behind
  a straddling portal, stay hidden.
- [ ] *Guard:* a cell inside the frustum, reachable only through a small opening whose
  rect misses its portal, stays hidden.
- [ ] *Guard:* a portal wholly behind the camera is rejected.
- [ ] *Guard:* a blocked portal blocks in both directions; a closed door keeps a closet
  out of both the draw set and fog reach, including with the camera pressed against the
  door within the bypass distance. (O37)
- [ ] A distant portal coplanar with the camera is clipped normally; its cell is hidden
  when the opening lies outside the source rect.

Bounded cost:
- [ ] On the stacked-slab fixture, the exact walk exceeds today's step cap. The new walk
  makes at most k times the summed degree of reached cells in portal tests, and never
  trips. k is a fixed number in the test.
- [ ] On every fixture, no cell expands more than 32 times.
- [ ] A cell whose rect grows twice before it is expanded expands once, against both
  contributions. (O22)
- [ ] On the open lattice, the new walk finishes under the cap on a thread with a small
  fixed stack.
- [ ] Every outbound portal the walk looks at counts once toward the cap and the
  portal-test count, including facing-rejected, blocked and solid-leading ones. (O11)
- [ ] A walk whose portal tests equal the cap completes on the walk path; one that needs
  one more takes the fallback. A camera cell with no portals never trips. (O24)
- [ ] The step cap is at least twice the new walk's worst portal-test count on every
  fixture, the seeded one included.
- [ ] *Guard:* over the superset sweep's cameras and fixtures, shuffling each cell's
  portal list yields an identical visible set and fog reach.
- [ ] *Guard:* a forced cap trip takes the bounded frustum-set fallback, never the
  draw-all set.
- [ ] *Guard:* a walk the cap stops returns only cells the uncapped walk reaches, and
  always the camera cell. (O5)
- [ ] The cap-trip fallback's fog reach contains everything the uncapped walk reaches
  from the same camera: a camera beside a portal looking away from it, and a camera
  within the bypass distance of a portal whose far cell lies wholly behind the eye or
  beside it outside the view's side edge. (O6, O26, O30)

Bake and instruments:
- [ ] For an unchanged fixture map, the id-51 cache key differs from its pre-change value,
  which the test pins as captured on the pre-change build. The bake's sampling
  projections and the cell-residency stage version match their values before; only the
  walk epoch moves the key.
- [ ] *Guard:* the tests that make a drawn cell outside the baked set never-refused
  visible demand pass unchanged.
- [ ] The dry run's report describes the sampled set as a sample of the new walk, not a
  lower bound for any FOV, in every section that described it, the tiles note included,
  and its test asserts that string. The bake's sampling module says the same.
- [ ] In the walk measurement's completed-reads mode, on a small fixture, a drawn block
  deferred behind a retiring generation counts once per frame however many drains the
  frame runs, a second drain in one frame does not bring retirement forward, and a drawn
  block nothing demanded counts as left behind by admission. A run records per frame
  whether its walk tripped, and a paired run splits its frames by that record. (O29,
  O38)

### Manual

Name the machine. "Pre-change" is the last commit before the walk change: it carries
this brief's measurement modes and Settling log, the old walk and epoch, and reads the
base commit's bake. The rect build reads a bake made after the epoch bump. (O14, O19)
- [ ] Sweep: run the real traversal over the sweep poses and headings in `research.md`
  on the hallway, reflection-lake-test and occlusion-test, single-threaded release, plus
  straight up and down at the hallway shaft poses. Zero cap trips and zero unclassified
  misses, and the cap is at least twice the worst portal-test count over the swept maps.
  The id-51 bake reports no step-limit walks on any swept map. Report R1.
- [ ] Residency gate: run the lightmap walk measurement on the hallway random and tour
  walks at default levers, at 100° 16:9, 130° 16:9 and 130° 4:3, at pitch 0 and ±45°, in
  the completed-reads mode, both builds alike, the rect run split by the pre-change run's
  trip record (O17, O18, O29, O38). On the frames the gate keeps (O28), at every
  projection and pitch, the rect build's block-frames deferred behind a retiring
  generation do not exceed the pre-change build's, and the rect build reports zero
  step-limit frames. Report R2; repeat on movement-feel and report R3.
- [ ] SH reach: rerun the SH reach measurement on the hallway and campaign-test with each
  build's bake. Report worst and p95 SH mandatory bytes per camera cell, and any
  device-limit cluster refusals. The worst cell stays under the 256 MiB SH floor.
- [ ] Settle: enter the hallway at its spawn, and at the shaft and arena poses through the
  start-pose override, at pitch 0 and ±45° at the shaft poses, and reflection-lake-test
  at its spawn, each a fresh launch. Per the Settling log, rect-build settle frames take
  the walk path, never the step-limit fallback, and no entry hits the settle timeout or
  logs its warning. Report R4. (O13, O16, O19, O21)
- [ ] Live: at the shaft A stair room, in the arena and at reflection-lake-test's spawn,
  with CPU timing on, the log shows no step-limit marker. Report frame time, walk time
  and collected mesh instances against the pre-change build at the same poses.
- [ ] Visual: walk the shaft spirals, turn at portal boundaries, and stand in doorways
  and on portal planes. No geometry pops out or goes missing. Report any in-view
  dynamic shadow that drops compared with the pre-change build.

## Path

- First slice: the rect walk behind the existing entry point, plus the superset oracle on
  the stacked-slab fixture, testing conservativeness at the near plane, at the bypass and
  under floating-point slop. Build it first; it touches only the visibility crate.
- Commit order is separate (O19): the `portal_vis` split, behavior-preserving; then the
  measurement modes and the Settling log; then the walk with the epoch bump. Rebase the
  walk onto the instrument commits before landing. The pre-change build is the last
  instrument commit, so no instrument commit carries the walk or the epoch bump.
- `cargo test -p postretro-visibility` does not build at 800bbe186:
  `crates/level-loader/src/prl.rs` imports `TeleportersSection` outside its `load-prl`
  gate. Gate it in the first commit unless main already has.
- Seams: `flood`, `portal_traverse_detailed`, `clip_polygon_to_frustum`,
  `narrow_frustum`, `camera_on_polygon_plane` (`APEX_ON_PORTAL_PLANE_EPSILON`),
  `MAX_PORTAL_WALK_STEPS`, `VisibilityPath::PortalStepLimitFallback`,
  `fog_reachable_frustum_fallback`. Walk counters leave through `cpu_stages::record_walk`;
  the title bar reads `VisibilityStats`.
- The bake's step-limit branch in `cell_residency_bake::pvs_sampling` calls the public
  frustum-only `portal_traverse`; point it at the new walk with the view-projection the
  bake already builds.
- Seeded shape: the spike's `portal_traverse_seeded` on `claude/planar-water-spike`
  (`research.md` §Re-grounding at 800bbe186). Its one-flood-per-seed loop is
  order-dependent under one cap (O33). Today's near-plane slide sits in the top-level
  entry; it moves to the camera entry.
- Reference implementation: `prototype/src/alt_vis.rs`, the `rect256` variant. Read it,
  don't merge it. Its measured choices: front-to-back worklist by nearest clipped vertex,
  a cell pushed once while queued, cross a portal only from the camera's side, clip
  camera-plane-crossing portals in 3D before projecting. Its bypass hands the neighbour
  the full screen.
- Rival shape: a per-cell octagon, tighter under pitch for more steps and CPU. Keep the
  region behind one type so the swap stays local.
- Walk scratch (rects, stamps, queue) is reused per caller across frames, and each bake
  thread holds its own (O7, O25, O35).
- The fallback's fog test culls cell bounds per plane against the slid frustum. A
  near-only widening still drops a neighbour beside the eye; push every plane out by the
  bypass distance, or add a distance-to-eye keep test (O26, O30).
- The oracle's off-screen flood comes from `narrow_frustum` dropping the parent's side
  planes at a bypass hop; keeping them is the measured fix. The oracle also gains a
  seeded entry with the walk's seed handling and near-plane order (O31, O32).
- `walk_measurement` fixes FOV, aspect and pitch as constants, retires per drain call,
  sums deferrals per drain over every ready block, and counts paths only in aggregate.
  Its pool has no device limit. The gate needs projection and pitch modes, completed
  reads, frame-counted retirement, per-frame causes per drawn block, and a per-frame trip
  record (O29, O38).
- Settling tracks one combined unsettled peak and logs only its timeout. The per-entry
  log R4 reads is new.
- Test instruments: a per-cell expansion count and a facing-reject counter behind
  `#[cfg(test)]`, and a cell locator for each synthetic fixture.
- `cell_residency_bake::pvs_sampling`, the dry run's `visible_set` and its tiles note call
  the sampled set a lower bound for any FOV, and a dry-run test pins that report string.
  Restate them.
- After the epoch bump, rebake the measured maps. The hallway `.prl` is gitignored, so
  its sweep stays a measurement; fixtures carry the automated proof.
- The closet-reveal door tests and the SH controller doorway test stay exact.
- At landing, §2 drops "decided, not yet built", its description of the chain walk, and
  "lowered" if the cap keeps its value; its fallback paragraph states the widened fog
  test. `build_pipeline.md` §Runtime visibility drops "decided, not yet built".

## Open questions
- Snap grid resolution and the cap value — **delegated**: the cap keeps at least 2×
  headroom over every measured walk; lowering it is allowed, not required (`research.md`
  §Variant results, §Re-grounding at 800bbe186).
- "Near the polygon" distance for the bypass tightening — **delegated**.
- Where the sweep instrument lives and how it reaches the tightened oracle —
  **delegated**.
- The portal-test constant k — **delegated**: the new walk's worst steps per summed
  degree over the fixture sweep, with at most 2× headroom, and never above the per-cell
  expansion bound.
