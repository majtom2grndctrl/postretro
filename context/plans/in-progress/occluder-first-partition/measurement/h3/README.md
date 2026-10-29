# occluder-first-partition: Task 9d (H3) frame cost

Recorded 2026-10-28, 15:20–18:56 PDT, on `claude/occluder-first-partition` at HEAD `70c634122`. The tree was clean, and no source changed.

**The live windowed runs did not happen.** The session's screen locked at 15:13:11, eight minutes before the first run (`CGSSessionScreenIsLocked` true). A windowed engine renders no frames while the screen is locked (`rendering_pipeline.md` §12). The live pilot reached `Window ready`, then logged no `[CpuTiming]` window in 240 s, with the GPU at 0% (`records/live-pilot/`). The screen stayed locked to the end. Unlocking needs the owner.

What could be measured offline was measured instead, on the named machine, and the answers below rest on it:

- **Walk cost.** The engine's own `determine_visible_cells`, timed in-process at every H3 pose and over the full 9g street and rooftop sets (`tools/walktime.main.rs`). This is deterministic CPU work, unaffected by the lock. It answers (a).
- **GPU cost.** Offscreen 4K static captures (`--capture`, which renders while locked), read through Metal System Trace GPU intervals. These are relative and indicative, not live frame times. Their caveats are listed under §GPU below. They bear on (b) and (c).
- **Pool layers and VRAM.** The capture report's streamed-pool counters and `ioreg` AMD VRAM polling, for (d).

The live `--start-pose` runs remain owed. The live script `scripts/run1.sh` is piloted up to launch, so they can follow once the screen is unlocked.

## Pinning (testing_guide §Resource bounds)

- **Machine.** MacBookPro16,1, Intel i9-9980HK (8 cores, 2.4 GHz), 32 GB, AMD Radeon Pro 5300M 4 GB (Metal, no timestamp queries), with an Intel UHD 630 beside it. macOS 26.6.2 (25G83). Displays: the built-in Retina panel and an external 5K main display, both locked and blanked. Nothing else ran: no bake, build or second engine overlapped any timed run.
- **Engine build.** `cargo build -p postretro --features dev-tools`, the dev profile `frame-cost/run1.sh` used, frozen as `postretro` `9164ab4a6432` (live; pilot only). The capture build is the same with `capture` added, `postretro-capture` `3a82f49ca8d1`.
- **Walk tool.** `tools/walktime.main.rs`, built outside the workspace in four variants:
  - against `crates/visibility` (the 5,000-step cap, "capped"), and against 9g's copy whose only edit is `MAX_PORTAL_WALK_STEPS` = 10,000,000 ("uncapped"; `diff -r` against HEAD shows only that line);
  - each in the workspace dev profile (opt 2, debug assertions, the engine's H3 build) and the workspace release profile;
  - binaries `9527009ddd4d` (capped dev), `e4d7c32567d0` (capped release), `aa01cc36964a` (uncapped dev) and `009f14ae2c41` (uncapped release).

  The tool uses 9g's harness camera (100° HFOV at 16:9, near 0.1 m, yaw and pitch) and `TimingGate::ON`. Per pose it makes 3 warm calls, then calls until 40 ms have passed and at least 15 calls are done, and reports the median of the whole call (the engine's `visibility` stage) and of the `PortalWalk` stage. H3 pose sets get 3 repeats. Full sets get one pass, since per-pose spread is 0.2–0.4% median.
