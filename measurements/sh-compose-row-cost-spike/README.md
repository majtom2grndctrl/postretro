# Measurements — sh-compose-row-cost-spike

The brief is `context/plans/done/sh-compose-row-cost-spike/index.md`, and the findings are `findings.md` beside it.

## Pins

**Machine.** 16-inch MacBook Pro: Intel Core i9-9980HK, 32 GB RAM, AMD Radeon Pro 5300M (4 GB, Metal) beside an Intel UHD 630. macOS 26. GPU timestamps are unsupported, so pass time comes from Metal System Trace.

**Display and settings.** `~/Library/Application Support/postretro/settings.toml` was unchanged across all runs, and matches shadow-fill-cost:
- window mode `exclusive` at 2688×1680, 60 Hz, render resolution `half`;
- shadow and fog quality `low`, Surface Depth `off`, vsync on.

**Build.** Every batch runs from one release binary of the probes branch (`sh-compose-row-cost-spike-probes`), built with:

```
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler --features postretro/capture
```

- Each build is copied to `scratchpad/bin/probe-<commit>/`, beside its `scripts-build`.
- Batches differ in probe commit. `batches.json` (from `manifest.py`) records each batch's commit, the binary's SHA-256 and the fixture prefixes.
- `POSTRETRO_SPIKE_ARMS` (A) and `POSTRETRO_SPIKE_ARMS_B` (paired B) select arms at pipeline creation.

**Worktree content.**
- The gitignored PRLs, `content/dev/**/*.js` and `content/base/` come from the main checkout: the PRLs are symlinked and the rest copied.
- `baked/` is symlinked. Batch `arena1` ran before that link, so it had placeholder material textures.

**Fixtures** (SHA-256 prefixes):
- timed: `stress-warren-hallway-inspection.prl` `a543d0e0291dbceb`, `campaign-test.prl` `2470294d8cb54730`, `kinematic-platform.prl` `8939e012bc683c0c`;
- capture scenes and row-count probes only: `stress-warren-mini.prl` `d3a3610b57793890`, `campaign-test-sh-fidelity-{1,7}.prl`.

## Protocol

- **Batches (`batch.sh`).**
  - `caffeinate -d -i -u` is held, and user activity is re-declared every few seconds.
  - There is a 15 s idle gap before every launch. Arms are interleaved round by round.
  - Each run is one 4 s Metal System Trace, taken after 3 `[CpuTiming]` windows. The trace is exported, reduced by `gpu_time.py`, gzipped to the ignored `raw/` and deleted at once, together with its `instruments*.ktrace` temp file.
  - An idle `ioreg -c IOAccelerator` snapshot is taken per batch. The batch stops at the first machine-state failure.
- **Valid run (`run.py`).** It stays in the foreground, unlocked, with no screen saver. Its `[SH spike counts]` windows after the first hold `min == max` rows and an unchanged per-level mix. It logs its arms. GPU clock, temperature and power are sampled through the trace.
- **Metric: time per compose pass encoder** (`compose_per_encoder_ms` in each `runs/*-gpu.json`).
  - Every frame dispatches each compose pass once, as one encoder.
  - `gpu_time.py` takes the union of an encoder's nested intervals and counts a coalesced row once.
  - Per-frame values divide by the indirect compose encoder count.
- **Every reported delta is paired (`PAIRED=1`).**
  - B arm pipelines alternate with A's frame by frame inside one launch. B passes are labelled `… [B]`.
  - A bare arm pairs against the baseline. `a/b` pairs arm a against arm b; for example, `floor/floor+no-base` splits the floor.
  - `summarize_paired.py` writes `paired-<batch>.json`: per run A, B and Δ; per arm the median Δ and min..max.
  - The A/A pair reads within ±0.02 ms.
- **`summarize.py`** is the between-launch summary. It is kept only for `arena1`, `arena3` and `diag1`.
- **Cost model.** `fit_model.py` → `model.json`, with the bootstrap spread. Its counts come from the `runs/counts-*` probes.
- **Byte identity.** `capture.sh <arm>[@tag]` runs the 12 `capture/*.scene.json` scenes under `POSTRETRO_SH_STREAMING=sync-proof`. It dumps both composed `rgba16float` atlases (`POSTRETRO_SPIKE_ATLAS_DUMP`), compares them with `capture/out/baseline/` (`compare_atlas.py` → `compare.json`), and deletes the dumps of about 3.6 GB per arm. Oracle-test runs are in `capture/oracle.txt`.
- **Counters.** `counters.py` averages Metal GPU Counters over compose intervals (runs `counters-*`). It turned out to be non-discriminating.
- **H-e gate.** `he-gate/` holds the static MSL check count.

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
| `array-free` | lever (H-a/H-d) | every level fuses read, sum and store per texel. L1/L2 lanes reconstruct from their own reads of the kept corners. No array, no lattice. Implies `scale-shared`. |
| `unroll36` | lever modifier (H-e) | `array-free`'s texel loop is emitted unrolled |
| `l0-only` | lever modifier (H-d) | `array-free`'s coarse branch is compiled out (exact only for L0 content) |
| `vec3-accum` | lever (H-a, Pass B) | Pass B's accumulator drops its dead alpha |
| `const-tile` | lever (H-e) | `read_delta_texel` uses the PRL-pinned 6×6 RGB16F tile instead of runtime divisions |
| `coalesced-b` | lever (H-c, Pass B) | section-45 tiles are repacked texel-major at upload and addressed that way; A and B must both carry it |

The `trusted` arm (wgpu checks off) was never built: the session's permission classifier refused its `unsafe` call site. See `findings.md`.

## 1660 Super reading (`1660/`)

The F8 handoff ran on the owner's Windows machine (GTX 1660 Super, Vulkan). Results are in `findings.md` §1660 Super reading.

- **Build.** The same command and probe commit (`1a052cfed`) as above. `1660/batches.json` records the binary SHA-256s, fixture prefixes, adapter, driver and settings.
- **Metric.** GPU timestamps (`POSTRETRO_GPU_TIMING=1`), not Metal System Trace. A launch's value is the median of 8 `[gpu-timing]` windows of 120 readbacks, after 3 warm-up windows. One arm per launch, unpaired.
- **`run1660.py`** interleaves arms round by round with a 10 s idle gap. It records each launch's kept windows, its `[SH spike counts]` lines and per-second `nvidia-smi` clocks to `runs/*.run.json`. Its paths are this machine's. Full logs are ignored.
- **`summarize1660.py`** checks row stability and writes `summary-*.json`: per arm the median and range over launches, and per-round deltas against baseline.
