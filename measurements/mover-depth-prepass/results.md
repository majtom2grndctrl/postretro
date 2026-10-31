# Mover depth pre-pass: Mac A/B results

**Headline** (Radeon Pro 5300M, Metal System Trace, 3840×2160 headless capture, 2 interleaved rounds × 240 frames, after − before, per-round range):
- **Wall pose** (closed wall ≈37% of frame): Forward **−11.62 ms** (42.77 → 31.15, −27%) [−12.04..−11.20]; Kinematic Brush **−0.69 ms** (19.62 → 18.93) [−0.72..−0.66]; Depth Pre-Pass **+0.24 ms** (0.48 → 0.72) [+0.11..+0.36].
- Depth + Forward + Kinematic sum **−12.07 ms** (62.87 → 50.80, −19%) [−12.58..−11.56]; whole GPU frame −11.69 ms (66.64 → 54.94, −17.5%). North face: sum −5.88 ms (−12%).
- **Null pose** (zero mover draws): sum −0.06 ms [−0.12..−0.01], frame −0.20 ms [−0.25..−0.15], at the noise floor. Movers recorded but off-screen: sum −0.15 ms, frame −0.11 ms, unchanged.
- **Pixel parity:** bit-identical before/after at all four poses (4K, 0 differing pixels).
- **GPU tests on Metal:** `postretro-renderer` 1167 passed / 0 failed under `POSTRETRO_REQUIRE_GPU=1`. All 23 `planar_gpu_*` tests (incl. the new main-view mover test), gate and kinematic_brush tests executed, none skipped. `postretro` capture/mover tests: 225 passed / 0 failed.
- Caveat: the Forward/Kinematic/Depth split is the trace's boundary attribution between back-to-back render passes; the sum and frame figures are label-independent (see below).

Kit, poses and reproduction: `README.md`. Machine-readable: `results.json`. Per-run tables:
`results-batch.md` (the batch), `results-pilot.md` (the pilot). Per-run records, capture
reports, logs and clock samples: `runs/`. Parity: `parity.json`; framing previews (after
binary, 640 px): `previews/`.

## Setup

- Before `5971fb2b6` (parent of `0e93759a8`); after `16c7659ff` (this branch: `0e93759a8` +
  `eb7e97412` + a merge of `origin/main`'s netcode/lifecycle work, no renderer code). Both
  `--release --features capture,dev-tools`, `CARGO_INCREMENTAL=0`.
- MacBookPro16,1, i9-9980HK, Radeon Pro 5300M 4 GB (Metal, no timestamp queries), macOS 26.6.2,
  xctrace 16.0, AC power, display held awake by caffeinate. No build, test or second engine
  during any traced run (the batch checks for cargo/rustc/prl-build/postretro every 2 s).
- Headless `--capture`: no surface; scene extent = 3840×2160, render divisor 1 (§7.8). FOV 100.
  `POSTRETRO_SH_STREAMING=sync-proof`, `RUST_LOG=info`. 120 warmup + 240 sampled frames per
  run; 15 s idle gap before every launch; arm order forward, then reversed.
- Idle before the batch (ioreg IOAccelerator): 5300M in-use VRAM 925 MB, free 3.02 GB,
  Device Utilization 0–11% across accelerators.
- Busy GPU core clock medians 1166–1216 MHz in every run (saturated); temperature medians
  74–82 °C (range 66–83 °C).
- Map: `mover-setpiece.prl` compiled once with this branch's prl-build (the compiler is
  unchanged between the two commits: `git diff 5971fb2b6 HEAD -- crates/level-compiler` is
  empty). Warm dev bake; no SDF atlas, so the SDF Shadow Pass never runs on this map.

## Per arm (batch, mean of 2 valid runs, ms per frame)

| arm | Depth | Forward | Kinematic | Sum | GPU frame |
|---|---|---|---|---|---|
| wall before | 0.482 | 42.768 | 19.618 | 62.868 | 66.636 |
| wall after | 0.720 | 31.148 | 18.932 | 50.801 | 54.944 |
| north before | 0.157 | 39.767 | 9.197 | 49.121 | 51.461 |
| north after | 0.172 | 34.117 | 8.956 | 43.245 | 45.534 |
| offscreen before | 0.136 | 1.904* | 36.456* | 38.497 | 40.851 |
| offscreen after | 0.133 | 2.557* | 35.659* | 38.349 | 40.738 |
| null before | 0.128 | 28.701 | — | 28.829 | 31.228 |
| null after | 0.128 | 28.640 | — | 28.768 | 31.032 |

\* Boundary attribution, not pass cost: at `offscreen` the movers are GPU-clipped, and the
Kinematic Brush encoder reports the drain of the forward pass's work (see Attribution).

## Deltas, after − before (batch mean; [min..max] of the two per-round paired deltas; pilot)

