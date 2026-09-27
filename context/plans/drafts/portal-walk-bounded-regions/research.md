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
