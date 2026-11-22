# ui-text-prepare-churn: Mac A/B (M1, M2, M3)

Collected 2026-11-21 (America/Los_Angeles) on the compatibility-floor Mac (AMD Radeon Pro 5300M, Metal). Before = `94f024081` (`0297abebe^1`). After = `0297abebe` (the #645 merge). The first-parent diff is exactly the PR (18 files, renderer + stage-timing + context only). Build commands, binary and PRL digests, and how the maps were produced: `builds.json`.

## Conditions

These follow `research.md` §Measurement conditions to pin, with three differences.
- Same map bytes for both binaries, not the same binary: this is a two-commit A/B.
- Spawn pose, not a start pose.
- No warm shadow cache step beyond discarding the first three windows.

Every run used:
- a plain release build with symbols, no `dev-tools`;
- `POSTRETRO_CPU_TIMING=1`, vsync on;
- surface 2560×1440 and scene 1280×720 (divisor 2), the same on every run;
- the window in front, checked every second;
- eight 120-frame windows, with the median of the final five taken per run.

Each run directory holds three snapshots:
- `ioreg -c IOAccelerator -l` idle snapshot before launch (`runs/<label>.idle.ioreg.txt.gz`, gzipped);
- `pmset -g therm` (`runs/<label>.therm.txt`);
- a per-second GPU temperature, clock and CPU speed limit poll (`runs/<label>.run.json`).

Across all 20 accepted runs (16 M1 + 4 M2):
- CPU_Speed_Limit was 100 before and during every run.
- GPU temperature was 62–78 °C at idle and 62–93 °C during runs.
- Idle in-use VRAM was 1.06–1.19 GB, and idle GPU utilisation was 2–10%.
- The screen stayed unlocked for every run; the harness checks `CGSSessionScreenIsLocked` on every poll.
- No 150–260 ms frame regime appeared. Every M1 window averaged 16.7–18.7 ms except one, the second of the final five in `m1r1-before-campaign`, at 20.3 ms (a hitch, max 42 ms). The per-run median absorbs it.

Runs went in four rounds of 4, alternating which binary went first in each round (B A B A / A B A B …), with a 45 s gap between runs. Nothing else built or ran beside them.

## M1: medians (ms), median of per-run medians, [min–max across 4 runs]

| Map | Row | Before | After | Δ |
|---|---|---:|---:|---:|
| campaign-test | `rec_ui` | 0.576 [0.567–0.578] | 0.072 [0.071–0.074] | −0.504 |
| campaign-test | `render_submit` | 3.762 [3.734–3.781] | 3.011 [2.970–3.047] | −0.751 |
| campaign-test | work CPU | 6.402 [6.351–6.545] | 5.312 [5.224–5.486] | −1.090 |
| campaign-test | frame total | 18.202 [18.078–18.228] | 18.075 [17.782–18.188] | −0.127 (vsync-bound) |
| hallway-inspection | `rec_ui` | 0.456 [0.454–0.457] | 0.061 [0.060–0.062] | −0.395 |
| hallway-inspection | `render_submit` | 2.995 [2.971–3.042] | 2.260 [2.205–2.293] | −0.735 |
| hallway-inspection | work CPU | 6.558 [6.508–6.576] | 5.620 [5.541–5.673] | −0.938 |
| hallway-inspection | frame total | 17.042 [16.971–17.114] | 17.108 [16.983–17.230] | +0.066 (vsync-bound) |

The before and after ranges do not overlap on any CPU row. Frame total is vsync wait plus work. The saving moves into `wait_acquire`, so total does not change at vsync.

`ui_text_spans_prepared` on the after binary was exactly 0 in every window of every run on both maps, all eight windows, max 0. The before binary has no such row.

**M1: pass.** Both medians fall on both maps, and the settled prepare count is 0.

## M2: `sample` profiles (10 s at 1 ms, in level, after two windows)

`render_submit` here is the body of `submit_windowed_frame`, inlined in release. It is the `CommandEncoder::finish` and `UploadQueue::submit` frames directly under `render_frame_indirect`. Counts are inclusive main-thread samples (`profile.py`, `runs/m2-*.profile.json`).

| Map | Binary | `render_submit` samples | `triage_submissions` share | raw wgpu `queue_write_buffer` share | glyphon `queue_write_buffer` samples (anywhere) |
|---|---|---:|---:|---:|---:|
| campaign | after | 1167 | 3.6% (42) | 0% | 0 |
| campaign | before | 1568 | 24.7% (388) | 0% | 141 |
| hallway | after | 1130 | 3.0% (34) | 0% | 0 |
| hallway | before | 1384 | 30.4% (420) | 0% | 140 |

`queue_write_buffer` is the raw `Global::queue_write_buffer`. It never sits under submit: glyphon's writes happened in `rec_ui`, and their staging frees landed in `triage_submissions` at submit. After the change, both after profiles contain no `queue_write_buffer` frame at all. The only glyphon frames left are `TextRenderer::render`, the draw from retained vertices (2 and 1 samples). The engine's own batched `UploadQueue::write_buffer` remains, as expected.

**M2: pass.** Text writes are gone at rest.

## M3: combat residue — needs owner

Damage numbers need a player hitting a dummy or enemy. The engine has no scripted fire or damage hook that a run can drive without input, so M3 was not run. Synthetic OS input into the owner's session was not attempted. To run it on `main` (which contains #645, and whose PRLs load there):

```
POSTRETRO_CPU_TIMING=1 RUST_LOG=info cargo run -p xtask -- run --release -- content/dev/maps/combat-demo.prl
```

1. Shoot the target dummies (east of spawn) steadily for at least two full timing windows, about 5 s at 60 Hz.
2. From the `[CpuTiming]` lines covering that stretch, record `ui_text_spans_prepared=avg/max`. Also record `rec_ui` and `render_submit`.
3. A settled window before and after the firefight should read 0.

If main's `combat-demo.prl` (built Nov 13) fails to load, rebake it first: `cargo run -p postretro-level-compiler --release -- content/dev/maps/combat-demo.map`.

## Invalid and excluded runs

- `runs/invalid/pilot-before-campaign.*`, three failed attempts before the valid pilot:
  1. The screen was locked; the run stalled at "Window ready".
  2. The worktree had no compiled `start-script.js`, and a release build needs one precompiled.
  3. main's PRL would not load at the old commit (`ClusterDirectoryVersionMismatch` v3 vs v2), so both maps were rebaked.
- `pilot-*` runs are pilots: valid, but excluded from the medians. `pilot-before-campaign` sat at "Window ready" for about 4 min until the window was raised. `NSRunningApplication.activate` is refused under cooperative activation, so `run.py` now raises the window through System Events and invalidates any run that loses focus during timing.
- `m2-*` runs carry a concurrent `sample`, so they are excluded from M1. Their timing windows are in the logs.

## Files

- Harness: `run.py`, `batch.sh`, `reduce.py`, `profile.py`, `foreground.swift`, `lockstate.swift`.
- Per run: `runs/<label>.{log,run.json,reduced.json,idle.ioreg.txt.gz,therm.txt}`.
- M1 aggregate: `runs/m1-summary.json`.
- M2: `runs/m2-*.sample.txt` and `runs/m2-*.profile.json`.
- Bake logs: `runs/bake-*.log`.
