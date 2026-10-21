# Fluid SSR reflections: Metal System Trace (pilot)

- Commit `af8bfef61c643c48802a3f9569577581816fbfc8` on `claude/fluid-ssr-reflections-mac-perf`; binary `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/7c4738f3-5c98-40f7-a76f-fe5b9b27ed9f/scratchpad/bin/postretro` (built 2026-10-20T17:22:25Z)
- Machine: Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz | 26.6.2; adapter: {'name': 'AMD Radeon Pro 5300M', 'backend': 'Metal', 'device_type': 'DiscreteGpu'}
- Capture: 120 warmup + 360 sampled frames per run, 1 round(s) ABBA, engine env {"RUST_LOG": "info", "POSTRETRO_SH_STREAMING": "sync-proof"}, light_term_mask omitted (all terms)
- Idle before the batch (ioreg IOAccelerator): {"inUseVidMemoryBytes": [1221402624, 1221402624, 1221402624, 1221402624, 0, 0], "vramFreeBytes": [2081587200, 2081587200, 2081587200, 2081587200], "Device Utilization %": [0, 1, 0, 0, 1]}
- Each pass figure is its GPU time per complete steady frame (gpu_time.py `complete`): frames bounded by the cull-pass frame marker, holding exactly one Depth Pre-Pass, Textured Pass and Fluid Surface Pass encoder, and one Fluid Reflection Pass encoder when the arm records it. `frame` is the union of every labelled GPU interval per complete frame. `forward` is Textured Pass. Values are mean ± half the min-max range across valid rounds. `ABSENT`: the Fluid Reflection Pass was expected (medium and above) and missing, an anomaly. `-`: the arm records no such pass.
- Clock: busy median core clock (min-max) per run. VRAM free MB: before launch / minimum during the run, per run.

| pose | res | config | valid | fluid_reflection ms | fluid_surface ms | forward ms | whole-frame GPU ms | clock MHz per run | VRAM free MB per run |
|---|---|---|---|---|---|---|---|---|---|
| near | auto 1536x960 | off | 1/1 | - | 0.457 | 4.819 | 6.025 | 950 (865-1009) | 1932/1340 |
| near | auto 1536x960 | high | 0/1 | - | - | - | - | 795 (244-980) | 1911/1309 |

## Per run

| run | round | fluid_reflection ms | fluid_surface ms | forward ms | frame ms | complete/steady frames | clock MHz | VRAM free MB | valid |
|---|---|---|---|---|---|---|---|---|---|
| pilot-near-auto-off-r1 | 1 | - | 0.457 | 4.819 | 6.025 | 349/360 | 950 (865-1009) | 1932/1340 | yes |
| pilot-near-auto-high-r1 | 1 | 4.089 | 2.453 | 6.516 | 13.785 | 358/360 | 795 (244-980) | 1911/1309 | NO |

## Invalid runs (rerun, do not report)

- `pilot-near-auto-high-r1`: GPU frame time differs between complete (13.7853 ms) and incomplete (16.011 ms) frames: dropped rows lost time, not only labels
