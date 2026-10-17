# Cell-rate spike findings: GTX 1660 Super

> **Spike:** pixel-style fluid reflections at cell rate. It informs the third lever of the draft brief "reflection-probe fluid-pass performance".
> **Machine:** Windows 11, GTX 1660 Super, Vulkan, engine GPU timestamps (`POSTRETRO_GPU_TIMING=1`).
> **Branch:** `claude/cell-rate-spike-1660`. Engine at `65ca0fbb3`; the `frag2` arm and its lever were added after it. Debug only, never merged.
> **Companion:** the 5300M run is `findings-5300m.md` on `claude/cell-rate-spike`, from an independent implementation and a Metal System Trace kit. `findings.md` will consolidate the two. The 5300M is the target GPU. These 1660 figures show the shape of the costs; the Mac decides the thresholds.
> **Data:** `measurements/cell-rate-spike/results-1660.{md,json}`, `runs/ts-1660-*.json`, `runs/maskfree-1660-*.json`, `runs/count-stats.json`, `gates-1660.{md,json}`, `gate-split-cells.json`. The kit that produced them is in `measurements/cell-rate-spike/1660/`, copied from `claude/cell-rate-spike-1660` at `f63f94cd4`; its paths assume it runs from `measurements/cell-rate-spike/`.

## Verdict

- **The cell buffer pays only where cells cover more than about two pixels.** Below about one pixel per cell it loses.
- **On the 1660 it works best combined with a split draw.** The surface draws through the reflection-free pipeline, and a second, premultiplied draw adds the reflection.
- **The split matters more than the cells on e24, on the 1660 only.** There, most of e24's reflection cost is not the reflection's arithmetic. It comes from any long-latency read inside the heavy lighting shader, the effect §7.3a "Two pipelines" calls the reflection block's "mere presence". The 5300M timed this branch's binary and has no such penalty: its one-draw cells already sit at reflection-free cost, and the split only adds its overlay (0.5–2.4 ms at 4K). The split is a per-GPU fix, not a general one.
- **A ratio precompute alone recovers little there.** At e24 near 4K it saves 1.6 ms of the 10.0 ms the reflection adds.
- **The per-cell frustum cull never earned its place.**

## What was built

| Piece | Form |
|---|---|
| Cell atlas | One RGBA16F texel per water texel cell (`floor(uv * dims)`), shelf-packed per probe-bound face, with an exact f64 affine texel-to-world fit per face. e24: 1344×1472, 15.8 MB. Probes map: 512×640, 2.6 MB. |
| Fluid Reflection Cell Pass | Compute, 8×8 tiles of the drawn reflecting faces seen from above. One thread per cell calls the fragment's own WGSL functions. The cell centre comes from the affine map, texels per pixel is analytic at the centre, and the output is the capture times the ratio plus Fresnel times the shadow weight. |
| `cell` | The reflecting fragment reads its cell's texel (flat per-face offset and rect through a vertex attribute) in its own fragment entry, `fs_cells`. |
| `cellcull` | `cell` plus a per-cell frustum cull: the four corners against the six clip planes. |
| `cell2` | Two draws: the surface through the reflection-free pipeline, then `fs_cells_overlay` reading the cell, blended premultiplied `(F·R, F)`. The single-draw composite factors exactly into these two blends. |
| `frag2` | The same two draws without cells. The overlay evaluates the reflection per fragment (`fs_frag_overlay`). |
| `free` | Baseline: every draw takes the reflection-free pipeline under the all-terms mask (`fluid_reflection_off`). |
| `r1` | The live-lighting ratio forced to 1 in every path, to approximate the precompute's floor. |

Levers live in the capture scene file and a dev-tools group ("Fluid reflections (spike)").

## Honesty gates

Real-map capture PNGs (warmup 4 + 4 frames) were diffed against an `origin/main` build of a8c6e3300. That build carries one local, uncommitted line: an env hook that selects smooth style, since main's capture cannot. Adapter tests are in `fluid_surface_frame_test.rs`.

