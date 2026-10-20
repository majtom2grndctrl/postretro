# Fluid SSR reflections: Metal System Trace (quick)

- Commit `af8bfef61c643c48802a3f9569577581816fbfc8` on `claude/fluid-ssr-reflections-mac-perf`; binary `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/7c4738f3-5c98-40f7-a76f-fe5b9b27ed9f/scratchpad/bin/postretro` (built 2026-10-20T17:22:25Z)
- Machine: Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz | 26.6.2; adapter: {'name': 'AMD Radeon Pro 5300M', 'backend': 'Metal', 'device_type': 'DiscreteGpu'}
- Capture: 120 warmup + 360 sampled frames per run, 1 round(s) ABBA, engine env {"RUST_LOG": "info", "POSTRETRO_SH_STREAMING": "sync-proof"}, light_term_mask omitted (all terms)
- Idle before the batch (ioreg IOAccelerator): {"inUseVidMemoryBytes": [1261772800, 1261772800, 1261772800, 1261772800, 0, 0], "vramFreeBytes": [2003939328, 2003939328, 2003939328, 2003939328], "Device Utilization %": [0, 1, 0, 0, 1]}
- Each pass figure is its GPU time per complete steady frame (gpu_time.py `complete`): frames bounded by the cull-pass frame marker, holding exactly one Depth Pre-Pass, Textured Pass and Fluid Surface Pass encoder, and one Fluid Reflection Pass encoder when the arm records it. `frame` is the union of every labelled GPU interval per complete frame. `forward` is Textured Pass. Values are mean ± half the min-max range across valid rounds. `ABSENT`: the Fluid Reflection Pass was expected (medium and above) and missing, an anomaly. `-`: the arm records no such pass.
- Clock: busy median core clock (min-max) per run. VRAM free MB: before launch / minimum during the run, per run.

| pose | res | config | valid | fluid_reflection ms | fluid_surface ms | forward ms | whole-frame GPU ms | clock MHz per run | VRAM free MB per run |
|---|---|---|---|---|---|---|---|---|---|
| near | auto 1536x960 | off | 0/1 | - | - | - | - | 839 (528-1044) | 1485/900 |
| near | auto 1536x960 | medium | 1/1 | 0.613 | 2.440 | 4.913 | 8.691 | 926 (880-1048) | 1482/893 |
| near | auto 1536x960 | high | 1/1 | 3.420 | 2.409 | 4.903 | 11.482 | 1042 (670-1133) | 1482/880 |
| near | auto 1536x960 | very_high | 1/1 | 3.035 | 2.435 | 5.087 | 11.289 | 971 (400-1109) | 1480/876 |
| oblique-large | auto 1536x960 | off | 0/1 | - | - | - | - | 687 (345-904) | 1478/939 |
| oblique-large | auto 1536x960 | medium | 0/1 | - | - | - | - | 814 (486-986) | 1470/925 |
| oblique-large | auto 1536x960 | high | 1/1 | 2.324 | 1.140 | 4.438 | 8.716 | 1013 (844-1135) | 1470/912 |
| oblique-probes | auto 1536x960 | off | 0/1 | - | - | - | - | 894 (676-1037) | 1470/847 |
| oblique-probes | auto 1536x960 | high | 1/1 | 2.246 | 1.387 | 4.708 | 10.343 | 949 (747-1002) | 1465/824 |
| near | auto 1536x960 | high:pixel:fraction=1.0,steps=145,divisor=135,thickness=1.5 | 1/1 | 2.293 | 2.423 | 4.914 | 10.376 | 1009 (924-1117) | 1460/858 |
| near | auto 1536x960 | high:pixel:fraction=1.0,steps=289,divisor=270,thickness=1.5 | 1/1 | 4.492 | 2.428 | 4.860 | 12.545 | 1049 (339-1219) | 1460/852 |
| near | auto 1536x960 | high:pixel:fraction=1.0,steps=578,divisor=540,thickness=1.5 | 1/1 | 12.034 | 2.473 | 5.998 | 21.479 | 1134 (399-1201) | 1460/858 |
| near | auto 1536x960 | medium:pixel:fraction=0.375,steps=64 | 1/1 | 0.448 | 2.471 | 5.057 | 8.704 | 966 (564-1036) | 1460/873 |
| near | auto 1536x960 | medium:pixel:fraction=0.75,steps=64 | 1/1 | 1.492 | 2.477 | 5.100 | 9.835 | 1004 (701-1064) | 1460/865 |

