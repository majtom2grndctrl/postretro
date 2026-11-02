# Results — surface-radiance-cache measurements (Task 8, AC 35 and 36)

Report, not gate. Raw Build Summaries and stage-log excerpts: `results/raw/`; all numbers machine-readable in `results/results.json`.

## Conditions

- Machine: Intel(R) Core(TM) i9-9980HK @ 2.40 GHz, 16 logical cores, macOS (Darwin 25.6.0). `prl-build` default workers: 14 (`-j` default printed by `--help`).
- Cold = `prl-build <map> --release --no-tui` (no cache), `RUST_LOG=info`, wrapped in `caffeinate -dims`, `--baked-root` pointed at a scratch dir. One bake at a time; `pgrep -fl 'cargo|prl-build|rustc'` was clean before every run (recorded as `pgrep-check: clean` in each `*.time.txt`).
- Before: `prl-build-base`, cargo `--release` build of base commit d2bda6032. After: `target/release/prl-build` rebuilt at HEAD 194113525 (`cargo build --release -p postretro-level-compiler`). Same flags for both. `scripts-build` (current HEAD build) sat beside the base binary so its data-script stage could run.
- Pilot: hallway-mini with the after binary ran end to end first (valid Build Summary), then the batch. No run in the batch failed except the one noted below.
- Not controlled: thermals (Intel Mac slowdown investigation is open). Wall times per row look stable (campaign-test pairs within 1 s), so no regime shift was seen, but no temperature was logged.

## AC 36 — cold Build Summary, before -> after (seconds)

| Map | Run | SH Bake before | SH Bake after | ratio | Delta SH before -> after | Lightmap before -> after | Total before -> after |
|---|---|---|---|---|---|---|---|
| campaign-test | 1 | 1.64 | 4.36 | 2.66x | 0.49 -> 0.51 | 11.29 -> 11.73 | 24.34 -> 26.80 |
| campaign-test | 2 | 1.74 | 5.12 | 2.94x | 0.50 -> 0.66 | 11.91 -> 11.72 | 24.21 -> 27.36 |
| stress-warren-mini | 1 | 25.14 | 23.22 | 0.92x | 6.05 -> 6.32 | 208.77 -> 210.41 | 290.04 -> 289.75 |
| stress-warren-hallway-inspection-mini | 1 | 74.81 | 49.90 | 0.67x | 0.00 -> 0.00 | 340.32 -> 345.36 | 445.20 -> 426.66 |
| full hallway (`stress-warren-hallway-inspection`) | 1 | 267.49 | 119.00 | 0.44x | 15.93 -> 15.70 | 865.96 -> 852.64 | 1235.83 -> 1072.47 |

The after binary is slower on `campaign-test` (about 2.7-2.9x on SH Bake, +2.5-3 s total) and faster as the map grows: hallway-mini loses a third of SH Bake. campaign-test has 11 static lights (small fixed cost: patch set, layers, later bounces and gather do not shrink with a small map); hallway-mini has 274 static lights and 915,467 patches. The Lightmap Bake column is flat within noise (the change does not touch it). Peak RSS: ~4.1 GB for mini and hallway-mini on both binaries, ~1.2 GB for campaign-test.

## AC 35 — light-count sweep

Fixture: `gen_light_sweep.py` — one sealed 8192 x 8192 x 256 hall, 6x6 pillar field, lights tiled on a grid at z = 216, each `light 150`, `_falloff_range 768`, white-warm. Identical geometry at every count (402,368 patches, same probes); only the light grid changes. N = 16, 2N = 32, 4N = 64; the extra 8N and 16N rows are an extension. Textures are NOT resolved for this fixture (map lives outside a `content/<mod>/maps` layout; log says `cannot read texture root`), so surfaces use the default bounce albedo. That is the same for both binaries.

Cold, `--release`, seconds:

| Lights | After: SH Bake | After: patch-direct (plateau) | After: later bounces + gather | After: total | Before: SH Bake | Before: total |
|---|---|---|---|---|---|---|
| 16 (N) | 14.39 | 8.57 | 5.82 | 62.41 | 4.64 | 52.33 |
| 32 (2N) | 14.21 | 8.06 | 6.15 | 73.83 | 6.11 | 62.92 |
| 64 (4N) | 14.13 | 8.05 | 6.08 | 98.60 | 9.55 | 91.91 |
| 128 (8N) | 13.85 | 8.06 | 5.79 | 128.02 | 16.52 | 130.29 |
| 256 (16N) | 14.57 | 8.56 | 6.01 | 180.96 | 31.15 * | failed * |

