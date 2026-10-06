# Measurements — sh-compose-row-cost-spike

Brief: `context/plans/in-progress/sh-compose-row-cost-spike/index.md`. Findings: `findings.md` beside it.

## Pins

**Machine.** 16-inch MacBook Pro: Intel Core i9-9980HK, 32 GB RAM, AMD Radeon Pro 5300M (4 GB, Metal) beside an Intel UHD 630. macOS 26. GPU timestamps are unsupported, so pass time comes from Metal System Trace.

**Display and settings.** `~/Library/Application Support/postretro/settings.toml` is unchanged across all runs and matches shadow-fill-cost:
- window mode `exclusive` at 2688×1680, 60 Hz, render resolution `half`;
- shadow and fog quality `low`, Surface Depth `off`;
- vsync on (engine default).

**Build.** Every arm runs from one release binary built from the probes branch (`sh-compose-row-cost-spike-probes`): `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler --features postretro/capture`. `POSTRETRO_SPIKE_ARMS` selects arms at pipeline creation. Each batch manifest records the probe commit and the binary's SHA-256.

**Fixtures** (gitignored PRLs, symlinked from the main checkout; SHA-256 prefixes):
- `stress-warren-hallway-inspection.prl` `a543d0e0291dbceb`
- `campaign-test.prl` `2470294d8cb54730` (re-baked 2026-10-05; shadow-fill-cost read `574be03376fdb9cf`)
- `kinematic-platform.prl` `8939e012bc683c0c`

## Protocol (`batch.sh`)

- `caffeinate -d -i -u` is held for the batch, and user activity is re-declared every few seconds.
- There is a 15 s idle gap before every launch. Arms are interleaved round by round, all inside one session.
- Every run is one Metal System Trace of 4 s, taken after 3 `[CpuTiming]` windows. Each trace is exported, reduced with `gpu_time.py`, and deleted at once, together with its `instruments*.ktrace` temp file.
- A run is valid only if all of these hold:
  - it stays in the foreground, unlocked, with no screen saver;
  - its `[SH spike counts]` windows after the first hold `min == max` rows and an unchanged per-level mix;
  - it logs its arm list.
- The batch stops at the first machine-state failure.
- An idle `ioreg -c IOAccelerator` snapshot is taken per batch (`runs/<batch>-idle.ioreg.txt`).
- Metric: labelled pass time per camera frame (`Streamed SH Compose`, `Streamed Animated Direct SH`), as `gpu_time.py` reduces it. `summarize.py` reports each arm's median and its min..max spread.

## Byte identity (`capture.sh`, `compare_atlas.py`)

- `capture.sh <arm>` runs every scene in `capture/` under `POSTRETRO_SH_STREAMING=sync-proof`. It dumps the composed `rgba16float` indirect and animated-direct atlases (`POSTRETRO_SPIKE_ATLAS_DUMP`).
- Scenes: three views (stress-warren-mini animroom, the hallway arena, kinematic station), each gated and force-full-resident, at stepped times t = 0.5 s and 1.0 s.
- `compare_atlas.py <baseline> <arm>` compares the raw bytes. On a mismatch it reports the differing texels and the maximum absolute RGB deviation.

## Arms

| Arm | Kind | Rewrite |
|---|---|---|
| `baseline` | — | none |
| `floor` | floor | entry loops never run (`end = start`) |
| `no-base` | ablation | skip the base / intermediate atlas read |
| `stores-off` | ablation | store loop behind a never-true guard |
| `accum-scalar` | ablation (H-a) | one-element accumulator; every texel aliases it |
| `const-scale` | ablation (H-b) | `animated_light_scale` returns a constant |
| `rank0` | ablation (H-c) | every lane reads probe rank 0's delta |
| `skip-coarse` | ablation (H-d) | L1/L2 entry loop never runs |
| `single-slot` | ablation (H-d) | L1 reconstruction reads one slot |
| `scan-parallel` | lever (fixed term) | lane 0's 64-candidate scan becomes a workgroup `atomicMin` over the lanes' own words |
| `scale-shared` | lever (H-b) | one lane per CSR entry evaluates the scale into workgroup memory (≤ 64 entries per row, else the old path) |
| `texel-outer` | lever (H-a) | L0 rows fuse the base read, entry sum and store per texel; implies `scale-shared` |
| `vec3-accum` | lever (H-a, Pass B) | Pass B's accumulator drops its dead alpha |
| `const-tile` | lever (H-e-adjacent, safe) | `read_delta_texel` uses the PRL-pinned 6×6 RGB16F tile instead of runtime divisions |

The `trusted` arm, which turns off wgpu's injected checks with `create_shader_module_trusted`, was not built. The session's auto-mode permission classifier refused the `unsafe` call site, so its timing waits on the owner (see `findings.md`). The static MSL gate is in `he-gate/`.

Stacked arms are written `a+b` in batch arm lists and become `POSTRETRO_SPIKE_ARMS=a,b`.
