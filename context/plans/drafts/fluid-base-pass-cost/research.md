# fluid-base-pass-cost — research

Read at `b0a5c0f58`. Kit, per-run records and results: `measurements/fluid-base-pass-cost/` (`results-ablation{1,2,3}.{md,json}`, `metal/runs/`).

## Method

- **Machine:** 16-inch MacBook Pro, AMD Radeon Pro 5300M (4 GB), Metal, mains power, caffeinate held. No `cargo`, `rustc`, `prl-build` or second engine during any run (the kit checks every 2 s and marks a run invalid otherwise; every run below is valid).
- **Recording:** Metal System Trace of one headless `--capture` per run, 120 warmup + 120 sampled frames, `POSTRETRO_SH_STREAMING=sync-proof`. The complete-frame reduction of `claude/cell-rate-spike`'s kit (`rendering_pipeline.md` §12). Two interleaved rounds per arm (forward, then reversed), 15 s idle before each launch.
- **Arms:** one engine binary built from `b0a5c0f58` plus an uncommitted shader-override hook (`measurements/fluid-base-pass-cost/metal/experiment-hook.patch`). Each arm loads a whole fluid shader module with one textual edit. `stock` (no override) and `base` (override, unedited) agree within 0.02 ms.
- **Variant selection:** every arm runs light mask `0x13F` (all terms but specular), which selects the reflection-free pipeline. Capture draws water at opacity 0.6 with no fog, drift or wave, and placeholder textures.
- **Poses:** `run.py`'s. `near` = e24 still pool from 1.6 m straight down (−4.8768, 1.6, −44.2976; pitch −89), one draw covering the full screen. `test` = probes-map pool straight down (−6.5024, 1.6, −6.5024; pitch −89).
- **Clock:** 4K held 1150–1230 MHz busy median in every run; 1080p sat lower (940–1140 MHz), so 1080p rows show direction, not exact ratios.

## Attribution (reflection-free fluid pass, ms per frame)

Mean of 2 runs; spread is max − min. Whole frame is the union of every labelled GPU interval per complete frame.

| Arm | What it removes | near 4K fluid (spread) | near 4K whole frame | probes 4K fluid (spread) | probes 4K whole frame |
|---|---|---|---|---|---|
| stock | nothing (no override) | 55.44 (0.04) | 81.21 | | |
| base | nothing | 55.43 (0.06) | 81.12 | 22.28 (0.01) | 51.26 |
| nosh | the SH sample (both baked terms read 0) | **2.05** (0.00) | 27.95 | **2.05** (0.01) | 25.68 |
| shindirect | the baked direct SH atlas (indirect only) | 26.80 (0.04) | 52.53 | 12.95 (0.01) | 40.75 |
| nodyn | the runtime light loops | 53.18 (0.55) | 78.88 | 20.96 (0.02) | 48.76 |
| nowarp | whole-texel motion | 53.42 (0.11) | 79.05 | 20.76 (0.14) | 47.67 |
| nofog | the depth-fog block (never runs in capture) | 54.95 (0.37) | 80.57 | | |
| albedo | everything after the warped albedo sample | 1.90 (0.00) | 27.57 | 1.91 (0.01) | 25.69 |
| flat | everything: constant colour | 0.44 (0.00) | 26.17 | 0.44 (0.00) | 24.17 |
| **vertexsh** | SH moved to `vs_main` on today's coarse mesh | **2.11** (0.01) | **27.99** | **2.12** (0.00) | **25.74** |

Batches: ablation1 (all but the last row), ablation3 (vertexsh, with its own base: 55.02 near, 22.27 probes). Textured Pass at the near pose: 23.8 ms; at the probes pose: 19.2–20.5 ms.

1080p, near (ablation3): base 13.92 ms (whole frame 20.95), vertexsh 0.58 ms (whole frame 7.85).

