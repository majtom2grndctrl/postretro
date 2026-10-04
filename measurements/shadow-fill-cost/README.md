# Foreground before/after proof — shadow-fill-cost

Collected 2026-10-04.

**Builds.**
- Before = `94cea2ea7` (main with the GPU shadow cone cull).
- After = `6c7f0a1aa` (CPU shadow world reach).
- Both are plain release builds with symbols kept and no `dev-tools`: `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler`.
- The capture builds add `--features capture`.

## Pins

**Machine.** 16-inch MacBook Pro: Intel Core i9-9980HK (16 logical CPUs), 32 GB RAM, AMD Radeon Pro 5300M (4 GB, Metal) beside an Intel UHD 630. macOS 26.6.2. GPU timestamps are unsupported, so GPU time comes from Instruments.

**Display and settings.** Player settings are `~/Library/Application Support/postretro/settings.toml`, unchanged across all runs:
- window mode `exclusive` at 2688×1680, 60 Hz, render resolution `half`;
- shadow and fog quality `low`, Surface Depth `off`.

Vsync is on (the engine default). SH streaming uses the engine default for timed runs and `POSTRETRO_SH_STREAMING=sync-proof` for captures, as the capture tests require. One engine process at a time; no other workload was started. Other desktop apps stayed open.

**Idle machine state** (engine closed, per batch; `runs/*-clean-idle.ioreg.txt`):

| Batch | In use (5300M) | Free | 5300M utilization | Other accelerator |
|---|---|---|---|---|
| hallway | 0.93 GB | 3.27 GB | 0–7% | 11% |
| campaign | 0.97 GB | 3.23 GB | 0–4% | 7% |
| kinematic spawn | 0.97 GB | 3.06 GB | 0–9% | 9% |
| kinematic station | 0.97 GB | 3.23 GB | 0–36% (one sample) | 9% |

**Fixtures** (prebuilt, gitignored PRLs; SHA-256 prefixes):
- `stress-warren-hallway-inspection.prl` `a543d0e0291dbceb`, baked Sep 29;
- `campaign-test.prl` `574be03376fdb9cf`, baked Sep 29;
- `kinematic-platform.prl` `8939e012bc683c0c`, baked Oct 4.

Their compiler commits were not recorded. This change does not touch the compiler.

**Poses.**
- **Hallway:** on the lift carrying the dynamic point light, `--start-pose=0,3.05,105.664,0,0`. Its six cube faces cold-fill into the dynamic depth cache on every moving frame. The reach walk ran in 93.5, 78 and 79.5 of 120 frames (per-run medians), matching the lift's 1.4 s rest at each end.
- **Campaign-test:** map spawn.
- **Kinematic-platform:** map spawn (`kinematic`), and the mover promotion station (`kinematicmid`, `--start-pose=-6.5,1.22,-27.94,0,0`). Carried dynamic lights walk reach in all 120 frames.

**Protocol (`clean.sh`).**
- A 15 s idle gap before every launch. Builds alternate before/after three times per pose, then one 4 s Metal System Trace per build.
- `caffeinate -d -i -u` is held for the batch, and user activity is re-declared every few seconds.
- Every check records foreground, screen saver, and lock state. A run is valid only if it completes its windows in the foreground, unlocked, with no screen saver. The batch stops at the first invalid run. Every run reported here is valid.
- Upstream limits: `run.py` gives up after 240 s; traces lower the frame rate, so trace frame counts are not timing evidence.

**Metrics.**
- **Timing:** each run's value is the median of its `[CpuTiming]` window averages over windows 3–8 (120 frames each). A build's value is the median across its three runs. Per-window values are in `runs/*.timing.json`.
- **GPU time:** `gpu_time.py` sums `metal-gpu-intervals` durations per labelled pass, deduplicated by command buffer, encoder, and start time. It divides by a frame count: the largest count among once-per-frame passes. Metal coalesces some Textured Pass encoders on the hallway, so that pass alone undercounts frames. Driver rows (`GPU Execution`, `GL/CL`, paging) are excluded.

## Results

| | Hallway before | Hallway after | Campaign before | Campaign after |
|---|---:|---:|---:|---:|
| Frame total (ms) | 32.18 | **26.22** | 17.05 | 16.99 (vsync-bound) |
| `render_submit` (ms) | 5.55 | **3.86** | 3.43 | 3.48 |
| `rec_shadow_depth` (ms) | 0.097 | 0.118 | 0.050 | 0.050 |
| `rec_shadow_reach` (ms, frames it ran) | — | 0.047 | — | 0.025 (7 of 120 frames) |
| Shadow GPU per frame (ms) | 6.57 | **0.77** | 0.21 | 0.16 |
| — former Shadow Cull Pass | 1.82 | — | 0.05 | — |
| — Dynamic Cube World Depth Cache | 4.13 | 0.10 | — | — |
| HAL `draw_indexed_indirect` CPU (ms/frame, `sample`) | 1.80 | 0.01 | 0.04 | 0.01 |

| | Kinematic spawn before | after | Kinematic station before | after |
|---|---:|---:|---:|---:|
| Frame total (ms) | 32.75 | **32.03** | 35.10 | **33.96** |
| `render_submit` (ms) | 8.03 | **7.71** | 5.09 | **4.80** |
| `rec_shadow_reach` (ms) | — | 0.014 | — | 0.014 |
| Shadow GPU per frame (ms) | 1.56 | **0.95** | 1.58 | **1.29** |
| — former Shadow Cull Pass | 0.73 | — | 0.61 | — |

