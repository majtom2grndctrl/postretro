# reflection-probe-fluid-pass-performance

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §2, §7.3a, §10; `context/lib/resource_management.md` §5.3; `context/lib/testing_guide.md` §Resource bounds · read at a8c6e3300 · builds after `fix/fluid-reflection-free-entry-point`

## Problem
Developer-raised, from the roadmap and the cell-rate and blend-cost spikes.
- **Cost.** On the Radeon Pro 5300M, the Mac perf target (§10), reflecting water costs 9.6 ms over the reflection-free variant for full-screen 4K water at e24 and 13.6 ms on the probes map (`research.md` §Evidence). Scaled by pixel count, that is about 1.7–2.4 ms at Auto on the built-in display (1536×960). Once `fluid-base-pass-cost` moves the baked-light lookup per vertex, the reflection-free pass falls to a few milliseconds (that brief's research), and the reflection becomes most of the fluid pass's cost.
- **Cause.** Every fragment re-evaluates work the result does not vary by:
  - live lighting, which is a function of the probe and the frame;
  - in pixel style, a reflection that is one value per water texel cell yet is computed per pixel.
- **Done means:**
  - live lighting is evaluated at most once per probe per frame;
  - pixel-style water whose cells cover enough pixels pays per cell, not per pixel, and only for cells not hidden behind opaque geometry;
  - pixel-style reflecting water at the near and probes views at 4K costs within a millisecond of reflection-free on the 5300M;
  - the reflection looks the same within the parity bounds below.

## Decisions
- **Scope: the reflecting variant only.** The reflection-free variant keeps today's cost and image. The precompute serves both styles; the cell path is pixel style only (`rendering_pipeline.md` §7.3a Style).
- **Live lighting once per probe per frame, on the GPU.**
  - On a frame with any reflecting-variant draw, a compute dispatch evaluates, for every resident probe, the per-lobe sums the reflection's live-lighting ratio is linear in. Every face drawn that frame therefore reads current sums, and `reflection-probe-seams`' blend neighbours need nothing extra.
  - No fragment entry evaluates a probe's stored lights. The surface's own runtime lighting is unchanged.
  - GPU over CPU: the dispatch reads the buffers script writes land in, so scripted lights cannot read stale; the CPU form needs a mirror of bridge samples (`research.md` §Precompute).
- **No early-out.** It removes visible reflections of lit walls at mixed views and buys under 1 ms at Auto on the 5300M's built-in display except for a pool filling the frame seen straight down (`research.md` §Early-out).
- **Cell path for pixel style.**
  - Why it ships: it matches per-fragment within the parity bound except in cells the per-fragment path itself splits, and its saving grows with resolution and texel size (`research.md` §Yardstick).
  - A compute pass evaluates each cell of the cell-path draws' faces (chosen below) and writes the frame's cell pool; the pixel-style fragment reads its cell's record.
  - Its fragment is a new entry point over the shared fluid shading, never an override constant (§7.3a Two pipelines, as `fix/fluid-reflection-free-entry-point` rewrites it). Each entry declares only the globals it uses.
  - Its pipelines build at full init beside the other fluid pipelines, never during a frame.
- **Hidden cells cost no evaluation.** The cell pass runs after the depth pre-pass and skips a cell the scene depth hides over its whole screen footprint, so a cell any fragment sees is always evaluated. Neither the room cull, a conservative superset (`rendering_pipeline.md` §2), nor the CPU estimate sees a face behind a wall.
- **The cell pool holds the probe term only.** `fluid-ssr-reflections` keeps its own target at the player's quality tier, so its cost stays tied to that tier, the condition of its `index.md` §1.1 exception.
- **Per-draw path choice on the CPU, with hysteresis.**
  - A draw takes the cell path when its estimated on-screen pixels per dispatched cell reach a cutoff in [1, 2], from this frame's camera, extent and face geometry with no GPU readback. One cutoff and one margin serve every backend, chosen on the 5300M.
  - A draw switches path only after its estimate crosses the cutoff by the margin, with the state kept per volume, so view bob near the cutoff never toggles it. A volume with no state takes the side the cutoff alone gives. Overflow at the pool's capacity edge may still toggle a draw, within the parity bound.
- **A per-frame cell pool sized to the screen.**
  - Each frame, the cell-path draws' faces, clipped to the screen, pack by a fixed priority into a pool sized from the scene extent and the cutoff, not from the level's faces (`research.md` §Cell storage sizing). Each face's pool origin rides a bounded per-frame face record.
  - A draw any of whose faces or face records do not fit takes the per-fragment path that frame.
  - It is allocated only while an installed fluid level has a probe-bound face under pixel style. It allocates or releases on level install, unload and a committed style switch, and rebuilds with the scene targets on an extent change (§7.3a Install).
  - Written and read in one frame, with every read record written that frame. It holds no history and no precomputed intersections, so `plans/done/reflection-probes`' no-history gate extends to it.
- **Cell-path look.** Its fade decides once, at the cell centre. A cell straddling its fade threshold no longer splits its reflection; the albedo still splits. This diverges from §7.3a Style's split sentence, which changes at promotion, as does §7.3a Capture's fixed pixel style.
- **No split draw here, despite the 1660 perf floor (§10).** It needs a per-backend switch or a measured gate (`research.md` §The 1660 floor), so it is a follow-up brief. This brief records the 1660 cost that shows whether the read penalty survives (Manual).
- **Non-goals:**
  - The reflection-free fluid pass's own cost: `fluid-base-pass-cost` (draft on branch `claude/fluid-base-pass-cost`).
  - Probe blending and streaming: `reflection-probe-seams` and `reflection-probe-streaming` (drafts on branch `claude/probe-streaming-brief-364a5e`).
  - The SSR layer's target and its composite with the probe term: `fluid-ssr-reflections`.
  - Any author or player surface: both paths match within parity, so there is nothing to choose.

## Acceptance
Orderings cite `research.md` §Pins.
### Automated: unit and source gates
- [ ] The precomputed ratio matches the per-fragment ratio within 1e-4 across random probes, lights, lobes and directions, including a light past the probe's stored few, which holds its reference.
- [ ] Within a frame: script light writes, the descriptor flush, the precompute, the fluid pass; no light write lands between the precompute and the fluid pass. A source-order gate pins it (P1).
- [ ] Within a frame, in the game and in capture alike, in one submission: the pack and face records written; the depth pre-pass and the precompute, either order; then the cell pass and anything feeding the fragment's live-lighting sums; then the fluid pass. A source-order gate pins it (P2).
- [ ] Entry globals, pinned the way the reflection-free entry's are: no fragment entry uses the baked animated-light descriptors or reaches an evaluation of a probe's stored lights; the cell-path entry uses none of the probe search's globals; the reflection-free entry declares none of the new sums, pool or face-record globals; the per-fragment reflecting entry declares none of the pool's or face records'.
- [ ] The no-history gate's closed binding inventory covers the fluid, precompute and cell-pass shaders and lists the live-lighting sums, the cell pool and the face records by name; none is named for history, reprojection or intersections.
- [ ] No fluid, precompute or cell pipeline is created outside full init: not in the frame path, level install or unload, a style commit or the extent-change rebuild. No pool is allocated outside the extent-change rebuild, level install or unload, and a style change committed while a level is installed. A source gate refuses both.
- [ ] The precompute's light evaluation keeps the per-fragment evaluation's source gates, retargeted to its shader: it reads no promotion weight, crossfade, lighting mask, debug override or SH data, indexes descriptors by roster index through the shared lighting group, and reads the frame's animation time.
- [ ] The precompute and cell evaluation each have a GPU-free reference, and a test pins each shader text against it (§7.3a References).
- [ ] Budget pins pass with the new bindings, including the compute stages' storage, uniform and sampled-texture counts; no shared group gains a stage its shader does not read.
- [ ] The precompute and cell passes each record a labelled GPU timing pair inside the frame's prefilled query set.
- [ ] Cell-path candidates come only from draws whose variant this frame is reflecting under the mask the fluid pass picks entries by; a draw rendering reflection-free (reflectivity 0, no probe-bound face, a mask with a term off) contributes no cell.
- [ ] The cutoff and margin are each one value, read by no backend or adapter query. A GPU-free test computes each estimate from camera, scene extent and face geometry alone, counting scene-target pixels: at a fixed camera and surface extent, halving the scene extent quarters every estimate.
- [ ] The per-frame pack gives every packed face a rect inside the pool, disjoint from every other, in the fixed priority; a draw takes the cell path only if all its faces and face records fit.
- [ ] Hysteresis: an estimate oscillating across the cutoff by less than the margin never switches path; a rising estimate switches once past cutoff plus margin, a falling one once past cutoff minus margin; a volume with no state, including every volume on a newly installed level, takes the cutoff's side; two draws whose far-to-near order swaps at fixed estimates each keep their path (P9, P12).
### Automated: GPU gates (a GPU machine, under `POSTRETRO_REQUIRE_GPU`; captures, the GPU test harness, the DX12 FXC test)
- [ ] Every fluid pipeline and the compute pipelines compile under the DX12 FXC test on a Windows DX12 adapter, its log showing it ran, not skipped.
- [ ] Pool lifecycle in any order: no pool for an extent change with no fluid level, a fluid level with no probe-bound face, or smooth style; an install after an extent change sizes from the current extent; an install and an extent change, or a style switch and an extent change, on one frame in either order, or an unload followed by an install, leave one pool at the current extent, bound to the fluid pass; an install followed by an unload leaves none; unload or a switch to smooth releases it and the pass stays bindable; a frame whose extent changes packs and judges overflow against the new extent (P6, P7, P8, P13).
- [ ] Dispatches: a frame with no reflecting-variant draw, including one of only reflection-free fluid and one whose light-term mask shuts every reflecting draw, dispatches no precompute and no cells. A frame with reflecting draws dispatches the precompute once, whatever the draw count; all per-fragment, all overflowed or smooth style dispatches no cells (P3).
- [ ] A reflecting face first drawn this frame, after a camera turn or from behind a wall, with a stored light of its probe forced away from its reference, shows that light on that frame; on a frame whose drawn faces all bind one probe, an undrawn probe's sums, read back, carry its forced light too (P4).
- [ ] After an unload and an install of a second level with a different probe count and different stored lights, the first frame's precompute reads the second level's probes: at a view where a stored light differs from its reference, it matches the build before within 1/255 on that frame (P14).
- [ ] The reflection-free variant and every reflection-off case in §7.3a Off render identically to before, per fragment and with the cell path forced over a poisoned pool, including a face seen from its down side and a cell-path draw with a face bound to no probe; reflectivity 0, which capture cannot set, is proven by its selecting the reflection-free variant.
- [ ] Precompute against the build before it, within 1/255, at a view where a stored light differs from its reference: per-fragment pixel style, smooth style, and a stored light on an animated curve at time 0 and at a later time reached after a prior frame at a different time (P10).
- [ ] Cell path forced vs per-fragment pixel style within 2/255 at near, probes and far views, at animation time 0 and at least two later times, and with the eye inside the proximity band above the plane. Every exception lies in a cell the per-fragment image draws in two values.
- [ ] A pool, face records and live-lighting sums pre-filled with a poison value render identically to cleared ones at every parity view, across a wall's edge, after a camera turn brings a new probe's face into view, with a capped pool in which a cell-path draw overflows, and with cell-path faces crossing the screen edge and extending behind the camera; and after a camera jump, or a face hidden on one frame and visible on the next, each matches a fresh capture at a second pose whose estimates lie outside the margin of the cutoff (P5).
- [ ] Hidden cells: a cell-path face wholly behind an opaque wall in a visible room dispatches its cells yet evaluates none; unoccluded, every cell evaluates; a face cut by a wall's edge renders within the parity bound on both sides, skip on and off alike; with a capped pool, a hidden near face that packs ahead of a visible far face evaluates no cells and the far face draws per fragment within the parity bound.
- [ ] The first frame after a style switch to smooth, as a reload makes, dispatches no cells and matches a fresh smooth capture; on a level installed under smooth, the first frame after a switch to pixel takes the cell path where chosen. Capture switches style through the same commit, amending the gate that keeps capture off the style setter (P11, P13).
- [ ] Path choice: the near and probes views, whose estimates lie well above any allowed cutoff, take the cell path; every other parity view lands on the side the chosen cutoff puts its screen-clipped estimate; the report gives the cutoff and each estimate, and each equals the GPU-free computation at that view.
- [ ] A frame whose cell-path draws exceed a capped pool or face-record capacity draws the draws that do not fit per fragment within the parity bound, and at a fixed view the same draws fall back every frame (P8).
### Manual
- [ ] **5300M, near and probes 4K:** reflecting water with all levers costs at most reflection-free + 1.0 ms, by pass cost, with whole-frame GPU time beside it.
- [ ] **5300M, far 4K and mid 1080p:** cost no more than today's per-fragment path.
- [ ] **5300M at Auto (1536×960), near, probes, mid and low:** measured and reported against today and against reflection-free, with each run's mean clock.
- [ ] **5300M, each lever as an increment:** the precompute against the build before it, in both styles; the cell path against the precompute alone; the hidden-cell skip on against off. Measured and reported.
- [ ] **5300M, `stress-env-volumes`, a pose where a cell-path face lies wholly behind a wall in a visible room, plus cover, 4K and Auto:** the hidden-cell skip's cost and saving. Measured and reported.
- [ ] **GTX 1660, Vulkan:** each measured view against today, naming any the one cutoff slows, and the reflecting-minus-free cost at e24 near 4K. Measured and reported.
- [ ] Each manual cost row names its baseline build and whether `fluid-base-pass-cost` is in it (`research.md` §Measurement fixture).
- [ ] **Owner look:** in-engine at steep and grazing views over e24's pools and the probes map, pixel-style water still breaks into moving blocks, and no cell flickers at a wall's edge or at the path cutoff as the camera moves.

## Path
- **Order:** the levers are the precompute, then the cell path, then the hidden-cell skip; seams depends on the precompute and the cell pass reads it. Measure on the 5300M after each, which is how the lever increments are taken. Capture needs switches for the cell path and the skip, a forced cell path, pool and face-record caps, poison pre-fills and a prior frame at another pose, time or style (the spike's capture levers are the precedent).
- **Precompute:** one RGB sum per §7.3a lobe per probe, from the same descriptors and samples as SH compose. It writes a buffer the cell pass reads as storage; the fragment reads it as a uniform, by a copy or by one buffer carrying both usages, since fragment storage is spent (§10).
  - Rival: a CPU precompute (the blend spike's shape) with a bridge-sample mirror.
  - Rival, unmeasured: one live value per light per frame, reused by every consumer. Costs more per fragment; worth weighing only if the IP review rules against per-probe sums (`ip-research.md` Lever A).
- **Cell pass:** reuse the fragment's search, fade, tap, ratio and Fresnel functions; write reflection colour and weight; keep the proximity band per fragment. Estimate pixels per cell from projected face area over cell count. Pack nearest first and skip a per-cell frustum cull (`research.md` §Cell storage sizing, §What the spikes rule out).
- **Hidden-cell test:** a cell's footprint can straddle a wall's edge, so test it conservatively (its corners, or the farthest depth over it), never the centre alone. A single cell cannot fall back per fragment: the cell entry carries no search globals.
- **Precedents:**
  - `claude/cell-rate-spike` (5300M implementation): `crates/render-cpu/src/fluid_surface/reflection_cells.rs` for the affine fit, rect and shelf pack; `crates/renderer/src/render/fluid_reflection_cells.rs`; `fluid_reflection_cells.wgsl`; capture-scene `style`, `pre_camera`, levers and cell stats.
  - `claude/blend-cost-spike`: `fluid_reflection_ratio_precomputed` and its group 5 uniform.
  - Neither branch merges.
  - `fix/fluid-reflection-free-entry-point`: `fluid_fragment_entry_point`, the shared `fluid_shade()`, and its test that the reflection-free entry's globals exclude the probe bindings.
- **Face records:** pool origins change every frame, so they cannot ride an install-time vertex buffer as the spike's did. Prefer a uniform table keyed by a face index on the vertex stream; its capacity is the face-record bound.
- **Large files:** `fluid_surface.wgsl` and `renderer_render_frame.rs` are past the size guideline, and `fluid_surface.rs` is close. Split before extending.

## Open questions
- IP line for each lever, from `ip-research.md` §For counsel:
  - the precompute against Tencent claims 10/19 and 11–14, and US12293451B2;
  - the same-frame cell storage against the texture-space shading patents.

  Owner: Dan, with counsel — **blocks build**
- Cutoff and margin values — **delegated**.
- Hidden-cell footprint test form — **delegated**: the executor picks it and reports the measured saving.
- How the pool binds to the fluid pass when not allocated: group 5 is built at init and rebuilt only with the scene targets. Placeholder binding or a rebuild on allocation. `fluid-ssr-reflections` holds the same question as blocking for its target — **delegated**: whichever brief builds first answers it, and the second follows.
