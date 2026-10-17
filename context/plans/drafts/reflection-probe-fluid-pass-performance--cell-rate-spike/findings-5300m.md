# Cell-rate spike findings: Radeon Pro 5300M

> **Spike:** pixel-style fluid reflections at cell rate. It informs the third lever of the draft brief "reflection-probe fluid-pass performance".
> **Machine:** 16-inch MacBook Pro, Intel i9-9980HK, AMD Radeon Pro 5300M (4 GB), macOS 26.6.2, Metal. On mains power; a caffeinate assertion held the display and system awake for every timed run.
> **Branch:** `claude/cell-rate-spike`. Engine at `3f4cabed5`, off `a8c6e3300`. Debug only, never merged.
> **Companion:** `findings-1660.md` (GTX 1660 Super, Vulkan), from an independent implementation on `claude/cell-rate-spike-1660`. `findings.md` consolidates the two. The 5300M is the target GPU.
> **Data:**
> - `measurements/cell-rate-spike/results-5300m.{md,json}`: this implementation.
> - `measurements/cell-rate-spike/results-5300m-impl1660.{md,json}`: the 1660 branch's binary timed on this machine.
> - Kit, per-run JSON and gate results: `measurements/cell-rate-spike/metal/` (`runs/`, `runs-impl1660/`, `gates.json`, `gate-runs/`).

## Verdict

- **The cell buffer wins on the 5300M wherever cells cover about 2 or more visible pixels.**
  - Every measured view at 1.8 px per cell or more costs 11–37% less than per-fragment pixel style.
  - Every view at 0.76 or less costs more, up to 3× at 0.1 px per cell.
  - Break-even is about 1 px per cell by pass cost, and about 0.8 by whole-frame GPU time.
- **On this GPU the cell path already reaches reflection-free cost.**
  - At e24 near 4K, one-draw cells cost 55.2 ms against 56.3 for the reflection-free variant.
  - On the probes map: 23.1 against 22.9.
  - There is no long-latency-read penalty on Metal, so the 1660's split draw buys nothing here and only adds its overlay (0.5–2.4 ms at 4K).
- **Most of the gain survives with the ratio forced to 1:** 88% at e24 near, 62% on the probes map.
- **The per-cell frustum cull does not earn its place.** It trims the cell pass by 0.06–0.22 ms and never loses, but that sits at or below run spread on every view.
- **The fluid pass is expensive with or without reflections.** At full-screen 4K the reflection-free fluid pass alone costs 56 ms at e24 near and 23 ms on the probes map; reflections add about 10–14 ms on top. Cell rate removes the reflection's share; the rest of the pass is outside this spike.

## What was built

| Piece | Form |
|---|---|
| Install | Per reflecting face: a least-squares affine fit `world = O + U·t.x + V·t.y` over texel space `t = uv·dims`, with a residual check. Cell rect from `floor(min t)` to `ceil(max t) − 1`, so a face edge on a whole texel adds no empty row. Shelf-packed, tallest first, into one `Rgba16Float` storage-and-sampled atlas, at most 4096 wide. A face that does not fit keeps the per-fragment path. e24: 1360×1472, 16.0 MB, 17 faces. Probes map: 512×640, 2.6 MB, 8 faces. |
| Fluid Reflection Cell Pass | Its own compute pass, labelled exactly that. Each frame, the 8×8 tiles of the drawn reflecting faces are copied into a preallocated tile list with the draw's reflectivity patched in, then dispatched once, one thread per cell. The shader is appended to the fluid shader's source and calls the fragment's own functions unchanged: displayed offset, rim pull, tilted normal, search with shadow fade, colour tap, ratio and Fresnel. Texels per pixel is analytic at the cell centre. It writes the reflection colour and its Fresnel weight. |
| Cull lever | A per-cell bounding sphere of radius `0.5·max(|U+V|, |U−V|)` against the frustum planes. |
| Fragment | A `FLUID_REFLECTION_CELLS` override inside the reflecting variant's `fs_main`. The fragment reads its cell's texel through `textureLoad` (group 4, binding 4), clamped to the face's rect. Per-face atlas origin, rect minimum and extent arrive as a flat `vec4<u32>` vertex attribute (location 5). The proximity band stays per fragment. Smooth style and the reflection-free variant are untouched. |
| Ratio lever | `FLUID_REFLECTION_RATIO_ONE`: an early return of 1 in `fluid_reflection_ratio`, in both paths. |
| Levers | Capture scene: `fluid_reflection_cells {cells, frustum_cull, ratio_one}`, `fluid_reflection_style`, `pre_camera`, and `fluid_reflection_cell_stats`, which adds atlas size, cells dispatched and evaluated, and visible reflecting fragments from an occlusion query. Plus three dev-tools checkboxes. |

