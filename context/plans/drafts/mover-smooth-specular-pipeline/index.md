# mover-smooth-specular-pipeline

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Texel-centre highlight), §7.3 (Mover register cliff), §7.3a (Two pipelines), §8, §10 (Target hardware), §12 · `context/lib/testing_guide.md` · read at 1a66523ae · evidence: `research.md`

## Problem
specular-texel-sharpness's cost measurement (`plans/done/specular-texel-sharpness`, Task 9) found the mover pass slower than the build before it on the Mac target: +2.4 ms (+11%) under `smooth` and +4.1 ms (+19%) under `pixel`, at 4K with Surface Depth Off. `smooth` exists to keep the old look, so its extra cost buys nothing. Cause: one mover shader compiles both styles behind a per-frame style word, and the pixel path's code plus the loop's two accumulators (lit sum, tinted specular) cost every smooth fragment about 2.8 ms. A follow-up spike confirmed this and found the shape that recovers it (`research.md`). When done, smooth movers cost no more than the build before specular-texel-sharpness, pixel movers cost no more than today, and a live style change still reaches the next frame without compiling anything.

## Decisions
- **Two mover beauty pipelines, one per style, both built at renderer init.** Each frame draws with the pipeline its style snapshot names. The main view and the planar mirror use the same one on a frame. A style change compiles nothing and rebuilds no bind group (owner). This revises the done brief's "applies live without rebuilding any pipeline": the second pipeline is built ahead of time, never on a change. The mover depth pipeline reads no style and stays single.
- **One per-frame style snapshot still drives forward and mover.** The snapshot taken with group 0 sets forward's style bit and picks the mover pipeline, so the two passes cannot disagree within a frame (§4, Style transport). Forward's shader and style transport do not change.
- **Smooth pipeline: one specular sum.** No pixel path. Diffuse and specular accumulate apart in the light loop. After the loop, specular takes the albedo, or the texel's tint colour where tinted, in one multiply. The spike measured this shape at 120 VGPRs and below the pre-feature build's time (`research.md` §V5s).
- **Smooth mover output differs from the pre-change evaluation only by float reassociation, not bit for bit (owner).** Summing specular apart from diffuse moves the rounding; the spike saw at most 1/255 on 2–8 pixels per 2 Mpx. Both exact-match smooth-only shapes it tried crossed the cliff, and the one it timed cost about 11 ms. Forward smooth stays bit-exact. This revises the done brief's smooth and untinted byte-identity rows for the mover only.
- **Pixel pipeline: today's pixel path, bit for bit.** Its output and its frozen-loop oracles do not change.
- **The register screen and a clock-pinned re-time gate both pipelines before landing.** The cliff moves with code shape, and this compiler changed tiers on shape alone (`research.md` §Cliff bisection; §7.3, Mover register cliff). Either pipeline past today's occupancy tier stops for the owner.
- **Lever: all backends, measured on Metal only.** Both pipelines ship everywhere; the win is priced on the Mac target, and the GTX 16-series perf floor is unmeasured (§10, Target hardware).
- **Durable capture.** §7.3 records the cliff trigger: a smooth-only mover with two light-loop accumulators reached the next tier on this compiler, branch or select alike. The style-transport and tint-output bullets of §4 follow the new shape. §8's "no shader-variant system" becomes the rule that holds: a per-material difference is a data branch in one shader, except where a variant's mere presence costs, as fluid's per-draw reflecting variant does (§7.3a); a global render-profile style may select between pipelines built at init, as bloom's pixelated mode does.
- **The measurement method lives in the repo.** The build brings the spike's `measurements/specular-mover-cost/` kit and README from `spike/specular-mover-cost` onto its branch, without the spike's capture-hook commit, which never merges (`research.md` §Kit).
- **Non-goals.** Pixel's own ~4 ms: the spike found nothing recoverable. Forward-pass cost: Task 9 measured forward smooth flat, and forward pixel's +0.4–1.4 ms is unattributed and inside the ±1.7 ms the spike saw forward move with its shader unchanged. Any player option: style stays a mod setting (`player_options.md`). A generic paired-A/B mechanism: `drafts/gpu-pass-paired-ab` owns it.

## Acceptance

