# occluder-first-partition: live H3 run scripts (Task 9d, owed live runs)

Prepared 2026-10-29 on `claude/occluder-first-partition` at HEAD `e7ab1ec78`. Nothing here has been run on the GPU yet. `../README.md` records the offline H3 work. These scripts run what it says the live H3 run still owes. The offline scripts in `../scripts/` are untouched; they point at a deleted scratchpad.

## What the batch measures

For before, 9l cap26 and 9l cap19, with the sides rotating per pose (before-cap26-cap19, then cap26-cap19-before, and so on, as `batch_cap.py` did):

- **occlusion-test, the 9 `../../frame-cost/poses.txt` poses (`fc:`).** Live frame cost on the base and new bakes: walk time, portals tested, render submit, work CPU, frame total, and GPU frame time per pose.
- **stress-street, the 4 `stress-street.README.md` sweep poses (`rd:`).** Live GPU on the street. Offline 4K traces showed +29% at `rd:promenade` and +68% at `rd:elevated` on both caps.
- **stress-street, 2 walk-factor poses (`wk:`).** These are the costliest 9l street-level walk (`g11.26.1@0`) and rooftop walk (`roof:A0.s0.e@0/-15`) that stay on the portal path on all three sides. After the GPU runs, `walk_inproc.sh` times the same poses in-process (CPU only), and `collect.py` divides live µs per portal test by in-process µs per test. That confirms or replaces the ≈ 1.4× live walk factor.

Each run launches the windowed engine at `--start-pose`. It discards 2 `[CpuTiming]` windows of 120 frames, keeps 8, then attaches a 6 s Metal System Trace. The run then kills the engine and reduces the trace to GPU busy time per frame. The adapter has no timestamp queries, so the trace is the only GPU source (`rendering_pipeline.md` §12). Settings are the player's own: `window_mode = "windowed"`, `render_resolution = "half"`, vsync on. Each run logs its surface, scene and divisor extents. The batch copies `settings.toml` into its directory.

**What it cannot cover:**
- **The hallway, campaign-test and arena.** 9l kept only occlusion-test and stress-street PRLs (`../../9l/remeasure/README.md`).
- **Pool growth's retiring-generation transient.** No run moves the camera.
- **A pinned GPU clock.** A live vsync-bound frame leaves the GPU lightly loaded, so its clock drifts (§12 "Pin the GPU clock"). The GPU column is what a player's frame costs at whatever clock the driver picked. The `MHz` column beside it shows that clock. Compare sides within a pose, not across poses.

## Inputs (frozen)

| What | Path | SHA-1 prefix | Build |
|---|---|---|---|
| Engine | `target/occluder-h3live/bin/postretro` | `9164ab4a6432` | `cargo build -p postretro --features dev-tools`, the dev profile (development_guide §Build and run; the build `frame-cost/run1.sh` and offline H3 used). Byte-identical to offline H3's frozen live engine: every crate change since `70c634122` is in `crates/level-compiler`, which `postretro` links only as a dev-dependency. |
| Sidecar | `target/occluder-h3live/bin/scripts-build` | `bee2f97ea04f` | `cargo build -p postretro-script-compiler --bin scripts-build`, beside the engine as `xtask run` places it |
| In-process walk tool | `target/occluder-h3live/bin/walktime-capped` | `7992dd951ea2` | `../tools/walktime.main.rs` with `walktime-capped.Cargo.toml` (dev profile, `crates/visibility` at HEAD), built outside the workspace; source copy in `target/occluder-h3live/walktime-src/` |

- **PRLs.**
  - before: `target/occluder-9g/before/{occlusion-test,stress-street}/*.prl`, the baselines' bakes (hard links), with `--baked-root target/occluder-baselines/baked`.
  - cap26 and cap19: `target/occluder-9l/{cap26,cap19}/{occlusion-test,stress-street}/*.prl`, with `--baked-root target/occluder-9l/baked`.
  - The three `baked/` trees are identical (`diff -rq`).
- **Poses.** `poses.live` holds `label map start_pose floor`.
  - `--start-pose` is the capsule centre, and the pawn settles to floor + 0.81, so the live eye is at floor + 1.31.
  - The in-process tool times both that eye and the sweep convention's floor + 1.5. A check run reproduced 9l's 4,164 tests for `g11.26.1@0` at cap26 at floor + 1.5.

## Scripts

