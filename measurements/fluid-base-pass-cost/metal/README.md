# Fluid base-pass cost: Metal ablation kit (5300M)

Attributes the reflection-free Fluid Surface Pass's cost on the Radeon Pro 5300M by timing ablated copies of its shader. Findings: `context/plans/done/fluid-base-pass-cost/research.md`.

## How it works

- `run.py` and `gpu_time.py` are copies of `measurements/cell-rate-spike/metal/` (branch `claude/cell-rate-spike`): Metal System Trace recording, watchdog, clock polling, machine-state checks, complete-frame reduction, trace deletion. `run_ablation.py` drives them with its own arms.
- `variants.py` writes whole fluid shader modules into `variants/`, each with one textual edit to `fs_main` (or `fluid_baked_light`, the per-vertex baked terms, so `nosh`/`shindirect`/`shshared`/`noocc` act per vertex; the old `vertexsh` arm is now the shipped shader, `base`). Every edit must match exactly once.
- The engine reads a variant through `POSTRETRO_FLUID_WGSL_EXPERIMENT`. That hook is **not** in the engine: apply `experiment-hook.patch` in a scratch tree, build, copy the binary out, and revert. Never commit it.
- Every arm runs under light-term mask `0x13F` (all terms but specular), which selects the reflection-free pipeline (`rendering_pipeline.md` §7.3a). Capture draws water at opacity 0.6 with no fog, drift or wave.

## Run

```
git apply measurements/fluid-base-pass-cost/metal/experiment-hook.patch
CARGO_INCREMENTAL=0 cargo build --release -p postretro --features capture,dev-tools
mkdir -p <scratch>/bin && cp target/release/postretro <scratch>/bin/postretro
git apply -R measurements/fluid-base-pass-cost/metal/experiment-hook.patch
python3 measurements/fluid-base-pass-cost/metal/variants.py
CELL_BIN=<scratch>/bin/postretro python3 measurements/fluid-base-pass-cost/metal/run_ablation.py \
  --tag <name> --arms near-4k:base,near-4k:nosh,test-4k:base
```

Arms are `<config>:<variant>[:<mask hex>]`; configs and poses are `run.py`'s (`near` = e24 still pool straight down, `test` = probes-map pool straight down). Maps come from `prl-build --no-cache` (see the cell-rate kit's README). Defaults: 120 warmup + 120 sampled frames, 2 interleaved rounds, 15 s idle gap. Results go to `../results-<tag>.{md,json}`; per-run records to `runs/`.

`variants.py` also writes two image-only diagnostics, `shpath` and `shpathfloor`, which colour each water pixel by the SH sampler path it takes (at the surface, and at the scene point behind it).

## Landing A/B

