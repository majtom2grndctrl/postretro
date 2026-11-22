# reflection-probe-cell-path

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §2, §7.3a, §10; `context/lib/resource_management.md` §5.3; `context/lib/testing_guide.md` §Resource bounds · read at c0dfd54b · builds after `reflection-probe-fluid-pass-performance` and counsel on `../reflection-probe-fluid-pass-performance/ip-check.md` §For counsel 3 and 5

## Problem
Developer-raised, from the cell-rate spike and the `fluid-base-pass-cost` landing runs. On the Radeon Pro 5300M, the Mac perf target (§10), the reflection is now most of the reflecting fluid pass: 10.2 ms over reflection-free at e24 near 4K and 12.7 ms on the probes map (`../reflection-probe-fluid-pass-performance/research.md` §Evidence). In pixel style the reflection is one value per water texel cell, yet every pixel computes it, and every hidden pixel's cell would too if computed apart from the depth-tested draw. Done means pixel-style water whose cells cover enough pixels pays per visible cell, not per pixel. Pixel-style reflecting water at the near and probes views at 4K then costs within a millisecond of reflection-free, and looks the same within the parity bounds below.

## Decisions
- **Scope: pixel style's probe term.** The reflection-free variant keeps today's cost and image. Smooth style stays per fragment (`rendering_pipeline.md` §7.3a Style).
- **Reads the sibling's live-lighting sums.** The cell pass evaluates the ratio from `reflection-probe-fluid-pass-performance`'s per-probe sums and evaluates no stored light.
- **No early-out.** It removes visible reflections of lit walls at mixed views and buys under 1 ms at Auto on the 5300M's built-in display, except for a pool filling the frame seen straight down (`research.md` §Early-out).
- **Cell path for pixel style.**
  - Why it ships: it matches per-fragment within the parity bound, except in cells the per-fragment path itself splits. Its saving grows with resolution and texel size (`research.md` §Yardstick).
  - A compute pass evaluates each cell of the cell-path draws' faces (chosen below) and writes the frame's cell pool. The pixel-style fragment reads its cell's record.
  - Its fragment is a new entry point over the shared fluid shading, never an override constant (§7.3a Two pipelines). Each entry declares only the globals it uses.
  - Its pipelines build at full init beside the other fluid pipelines, never during a frame.
- **Hidden cells cost no evaluation.** The cell pass runs after the depth pre-pass. It skips a cell the scene depth hides over its whole screen footprint, so a cell any fragment sees is always evaluated. Neither the room cull, a conservative superset (§2), nor the CPU estimate sees a face behind a wall.
- **The cell pool holds the probe term only.** `fluid-ssr-reflections` keeps its own target at the player's quality tier, so its cost stays tied to that tier, the condition of its `index.md` §1.1 exception.
- **Per-draw path choice on the CPU, with hysteresis.**
  - A draw takes the cell path when its estimated on-screen pixels per dispatched cell reach a cutoff in [1, 2]. The estimate uses this frame's camera, extent and face geometry, with no GPU readback. One cutoff and one margin serve every backend, chosen on the 5300M.
  - A draw switches path only after its estimate crosses the cutoff by the margin. The state is kept per volume, so view bob near the cutoff never toggles it. A volume with no state takes the side the cutoff alone gives. Overflow at the pool's capacity edge may still toggle a draw, within the parity bound.
- **A per-frame cell pool sized to the screen.**
  - Each frame, the cell-path draws' faces, clipped to the screen, pack by a fixed priority into a pool. The pool is sized from the scene extent and the cutoff, not from the level's faces (`research.md` §Cell storage sizing). Each face's pool origin rides a bounded per-frame face record.
  - A draw any of whose faces or face records do not fit takes the per-fragment path that frame.
  - It is allocated only while an installed fluid level has a probe-bound face under pixel style. It allocates or releases on level install, unload and a committed style switch, and rebuilds with the scene targets on an extent change (§7.3a Install).
  - Written and read in one frame, with every read record written that frame. It holds no history and no precomputed intersections, so `plans/done/reflection-probes`' no-history gate extends to it.
- **Cell-path look.** Its fade decides once, at the cell centre. A cell straddling its fade threshold no longer splits its reflection; the albedo still splits. This diverges from §7.3a Style's split sentence and from §7.3a Capture's fixed pixel style; both change at promotion.
- **IP fences (owner, 2026-11-22), from `../reflection-probe-fluid-pass-performance/ip-check.md` §Lever C.**
  - Cell resolution stays fixed by the water's texel grid. No per-face level of detail, mip level or cell size comes from distance, projected area or the pixels-per-cell estimate, which picks a path only (Qualcomm '166, EP4409531B1).
  - A face packs whole texel rows and columns, clipped only to bound the pool. No face is resampled to fit a budget.
  - The hidden-cell test runs inline in the invocation that evaluates the cell. No pass writes a per-cell or per-tile visibility bitfield that a later dispatch or allocation reads. No any-visible flag gates a dispatch. Visible cells are never compacted into indirect dispatch arguments or into the pool (Intel '475, Qualcomm '715).
  - Owner IP review before any of these changes.