| Script | Role |
|---|---|
| `batch.sh` | The driver: pilot, full batch, `--check`, resume. Holds one `caffeinate -dimsu` for the whole batch. |
| `run_live.sh` | One live run (engine, CPU windows, attached trace, kill, reduce) |
| `sentinel.sh` | Regime sentinel: before build, occlusion-test `fc:P1`, 1 + 3 windows, no trace |
| `gate.sh` | Thermal gate: wait until temp ≤ `GATE_TEMP_C` and ≥ `MIN_PAUSE_S` have passed, at most `MAX_WAIT_S` |
| `machstate.sh` | One line of machine state: `amdstat.sh`, `pmset -g therm`, `machdep.xcpm.cpu_thermal_level`, screen saver, screen lock |
| `idle_temp.sh` | Idle GPU temperature calibration |
| `walk_inproc.sh` | In-process walk timing at the live poses (CPU only, after the GPU runs) |
| `cpuwin.py`, `gpulive.py`, `collect.py` | Reduce the CPU windows and the trace; write the summary |
| `amdstat.sh` | Copied from `../scripts/`; one AMD `PerformanceStatistics` sample |

## Before starting

- Reboot first (plan §Next 3).
  - Unlock the screen and leave the session unlocked. The lock delay on this Mac is immediate, and a locked session renders no frames.
  - `batch.sh` holds one `caffeinate -dimsu` for the batch's life, and each run adds its own. Earlier runs blanked because the display slept in the gaps between per-run assertions. Wrapping the command in `caffeinate -dimsu` as below adds a third holder, which is harmless.
- Nothing else may run: no build, bake or second engine.
  - The batch refuses to start while `postretro`, `prl-build`, `rustc` or `cargo` runs. `ALLOW_BUSY=1` overrides this, and rust-analyzer's `cargo check` counts as running.
  - Quit apps that hold the discrete GPU. Idle VRAM in use is logged before every run.
- The engine window must stay in front. Don't switch apps during runs. A run that gets no frames in 240 s stops the batch with `NO_FRAMES`.
- Disk: each trace leaves about 1 GB in `$TMPDIR` until the run deletes it. The batch stops below 15 GB free.

Run every command from any directory. All paths are absolute. `L` below is shorthand for this directory, `/Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live`.

### (a) Calibrate the gate (after the reboot, machine idle)

```sh
L=/Users/dhiester/Projects/Personal/postretro/context/plans/done/occluder-first-partition/measurement/h3/live
$L/idle_temp.sh 10
```

The script polls every 5 s for 10 minutes. It prints min / median / max temperature, clock and VRAM, and `suggested GATE_TEMP_C` (idle median + 4 °C), and logs to `target/occluder-h3live/idle-<date>.log`. Use the suggestion as `GATE_TEMP_C`. If the pilot's `gate` lines show waits well past `MIN_PAUSE_S` (over about 120 s per run), raise it by 2 °C.

### (b) Pilot: sentinel + one pose on each side (about 7–10 min)

```sh
$L/batch.sh --check --pilot <GATE_TEMP_C>          # plan and inputs only; prints CHECK_OK
caffeinate -dimsu $L/batch.sh --pilot <GATE_TEMP_C>
```

The pilot pose is `rd:promenade` (`PILOT_POSE=<label>` changes it). It exercises the street PRLs, the attached trace and the guardrail pose, and the sentinels exercise occlusion-test. The pilot passes when:
- every line in `status` has `rc=0` and a `gpu_ms_per_frame`;
- `summary.md` has a full row for each side;
- each meta's `extents:` line matches the others.

### (c) Full batch: 45 runs + 6 sentinels + in-process walk

```sh
$L/batch.sh --check <GATE_TEMP_C>
caffeinate -dimsu $L/batch.sh <GATE_TEMP_C> [MIN_PAUSE_S=30] [MAX_WAIT_S=600]
```

- **Duration.** About 90 minutes at the gate floor: a 30 s pause plus about 80 s per traced run, about 1 minute per sentinel, and about 1–2 minutes of in-process walk timing. Every minute the gate waits on temperature adds to that, at most 10 per run.
- **Options.** Set `BATCH=<name>` to name the directory; the default is `full-<date>-<time>`. Re-running with the same `BATCH` resumes: it keeps the base sentinel, re-probes, and skips runs whose meta says `verdict=OK`. `REPS=2` doubles the pose passes, with the rotation continuing. `SENTINEL_EVERY=<n>` sets the sentinel interval (default 9). `batch.sh`'s header lists every option.

## Output: `target/occluder-h3live/runs/<BATCH>/`