## Per run

| run | round | fluid_reflection ms | fluid_surface ms | forward ms | frame ms | complete/steady frames | clock MHz | VRAM free MB | valid |
|---|---|---|---|---|---|---|---|---|---|
| quick-near-auto-off-r1 | 1 | - | n/a | n/a | n/a | n/a/n/a | 839 (528-1044) | 1485/900 | NO |
| quick-near-auto-medium-r1 | 1 | 0.613 | 2.440 | 4.913 | 8.691 | 302/360 | 926 (880-1048) | 1482/893 | yes |
| quick-near-auto-high-r1 | 1 | 3.420 | 2.409 | 4.903 | 11.482 | 282/360 | 1042 (670-1133) | 1482/880 | yes |
| quick-near-auto-very_high-r1 | 1 | 3.035 | 2.435 | 5.087 | 11.289 | 294/360 | 971 (400-1109) | 1480/876 | yes |
| quick-oblique-large-auto-off-r1 | 1 | - | 0.268 | 5.281 | 6.505 | 291/360 | 687 (345-904) | 1478/939 | NO |
| quick-oblique-large-auto-medium-r1 | 1 | 0.504 | 1.283 | 4.913 | 7.610 | 283/360 | 814 (486-986) | 1470/925 | NO |
| quick-oblique-large-auto-high-r1 | 1 | 2.324 | 1.140 | 4.438 | 8.716 | 293/360 | 1013 (844-1135) | 1470/912 | yes |
| quick-oblique-probes-auto-off-r1 | 1 | - | 0.226 | 4.495 | 6.660 | 317/360 | 894 (676-1037) | 1470/847 | NO |
| quick-oblique-probes-auto-high-r1 | 1 | 2.246 | 1.387 | 4.708 | 10.343 | 280/360 | 949 (747-1002) | 1465/824 | yes |
| quick-near-auto-high-pixel-fraction1.0-steps145-divisor135-thickness1.5-r1 | 1 | 2.293 | 2.423 | 4.914 | 10.376 | 298/360 | 1009 (924-1117) | 1460/858 | yes |
| quick-near-auto-high-pixel-fraction1.0-steps289-divisor270-thickness1.5-r1 | 1 | 4.492 | 2.428 | 4.860 | 12.545 | 296/360 | 1049 (339-1219) | 1460/852 | yes |
| quick-near-auto-high-pixel-fraction1.0-steps578-divisor540-thickness1.5-r1 | 1 | 12.034 | 2.473 | 5.998 | 21.479 | 219/360 | 1134 (399-1201) | 1460/858 | yes |
| quick-near-auto-medium-pixel-fraction0.375-steps64-r1 | 1 | 0.448 | 2.471 | 5.057 | 8.704 | 289/360 | 966 (564-1036) | 1460/873 | yes |
| quick-near-auto-medium-pixel-fraction0.75-steps64-r1 | 1 | 1.492 | 2.477 | 5.100 | 9.835 | 294/360 | 1004 (701-1064) | 1460/865 | yes |

## Invalid runs (rerun, do not report)

- `quick-near-auto-off-r1`: xctrace export failed: 
- `quick-oblique-large-auto-off-r1`: GPU frame time differs between complete (6.5051 ms) and incomplete (7.4923 ms) frames: dropped rows lost time, not only labels
- `quick-oblique-large-auto-medium-r1`: GPU frame time differs between complete (7.6101 ms) and incomplete (8.0307 ms) frames: dropped rows lost time, not only labels
- `quick-oblique-probes-auto-off-r1`: GPU frame time differs between complete (6.6603 ms) and incomplete (7.4578 ms) frames: dropped rows lost time, not only labels