- **PRLs.**
  - before: `target/occluder-baselines/<map>/<map>.prl`, with `--baked-root target/occluder-baselines/baked`.
  - cap19 and cap26: `target/occluder-9g/<side>/<map>/<map>.prl`, with `--baked-root target/occluder-9g/baked`.
  - **Hallway rebaked** for cap19 and cap26 with 9g's frozen `prl-build-cap19` `dd9962f623e0` and `prl-build-cap26` `5910f5856f08`, using `--sh-probe-spacing 4.0 --no-cache`, into `target/occluder-h3/<side>/` (`scripts/bake_hall.sh`, `records/hallway-*.bake.time`, 918 and 959 s). Both match 9g's bakes byte for byte in size (2,677,370,617 and 2,696,704,065 B, the sizes 9g's sweep headers record). Cells and portals match 9g too: cap26 has 7,439 cells and 2,368 runtime portals, cap19 7,846 and 2,950. Free disk fell from 25 to 18 GB, and `target/debug/incremental` was not touched.
  - Capture resolves receiver models from the PRL's grandparent directory. `target/occluder-{baselines,9g/cap19,9g/cap26,h3/cap19,h3/cap26}/models` are symlinks to `content/dev/models`; before them, all 36 campaign-test attempts failed with no report.
- **Poses.** In `poses/`, eye poses in engine metres:
  - occlusion-test: the 9 `frame-cost/poses.txt` poses. Those are `--start-pose` capsule centres at y 1.0, so the eye is at centre + 0.5 m (`player.ts`: centre 1.0 m above the feet, `eyeHeight` 0.5).
  - street: the 8 costliest uncapped street walks and the 8 costliest rooftop walks from 9g, including `g5.3.0@180`, `g6.3.0@180`, `roof:B4.s2.c@180/-15` and `roof:B3.s1.e@180/-15`. Plus the 4 `stress-street.README.md` sweep poses, whose 0.81 m centre-above-floor converts to an eye at floor + 1.5 m (floors 0, 6 and 11 m, `tools/stress_street.py`).
  - campaign-test and arena: 4 each, ranked by tris-in-frustum rise against before in 9g's sweep (`scripts/pick_tris.py`).
  - hallway: `g13.16.2@0`, the only sweep pose in one of cap26's five largest mandatory-set cells (cell 4575, 112.5 M texels; no sweep pose lands in the four larger ones).
- **Captures.** 3840×2160 offscreen, 240 warm-up frames and 300 sampled frames, `POSTRETRO_SH_STREAMING=sync-proof` (capture requires it). They ran under `caffeinate -dimsu`, with `pgrep -x ScreenSaverEngine` checked before and after each run (never present) and the AMD GPU polled every second. A trace run re-runs the same scene under `xctrace record --launch` (90 s limit). Busy time is the per-frame union of every postretro GPU interval, with frames delimited by the once-per-frame `Capture Measurement Encoder` and the first 60 frames skipped (`gpuframes.py`).
- **Upstream limits.** The 5,000-step cap, the 15-layer pool cap the pool grows from, and the capture's 1-in-flight submit wait. No override touches the workload.

## (a) Step-cap re-size: walk cost per 1,000 portal tests

The walk's cost is linear in portal tests on every side and set (R² 0.990–0.998). The fit spans 2,960 street poses and 608 rooftop poses:

| Profile | µs per 1,000 tests (fits over the 3 sides × 2 sets) | per-pose ratio, p50 / p95 |
|---|---|---|
| dev (the engine's build) | 197.5–204.5 | 193–198 / 208–216 |
| release | 154.5–160.4 | 151–155 / 162–171 |

Before's 18,513-test walk takes 3.71 ms dev, on the same line, so the fit holds to 3× the cap.

**Live factor.** This hot in-process loop is a lower bound. research.md's live dev-build runs on occlusion-test (2026-10-26, `frame-cost/results/cpu_timing.txt`) measured `portal_walk` at 0.28–0.35 µs per test, median about 0.30 (18 pose × variant rows of 100 tests or more). The tool measures 0.21 µs per test at the same poses on the before PRL (P1 2,019 tests, 0.426 ms). So live ≈ 1.4× the tool (range 1.3–1.7). The factor is an estimate and still needs a live street run.

| Walk | tests | in-process, dev | est. live, dev (×1.4) | release in-process |
|---|---|---|---|---|
| Worst swept walk, cap26 (`roof:B4.s2.c@180/-15`) | 6,234 | 1.26 ms | ≈ 1.8 ms | 0.99 ms |
| Worst swept walk, cap19 (same pose) | 7,242 | 1.47 ms | ≈ 2.1 ms | 1.15 ms |
| Step cap 12,468 (cap26's 2× rule) | 12,468 | 2.49 ms (fit) | ≈ 3.5 ms (3.2–4.2) | 1.95 ms |
| Step cap 14,484 (cap19's 2× rule) | 14,484 | 2.95 ms (fit) | ≈ 4.2 ms (3.8–4.9) | 2.31 ms |
| Today's trip (walk to 5,000 + frustum fallback), median over tripped poses | 5,000 | 1.08 ms | ≈ 1.5 ms | – |

Against a 16.7 ms frame:

- The worst walk any swept pose would take is about 1.8 ms live (11% of the frame).
- A walk at the cap itself, which no swept pose reaches, is about 3.5–4.2 ms (21–25%).
- research.md's live occlusion-test frames ran work CPU 5.5–8.3 ms beside about 12.7 ms of GPU, vsync-bound at 16.6 ms. Even a cap-length walk on top of that CPU work stays under 16.7 ms on CPU. The street's live CPU and GPU frame are unmeasured.

The walk on the costliest street and rooftop poses costs about the same as today's trip (1.1–1.3 ms against 1.08 ms in-process). A trip also discards the walk and frustum-culls every cell, and the GPU pays for that (§(c)).

**The step-cap re-size is cheap on CPU.** Re-sizing to 12,468 adds about 0.1–0.2 ms in-process at the currently tripping poses, and bounds the worst case at about 2.5 ms in-process. The re-size remains an owner ruling, and one live street run should confirm the 1.4× factor.

Per-pose walk rows, capped and uncapped on all three sides with trip flags, are in `tables.md` §Walk cost. Every street and rooftop pose in the H3 set that tripped at cap26 also tripped before. `roof:B3.s1.e@180/-15` is the exception: 4,805 tests before (no trip), 6,003 at cap26.

## GPU: what the offline captures can and cannot say

- **Completion time is paced, not GPU-bound, on the locked machine.** The capture's per-frame completion samples cluster near multiples of 16.7 ms: 16.6, 34, 50, 66.6 ms. During street captures the GPU sat at about 55% utilization and 300–1,000 MHz. So the repeat-1 completion table in `tables.md` exaggerates differences that cross a step. Do not quote it.
- **The traced GPU busy time is the measure used.** It counts the per-frame union of GPU intervals, with paging (`Page On`, SDMA) and upload copies included where they overlap nothing else. Clock is uncontrolled, at a mean busy clock of 944 MHz with runs ranging 445–1,208 MHz. The scene is 4K with no upscale, against the live 1280×720 scene at divisor 2. The capture has no viewmodel or player and runs `sync-proof` SH. So differences under about 5% are noise, and absolute values are not live frame times.
- Only one repeat of each traced pose exists. The batch stopped after repeat 1 once the pacing was found. Then the regime made further runs invalid (§(d)).

Traced 4K GPU busy per frame, median (p95), ms. All runs predate the regime except the two marked.

| Map, pose | before | cap19 | cap26 | Main movers (before → cap26, ms per frame) |
|---|---|---|---|---|
| occlusion-test `P1` | 35.3 (37.2) | 29.7 (30.6) | 29.3 (30.2) | Textured 31.2 → 26.0 |
| occlusion-test `V1` | 28.3 (30.4) | 28.3 (29.3) | 28.6 (30.6) | none |
| street `rd:promenade` | 28.7 (29.6) | 37.1 (53.6) | 37.1 (53.4) | Depth pre-pass 0.25 → 1.76, upload copies 0 → 4.7, Textured 24.7 → 27.9 |
| street `rd:elevated` | 31.7 (32.2) | 53.0 (54.1) | 53.4 (54.5) | Depth pre-pass 0.31 → 1.95, upload copies 0 → 8.8, Textured 27.9 → 30.5 |
| street `roof:B3.s1.e@180/-15` (trips on the caps only) | 32.9 (34.1) | 47.7 (51.8) | 47.6 (51.8) | Depth pre-pass 0.35 → 2.47, upload copies 0 → 5.8, Textured 27.5 → 31.6 |
| street `st:g5.3.0@180` (trips everywhere) | 49.2 (53.4) | 44.2 (48.4) | 43.9 (47.8) | Textured 35.0 → 32.0 |
| arena `g8.3.0@180` | 29.4 (30.2) | 31.2 (33.5) | 32.2 (33.6) | Textured 26.3 → 28.2 |
| hallway `g13.16.2@0` | 25.9 (27.8) | 21.9 (24.4) | 21.5 (28.2) | Textured 22.1 → 20.2 |
| campaign-test `g1.9.0@270` | 53.3 (59.5) | *257 (268), regime* | *259 (270), regime* | caps unmeasured outside the regime |

CPU `render_record` in the capture tracks the drawn set. On the street it is 0.75–2.81 ms before against 0.88–3.16 ms on the caps, over the 20 poses. On the arena it is 0.51–0.88 against 0.57–1.08 ms, and on the hallway 1.60 against 1.87–1.92 ms. At occlusion-test P1 it falls, 1.45 → 1.18 ms (`tables.md`). Walk times per pose are in §(a) and `tables.md`. Render submit and work CPU need the live run.

## (b) Cap value

There is no GPU difference between cap19 and cap26 at any traced pose. Every pair lies within 1.0 ms (3% or less), inside the uncontrolled-clock noise: P1 29.7 / 29.3, `rd:elevated` 53.0 / 53.4, `rd:promenade` 37.1 / 37.1, arena 31.2 / 32.2, hallway 21.9 / 21.5. Repeat 1's untraced completion medians agree, at a median gap of 0.6% over 30 pairs (max 23%, where pacing steps).

cap26 walks less at every costly pose, 10–15% at the street and rooftop maxima (1.22 against 1.40 ms in-process at `g5.3.0@180`). It also trips less (9g: 19 + 22 against 29 + 34).

cap19 holds a smaller pool at the hallway pose: 23 layers, 336 MiB active, against cap26's 39 layers (560 MiB) and before's 34 (490 MiB). The AMD VRAM peak during the run is 2.40 GiB against 2.62, and before's is 2.54.

**On frame time, the cap value is a wash.** cap26 is cheaper on the walk by about 0.2 ms at worst. cap19 is cheaper on hallway pool VRAM by about 220 MiB.

## (c) Guardrail: do tris and texels show up?

- **occlusion-test and hallway: no.** GPU falls (P1 −17%, hallway −17%), or is flat (V1).
- **arena: slightly.** The highest-rise pose is up 6–10% (+1.8–2.8 ms at 4K), within reach of noise from one repeat.
- **street: yes, and larger than its +6% tris predicts.** At the typical README poses, GPU busy rises 29% (`rd:promenade`) and 68% (`rd:elevated`) at both caps. Two passes that were nearly absent before carry most of it:
  - `Depth Pre-Pass`, 0.25–0.35 → 1.65–2.55 ms;
  - `Per-frame Upload Copies`, 0 → 4.2–9.0 ms per frame. It shows before only at a tripping pose (6.8 ms at `g5.3.0@180`).

  `Textured Pass` rises 10–19%. `roof:B3.s1.e@180/-15` pays the same pattern because it trips on the caps (+45%). Where every side trips (`g5.3.0@180`), the caps are cheaper (−11%).
- **campaign-test: unknown.** Only before was traced outside the regime.

These are 4K offline numbers under the caveats above. The street rise needs live confirmation before it is judged against the guardrail. If it holds, it is a renderer-side cost of large cells (depth pre-pass draw set and per-frame upload size), not of the walk.

## (d) VRAM and the slowdown regime

- **Pool layers** (capture report, after the preload drain), before / cap19 / cap26:
  - occlusion-test 10 / 10 / 9 (154 / 154 / 140 MiB active);
  - street and arena 2 / 2 / 2 (42 MiB);
  - hallway `g13.16.2@0` 34 / 23 / 39 (490 / 336 / 560 MiB).

  AMD VRAM in use peaked at 1.3–2.2 GiB on most maps and at 2.40–2.62 GiB on the hallway, all under the 4 GiB card. Growth itself was not exercised: a static capture drains once, `growths` 0. The retiring-generation transient 9f estimated is therefore still unmeasured.
- **Regime.** There was one onset, at about 18:16, at the start of the first cap19 capture on campaign-test, immediately after campaign-test's before capture (normal, 53 ms).
  - From then on every run was in the regime, every side including before. occlusion-test P1 cap26 went from 34 to 225 ms completion. campaign-test ran 261–268 ms on all three sides, over 2 repeats each.
  - Six probes over 18:40–18:54 held at 223–226 ms while the GPU cooled from 69 to 56 °C (`records/capture/probe.out`).
  - At onset there was no display power event (`pmset -g log`), and the VRAM peak (2.1–2.2 GiB) was below the normal hallway cap26 run four minutes earlier (2.47 GiB).
  - Idle VRAM in use before a run ranged 0.68–1.23 GiB, median 0.80, over 141 runs before onset, and 0.90–1.39 GiB, median 1.16, over 13 after. This is the one VRAM signal that moved, and it could be a cause or an effect.
  - Since before is affected equally and the pool sizes are small at the onset pose (14–15 layers), **the regime does not correlate with this brief's pool size or growth.** It is machine state, consistent with the open lead in the owner's notes.
- **Incidence.** No run before onset was in the regime: 104 accepted captures and 25 traces. After onset, all 15 runs were: 2 traces, 6 untraced captures, 1 recheck and 6 probes. They were excluded from every table except where marked, and not repeated, since the regime did not clear. No run was discarded for a screen saver: `ScreenSaverEngine` was never present.

## Owner rulings needed

1. **The live H3 run is still owed.** It needs the screen unlocked and kept awake, since screen lock blanks the display about 16 s after each `caffeinate` assertion ends. Ideally the regime is also cleared first: a reboot, or wherever the regime lead goes. It should confirm the 1.4× live walk factor on one street pose and measure live GPU on the street README poses.
2. **Step-cap re-size.** On CPU the case holds. The walk is linear at about 0.20 µs per test (dev), so a 12,468 cap's worst case is about 2.5 ms in-process, ≈ 3.5 ms live. The real worst swept walk is about 1.3 ms, ≈ 1.8 ms live, and a trip today costs about as much CPU plus a frustum-wide draw.
3. **Street GPU at typical poses** rises 29–68% (4K offline) at both caps, through the depth pre-pass and per-frame uploads. This is a guardrail question the brief did not expect. It needs live confirmation, and possibly a follow-up outside this brief's frozen renderer.
4. **Cap value.** The two caps tie on GPU. cap26 wins the walk and trips; cap19 wins hallway pool VRAM.

## Files

- `README.md` (this file) and `tables.md` (generated: `parse_walk.py`, `parse_cap.py`, `parse_trace.py`).
- `poses/`, the H3 pose sets. `tools/walktime.main.rs` with `walktime-{capped,uncapped}.Cargo.toml`. `gpuframes.py`, the per-frame GPU busy reducer.
- `scripts/`:
  - `batch_walk.sh` and `batch_cap.py` (the batches);
  - `run_cap.sh` and `trace_cap.sh` (one capture, plain or traced), `trace_set.sh`;
  - `run1.sh` (the live run, piloted to launch only);
  - `amdstat.sh`, `probe.sh` (the regime probe), `bake_hall.sh`, `pick_tris.py`.
- `records/`:
  - `walk/`: H3 pose-set records, 3 repeats, and the 40 costliest poses of each full-set record;
  - `trace/*.frames.txt`;
  - `capture/batch_cap.status` (every attempt and verdict) and `probe.out`;
  - `live-pilot/`;
  - `hallway-*.bake.time`.
- Raw capture reports, AMD polls, full walk records and the hallway PRLs (`target/occluder-h3/`) stay outside the repo, in the session scratchpad and gitignored `target/`.