| Gate | Result |
|---|---|
| Cell buffer vs per-fragment pixel style within 2/255 | **Passes, with every exception explained.** Probes 4K: max 1, 0 pixels over 2. e24 near: 840 of 8.29M pixels over 2 at 4K and 164 at 1080p, max 9. Far: 32 at 4K and 18 at 1080p, max 10. At near 4K, 766 of the 840 lie in cells that per-fragment pixel style itself draws in more than one value (speckle inside one cell; the cell pass draws one of them). The rest are 1–2 px fragments on cell seams, too small to classify. Far's lie on sub-pixel cells at their fade threshold (`gate-split-cells.json`). Adapter test: 0 of 4096 differ straight down and oblique; at grazing, 13 differ, all in fade-straddling cells. |
| Cell-centre difference | The fragment estimates the centre and texels per pixel from screen derivatives; the cell pass uses the exact affine map and analytic texels per pixel. They agree except where the search or shadow fade is discontinuous inside a cell, or a cell straddles its fade threshold. In both cases the per-fragment path itself splits the cell. |
| Every fragment of one cell reads one texel | Passes. The read key is `floor(uv * dims)`, the key pixel style floors. The adapter test finds an in-cell spread of 0 at three poses (100–294 cells on screen). |
| Smooth style and the reflection-free variant bit-identical to main | Passes: max 0 at all 5 views, with every lever off and with the cell levers on. Per-fragment pixel style with levers off is also bit-identical to main. |
| No stale cells | Passes. The adapter test renders pose A then B on one renderer, cull off and on, and matches a fresh renderer at B bit for bit. The cull adds no difference on real maps (max 0). |
| Two-draw forms | `cell2` vs `cell` and `frag2` vs `frag`: max 1/255 at every view (blend rounding). |
| DX12 FXC | `dx12_fxc_compiles_every_full_init_pipeline` passes. It covers all 7 fluid render pipelines and both cell compute pipelines. |

The 5300M session's first gate found 0 pixels over 2 at e24 near, but its captures rendered at time 0 (no measurement block) while these rendered after 8 steps. Redone at matching times, its own implementation shows the same exceptions: 164 px over at t8, 369 at t60 and 0 at t240 at near 4K (max 8/255); 0 at every time on probes; up to 168 at its far 4K (max 23). By hand inspection, 100% of its t8 pixels and 95% of t60 are cells the per-fragment image draws in two values while the cell image shows one of them. Its largest cluster is a cell cut by the screen edge, where the derivative-estimated centre extrapolates furthest. The exceptions are the derivative-vs-affine cell-centre difference, common to both implementations.

## Views

| View | Map, pose (engine m; yaw; pitch) | Faces | Cells | Kept by cull | Visible fluid px | px per cell |
|---|---|---|---|---|---|---|
| near 4K / 1080p | e24 still pool: −4.8768, 1.6, −44.2976; 0; −89 | 1 | 65,536 | 20,078 | 8.29M / 2.07M | 127 / 32 (413 / 103 kept) |
| probes 4K | reflection-probes-test pool: −6.5024, 1.6, −6.5024; 0; −89 | 1 | 36,864 | 12,844 | 8.29M | 225 (646 kept) |
| far 4K / 1080p | e24 fluid-motion wing, map (2300, 1000, 400): −25.4, 10.16, −58.42; −90; −30 | 10 | 1,409,024 | 1,338,209 | 999k / 250k | 0.71 / 0.18 |

Visible fluid pixels come from an occlusion query over the fluid pass. Far looks south over both rows of pools, 10.6 m above the water, from 10–26 m along the view. The scale-0.25 fine pool alone holds about 1M cells. Positions on the wing's x 2064 line render half void, so they were not used.

## Measurements

