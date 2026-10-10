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
- Rerun: `target\release\portalsweep.exe alt <prl> out_alt` (about 2 min). The
  Cargo.toml path dependencies point at the owner's checkout.

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
| SH targets | `ShResidencyController::update_targets` / `visible_clusters` | More non-evictable Visible clusters; unknown cell id is a hard `InvalidTopology` error |
| SH sampled rows | `SampleRegionIndex::resolve` | More rows composed |
| Particles | `ParticleRenderCollector::collect_sprite` | More sprites |
| Meshes | `mesh_visible_in_cell`; cap in `mesh_instances.rs` | Linear `contains` scan; drops at the instance cap are order-dependent |
| Movers | `mover_visible_against_cell_bounds` | More AABB tests and draws |
| Capture | `collect_capture_receiver_draws` | Report residency numbers shift |
| Diagnostics | `VisibilityStats::walk_reach`, overlays | Counts inflate |
| Offline dry run (test-only) | `lightmap_residency_dry_run::pvs_sampling` | Its "lower bound" claim becomes false |
| Lightmap demand | `BlockDemand::mark_drawn` | Walk-frame drawn cells become never-refused Visible blocks; over-inclusion grows the pool and inflates `drawn_outside_baked_set` |
| id-51 bake | `cell_residency_bake::pvs_sampling::walk_cube_faces` | Over-inclusion enters the baked mandatory set |
| Capture preload | `capture/lightmap.rs` preload | More blocks preloaded for capture |
| Walk measurement | `walk_measurement` controller, `drawn_outside_baked_set` | Counts blocks drawn outside the baked set; the bake-gap instrument |

No consumer reads the set as line of sight or checks it for equality at runtime.

### id-51 bake vs runtime projection

On `feat/lightmap-cell-blocks`, `pvs_sampling::walk_cube_faces` samples a 3×3×3 lattice
of eye points per cell, six faces each, with a square 94° projection, then unions,
dilates one hop (`dilate_one_hop`) and builds the lead map (max lead 32 m). Runtime uses
the player's FOV (≤130°) and window aspect. The exact walk's result is geometric, so
the cube faces cover any runtime frustum. The rect's over-inclusion depends on the
projection: the bounding rect of several openings, and the snap cell's world size,
change with FOV, aspect and view direction. The one-hop dilation hides some of the
difference and guarantees none of it. Owner decision (2026-09-29): measure the gap here;
the lightmap plan widens its bake projections if the gap is material.

## Direction review notes (validate-plan, 2026-09-29)

- Restart-hybrid (exact walk to a budget, discard on a trip, rerun as rect) escapes the
  order-dependence objection, since the trip depends on total steps, not portal order.
  Rejected anyway: a trip costs budget plus rect steps, the contract loosens regardless,
  and the id-51 bake would sample a walk whose meaning changes per pose.
- `done/sh-streaming--reveal-gate-and-warm-horizon` settles on every Visible target, so
  a superset enlarges its settle set.
- At promotion, `rendering_pipeline.md` §2's algorithm description ("the id Tech 4
  approach", "narrows the frustum", "exact portal set") needs a rewrite, not a word swap.
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
| O8 | Locator vs side test | The locator reports A while the camera sits within the bypass distance on B's side of portal A–B. The bypass test runs before the facing test. | A and B both visible, both directions. Beyond the bypass distance, the result is a superset of the oracle's. |
| O9 | Near-plane slide vs clipping | The near plane slides to the eye before any cell's portals are clipped, not only the camera cell's. | Camera within the render near distance of two consecutive portals (a corner): the cell past the second portal is visible. |
| O10 | Bypass boundary | Camera on a portal plane just outside the polygon, inside then beyond the bypass distance; separately, on a plane shared by several coplanar slab portals. | Inside: full screen both directions. Beyond: clipped normally. Shared slab plane: cells above, below and their lateral neighbours visible. |
| O11 | Portal-test counting | Each outbound portal counts once per cell expansion, before any blocked, solid, facing or clip rejection. | Counts match the prototype's "step" and the CPU log's portal-test count; the cap means what it meant during measurement. |
| O12 | Draw and fog from one walk | Both sets come from the walk's final state, after the worklist drains. | The draw set is exactly the drawable part of fog reach. |

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
