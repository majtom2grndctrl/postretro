# reflection-probe-cell-path — research

Derivation behind the brief's decisions. Written at `a8c6e3300` before the split from `reflection-probe-fluid-pass-performance`; re-read at `c0dfd54b`. Post-landing costs and live lighting: `../../done/reflection-probe-fluid-pass-performance/research.md`. IP: `../../done/reflection-probe-fluid-pass-performance/ip-check.md` §Lever C.

## Evidence

**Cell-rate spike.** Branch `claude/cell-rate-spike` (debug only). **Deleted from origin by 2026-11-22**, with its findings, data and the 5300M implementation; the figures below survive only here. `claude/cell-rate-spike-1660` survives, with `findings-1660.md` and the 1660 implementation:
- Consolidated findings: `context/plans/drafts/reflection-probe-fluid-pass-performance--cell-rate-spike/findings.md`, with per-GPU notes `findings-5300m.md` and `findings-1660.md` beside it.
- Data: `measurements/cell-rate-spike/`.
- Two independent implementations: the 5300M branch (`3f4cabed5`) and `claude/cell-rate-spike-1660`.

**Blend-cost spike.** Branch `claude/blend-cost-spike`. Findings at `context/plans/drafts/reflection-probe-seams--blend-cost-spike/findings.md` on `claude/probe-streaming-brief-364a5e`; data in `measurements/reflection-blend-cost-spike/` there.

### 5300M, Metal System Trace, 4K, pixel style, reflectivity 1

Cost is Fluid Surface Pass + Fluid Reflection Cell Pass, ms per frame, mean of 2 interleaved runs.

| View | px per cell | per-fragment | cells (one draw) | reflection-free | per-fragment, ratio 1 |
|---|---|---|---|---|---|
| e24 near (straight down) | 127 | 65.88 | 55.21 | 56.25 | 64.71 |
| probes (straight down) | 225 | 36.51 | 23.08 | 22.92 | 31.32 |
| e24 mid (pitch −30) | 3.05 | 26.06 | 20.93 | | |
| e24 low (pitch −15) | 1.83 | 12.75 | 11.35 | | |
| e24 far (wing, pitch +5) | 0.40 | 3.33 | 4.39 | 2.79 | 3.25 |

At 1080p:
- mid 0.76 px/cell: 6.28 per-fragment against 7.00 cells.
- tilt 7.06 px/cell: 9.84 against 8.49.
- far 0.10 px/cell: 0.94 against 2.83.

Break-even is about 1 px per dispatched cell by pass cost, about 0.8 by whole-frame GPU time. The GTX 1660 run lands at about 1 by whole frame.

### What the spikes rule out on the 5300M

- **Split draw.** Surface reflection-free plus a premultiplied overlay.
  - The 1660 binary timed on the 5300M: `frag2` 66.98 against `frag` 66.67 at e24 near 4K, so the split recovers nothing.
  - `cell2` costs 0.5 ms over reflection-free at near and 2.4 ms over one-draw cells on probes.
  - The 1660 has a read penalty at e24 (8.4 ms at near 4K, found by replacing the cell read with a constant; the split recovers 7.7 ms) that the 5300M lacks.
- **Per-cell frustum cull.** Saves 0.03–0.24 ms, within run spread on every view.
- **Third pipeline variant for blend code.** The blend spike measured the reflection block's presence at 1.1–1.6%.

### The 1660 floor (`findings-1660.md` on `claude/cell-rate-spike`)