The A/B is `stock` (this branch's shader) vs `today` (the module as `main` composed it, per-fragment baked SH).

- `landing_variants.py [ref]` writes `variants/today.wgsl` (the module from `main` through the hook) and `variants/today-flat.wgsl` (the same, every water fragment opaque magenta: the water mask).
- `parity.py [W H]` captures `stock`, `today` and `today-flat` through `--capture` at 960×540 per view (`near`, `probes`, `mid`, `under`) and writes `results-landing-parity.json`.
- Two-binary A/B (`landing2`): variants `before` and `after` launch `CELL_BIN_BEFORE` / `CELL_BIN_AFTER` (each named `postretro`, in its own directory) with that binary's own shader, no override. Build `before` from the merge-base in a disposable worktree (`development_guide.md` §Worktree builds), e.g. `CELL_BIN_BEFORE=<scratch>/before/postretro CELL_BIN_AFTER=<scratch>/after/postretro python3 measurements/fluid-base-pass-cost/metal/run_ablation.py --tag landing2 --arms near-4k:before,near-4k:after,near-4k:before:17F,near-4k:after:17F`. Other variants still launch `CELL_BIN`.
- `under` is a pose a metre below e24's still pool, looking up at the surface's underside. `near-auto`, `test-auto` and `mid-auto` are the `near`/`test`/`mid` poses at 1536×960, the 5300M's Auto resolution.

## Glossy world reflections (checkpoint 2)

Rows M5 and M6 of `context/plans/done/glossy-world-reflections/`. Configs `glossy-{across,down}-{auto,1440,4k}` on `glossy-reflections-test`: `across` stands at the spawn at eye height looking at the monument (floor about 55% of the frame), `down` looks straight down at open floor. A fourth arm field sets the capture's `reflection_quality` (`glossy-across-4k:after:17F:off`); glossy arms need mask `17F`, since the overlay draws only under the whole mask. `MAP_ROOT_BEFORE` / `MAP_ROOT_AFTER` name a `content/<mod>/maps` directory holding each binary's own `prl-build --no-cache` output (the after glossy map with `--glossy-reflections`); give it a sibling `baked/` through `--baked-root` and symlink the rest of `content/dev`, so the engine's grandparent derivation finds the sidecars. `--report glossy` writes overlay, sums, forward, whole-frame, pass sum and overlap; its clock column reads busy samples only. `--abort-invalid` stops a pilot at its first invalid run. Results: `../results-glossy-m5m6.md` (pilot: `../results-glossy-pilot.md`).

### Floor-only ceiling fixture (Task 6 on)

The owner-approved overlay and forward ceilings (`plan.md`, checkpoint 2) measure per-pixel cost on `maps/glossy-floor-only.map`: the Task 5 `glossy-reflections-test` map with its north wall's inner face retextured matte (`50-free-textures/concrete_stone_021`), so only the 7 floor faces reflect once walls gain probes. Copy it into each binary's `MAP_ROOT_*` as `glossy-reflections-test.map` and bake it with that binary's own `prl-build --no-cache --glossy-reflections`; the `glossy-*` poses then see Task 5's glossy pixels. The growing dev map is measured as a reported baseline, not against the ceilings. Results: `../results-glossy-task6-flooronly.md` (ceilings), `../results-glossy-task6-current.md` (baseline), `../results-glossy-task6.md` (Task 5 map rebaked, wall reflecting).

### Floor layer (Task 11 on)

`--report glossy` also reports `chain` (every `Glossy Floor Chain Level` pass in the frame, summed: timing pair 24), `march` (`Glossy Floor March Pass`, pair 25) and `guard` (`Glossy Mover Guard Pass`, pair 26, on frames with a mover draw). A glossy arm at a floor tier (`medium`, `high`, `very_high`, `ultra`, or no quality field) also requires one floor march per complete frame, and a `low` arm requires the overlay, which draws the probe term alone there. On floor-tier frames the trace attributes part of the Textured Pass's time to the first chain level, so read forward and chain together (`../results-glossy-task11-diag.md`). Results: `../results-glossy-task11-flooronly.md` (ceilings, attribution), `../results-glossy-task11-tiers.md` (per tier, proposed ceilings), `../results-glossy-task11-current.md` (baseline).

### Parity A/B and M6–M8 (Task 12b)

`maps/glossy-floor-mover.map` is the floor-only fixture plus one parked kinematic mover on the glossy floor (row M7); configs `glossy-mover-across-{auto,1440,4k}` read it, and their complete frames require the `Kinematic Brush Pass`, reported in the `mover` column. An optional fifth arm field sets the capture's `specular_style` (`glossy-mover-across-4k:after:17F:high:smooth`; an empty fourth field keeps the default quality). `XCTRACE_GRACE` (seconds, default 180) is how long xctrace may keep saving after the engine's report before the run is interrupted; HEAD at a floor tier with a mover on screen needs more than 180 (Task 12b ran 900). Results: `../results-glossy-task12-m6.md`, `../results-glossy-task12-m7.md`, `../results-glossy-task12-m8.md`, `../results-glossy-task12-parity.md`.
