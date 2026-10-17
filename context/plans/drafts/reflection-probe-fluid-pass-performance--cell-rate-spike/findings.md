# Cell-rate spike findings (consolidated)

> **Spike:** pixel-style fluid reflections at cell rate. It informs the third lever of the draft brief "reflection-probe fluid-pass performance": does a same-frame cell buffer pay off, and under what conditions?
> **Two runs, two independent implementations:**
> - **Radeon Pro 5300M** (the target GPU): macOS, Metal, Metal System Trace. Branch `claude/cell-rate-spike`, engine `3f4cabed5`. Detail in `findings-5300m.md`.
> - **GTX 1660 Super:** Windows 11, Vulkan, engine GPU timestamps. Branch `claude/cell-rate-spike-1660`, engine `65ca0fbb3` + `df5dd7e99`, findings at `f63f94cd4`. Detail in `findings-1660.md`.
>
> Both branches start from `a8c6e3300`. Debug only; neither is merged.
> **Data:** `measurements/cell-rate-spike/`:
> - `results-5300m.{md,json}`, `results-5300m-impl1660.{md,json}` (the 1660 binary timed on the 5300M), and `metal/` (that kit, per-run JSON, gates).
> - `results-1660.{md,json}`, `gates-1660.{md,json}`, `gate-split-cells.json`, `runs/` (1660 per-run JSON) and `1660/` (that kit).
>
> **Reading the columns:** the GPUs are kept in separate columns. The rules come from the 5300M; the 1660 shows how the costs move on another GPU and API.

## Recommendation for the brief's third lever

1. **Ship a one-draw cell buffer behind a per-draw pixels-per-cell test.**
   - A draw takes the cell path at 2 or more visible pixels per dispatched cell, and the per-fragment path below 1. Between 1 and 2 the choice is within about 10% either way.
   - On the 5300M, every measured view at 1.8 px per cell or more wins 11–37%, and every view at 0.76 or less loses (up to 3× at 0.1).
   - For a face seen square-on, pixels per cell is about 1 / texels-per-pixel². A per-draw estimate from projected area over cell count is enough.
   - Keep the atlas and the per-face affine fit as built. The atlas is resident VRAM charged to the 2 GB budget: about 16 MB for e24's 17 faces, 2.6 MB for the probes map.
2. **Drop the per-cell frustum cull.** On the 5300M it saves less than run noise on every view; on the 1660 it sometimes loses. Clip per face or per rect on the CPU if the dispatch ever grows.
3. **Keep the live-lighting ratio precompute.** The cell path keeps 62–88% of its gain with the ratio forced to 1 on both GPUs. On the 5300M the ratio is worth 1.2 ms per fragment at e24 near 4K and 5.2 ms on the probes map.
4. **The split draw only where a measured read penalty exists.** The split draws the surface reflection-free and adds the reflection in a premultiplied overlay.
   - The 1660 has a penalty at e24: the split is worth 7.7 ms at near 4K.
   - The 5300M has none: the split recovers nothing and costs 0.5–2.4 ms at 4K.
   - That needs a per-backend switch or a measured gate, and is likely a follow-up spike rather than part of the brief.
5. **Report whole-frame GPU time beside pass cost** for the cell path in any future measurement. The cell pass overlaps other encoders on Metal, so its own interval is shared GPU time.

## Results side by side

Cost is Fluid Surface Pass + Fluid Reflection Cell Pass in ms per frame, mean of two interleaved runs, ratio live. Pixel style at reflectivity 1.
- `frag`: per-fragment pixel style.
- `cells`: one-draw cell buffer.
- `cell2`: the split with cells (1660 implementation).
- `free`: the reflection-free variant.

Each run measured its own implementation; the 5300M also timed the 1660 binary (last table).

| View | 5300M frag | 5300M cells | 5300M free | 1660 frag | 1660 cell | 1660 cell2 | 1660 free |
|---|---|---|---|---|---|---|---|
| e24 near 4K (127 px/cell) | 65.88 | **55.21** | 56.25 | 22.24 | 20.62 | **12.87** | 12.24 |
| e24 near 1080p (32) | 16.55 | **14.07** | 14.28 | 5.73 | 5.33 | **3.50** | 3.11 |
| probes 4K (225) | 36.51 | **23.08** | 22.92 | 10.85 | **6.23** | 7.14 | 6.48 |
| far 4K (5300M 0.40 / 1660 0.71) | **3.33** | 4.39 | 2.79 | **1.91**† | 1.96 | 2.23 | 1.38 |
| far 1080p (0.10 / 0.18) | **0.94** | 2.83 | 0.89 | **0.57** | 0.81 | 0.93 | 0.42 |

- **The far views differ between runs:** each run chose its own far pose (see the platform findings).
- **†** By whole frame the 1660's one-draw cell loses at far 4K (+0.19 ms), so per-fragment is best there.
- **The 5300M's `free`** is the lighting mask without the specular bit. The 1660's is a lever that forces the reflection-free pipeline under the full mask; the 1660 measured both at 12.4 ms at near 4K.

**The 5300M's mid-range poses** set the threshold (5300M only):

