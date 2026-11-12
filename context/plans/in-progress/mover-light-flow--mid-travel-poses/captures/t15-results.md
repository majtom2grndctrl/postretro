# Task 15: slice proof (geometry knots + mean-and-variance depth)

Branch `feat/mover-light-flow--mid-travel-poses` at `0409c42bf` (clean tree, release `prl-build` and `postretro --features capture` built there, frozen in scratchpad `t15/bin/` with hashes in `t15/bin/tree.txt`). Measured 2026-11-11, owner away, display held by `caffeinate -dimsu`. No engine or compiler code was changed; the one repo edit is the AC 36 test.

## Bakes (`--release`, cold, fresh `--cache-dir`)

| Map | Cold bake | id 57 total | direct | bounce | depth | Closed-pose tiles | Live tiles |
|---|---|---|---|---|---|---|---|
| mover-setpiece | 39.1 s | 11,392,019 B | 2,995,659 | 4,494,524 | 3,901,428 | 9,695 | 1,248 |
| enemy-movers | 13.6 s | 4,345,827 B | 641,927 | 1,989,748 | 1,713,712 | 4,252 | 751 |

Pose counts (Build Summary, even + geometry):
- setpiece wall 57 (15 + 42), room 32 (6 + 26);
- em_lift 11 (3 + 8), em_sweeper 25 (7 + 18), em_crusher_open 21 (6 + 15), em_crusher_wall 32 (9 + 23).

Notes:
- The setpiece bake is 39.1 s against task 13's 42.7 s (same id-57 bytes as task 14's 11.39 MB, so no drift); pre-task-13 it was 24.4 s. Cold bakes are single runs, so ±2 s is noise.
- em_sweeper counts **7** even poses, not the 8 the formula gives (ceil(6.50 m / 1 m) + 1). Its even knot 4/7 = 0.5714 sits 0.01 m from the geometry knot 0.56985 and merged into it (merge tolerance 0.025 m), so the summary counts it once, as a geometry knot. Placement is right; the label is only a counting artifact.
- The M7 variant bakes (four `mover-setpiece` copies) carry an id 57 byte-identical to this one (`m7/logs/sections-head.txt`).

## Part A: motion recapture (GPU, offscreen)

