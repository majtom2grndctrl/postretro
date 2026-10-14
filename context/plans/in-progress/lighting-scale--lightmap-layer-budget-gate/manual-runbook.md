# External acceptance runbook

All checks below pass. Compiler measurements are recorded in measurement-results.md; owner normal-streaming and forced-all-resident visual/log observations are recorded in plan.md. Commands remain here for reproduction.

## Compiler measurements (34–35)

Use the same machine, worker count, release profile and flags for baseline and implementation. Record machine/GPU class and RAM, commit, workers, cache mode, density and full command. Baseline is 45c4fe5 in an isolated checkout with its own target directory. Its compiler and script sidecar must both come from that checkout.

In each checkout:

```sh
cargo build --release -p postretro-level-compiler -p postretro-script-compiler
```

Owner requested PRLs directly in the main checkout's `content/dev/maps`. Use absolute output paths there, even for the isolated baseline checkout. Keep the current 0.025 output at `stress-warren-hallway-inspection.prl`; name the current comparison `stress-warren-hallway-inspection-0040.prl` and baseline comparison `stress-warren-hallway-inspection-baseline-0040.prl`. Use the main checkout's absolute `baked` path for sidecars so the dev engine reads them normally. Evidence directory for this run: `/private/tmp/postretro-lightmap-proof-ztdf8vxi/evidence`. From the implementation checkout, run at both 0.025 and 0.04; from baseline, run at 0.04:

```sh
RUST_LOG=info target/release/prl-build \
  content/dev/maps/stress-warren-hallway-inspection.map \
  --lightmap-density 0.025 --no-cache --no-tui -j 4 -v \
  --baked-root /Users/dhiester/Projects/Personal/postretro/baked \
  -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection.prl
```

For 0.04, replace the density and use the named current/baseline comparison filename above. Keep default soft-shadow samples, probe spacing and working-set budget on both; do not override an upstream limit to claim the target run passed.

Sample the compiler PID's RSS at 100 ms intervals, starting at the plain reporter's `Lightmap bake...` marker and ending at `fused lightmap/shadowmask stage returned`. Record the maximum sample, sample count and stage elapsed time. This is the process RSS high-water sample **within the lightmap stage**, including earlier outputs still resident. A whole-process peak from `time -l` cannot substitute: another stage may set it. The completed 0.025 run peaked during animated light weight maps.

Completed results: [measurement-results.md](./measurement-results.md).

Record for implementation 0.025: successful exit, packed bake layers, runtime pool layers (verbose `Lightmap runtime all-resident pool` line), exact predicted bytes (admission line), sampled stage peak and total wall time. Record SH/delta upstream cost separately from the actual whole-process peak. For 0.04, the implementation stage peak must not exceed the same measurement at baseline.

## Engine observations (36–37)

From the implementation checkout, normal streaming:

```sh
env -u POSTRETRO_LIGHTMAP_STREAMING RUST_LOG=info \
  cargo run -p xtask -- run --features dev-tools -- \
  content/dev/maps/stress-warren-hallway-inspection.prl --mod dev --baked-root baked
```

Confirm lightmaps render and no neutral-placeholder rejection appears. Record the observation and any capture.

The completed 0.025 hallway uses 230 runtime layers and does not warn. For AC37, use `lightmap-runtime-floor-verification.prl`, baked from the adjacent map at density 0.0006. Its successful final bake reports 340 runtime pool layers and the expected warning.

```sh
RUST_LOG=info POSTRETRO_LIGHTMAP_STREAMING=all-resident \
  cargo run -p xtask -- run --features dev-tools -- \
  content/dev/maps/lightmap-runtime-floor-verification.prl --mod dev --baked-root baked
```

Confirm the level renders with the neutral placeholder bound. The renderer must log `Lightmap cell blocks rejected`, name 340 required pool layers against the 256-layer device limit, and say it is degrading to the neutral placeholder. Record the observation and any capture. SDF direct lighting can remain visible, so brightness alone does not prove the placeholder path; include the rejection log and lightmap memory meter when available.

Retain the requested dev-folder PRLs for owner inspection. After recording the evidence, remove the session measurement traces and temporary baseline source/target; keep source checkouts, dev outputs and unrelated artifacts intact.