**Reading.**
- The probe sampler is 96% of the pass at e24 near (53.4 of 55.4 ms) and 91% on the probes map (20.2 of 22.3).
- The fluid samples two atlases per fragment (indirect and baked direct SH, `fluid_sample_sh` → `sample_sh_indirect_direct_corners`), and the second costs as much as the first: 28.6 + 24.8 ms at near, 9.3 + 10.9 on the probes map. The world forward pass samples one atlas and takes baked direct from the lightmap. That is why one water plane costs about twice the forward pass.
- Everything else together is about 2 ms at full-screen 4K: warp ~2 ms, light loops ~1.3–2.3 ms (no runtime light reaches either pose), fog code shape ≤ 0.5 ms (within spread), raster and blend 0.44 ms.
- No evidence for the other leads: overdraw is one layer (the cell-rate spike's occlusion query counted exactly the screen's pixels at near), and blending is in the 0.44 ms floor.

## The sampler's path does not explain it

Image diagnostics `shpath` / `shpathfloor` colour each pixel by `sh_sample.wgsl`'s path:
- **e24 near:** the water's centre takes the L1 whole-cell path (16 stored-tile taps for two atlases plus a 64-weight reconstruction per atlas); its sides take the per-corner path with mostly L1 corners. The floor behind takes the per-corner L0 path almost everywhere.
- **probes map:** the water is per-corner L0 everywhere, the cheapest path, and the sampler still costs 20 ms at full-screen 4K.

So the cost is not one pathological path. The per-fragment sampler is expensive on this GPU even at its cheapest: about 11 ms per atlas at full-screen 4K on the L0 path.

## Code reshapes are a lottery on this compiler

| Arm | Change | near 4K fluid | probes 4K fluid |
|---|---|---|---|
| base | — | 55.33 | 22.31 |
| shshared | indirect and direct share every weight, indirection load and moment load (bit-identical image) | 53.48 (−1.9) | 26.83 (**+4.5**) |
| noocc | probe-depth visibility off: strictly less work | 73.33 (**+18.0**) | 33.11 (**+10.8**) |

ablation2, 2 runs each, spread ≤ 0.6 ms. Removing work made the pass up to 48% slower. That matches §7.3's Surface Depth note: on AMD's Metal compiler the code shape of a large fragment shader moves its register allocation, and with it occupancy, more than its operation count does. A tighter SH sampler is therefore no reliable lever here: each variant is a fresh draw from the compiler, and an all-backends shader change measured only on one compiler can regress another. `shshared` also moved the Textured Pass by +2 ms in the same frame (whole frame +1.7 ms).

## Lower-rate evaluation