Harness: `t15/tools/tcap.py` (motion `mcap.py`'s five views and metrics, with a per-column engine and PRL), `tanalyze.py`, `tsummary.py`. Offscreen captures at 960×540, `POSTRETRO_SH_STREAMING=sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover.

Columns:
- **amend**: HEAD engine (epoch 6) + HEAD PRLs above. New: 295 frames on the old grids (0.02 room, 1/70 faces), plus every truth knot, plus the amend room's 30 mid knots ±0.001 on the ceiling.
- **head**: pre-amendment engine and PRLs (`motion/bin`, epoch 5), reused from `motion/cap/` on the old grids; 174 new frames at the truth knots off those grids.
- **two**: `light_poses 2`, reused.
- **truth**: `advice/ws/.../xfine-*.prl` (`light_poses` 41 room, 57 sweeper, epoch 5) on the pre-amendment engine, at its own knots only (room k/40, sweeper k/56), where depth is exact under either rule.

Pilot: one t′ per view per new column, frames load and differ. The truth pilot first rendered with placeholder textures, because `advice/ws/` had no `baked/` materials: I linked `advice/ws/baked` → `t15/ws/baked` (same content-hashed `.prm` files) and recaptured. No other capture logged a warning past the known `debug` prefix line. 966 new frames in all.

### Against the truth, at the truth knots (crop mean luma, Rec. 709)

| View | mean \|amend − truth\| | max (t′) | mean \|head − truth\| | max (t′) | per-cell max, amend / head |
|---|---|---|---|---|---|
| ceiling | **0.22** | 2.18 (0.125) | 4.41 | 15.99 (0.150) | 4.98 / 30.97 |
| floor | **0.11** | 0.65 (0.425) | 1.19 | 6.71 (0.550) | 9.82 / 23.23 |
| interior (frame mean) | **0.06** | 0.31 (0.875) | 0.40 | 1.97 (0.550) | 8.97 / 23.40 |
| east face | **0.58** | 3.67 (0.161) | 3.87 | 12.91 (0.089) | 15.18 (0.982) / 17.90 |
| west face | **0.13** | 1.37 (0.232) | 0.86 | 2.74 (0.250) | 2.70 / 7.05 |

amend sits on the truth curve in every view, 6–19× closer than head on the mean (`t15-*-luma.png`, lower panels).

### Per artifact

- **Ceiling knot flash: gone.** Over the amend room's 30 mid knots, the mean drop at the knot against the average of ±0.001 t′ is 0.07 on average, 0.31 at worst (knot 0.116); none goes dark. Before: 5–6 at knots 0.2 / 0.4 (1.38 at 0.400 against 6.41 / 6.67 at ±0.001).
- **Ceiling lattice dots: much reduced, not gone.** 12c peaks between knots (t′ 0.1–0.95): amend median 9 (max 29, bright excess ≤ 10.2) against head 24 (max 36) and truth 1 (max 24) on the truth grid. A faint diamond lattice shows at x6 gain at t′ 0.42–0.44 (`t15-ceiling-sheet-t0.400-x6.png`); head's dots there are dark and strong. Mean residuals against truth are three ~2-luma bumps at t′ 0.125, 0.35 and 0.575, each inside a short interval between geometry knots (0.116–0.139, 0.340–0.363, 0.563–0.587) just after the ceiling passes a probe layer.
- **East-face sawtooth: gone.** Peak-to-trough per pre-amendment period (1/7) over t′ ≤ 4/7: amend 8.2 / 5.6 / 2.1 / 1.1, truth 8.6 / 4.1 / 1.5 / 0.9, head 7.7 / 10.7 / 9.9 / 6.2. amend's first two periods are the truth's own lattice-crossing drops, not a sawtooth. Residual vs truth over t′ ≤ 0.6 spans 4.7 (head 13.1). Left over: amend − truth +2.9 to +3.7 at t′ 0.036 and 0.161–0.196, and −1 to −2 at t′ 0.89–0.98 (head shows the same end residual: it is the long last interval 0.877 → 1, below).
- **West face:** amend within 1.4 of truth everywhere (dips at 0.232 and 0.696, both on knot-dense spans); head's −2.7 dips and the drift after 0.73 are gone.
- **Floor / interior:** amend tracks truth within 0.65 / 0.31 in the mean. head's floor deficit (−6.7 at 0.55) is gone. The floor's probe-layer hand-over steps (t′ 0.33, 0.55, 0.78) remain, as in truth: per-cell step max on the truth grid amend 34.3, truth 34.4.
- **Steps:** on the truth grid, amend's per-cell step max matches truth's in every view (ceiling 9.2 / 9.5, east 14.9 / 18.4, west 11.0 / 11.0, interior 17.1 / 18.8); head's ceiling peaks at 29.3. On the old grid amend's median ceiling step (2.9) is above head's (1.8): truth's own median is 2.8, so amend follows real change that head's blend smeared.

### AC 36 test

`crates/level-compiler/src/pipeline/mover_motion_proof_tests.rs` (662 lines), registered as a child module of `mover_mid_pose_compose_tests.rs` (two added lines) so it reuses that file's `Build` (in-process bake, section round-tripped, piecewise bounce, direct q with across marks), `MoverAbsentDepth::base_moments`, `depth_segment`/`mix_depth_tile`, `sample_tile_moments` and the crate's `display_levels`. Tests, both `#[ignore]`d:
- `the_rising_rooms_ceiling_and_floor_read_the_fine_bake_in_motion`
- `the_sweepers_faces_read_the_fine_bake_in_motion`

Each bakes the map three times in-process (as authored; `light_poses` 41 or 57 on the tested mover; `light_poses` at the even count, 6 or 8) and reads 7×7 receivers half a unit off each surface at every truth knot. Weights: trilinear × smooth backface ((cos+1)/2)^8 + 0.03 × Chebyshev (bias 0.05, floor 0.03, min variance 1e-4) on the live tile composed by the shipped rule. Values: each build's bounce and direct toward the normal, lit by the truth's weights, judged in display levels (bounce through the indirect scale, direct at weight 1). The pre-amendment candidate is the even-pose build read through-base on every interval. Run time ≈ 80 s for both.

Port check: the truth's own far-side shares reproduce `advice-results.md` exactly — ceiling 0.016 (worst 0.038; advice 0.016 / 0.037), floor 0.008, east 0.046, west 0.047.

**Result: both tests FAIL on HEAD** (cargo test … `-- --ignored`, 2 failed, 81.5 s). Bars read per truth knot: mean over knots of |Δshare| ≤ 0.01, max ≤ 0.05; values ≤ 3 levels at worst.

| Surface | shipped share (truth) | mean \|Δ\| | worst \|Δ\| (t′) | values worst (t′; bounce / direct) | pre-amendment mean / worst \|Δ\|, values |
|---|---|---|---|---|---|
| ceiling | 0.017 (0.016) | 0.0014 ✓ | 0.018 ✓ | **5.92** (0.575; 5.92 / 0.00) | 0.103 / 0.361, 15.2 |
| floor | 0.008 (0.008) | 0.0005 ✓ | 0.005 ✓ | **4.28** (0.825; 0.10 / 4.38) | 0.081 / 0.330, 21.7 |
| east face | 0.053 (0.046) | **0.0104** | **0.329** (0.107) | **11.98** (0.982; 0.09 / 16.76) | 0.081 / 0.446, 12.3 |
| west face | 0.050 (0.047) | 0.0048 ✓ | **0.069** (0.232) | **4.33** (0.696; 2.42 / 3.89) | 0.092 / 0.391, 7.5 |

Mutation check: with `depth_segment` forced to raw through-base on every interval (geometry knots kept), the shipped shares fall to pre-amendment levels: ceiling mean |Δ| 0.0014 → 0.103, floor 0.0005 → 0.080, east 0.0104 → 0.088, west 0.0048 → 0.092 (`logs/test-mutation-raw.log`); restored, `git diff` on the file empty. The in-test even-pose candidate is the other mutation (even knots only): it misses every share bar by 7–10×.

Why the bars are not met, as the evidence shows (bars left as the brief states them):
- **Depth passes on the room** and nearly on the faces: the trip means are within 0.003–0.007 of truth on every surface (the advice table's reading). The face misses are single knots: east t′ 0.107 (0.329) and west 0.232 (0.069), each a truth knot inside a short geometry-knot interval where the face is crossing a probe layer. `advice-results.md` warned its knots coincided with its samples, so this between-knot behaviour was untested there.
- **Values miss on every surface**, mostly direct, and not from knots: east's 16.8 direct levels at t′ 0.982 come from the last interval (0.877 → 1, ~0.8 m) where direct q lerps between knots; the pre-amendment build misses identically there (17.3), and the GPU frames agree (east per-cell amend − truth 15.2 at 0.982, crop mean −2.0). The ceiling's 5.9 bounce levels at t′ 0.575 are the third ~2-luma ceiling bump above (GPU per-cell 5.0 at 0.575). The advice harness reported ~0.5 levels because it counted direct only through lights carrying own tiles and dropped base-dependent receivers (4–7%); this test composes every static light's term, as AC 19's test does, and reads the BC6H-free base.
- So, as written, AC 36's value bar asks for knot density that geometry knots do not supply mid-interval, independent of depth. That is for the owner.

## Part B: M7 frame-cost A/B

Harness: the old `m7/` tools copied to `t15/m7/tools/`, with two changes: `batch.zsh` paths point at `t15/m7/`, and its free-disk floor is 15 GB, not 25; `run.py` treats Activity Monitor (open on the desktop, 52–65% CPU spikes) as it treats WindowServer, after it invalidated two pilot runs. Other foreign spikes (PerfPowerServices 68%) were excluded and retried by the batch's rule.

HEAD variants were baked with HEAD's `prl-build` (id 57 identical across the four, and identical to the cold bake above); the 83df variants and binary are the old `ws83/` and `bin/83dfb5317/`. Fresh `start-script.js` from HEAD's `scripts-build` (the scripting SDK changed since 937ba). Pilots: one run per config per build, all valid. Then 3 rounds, interleaved HEAD r1, 83df r1, HEAD r2 … so both builds share conditions. 2560×1440 windowed (surface = scene, divisor 1), async streaming, clock 1226–1229 MHz median every run, GPU 85–88 °C, no thermal throttle (`pmset -g therm` clean), no 150–260 ms regime, every run rendered (≥ 2 timing windows inside the trace). No cargo, bake or capture ran during the batches. Traces deleted after export; no ktrace left.

Per travelling mover minus docked, ms per frame (median of 3 valid rounds; `t15/m7/runs/ab-table.md`):

| build | mover | bounce compose | Pass A | uploads | sum | live-tile writes / 120 frames |
|---|---|---|---|---|---|---|
| HEAD 0409c42bf | room | +1.044 | +0.654 | +0.127 | +1.810 | 2748.5 |
| HEAD 0409c42bf | wall | +1.545 | +1.268 | +0.064 | +2.877 | 960.0 |
| HEAD 0409c42bf | both (per mover) | +0.909 | +0.637 | +0.078 | +1.623 | 2520.0 |
| 83df re-run | room | +0.955 | +0.583 | +0.142 | +1.681 | 2204.0 |
| 83df re-run | wall | +1.422 | +1.253 | +0.073 | +2.748 | 952.0 |
| 83df re-run | both (per mover) | +0.859 | +0.667 | +0.062 | +1.589 | 2040.0 |
| 83df (old batch) | room / wall / both | +1.022 / +1.395 / +0.856 | +0.619 / +1.242 / +0.667 | +0.065 / +0.050 / +0.051 | +1.691 / +2.686 / +1.572 | 2280 / 960 / 2040 |

Pass B is +0.000 everywhere. Per-run compose (bounce + Pass A + B, not minus docked):
- wall: HEAD 2.756 / 2.813 / 2.833, 83df 2.618 / 2.675 / 2.724: **the ranges do not overlap**; HEAD +0.13 ms (≈ +5%), almost all in bounce compose (+0.12).
- room: HEAD 1.683 / 1.698 / 1.760, 83df 1.425 / 1.538 / 1.685: HEAD +0.13 in the medians, ranges touch.
- both: HEAD 2.979 / 3.091 / 3.139, 83df 3.023 / 3.052 / 3.182: no difference.

Read: compose grew past run-to-run noise on the wall (15 → 57 knots), by about 0.12 ms in `mover_bounce.wgsl`; Pass A (`mc_interval`) moved +0.015 there. That is task 17's trigger as the plan states it. It is ~5% of a cost that already misses its 0.2 ms budget by 8–14×. The room (6 → 32 knots) is up a similar amount, within a wider noise band. Live-tile writes per window rose on the room (2204 → 2749) with no measurable upload change (+0.127 vs +0.142).

## Tasks 16 and 17

- **Task 16 (depth-only inside knots): not called for by these numbers.** Setpiece id 57 is 11.39 MB (task 13's 18.31 MB fell to 11.39 with task 14's blend; depth 3.90 MB), enemy-movers 4.35 MB; closed-pose tiles 9,695 / 4,252; cold bakes 39 s and 14 s. Inside knots carry real state at the knots the AC 36 test reads, and the remaining errors are between knots, not in bytes or bake time.
- **Task 17 (per-mover knot bracket): the trigger fired, narrowly.** Wall +0.12–0.13 ms over the pre-brief build with non-overlapping run ranges; room +0.13 in the median within noise; both-movers flat. The brief's M7 row ("no measurable frame cost over the pre-brief build") is not met on the wall as measured.

## Surprises

- AC 36 fails on HEAD; the failing parts are mostly direct values between knots (the long last sweeper interval, and the ceiling's short post-crossing intervals), which the offline evidence did not measure.
- em_sweeper's even-pose count reads 7, not 8: one even knot merged into a geometry knot (placement right, count label only).
- The ceiling still shows a faint lattice at 0.42–0.46 between knots (12c median 9 peaks vs truth 1).

## Files

All under `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/t15/`:
- `charts/{ceiling,floor,interior,east,west}-{luma,steps}.png`; `sheets/ceiling-sheet-t0.400-x6.png`, `sheets/east-sheet-t0.143.png`, `sheets/east-sheet-t0.571.png` (rows amend / head / two / truth).
- `results/{view}.json`, `results/summary.json`; `cap/` (966 frames).
- `logs/bake-amend-*.log`, `logs/test-final.log`, `logs/test-mutation-raw.log`; `m7/runs/{pilot-head,pilot-83df,head,83df}/`, `m7/runs/ab-table.md`.
- Tools: `tools/` (captures and analysis), `m7/tools/` (trace harness).
