# Foreground before/after proof — shadow-fill-cost

Collected 2026-10-04 on AMD Radeon Pro 5300M / Metal (macOS 26.6.2). Before = `94cea2ea7` (main with the GPU shadow cone cull), after = `6c7f0a1aa` (CPU shadow world reach). Both are plain release builds with symbols kept, no `dev-tools`. Settings were unchanged across runs: exclusive window mode, render resolution `half`, shadow and fog quality `low`, Surface Depth `off`.

**Machine state.** Engine closed: in-use VRAM ≈ 0.93 GB, free VRAM ≈ 3.27 GB, utilization 0–7% (`runs/*-clean-idle.ioreg.txt`). Other desktop apps stayed open.

**Protocol.** Every launch is preceded by a 15 s idle gap. Builds alternate before/after three times per map. `caffeinate -d -i -u` is held for the whole batch. Each run records foreground state and whether a screen-saver process appeared. All 16 clean runs stayed foreground with no screen saver.

A timing value is the median over windows 3+ of each run's `[CpuTiming]` window averages (120 frames each), then the median across the three runs. Vsync is on. GPU time comes from separate 4 s Metal System Trace captures. `gpu_time.py` sums `metal-gpu-intervals` durations per pass label, deduplicated by command buffer and encoder, then divides by Textured Pass count.

**Poses.**
- Hallway: on the lift carrying the dynamic point light (`--start-pose=0,3.05,105.664,0,0`). Its six cube faces cold-fill on every moving frame; the reach walk ran in about 92 of 120 frames.
- Campaign-test: map spawn.

## Results

| | Hallway before | Hallway after | Campaign before | Campaign after |
|---|---:|---:|---:|---:|
| Frame total (ms) | 32.18 | **26.22** | 17.05 | 16.99 (vsync-bound) |
| `render_submit` (ms) | 5.55 | **3.86** | 3.43 | 3.48 |
| `rec_shadow_depth` (ms) | 0.097 | 0.118 | 0.050 | 0.050 |
| `rec_shadow_reach` (ms, frames it ran) | — | 0.047 | — | 0.025 |
| Shadow GPU per frame (ms) | 7.60 | **1.09** | 0.21 | 0.16 |
| — former Shadow Cull Pass | 2.11 | — | 0.05 | — |
| — Dynamic Cube World Depth Cache | 4.78 | 0.15 | — | — |
| HAL `draw_indexed_indirect` CPU (ms/frame, `sample`) | 1.80 | 0.01 | 0.04 | 0.01 |

- **M1 baseline:** recorded above (before columns).
- **M2:**
  - On the hallway, `render_submit` falls by 1.69 ms and frame time by 6.0 ms. That is about 31 → 38 fps, GPU-bound.
  - The reach walk itself costs 0.047 ms per frame on the hallway and 0.025 ms on campaign-test.
  - On campaign-test, `render_submit` reads 0.05 ms (1.4%) higher, and the three runs per build don't overlap. The `sample` profiles attribute no cause: `CommandEncoder::finish` differs by 26 samples out of about 1,000, within sampling noise. Warm campaign frames issue the same GPU commands in both builds. Treat it as a difference between binaries, not a shadow cost; owner's call.
- **M3:** summed shadow-depth GPU time, including the former shadow-cull pass, falls on both maps.
- **M5:** about 21.4 MiB of GPU buffers freed on the hallway. This is computed from the deleted allocations (168,960 B × 132 regions plus scratch), not measured.

## Discarded runs

The first measurements are in `runs/suspect/`. A screen saver ran during some of them, and they can't be told apart afterwards. They showed a bogus steady 23.4 ms "regime" that stayed with vsync off. The screen-saver check and `caffeinate` exist because of this.

## Reproduce

From the workspace root, with release binaries for each commit (`CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler`; copy `postretro` and `scripts-build` side by side) and the foreground helper compiled from `measurements/release-indirect-validation/runtime/foreground.swift`:

```
BEFORE_BIN=… AFTER_BIN=… RUN_FOREGROUND=… TRACE_DIR=<scratch> \
  measurements/shadow-fill-cost/clean.sh hallway stress-warren-hallway-inspection.prl --start-pose=0,3.05,105.664,0,0
… clean.sh campaign campaign-test.prl
python3 measurements/shadow-fill-cost/summarize.py hallway campaign > measurements/shadow-fill-cost/results.json
RUN_SAMPLE=1 RUN_WINDOWS=5 RUN_FOREGROUND=… python3 measurements/shadow-fill-cost/run.py <label> <bin> <map> [pose]
python3 measurements/shadow-fill-cost/profile.py <label> <clean frame ms>
```

Traces are exported and deleted immediately, along with their `instruments*.ktrace` temp files. Compressed GPU exports stay in the ignored `raw/` directory.
