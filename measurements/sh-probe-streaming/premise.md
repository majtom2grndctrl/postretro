# Slice 1 SH-probe streaming premise read

**Terminal status: `not-yet-evaluable` (2026-09-18).**

The bounded automated capture could not initialize a wgpu adapter on the
available MacBook Pro host. This is an automation/environment finding, not
evidence that resident SH does or does not affect frame time. No GTX 1660
Super was exposed to this task environment, so no result is claimed for that
machine.

## Scope and fixed protocol

The intended stress-warren fixture was not used at 1.0 m: existing project
evidence records that its default 1.0 m spacing produces millions of probes.
The approved production-quality mid-sized substitution was
`content/dev/maps/campaign-test.map`.

| Control | Value |
| --- | --- |
| Source map | `content/dev/maps/campaign-test.map` |
| Source SHA-256 | `7dfde79e6f9e73a91e988a173875932ab1c432d6be501fd90932222171f5b323` |
| Source revision | `2d394f2b9c7516a765ca9d377d0fb59ba4fcf258` (`feature/sh-probe-streaming`) |
| Compiler workers | `RAYON_NUM_THREADS=8` |
| Bake cache / quality mode | compiler `--release`: exact lighting and cache bypass; this is the compiler option, **not** the Cargo release profile |
| Fine / coarse spacing | `1.0 m` / `8.0 m`; no other compiler option differed |
| Camera | `[-65.8368, 1.8288, -45.9232]`, yaw `0°`, pitch `0°`, horizontal FOV `100°` |
| Resolution | `1280×720` RGBA8 capture |
| Timed workload | 120 warmup frames, 600 sample frames; static scene, no per-sample PNG readback |
| Scene files | [`premise/fine-1.0m.scene.json`](premise/fine-1.0m.scene.json), [`premise/coarse-8.0m.scene.json`](premise/coarse-8.0m.scene.json) |

The camera is the transformed `campaign-test` player-spawn pose previously
used by the project's capture evidence. The generated PRLs were deliberately
under `content/dev/maps/`, preserving the normal `content_root_from_map`
derivation and material lookup. The retained scenes name those session-owned
PRLs but are historical protocol evidence only: the PRLs were deleted during
cleanup below.

## Exact commands and outcomes

```sh
RAYON_NUM_THREADS=8 cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o content/dev/maps/.slice1-sh-probe-20260918-fine-1.0m.prl --release --sh-probe-spacing 1.0 --no-tui
RAYON_NUM_THREADS=8 cargo run -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o content/dev/maps/.slice1-sh-probe-20260918-coarse-8.0m.prl --release --sh-probe-spacing 8.0 --no-tui
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- capture measurements/sh-probe-streaming/premise/fine-1.0m.scene.json
```

Both cold compiler bakes completed. The first invocation of the capture command
compiled the capture-feature binary but the task runner returned without a
child status, diagnostics, or published artifact. The command was invoked one
additional time unchanged solely to obtain its terminal diagnostic; that run
reached PRL/material loading and failed before renderer construction:

```text
[Capture] failed to initialize offscreen frame capture renderer: frame capture requires a GPU adapter: No suitable graphics adapter found; noop not requested, vulkan support not compiled in, metal found no adapters, dx12 support not compiled in, gl not requested, webgpu support not compiled in
```

This definitive missing-adapter failure ended the bounded automated attempt.
The coarse capture and alternating repetitions were not run: an A/B pair did
not succeed, and repeating an adapter-creation failure would not add evidence.

## Compiler section-footprint evidence