The lever pipelines (3 render, 4 compute) are built only when a lever is set. The shipped pipelines keep main's override constants. They are not byte-identical to main: they bind the fourth vertex buffer and declare the atlas binding. Their output is bit-identical to main's (gates below).

## Honesty gates

| Gate | Result |
|---|---|
| Cell buffer vs per-fragment pixel style within 2/255 | **Passes, with every exception explained.** At animation time 0: near 4K and 1080p and probes 4K are within 1/255 with 0 pixels over 2. Far shows 64 px over at 4K and 20 at 1080p, max 23. At nonzero times (measurement blocks of 4+4, 30+30 and 120+120 frames): near 4K 164 / 369 / 0 px over (max 8); near 1080p 19 / 39 / 0; probes 0 at every time; far 4K 143 / 168 / 67 (max 23); far 1080p 57 / 96 / 20. The ratio forced to 1 gives the same counts. |
| The exceptions | All are the cell-centre difference. The fragment estimates the cell centre and texels per pixel from screen derivatives; the cell pass uses the exact affine map and analytic texels per pixel. Every near-4K over-limit pixel at t8 and 95% at t60 were inspected by hand. Each sits in a cell the per-fragment image draws in two values, while the cell image shows one of them across the whole cell. The largest t8 cluster (142 px) is a cell cut by the screen's left edge: its true centre is off screen, so on-screen fragments extrapolate their derivative estimate furthest. Far exceptions are isolated 1–4 px clusters on distant pools, the pattern of cells at their fade threshold. The adapter test attributes 12 of 12 such pixels to that threshold. The two automated attempts to classify every pixel were inconclusive, because the water's own lighting varies inside a few pixels. The GTX 1660 run finds the same exceptions in its implementation. |
| Every fragment of one cell reads one texel | Passes, in the adapter test only. 3436 pixels across 82 cells each show exactly their cell's texel from a unique per-texel pattern. Not checked on the real maps. |
| Smooth style and the reflection-free variant bit-identical to main | Passes: identical at all 5 configs. Main is a build of `a8c6e3300`. Per-fragment pixel style with every lever off is also identical to main. Smooth with the cell levers on is identical to smooth with them off. The reflection-free variant (lighting mask without the specular bit) with every lever on is identical to main's. |
| No stale cells | Passes. A capture that renders a different pose first (+1 m, +0.5 m, −1 m, yaw +90°, pitch +30°) and then jumps to the pose is identical to a fresh capture, with and without the cull, at all 5 configs. The adapter test also passes. |
| Adapter tests A–D | All pass on Metal at 64×64. Parity: worst 0–1/255 straight down, including cells straddling a probe-box rim; at an oblique pose 12 of 2176 px are over, all fade-threshold cells. One texel per cell, no stale cells, and lever-off identity also pass. All 8 lever combinations build and draw. |
| DX12 FXC | **Not run.** `dx12_fxc_test` only builds on Windows. This branch extends it to all 8 lever combinations, but the extension has never been compiled. The 1660 branch's FXC test passes for its own pipelines, which are a different implementation. |

## Views