- **Precedent in this repo.** The billboard pass hoists SH indirect and direct into `vs_main` (`billboard.wgsl` `vs_main`, `sh_bind_group_layout_entries`' VERTEX visibility, §7.4). Group 3's SH bindings the fluid reads are already VERTEX-visible: the `vertexsh` pipeline validated and drew.
- **Established technique.** Translucent surfaces lit from irradiance volumes or probe grids are commonly lit per vertex or per object, not per pixel, because the field varies at probe spacing (light-probe-lit transparents and particles in shipped engines; Valve's ambient cube per vertex). The SH field here varies at the probe spacing (about 1 m), trilinear between probes, with probe-depth visibility added.
- **Cost probe.** `vertexsh` moves the whole baked term to `vs_main` on today's mesh (face fans, so a few vertices per face). The pass falls to the `nosh` floor: 55.0 → 2.1 ms at near 4K, 22.3 → 2.1 on the probes map, 13.9 → 0.58 at near 1080p. A grid adds vertex work proportional to its vertex count; at sub-probe spacing that is thousands of vertices per level, not millions of fragments.
- **Look on the coarse mesh.** Against per-fragment, at 960×540: near max 14/255, probes max 15/255, mid (pitch −30) max 19/255, mean over the whole image 2.4–4.8. That is faceting across face-sized triangles; a sub-probe grid is what removes it. Parity on the grid is the build's proof, not yet measured.

## Rivals

| Shape | For | Against |
|---|---|---|
| Per-vertex baked SH on an install-time world-lattice grid (chosen) | Billboard precedent; no new pass, binding or persistent GPU state; all backends; reversible | Look change bounded by grid spacing; dynamic lights must stay per fragment (E24's no-faceting row) |
| Texel-cell lighting in the reflection brief's cell pool | One per-cell evaluation point for fluid; matches the "grain is the texture" look | New compute pass and per-frame pool, couples to an unbuilt brief; lighting blocks per texel unless filtered; cells below 1 px cost more than per fragment (cell-rate spike) |
| Half-resolution fluid lighting target with upsample | Independent of mesh | New targets and pass; depth-aware upsample over translucency; edge artefacts at pool walls |
| Tighter per-fragment SH sampler | No look change | Measured lottery above; cannot be validated off the 5300M |
| Per-vertex on today's face fans, no grid (the `vertexsh` arm) | No grid, cap or shared-edge proof; measured 2.1 ms | Measured faceting across face-sized triangles: max 14–19/255 at 960×540 (§Lower-rate evaluation) |
| Per-face constant baked term | Cheapest | Visible: a pool lit by one value; fails "matches baked lighting on adjacent walls" (E24) |
| Compile-time tessellation in prl-build | prl-build already closes T-junctions, splits faces at 2 mm and knows the SH lattice; a cap warning reaches the mapper at build time | Changes the PRL format; freezes sample density into baked data, so retuning needs a rebake; couples a renderer quality choice to the format |
| World-space irradiance texture per fluid face, written by compute when SH compose runs | No mesh change, no T-junction proof, no vertex cap; near free while lights are static | New pass, new fragment binding (fluid fragment storage is spent, §10), persistent VRAM, chart gutters; refreshes every frame while any animated light runs; falls in the texture-space shading class the reflection brief already sends to IP review, where a per-vertex grid is long-standing practice |

## Grid notes for the executor

- **Shared edges.** The compiler splits fluid faces at 2 mm depth and inserts collinear vertices to close T-junctions (§7.3a Triangulation). A grid cut per face would put new vertices on a neighbour's edge that the neighbour lacks: a T-junction, a crack. Lattice planes fixed in world space cut a shared edge at the same points from both faces, provided both compute each crossing the same way (for example, from the edge's lexicographically lower endpoint).
- **Lattice alignment.** Planes on the SH probe lattice (and its subdivisions) keep each sub-polygon inside one probe cell, where the field is closest to linear.
- **Streams.** Drift (location 3) and probe index (location 4) are per vertex and written per face; a face's grid vertices must stay contiguous so its ranges still cover them. The reflection cell-rate spike's per-face affine fit reads face vertices; extra vertices on the plane do not change it.
- **Which lattice.** Align to the base (finest) SH lattice. Scaled adaptive nodes (§4) reconstruct L1 across a 2× or 4× node edge, so base-lattice planes over-sample them, which is harmless; do not align to node boundaries.
- **Code shape.** §7.3's Surface Depth note (AMD Metal register allocation) applies to the new `vs_main` and to every later `fs_main` edit: re-measure the reflection-free pass on the 5300M when either changes.
- **Bound.** A mapper can author a huge ocean face. Vertex count must not scale without bound; coarsen the spacing past a per-level cap and warn, as other install-time bounds do (`development_guide.md` §1.4).

## Wider finding

The world forward pass runs the same per-fragment sampler for SH indirect. At the near pose its floor takes the cheapest path, and the whole Textured Pass is 23.8 ms at 4K. Its SH share was not attributed here; it is a candidate for its own measurement.

## Interaction with `reflection-probe-fluid-pass-performance`

That draft is not merged: it lives on `claude/reflection-perf-brief`. Its Decisions say the reflection-free variant "keeps today's cost and image"; this brief changes both, so that line needs a coordination edit when either lands.

That draft's done-row is "reflecting water within 1 ms of reflection-free". Its reflecting variant runs the same base lighting, so this brief lowers both variants' floor by the same ~53 ms at near 4K. After it lands, the reflection term (10–14 ms at full-screen 4K) is most of the reflecting pass, and that brief's levers carry more of the frame.
- **GTX 1660 read penalty.** On the 1660 the reflecting variant pays ~8.4 ms at e24 near 4K for one long-latency reflection read held across the heavy per-fragment lighting it shares a shader with (`origin/claude/cell-rate-spike`, `findings-1660.md` items 4–7). That penalty is the whole case for the reflection brief's deferred split draw. Moving the baked lighting out of `fs_main` plausibly removes it, so the split-draw question should be measured after this lands; this brief's 1660 row records the reflecting-minus-free difference at e24 near for that reason.
- **Per-frame face records.** The reflection brief's candidate "per-frame rewrite of the drawn faces' vertex words" would scale with this grid's vertex count, not the face count. Its other candidate, a uniform table keyed by face index, is unaffected. A coordination note for that draft.

## Foreclosure and neighbours

- **Normal-perturbing water.** Per-vertex baked light cannot respond to a per-pixel normal. Today's wave is a UV warp with no height, so nothing is lost now; a future normal-mapped or height-wave water re-opens per-fragment baked SH, or interpolates per vertex and corrects per fragment.
- **`fluid-ssr-reflections` fallback.** That draft names Fresnel-weighted reads from the baked SH probes as its fallback. Built per fragment, it would put back the cost this brief removes.

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | level install | SH volume (PRL section or streamed) installs inside geometry install, then fluid install reads its lattice | fluid packs on this level's lattice |
| P2 | level change, SH level to one with another lattice or none | geometry install clears fluid and replaces SH, then fluid install | the second level packs on its own lattice, or ungridded; nothing carries over |
| P3 | streamed-SH level | streamed state initialises after the legacy SH resources, which hold the placeholder lattice (origin 0, 1 m) | grid on the manifest base lattice, never the placeholder |
| P4 | level with no SH volume, or one the device rejects | SH resources fall to the placeholder | packs as today; ambient floor only; no error |
| P5 | level at or under the cap | count before pack | chosen spacing; no log |
| P6 | level past the cap, overflow reached only by the last face, which shares an edge with the first | count every face before packing any | one coarser spacing for every face; shared edge identical; under the cap; one log |
| P7 | level whose ungridded vertices alone exceed the cap | coarsening has no spacing left | packs as today; terminates; one log |
| P8 | face spanning several probe cells | cut by every lattice family crossing it | no sub-polygon wider than one spacing on any lattice axis |
| P9 | edge shared by a top face and a side face, and by a sloped face | each face cuts by every lattice family crossing the edge | identical vertices along the edge from both faces |
| P10 | original vertex exactly on a lattice plane | crossing coincides with a corner | no duplicate vertex; no zero-area triangle |
| P11 | face edge lying in a lattice plane | plane is parallel to and on the edge | no vertex added along it; no sliver strip |
| P12 | crossing within the collinear tolerance of an original vertex on a shared edge | snap decided from the edge alone | both faces snap the same way |
| P13 | edge with an open T-junction today (burial, or between volumes): face A's edge spans two of face B's segments | each face cuts its own edges | crossings agree on both sides; the existing T-vertex stays; no new one |
| P14 | level with no fluid | install returns before packing | no grid, no cap log |
| P15 | drift re-resolve (script or fluid table reload) | drift rewrite runs over the packed mesh | grid untouched; drift stream covers every grid vertex |
| P16 | camera below, then crossing, the water plane | per-vertex and per-fragment terms read one camera position per frame | baked term takes the viewer's side the same frame dynamic lights do |
