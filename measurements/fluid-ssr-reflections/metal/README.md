# Fluid SSR reflections: Metal timing kit (5300M)

Times the screen-space Fluid Reflection Pass, and the passes around it, on the Radeon Pro 5300M through Instruments' Metal System Trace. That Metal adapter has no timestamp queries, so `POSTRETRO_GPU_TIMING` reports nothing there (`rendering_pipeline.md` §12, "Without timestamp support").

## How it works

- `run.py` and `gpu_time.py` are copies of `measurements/fluid-base-pass-cost/metal/`. They provide the recording, the `xctrace record` watchdog, ioreg clock polling every 0.5 s, and the machine-state checks every 2 s: no `cargo`, `rustc`, `prl-build` or second `postretro` may run, and the screen must not lock or start a screen saver, or the run is invalid. They also do the complete-frame reduction and delete the trace once it is reduced. Each file's docstring lists what changed. In short: the Fluid Reflection Cell Pass is replaced by the Fluid Reflection Pass, the scene gains the SSR fields, `light_term_mask` is omitted (every term is on), `POSTRETRO_SH_STREAMING=sync-proof` is kept (as on the 1660), and the binary comes from `SSR_BIN`.
- `run_sweep.py` is the driver. Each arm is one capture scene. Runs are interleaved ABBA over the rounds (forward, then reversed), with a 15 s idle gap before every launch.
- Per pass, the figure is GPU time per complete steady frame. Frames are bounded by the cull-pass marker, and a frame counts as complete only when it holds exactly one Depth Pre-Pass, Textured Pass and Fluid Surface Pass encoder, plus one Fluid Reflection Pass encoder when the arm records it. Whole-frame time is the union of every labelled interval in a complete frame. A pass's time is the union of its encoder's intervals, so nested and coalesced rows count once. Frame counts never come from render-pass encoder counts.
- From medium quality up, the Fluid Reflection Pass is required. If it is missing, the run is flagged as an anomaly and reported as `ABSENT`, never as 0. At off the pass must not appear. At low the kit records whatever the trace holds.

## Arms

`<pose>-<res>:<quality>[:style][:k=v,...]`, for example `near-auto:high:pixel:steps=128,divisor=270,thickness=1.0,fraction=1.0`.

| part | values | scene field |
|---|---|---|
| pose | `near` (e24 still pool, straight down), `probes` (reflection-probes-test pool, straight down), `oblique-near`, `oblique-probes`, `oblique-large` (placeholders) | `map`, `camera` (fov 100) |
| res | `4k` 3840×2160, `auto` 1536×960, `ext` from `SSR_EXT_RES=WxH` | `resolution` |
| quality | `off` `low` `medium` `high` `very_high` `ultra` | `reflection_quality` |
| style (optional) | `pixel` `smooth` | `reflection_style` |
| k=v (optional) | `fraction=<f>`, `steps=<n>`, `divisor=<n>`, `thickness=<m>`, any subset | `reflection_march` |

A field the arm leaves out is left out of the scene. Several arms are separated by commas: `--arms near-auto:off,near-auto:high:steps=96,divisor=180`.

The oblique poses are placeholders. A real run refuses any arm whose pose or res is still pending. Pass `--skip-pending` to drop those arms instead, and `--dry-run` lists them as `PENDING`. To fill a pose, edit `POSES` in `run_sweep.py` or pass `--poses poses.json` (or set `SSR_POSES`). That file maps each pose name to a scene fragment: `{"oblique-probes": {"map": "content/dev/maps/reflection-probes-test.prl", "camera": {"position": [x, y, z], "yaw_deg": 0, "pitch_deg": -20}}}`.

Presets (`--preset`):

- `tiers`: every quality × `near`, `probes` × `4k`, `auto`; then `near-auto:medium:steps=48`; then `near-ext:off` and `probes-ext:off`; then every quality × the three oblique poses × `auto`, `ext`.
- `sweep`: `near-auto:high:pixel`, fraction 1.0, thickness 1.0, steps {64, 96, 128, 192, 256} × divisor {135, 180, 270, 350}.
- `pilot`: `near-auto:off`, `near-auto:high`, `near-4k:ultra`, `probes-auto:medium`.
- `confirm-smooth`: re-runs each `--arms` finalist with style `smooth`.
- `finalists-ext`: re-runs each `--arms` finalist at res `ext`.

## Build

The binary must be named `postretro`, because the reduction keeps only trace rows from a `postretro (pid)` process. Build it, then copy it out so a later build cannot change it mid-batch:

```
CARGO_INCREMENTAL=0 cargo build --release -p postretro --features capture,dev-tools
mkdir -p <scratch>/bin && cp target/release/postretro <scratch>/bin/postretro
```

The binary must already parse `reflection_quality`, `reflection_style` and `reflection_march`. The capture scene is `deny_unknown_fields`, so a binary without them rejects the scene. Each batch opens with an untraced smoke capture of the arm that sets the most of these fields, to catch that before any trace is recorded.

## Maps

The kit reads compiled `.prl` files from `content/dev/maps/` (`e24-environment-test.prl` and `reflection-probes-test.prl`). It refuses to start if one is missing. The compile lines come from `reflection-probes-test.map`'s header and `e24-environment-test.README.md` (the `.map` header has none):

```
cargo run -p postretro-level-compiler -- content/dev/maps/e24-environment-test.map -o content/dev/maps/e24-environment-test.prl
cargo run -p postretro-level-compiler -- content/dev/maps/reflection-probes-test.map -o content/dev/maps/reflection-probes-test.prl
```

Finish every build and bake before starting a batch. If a `cargo`, `rustc`, `prl-build` or `postretro` process is running at start, the kit aborts. Between runs it waits up to 30 min for one to finish, and a run that sees one is invalid.

## Run

Use mains power, close other GPU apps, and leave the screen unlocked (caffeinate is held for you). Check every command first with `--dry-run`, which prints each arm's scene JSON, its launch command and the run order, and launches nothing.

```
export SSR_BIN=<scratch>/bin/postretro
export SSR_EXT_RES=2560x1440        # the external monitor's play extent, once known
K=measurements/fluid-ssr-reflections/metal/run_sweep.py

python3 $K --preset pilot --dry-run
python3 $K --preset pilot --pilot                     # one launch per arm, stops at the first invalid run
python3 $K --preset tiers --skip-pending              # or fill the oblique poses first
python3 $K --preset sweep
python3 $K --preset confirm-smooth --arms <finalist>,<finalist> --tag smooth
python3 $K --preset finalists-ext --arms <finalist>,<finalist> --tag ext
```

Defaults are 120 warmup + 360 sampled frames and 2 rounds. Override them with `--warmup`, `--samples` and `--rounds`. Results go to `../results-<tag>.{md,json}`, where the tag defaults to the preset name and gets `-pilot` under `--pilot`. The table gives fluid_reflection, fluid_surface, forward (Textured Pass) and whole-frame GPU ms, each as the mean ± half the min–max range across valid rounds. It also gives each run's busy-median core clock and its VRAM free (before launch / minimum during the run). Per-run records (`*.run.json`, `*-gpu.json`, scenes and capture reports) go to `runs/`, and gzipped exports to `raw/`.

Other env: `IDLE_GAP` (15 s), `RECORD_TIMEOUT` (1200 s watchdog), `MIN_FREE_GB` (25), `SKIP_RAW=1`, `TRACE_DIR`. `CLOCK_FLOOR_MHZ` is off by default because the old kit had no floor; set it to mark runs below that busy median clock invalid. `SH_STREAMING=<mode>` overrides `POSTRETRO_SH_STREAMING` (default `sync-proof`).