Cost is Fluid Surface Pass plus Fluid Reflection Cell Pass, in ms per frame. Each figure is the mean of two interleaved runs (240 warmup + 360 sampled frames) ± half the run spread. Capture draws pixel style at reflectivity 1. `free` has no ratio, so it appears once per view.

| View | ratio | frag | cell | cellcull | cell2 | frag2 | free |
|---|---|---|---|---|---|---|---|
| near 4K | live | 22.24 ±0.11 | 20.62 ±0.07 | 20.60 ±0.13 | **12.87** ±0.04 | 14.97 ±0.05 | 12.24 |
| near 4K | r1 | 20.65 ±0.01 | 21.22 ±0.02 | 20.42 ±0.05 | **12.66** ±0.04 | 14.66 ±0.01 | |
| near 1080p | live | 5.73 ±0.02 | 5.33 ±0.01 | 5.32 ±0.01 | **3.50** ±0.00 | 3.95 ±0.03 | 3.11 |
| near 1080p | r1 | 5.27 ±0.01 | 5.32 ±0.01 | 5.31 ±0.01 | **3.50** ±0.00 | 3.93 ±0.09 | |
| probes 4K | live | 10.85 ±0.02 | **6.23** ±0.00 | 6.22 ±0.00 | 7.14 ±0.00 | 10.26 ±0.02 | 6.48 |
| probes 4K | r1 | 9.12 ±0.05 | **6.26** ±0.05 | 6.22 ±0.01 | 7.13 ±0.00 | 8.70 ±0.00 | |
| far 4K | live | 1.91 ±0.00 | 1.96 ±0.01 | 1.99 ±0.00 | 2.23 ±0.00 | **1.73** ±0.00 | 1.38 |
| far 4K | r1 | 1.91 ±0.00 | 1.91 ±0.01 | 1.94 ±0.00 | 2.18 ±0.00 | **1.73** ±0.00 | |
| far 1080p | live | **0.57** ±0.00 | 0.81 ±0.00 | 0.84 ±0.00 | 0.93 ±0.00 | 0.60 ±0.00 | 0.42 |
| far 1080p | r1 | **0.57** ±0.00 | 0.77 ±0.00 | 0.79 ±0.00 | 0.89 ±0.00 | 0.59 ±0.00 | |

The cell pass itself costs:

| View | cell | cellcull |
|---|---|---|
| near | 0.031 ms | 0.022 ms |
| probes | 0.027 ms | 0.019 ms |
| far 4K | 0.466 ms | 0.500 ms |
| far 1080p | 0.371 ms | 0.393 ms |

**Noise.** Within a near-4K arm the two runs agree to 0.1–0.2 ms, but arms that should match can differ by up to 0.8 ms (`cell-r1` 21.22 against `cellcull-r1` 20.42). Read near-4K differences under about 0.8 ms as noise. Every other view repeats to 0.1 ms or better.

**Whole-frame check.** The 5300M trace showed its cell pass overlapping other encoders (96–99% of its interval), so a pass's own interval is shared GPU time, and the overlapped pass can slow down. Engine timestamps on Vulkan can hit the same thing. Capture waits for the GPU every frame, so each run's median CPU completion time is a whole-frame figure that no overlap can hide. Against `frag`, the difference in completion time matches the difference in cost:

| View | Cost metric vs whole frame |
|---|---|
| near and probes | Agree within 0.1–0.3 ms, every arm and ratio |
| far 4K | Cells look slightly worse by whole frame: `cell` +0.04 → +0.19 ms, `cellcull` +0.08 → +0.22, `cell2` +0.31 → +0.52 |
| far 4K, `frag2` | Flips: −0.18 by cost, +0.16 by whole frame |
| far 1080p | `cell` +0.25 → +0.41 ms; `frag2` +0.03 → +0.11 |

Completion time also holds the CPU's per-frame tile-list build (about 22k tiles at far), so part of the far gap may be CPU. Either way the far verdicts below use the whole-frame figure. The per-cell nanosecond figures in the analysis are pass-interval figures; read them as lower bounds at far.