| Config | Map, pose (engine m; yaw; pitch) | Reflecting draws | Cells dispatched | Kept by cull | Visible reflecting px | px per cell |
|---|---|---|---|---|---|---|
| near 4K / 1080p | e24 still pool: −4.8768, 1.6, −44.2976; 0; −89 | 1 | 65,536 | 20,572 | 8.29M / 2.07M | 127 / 32 |
| probes 4K | probes-map pool: −6.5024, 1.6, −6.5024; 0; −89 | 1 | 36,864 | 13,112 | 8.29M | 225 |
| far 4K / 1080p | e24 fluid-motion wing from its west end: −4.877, 4.0, −40.64; 0; +5 | 4 | 1,294,336 | 1,233,547 | 513k / 128k | 0.40 / 0.10 |
| low 4K | e24 near spot: −4.8768, 1.6, −44.2976; 0; −15 | 5 | 1,359,872 | 1,237,229 | 2.48M | 1.83 |
| mid 4K / 1080p | same spot; 0; −30 | 5 | 1,359,872 | 1,241,873 | 4.14M / 1.03M | 3.05 / 0.76 |
| tilt 1080p | same spot; 0; −45 | 3 | 204,800 | 80,945 | 1.45M | 7.06 |

- **Pixels per cell:** visible reflecting fragments over dispatched cells, both from `fluid_reflection_cell_stats`.
- **Visible reflecting fragments:** an occlusion-query sample count over the reflecting draws, so overlapping faces count once each.
- **Low, mid and tilt** were added after the main batch to fill the gap between 0.4 and 32 px per cell. They pitch the near pose up so the wing's other pools enter the view, including the fine pool at 0.25 texel scale, which holds about 1M cells.
- **Atlas:** e24 1360×1472 (16,015,360 B), probes map 512×640 (2,621,440 B).

## Measurements

**Method.** Metal System Trace, reduced with the complete-frame method of `measurements/reflection-blend-cost-spike/`:
- Each run is one headless `--capture` with 240 warmup and 240 sampled frames, `POSTRETRO_SH_STREAMING=sync-proof`.
- Frames are framed by the Candidate Cull Pass. A frame counts only if it holds exactly one depth, forward and fluid encoder, plus exactly one cell-pass encoder in cell arms and none in the others.
- **Cost** is Fluid Surface Pass plus Fluid Reflection Cell Pass per frame.
- Two interleaved runs per arm (the second round in reverse order), a 15 s idle gap before each launch, ioreg clock and temperature polled throughout.
- Busy-process, screen-lock and screen-saver checks before every run.
- Core clock: about 1170–1227 MHz at 4K, about 1000–1170 MHz at 1080p (§12).
- Run validity:
  - Main batch (30 arms): 60 of 60 runs valid.
  - Free and mid-range arms: 26 runs, with one invalid probes `free` run (the trace dropped rows that carried time), rerun valid.
  - The 1660 binary: 28 of 28 runs valid.
  - Low-resolution probes arms: discarded (below).

**Cost in ms per frame, mean of 2 runs ± half the run spread:**

| Config | ratio | frag | cells | cull | free |
|---|---|---|---|---|---|
| near 4K | live | 65.88 ±0.08 | **55.21** ±0.01 | 55.18 ±0.25 | 56.25 ±0.07 |
| near 4K | r1 | 64.71 ±0.17 | **55.30** ±0.15 | 55.21 ±0.14 | |
| near 1080p | live | 16.55 ±0.01 | **14.07** ±0.00 | 14.01 ±0.01 | 14.28 ±0.00 |
| near 1080p | r1 | 16.25 ±0.04 | **14.06** ±0.02 | 14.07 ±0.10 | |
| probes 4K | live | 36.51 ±0.00 | **23.08** ±0.04 | 22.84 ±0.09 | 22.92 ±0.06 |
| probes 4K | r1 | 31.32 ±0.01 | **22.98** ±0.08 | 22.83 ±0.03 | |
| far 4K | live | **3.33** ±0.00 | 4.39 ±0.02 | 4.31 ±0.00 | 2.79 ±0.01 |
| far 4K | r1 | **3.25** ±0.01 | 4.27 ±0.00 | 4.19 ±0.01 | |
| far 1080p | live | **0.94** ±0.00 | 2.83 ±0.14 | 2.72 ±0.06 | 0.89 ±0.01 |
| far 1080p | r1 | **0.92** ±0.00 | 2.78 ±0.04 | 2.70 ±0.05 | |
| low 4K | live | 12.75 ±0.01 | **11.35** ±0.00 | | |
| mid 4K | live | 26.06 ±0.01 | **20.93** ±0.01 | | |
| mid 1080p | live | **6.28** ±0.00 | 7.00 ±0.03 | | |
| tilt 1080p | live | 9.84 ±0.00 | **8.49** ±0.04 | | |