### Automated
- [ ] The smooth fragment's shade after its post-loop composition matches the frozen pre-change loop composed the old way, within the float-reassociation bound the tint tests use, on every existing whole-loop case: march and shadow pattern on and off, specular off, a sharp exponent, a tilted normal, a low eye, and one-, two- and four-channel untinted slots. The readback drives the smooth entry's own post-loop composition, not a harness copy. On every case except specular-off, some fragment's specular-on shade exceeds its specular-off shade by more than that bound.
- [ ] The pixel mover's existing test cases pass bit-exact, every assertion and oracle unchanged: texel-identical specular per light type and edge, frozen-loop identity, sharpness decode, tint and the marker cases. Only how a case selects pixel, and which function it splices, may change.
- [ ] Forward's smooth and untinted identity tests pass unchanged and bit-exact.
- [ ] Under each style, diffuse is identical with and without tint, and a tinted texel's specular matches the shared tint function.
- [ ] Building the mover pass creates exactly three render pipelines: smooth beauty, pixel beauty and depth.
- [ ] Both mover pipelines build with cube arrays off, and both fragment entries pass naga validation in the no-cube source.
- [ ] Each mover record reports to tests which style's pipeline it bound for its view. The live-change, mirror and first-frame rows read that report for the main view and the mirror (R1).
- [ ] Each beauty pipeline is built with its own style's fragment entry, and the style a record reports is the value it chose the pipeline by, not a separate label: a source pin ties each entry name to its style at pipeline creation. On the mirror row's frame the mirror's report is present, so an empty mirror cannot pass.
- [ ] The smooth entry's light loop, and every helper it calls, uses no implicit-derivative sample (§8, DX12 portability), pinned by a source test.
- [ ] A live style change draws movers with the other style's pipeline on the next frame, in both directions. The frame it lands creates no pipeline and as many GPU objects of each kind as an unchanged frame, counted on the render thread.
- [ ] On a frame the planar mirror draws movers, it binds the main view's pipeline, including the frame a style change lands (R1).
- [ ] A source pin shows every mover pipeline selection, main view and planar mirror alike, reads the frame's style snapshot, never the live boot-state style (R1).
- [ ] A style committed before full init, before a full-renderer rebuild (suspend and resume), before a level install, or before a load in flight draws that style's mover pipeline on the first frame (R2).
- [ ] Under smooth, Surface Depth Off then On keeps smooth highlights. A style change while Surface Depth is Off leaves the march off.

### Manual
- [ ] Mac target, register screen of both pipelines: each holds today's occupancy tier (8 waves per SIMD under wave32). Counts recorded in `tools/metal_register_screen/README.md`.
- [ ] Mac target, 4K, Surface Depth Off, setpiece "wall" and dev-map "ramp" poses, clock-pinned and interleaved per the kit: the smooth mover pass pooled median is at or below the pre-feature build's (1c7fc9e3a), and the pixel pooled median is no more than 0.15 ms above the pixel median at 1a66523ae, the build before this brief. Both deltas are reported with per-run brackets.
- [ ] In-engine, the dev map's movers look unchanged under each style, and toggling the style with the water mirror in view shows no stall.
- [ ] `rendering_pipeline.md` §7.3 records the two-accumulator cliff trigger, §4's style-transport and tint-output bullets describe pipeline selection and the one-sum smooth shape, and §8 states the data-branch versus init-built pipeline rule.
- [ ] `measurements/specular-mover-cost/` and its README land on the build branch. No commit applies the Surface Depth Off hook to engine source.

## Path
- Seams: `KinematicBrushPass::new` (pipeline creation), `KinematicBrushPass::record_draws` (main view; the mirror calls it from `planar_reflection/record.rs`), `accumulate_dynamic_direct`, `mover_specular_point` and `fs_main` in `kinematic_brush.wgsl`, the `FullRenderer::frame_specular_style` field, `KinematicLightParams`, `gpu_creation_counts`.
- Mechanism: separate fragment entry points over shared code, as fluid's `fs_main` / `fs_main_reflecting` are, selected per frame as bloom selects its pixelated pipelines in `BloomPass::record`. The rival, one entry specialized by a WGSL `override` constant, is the spike README's first suggestion; §7.3a records why fluid rejected it: naga's Metal writer still declares a folded branch's globals as entry arguments, and the AMD driver charges per declared argument. Two composed modules is the fallback if entry points do not hold the count.
- The spike's V5s shader (`measurements/specular-mover-cost/variants/kinematic_brush.v5s.wgsl` on the spike branch) is the starting point for the smooth body. Keep the spike branch until the kit lands.
- The pixel entry may compile out the smooth branch, or skip the directional re-evaluation, if it stays bit-exact. A one-sum pixel loop is not bit-exact (the spike's V5p differs by 1/255 on 5–9 px), so the per-light tint branch stays. Each was worth at most 0.14 ms (`research.md` §Pixel).
- The kinematic specular harness rebuilds `fs_main`'s composition by hand and returns only the loop's lit sum; the smooth row needs it to drive the entry's real composition instead. The untinted-zero assertion in the tint tests holds only for the two-sum pixel loop.
- First slice: build the smooth entry in the V5s shape and screen both entries before wiring selection. If either crosses the tier, stop there.
- The style word in `KinematicLightParams` may become unread by the mover; retire it or keep it, and move its tests to pin pipeline selection.
- `tools/metal_register_screen/` captures one entry today; extend it to name the entry it screens.
- `kinematic_brush.rs` is large; if the pipeline split grows it, split the tests out first, in their own commit.

## Open questions
- Whether the mover's style word survives — **delegated**: the executor decides and reports it in the plan of record.
- Albedo read before or after the loop in the smooth pipeline — **delegated**: decided by screen and re-time, reported in the plan of record.