- **No split draw here, despite the 1660 perf floor (§10).** It needs a per-backend switch or a measured gate (`research.md` §The 1660 floor), so it is a follow-up brief. This brief records the 1660 cost that shows whether the read penalty survives the base-pass landing (Manual).
- **Non-goals:**
  - Live lighting: `reflection-probe-fluid-pass-performance`.
  - Probe blending and streaming: `reflection-probe-seams` and `reflection-probe-streaming` (drafts on branch `claude/probe-streaming-brief-364a5e`).
  - The SSR layer's target and its composite with the probe term: `fluid-ssr-reflections`.
  - Any author or player surface: both paths match within parity, so there is nothing to choose.

## Acceptance
Orderings cite `research.md` §Pins.
### Automated: unit and source gates
- [ ] Within a frame, in the game and in capture alike, in one submission: the pack and face records written; the depth pre-pass and the live-lighting sums pass, either order; then the cell pass; then the fluid pass. A source-order gate pins it (P2).
- [ ] Entry globals, pinned the way the reflection-free entry's are: the cell-path entry uses none of the probe search's globals; the reflection-free entry declares none of the pool or face-record globals; the per-fragment reflecting entry declares none of the pool's or face records'.
- [ ] The no-history gate's closed binding inventory covers the cell-pass shader and lists the cell pool and the face records by name; none is named for history, reprojection or intersections.
- [ ] No cell pipeline is created outside full init: not in the frame path, level install or unload, a style commit or the extent-change rebuild. No pool is allocated outside the extent-change rebuild, level install or unload, and a style change committed while a level is installed. A source gate refuses both.
- [ ] The cell evaluation has a GPU-free reference, and a test pins the shader text against it (§7.3a References).
- [ ] Budget pins pass with the new bindings, including the compute stage's storage, uniform and sampled-texture counts; no shared group gains a stage its shader does not read.
- [ ] The cell pass records a labelled GPU timing pair inside the frame's prefilled query set.
- [ ] Cell-path candidates come only from draws whose variant this frame is reflecting, under the mask the fluid pass picks entries by. A draw rendering reflection-free (reflectivity 0, no probe-bound face, a mask with a term off) contributes no cell.
- [ ] The cutoff and margin are each one value, read by no backend or adapter query. A GPU-free test computes each estimate from camera, scene extent and face geometry alone, counting scene-target pixels. At a fixed camera and surface extent, halving the scene extent quarters every estimate.
- [ ] The per-frame pack gives every packed face a rect inside the pool, disjoint from every other, in the fixed priority. A draw takes the cell path only if all its faces and face records fit. A packed face's rect covers its screen-clipped texel cells at one cell per texel, whatever its distance or estimate.
- [ ] Hysteresis (P9, P12):
  - an estimate oscillating across the cutoff by less than the margin never switches path;
  - a rising estimate switches once past cutoff plus margin, a falling one once past cutoff minus margin;
  - a volume with no state, including every volume on a newly installed level, takes the cutoff's side;
  - two draws whose far-to-near order swaps at fixed estimates each keep their path.
- [ ] Source gates: no pass writes a cell or tile visibility mask read by a later dispatch or allocation, and the cell dispatch's size comes from the pack alone.
### Automated: GPU gates (a GPU machine, under `POSTRETRO_REQUIRE_GPU`; captures, the GPU test harness, the DX12 FXC test)
- [ ] Every fluid pipeline and the cell pipelines compile under the DX12 FXC test on a Windows DX12 adapter, its log showing it ran, not skipped.
- [ ] Pool lifecycle in any order (P6, P7, P8, P13):
  - no pool for an extent change with no fluid level, a fluid level with no probe-bound face, or smooth style;
  - an install after an extent change sizes from the current extent;
  - an install and an extent change, or a style switch and an extent change, on one frame in either order, or an unload followed by an install, leave one pool at the current extent, bound to the fluid pass;
  - an install followed by an unload leaves none;
  - unload or a switch to smooth releases it, and the pass stays bindable;
  - a frame whose extent changes packs and judges overflow against the new extent.
- [ ] Dispatches: a frame that is all per-fragment, all overflowed or smooth style dispatches no cells (P3).
- [ ] The reflection-free variant and every reflection-off case in §7.3a Off render identically to before, per fragment and with the cell path forced over a poisoned pool. That includes a face seen from its down side and a cell-path draw with a face bound to no probe. Reflectivity 0, which capture cannot set, is proven by its selecting the reflection-free variant.
- [ ] Cell path forced vs per-fragment pixel style within 2/255 at near, probes and far views, at animation time 0 and at least two later times, and with the eye inside the proximity band above the plane. Every exception lies in a cell the per-fragment image draws in two values.
- [ ] Poisoned pool (P5): a pool and face records pre-filled with a poison value render identically to cleared ones at every parity view, under each of:
  - across a wall's edge;
  - after a camera turn brings a new probe's face into view;
  - a capped pool in which a cell-path draw overflows;
  - cell-path faces crossing the screen edge and extending behind the camera.

  After a camera jump, or a face hidden on one frame and visible on the next, each matches a fresh capture at a second pose whose estimates lie outside the margin of the cutoff.
