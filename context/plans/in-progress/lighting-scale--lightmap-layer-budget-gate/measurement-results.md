# Dev-map verification measurements

AC34–35 pass. AC36–37 remain owner visual checks. All four PRLs below were published successfully in `content/dev/maps` and are retained at the owner's request.

## Conditions

- Current: `80ca42a7a155c45340ca5ef96c48d7d0740fbf25`; baseline: `45c4fe5`.
- MacBookPro16,1; 32 GiB RAM; 8 physical / 16 logical CPU cores; AMD Radeon Pro 5300M Metal.
- Release compiler and script helper from each corresponding source snapshot; rustc 1.99.0 (b940084d7 2026-09-28).
- Four workers; `--no-cache --no-tui -j 4 -v`; default shadow samples, probe spacing and working-set budget. Main checkout's absolute `baked` root. No concurrent heavy builds or bakes.
- Hallway map and generated script match the baseline SHA-256 digests below.
- Compiler process RSS sampled every 100 ms. Lightmap interval starts at `Lightmap bake...` and ends at `fused lightmap/shadowmask stage returned`. Whole-process RSS is separate. Every run had zero sampling errors.

## Results

| Run | Bake / runtime layers | Predicted bytes | Lightmap peak KiB | Lightmap seconds / samples | Total seconds |
|---|---|---|---|---|---|
| Current hallway 0.025 | 310 / 230 | 3,910,553,420 | 5,668,252 | 1104.388 / 11,044 | 1995.885 |
| Baseline hallway 0.04 | 104 / not logged | not comparable (old formula) | 5,501,040 | 415.737 / 4,158 | 1301.234 |
| Current hallway 0.04 | 104 / 101 | 2,243,828,058 | 3,883,364 | 413.277 / 4,133 | 1296.540 |
| Runtime-floor fixture 0.0006 | 340 / 340 | 5,153,603,458 | 2,076,020 | 125.145 / 1,252 | 127.720 |

AC35 passes: current lightmap-stage RSS is 29.41% lower than baseline. Wall times differ by less than five seconds; no wall-time improvement claimed.

| Run | Sampled interval, seconds from launch | Output file | Bytes | Blocks |
|---|---|---|---|---|
| Current hallway 0.025 | 853.880–1958.267 | `stress-warren-hallway-inspection.prl` | 3,347,698,137 | 2,083 |
| Baseline hallway 0.04 | 864.513–1280.250 | `stress-warren-hallway-inspection-baseline-0040.prl` | 1,697,547,013 | 2,080 |
| Current hallway 0.04 | 863.292–1276.569 | `stress-warren-hallway-inspection-0040.prl` | 1,697,546,997 | 2,080 |
| Runtime-floor fixture 0.0006 | 0.105–125.250 | `lightmap-runtime-floor-verification.prl` | 1,683,651,690 | 340 |

SH remains a substantial upstream cost in the 0.025 run: the lightmap interval began at 853.880 s. SH base bake occupied about 754 s; the SH/delta family reached 5,589,372 KiB RSS. The whole-process peak was 6,930,056 KiB during animated light weight maps, after the lightmap interval. This run does not support the brief's earlier assumption that SH necessarily sets the overall peak or longest stage. AC34 uses the actual stage-bounded measurement.

## Runtime-floor fixture

`lightmap-runtime-floor-verification.map` preserves the two-room geometry and streaming hints from `sh-streaming-hinted-door.map`. It adds a player spawn facing the doorway and one static SDF light. At density 0.0006, the successful final compile warns that 340 cell blocks need 340 pool layers, exceeding the 256-layer runtime floor; id51 allows streaming. Default 16 GiB budget admitted the 5,153,603,458-byte prediction.

Published metadata confirms 340 real id22 blocks and nonempty id51; id42 is absent because no face admits a mask owner. Four additional singleton SH-cluster diagnostics report 65 primitives against a 64-primitive target. Exit status is zero. The fixture proves admission and provides an output for AC37; it does not prove the engine observation.

The 0.025 hallway uses 230 runtime layers and does not warn. Use the separate fixture for forced-all-resident rejection. Engine launch commands and expected observations are in [manual-runbook.md](./manual-runbook.md).