On kinematic-platform, the unchanged entity-occluder passes vary by up to 0.3 ms per frame between the single traces. So per-pass deltas below that are trace noise; the totals and the removed cull pass are not.

### Manual rows

- **M1 baseline:** the before columns above.
  - The brief also asks for the 120-frame dynamic cache log. That log is written only while GPU timing works (`finish_frame(full.frame_timing.is_some())`), and this adapter lacks timestamps, so no run logs it.
  - Substitute evidence that the hallway lift light fills: the traces count 4.4–4.8 Dynamic Cube World Depth Cache passes per frame, and `rec_shadow_reach` runs in 78–94 of 120 frames.
  - These are dynamic-cache cold fills, not uncached live draws. The brief's pose note says "uncached", but a moving dynamic light that owns a cache layer cold-fills that layer instead.
- **M2:**
  - **Hallway: pass.** `render_submit` falls 1.69 ms (5.55 → 3.86) and frame time 6.0 ms (about 31 → 38 fps, GPU-bound).
  - **Kinematic-platform: falls** at both poses (−0.33 and −0.29 ms).
  - **Campaign-test: not met as worded. Owner waiver requested.** `render_submit` reads 0.05 ms (1.4%) higher: 3.367 / 3.426 / 3.432 before against 3.456 / 3.476 / 3.504 after. The runs don't overlap, and after is higher in nearly every paired window.
  - Nothing measured explains it. The `sample` profiles differ by 27 samples in `CommandEncoder::finish` (1,006 → 1,033 of about 7,345 main-thread samples), the same size as the effect, so they neither confirm nor rule out a cause. Warm campaign frames issue the same GPU commands in both builds.
  - A difference between the two binaries' code layout is a guess, not a finding.
  - The reach walk itself costs 0.047 ms (hallway), 0.025 ms (campaign, in the 7 frames it ran) and 0.014 ms (kinematic) per frame.
- **M3: pass on all four poses.** Summed shadow-depth GPU time, including the former shadow-cull pass, falls on every pose.
- **M5: computed, not measured.** About 21.4 MiB of GPU buffers are freed on the hallway: 168,960 B × 132 regions of indirect args, plus 67 KB status scratch, 32 KB all-ones masks and 12.7 KB uniforms, at 8,437 leaves. A buffer-size computation against `94cea2ea7`'s `shadow_cull.rs` confirms it.
- **M6: pass** (next section).

The four `*-sample` profile runs and the captures went through `run.py` and the capture binary directly, outside `clean.sh`. Their run records show foreground and no screen saver. The sample runs predate the lock check.

## Forced-promotion capture (M6)

**Fixture.** `content/dev/maps/spawner-test.map`, baked beside its source as the capture tests do:

```
cargo run --release -p postretro-level-compiler --bin prl-build -- \
  content/dev/maps/spawner-test.map -o content/dev/maps/.spawner-test-sfc.prl --no-tui
```

Two bakes in this session produced identical images. Its `alarm_light` has a `prop_mesh` receiver in reach.

**Scenes.** `capture/w{0,050,100}.scene.json` force the alarm red and pin `force_promotion` w to 0, 0.5 and 1.0, using the camera from `capture_frame.rs`'s receiver golden. They write to the ignored `capture/out/`.

**Command.** Each scene ran as `POSTRETRO_SH_STREAMING=sync-proof <build>/postretro --capture <scene>`, with release `--features capture` builds of each commit.

**Results** (`capture/sha256.txt`):
- **Before vs after:** byte-identical at every weight, and the after build repeats exactly.
- **Negative control:** an after build whose reach returns no ranges, so promoted cold fills draw no world.
  - It matches at w = 0, where the zero weight frees the promoted slot and no fill runs.
  - It differs at w = 0.5 and w = 1.0: 21 of 307,200 pixels, max channel delta 5 and 8, in one region where the receiver samples the promoted world cache.
- So the scene does see the promoted world depth, but only in that small region. Byte identity there is the evidence that reach draws what the GPU cull drew.
- The committed `capture/w*.png` are the after build's images.

## Discarded runs

`runs/suspect/` holds two discarded sets:
- **First runs:** a screen saver ran during some of them, and they can't be told apart afterwards. They showed a bogus steady 23.4 ms "regime" that persisted with vsync off.
- **A first kinematic batch:** the session locked mid-batch, which zeroed some runs' windows. The old foreground flag still passed them, because an empty check list passed vacuously.

The current `run.py` closes both holes.

## Reproduce

From the workspace root:
1. Build both commits as above, and copy `postretro` and `scripts-build` side by side per build.
2. Compile `measurements/release-indirect-validation/runtime/foreground.swift`.
3. Run:

```
BEFORE_BIN=… AFTER_BIN=… RUN_FOREGROUND=… TRACE_DIR=<scratch> \
  measurements/shadow-fill-cost/clean.sh hallway stress-warren-hallway-inspection.prl --start-pose=0,3.05,105.664,0,0
… clean.sh campaign campaign-test.prl
… clean.sh kinematic kinematic-platform.prl
… clean.sh kinematicmid kinematic-platform.prl --start-pose=-6.5,1.22,-27.94,0,0
python3 measurements/shadow-fill-cost/summarize.py hallway campaign kinematic kinematicmid > measurements/shadow-fill-cost/results.json
RUN_SAMPLE=1 RUN_WINDOWS=5 RUN_FOREGROUND=… python3 measurements/shadow-fill-cost/run.py <label> <bin> <map> [pose]
python3 measurements/shadow-fill-cost/profile.py <label> <clean frame ms>
```

Traces are exported and deleted immediately, along with their `instruments*.ktrace` temp files. Compressed GPU exports stay in the ignored `raw/` directory.