## Analysis

### Where the reflecting variant's cost goes (e24 near 4K)

1. Per-fragment pixel style costs 10.0 ms over reflection-free (22.24 against 12.24).
2. Moving the reflection's evaluation to the cell pass removes only 1.6 ms (`cell` 20.62).
3. Two further tests show the remaining cost is not the shader's code:
   - Giving the cells variant its own fragment entry, with no per-fragment reflection code at all, does not change it (21.3 ms).
   - Replacing its one atlas `textureLoad` with a constant drops it to 12.57 ms, the reflection-free cost.
4. So on the 1660, one long-latency read inside this shader costs about 8.4 ms at e24 near 4K. The likely mechanism is occupancy: the read stays live across e24's heavy per-fragment lighting.
5. Taking the read out of that shader recovers it. `frag2` (the split, no cells) costs 14.97 ms; `cell2` costs 12.87 ms, 0.6 ms over reflection-free.
6. On the probes map the same read costs nothing: one-draw `cell` (6.23) even sits under reflection-free (6.48), within noise of it.
7. The penalty therefore depends on content: the weight of the lighting loop the read shares a shader with.

### Does the cell buffer win, and at what pixels per cell?

| Path | Break-even px per cell | How measured |
|---|---|---|
| One-draw cell | ≈ 0.5–0.8 by pass timing; about 1 by whole frame | far views, where the fragment's saving is the reflection's own work (0.42–0.50 ns per pixel against 0.23–0.33 ns per cell, pass intervals). Far 4K at 0.71 px per cell loses 0.1–0.2 ms by whole frame. |
| One-draw cell | ≈ 1.3–1.7 | probes, where near-wall cells search longer (0.59–0.73 ns per cell) |
| One-draw cell | ≈ 2.1–2.4 | e24 near, where the read penalty eats the saving |
| `cell2` vs `frag2` | ≈ 1 | far: the overlay is a constant cost in both, so cells must beat the per-fragment overlay alone |

Measured views sit at 0.18–0.75 or 32–646 px per cell; nothing between 1 and 30 was measured.

**Threshold for the brief:** a draw takes the cell path when its visible pixels per dispatched cell are at least 2, and the per-fragment path below 1. Between 1 and 2 the choice hardly matters on this GPU. Pixels per cell is about 1 / texels-per-pixel² for a face seen square-on. A per-draw estimate from the face's nearest texels per pixel, or projected area over cell count, is enough. The 5300M's mid-range poses confirm it on the target GPU: break-even lies between 0.76 and 1.83 px per cell; every view at 1.8 or more wins 11–37% and every view at 0.76 or less loses.

**Gain at the best path per view, against per-fragment, ratio live:**

| View | Best path | Change |
|---|---|---|
| e24 near 4K | `cell2` | −42% |
| e24 near 1080p | `cell2` | −39% |
| probes 4K | `cell` | −43% |
| far 4K | `frag` | 0 (`frag2` reads −9% by pass timing but +0.16 ms by whole frame) |
| far 1080p | `frag` | 0 |

Where cells are large, cells beat the per-fragment overlay:
- e24 near: `cell2` beats `frag2` by 2.1 ms at 4K and 0.45 ms at 1080p.
- probes: one-draw `cell` beats `frag2` by 4.0 ms.

### Split or not

The overlay draw costs about 0.6 ms at full 4K coverage (`cell2` against `free`, on both maps).

| Lighting shader | Better path | 1660 evidence |
|---|---|---|
| Heavy (e24) | The split: it removes an 8.4 ms penalty | `cell2` 12.87 against `cell` 20.62 |
| Light (probes) | One draw: the split's 0.6–0.9 ms is pure overhead | `cell` 6.23 against `cell2` 7.14 |

The 5300M timed this branch's binary (`cab5197a5`, Metal System Trace, 2 interleaved runs, ms live / r1):