- [ ] Hidden cells:
  - a cell-path face wholly behind an opaque wall in a visible room dispatches its cells yet evaluates none; unoccluded, every cell evaluates;
  - a face cut by a wall's edge renders within the parity bound on both sides, skip on and off alike;
  - with a capped pool, a hidden near face that packs ahead of a visible far face evaluates no cells, and the far face draws per fragment within the parity bound.
- [ ] The first frame after a style switch to smooth, as a reload makes, dispatches no cells and matches a fresh smooth capture. On a level installed under smooth, the first frame after a switch to pixel takes the cell path where chosen. Capture switches style through the same commit, amending the gate that keeps capture off the style setter (P11, P13).
- [ ] Path choice: the near and probes views, whose estimates lie well above any allowed cutoff, take the cell path. Every other parity view lands on the side the chosen cutoff puts its screen-clipped estimate. The report gives the cutoff and each estimate, and each equals the GPU-free computation at that view.
- [ ] A frame whose cell-path draws exceed a capped pool or face-record capacity draws the draws that do not fit per fragment, within the parity bound. At a fixed view the same draws fall back every frame (P8).
### Manual
- [ ] **5300M, near and probes 4K:** pixel-style reflecting water costs at most reflection-free + 1.0 ms by pass cost, with whole-frame GPU time beside it.
- [ ] **5300M, far 4K and mid 1080p:** cost no more than the per-fragment path with the live-lighting sums.
- [ ] **5300M at Auto (1536×960), near, probes, mid and low:** measured and reported against the build before and against reflection-free, with each run's mean clock.
- [ ] **5300M, each lever as an increment:** the cell path against the sums alone; the hidden-cell skip on against off. Measured and reported.
- [ ] **5300M, `stress-env-volumes`, at 4K and Auto, at a pose where a cell-path face lies wholly behind a wall in a visible room, and at the cover pose:** the hidden-cell skip's cost and saving. Measured and reported.
- [ ] **GTX 1660, Vulkan:** each measured view against the build before, naming any the one cutoff slows, and the reflecting-minus-free cost at e24 near 4K. Measured and reported.
- [ ] Each manual cost row names its baseline build (`research.md` §Measurement fixture).
- [ ] **Owner look:** in engine at steep and grazing views over e24's pools and the probes map. Pixel-style water still breaks into moving blocks, and no cell flickers at a wall's edge or at the path cutoff as the camera moves.

## Path
- **Order:** the cell path, then the hidden-cell skip. Measure on the 5300M after each. Capture needs switches for the cell path and the skip, a forced cell path, pool and face-record caps, poison pre-fills, and a prior frame at another pose, time or style.
- **Cell pass:** reuse the fragment's search, fade, tap and Fresnel functions, with the ratio read from the sums. Write reflection colour and weight; keep the proximity band per fragment. Estimate pixels per cell from projected face area over cell count. Pack nearest first and skip a per-cell frustum cull (`research.md` §Cell storage sizing, §What the spikes rule out).
- **Hidden-cell test:** a cell's footprint can straddle a wall's edge, so test it conservatively (its corners, or the farthest depth over it), never the centre alone. A single cell cannot fall back per fragment: the cell entry carries no search globals.
- **Precedents:**
  - `claude/cell-rate-spike-1660`: `fluid_reflection_cells.rs` and `fluid_reflection_cells.wgsl`. The 5300M branch (`claude/cell-rate-spike`), with the render-cpu affine fit and shelf pack, is gone from origin.
  - `fix/fluid-reflection-free-entry-point` (landed): `fluid_fragment_entry_point`, the shared `fluid_shade()`, and its test that the reflection-free entry's globals exclude the probe bindings.
  - `fluid-ssr-reflections` (landed): a 1×1 placeholder bound at full init, with group 5 rebuilt only at its lifecycle points.
- **Face records:** pool origins change every frame, so they cannot ride an install-time vertex buffer as the spike's did. Prefer a uniform table keyed by a face index on the vertex stream; its capacity is the face-record bound. The base-pass grid adds vertices per face, not faces, so a face-keyed table is unaffected.
- **Seams:** `reflection-probe-seams` puts nearby probes on the vertex stage, which a compute cell pass lacks. Whichever lands second gives the cell pass a compute-readable source for them.
- **Large files:** `renderer_render_frame.rs` and `fluid_surface.rs` are past `development_guide.md` §2.1's split line. Split before extending.

## Open questions
- IP line for the cell pool's sizing and allocation: `../reflection-probe-fluid-pass-performance/ip-check.md` §For counsel 3 (Qualcomm '166 and EP4409531B1) and 5 (Qualcomm '715). Owner: Dan, with counsel — **blocks build**
- Cutoff and margin values — **delegated**.
- Hidden-cell footprint test form — **delegated**: the executor picks it and reports the measured saving.
- How the pool binds to the fluid pass when not allocated: placeholder binding, following `fluid-ssr-reflections`, or a rebuild on allocation. Both group-5 instances (SSR and planar) carry it — **delegated**.
