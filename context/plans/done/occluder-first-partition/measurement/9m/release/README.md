# occluder-first-partition: Task 9m follow-up, release-build cost of the street CPU rise

Recorded 2026-10-29, 11:19–11:44 PDT, on `claude/occluder-first-partition`. The binary was built from `9e199f213`. HEAD moved to `c4b6c75eb` during the build, but that commit changes only `plan.md`. No source changed, and nothing outside this folder was written except `target/`. Machine: MacBookPro16,1, Radeon Pro 5300M, Metal, screen unlocked. One `caffeinate -dimsu` ran for the whole session. All 12 runs read `verdict=OK`. All four sentinels (start, after the pilot, after 6 runs, and at the end) read 16.52–16.53 ms. The extents were `surface=2560x1440 scene=1280x720 divisor=2`, with vsync on.

## Answer

**In release, cap26 adds +0.35 to +0.52 ms of work CPU per frame at the street poses. `render_submit` accounts for +0.18 to +0.26 ms of that, and `render_prep` for +0.15 to +0.17 ms.** The dev build's +2.3 to +2.8 ms `render_submit` rise does not exist in release. The release submit rise matches the dev build with objc2 debug assertions off (+0.25 / +0.32 ms). Frame totals stay vsync-bound at about 16.6 ms on both sides.

## Build

- `cargo build --release -p postretro --bin postretro`: the release profile (thin LTO, `strip = "symbols"`) with default features, so **no `dev-tools`**. This is the player build `xtask dist` makes.
- `cargo build --release -p postretro-script-compiler --bin scripts-build`.
- Frozen in `target/occluder-9m/bin/`, with hashes in `SHA1SUMS`:
  - `postretro` `edc0e34b8678`
  - `scripts-build` `3f5fe03ae91c`
- `POSTRETRO_CPU_TIMING` works in every build (`rendering_pipeline.md` §12), so no `dev-tools` was needed. The release side therefore carries none of 9m's dev-tools costs (`refresh_camera_cull_diagnostics`, `rec_cull_diagnostics`).
- The log confirms `indirect-call validation: off (build default)`.

## Method

- **Runner.** `9m/tools/run_bin.sh` with `BIN=target/occluder-9m/bin` and `POSTRETRO_UPLOAD_TIMING=1`, the same flags 9m used. Each run discards 2 windows and keeps 6. The before PRLs and baked roots come from `env.sh`, as in 9m.
- **Statistics.** `tools/reduce.py` takes the mean over windows 3–8 only. Traced runs keep logging windows while the trace is attached; those windows are excluded. The status lines' cpuwin values include them, so the status lines differ slightly from `table.txt`.
- **Order.** The order was interleaved, with the side order flipped between the two repeats (see `records/status`). Rep 1 attached a 6 s Metal System Trace after the CPU windows. Rep 2 ran without a trace.
- **Gate.** The pilot ran at 80 °C, and the main batch ran at **74 °C**. A 4-minute idle check right after the release build read 74–78 °C (`target/occluder-h3live/idle-20261029-1114.log`). That reading was still cooling from the build and carried desktop GPU load: utilization about 25% and 1.13 GiB of VRAM in use. During the batch, the gate passed at 59–74 °C, and it waited past 30 s once (51 s).
- **CPU throttle wait.** The pilot's post-trace state showed `cpu_speed_limit=64`, so `drive_rel.sh` also waits until `cpu_speed_limit=100` before each run. Every run had `cpuwait=0`.
- **Sentinel.** The regime sentinel is the frozen dev H3 binary (`sentinel.sh`), as in 9m. Its stop threshold is 49.5 ms.

## Results (ms, mean of 2 repeats; GPU from the single traced repeat)

