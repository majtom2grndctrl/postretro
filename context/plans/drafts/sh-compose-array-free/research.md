# sh-compose-array-free — research

Evidence comes from the `sh-compose-row-cost-spike` findings note (`context/plans/*/sh-compose-row-cost-spike/findings.md`) and its records in `measurements/sh-compose-row-cost-spike/`. Both land on main with the spike's branch. This brief's build reads them there.

## Measured lever
- The arm set is `array-free,unroll36`; `array-free` implies `scale-shared`. Its exported source is in `measurements/…/lever-wgsl/`.
  - `array-free`: per texel, the kernel reads the base (indirect: the static base when on; Pass B: the intermediate atlas), adds each entry's delta × scale, and stores. It keeps no 36-entry array and has no barrier after the scale cache.
  - L1/L2 lanes reconstruct from their own reads of the kept corner tiles. That replaces the shared-lattice workgroup path.
- `unroll36` replaces the texel loop with 36 blocks with constant `(x, y)`. It drops the loop-bound counter and the index math that naga emits per iteration.
- `scale-shared`: lanes cache light scales in workgroup memory when a row has at most 64 entries. Above that, each lane evaluates the scale per entry (the fallback). No fixture has exercised the fallback.
- The source was shown byte-identical in 12 capture scenes: 3 views × gated and full-resident × 2 times, covering 9 distinct indirect states and 8 distinct direct states. The capture oracle test passed. The negative control (`floor`) differed in all 12 scenes.

## Exported lever structure
The `compose_main` / `animated_compose_main` code is laid out in this order:
- the preamble, with an early return when `workgroup.x >= grid.row_count`;
- the scale-cache fill and its barriers;
- `if (grid.row_count > 0u) { 36 unrolled texel blocks; return; }`;
- the entire old kernel, unreachable.

The old kernel still holds the 36-entry accumulator array and the 288-vec4 `shared_kept_tiles` / `shared_kept_present` workgroup lattice. The compiler cannot prove the wrapper always true, so the tail may still claim that workgroup memory and registers. Its effect on the measured times is unknown, which is why deleting it is measured before it lands. The L1 helpers that only the tail uses are `reconstruct_l1_shared_texel` and `l1_shared_slot`; the lever's own L1 path is `spike_reconstruct_l1`.

## Fragility
On Metal, arms that *remove* indirect work made indirect 0.9–2.7 ms slower: `const-scale`, `rank0`, `scale-shared` alone, `const-tile`, `scan-parallel`, `texel-outer`, `skip-coarse`. Pass B moved the expected way or not at all. The individual indirect deltas of a four-lever mix summed to +1.06 ms, but the mix measured −1.27 ms. This fits a kernel sitting at a register-allocation threshold. Register statistics are unavailable on AMD/Metal, so it is unconfirmed. Consequence: an equivalent-looking source can land on the wrong side of that threshold.

## Targets (Mac, paired Δ, per compose encoder)
| Pose | Rows | Spike stacked Δ ind / B (ms) | Per row ind / B (µs) | 75% floor ind / B (µs/row) |
|---|---|---|---|---|
| Hallway arena | 2129 | −2.23 / −2.89 | 1.05 / 1.36 | 0.79 / 1.02 |
| Kinematic station | 2511 | −2.13 / −3.07 | 0.85 / 1.22 | 0.64 / 0.92 |

Projected through the contributing-row filter (spike model; rows drop to the entry-carrying set), the lever saves, in ms per frame ind / B:
- arena −0.07 / −0.10;
- campaign spawn −0.37 / −0.47;
- kinematic spawn and station −0.66 / −0.09.

The A/A null read −0.01 to +0.02 ms. Pre-change baselines (ms per frame, ind / B): arena 6.71 / 8.00, station 8.00 / 9.36. If the filter has landed first, compare per row: at the arena the ms deltas sit near the null band.

## No-builds
All are byte-identical. None earns a build.
- `texel-outer`, `scale-shared` alone, `scan-parallel` and `const-tile` regress indirect.
- `vec3-accum` has no effect.
- `coalesced-b` (texel-major delta repack) gains nothing.
- `l0-only` gains nothing on top of `array-free`.

## Format facts (ids 27/45)
- `Level::from_u8` accepts 0..=2 in each section's `from_bytes`, for ids 27, 41 and 45.
- `enforce_id41_only_coarsening_policy` (level-compiler `delta_sections.rs`) fills `cell_levels` with L0 for 27 and 45. `pipeline.rs` calls it unconditionally.
- CPU L1/L2 reconstruction is `render-cpu::sh_compose::reconstruct_delta_probe_tile`, covered by CPU tests. No GPU test runs either compose entry point.

## Measurement tooling
- The spike's `gpu_time.py` takes the union of each compose encoder's intervals. The shadow-fill-cost copy derives frames from render-encoder counts, which undercount by up to 26%. Use the spike's copy.
- `probes.patch` carries the paired A/B pipelines (`ComposePipelines`), `RowCountWindow` and the atlas dump. The patch base is `e4cfe8a12`. The shaders are unchanged between that commit and b9d0a4645.
