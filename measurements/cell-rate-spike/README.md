# Fluid reflection cell-rate spike: Mac Metal System Trace kit

**Send back `measurements/cell-rate-spike/results.md`** (and `results.json` and `runs/count-stats.json` beside it) when the batch finishes. They hold every figure the spike needs from the 5300M:
- each arm's fluid cost per frame (Fluid Surface Pass + Fluid Reflection Cell Pass) for every run;
- the core clock and temperature during each capture;
- per view: cells, visible fluid pixels, pixels per cell and atlas size;
- idle VRAM and utilisation.

For a clean read:
- keep the screen unlocked and awake;
- close every other GPU app;
- stay on mains power.

The batch is headless (`--capture` opens no window). If anything puts a window or the screen saver in front of the run, the run is invalid and the script says so. Leave the machine alone until it prints the table.

For: the owner's 16-inch MacBook Pro (Intel i9, Radeon Pro 5300M), macOS, with Xcode or the Command Line Tools installed (`xctrace`). Run every step from the repo root.

Findings: `context/plans/drafts/reflection-probe-fluid-pass-performance--cell-rate-spike/findings.md`.

## Steps

1. Get the spike branch (the kit lives on it):
   ```
   git fetch && git checkout claude/cell-rate-spike && git pull
   ```
2. Build the engine and the level compiler (release):
   ```
   cargo build --release -p postretro --features capture,dev-tools
   cargo build --release -p postretro-level-compiler --bin prl-build
   ```
3. Compile both maps beside their sources. The `.prl` files are gitignored, so a fresh clone has none. Do not skip `--no-cache`: the bake must be fresh. e24's cold compile takes minutes.
   ```
   target/release/prl-build content/dev/maps/e24-environment-test.map -o content/dev/maps/e24-environment-test.prl --no-cache --no-tui
   target/release/prl-build content/dev/maps/reflection-probes-test.map -o content/dev/maps/reflection-probes-test.prl --no-cache --no-tui
   ```
4. Before the batch:
   - Quit browsers, video, chat and anything else that draws.
   - Make sure nothing else is building: `pgrep -x cargo` and `pgrep -x postretro` should print nothing. The batch aborts if a `postretro`, `cargo`, `rustc` or `prl-build` process is running.
5. Run the batch:
   ```
   python3 measurements/cell-rate-spike/run.py
   ```
   The batch runs in three stages:
   - A 4-frame untraced smoke capture, to prove the binary, maps and scene work.
   - One untraced count capture per view (5 views). Each uses the cell buffer, its cull and the count lever, and records cells, visible fluid pixels and the atlas size.
   - 30 arms × 2 runs = 60 Metal System Traces. The second round runs in reverse order, with 15 s idle between launches.

   Each trace is exported, reduced and deleted at once, together with its `instruments*.ktrace` temp file. It needs about 25 GB free in `$TMPDIR` (override with `TRACE_DIR=...`). Plan for several hours (the blend-cost kit took a few hours for 28 traces). `--rounds 1`, or `--only` with a subset, gives a quick partial pass.
6. Results land in `measurements/cell-rate-spike/`:
   - `results.md` (the tables, printed at the end too);
   - `results.json`;
   - `runs/count-stats.json`;
   - under `runs/`, one `<arm>-r<N>.run.json`, `-gpu.json`, scene file and report per run, plus each arm's round-1 PNG to eyeball.

   `raw/` holds the gzipped trace exports and stays local.

## Arms

All captures use FOV 100, `POSTRETRO_SH_STREAMING=sync-proof` and `RUST_LOG=info`. Capture draws water in pixel style at reflectivity 1. Engine coordinates are metres: x = −map y, y = map z, z = −map x.

| View | Map | Position, yaw, pitch | Resolutions |
|---|---|---|---|
| near | e24-environment-test (still pool, `water_64` at scale 1) | −4.8768, 1.6, −44.2976; 0; −89 | 3840×2160, 1920×1080 |
| probes | reflection-probes-test (the pool) | −6.5024, 1.6, −6.5024; 0; −89 | 3840×2160 |
| far | e24-environment-test (fluid-motion wing, map 2300 1000 400, 10.6 m above the water, looking south across both rows of pools, 10–26 m away) | −25.4, 10.16, −58.42; −90; −30 | 3840×2160, 1920×1080 |

Each view runs three fluid paths, each with the live-lighting ratio as shipped (`live`) and forced to 1 (`r1`):
- `frag`: per-fragment pixel style, as on main;
- `cell`: the cell buffer;
- `cellcull`: the cell buffer with its per-cell frustum cull.

## Useful options

- `--list` shows the arms.
- `--only near-4k-frag-live,near-4k-cell-live` reruns a subset.
- `--rounds 1` gives a quick pass.
- `--dry-run` writes the scene files only.
- `--skip-stats` skips the count captures.
- A run that finds problems is marked invalid in `results.md` and listed at the bottom; rerun those arms with `--only`. Problems include:
  - the screen locked or the screen saver ran;
  - another process ran;
  - the adapter was not the discrete Metal GPU;
  - the fluid pass, or a cell arm's cell pass, was missing;
  - the frame count was off.

  A machine-state failure (lock, screen saver, foreign process) stops the batch after writing the table so far.
- `cost ms` comes from complete frames only. These are the last `samples` frames, bounded by the Candidate Cull Pass, that hold exactly one depth, forward and fluid row, plus one cell row in a cell arm. The trace drops some frames' rows and shifts labels there (`rendering_pipeline.md` §12). `results.md` shows how many frames were complete, and the label-independent GPU frame time over complete and incomplete frames. A gap over 3% marks the run invalid.
- Each traced capture is `warmup 240 + 240` frames per run (`--warmup`, `--samples`, or `CELL_WARMUP`/`CELL_SAMPLES`). The trace records the whole process; the figures use only the frames after the warmup.
- `CELL_BIN=...` points at another engine binary.

## On an adapter with timestamp queries

`run_timestamps.py` runs the same arms and count captures without a trace. It reads each pass from the engine's own GPU timing (`POSTRETRO_GPU_TIMING=1`, needs the dev-tools build) and writes `results-<tag>.md`/`.json`. The spike's Windows GTX 1660 Super figures came from it:
```
python measurements/cell-rate-spike/run_timestamps.py --tag 1660
```
