# probes.patch

`git format-patch --stdout e4cfe8a12..1a052cfed` on `sh-compose-row-cost-spike-probes`: 7 commits, 75,377 bytes. The base `e4cfe8a12` ("plan of record") is the `git merge-base` with `sh-compose-row-cost-spike`. The probes are throwaway, but these pieces are reusable:

- **`spike_dump_sh_atlases`** (`compose_spike.rs`, `impl Renderer`; called from `capture/prepared.rs` and `capture/driver.rs`) copies the streamed composed indirect and direct atlases to `{indirect,direct}.bin` plus `dims.json`. Use it to prove a lever's output is byte-identical to baseline.
- **`ComposePipelines`** (`compose_spike.rs`; used in `gpu/indirect.rs` and `direct_compose/passes.rs`) builds the A pipeline plus an optional B twin (`POSTRETRO_SPIKE_ARMS_B`) and alternates them per dispatch. The result is a paired A/B in one launch, with shared clocks, thermals and memory placement.
- **`RowCountWindow`** (`compose_spike.rs`; held in `sh_streaming.rs`, fed from `frame.rs`) logs per-level row counts, rows with entries, and lane-weighted entries.
- **`repack_texel_major`** (`compose_spike.rs`; used in `animated_runtime.rs`) repacks section-45 delta tiles texel-major at upload for the `coalesced-b` layout. A unit test checks it against the coalesced addressing.
- The arm rewrite layer (`build_source`, `apply_arm`, `replace_n`) is spike scaffolding. Its anchor-count asserts are the pattern to keep if it is ever reused.

`git diff --stat e4cfe8a12..1a052cfed`: 14 files changed, 1098 insertions(+), 61 deletions(-). All under `crates/` and no `.wgsl` touched:

```
renderer/src/render/compose_spike.rs 999+ | sh_compose.rs 10± | animated_direct_sh_compose.rs 10± | mod.rs 1+
renderer/src/render/sh_streaming.rs 10+ | sh_streaming/{frame.rs 10+, setup.rs 1+, tests.rs 1+}
sh_streaming/gpu/indirect.rs 28± | gpu/indirect/runtime.rs 9± | direct_compose/passes.rs 32± | passes/animated_runtime.rs 36±
postretro/src/capture/driver.rs 2+ | capture/prepared.rs 10+
```