| pose | Δ Depth | Δ Forward | Δ Kinematic | Δ Sum | Δ GPU frame | pilot Δ Sum / Δ frame |
|---|---|---|---|---|---|---|
| wall | +0.238 [+0.112..+0.364] | −11.620 [−12.036..−11.203] | −0.686 [−0.717..−0.655] | −12.067 [−12.579..−11.555] | −11.693 [−12.427..−10.958] | −12.651 / −12.636 |
| north | +0.015 [+0.013..+0.016] | −5.650 [−5.651..−5.648] | −0.242 [−0.250..−0.233] | −5.876 [−5.885..−5.868] | −5.928 [−5.939..−5.916] | −5.868 / −5.822 |
| offscreen | −0.003 | (+0.653)* | (−0.797)* | −0.147 [−0.272..−0.022] | −0.113 [−0.302..+0.076] | +0.014 / −0.015 |
| null | +0.000 | −0.061 [−0.113..−0.009] | — | −0.061 [−0.115..−0.007] | −0.197 [−0.247..−0.146] | +0.001 / +0.059 |

Reading:
- **Wall and north** behave as the change predicts. World hidden behind the mover is no
  longer shaded: Forward falls by 27% at the wall, which covers about 37% of the frame, and by
  14% at the north face. The mover pass is a little cheaper (−3.5% / −2.6%), and Depth
  Pre-Pass grows by the mover's depth. Run-to-run spread in the wall arms is larger
  (wall-before r1 vs r2 sums 63.43 vs 62.30 ms; the wall frames' p10–p90 spans about 7 ms
  inside every run), but both rounds and the pilot agree on a 11–12.7 ms saving.
- **Null:** the two binaries record the same passes there (no Kinematic Brush Pass, mover
  depth not drawn). Sum −0.06 ms and frame −0.20 ms over two rounds (pilot +0.00 / +0.06)
  is the A/B noise floor on this machine, about 0.6% of the frame.
- **Offscreen** (station 1 facing south-west; movers admitted by visible-cell bounds and
  recorded off-screen in both builds): sum and frame are unchanged within that noise.
  Off-screen mover depth costs nothing measurable.
- **Depth Pre-Pass at the wall** (+0.24 ms, rounds +0.11/+0.36, pilot +0.13) sits next to
  the forward pass in the same boundary attribution, so its exact size is soft. The sum
  already includes it.

## Attribution (why Sum and GPU frame are the primary figures)

Per encoder, the trace reports Depth Pre-Pass, Textured Pass and Kinematic Brush Pass as
back-to-back intervals on the one GFX channel, each starting where the previous one ends.
One raw frame at `offscreen` (before binary) read Depth 0.128 ms, Textured 0.159 ms and
Kinematic Brush 38.417 ms, though that pose's movers are entirely off-screen. One at `wall`
read 1.075 / 43.364 / 19.824 ms. An encoder with little work of its own absorbs the previous
encoder's drain, so the split is reliable only where each pass has real fragment work. The
sum of the three passes and the whole-frame union do not depend on labels. 3839 of the
batch's 3840 steady frames were complete; the one incomplete frame (north-before-r2)
read 51.11 ms against that run's 51.45 ms mean. Frames are counted by Candidate
Cull Pass encoders (365 per run = 360 + the capture's pre-warmup and output frames), never
by render-pass encoder counts.

## Rejected runs

None. Pilot 8/8 valid, batch 16/16 valid. One batch run (north-before-r2) had 239 of 240
complete frames, one frame missing its Kinematic Brush row, above the 90% bar. Rejection
criteria are in `README.md`.

## Parity

Untraced `--capture`, 4 + 4 frames, 3840×2160, both binaries at each pose. The decompressed
PNG streams are byte-identical at `wall`, `north`, `offscreen` and `null`: 0 differing
pixels, max channel delta 0, no diff images. Movers render as before at their rest pose.
`rendering_pipeline.md` §7.3 documents a coplanar mover/world tie that the mover now wins;
it does not occur at these poses (no exactly coplanar mover and world faces in view).

## GPU-adapter tests (Metal, this Mac)

- `POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro-renderer --features dev-tools`: **1167
  passed, 0 failed**, 10 ignored. With `POSTRETRO_REQUIRE_GPU=1`, a missing adapter would panic
  rather than skip, and no `GPU test skipped` / `[FluidSurfaceProof] skipped` line appears.
- Targeted re-run with `--nocapture` (`planar_reflection`, `kinematic_brush` filters): 62
  passed, 0 failed. Executed rather than skipped: all 23 `planar_gpu_*` tests, including
  `frame_content_tests::planar_gpu_main_view_mover_shades_over_the_wall_behind_it` (new)
  and `planar_gpu_mirror_only_mover_and_skinned_mesh_reflect`, which prints its rendered
  `[PlanarProof]` pixel; `gate_tests` 3/3, including
  `planar_recording_uses_only_the_frame_encoder`; and `kinematic_brush` 19/19, including
  `mover_depth_and_colour_entries_share_one_invariant_clip_path`. The renderer's offscreen
  adapter is the engine's default selection. The test output does not name the device, but
  capture runs of the same path report the AMD Radeon Pro 5300M.
- `POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro --features capture,dev-tools -- capture
  runtime_movers kinematic mover`: **225 passed, 0 failed**.
