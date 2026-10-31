# Mover depth pre-pass: Mac A/B kit (Radeon Pro 5300M)

Measures commits `0e93759a8` + `eb7e97412` ("movers join the depth pre-pass"): kinematic
brush movers write depth in the main and planar depth pre-passes through a vertex-only
`vs_depth`, and the Kinematic Brush Pass then shades `Equal` with read-only depth. Before
is `5971fb2b6` (the parent of `0e93759a8`); after is `16c7659ff` on
`claude/compassionate-bohr-en97cc`, which also merges `origin/main`'s netcode and
level-lifecycle commits (no renderer or capture code). Results: `results.md` and
`results.json` (which folds in the `results-batch.json` / `results-pilot.json` that `run.py`
writes); per-run tables in `results-batch.md` and `results-pilot.md`; per-run records in `runs/`;
pixel parity in `parity.json`; framing previews (after binary, 640 px) in `previews/`.

## Machine

MacBook Pro 16" 2019 (MacBookPro16,1), Intel Core i9-9980HK, AMD Radeon Pro 5300M 4 GB
(Navi 14, Metal, no timestamp queries, so `POSTRETRO_GPU_TIMING` reports unsupported)
beside Intel UHD 630; macOS 26.6.2; Xcode xctrace 16.0. On AC power, display awake
(`caffeinate`), no build, bake, test or second engine during any traced run.

## Files

- `run.py`: the batch driver, adapted from `measurements/fluid-base-pass-cost/metal/`
  (`run.py`, `run_ablation.py`): caffeinate with user activity re-declared, 15 s idle gap,
  idle ioreg snapshot, 0.5 s ioreg clock/temperature polling during each run,
  screen-lock / screen-saver / foreign-process checks, `xctrace record --launch` of a
  headless `--capture` run under a watchdog, `xctrace export` of `metal-gpu-intervals`,
  trace bundle and `$TMPDIR/instruments*.ktrace` deleted once reduced. Arms are
  `<pose>:<before|after>`, interleaved forward then reversed.
- `gpu_time.py`: per-encoder interval-union reduction. Frames are bounded by the
  Candidate Cull Pass (a compute pass whose rows the trace never drops), never by
  render-pass encoder counts (`context/plans/drafts/gpu-pass-paired-ab/index.md`). The timed
  span is the last `samples` framed frames; a frame is complete when Depth Pre-Pass and
  Textured Pass appear once and Kinematic Brush Pass / SDF Shadow Pass appear at their
  modal count. Figures are means over complete frames.
- `parity.py`: untraced 4 + 4-frame captures of each pose with both binaries, compared
  per pixel (count, max channel delta, histogram, diff image).
- `scenes/<pose>.json`: the capture scenes (`run.py --write-scenes`), with placeholder
  map/output/report paths that `run.py` fills per run.

## Poses

TrenchBroom units (z up, 0.0254 m/unit) map to engine metres as (x, y, z) → (−y, z, −x)
(`build_pipeline.md` §Supported Map Formats). Map yaw keeps its sign
(`entity_model.md` §4): engine forward is (−sin yaw, 0, −cos yaw), so yaw 90 faces map +y.
All arms: 3840×2160, FOV 100, pitch 0, eye at the README's station height (z 48 = 1.22 m).
Capture spawns movers at their authored rest pose: wall closed, room lowered.

| pose | map units, yaw | engine metres | what it shows |
|---|---|---|---|
| `wall` | (368, 720, 48), 90 | (−18.288, 1.2192, −9.3472) | Station 1: the closed wall's south face, ~37% of the frame (floor fills the lower half). Primary arm. |
| `north` | (368, 1536, 48), 270 | (−39.0144, 1.2192, −9.3472) | Station 3: the wall's north face and the enemy plinth. |
| `offscreen` | (368, 720, 48), 225 | (−18.288, 1.2192, −9.3472) | Station 1 facing south-west: no mover in the frustum, but both builds still record the movers (off-screen, GPU-clipped). |
| `null` | (1200, 1500, 48), 90 | (−38.1, 1.2192, −30.48) | Back room east, facing the north wall: zero mover draws in both builds. |

