# Task 15b: geometry-knot constants

Branch at `98c26ec42` plus uncommitted edits. Variant binaries were built one after another from temporary edits to `geometry_knots.rs`, then copied out (`t15b/bin/<variant>/`), so the source stayed mutated only during each build. Captures used `t15/bin/postretro` (0409c42bf engine; the PRL format is unchanged).

## The band and the minimum safe outside distance

- Gather rays start at the probe lifted 1 mm along the ray (`RAY_EPSILON`, `ray_traversal/occlusion.rs`).
- `enclosure_inset` = lift + √3·|p|·ε_f32 ≈ 1.0 mm at these coordinates: a probe counts as enclosed only 1 mm inside the brush.
- So the probe clears the face, with every lifted ray starting outside, once travel passes the interval end by inset + lift ≈ 2 mm. With the 1 mm side guard that gives **3 mm** (`OUTSIDE_KNOT_CLEARANCE_M`, new).
- **Finding:** before this change an outside knot's merge window ran to `a − SIDE_GUARD` (1 mm). At 4.4 u the ±25 mm merge tolerance never reached that far. At 1 u it would, and could park an outside knot inside the band, so the outside window's near edge now stops at the clearance. At 4.4 u this changes no byte: the o44 PRL is byte-identical to task 15's.
- The inside margin was left at 10 mm. No result pointed at it.

## Variants

amend = 4.4 u. Capture columns are against the truth bake at its knots; AC 36 rows are the shipped build at the truth knots. Setpiece bake times marked * overlapped another agent's cargo build.

| Outside distance | room / wall / sweeper poses | id 57 setpiece / enemy-movers | setpiece cold bake | ceiling GPU \|Δ\| mean / max | east GPU \|Δ\| mean | ceiling 12c peaks (median) | AC 36 ceiling / floor / east / west values (levels) | east share worst | west |
|---|---|---|---|---|---|---|---|---|---|
| 4.4 u (0.112 m) | 32 / 57 / 25 | 11.39 / 4.35 MB | 39.1 s (t15, quiet) | 0.22 / 2.18 | 0.58 | 9 | 5.92 / 4.28 / 11.98 / 4.33 | 0.329 | miss |
| 2 u (0.051 m) | 37 / 71 / 29 | 11.84 / 4.52 MB | 41.3 s | 0.17 / 1.46 | 0.58 | 10 | 4.57 / 5.77 / 11.98 / 2.35 | 0.353 | miss (share 0.068) |
| **1 u (0.025 m)** | **45 / 71 / 31** | **12.03 / 4.52 MB** | **47.2 s** | **0.09 / 0.45** | 0.58 | 9 | **4.52 / 5.33 / 11.98 / 1.13** | 0.384 | **pass** |
| 0.005 m (smallest safe) | 49 / 71 / 31 | 12.03 / 4.40 MB | 54.3 s* | 0.08 / 0.23 | 0.60 | 10 | 4.57 / 5.34 / 11.98 / 0.52 | 0.438 | pass |

enemy-movers cold bakes ran 12.2–13.1 s in every variant.

## What the sweep shows