- **Break-even by content.** One-draw cells break even at about 1 px per cell by whole frame at the far views, 1.3–1.7 on the probes map (near-wall cells search longer), and 2.1–2.4 at e24 near, where the read penalty eats the saving. The brief keeps one cutoff, chosen on the 5300M; the 1660 manual row reports any view it slows.
- **With the ratio forced to 1** (the precompute's floor), e24 near 4K: frag 20.65, one-draw cell 21.22, `cell2` 12.66, free 12.24 ms. The cell path keeps none of its gain there, and per-fragment still costs 8.4 ms over reflection-free. Only the split recovers it. Far views are unchanged by the ratio (1.91 → 1.91 at 4K). On the probes map the cell path keeps 62% of its gain (−4.62 → −2.86 ms).
- **Read penalty mechanism unconfirmed.** The spike names occupancy as likely: one long-latency read live across e24's heavy per-fragment lighting. `fluid-base-pass-cost` moves only the baked terms per vertex; runtime lights stay per fragment, so the penalty may persist after it lands. A 5300M case of register count moving this pass's cost: §Findings from glossy-world-reflections.
- **Why a follow-up.** The split helps only the 1660 and costs the 5300M 0.5–2.4 ms, so shipping it needs a per-backend switch or a measured gate with its own 1660 proof (consolidated `findings.md` recommendation 4).

### Live lighting

Moved to `../../done/reflection-probe-fluid-pass-performance/research.md` §Evidence. The cell path's gain with the ratio forced to 1, the sums' floor: 88% kept at e24 near 4K, 62% on probes 4K.

## Findings from glossy-world-reflections (landed 2026-11-25)
Sources: `../../done/glossy-world-reflections/plan.md` (Corrections, Review panel 8, Owner checkpoint 2, Task 12); `measurements/fluid-base-pass-cost/results-glossy-task12-m6.md`, `results-glossy-review-fixes.md` (run 2); `tools/metal_register_screen/README.md`; commit `95b1372c7`.

- **Registers moved this pass's cost (5300M).** The branch routed water's reflecting entry `fs_main_reflecting` through the overlay's axis-generic hemisphere search and the six-lobe ratio. It rose from 72 to 79 VGPRs; under wave32's 8-register granule that is 14 → 12 waves per SIMD. The Fluid Surface Pass on e24 rose +3.9 % / +4.3 % at 1440p / 4K with byte-identical output (M6). Restoring the up-only search and five-lobe ratio together returned the entry to 72 VGPRs and the pass to +0.15 % / +0.24 % of the pre-branch build in a same-session A/B (review-fixes run 2). The two changes were not split, and the search relied on the compiler folding a constant +Y axis, which the count suggests it did not do in full. The reflection-free `fs_main` is 55 VGPRs. This shows occupancy matters for this pass on AMD Metal; it does not confirm the 1660's read-penalty mechanism. A new cell or target entry should be register-screened against 72.
- **Baseline holds.** Water's pass cost is back within 0.25 % of pre-branch (review-fixes run 2), so this draft's cost figures, which predate the branch, still stand.
- **Sums placement.** The sums now dispatch once per frame with any fluid or overlay probe-term draw, after the depth pre-pass and SDF compute and before world forward (`rendering_pipeline.md` §7.3a Dispatch). So P2's "either order" is now fixed, depth pre-pass then sums; a cell pass recorded after the sums and before the fluid pass meets it.
- **Adjacent defect, not in scope.** With 8 or fewer water texels per metre, pixel-style water's screen-space target held no hits; smooth was unaffected, and 16/m gave 232 hits against smooth's 394 (plan Corrections, Task 12a). Existing pixel-style water proofs run on the 64-texel placeholder. The glossy floor layer does not share it: at 1–5 texels per metre its pixel style keeps at least 80 % of smooth's confidence (`glossy_floor_grazing_frame_test.rs`). Suspected cause, unverified: water's pixel-cell code the floor lacks, such as its cell centre, the rim pull into the probe box or the per-cell normal tilt. This brief reworks pixel style's per-cell probe evaluation, but the defect is in the screen-space layer, which it keeps out of scope.
- **Possible later consumer.** The glossy overlay's pixel style also evaluates its probe term once per texel, on every fragment: the texel-centre point costs about 0.13 / 0.27 ms at 1440p / 4K on the floor-only fixture, across pose (plan checkpoint 2, cost-cut re-baseline). A cell path could later serve glossy floors, beside the mover consumer in §Screen-space rival. Out of scope; weigh it only if the chosen shape allows. Its search is the axis form, not water's.

## Early-out

Why the brief ships none. Measured on `claude/cell-rate-spike` (`3f4cabed5`), data in `measurements/cell-rate-spike/stress/`.

**Map.** `stress-env-volumes`, baked with its README's flags: 574 fluid volumes, 256 probes (the cap). 322 volumes bind no probe and already take the no-probe gate (§7.3a Off).

**Poses** (`poses.json`): steep, 3 m over a 16 m pool, pitch −89; cover, 1.2 m over the same pool, pitch −40, water about 80% of the frame from steep to grazing; long, the arena, a dozen pools at 3–60 m; graze, 1.6 m over a pool looking across two.

**Fresnel share.** Footprint = pixels where reflecting and reflection-free captures differ; weight = Schlick, F0 0.02, per pixel's view ray.

| Pose | Share F < 0.03 | Share F < 0.05 | Of those, changed > 2/255 |
|---|---|---|---|
| steep | 0.99 | 1.0 | 0 |
| cover | 0.33 | 0.45 | 1.3–1.6% / 3.5–3.9% (spot-lit wall reflections) |
| long | 0.05–0.10 | 0.25–0.31 | ~0 |
| graze | 0 | 0 | — |

**Cost at Auto (1536×960), reflecting − free, ms.** Metal System Trace, 2 interleaved runs per arm. Auto clocks float (670–1080 MHz mean per run) and drop trace rows, so these carry about ±0.5 ms; 4K deltas hold to 0.25 ms.

| Pose | smooth | pixel, per fragment | Upper-bound saving at F < 0.05 |
|---|---|---|---|
| steep | 1.11–1.24 | 1.38–1.52 | 1.1–1.5 |
| cover | 1.09 | 1.82–1.94 | 0.5–0.9 (inside run noise) |
| long | 0.25–0.47 | 0.57–0.60 | ≤ 0.2 |
| graze | 0.45–0.47 | 0.52–0.54 | 0 |

The bound counts the reflecting variant's slower shared lighting, which a skip cannot remove, and omits the test's own cost. Only a full-frame straight-down view clears the owner's 1 ms bar, and pixel style there takes the cell path, already within about 1 ms of reflection-free. Brightness-bounded skips are limited by the same spot-lit patches; distance skips have nothing to cut, since distant pools are seen near grazing.

At 4K the same bound is large (steep 8–9.7 ms, cover 2.5–4.3 ms), so a later brief whose bar is native resolution may revisit it, after `fluid-base-pass-cost` lands.

## Yardstick

The owner's bar (≥ 1 ms at Auto, nothing a player notices) was set for the early-out. Applied to the cell path on the same stress views at Auto (reflecting − free, ms, `measurements/cell-rate-spike/stress/armtable.json`):

| Pose | per fragment | cells | saving | whole frame, frag → cells |
|---|---|---|---|---|
| steep | 1.38–1.52 | 0.96–1.22 | 0.3–0.6 | 16.9 → 16.3 |
| cover | 1.82–1.94 | 0.94–1.48 | 0.4–0.9 | 15.1 → 13.8 |

At 4K the same views save 8.8 ms (steep) and 8.9 ms (cover). Pixels per cell scale with pixel count: Auto on the 16-inch MacBook is 0.18× the pixels of 4K, so at Auto e24 mid (about 0.54 px per cell) and low (about 0.32) fall to per-fragment and only steep close views take the cell path. 1440p, Auto's row cap on large displays, is 0.44× of 4K; scaling the 4K saving by pixel count gives about 4 ms at steep, an estimate, not measured.

The cell path ships on parity, not on the Auto figure: it is within 2/255 of per-fragment except in cells per-fragment itself splits. The early-out changed 1.3–3.9% of skipped pixels at cover by more than 2/255.

## Hidden cells

Room culling tests each fluid volume's bounds against the visible cells' bounds (§7.3a Culling and order), and the visible set is a conservative superset (§2). The fluid draw depth-tests against the pre-pass (`fluid_depth_stencil_state`: `Less`, no depth write, no `discard` in the shader), so hidden fragments are rejected per pixel. A cell compute pass has no such test.

The spike's level-scoped atlas showed the waste at the graze pose: 110 reflecting draws dispatched 13.9M cells, and the cell arm cost 14 ms over reflection-free against 0.5 ms per fragment. The brief's screen clip and path choice remove most of that (0.06 px per cell sends graze per-fragment). A hidden face projecting large still takes the cell path, which the depth skip covers.

## Cell path

- **Same-frame, not history.** The cell storage is written by a compute pass before the fluid pass and read by it in the same frame. A pose jump renders identically to a fresh pose (spike gate). The done brief's grep gate bans previous-frame bindings and precomputed intersection tables; this is neither.
- **Fade decision.** The cell pass decides each cell's fade once, at its centre, from analytic texels per pixel. The per-fragment path decides it per fragment, so a cell straddling its threshold splits there and not in the cell path.
- **Parity exceptions.** Both spikes found the same set: cells the per-fragment path itself draws in two values, because the derivative-estimated cell centre lands across a discontinuity in the search or shadow fade, or across the fade threshold. Exceptions appear only at nonzero animation times at e24 near: up to 369 px over 2/255 at 4K, max 8–9.
- **Encoder overlap.** On Metal the cell pass overlaps other encoders for 96–99% of its interval, so its interval is shared time. Report whole-frame GPU time beside pass cost.

## Cell storage sizing

**The spike's level-scoped atlas** held one RGBA16F texel per cell of every probe-bound face, shelf-packed at install:

| Map | Atlas | Bytes | Cells |
|---|---|---|---|
| e24 (17 faces, including a 0.25-scale pool) | 1360×1472 | 16.0 MB | about 1.7M |
| reflection-probes-test | 512×640 | 2.6 MB | |

It grows quadratically with texture density, so a fine-scale lake would force a cap, leaving whole faces per fragment for the level's life.

**The chosen per-frame pool** holds only the cell-path draws' screen-clipped rects. A draw takes the cell path at the cutoff c px per cell or more, so visible, non-overlapping cell-path water needs at most pixels ÷ c cells. That is a sizing rule, not a ceiling: the CPU estimate counts faces behind walls and behind other water (the collector culls by visible-cell bounds only, and fluid faces are two-sided), so a frame can need more, and the draws that do not fit take the per-fragment path. Packing nearest first spends the pool where cells are largest on screen.

| Scene extent | Pixels | Cells at c = 2 | Bytes at 8 B | Cells at c = 1 | Bytes |
|---|---|---|---|---|---|
| Auto on the 16-inch MacBook, 1536×960 | 1.47M | 0.74M | 5.9 MB | 1.47M | 11.8 MB |
| 2560×1440 (Auto's row cap) | 3.69M | 1.84M | 14.7 MB | 3.69M | 29.5 MB |
| Native 3840×2160 | 8.29M | 4.15M | 33.2 MB | 8.29M | 66.4 MB |

Hysteresis keeps a draw on the cell path down to the cutoff minus its margin, so even visible water can exceed pixels ÷ c; overflow absorbs it, or the executor sizes from c minus the margin. Repacking every frame costs CPU shelf packing over the drawn faces only.

## Screen-space rival

Raised by `/validate-plan` (2026-11-22), unmeasured.
- **Shape:** a pass after the depth pre-pass redraws the cell-path draws into a probe-term target at a fraction of the scene extent. Every target pixel of a cell evaluates that cell's ray, as the SSR pass does in pixel style (§7.3a Rays). The fluid fragment reads the target nearest at its cell centre's projection.
- **Exactness:** a cell covering several target pixels reads its own value. A cell smaller than about one target pixel can read a neighbour's, so the per-draw path choice must send such draws per fragment. At ½ that is under about 4 screen pixels per cell; at ¼, under about 16.
- **Cost model:** at least one evaluation per target pixel, not per cell. Near views (127–225 px per cell at 4K, about 23 at Auto) evaluate a few times per cell rather than once. Hidden water costs nothing, because raster depth rejects it.
- **IP:** the reduced-resolution screen target is the class the shipped SSR layer already sits in (Killzone Shadow Fall, 2014, half-resolution reflection; `plans/done/fluid-ssr-reflections/ip-check.md`). The texture-space filings that gate the cell pool (Qualcomm '166, EP4409531B1, '715; Intel '475) need a texel-space atlas or mask it lacks. It still needs its own claims check before it ships.
  - **Claims check (2026-11-22):** `../../done/reflection-probe-fluid-pass-performance/ip-check.md` §Shape B. No filing found reads on this shape as Qualcomm '166 reads on the cell pool. Each reduced-resolution family found needs something the shape lacks: lobe-weighted upsampling (Square Enix US9342926B2), coarse quads in the rasterizer (AMD US10510185B2), per-region resolution metadata (Sony US9710881B2), execution-control buffers (Microsoft US10235799B2), or a direction test (Microsoft US10360720B1). Its raster depth test is ordinary depth testing in screen space, not a texture-domain mask (Intel '475). Fences if chosen: the target's fraction comes from the tier, never a per-draw or per-region estimate; one nearest load at the cell centre, no weighted upsampling; fallback per draw only; the probe term and SSR stay separate stored quantities, with s never in SSR's share (Tencent WO claim 15); the probe lookup keeps its own search (NetEase CN114748874B). Intel's stencil-set coarse-pixel patent (US12243125B2) appears absent; the rest of Intel's coarse-pixel family is unread at claim level. Counsel still reviews before ship.

**Second texel-rate consumer, for the shape choice.** `mover-smooth-specular-pipeline` (landed) measured the mover pass at 4K at 18.7–19.2 ms in smooth and 21.0–23.1 ms in pixel. Pixel's extra is evaluating lights at each texel's point per fragment (`rendering_pipeline.md` §12, Mover specular cost). That is the same per-pixel evaluation of a per-texel value as this brief's. A texel pool could someday serve it; a screen-space probe target could not. The texel pool's IP class is the same either way. Weigh it only if a brief proposes that consumer.

## Shape measurements

Not yet run. The brief's Open questions list the rows and the decision rule.

## Measurement fixture

| Item | Setting |
|---|---|
| Machine | 16-inch MacBook Pro, Radeon Pro 5300M, macOS, Metal; mains power, caffeinate, no GPU-heavy apps |
| Maps | `e24-environment-test`, `reflection-probes-test`, baked with `--no-cache` |
| Poses | near, probes, mid, far: engine coordinates in `findings-5300m.md` §Views |
| Capture | `--capture` at 3840×2160 (and 1920×1080 for mid and far), 240 warmup + 240 sampled frames, `POSTRETRO_SH_STREAMING=sync-proof` |
| Kit | `measurements/fluid-base-pass-cost/metal/` on main, whose `run.py` and `gpu_time.py` copy the deleted cell-rate kit: Metal System Trace, complete-frame reduction, two-binary A/B. Arms under mask `0x17F` select the reflecting variant |
| Metric | Fluid Surface Pass + any cell or live-lighting sums pass per frame, with whole-frame GPU time beside it |
| Baseline | The reflection-free arm of the same binary, and the build before with the live-lighting sums landed |
| Runs | 2 interleaved per arm |
| Cleanup | Traces and `$TMPDIR` ktrace files deleted after export |
| Auto rows | `--capture` at 1536×960; report each run's mean core clock beside its cost, since Auto clocks float (about ±0.5 ms) |
| Stress rows | `stress-env-volumes` baked with its README's flags. The steep, cover, long and graze poses (§Early-out) went with the deleted branch's `stress/poses.json`; re-record them, plus a hidden-face pose. The 256-probe cap leaves most pools probe-free |
| 1660 rows | GTX 1660 Super, Windows, Vulkan, engine timestamps via `measurements/cell-rate-spike/run_timestamps.py` on `claude/cell-rate-spike-1660`; baseline the reflection-free arm of the same binary; 2 interleaved runs |
| Pilot | One run per pose × resolution before a batch, reduced at once; halt on an unclassified invalid run |

The engine binary must be named `postretro`, because the reduction filters trace rows by process name.

## Pins

Orderings the Acceptance rows cite.

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| P1 | Moved: the sibling's P1 | — | — |
| P2 | Any frame with a cell-path draw | the frame's pack and face records written; depth pre-pass and the live-lighting sums pass, either order; then the cell pass; fluid pass; one submission | The cell pass sees this frame's depth and sums; both consumers see one sums pass |
| P3 | All per-fragment, all overflowed or smooth; N cell-path draws | — | No cell dispatch; one cell dispatch with disjoint rects |
| P4 | Moved: the sibling's P3 | — | — |
| P5 | A cell skipped or overflowed on N; a face hidden on N−1 and visible on N; a camera jump | the skip reads N's depth | No fragment reads a record N's cell pass did not write; N matches a fresh capture |
| P6 | Extent change on N, with or without overflow | pool rebuilt before N's pack | N packs, clips and judges overflow against the new extent |
| P7 | Install and extent change, either order on one frame; extent change with no fluid level | — | One pool at the current extent; none without fluid |
| P8 | Overflow; unload with the pool live | pack nearest first; clear, then the next frame | The same draws fall back at a fixed view; pool released, fluid pass still bindable |
| P9 | Estimate oscillating across the cutoff | hysteresis margin | Stable path at a fixed view; one switch each way on a slow sweep |
| P10 | Moved: the sibling's P4 | — | — |
| P11 | Style switched by reload between N−1 and N | path choice and shading read one style on N | A smooth frame dispatches no cells; a pixel frame takes the cell path where chosen |
| P12 | Two draws swap far-to-near order on N at fixed estimates | path state keyed to the volume, not the frame's draw order | Each keeps its path |
| P13 | Style switched to pixel on N on a level installed under smooth, including the boot style commit; or switched to smooth with the pool live | pool allocated or released at the style commit, before N's pack | N takes the cell path where chosen; no pool while smooth |
| P14 | Level A unloaded, level B installed, frame N | path state dropped at unload | Every B volume takes the cutoff's side |