| Pose / side | total | work | `render_submit` | `render_prep` | `render_record` | portal walk (tests) | GPU/frame | drawn cells / leaves* |
|---|---|---|---|---|---|---|---|---|
| `rd:elevated` before | 16.60 | 6.01 | 3.56 | 0.430 | 0.853 | 0.869 (2,762) | 10.02 | 261 / 1,270 |
| `rd:elevated` cap26 | 16.69 | 6.54 | 3.82 | 0.578 | 0.924 | 0.890 (2,577) | 10.18 | 374 / 3,103 |
| **Δ** | +0.09 | **+0.52** | **+0.26** | +0.15 | +0.07 | +0.02 | +0.17 | |
| `rd:promenade` before | 16.66 | 5.74 | 3.54 | 0.457 | 0.844 | 0.593 (1,867) | 9.72 | 206 / 1,019 |
| `rd:promenade` cap26 | 16.59 | 6.09 | 3.73 | 0.626 | 0.869 | 0.546 (1,953) | 9.55 | 287 / 2,770 |
| **Δ** | −0.07 | **+0.35** | **+0.18** | +0.17 | +0.03 | −0.05 | −0.17 | |
| `fc:P1` before (control) | 16.55 | 5.53 | 3.29 | 0.165 | 1.065 | 0.711 (2,023) | 12.59 | 229 / 799 |
| `fc:P1` cap26 | 16.62 | 4.72 | 3.19 | 0.159 | 0.977 | 0.113 (310) | 12.07 | 40 / 148 |
| **Δ** | +0.07 | −0.80 | −0.10 | −0.01 | −0.09 | −0.60 | −0.52 | |

\* Drawn cells and leaves (faces) come from 9m's offline `world_poses_ray_sweep` at the live eye (`9m/records/sweep.*.txt`). The build has no live counter.

- **`render_prep` sub-stages.** `lightmap_residency`, `lightmap_drain` and the particle stages are flat (≤ 0.003 ms change). The +0.15 to +0.17 ms is in untimed `render_prep` work. 9m attributed the non-dev-tools part of it to SH streaming `prepare_drains`/`update_targets`, whose SH clusters go 77 → 111.
- **Repeat spread.** The per-repeat `render_submit` delta was +0.28 and +0.23 at `rd:elevated`, and +0.11 and +0.26 at `rd:promenade`. The before side's submit varied by 0.04–0.19 ms between repeats.

## Release against dev (Δ before → cap26, ms)

| | `rd:elevated` submit | `rd:elevated` work | `rd:promenade` submit | `rd:promenade` work |
|---|---|---|---|---|
| dev, `dev-tools` (9m HEAD control) | +2.79 | +3.56 | +2.41 | +3.09 |
| dev, `dev-tools`, objc2 debug assertions off | +0.25 | +1.27 | +0.32 | +1.17 |
| **release, no `dev-tools`** | **+0.26** | **+0.52** | **+0.18** | **+0.35** |

- **Base cost.** Release base `render_submit` is 3.5–3.6 ms. The objc2-off dev build's was 4.0–4.1 ms and the dev control's was 6.6–6.8 ms.
- **Where the work gap goes.** The remaining work gap to objc2-off dev (about 0.75 ms) is the dev-tools cull diagnostics, plus dev-profile overhead on the `render_prep` share (dev +0.6 against release +0.16).

## Deviations

- **Gate.** 74 °C, not 67 °C (see Method).
- **CPU throttle wait.** Added, and every run passed it immediately.
- **Pilot reused.** The pilot run is `rd:promenade` before r1, and it counts in the table.
- **GPU per frame is one trace per cell.** The GPU clock was not pinned, so read the GPU deltas as noise-level, not as findings.

## Files

- `records/status`: gate, sentinel and run lines.
- `records/table.txt`: `reduce.py` output, per run and per-pose means.
- `records/*.meta`: per-run machine state and verdicts.
- `records/*.gpu.txt`: trace reductions.
- `tools/drive_rel.sh`, `tools/reduce.py`.
- Engine logs stay in `target/occluder-h3live/runs/9m-release/`.
