# Reflection blend-cost spike: Mac Metal System Trace kit

**Send back `measurements/reflection-blend-cost-spike/results.md`** (and `results.json` beside it) when the batch finishes; they hold every figure the spike needs from the 5300M: fluid-pass GPU ms per frame for each arm and run, the core clock and temperature during each capture, and idle VRAM and utilisation. For a clean read, keep the screen unlocked and awake, close every other GPU app, and stay on mains power. The batch is headless (`--capture` opens no window), but if anything puts a window or the screen saver in front of the run it is invalid, and the script says so; leave the machine alone until it prints the table.

For: the owner's 16-inch MacBook Pro (Intel i9, Radeon Pro 5300M), macOS, Xcode or the Command Line Tools installed (`xctrace`), run from the repo root.

## Steps

1. Get the spike branch, then bring this kit onto it (the kit lives on the drafts branch beside the findings note):
   ```
   git fetch && git checkout claude/blend-cost-spike
   git checkout origin/claude/probe-streaming-brief-364a5e -- measurements/reflection-blend-cost-spike
   ```
2. Build the engine and the level compiler (release):
   ```
   cargo build --release -p postretro --features capture,dev-tools
   cargo build --release -p postretro-level-compiler --bin prl-build
   ```
3. Compile both maps beside their sources. The `.prl` files are gitignored, so a fresh clone has none. The spike map's header gives the same command via `cargo run`. e24's cold compile takes minutes; do not skip `--no-cache`:
   ```
   target/release/prl-build content/dev/maps/reflection-blend-spike.map -o content/dev/maps/reflection-blend-spike.prl --no-cache --no-tui
   target/release/prl-build content/dev/maps/e24-environment-test.map -o content/dev/maps/e24-environment-test.prl --no-cache --no-tui
   ```
4. Before the batch: quit browsers, video, chat and anything else that draws; nothing else should be building (`pgrep -x cargo`, `pgrep -x postretro` print nothing). The batch aborts if a `postretro`, `cargo`, `rustc` or `prl-build` process is running.
5. Run the batch:
   ```
   python3 measurements/reflection-blend-cost-spike/run.py
   ```
   It first takes a 4-frame untraced capture to prove the binary, maps and scene work, then records 14 arms x 2 runs = 28 Metal System Traces (second round in reverse order), with 15 s idle between launches. Each trace is exported, reduced and deleted at once, together with its `instruments*.ktrace` temp file; it needs about 25 GB free in `$TMPDIR` (override with `TRACE_DIR=...`). The time it takes is not measured yet; plan for a few hours.
6. Results land in `measurements/reflection-blend-cost-spike/`: `results.md` (the table, printed at the end too), `results.json`, and under `runs/` one `<arm>-r<N>.run.json`, `-gpu.json`, scene and report per run (plus the round-1 PNG per arm, the render to eyeball). `raw/` holds the gzipped trace exports and stays local.

## Useful options

- `python3 .../run.py --list` shows the arms; `--only worst-blend,typ-single` reruns a subset; `--rounds 1` for a quick pass; `--dry-run` writes the scene files only.
- A run that finds problems (screen locked, screen saver, another process, adapter not the discrete Metal GPU, fluid pass absent, frame count off) is marked invalid in `results.md` and listed at the bottom: rerun those arms with `--only`. A machine-state failure (lock, screen saver, foreign process) stops the batch after writing the table so far.
- `fluid ms` comes from complete frames only: steady frames, bounded by the BVH Cull Pass, that hold exactly one depth, forward and fluid row. The trace drops some frames' Textured Pass row and shifts the fluid label onto forward's interval there (seen at 720p, where a plain per-frame average read 4.2–4.7 ms against a true ~6.0). `results.md` shows how many frames were complete and the label-independent GPU frame time over complete and incomplete frames; a gap over 3% marks the run invalid.
- The traced capture is `warmup 240 + 240` frames per run (`--warmup`, `--samples`, or `BLEND_WARMUP`/`BLEND_SAMPLES`); the trace records the whole process and the figures use only the frames after the warmup.

## Arms

Capture at 3840x2160 (to hold the AMD clock up, `rendering_pipeline.md` section 12), `POSTRETRO_SH_STREAMING=sync-proof`, `RUST_LOG=info`, band 1.0:

| Pose | Map | Position (engine m), yaw, pitch | Coverage |
|---|---|---|---|
| worst | reflection-blend-spike | -12.192, 7.62, -12.192; 0; -89 | on |
| typ | reflection-blend-spike | -6.5024, 1.2192, -28.2448; 0; -6 | off |
| e24 | e24-environment-test | -4.8768, 1.6, -44.2976; 0; -89 | on |

Modes `single`, `blend`, `precomputed`, `stochastic` for each pose, plus `typ-blendw0` and `e24-blendw0` (mode blend, coverage off, band 0.0: blend code present, no extra taps).