`free` is the reflection-free variant: the lighting mask without the specular bit (`0x13F`), which the fluid never reads. It also drops specular from the Textured Pass (−2.7 ms at 4K), but that pass is outside the cost.

**The cell pass itself:**

| Config | cells | cull |
|---|---|---|
| near 4K / 1080p | 0.19 / 0.18 ms | 0.13 / 0.13 ms |
| probes 4K | 0.57 ms | 0.35 ms |
| far 4K / 1080p | 1.67 / 2.02 ms | 1.59 / 1.91 ms |
| low 4K, mid 4K | 1.74 ms | |
| mid 1080p | 2.05 ms | |
| tilt 1080p | 0.42 ms | |

### Encoder overlap and whole-frame GPU time

The trace timeline shows the cell pass running concurrently with other encoders for 96–99% of its interval:
- **e24:** it runs beside the Depth Pre-Pass (0.15 ms).
- **Probes map:** it follows the SDF Shadow Pass and starts together with the 22 ms Textured Pass. Streamed SH Compose also runs every frame on that map, and not on e24.

A pass's own interval is therefore shared GPU time, not its isolated cost:
- The probes map's 0.57 ms for 36,864 cells (about 9 ns per evaluated cell, from the cull's slope) against e24's 1.3 ns is contention with the Textured Pass, not costlier cells.
- In the other direction, the overlapped pass can slow down. At far 4K the Textured Pass grows by about 1 ms with cells on.

As a check, the label-independent GPU frame (the union of every labelled interval per complete frame) against frag:

| Config | px per cell | cost Δ | whole-frame Δ |
|---|---|---|---|
| far 1080p | 0.10 | +1.89 | +0.87 |
| far 4K | 0.40 | +1.06 | +0.46 |
| mid 1080p | 0.76 | +0.72 | −0.18 |
| low 4K | 1.83 | −1.39 | −1.93 |
| mid 4K | 3.05 | −5.12 | −10.55 |
| tilt 1080p | 7.06 | −1.34 | −1.54 |
| near 1080p | 32 | −2.47 | −2.59 |
| near 4K | 127 | −10.66 | −10.81 |
| probes 4K | 225 | −13.43 | −12.02 |

On Metal the whole frame favours cells slightly more than the cost metric does, because the cell pass hides partly under other encoders. Mid 4K's whole-frame gain is twice its cost gain; that gap is not explained, so the verdicts use the cost metric and treat the whole frame as a direction check. The `free` arms' whole frame also carries the Textured Pass's specular saving and is not comparable.

**Discarded:** probes-map runs at 960×540, 640×360 and 480×270, meant to find that map's break-even with the cell work held fixed. The GPU sat at 360–800 MHz, and 9 of 12 traces dropped rows that carried time. The encoder-overlap finding answered their question instead. Their records remain in `metal/runs/` and the results file, marked as such.

## Analysis

### Does the cell buffer win, and at what pixels per cell?

| px per cell | Config | cells / frag |
|---|---|---|
| 0.10 | far 1080p | 3.00× |
| 0.40 | far 4K | 1.32× |
| 0.76 | mid 1080p | 1.11× |
| 1.83 | low 4K | 0.89× |
| 3.05 | mid 4K | 0.80× |
| 7.06 | tilt 1080p | 0.86× |
| 32 | near 1080p | 0.85× |
| 127 | near 4K | 0.84× |
| 225 | probes 4K | 0.63× |