- **Hypothesis confirmed on the ceiling.** The three ~2-luma bumps after each layer crossing shrink with the outside distance: 2.18 → 1.46 → 0.45 → 0.23. The west face passes AC 36 from 1 u down.
- **Not explained by it:**
  - The east share spike at t′ 0.107 worsens as the distance shrinks (0.33 → 0.44). It is a single truth knot, and the GPU frame there differs by −1 luma.
  - The east face's value misses and GPU residual (+3.7 at t′ 0.161) do not move at all.
  - The ceiling lattice dots (12c median 9–10 against the truth's 1) do not move.
  - Floor direct at t′ 0.825 gets slightly worse (4.3 → 5.3).

## Chosen: outside 1 u (0.0254 m), clearance 3 mm

0.005 m buys only 0.2 luma more on the ceiling. In exchange it adds 4 room knots, makes the east spike larger, and leaves 2 mm margin over the band, which a face oblique to the travel would eat into. 1 u keeps 8× the clearance and passes the west face.

Cost against 4.4 u:
- room 32 → 45 poses, wall 57 → 71, sweeper 25 → 31;
- id 57 +5.6% / +4%;
- setpiece cold bake +8 s (+21%).

## AC 36 at 1 u

| Surface | mean \|Δshare\| | worst (t′) | values worst (t′) | Verdict |
|---|---|---|---|---|
| ceiling | 0.0017 | 0.018 | 4.52 (0.650, bounce) | miss on values |
| floor | 0.0002 | 0.002 | 5.33 (0.825, direct) | miss on values |
| east | 0.0102 | 0.384 (0.107) | 11.98 (0.982, direct) | miss on all three |
| west | 0.0037 | 0.016 | 1.13 | **pass** |

## Value misses by interval (1 u)

Every miss lies in an interval with a geometry knot at one end or both. None lies in a purely even interval, because geometry knots populate every span. Columns: t′ of the miss, the interval and its length in even steps, the shipped miss, the pre-amendment miss at the same t′.

- **em_sweeper east (direct-dominated): long intervals, 0.53–0.86 of an even step (0.5–0.8 m).**
  - 0.036 (0.000–0.076): 4.63 vs pre 8.05.
  - 0.161 and 0.179 (0.143–0.230): 4.86 and 3.47 vs 6.05 and 5.87.
  - 0.768–0.839 (0.724–0.845): 3.5–8.4 vs 4.2–9.0.
  - 0.911–0.982 (0.877–1.000): 3.7–12.0 vs 5.0–12.3.
  - These misses predate the brief. Direct q interpolates linearly across 0.5–0.8 m of travel while the slab approaches the crushers. The pre-amendment build misses them by the same or more, and the worst, t′ 0.982 in the last 0.8 m interval, is 11.98 against 12.30.
- **Ceiling (bounce only, near-black surface):**
  - 0.575 (0.573–0.583, short): 3.31 vs pre 7.38.
  - On the knot at 0.600: 3.31 vs 2.88.
  - 0.625 and 0.650 (0.600–0.669, 0.34 step): 4.48 and 4.52 vs 3.73 and 3.56.
  - 0.675 and 0.700 (short): 4.0 and 3.6 vs 2.9 and 2.9.
  - These sit at the redundant-pose skip's scale (2 levels per probe). Even on a stored knot shipped reads 3.31, and pre-amendment reads 2.9–3.7 at the same t′, so the misses are not geometry-knot placement.
- **Floor (direct):**
  - 0.375 (0.21 step): 4.79 vs 9.81.
  - 0.725 (0.22 step): 3.07 vs 11.10.
  - 0.825 (0.806–0.892, 0.43 step): 5.33 vs 4.42.

## Recommendation: restate AC 36's bars (owner)

The value bar fails on misses the pre-amendment build shares: long-interval direct interpolation, and skip-scale bounce on a near-black ceiling. The share bar fails only at single knots. On the mean, east reads 0.0102 for mean |Δ| and about 0.010 for the difference of trip means, the reading `advice-results.md` used.

Proposed wording:

> **Motion proof.** CPU check against a fine bake (`light_poses 41` on the rising room, `57` on em_sweeper) at every one of its knots, on the room's ceiling and floor layers and em_sweeper's east and west faces.
> - Far-side Chebyshev share: the trip's mean within 0.01 of the truth's, and within 0.05 at every knot on the room's layers. On the faces, the worst knot is reported, not gated.
> - Composed values: at every knot, within 3 display levels of the truth, or within 1.5 levels of the pre-amendment build's miss there (even poses, through-base depth).
> - The pre-amendment build fails the share bar. Run under `--ignored`.
> - Value misses shared with the pre-amendment build go to a follow-up on direct interpolation over long intervals and on the skip bar's reach on dark surfaces.

Checked against the 1 u numbers:
- With a +1 allowance, every miss passes except ceiling t′ 0.675: 4.01 against 2.93 + 1 = 3.93, short by 0.08. +1.5 passes every surface.
- The 1.5 is fitted to this data, so the owner should treat it as a choice, not a measurement.
- The trip-mean share reading for east, about 0.010, is borderline. The test prints shares to three decimals, so the reading needs one more digit before it can be gated.
