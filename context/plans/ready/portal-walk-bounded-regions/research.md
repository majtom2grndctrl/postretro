# portal-walk-bounded-regions — research

Evidence gathered during the draft session. Numbers come from `prototype/` (a scratch copy
of the real traversal plus alternatives), run on stress-warren-hallway-inspection baked
2026-09-26 at 1 m SH / 0.04 m lightmap (5,671 cells, ~11.6k portal refs).

## Sweep setup

- 2,082 empty interior cells. Each sampled at cell centre and, where tall enough, at floor
  + 1.6 m: 3,935 positions.
- 8 yaws × pitch 0/+45/−45 per position: 94,440 walks.
- HFOV 100°, 16:9, near 0.1, far 4096. Single-threaded release build; µs = best of 2.
- The prototype's exact walk matched the engine's `determine_visible_cells` on 2,010
  cross-checked walks (0 mismatches).
- "Step" = one outbound portal tested (the engine's `considered`), including
  facing-rejected portals.
- Rerun: `cargo build --release` in `prototype/`, then `portalsweep alt <prl> out_alt`
  (about 2 min on the hallway). The path dependencies are relative to the checkout;
  `analyze_alt.py` summarizes the hallway's named regions, `analyze_alt_any.py` any map.

## Why the exact walk explodes

- `flood` is a port of Doom 3's `FloodViewThroughArea_r`: depth-first, portal-stack cycle
  guard only, no per-cell visited set. Doom 3 gets away with it because mappers place
  visportals by hand (tens of areas).
- Around the shaft spirals (13 floating 112×112×24 u platforms, 48 u apart,
  `emit_shaft_stairs` in `tools/gen_stress_map.py`), every platform plane splits the
  column into 0.6–1.2 m slab cells.
- Vertical cell boundaries at many depths cut the screen into many regions. Surviving
  chains scale with those regions: 1–4k accepted entries for about 80 visible cells.
- Tall bordering cells (degree 25–46) are entered up to 540 times per walk.
- About 80% of steps are clip rejects.
- Tripped poses: all 103 are around shaft A (x −46..−38, y 18.5..33.7, z 17..25 in
  engine coordinates). Shafts B and C reach 1.3–5.7k steps below the cap.
- Uncapped tripped walks: median 24.8k steps, max 37.1k.
- Frame-rate collapse on a trip comes mainly from the fallback, not from the walk
  itself. The fallback draws a median of 345 cells vs 77 truly visible, and empty fog
  reach turned off the fog, shadow and SH gates. `perf/hallway-inspection-followups`
  bounds that fallback.
- Epsilon sliver rejection was tested and is ineffective: 103→95 trips, and it started
  dropping real cells.

## Variant results (full sweep)

| variant | steps p50/95/99/max | µs p50/99/p99.9 | extra cells p50/95/max | ratio p95/max | worst cell re-entries |
|---|---|---|---|---|---|
| exact, uncapped | 41/889/6722/37139 | 8.2/1122/3402 | — | — | 540 |
| rect, Q=256 (chosen) | 32/228/709/1922 | 3.6/76/132 | 0/3/92 | 1.23/10.6 | 17 |
| rect, FIFO order | 36/339/1188/3458 | 3.4/112/– | 0/3/92 | 1.23/10.6 | 19 |
| rect, no facing filter | 32/263/894/3553 | 5.6/174/– | 0/4/98 | 1.28/11.3 | 58 |
| octagon, parent-region eye rule | 32/236/731/2252 | 3.7/99/177 | 0/4/92 | 1.32/27 | 17 |
| octagon, clip-then-project | 33/257/848/2717 | 3.9/108/189 | 0/3/92 | 1.20/10.6 | 28 |
| hybrid, 500 exact steps then rect | 41/675/1231/2528 | 6.9/170/243 | 0/0/60 | 1.00/3.0 | 18 |

- Frustum-all (the old fallback) draws p50/p95/max 151/1212/2066 cells: a median of
  21× the exact set.
- Rect on the 103 trip poses: at most 1,922 steps, about 241 µs, worst 1.28× exact.
- Every variant missed 0 cells the exact walk reaches at yaw 0.
- Shuffling portal lists changed 0 poses for the exact walk and every region variant.
  It changed 927 poses (hybrid at 2k) and 1,341 (hybrid at 500), because where the
  budget cuts off depends on order.
- Q=64 was looser than Q=256 and saved no steps.
- Lowered cap: the rect walk's worst sweep pose took 1,922 steps, so a 5,000-step cap leaves about 2.6× headroom.
- The parent-region eye rule fired on 80% of poses, because large floor portals cross
  the camera plane; hence the octagon's 27× worst case. Clip-then-project avoids it.
- Full per-region tables: `prototype/results/alt_summary.txt`.

## Octagon vs rect by geometry

- Every repo map is almost entirely axis-aligned; the most angled has ≤3% non-axis
  portal edges.
- Rotating the view by +15° and +22.5° (equivalent to rotating walls) left the
  octagon's share of rect's extra cells at 0.84–0.91× at pitch 0, and 0.68–0.75× at
  ±45° pitch.
- The advantage comes from pitch slanting vertical edges, not from wall angle. It saves
  about 0.14 drawn cells per frame, at 1.2× the steps and 1.45× the p99 CPU.

## Exact-walk floating-point leaks

- The yaw-offset sweeps found 5 poses where the exact walk reached a cell neither region
  variant reached, the same poses for both.
- Replaying each chain (`dbg` mode) showed degenerate steps no screen region can mirror:
  - a near-zero-area sliver crossing a portal backward, from the side away from the
    camera;
  - or a final clipped polygon outside the frustum (NDC y −2.5 or +2.2, x −1.0006).
- These are leaks in the exact walk, not visible cells. This is why the superset
  acceptance row classifies oracle-only cells rather than comparing blindly.

## Camera-on-plane bypass

- `camera_on_polygon_plane` tests the signed distance to the polygon's supporting plane
  (1 mm, `APEX_ON_PORTAL_PLANE_EPSILON`), not the polygon.
- It fired on 2,202 poses (2.3%), often for distant portals that share a plane with the
  camera, and skipped clipping there.
- A superset walk must use the full screen in that case. That produced the worst
  looseness: at x = 0.00, 16 cells exact vs 108 rect.

## Algorithm survey

| Candidate | Cost bound | Looseness | Notes |
|---|---|---|---|
| Per-cell grow-only rect (Descent `build_segment_list`, Source `R_FlowThroughArea` union) | visible cells × degree × bounded growths (needs outward snap) | fills the bounding box of L-shaped or two-opening unions | Descent re-queues on growth, breadth-first |
| Per-cell octagon (Thief) | same | tighter on diagonals | Barrett notes two far-apart openings inflate it to the whole cell |
| Coverage bitmask (Umbra 3) | cells × degree × newly set bits | tightest | needs a conservative rasterizer and per-cell memory |
| Hull / k-rects | hull unbounded unless capped | between octagon and bitmask | merge rule must stay a superset |
| Memo by (cell, entry portal) | Σ deg² | tighter across portals | storing only the first frustum drops chains |
| Hybrid exact-then-region | budget + region bound | exact on ordinary maps | order-dependent at the cut |

Sources:
- id Software, `DOOM-3/neo/renderer/RenderWorld_portals.cpp`
- Descent `RENDER.C` (videogamepreservation mirror)
- Sean Barrett, "Thief rendering" (nothings.org)
- Source `r_areaportal.cpp` (leaked 2017 mirror; unofficial)
- Godot 3 `portal_tracer.cpp`
- Umbra 3 traversal article (repost)
- Build engine internals (fabiensanglard.net)
- Luebke & Georges 1995: PDF unreadable; confirmed only via secondary summaries.

## Re-grounding at 0fac87bd3

Source recheck before review; the Problem's mechanism and seams held.

- `flood`'s only guards are the chain-cycle check, `MAX_PORTAL_CHAIN_DEPTH` and the step
  budget. `portal_traverse`'s doc rejects a per-cell visited set on purpose: keying on
  cells drops every chain after the first. The rect's grow-only rule is what makes a
  per-cell state safe.
- `camera_on_polygon_plane` true skips `clip_polygon_to_frustum` only; the walk still
  narrows against the full polygon.
- The bounded step-limit fallback landed in dc0253ef7: draw set from
  `visible_cells_frustum_all`, fog reach from `fog_reachable_frustum_fallback`.
- Walk CPU time landed with `cpu-frame-profiling`: `cpu_stages::record_walk` →
  `VisibilityStage::PortalWalk` (`portal_walk`) plus considered/accepted/reject counts.

### Visible-set consumers

| Consumer | Symbol | Superset effect |
|---|---|---|
| Camera cull | `write_bitmask_from_cells`; `gather_candidate_leaves` | More leaves drawn; out-of-range id falls back to the tree walk |
| Animated lightmap compose | `AnimatedLightmap::dispatch` | More tiles composed; must share the draw set |
| Fog reach | `compute_fog_cell_mask` | More volumes marched; empty = draw-all |
| Shadow eligibility | `light_reaches_visible_cell` | More eligible lights |
| Shadow slot ranking | `assign_shadow_pool_slots_with_promoted_baked`, `candidate_slot_score` | Score ignores visibility: an extra light can take a slot from an in-view one |
| SH targets | `ShResidencyController::update_targets` / `visible_clusters` | More non-evictable Visible clusters, each a settle target (`is_settle_target`); unknown cell id is a hard `InvalidTopology` error |
| SH sampled rows | `SampleRegionIndex::resolve` | More rows composed |
| Particles | `ParticleRenderCollector::collect_sprite` | More sprites |
| Meshes | `mesh_visible_in_cell`; cap in `mesh_instances.rs` | Linear `contains` scan; drops at the instance cap are order-dependent |
| Movers | `mover_visible_against_cell_bounds` | More AABB tests and draws |
| Fluid surfaces | `FluidSurfaceRenderCollector::collect` | More translucent surfaces drawn and sorted |
| Capture | `collect_capture_receiver_draws` | Report residency numbers shift |
| Diagnostics | `VisibilityStats::walk_reach`, overlays | Counts inflate |
| Offline dry run (test-only) | `lightmap_residency_dry_run::pvs_sampling` | Its "lower bound" claim becomes false |
| Lightmap demand | `BlockDemand::mark_drawn` | Walk-frame drawn cells become never-refused Visible blocks; over-inclusion grows the pool and inflates `drawn_outside_baked_set` |
| Settle set | `update_capture_view`, `unsettled_blocks`, `Session::settle_report` | Settling demands every drawn block as Visible; a superset enlarges what reveal waits on, bounded by the Settling timeout |
| id-51 bake | `cell_residency_bake::pvs_sampling::walk_cube_faces` | Over-inclusion enters the baked mandatory set |
| Capture preload | `capture/lightmap.rs` preload | More blocks preloaded for capture |
| Walk measurement | `walk_measurement` controller, `drawn_outside_baked_set` | Counts blocks drawn outside the baked set; the bake-gap instrument |

No consumer reads the set as line of sight or checks it for equality at runtime. Candidate-cull's output identity rests on spans matching the visible cells, and dynamic-light cull's zero-slot guarantee rests on closed portals blocking; a superset keeps both.

### id-51 bake vs runtime projection

On `feat/lightmap-cell-blocks`, `pvs_sampling::walk_cube_faces` samples a 3×3×3 lattice
of eye points per cell, six faces each, with a square 94° projection, then unions,
dilates one hop (`dilate_one_hop`) and builds the lead map (max lead 32 m). Runtime uses
the player's FOV (≤130°) and window aspect. The exact walk's result is geometric, so
the cube faces cover any runtime frustum. The rect's over-inclusion depends on the
projection: the bounding rect of several openings, and the snap cell's world size,
change with FOV, aspect and view direction. The one-hop dilation hides some of the
difference and guarantees none of it. The brief gates the gap rather than widening the bake; see §Bake-gap ruling.

## Direction review notes (validate-plan, 2026-09-29)

- Restart-hybrid (exact walk to a budget, discard on a trip, rerun as rect) escapes the
  order-dependence objection, since the trip depends on total steps, not portal order.
  Rejected anyway: a trip costs budget plus rect steps, the contract loosens regardless,
  and the id-51 bake would sample a walk whose meaning changes per pose.
- `done/sh-streaming--reveal-gate-and-warm-horizon` settles on every Visible target, so
  a superset enlarges its settle set.
- `rendering_pipeline.md` §2 already carries the bounded-region walk (marked decided, not yet built) and the superset contract, since a424aaf. At landing, §2 drops the "decided, not yet built" marker and its description of today's chain walk.
- The "no PVS bake" decision is what makes "the id-51 bake samples the new walk" load
  bearing: a geometric bake would remove the projection gap.

## Ordering pins

From `/review-brief` (rows lens, 2026-09-29). Acceptance rows cite these ids.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| O1 | Portal side for the facing filter | Each portal's source side is settled before any walk, from data every fixture and shipped map carries; the per-frame camera side test runs after, per portal. | Fixtures with a one-leaf locator or shared cell bounds still orient every portal correctly. No existing positive visibility assertion changes. Hand-worked risk: the prototype's locator-probe orientation, with its cell-centre fallback, flips a portal in the existing two-path fixture and drops its far cell. |
| O2 | Re-expansion after descendants | C is reached through a narrow opening and its descendants expand; later, a wider or later-queued path grows C's rect. | C is re-queued and its descendants re-tested against the grown rect; newly exposed cells become visible; C expanded more than once. With the facing filter a cycle cannot drive this, so a diamond of unequal path lengths does. |
| O3 | Growth of exactly one grid step | One contribution grows an edge by one snap step; another equals or sits inside the rect. Growth compares snapped values. | One-step growth re-queues. A no-op contribution never does, which keeps termination. |
| O4 | Zero-iteration walk | Camera cell has no portals, or all face away or lie behind the eye. | Walk path, not a fallback. Fog reach is the camera cell alone; the draw set holds it when drawable. Walk time recorded; portal tests equal the camera cell's degree. |
| O5 | Cap trips mid re-expansion | The cap trips with cells still queued. | The frame uses the fallback sets and discards the partial walk. The walk entry the id-51 bake re-runs on a trip returns only cells the uncapped walk reaches, always including the camera cell. |
| O6 | Fallback vs walk, same camera | Trip frame vs non-trip frame at one pose; the bypass hands a neighbour the full screen unclipped. | Fallback fog reach contains the walk's fog reach, including a camera beside a portal looking away from it. This corner already fails with today's bypass. |
| O7 | State across calls | A door toggles between frames; the camera cell changes; bake threads walk concurrently. | Each walk equals a walk from fresh state. |
| O8 | Locator vs side test | The locator reports A while the camera sits within the bypass distance on B's side of portal A–B. The bypass test runs before the facing test. | A and B both visible, both directions. Beyond the bypass distance the locator reports the cell holding the eye, and the result is a superset of the oracle's. |
| O9 | Near-plane slide vs clipping | The near plane slides to the eye before any cell's portals are clipped, not only the camera cell's. | Camera within the render near distance of two consecutive portals (a corner): the cell past the second portal is visible. |
| O10 | Bypass boundary | Camera on a portal plane just outside the polygon, inside then beyond the bypass distance; separately, on a plane shared by several coplanar slab portals. | Inside: full screen both directions. Beyond: clipped normally. Shared slab plane: cells above, below and their lateral neighbours visible. |
| O11 | Portal-test counting | Each outbound portal counts once per cell expansion, before any blocked, solid, facing or clip rejection. | Counts match the prototype's "step" and the CPU log's portal-test count; the cap means what it meant during measurement. |
| O12 | Draw and fog from one walk | Both sets come from the walk's final state, after the worklist drains. | The draw set is exactly the drawable part of fog reach. |
| O13 | Cap trip on a Settling frame | The presented pose's walk trips the cap. Settling demand marks every drawn block and cluster Visible on any path, and the settle check reads that demand in the same frame. The pose is fixed, so every Settling frame trips again. In Running, the same trip holds lightmap blocks without reading them, while SH still demands every drawn cluster. | The settle set becomes the fallback's frustum draw set for both resources and never shrinks. Reveal waits on it until settled or timed out. A lowered cap trips sooner on content the sweep never measured. |
| O14 | Pre-change vs rect residency runs | The pre-change build reads the base commit's bake. The rect build reads a bake made after the epoch bump; the epoch is hashed only into the id-51 cache key, and the runtime never checks a .prl's walk epoch. The harness preloads the spawn with no drawn blocks, unlike Settling. | Each run names the bake it read; a rect run on a pre-bump bake is invalid. Spawn frames are reported apart from the gate. |
| O15 | Over-included cell flickers | A cell enters the rect set on frame N, leaves on N+1, and returns on N+2. Lightmap demand drops it on N+1, and that frame's drain frees its blocks. SH keeps its clusters through hysteresis. | Lightmap re-reads the blocks on N+2. A re-read whose pair needs growth while a generation retires is deferred and counts a miss. The cost is reported, not bounded. |
| O16 | Reveal frame vs last Settling frame | The last Settling frame settles the held view. The reveal frame projects at the same extents but ticks once, so the pose can move, and the snapped rect set can change with it. | Blocks the reveal frame draws outside the settle set are misses counted on that frame. For a cell the exact walk reaches at the settled pose, the count is zero. Over-included blocks may miss without a visible pop. |
| O17 | Drains per frame in the synchronous mode | Frame N's walk issues reads after admitting earlier completions. The mode waits for those reads, then admits and drains again before the frame's misses are counted. The pool mirror retires a grown-out generation after a fixed number of drains. | Retirement counts frames, not drain calls, so a second drain in one frame does not shorten it. Misses are counted once per frame, after the last drain. The pre-change build runs the same mode. |
| O18 | Growth retiring under the next frame's demand | Frame N installs its whole view and grows the pool; the old generation retires. Frame N+1 turns and its reads complete at once, needing room. | Those pairs defer behind the retiring generation and draw non-resident on N+1. Read latency usually hides this in play, so the mode overstates it. The harness's pool has no device limit, so every deferral there is behind a retiring generation (O29). |
| O19 | Measurement commits vs walk change | The harness's projection, pitch and synchronous-read modes, and any Settling instrument, land before the rect walk and the epoch bump. The portal_vis split lands first and changes no behavior. | The pre-change build is the last commit before the walk change, with every instrument and the old walk and epoch, read on the base commit's bake. The epoch bump lands with the walk change, so no instrument commit invalidates the pre-change bake. |
| O21 | Start pose vs Settling | The start-pose override moves the pawn and camera after the map spawn and before the presented pose is fixed, on the first gameplay install only. Settling then projects the held view from that pose. | The settle set is the start pose's. A second level load in the same session enters at the map spawn, so each Settle entry is a fresh launch. |
| O22 | Growth while queued | C is queued; two contributions grow its rect before it is expanded. | C expands once, against the union; one expansion is counted. |
| O23 | Bypass at a portal off the camera cell | The eye is within the bypass distance of a portal between two cells, neither of them the camera cell; the source cell's rect is smaller than the screen. | The neighbour receives the source cell's rect, never more. Crossing back adds nothing, so neither cell re-expands from the bypass. The cell past the second portal is still visible (O9). |
| O24 | Cap boundary | The cap is checked before each portal test. One walk needs exactly cap tests; another needs none. | Exactly cap tests completes on the walk path, and one more test trips it. A walk that tests no portal never trips, even at cap 0. Today's walk trips on cell entry at cap 0 even with no portals; the new walk does not. |
| O25 | Scratch across worlds and wraps | One caller walks world A, then a larger world B; separately, the scratch's generation counter wraps between two walks. | Each walk equals a walk from fresh state; no cell reads as reached from an earlier walk. |
| O26 | Fallback vs bypassed neighbour behind the eye | The walk takes the bypass for a portal within the bypass distance of the eye, and the neighbour's bounds lie wholly behind the eye by less than that distance. The cap trips on the same pose. | The fallback's fog reach still holds that neighbour, because it keeps every cell whose bounds come within the bypass distance of the eye. The fallback's draw set is unchanged. |
| O27 | Seeded walk | Several seeds, none holding the eye; the caller's eye is the mirrored eye and its near plane is the water plane; seeds arrive in any order. | No near-plane slide and no locator call. Every usable seed is queued with the caller's full screen before any cell expands, and all seeds share one region state and one cap. The result is a superset of the oracle seeded from the same cells. A cap trip is returned to the caller. One seed at the camera cell under the slid frustum equals the camera entry. |
| O28 | Pre-change trip frame in a gate run | The pre-change walk trips on frame N, so N's lightmap demand is the camera cell's baked set: drawn blocks are held, never read or grown for. The rect build walks the same pose, demands every drawn block as Visible, and may grow its pool on N; that generation retires across the following drains. | N leaves the gate, and so does each frame after it until a pool growth either build began on N has retired. Both builds report those frames' deferred and non-resident block-frames apart, with their count per run. The pre-change build's non-resident draws on N count as left behind by admission. |
| O29 | Deferral attribution in the completed-reads mode | Frame N's reads complete, then the frame admits and drains, possibly more than once, before misses are counted. | Misses are counted once per drawn block per frame, after the last drain. A drawn block still non-resident then counts as deferred behind a retiring generation when that drain carried and deferred it; any other drawn non-resident block counts as left behind by admission. Retirement advances once per frame however many drains run. The pre-change build runs the same mode and attribution. |
| O30 | Fallback vs bypassed neighbour beside the eye | The walk takes the bypass for a portal within the bypass distance of the eye. The neighbour's bounds lie outside one side plane of the slid frustum, within that distance of the eye, and the camera looks away. The cap trips on the same pose. | The fallback's fog reach holds the neighbour. A near-only widening drops it, because the fog test culls per plane. The draw set is unchanged. |
| O31 | Bypass vs the near plane | A portal lies within the bypass distance of the eye and wholly behind the walk's near plane. On the camera entry that plane passes through the eye, so the portal is behind or beside the eye. On the seeded entry the near plane is the water plane and the eye is behind it. | Camera entry: the bypass applies and the neighbour is reached (O6, O26). Seeded entry: the near-plane test runs first and the portal is rejected, so no cell behind the water plane is reached through it. The oracle's seeded entry follows the same order. |
| O32 | Seed list shape | The caller passes no seeds, the same cell twice, or a seed that is out of range, solid, exterior, or wholly outside the caller's frustum. | No usable seed: an empty set, no portal tests, no trip, no locator call. A repeated seed changes neither the set nor the portal-test count. Unusable seeds are skipped, never visible and never indexed. Skipping a wholly off-screen seed is superset-safe: no eye ray inside the frustum passes through it. |
| O33 | Seed order under one cap | Seeds arrive in any order; all share one cap. | Every usable seed is queued before any cell expands, and the worklist breaks ties by cell id, not arrival. Seed order changes neither the set, the portal-test count, nor whether the cap trips. The spike's one-flood-per-seed loop, which stops at the first trip, lacks this property. |
| O34 | Seeded cap trip | The seeded walk's portal tests reach the cap with cells still queued. | The trip is returned to the caller with the walk's counters. The walk applies no fallback set of its own, and the camera entry's fallback never runs. Any cell returned is one the uncapped seeded walk reaches. |
| O35 | Two entries on one tick | One caller runs the camera walk, then a seeded walk, on the same scratch, before the camera result is last read; the next frame runs the camera walk again. | The camera result is unchanged by the seeded walk, and each walk equals a walk from fresh state. The seeded walk's counters and time are not added to the camera walk's counters or its step-limit marker. |
| O36 | Eye on a distant portal's plane | The eye lies on a portal's plane beyond the bypass distance of its polygon, so the facing sign is zero within float error; the oracle crosses it, clipped. | The walk's facing test and the superset check's backward-crossing exception use one sign rule with one tolerance. A crossing the walk rejects as away from the camera is classified as backward, so no oracle-only cell reached through it is unclassified. |
| O37 | Closed door at the eye | The eye is within the bypass distance of a blocked portal's polygon: a player pressed against a closed door. | The blocked test runs before the bypass and the facing test, as today. The closet stays out of the draw set and fog reach, and the test counts once toward the cap. |
| O38 | Pre-change trip frames, known to the rect run | The pre-change run finishes first and records, per frame, whether its walk tripped. The rect run walks the same seeded path, eyes, yaws and pitches on its own bake and splits its frames by that record. | Frame i is the same pose in both runs. A pair whose pose sequence or locate-mismatch count differs is invalid and rerun. The gated frames are identical for both builds. |

O20 retired: its SH device-limit dependency landed in `done/sh-streaming--pool-growth-device-limit`.

Premise pins (premise lens):

- The walk path's fog reach filters only non-solid; only the step-limit fallback filters
  exterior. "Non-exterior on fog reach" holds on the walk path because the compiler's
  exterior flood is closed under portal adjacency (`level-compiler` `visibility` exterior
  flood). The rect walk keeps this without a new filter.
- On `feat/lightmap-cell-blocks`, `cell_residency_bake/pvs_sampling.rs` documents its
  sampled set as a lower bound on visibility that holds for any runtime FOV. The rect
  walk breaks both claims; whichever plan lands second restates them.

## Owner decisions (2026-09-29, review round)

- `rendering_pipeline.md` §2's "per-frame portal traversal is cheap at modern cell
  counts" holds only with a per-cell bound; the chain walk is cheap only on hand-placed
  portals.
- Landing order: this brief waits for `spatial-residency--lightmap-cell-blocks`, so the
  bake-gap, pitch and lightmap-cost rows always run here.
- Lightmap cost: runtime cells outside the baked set are rect over-inclusion (truly
  visible cells are inside it: the exact walk is geometric and the cube faces cover any
  FOV, up to eye-point sampling). Their cost is pool growth, and a truly visible block
  that waits on a retiring generation misses visibly. Making them refusable would also
  refuse the bake's genuine sampling misses, the error never-refuse exists to prevent.
- Cost rows assert fixed numbers that fail on the chain walk and catch a rect-walk
  regression: 32 expansions per cell (prototype worst 17), and portal tests at most k ×
  summed degree, with k set from the prototype's worst case with at most 2× headroom.
- Oracle exception: a backward crossing is classified by sign alone. A ray crosses a
  portal plane once, so any backward crossing is a leak regardless of area.

## Re-grounding at 5d197a5e9

- Every Decisions premise holds: no per-cell state in the flood, the infinite-plane camera bypass, the 20,000-step cap, the bounded fallback, and the hashed `PORTAL_WALK_EPOCH`.
- Lightmap cell-blocks is on main. `pvs_sampling` now lives in the production `cell_residency_bake` module, and its lower-bound and any-FOV claims sit there and in `lightmap_residency_dry_run/visible_set.rs`. This brief restates both. References above to `feat/lightmap-cell-blocks` describe the code that landed.
- The consumers new since 0fac87bd3 (lightmap demand, the id-51 bake, capture preload, walk measurement) are now in the consumer table. None of them treats a visible cell as line of sight.
- The bake-gap instrument is `walk_measurement` with `drawn_outside_baked_set`. It counts blocks, not cells. The exact-walk baseline is in `done/spatial-residency--lightmap-cell-blocks/findings.md`: 415 block-frames over 236 frames on a random walk, and 1,056 over 621 frames on a tour.
- `lightmap-oversize-cells-and-faces` is order-independent of this brief. The two share no code seam, and its cache-key pin keeps id 51 valid. Pre- and post-change builds share one main commit, so the lightmap layout matches.

## Re-grounding at 27ca6a6

- Walk premises unchanged: the only `crates/visibility` change since 5d197a5e9 moved tests to `glam::camera` constructors.
- New consumer: E24's fluid surface cull (`FluidSurfaceRenderCollector::collect`, via `collect_fluid_surface_draws`) draws a volume whose AABB meets any visible cell. Safe for a superset.
- The SH reveal gate landed (`done/sh-streaming--reveal-gate-and-warm-horizon`). `streaming::cell_demand` (`DemandFrame`, `PathDemand`) now feeds both resources. Outside the camera cell's id-51 set:
  - Lightmap: `BlockDemand::target` returns `BlockTarget::VISIBLE` for any non-Mandatory drawn block; only Band blocks are refused or evicted.
  - SH: drawn clusters get `TargetClass::Visible`, which pressure never trims; overflow shows as `non_evictable_overshoot_bytes`. SH reach (`sh_streaming::reach`) reads only the id-51 camera set, so over-included clusters count once per episode in `ShMissBuckets::outside_reach`.
  - Both feed the settle set. Nothing treats an outside cell as an error.
- Bake cost: the Cell Residency Set stage took 6.63 s on the hallway and 0.26 s on campaign-test (`done/spatial-residency--lightmap-cell-blocks/plan.md`, rebake of 2026-09-29). Extra projections scale it linearly.
- Exact-walk baseline at 100° 16:9, L 16 m, cap 15 (`done/spatial-residency--lightmap-cell-blocks/findings.md`): hallway random 0 growths, peak 15 layers; tour 1 growth, peak 16; 0 drawn-missing on both. The residency-gate runs re-measure the pre-change build at each projection and pitch, not just reuse these.
- `lightmap-oversize-cells-and-faces` landed; movement-feel's multi-block walk: 73 blocks, peak 6 layers, 0 misses.

## Bake-gap ruling (2026-10-13, review-brief round 2)

- Widening the bake's projections was withdrawn. It enlarges the never-refused mandatory set M(c, L). At cap 15, the lead sweep in `done/spatial-residency--lightmap-cell-blocks/findings.md` shows a larger mandatory set raising growths (random 0→1, tour 1→5) and peak layers (16→20). Drawn-only demand barely moves growth: at L = 0 the tour drew thousands of outside-set block-frames with 0 growths. "Widen until within the line" could not converge.
- The walk measurement's misses race wall-clock disk reads (`walk_measurement` frames sleep to a deadline), and its drawn-not-resident count includes occluded, over-included blocks. Completing reads before the frame draws makes the run deterministic; the gate then counts pool deferrals by cause (see gate semantics below).
- The lead sweep varies L, not the bake's projections, so it is an analogy for widening, not a measurement of it. Its L = 0 tour also drew 5 non-resident and 9 not-resident block-frames in 5 frames.
- The harness looks only at pitch 0; the shafts' vertical over-inclusion needs ±45°.
- The exact walk becomes a test-only oracle the engine harness cannot call. The comparison build is therefore the pre-change build: the last instrument commit, reading the base commit's bake (O14, O19). It keeps the old infinite-plane bypass, so its numbers are looser than the tightened oracle's.

## SH pool device limit (traced 2026-10-13)

Historical: `plans/done/sh-streaming--pool-growth-device-limit` (c63ddd09a) fixed this. SH growth now clamps to the device limit and refuses one cluster when the minimum does not fit, instead of ending the session. The gate semantics below still hold.


- `AtlasShape::grown_for_slots` doubles layers and returns `GpuCapacity` when the doubled count exceeds `max_texture_array_layers`, even when the minimum needed fits. `grow_sparse` doubles the same way, and `validate_storage_buffer_size` errors rather than clamps.
- Only `GpuRetirementPressure` is retried. Any other drain error reaches the frame loop, which sets `exit_result` and exits, in Settling and Running alike.
- `rendering_pipeline.md` §Device limits reads as if SH degrades cleanly at the limit; that guard covers volume setup only.
- The lightmap pool defers at its device limit instead (`placement.rs`, `device_limited`).
- Superset demand moves maps toward the SH limit, so `sh-streaming--pool-growth-device-limit` lands first.
- Gate semantics (owner, round 3): completing reads still leaves non-resident draws from the per-drain install budget, first-draw latency and refused band blocks. Only pool deferrals measure over-inclusion's cost, and the gate is relative because the pre-change build was never measured at 130° or ±45°.
- Projections: horizontal FOV is fixed and vertical follows aspect, so 130° 4:3, not 21:9, is the widest frustum.

## Owner rulings (2026-10-13, after the round-3 premise follow-up)

- Cost-bound citation: Descent alone re-queues on growth (§Algorithm survey); Source's `R_FlowThroughArea` unions a per-area rect without a stated bound, and is cited only for the union.
- The sliver rationale for snapping was dropped: an outward snap of a bound on a grid line is still zero width; the tangent-portal Acceptance row pins the behaviour instead.
- The residency gate charges the whole change, bake and runtime together, because both builds read their own bake and the rect-sampled mandatory set is part of this change's cost. A zero baseline requires zero. Both builds run the same synchronous mode, so O18's overstatement applies to both.
- The mesh instance cap orders by visibility tier (forward-visible, shadow-only, promoted-static), then first seen; neither it nor `candidate_slot_score` has a line-of-sight term.

## Re-grounding at 800bbe186 (assessment 2026-10-25)

Full record: `assessment-2026-10-25.md`. Intel i9-9980HK, Radeon Pro 5300M, macOS, Metal.

- No walk source changed since 27ca6a6. The prototype's exact walk still matches `determine_visible_cells` (2,010 walks, 0 mismatches).
- The cap trips beyond the hallway. Same sweep generator as §Sweep setup, run on each map's local `.prl`:

| map | walks | exact trips at 20k | exact steps p99 / max | rect256 steps max | rect µs p50/p99/max | rect extra cells p95/max |
|---|---|---|---|---|---|---|
| stress-warren-hallway-inspection | 94,440 | 103 | 6,722 / 37,139 | 1,922 | 2.8/65/303 | 3/71 |
| reflection-lake-test | 5,208 | 203 (170 of 1,736 at pitch 0) | 27,793 / 45,064 | 1,533 | 17.9/117/309 | 4/50 |
| occlusion-test | 13,128 | 394 | 35,892 / 75,774 | 1,987 | 31/177/378 | 21/182 |
| stress-env-volumes | 151,920 | 134 | 8,462 / 54,407 | 1,554 | 2.9/74/252 | 3/65 |
| reflection-probes-test | 3,288 | 5 | 5,495 / 105,987 | 666 | 8.2/42/62 | 2/13 |
| movement-feel | 2,736 | 0 | 8,662 / 12,967 | 937 | 4.9/54/128 | 2/28 |
| campaign-test | 8,928 | 0 | 2,165 / 4,028 | 601 | 5.5/32/109 | 2/24 |
| e24-environment-test | 3,840 | 0 | 982 / 1,846 | 369 | 4.5/24/45 | 2/14 |

- Worst rect walk over every map: 1,987 steps (occlusion-test), so a 5,000-step cap leaves 2.5× headroom.
- Live, reflection-lake-test spawn, dev profile with `dev-tools`, `POSTRETRO_CPU_TIMING=1`: every frame took the step-limit fallback at 20,000 portal tests; `portal_walk` 3.56–3.81 ms of 10.2–11.1 ms work CPU; settle hold 1,040 ms. There the cost is the walk itself: frustum-all draws only 1.1× the exact set at p50.
- Rect misses vs the exact walk (campaign-test 24 poses, env-volumes 8, lake 4, occlusion-test 2) were replayed for two poses; both fell in the oracle exception classes. The campaign-test one starts at a bypass hop: the bypass feeds the unclipped polygon to `narrow_frustum`, which keeps the portal, edge and far planes but not the parent's side planes, so the exact walk floods off screen. Hence the oracle Decision.
- Spike seeded walk (`claude/planar-water-spike`, `portal_traverse_seeded`): several seeds, one shared budget, no apex cell, solid, exterior or off-frustum seeds skipped, one `flood` per seed; the mirror replaces the near plane with the water plane and never calls the locator.
- Residency baseline bias: on a step-limit frame, `PathDemand::of` maps the path to `CameraSet`, so lightmap demand holds drawn blocks without reading them. The pre-change build's trip frames therefore add no reads or growths, which is why O28 takes them out of the gate.
- The bake's step-limit branch calls the public `portal_traverse`, whose `Frustum` carries planes only; the rect needs the view-projection.
- The SH device-limit dependency is in `done/` (c63ddd09a).

## Report sets

Manual rows cite these. "Both builds" means pre-change and rect.

| id | run | report |
|---|---|---|
| R1 | Sweep, per map | Portal-test counts; CPU µs p50/p99/p99.9/max; extra cells vs exact p50/p95/max, per map and for the hallway's shafts and arena, against the prototype numbers above. Those were measured against the untightened exact walk with a full-screen bypass, so differences are not by themselves a regression or a win. At the shaft and arena poses, the change in visible SH clusters and shadow-eligible lights. |
| R2 | Residency gate, both builds, per projection and pitch | Non-resident block-frames by cause (left behind by admission, deferred behind a retiring generation); outside-set block-frames; pool growths; peak occupied layers; bytes read and physical reads (O15); the baked mandatory set per camera cell (p50, p95, max blocks); the id-51 stage time. Spawn frames apart. Frames outside the gate apart: their count and each build's deferred and non-resident block-frames (O28). |
| R3 | movement-feel, both builds | R2's fields, plus outside-set blocks per drawn cell outside the baked set. |
| R4 | Settle, per entry and build | Step-limit frames from the Settling log; the settle set's peak in lightmap blocks and SH targets; settle hold time; misses counted on the reveal frame. |