The cost model reproduces the crossing:
- A cell costs about 1.3 ns at 4K clocks and about 1.5 ns at 1080p clocks (large dispatches, pass interval over cells evaluated).
- A visible pixel saves about 1.2–1.7 ns of fragment work (frag fluid minus cells fluid, over visible reflecting fragments).
- Break-even is their ratio: 1.0–1.2 px per cell on the large e24 dispatches (low 4K, mid 1080p, far 4K) and 1.5 at far 1080p.
- Small dispatches add a fixed cost of about 0.1 ms, which their hundreds of pixels per cell repay many times over.

**Threshold:** a draw takes the cell path at 2 or more visible pixels per dispatched cell, and the per-fragment path below 1. Between 1 and 2 the difference is within about 10% either way. This matches the 1660's rule.

### Split or one draw

This machine timed the 1660 branch's binary (`cab5197a5`, same kit, 2 interleaved runs, ms live / r1):

| Config | frag | cell2 | frag2 | free |
|---|---|---|---|---|
| near 4K | 66.67 / 65.76 | 57.88 / 58.04 | 66.98 / 66.43 | 57.37 |
| probes 4K | 36.76 / 31.54 | 25.51 / 25.26 | 35.75 / 32.05 | 23.13 |

- **No read penalty on Metal:**
  - `frag2`, the split without cells, saves nothing at e24 near (66.98 against 66.67).
  - This implementation's one-draw cells sit at reflection-free cost: 55.21 against 56.25 near, 23.08 against 22.92 probes.
  - Near 4K one-draw cells even read about 1 ms under `free`; the cause is not established.
- **The overlay draw is pure overhead here:** `cell2` costs 0.5 ms over that binary's `free` at near 4K, and 2.4 ms over this implementation's one-draw cells on the probes map.
- **Cross-build check:** that binary's `frag` reads 0.8 ms above this implementation's at near 4K and 0.25 ms above on the probes map, so cross-implementation differences under about 1 ms are build-to-build noise.

### Does the frustum cull earn its place?

No. Cells removed and the change in total cost:

| Config | Cells removed | Total change | Run spread |
|---|---|---|---|
| near | 69% | −0.03 ms | 0.5 ms |
| probes | 64% | −0.24 ms | 0.17 ms |
| far 4K | 5% | −0.08 ms | |
| far 1080p | 5% | −0.11 ms | |

Unlike on the 1660 it never loses, but on every view it saves less than the noise between equivalent arms. A per-face or per-rect clip on the CPU would take the near case's trim without a per-cell test.

### How much survives the ratio forced to 1?

| Config | One-draw cells vs frag, live → r1 | Share surviving |
|---|---|---|
| near 4K | −10.66 → −9.41 ms | 88% |
| near 1080p | −2.47 → −2.19 ms | 88% |
| probes 4K | −13.43 → −8.34 ms | 62% |

**What the ratio is worth per fragment:**
- **e24:** 1.16 ms at near 4K. At capture time e24's stored lights sit at their reference, so the ratio is exactly 1 there and the lever changes no pixels; it only removes the four-light loop.
- **Probes map:** 5.19 ms. A light differs from its reference, so the lever changes the image there too.
- **Implication:** on the 5300M the precompute is worth more on the probes map than the 1660's 1.7 ms suggested, and the cell path still keeps most of its gain once the precompute lands.

## Recommendation (5300M)

1. **One-draw cell buffer behind a per-draw pixels-per-cell test:** cells at 2 or more visible pixels per dispatched cell, per-fragment below 1.
2. **No split draw on this GPU.** There is no read penalty to remove, and the overlay costs 0.5–2.4 ms at 4K.
3. **Drop the per-cell frustum cull.**
4. **Keep the ratio precompute.** It is worth 1.2–5.2 ms per full-screen 4K draw here, and 62–88% of the cell gain survives it.
5. **Report whole-frame GPU time beside pass cost** for any compute that can overlap other encoders.
6. **Open:** the reflection-free fluid pass itself (56 ms full-screen at e24 near 4K) is the largest cost seen. It is outside this spike, and it is the next thing to measure on this GPU.