| File | Holds |
|---|---|
| `status` | Every gate verdict, sentinel line and run line (rc, gate flag, frame total, work, walk ms, tests, µs/test, trips, render submit, GPU ms per frame), and the stop or `BATCH_DONE` line |
| `batch.meta` | Start time, HEAD, binary and PRL hashes, model and macOS, display and pmset settings, starting machine state |
| `settings.toml` | The player settings the engine read |
| `plan.txt` | The run order |
| `base_sentinel` | The batch-start sentinel's frame total in ms, the regime reference |
| `gate.log` | Every gate poll, one per second: time, seconds waited, amdstat line |
| `<tag>.meta` | One run: pose, PRL, `pre:` / `post:` machine state, window count, trace start and end, export size, extents, vsync, WARN and ERROR counts, `gate:` line, `verdict=` |
| `<tag>.log` | The engine log (`RUST_LOG=info`, `POSTRETRO_CPU_TIMING=1`) |
| `<tag>.amd` | The per-second AMD poll for the whole run, with `trace_start` / `trace_end` markers |
| `<tag>.cpu.txt` | `cpuwin.py`: kept-window means and maxima per stage, `us_per_test`, step-limit frames |
| `<tag>.gpu.txt` | `gpulive.py`: busy fraction, GPU ms per frame (all channels, and graphics + compute only), the counted cross-check, top passes per frame |
| `trace/<tag>.rec.log` | xctrace's own output. The `.trace` bundle and export are deleted after reduction; `KEEP_XML=1` keeps a gzipped export. |
| `sentinelNN.*` | Sentinel runs, with the same files as a run (no trace) |
| `walk/` | In-process walk pose files and `<side>.<map>.r<1-3>.txt` |
| `summary.md` | `collect.py`: per-run table, and the live walk factor table with its median |

Tags are `<map>.<label with : / @ → _>.<side>.r<rep>`. The status file is the first thing to read. `summary.md` is what goes into `../README.md` and `tables.md` once the runs are judged.

## Reading the flags

- **`GATE_TIMEOUT waited=600 temp=<C>`** (on a run's `status` line and its `gate:` meta line): the GPU did not reach `GATE_TEMP_C` within `MAX_WAIT_S`, and the run went ahead anyway. Read that run as warm. One or two are fine. Many in a row mean the gate is below what the machine reaches between runs: raise it, or check what else heats the GPU. `GATE_NO_TEMP` means `amdstat.sh` returned no temperature for 10 s, and the batch stops.
- **`REGIME_ONSET sentinel=<ms> base=<ms> (> 3x) | pre … | post …`**: a sentinel's frame total exceeded 3× the batch-start sentinel. The batch stops and burns no more runs. Healthy totals are vsync-bound near 16.7 ms; the regime ran about 225 ms offline. The `pre` and `post` sections hold the sentinel's GPU temperature, clock and VRAM, CPU thermal limits and xcpm level, which is the evidence for or against the thermal hypothesis. Sentinels run at batch start, every 9 runs, at the end, and immediately after any run whose frame total crosses the threshold (`confirming with a sentinel`). That catches the regime between periodic sentinels without blaming a costly pose.
- **`REGIME_AT_START`**: the batch-start sentinel ran over `SENTINEL_MAX_BASE_MS` (30). The machine is already in the regime, so go straight to the procedure below.
- **`SCREEN`, `NO_FRAMES`, `SENTINEL_FAILED`, `LOW_DISK`, `BUSY`**: these stop the batch. Fix the cause and re-run with the same `BATCH` to resume. `NO_FRAMES` with slow partial windows is checked against the regime first.

## If the regime starts: cool down, then re-probe

1. Leave the batch stopped. Don't reboot yet.
2. Idle the machine for 15–20 minutes with nothing running, logging the cool-down. `idle_temp.sh` also keeps the display awake:

   ```sh
   $L/idle_temp.sh 20
   ```

3. Re-run the sentinel alone in the same batch directory, then read its line:

   ```sh
   $L/sentinel.sh /Users/dhiester/Projects/Personal/postretro/target/occluder-h3live/runs/<BATCH> reprobe01
   ```

   It prints `reprobe01 total_ms=… rc=… <post machine state>`.
4. Record in `../README.md` §(d) whether it cleared, before any reboot: the frame total, the GPU temperature then against at onset, and idle VRAM in use. Last time, six probes held at 223–226 ms while the GPU cooled from 69 to 56 °C, and idle VRAM in use rose from a 0.80 to a 1.16 GiB median.
   - **Cleared** (near the base sentinel): resume with the same `BATCH`.
   - **Not cleared:** record it, reboot, and start a new `BATCH`, so pre- and post-reboot runs do not mix in one table.