## Real hallway payload parity at 0.04

Sections 22 and 42 have identical lengths, container-entry versions and SHA-256 hashes across baseline/current outputs. Hashing reads at most 1 MiB at a time and ran after both measured bakes completed. Other sections were not compared.

| Section | Container-entry version | Bytes | SHA-256 |
|---|---|---|---|
| 22 | 1 | 455,032,532 | `6409a05c7bc0a2842a1e11d25a01d0a25b02e8ead523b7ed98dc04876af0f063` |
| 42 | 2 | 606,735,988 | `b6da144c286b4769903cba615f85a84db4bb6cf81193509c4754dcab06b98953` |

## Exact commands

The measurement wrapper set `RUST_LOG=info` and `NO_COLOR=1`, captured output, and sampled only its spawned compiler PID. These are the actual child invocations; tool binaries were copied from the corresponding release build. Baseline cwd was `/private/tmp/postretro-lightmap-proof-ztdf8vxi/baseline-src`; current cwd was the workspace root.

Current hallway 0.025:

```sh
/private/tmp/postretro-lightmap-proof-ztdf8vxi/binaries/current/prl-build content/dev/maps/stress-warren-hallway-inspection.map --lightmap-density 0.025 --no-cache --no-tui -j 4 -v --baked-root /Users/dhiester/Projects/Personal/postretro/baked -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection.prl
```

Baseline hallway 0.04:

```sh
/private/tmp/postretro-lightmap-proof-ztdf8vxi/binaries/baseline/prl-build content/dev/maps/stress-warren-hallway-inspection.map --lightmap-density 0.04 --no-cache --no-tui -j 4 -v --baked-root /Users/dhiester/Projects/Personal/postretro/baked -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection-baseline-0040.prl
```

Current hallway 0.04:

```sh
/private/tmp/postretro-lightmap-proof-ztdf8vxi/binaries/current/prl-build content/dev/maps/stress-warren-hallway-inspection.map --lightmap-density 0.04 --no-cache --no-tui -j 4 -v --baked-root /Users/dhiester/Projects/Personal/postretro/baked -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection-0040.prl
```

Runtime-floor fixture 0.0006:

```sh
/private/tmp/postretro-lightmap-proof-ztdf8vxi/binaries/current/prl-build content/dev/maps/lightmap-runtime-floor-verification.map --lightmap-density 0.0006 --no-cache --no-tui -j 4 -v --baked-root /Users/dhiester/Projects/Personal/postretro/baked -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/lightmap-runtime-floor-verification.prl
```

## Source and binary digests

| File | SHA-256 |
|---|---|
| `content/dev/maps/stress-warren-hallway-inspection.map` | `565355b00b3d5201513bfc14334d5e1f73e8339cd9901b3ecf55ee785e2b6378` |
| `content/dev/maps/stress-warren-hallway-inspection.generated.ts` | `e1ca835b8dc54e634f92a480a6dea1e59fab10d44cc1b3b7085b619e604dceb5` |
| `current/prl-build` | `f93c45462dceba23a8e8c6da89c4bab358e1e03addadb39bef9dd159c7fa3625` |
| `current/scripts-build` | `043c5c2e347c143fdcd9c48e0fd9f83543ac38250297a9f4501fb61774401aae` |
| `baseline/prl-build` | `f270c877a98a3f84d480673ea8c4e2af92cf17ec67c522c822fbd8deaeb9e619` |
| `baseline/scripts-build` | `82d1a32b11c84a62c27607e4fa5cb7fad81d4c8903b6fb084b38a78ae8e2131e` |
| `content/dev/maps/lightmap-runtime-floor-verification.map` | `fe70e4c6dba25def451e297a12ec8262f9f78f5ef223e6dd125595a2e8862427` |

## Evidence lifecycle

Measurements were collected under `/private/tmp/postretro-lightmap-proof-ztdf8vxi`. Raw traces, logs, comparison binaries and the temporary baseline source archive can be removed after this record is committed. Dev-folder outputs remain for owner inspection; the existing stage cache is untouched.
