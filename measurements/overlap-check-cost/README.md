# Tick overlap check cost on stress-env-volumes

The per-tick CPU cost of the movement overlap check (`movement.md` §1), taken from its
own stage, `sim_overlap_check`, on `stress-env-volumes` (64 agents plus the player
pawn). This was measured once to decide between checking every body and checking only
bodies that moved. A body that did not move already skips the check (`start == end`).

## Result

| Run | `sim_overlap_check` per tick | `sim_tick` per tick |
|---|---|---|
| Check on (`POSTRETRO_OVERLAP_CHECK` unset, debug-assertion build) | 0.024 ms (0.040 ms in the first 600 ticks) | 3.42 ms |
| Check off (`POSTRETRO_OVERLAP_CHECK=0`) | absent | 3.44 ms |

Values are cumulative per-tick means at tick 3600; the 600-tick checkpoints are in
`results.txt`. The checkpoints are matched elapsed times, since agents converge on the
player.
- **Cost:** under the 0.1 ms per-tick threshold (above it, the check would run only for
  bodies that moved that tick), so every moving body is checked.
- **Tick time:** `sim_tick` is the same on and off, within run-to-run noise.
- **Determinism:** both runs' state dumps are byte-identical. The check reports and
  never moves a body.
- **Warnings:** no overlap warning fired in either run.

## Method and its limit

This container has no GPU or display, so the windowed `POSTRETRO_CPU_TIMING` run in
`content/dev/maps/stress-env-volumes.README.md` cannot run. The headless batch driver
(`cargo run -p xtask -- observe`) runs the same fixed tick, `simulate_tick`, on the same
map, but batch mode keeps stage timing off by design. `timing.patch` is a
measurement-only patch, applied locally and never merged, that turns the tick's `TimingGate` on under `OVL_MEASURE=1` and prints
cumulative per-tick means every 600 ticks. Everything else is the shipped code.

- Machine: 4-vCPU Intel Xeon @ 2.80 GHz (cloud container), nothing else running.
- Build: default `dev` profile (`cargo run -p xtask -- observe`, features
  `observability`), debug assertions on.
- Map: `prl-build content/dev/maps/stress-env-volumes.map -o
  content/dev/maps/stress-env-volumes.prl --sh-probe-spacing 10.0 --lightmap-density
  0.25 --no-cache` (the README's interactive bake).
- Run: `OVL_MEASURE=1 [POSTRETRO_OVERLAP_CHECK=0] cargo run -p xtask -- observe
  runspec.json`, with `runspec.json` = `{"map":
  "content/dev/maps/stress-env-volumes.prl", "ticks": 3600, "dump": {"cap": 1,
  "events": false}}`. No input: the player stands at its spawn and agents act on the AI
  they run headless.
- Cleanup: the baked `.prl` and the scratch logs were deleted afterwards.

A windowed release run on the owner's machine remains the reference for frame-level
cost.
