# Fluid base-pass cost: Metal ablation kit (5300M)

Attributes the reflection-free Fluid Surface Pass's cost on the Radeon Pro 5300M by timing ablated copies of its shader. Findings: `context/plans/drafts/fluid-base-pass-cost/research.md`.

## How it works

- `run.py` and `gpu_time.py` are copies of `measurements/cell-rate-spike/metal/` (branch `claude/cell-rate-spike`): Metal System Trace recording, watchdog, clock polling, machine-state checks, complete-frame reduction, trace deletion. `run_ablation.py` drives them with its own arms.
- `variants.py` writes whole fluid shader modules into `variants/`, each with one textual edit to `fs_main` (or `vs_main`). Every edit must match exactly once.
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
  --tag <name> --arms near-4k:base,near-4k:vertexsh,test-4k:base
```

Arms are `<config>:<variant>[:<mask hex>]`; configs and poses are `run.py`'s (`near` = e24 still pool straight down, `test` = probes-map pool straight down). Maps come from `prl-build --no-cache` (see the cell-rate kit's README). Defaults: 120 warmup + 120 sampled frames, 2 interleaved rounds, 15 s idle gap. Results go to `../results-<tag>.{md,json}`; per-run records to `runs/`.

`variants.py` also writes two image-only diagnostics, `shpath` and `shpathfloor`, which colour each water pixel by the SH sampler path it takes (at the surface, and at the scene point behind it).