\* Before binary at 256 lights failed after its SH stage with `SH delta payload cap exceeded before packing: id 41 328752864 bytes ... cap 268435456` (the id-41 delta payload, a stage this change leaves alone but whose size depends on the base bake's output; the after binary's id 41 was 264,907,800 bytes, just under the cap). SH Bake 31.15 s read from stage-begin timestamps.

Reading: after-binary SH Bake is flat in light count (13.9-14.6 s across a 16x range); the before binary's estimator grows roughly linearly (4.6 s -> 31 s) and crosses over between 64 and 128 lights on this fixture. The after binary pays a fixed ~14 s on this fixture (402k patches) that the old estimator did not, so it loses below ~64 lights here. Total build time is dominated by Lightmap Bake, which scales with lights in both binaries.

### What the split is, and what is missing

`surface_radiance` logs no timed line. The only per-phase signal is the SH Bake progress bar in stderr, which I timestamped (`ts.py`, `phases.py`). It advances a fraction of a percent while patch layers shade, then jumps to 100% in one smooth ramp. I report the plateau as "patch-direct" and the ramp as "later bounces + gather". Accuracy about +-0.5 s (progress samples every 0.5 s). Caveats:

- Later bounces and gather are NOT separated: the ramp is smooth, no timed line splits them.
- The "plateau" includes any setup before shading (patch set build, gate) as well as per-layer shading, so it cannot show patch-direct growth with lights. It stayed at 8.1-8.6 s from 16 to 256 lights; since per-light layer work grows by construction (gate line: layer bytes 56 MB at 16 lights, 97 MB at 64, 248 MB at 256), the per-light shading here is small against a fixed ~8 s of setup + the per-patch work. A timed line per phase would be needed to say more. I added no code.
- Before-binary phase columns are meaningless (different estimator) and omitted.

Gate-line predictions (after binary): peak 385 MB at every count (the probe-gather term dominates); layers 56 -> 97 -> 248 MB at 16 / 64 / 256 lights.

## Warm one-light edit — small-scale pilot (campaign-test, not an AC row)

`warm-edit.sh`: cold fill of a private cache, +10% intensity on the first static point light, warm rebuild. SH Bake seconds:

| Binary | Build A (empty cache) | Build B (one light edited) | Cache outcome in B |
|---|---|---|---|
| before | 2.73 | 1.20 | `sh_group` 5052 hit / 1560 miss |
| after | 4.21 | 4.38 | `sh_patch_layer` 10 hit / 1 miss; `sh_base_volume` miss |

After-binary B phases: plateau 3.05 s, ramp 1.33 s. As designed, the warm edit still re-gathers the whole volume, so warm SH Bake is about the cold cost; the old warm build was faster because it re-baked only the edited light's groups (and shipped approximate bytes). The full-hallway comparison (AC 34) is the owner's run.

## Full hallway, 2026-10-31 (this Mac)

- Cold `--release`, one bake at a time, 5-minute cool-down, `pmset -g therm` CPU speed limit 100 at each start (`results/raw/hallway/gate.txt`). SH Bake 267.5 → 119.0 s (2.25× faster), total 1235.8 → 1072.5 s. Peak memory footprint 4.44 → 4.39 GB.
- The SH base gate admitted 1,747,036 patches and 757 static lights, predicting a 4.98 GB peak (the layer phase), against the 16 GiB default (`results/raw/hallway/sh-base-gate.txt`).
- **AC 34 / AC 37 (warm) not measured.** The base binary's warm build A filled its private cache to 28 GB on disk (26.57 GiB spared set, 277,515 entries, against the 2 GiB `--cache-max-size`; the cache keeps a live map's spared set). That took the volume to 3 GB free and the build ended without a Build Summary, so the batch aborted. The cache was deleted at once. This cache size is a property of the existing warm stages on the full hallway, not of this branch, and a warm pair needs about 2 × 28 GB plus headroom. Run the warm pair on a machine with more free disk, or one binary at a time, deleting each cache before the next.