The compiler validates each emitted PRL before publication. After each successful
bake, the table below was read from its validated output container (the same
section descriptors that the compiler's footprint logger reports). It is retained
here rather than retaining the 166 MiB / 149 MiB disposable PRLs. Each container
has 32 sections and 712 bytes of header plus section table; all rows below are
payload bytes.

| ID / section | Fine 1.0 m | Coarse 8.0 m |
| --- | ---: | ---: |
| 15 Portals | 52,412 | 52,412 |
| 16 TextureNames | 485 | 485 |
| 17 Geometry | 136,140 | 136,140 |
| 18 AlphaLights | 2,344 | 2,344 |
| 19 Bvh | 99,048 | 99,048 |
| 21 LightInfluence | 528 | 528 |
| 22 Lightmap | 25,165,872 | 25,165,872 |
| 23 ChunkLightList | 6,652 | 6,652 |
| 24 AnimatedLightChunks | 19,964 | 19,964 |
| 25 AnimatedLightWeightMaps | 63,352,264 | 63,352,264 |
| 26 LightTags | 365 | 365 |
| 27 DeltaShVolumes | 5,321,468 | 24,862 |
| 28 DataScript | 8,405 | 8,405 |
| 29 MapEntity | 55,067 | 55,067 |
| 30 FogVolumes | 707 | 707 |
| 31 FogCellMasks | 1,860 | 1,860 |
| 32 TextureCacheKeys | 452 | 452 |
| 34 OctahedralShVolume | 2,917,644 | 12,028 |
| 35 DirectShVolume | 1,364,284 | 5,260 |
| 36 NavMesh | 21,986 | 21,986 |
| 37 CellDrawIndex | 6,260 | 6,260 |
| 38 Cells | 26,984 | 26,984 |
| 39 CellLocator | 14,832 | 14,832 |
| 40 EntityShadowLights | 20 | 20 |
| 41 DirectShDeltaVolumes | 1,600,656 | 25,062 |
| 42 ShadowmaskAtlas | 67,108,884 | 67,108,884 |
| 43 KinematicGeometry | 4,213 | 4,213 |
| 44 TriggerVolumes | 875 | 875 |
| 45 AnimatedDirectShDeltaVolumes | 4,695,612 | 21,186 |
| 46 CellVisibility | 79,260 | 79,260 |
| 47 BillboardDirectScatterVolume | 1,552,261 | 5,669 |
| 48 AnimatedBillboardDirectScatterDeltaVolumes | 479,730 | 3,702 |
| **Payload total** | **174,097,534** | **156,263,648** |
| **Whole PRL (payload + 712 B container)** | **174,098,246** | **156,264,360** |

The spacing-only change reduces whole-container disk bytes by 17,833,886 B
(10.244%; fine/coarse = 1.1141×). This is disk-footprint evidence, not a
runtime-residency or frame-time result.

## Renderer and timing evidence

The task host identifies as the requested MacBook Pro and reports macOS
`26.6.2` (build `25G83`), 8 CPU cores at 2.4 GHz, and 32 GB memory. System
profiling lists Intel UHD Graphics 630 and AMD Radeon Pro 5300M (4 GB), but
wgpu selected neither: Metal reported no adapters. No driver string is
available because adapter creation failed.

`POSTRETRO_GPU_TIMING=1` was set, but timestamp-feature availability cannot be
evaluated without an adapter. The capture measurement report is intentionally
published only after the renderer has prepared the scene and the PNG succeeds;
therefore no report or PNG was produced. The following values are absent rather
than zero:

| Evidence | Absence reason |
| --- | --- |
| Fine/coarse measurement JSON paths | `fine-1.0m.report.json` was never staged/published because renderer creation failed; coarse capture was not attempted after the same decisive prerequisite failure |
| Renderer-accounted SH bytes | no renderer / level-GPU install |
| CPU completion samples, median, p95 | no completed capture frame |
| GPU windows / pass values | no adapter, so no timestamp-query capability or frame-timing state |
| Settled manual-observation windows | no wgpu adapter is exposed to this task environment, so a windowed renderer and its title timing cannot be started |
| GTX 1660 Super result | hardware was not present/exposed to this task; no wait or substituted result was fabricated |

The approved MacBook observational fallback cannot run under that same missing
renderer-adapter precondition. The physical adapters reported by the OS do not
override the renderer's actual `metal found no adapters` result.

## Interpretation and cleanup

No A/B frame-time delta was measured. In particular, the disk reduction above
must not be treated as a negligible, material, positive, or negative runtime
effect. The next owner decision remains open until this protocol can run on a
machine where wgpu exposes an adapter (preferably the intended GTX 1660 Super),
or an approved manual observation can actually render.

Cleanup was performed after recording the container evidence. The two
session-owned hidden PRLs were deleted:

```text
content/dev/maps/.slice1-sh-probe-20260918-fine-1.0m.prl    (174,098,246 B; SHA-256 99783607997cd4124a425d0da2c802f247461537560c6d37a3b7acbc0ecebfc4)
content/dev/maps/.slice1-sh-probe-20260918-coarse-8.0m.prl (156,264,360 B; SHA-256 23a38ae16adb0941a7e53da3082594a1c3495a0b195ad287784cc25349bac8b2)
```

No throwaway PNG, measurement JSON, or session-owned `.pack-*` staging file
was published. The two small scene JSON files and this finding are retained;
no large binary artifact is committed.