| View | frag | cell2 | frag2 | free |
|---|---|---|---|---|
| near 4K | 66.67 / 65.76 | 57.88 / 58.04 | 66.98 / 66.43 | 57.37 |
| probes 4K | 36.76 / 31.54 | 25.51 / 25.26 | 35.75 / 32.05 | 23.13 |

There is no read penalty on Metal:
- `frag2` recovers nothing (66.98 against `frag` 66.67).
- The 5300M's own one-draw cells sit at reflection-free cost: near 4K 55.2 against its `free` 56.3, probes 23.1 against 22.9.
- `cell2`'s overlay costs about 0.5 ms at near 4K and 2.4 ms on probes 4K against the 5300M's one-draw cells.

So the two GPUs disagree. On the target GPU one draw with cells is best wherever cells win. On the 1660 the split is worth 7.7 ms at e24 near 4K. The brief needs the split only where a read penalty exists: a per-backend choice, or a measured gate, never the default.

### Does the frustum cull earn its place?

**No, not on these views.**

| View | Cells removed | Effect on cell pass |
|---|---|---|
| near | 69% | Saves 0.009 ms of a 0.03 ms pass |
| far | 5% | Its own test costs more than it removes: +0.02–0.03 ms |

The cull only pays where large faces reach far off-screen with small cells. That is the sub-pixel regime where the cell path loses anyway. A per-face or CPU-side rect clip would get the near case's trim for free.

### How much survives the ratio forced to 1 (the precompute's world)?

| Comparison | live → r1 | Share surviving |
|---|---|---|
| probes 4K, one-draw cell vs frag | −4.62 → −2.86 ms | 62% |
| e24 near 4K, one-draw cell vs frag | −1.6 → noise | none |
| e24 near 4K, `cell2` vs frag | −9.37 → −7.99 ms | 85% |
| e24 near 4K, `frag2` vs frag | −7.27 → −5.99 ms | 82% |

At e24 near 4K, the ratio is worth 1.59 ms per fragment (22.24 → 20.65) and 0.2 ms in the cell paths. The ratio was image-neutral on e24 at capture time: its stored lights sit at their reference.

**The precompute's world still pays the split.** With the ratio at 1, per-fragment pixel style at e24 near 4K still costs 20.65 ms, 8.4 ms over reflection-free. The precompute and the Fresnel fade-out do not reach that cost; the split, or a cell read outside the lighting shader, does.

## Recommendation for the brief's third lever

1. **Ship the cell buffer behind a per-draw pixels-per-cell test.** Use cells at 2 or more px per cell, and the per-fragment path below 1. Here 0.03 ms of compute buys −1.6 to −4.6 ms in one draw; on the 5300M one-draw cells win 11–37% at 1.8 px per cell and above.
2. **Keep the split as a per-GPU option, not the default.** It draws the surface through the reflection-free pipeline and adds the reflection in a premultiplied overlay draw, exact up to blend rounding. On the 1660 it removes an 8.4 ms read penalty at e24 near 4K (`cell2` 12.87 against `cell` 20.62). On the 5300M there is no penalty, and it only adds its overlay (0.5–2.4 ms at 4K). Keep the atlas and per-face fit as built. The atlas is resident VRAM charged to the 2 GB budget: 15.8 MB for e24's 1.68M cells, 2.6 MB for the probes map.
3. **Drop the per-cell frustum cull.** Clip per face or per rect on the CPU if the dispatch ever grows.
4. **Keep the precompute.** It is worth 1.6 ms per fragment at e24 near 4K and 1.7 ms on probes. But in this content it is the smallest of the three levers.

## Open for consolidation

- Resolved: the 5300M's `free` arm and its run of this branch's arms show no read penalty on Metal (above).
- Resolved: the near-gate gap was animation time (see Honesty gates).
- Kit layout: both kits sit in `measurements/cell-rate-spike/`. The 5300M session plans to move its Metal kit to `metal/` at consolidation.
