# External acceptance runbook

Rows 34–37 block landing. Record results in plan.md; do not infer a visual or RSS pass from unit tests.

## Compiler measurements (34–35)

Use the same machine, worker count, release profile and flags for baseline and implementation. Record machine/GPU class and RAM, commit, workers, cache mode, density and full command. Baseline is 45c4fe5 in an isolated checkout with its own target directory. Its compiler and script sidecar must both come from that checkout.

In each checkout:

```sh
cargo build --release -p postretro-level-compiler -p postretro-script-compiler
```

Create a session-owned output directory outside the checkout and retain its absolute path as `proof_root`. From the implementation checkout, run at both 0.025 and 0.04; from baseline, run at 0.04:

```sh
RUST_LOG=info target/release/prl-build \
  content/dev/maps/stress-warren-hallway-inspection.map \
  --lightmap-density 0.025 --no-cache --no-tui -j 4 -v \
  --baked-root "$proof_root/baked" -o "$proof_root/hallway-0025.prl"
```

For 0.04, replace the density and output filename (`hallway-0040.prl`). Give baseline and implementation separate output directories. Keep default soft-shadow samples, probe spacing and working-set budget on both; do not override an upstream limit to claim the target run passed.

Sample the compiler PID's RSS at 100 ms intervals, starting at the plain reporter's `Lightmap bake...` marker and ending at `fused lightmap/shadowmask stage returned`. Record the maximum sample, sample count and stage elapsed time. This is the process RSS high-water sample **within the lightmap stage**, including earlier outputs still resident. A whole-process peak from `time -l` cannot substitute: earlier SH stages set it.

Record for implementation 0.025: successful exit, packed bake layers, runtime pool layers (verbose `Lightmap runtime all-resident pool` line), exact predicted bytes (admission line), sampled stage peak and total wall time. Name SH/delta stages as the upstream limit on total wall time and process RSS. For 0.04, the implementation stage peak must not exceed the same measurement at baseline.

## Engine observations (36–37)

From the implementation checkout, normal streaming:

```sh
env -u POSTRETRO_LIGHTMAP_STREAMING RUST_LOG=info \
  cargo run -p xtask -- run --features dev-tools -- \
  "$proof_root/hallway-0025.prl" --mod dev --baked-root "$proof_root/baked"
```

Confirm lightmaps render and no neutral-placeholder rejection appears. Record the observation and any capture.

If the 0.025 compile warned that its all-resident pool exceeds the runtime floor, load the same output with:

```sh
RUST_LOG=info POSTRETRO_LIGHTMAP_STREAMING=all-resident \
  cargo run -p xtask -- run --features dev-tools -- \
  "$proof_root/hallway-0025.prl" --mod dev --baked-root "$proof_root/baked"
```

Confirm the placeholder renders, and the renderer rejection names exactly the pool-layer count in the compiler warning. If this map did not warn, use a fixture whose runtime pool count exceeds 256; a bake-layer count alone is insufficient.

Delete the session-owned outputs, caches, measurement traces and temporary checkout/target after recording the evidence. Keep source checkouts and unrelated artifacts intact.