| px per cell | View | frag | cells | cells / frag |
|---|---|---|---|---|
| 0.76 | e24 mid 1080p | 6.28 | 7.00 | 1.11× |
| 1.83 | e24 low 4K | 12.75 | 11.35 | 0.89× |
| 3.05 | e24 mid 4K | 26.06 | 20.93 | 0.80× |
| 7.06 | e24 tilt 1080p | 9.84 | 8.49 | 0.86× |

**The 1660 binary timed on the 5300M** (same Metal kit, ms live / r1):

| View | frag | cell2 | frag2 | free |
|---|---|---|---|---|
| e24 near 4K | 66.67 / 65.76 | 57.88 / 58.04 | 66.98 / 66.43 | 57.37 |
| probes 4K | 36.76 / 31.54 | 25.51 / 25.26 | 35.75 / 32.05 | 23.13 |

## What the two GPUs agree on

- **Break-even is about 1 px per cell.**
  - 5300M: between 0.76 and 1.83 by pass cost; about 0.8 by whole frame.
  - 1660: 0.5–0.8 by pass cost, about 1 by whole frame.
  - The 1660's e24 near figure (2.1–2.4) is inflated by its read penalty.
  - The rule "cells at 2 or more, per-fragment below 1" holds on both.
- **The cell pass is cheap where it wins.** It costs 0.19 ms on the 5300M and 0.03 ms on the 1660 at e24 near. It grows with cell count: 1.3–2.0 ms on the 5300M and about 0.4–0.5 ms on the 1660 for 1.3–1.4M cells.
- **The per-cell frustum cull does not earn its place.**
- **Most of the gain survives the ratio forced to 1:**
  - 5300M one-draw cells: 88% at e24 near, 62% on the probes map.
  - 1660: 62% for one-draw cells on the probes map, 82–85% for the split forms at e24.
- **The honesty gates pass on both, with the same exceptions.**
  - Smooth style, the reflection-free variant and lever-off pixel style are bit-identical to main.
  - No stale cells.
  - Cell vs per-fragment pixel style is within 2/255 except in cells the per-fragment path itself splits:
    - the derivative-estimated cell centre landing across a discontinuity in the search or shadow fade;
    - cells at their fade threshold, mainly at far.
  - On the probes map both show 0 pixels over 2/255.
  - At e24 near 4K the 5300M shows 164–369 px over at nonzero animation times (max 8); the 1660 shows 840 (max 9).

## Where they disagree

| Question | 5300M (Metal) | 1660 (Vulkan) |
|---|---|---|
| Does one-draw cells reach reflection-free cost at e24? | Yes: 55.2 against 56.3 ms | No: 20.6 against 12.2 ms |
| Read penalty inside the heavy lighting shader | None: `frag2` saves nothing | About 8.4 ms at e24 near 4K; a constant in place of the atlas read removes it |
| Best path at e24 near 4K | One-draw cells (−16%) | Split with cells, `cell2` (−42%) |
| Cost of the split's overlay at 4K | 0.5–2.4 ms, no benefit | About 0.6 ms, worth it at e24, overhead on probes |
| Absolute fluid cost | About 3× the 1660's | |

The split stays a per-GPU option. Shipping it by default would cost the target GPU 0.5–2.4 ms per full-screen 4K draw for nothing.

## Measurement caveats

- **Encoder overlap (5300M).** The cell pass runs concurrently with other encoders for 96–99% of its interval: beside the Depth Pre-Pass on e24, and alongside the 22 ms Textured Pass on the probes map.
  - Its interval is shared GPU time. The probes map's apparent 9 ns per cell, against e24's 1.3, is contention, not costlier cells.
  - The overlapped pass can also slow down: at far 4K the Textured Pass grows about 1 ms with cells on.
  - On Metal the whole frame favours cells slightly more than the cost metric does.
  - The 1660 checked with median CPU completion time (capture waits for the GPU every frame). There the whole frame favours cells slightly less at far, partly from the CPU's per-frame tile-list build.
- **Animation time.** A plain capture renders at time 0, where the 5300M's e24 near gate showed 0 exceptions. A capture with a measurement block renders at its last step, where both runs see the split-cell exceptions. Parity gates should sample several times.
- **Noise.** 5300M: runs within an arm agree to 0.5 ms at 4K and 0.3 ms elsewhere. Cross-build differences under about 1 ms are noise (the 1660 binary's `frag` reads 0.8 ms above the 5300M implementation's at e24 near 4K). 1660: near-4K differences under about 0.8 ms are noise between arms.
- **FXC.** The 1660 branch's DX12 FXC test passes for its 7 render and 2 compute pipelines. The 5300M branch's extension to its 8 lever combinations has never been compiled; it needs a Windows run if that implementation goes further.
- **Discarded.** The 5300M's reduced-resolution probes runs sat at 360–800 MHz with dropped trace rows; they are not used.

## Open

- The reflection-free fluid pass itself is the largest cost either run saw on the 5300M: 56 ms full-screen at e24 near 4K, 23 ms on the probes map. It is outside this spike and is the next thing to measure on the target GPU.
- Why the 1660 pays a read penalty at e24 and the 5300M does not: occupancy is the likely mechanism on the 1660, and it is unmeasured. Only matters if the split ships as a per-GPU option.
