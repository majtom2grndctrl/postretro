# Reflection-probe compile cost

What the reflection-probe stage (PRL id 54) costs in a cold `prl-build`, split by sub-stage, with
ray counts and capture-ray outcomes. Measured for the `local-edit-warm-builds` brief, to say
whether the probe stage is worth caching finer than its one whole-section entry.

## Result

The probe stage costs 0.13 to 0.46 s per cold build on these maps, at most 2.2 % of the
build (`glossy-reflections-test`: 0.45 s of 21 s). The rest of the build dominates: SH bake
(48 s of 78 to 82 s on `reflection-lake-test`), packing and materials.

Inside the stage, two parts matter:

- **Capture** (trace and shade every probe's 63,504 rays) scales with probe count, at
  about 19 to 52 ms per probe at 6 workers.
- **The colour-mip prefilter** runs only when a glossy world face binds a probe (6 mips).
  It then matches or exceeds capture: 0.15 s against 0.21 s for 10 probes, 0.09 s against
  0.04 s for 2 probes. BC6H encoding is negligible.

Placement, census, exclusion classification and transparent faces together take under 20 ms.
Computing the cache key (`cache_key`) takes 50 to 190 ms, more than placement. The key
hashes the stage's inputs, the SH volume included, and runs even under `--no-cache`.

## Setup

- **Commit measured:** `27c25a13d8b848c075439f31164eedb45651bc51` (`glossy-world-reflections`
  HEAD), in a separate worktree.
- **Machine:** Intel MacBook Pro, Radeon Pro 5300M. CPU: Intel(R) Core(TM) i9-9980HK CPU @
  2.40GHz, 16 logical cores. The owner may have been using the machine, so the runs used
  `nice -n 10`.
- **Build:** `cargo build -p postretro-level-compiler --bin prl-build`, dev profile
  (opt-level 2, debug assertions on). The cold build took 4 min 04 s at `-j 6`.
- **Bake:** `prl-build <map> -o <scratch>/out.prl --baked-root <scratch>/baked --no-cache
  --no-tui -j 6 [--glossy-reflections]`, with a fresh output directory and baked root per
  run.
- **Worker count:** `-j 6` gives the compiler 6 governor permits. The capture's rayon work
  holds a permit per item, so capture, prefilter and re-shade ran 6 wide. Some other stages
  own a pool sized to the core count.
- **Instrumentation:** temporary and not committed. It added `std::time::Instant` laps in
  `reflection_probes/mod.rs` and `probe_capture.rs`, outcome and shadow-ray counters in
  `ShadeScratch` (`shading.rs`), carried out through `RowCapture` (`capture.rs`), and an
  sdf light count (`sdf_lights.rs`). The stage's bytes are unchanged: the colour chain was
  split into its three calls in the same order.
- **Runs:** two timed runs per map. A third run per map, with `PROBECOST_RESHADE=1`,
  recorded every shaded surface for the re-shade experiment. The re-shade run's own capture
  timings match the timed runs.

Rays per probe: a 128² tile with a 1-texel border leaves 126² interior texels. With 4
rotated-grid supersamples per texel (`SUPERSAMPLE_OFFSETS`), that is **63,504 primary rays
per probe**. Every probe traces all of them; there is no adaptive sampling.

## Stage split (seconds, run 1 / run 2)

| Sub-stage | glossy-reflections-test `--glossy-reflections` | reflection-lake-test | glossy-reflections-showcase `--glossy-reflections` | reflection-probes-test (extra) |
|---|---|---|---|---|
| Probes / colour mips | 10 / 6 | 1 / 1 | 2 / 6 | 3 / 1 |
| Glossy census (`glossy_world_faces`) | 0.008 / 0.005 | 0.012 / 0.011 | 0.005 / 0.005 | 0.013 / 0.012 |
| Placement + box fit (`place_probes`) | 0.009 / 0.009 | 0.000 / 0.001 | 0.001 / 0.000 | 0.003 / 0.003 |
| Exclusion classification | 0.000 / 0.000 | 0.000 / 0.000 | 0.000 / 0.000 | 0.001 / 0.001 |
| Transparent faces + `ProbeExclusion` build | 0.000 / 0.000 | 0.000 / 0.000 | 0.000 / 0.000 | 0.000 / 0.000 |
| Cache key | 0.049 / 0.051 | 0.092 / 0.087 | 0.001 / 0.001 | 0.179 / 0.186 |
| Scene setup (samplers, roster check) | 0.001 / 0.001 | 0.014 / 0.012 | 0.000 / 0.000 | 0.003 / 0.003 |
| **Capture: trace + shade (`capture_row`)** | **0.210 / 0.220** | **0.048 / 0.042** | **0.039 / 0.037** | **0.136 / 0.155** |
| **Colour chain (mip 0 BC6H + prefilter + mip BC6H)** | **0.148 / 0.160** | 0.002 / 0.002 | **0.088 / 0.085** | 0.002 / 0.003 |
| Distance tile + light shares | 0.005 / 0.005 | 0.000 / 0.000 | 0.002 / 0.002 | 0.002 / 0.002 |
| **Stage total** | **0.434 / 0.455** | **0.169 / 0.156** | **0.138 / 0.130** | **0.339 / 0.366** |
| Build Summary "Reflection Probes" | 0.44 / 0.46 | 0.18 / 0.16 | 0.14 / 0.13 | 0.35 / 0.37 |
| Build Summary total (cold build) | 21.13 / 20.95 | 82.27 / 77.53 | 13.62 / 15.67 | 171.52* / 14.14 |

\* Run 1 of `reflection-probes-test` spent 157.9 s in DataScript, the first scripts-build
in the fresh worktree. That is a one-off; run 2 is representative.

Colour chain parts, summed across workers (run 1). The wall time above is roughly this sum ÷ 6:

| Map | mip 0 BC6H | prefilter mips 1–5 | mips 1–5 BC6H |
|---|---|---|---|
| glossy-reflections-test (10 probes) | 0.020 | 0.717 | 0.004 |
| glossy-reflections-showcase (2 probes) | 0.004 | 0.170 | 0.001 |

The prefilter costs about 72 to 85 ms of worker time per probe. On a map with fewer probes
than workers (showcase: 2 probes, 6 permits), it parallelizes only per probe, so its wall
time stays near one probe's cost.

## Capture-ray outcomes

| Map | Rays | Miss | Hit | LeakGuard | Fill |
|---|---|---|---|---|---|
| glossy-reflections-test | 635,040 | 306,596 (48.28 %) | 320,689 (50.50 %) | 7,388 (1.16 %) | 367 (0.06 %) |
| reflection-lake-test | 63,504 | 0 (0.00 %) | 63,434 (99.89 %) | 34 (0.05 %) | 36 (0.06 %) |
| glossy-reflections-showcase | 127,008 | 63,426 (49.94 %) | 63,582 (50.06 %) | 0 | 0 |
| reflection-probes-test (extra) | 190,512 | 0 | 184,510 (96.85 %) | 4,333 (2.27 %) | 1,669 (0.88 %) |

Glossy probes miss almost exactly half their rays. 9 of the 10 axis-aligned glossy probes
on `glossy-reflections-test` miss exactly 31,752 = 63,504 / 2. The 45° slope probe misses
20,828. The likely cause is the self-transparent reflector plane: rays into the back
hemisphere pass through the probe's own faces into solid, where there is no front face to
hit. A miss is not shaded, but it still traverses the BVH. Water probes and
`reflection-probes-test` miss nothing. LeakGuard and Fill stay under 3.2 % everywhere.

## SDF shadow rays

`CaptureScene::shade` traces one `segment_clear` per selected sdf light (≤ 4 per hit,
`SDF_SELECT_K`) that is in range and faces the surface (n·l > 0).

| Map | Spec lights | SDF lights | SDF shadow rays per probe |
|---|---|---|---|
| glossy-reflections-test | 15 | 0 | 0 |
| reflection-lake-test | 4 | 0 | 0 |
| glossy-reflections-showcase | 7 | 0 | 0 |
| reflection-probes-test (extra) | 7 | 1 | 0 / 11,637 / 0 (probes 0–2) |

None of the requested maps has an sdf light, so their capture traces no shadow rays.
`reflection-probes-test` was added as the only probe map in `content/dev/maps` with one. Of its
probe 1's 63,504 shaded surfaces, 18,702 select the light and 11,637 trace a shadow ray
(0.18 per primary ray). The other two probes select it nowhere.

## Re-shade without re-trace

The `PROBECOST_RESHADE=1` run recorded each primary ray's `(SurfacePoint, distance)` for every
non-miss outcome during capture. After capture, it timed a shading-only pass over them twice:
`CaptureScene::shade` with the same sample angle, 6 permits wide, rows as work items.

| Map | Shaded surfaces | Re-shade (pass 1 / 2) | Capture trace + shade, same run | Re-shade ÷ capture |
|---|---|---|---|---|
| glossy-reflections-test | 328,444 | 0.126 / 0.127 | 0.208 | 61 % |
| reflection-lake-test | 63,504 | 0.019 / 0.020 | 0.043 | 46 % |
| glossy-reflections-showcase | 63,582 | 0.019 / 0.018 | 0.038 | 49 % |
| reflection-probes-test (extra) | 190,512 | 0.095 / 0.096 | 0.142 | 67 % |

Shading (material sample, lightmap, SH and delta reads, sdf direct) is about half to
two-thirds of capture, and primary tracing is the rest. The re-shade understates shading
slightly. Its materials are already decoded: `SurfaceMaterials` loads each `.prm` lazily on
first touch, during capture. Its shadow rays are re-traced, because they belong to
shading.

## Reading

- The probe stage is small next to the rest of a cold build on every map measured: under
  0.5 s, at most 2.2 % of build time. Its time grows linearly with probe count, at 63,504
  rays per probe, plus 72 to 85 ms of worker time per probe for the glossy prefilter.
- With glossy reflections on, the colour-mip prefilter is the largest single cost after
  capture, and on small probe counts it is the largest. A shading-only edit (a light, a
  material strength) re-runs both shading and prefilter even if tracing were cached.
- Primary tracing is 33 to 54 % of capture. So caching traced hits and re-shading would at
  best save about half of capture, which is tens of ms per probe here. About half of glossy
  rays are back-hemisphere misses.
- Cache key computation costs 50 to 190 ms, comparable to capture on 1 to 3 probe maps.
- These are dev-content maps with 1 to 10 probes and no SDF-lit glossy probe. A large map
  with hundreds of probes would scale these numbers per probe. The absolute stage share
  would grow, but the split between sub-stages should hold.