**Why two "no mover" poses.** Movers are admitted to the beauty pass by visible-cell
bounds (`runtime_movers.rs` `mover_visible_against_cell_bounds`), not by the frustum.
Every arena pose tried (station 1 at yaw 225, (150, 150) at yaw 225) recorded a Kinematic
Brush Pass every frame: the lowered room's roof is flush with the arena floor, so its
bounds touch the arena cells. `null` was found by probing back-room poses at 4K and
reading the Kinematic Brush Pass count per frame from the trace (720p probes drop
render-pass rows and cannot answer it). `offscreen` keeps the brief's original
"station 1 facing away" pose as the admitted-but-unseen case.

## Reproduce

From the workspace root. Binaries are built once per commit; the before binary comes
from a disposable worktree (`development_guide.md` §Worktree builds).

```bash
SCRATCH=<scratch dir>
# after (this branch)
CARGO_INCREMENTAL=0 cargo build --release -p postretro --features capture,dev-tools
mkdir -p $SCRATCH/after && cp target/release/postretro $SCRATCH/after/postretro
# before (parent of the change)
git worktree add $SCRATCH/wt-before 5971fb2b6
(cd $SCRATCH/wt-before && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$SCRATCH/wt-before/target \
  cargo build --release -p postretro --features capture,dev-tools)
mkdir -p $SCRATCH/before && cp $SCRATCH/wt-before/target/release/postretro $SCRATCH/before/postretro
rm -rf $SCRATCH/wt-before/target && git worktree remove $SCRATCH/wt-before
# map: the compiler is untouched by the change, so one .prl serves both arms. It must
# sit under content/<mod>/maps/ so the engine finds the repo's baked/materials sidecars.
cargo run --release -p postretro-level-compiler -- content/dev/maps/mover-setpiece.map \
  -o content/dev/maps/mover-setpiece.prl

export BIN_BEFORE=$SCRATCH/before/postretro BIN_AFTER=$SCRATCH/after/postretro \
       MAP_PRL=$PWD/content/dev/maps/mover-setpiece.prl
python3 measurements/mover-depth-prepass/parity.py $SCRATCH/parity
# pilot: one run per arm, stop at the first invalid run
python3 measurements/mover-depth-prepass/run.py --tag pilot --rounds 1 --abort-on-invalid
# batch: 2 interleaved rounds, 120 warmup + 240 sampled frames, 15 s idle gap
python3 measurements/mover-depth-prepass/run.py --tag batch --rounds 2
```

A run is rejected when: the busy GPU clock median is under 1000 MHz or fewer than five
samples read busy; the screen locked or the screen saver ran; a cargo/rustc/prl-build
process was seen; the Candidate Cull count does not fit warmup + samples; fewer than 90%
of steady frames are complete; Depth Pre-Pass / Textured Pass are not once per frame; the
Kinematic Brush Pass modal count is not 1 (0 at `null`); or the GPU frame time over
complete and incomplete frames disagrees by more than 3%.

## Caveats

- **Per-pass split between consecutive render passes is a boundary attribution.** On this
  GPU the trace reports Depth Pre-Pass, Textured Pass and Kinematic Brush Pass as
  back-to-back intervals on the one GFX channel, each starting where the previous one
  ended. An encoder whose own work is tiny absorbs the drain of the previous one: at
  `offscreen` (movers recorded but GPU-clipped) one frame read Textured Pass 0.16 ms and
  Kinematic Brush Pass 38.4 ms. The label-independent figures are the sum of the three
  passes and the whole-frame union; read the Forward / Kinematic split as meaningful
  only where both passes do real fragment work (`wall`, `north`).
- The `.prl` is a warm (cached) dev bake: prl-build warns that its SH indirect is
  approximate. Both arms load the same file, so the A/B is unaffected.
- The map has no SDF atlas (`SdfAtlas section missing — runtime SDF shadow pass
  disabled`), so the SDF Shadow Pass never runs here; the change's SDF interaction
  (tracing from mover depth) is not exercised by this map.
- `before` lacks `origin/main`'s netcode/lifecycle commits that `after` merged; they touch
  no renderer, capture or shader code.
