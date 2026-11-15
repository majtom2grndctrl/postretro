# Geometry knots and the depth blend: offline evidence

Measured 2026-11-11, offline (Python over baked PRLs, no GPU), by a reviewer agent. The scripts are in `advice-harness/` (`prl57.py` parses id 57, `val57.py` decodes values, and `view*.py` and `val_eval*.py` do the evaluations). The motion captures behind them are in `motion-results.md`.

**Method.**
- Receivers sit on the rising room's ceiling and floor and on em_sweeper's east and west faces.
- At each sample time, the script computes the shader's probe weights: trilinear × smooth backface × Chebyshev, with a bilinear tile tap.
- **Far-side share** is the normalised weight on probes behind the surface.
- **Truth** is a `light_poses 41` bake of the room and a `light_poses 57` bake of the sweeper, sampled at every one of their knots.
- Value error is linear irradiance toward the face normal. About 250 display levels per unit, ±35%.

## Depth: far-side share, mean over the trip (worst in brackets for the ceiling)

| Candidate | Room knots | Ceiling | Floor | Sweeper knots | East | West |
|---|---|---|---|---|---|---|
| Truth | 41 | 0.016 (0.037) | 0.008 | 57 | 0.046 | 0.047 |
| Head (even poses, through-base) | 6 | 0.119 (0.386) | 0.089 | 8 | 0.127 | 0.139 |
| Head, mean-and-variance blend | 6 | 0.126 (0.421) | 0.065 | 8 | 0.099 | 0.125 |
| One knot per enclosure, mean-and-variance | 11 | 0.075 (0.385) | 0.043 | 9 | 0.091 | 0.120 |
| Knots at both enclosure ends, mean-and-variance | 25 | 0.020 (0.068) | 0.009 | 18 | 0.049 | 0.050 |
| Both ends, placed one fine step (4.4 u) loose | 16 | 0.055 (0.371) | 0.026 | 17 | 0.054 | 0.075 |

- **Raw moment lerp.** Mixing two moment sets adds s(1−s)·Δμ² of variance. Chebyshev therefore passes nearly everything once a probe is 0.001–0.003 t′ past a knot.
- **The missing state.** The probe layer just behind a moving surface is enclosed by the thin slab mid-interval, and no even knot stores that state.
- **Placement must be exact.** One fine step of slack brings the spikes back.

## Values: composed error against truth

| Set | Head | Head + enclosure-end knots | Dense (11/15) | Head + knots one step outside each end |
|---|---|---|---|---|
| East face, worst | 0.054 (~13 levels) | 0.057 | 0.031 | 0.002 (~0.5) |
| East face, mean | 0.0131 | 0.0130 | 0.0036 | 0.0008 |
| West face, worst | 0.016 | 0.015 | 0.020 | 0.002 |
| Room ceiling, worst | 0.0215 | 0.0213 | 0.0215 | 0.0006 |
| Room floor, worst | 0.0018 | 0.0007 | 0.0010 | 0.0002 |

- **Inside knots don't help values.** An enclosed mid pose blends values across, so knots inside the enclosure leave the error unchanged.
- **Outside knots fix values.** The lit → shadowed switch then happens only while the probe is inside the slab.
- **Depth needs inside knots and values need outside knots.** With both at each end, depth reads 0.049 east and 0.016 ceiling.
- **Caveats.**
  - The knots coincide with the sample grid, so behaviour between samples is untested.
  - The room's knot set (38) nearly equals the truth bake's (41), so the room result is not an independent test.
  - Lights without own tiles are excluded.
  - 4–7% of sweeper receiver-times depend on the BC6H base, which the scripts cannot decode.
