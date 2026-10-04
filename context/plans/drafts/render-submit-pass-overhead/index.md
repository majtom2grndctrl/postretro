# render-submit-pass-overhead

Brief · compact · reads: `context/lib/rendering_pipeline.md` §1, §7, §12 · `context/lib/development_guide.md` §1.4, §6.4 · `context/lib/experimental_spikes.md` · read at a6a67bcce

## Problem
Developer profiling on the compatibility-floor Mac. `render_submit` is the largest CPU stage (3.81 ms hallway, 3.48 ms campaign at `dcde8f292`, vsync on, validation off). The cause of its largest remaining bucket is confirmed in source and profile: wgpu 29 replays every render and compute pass at `finish`, and on Metal each pass creates its own render encoder plus two `MTLCommandBuffer`s — the pass and an empty "Pre Pass" transition buffer — each committed and freed separately. That per-pass work is about 36% of the stage on the hallway and 47% on campaign, roughly 35–40 µs per pass, with the bloom chain holding about half of a frame's passes. Its size after `visible-span-draws`, and whether CPU submit cost bounds any frame with vsync off, are unmeasured; a second large bucket (staging-buffer frees from glyphon's per-frame text writes) is not per pass at all. This is a build-to-learn brief. When done, every CPU-timing window reports the frame's recorded pass count, the per-pass cost and the post-`visible-span-draws` stage split are measured on both maps in both vsync modes, and a findings note says which pass-reduction brief, if any, to draft next.

## Decisions
- **Spike, not fix.** The lever choice depends on numbers that do not exist: the stage split after `visible-span-draws` (its timing rows were waived) and the vsync-off bound. Shape follows `context/lib/experimental_spikes.md`: honesty gates pass/fail, findings measure-and-report, findings note last.
- **One instrument: a per-frame pass count in the CPU-timing window.** A count-kind stage in the renderer's stage set under `render_record`, counting every render and compute pass the frame encoder records. Renderer-owned, because only the renderer sees passes (`context/lib/index.md` §2 Renderer owns GPU); the binary folds it by label with no edit (`rendering_pipeline.md` §12). Runtime-gated by the existing timing env var, in every build (`development_guide.md` §6.4). It obeys the §12 rules: absent when the frame did not run, never zero for absent, nothing accumulated when timing is off. The `dev-tools` overlay's own submit is not counted; it is its own stage.
- **Coverage is guarded from source.** A pass the count misses makes every µs-per-pass figure wrong, so a source-derived guard lists every pass site in renderer non-test source and fails on an uncounted one (`context/lib/testing_guide.md` §3 Drift guards derive from the source).
- **Per-pass cost is measured by subtraction with the existing bloom switch.** `POSTRETRO_BLOOM=0` removes the bloom chain, the largest pass group, with no draw content to confound the delta. No new switch and no code change per candidate lever: a throwaway toggle per hypothesis is scope, and §6.4 wants diagnostics uniform.
- **Non-goal: restructuring passes or patching wgpu here.** Which lever (bloom chain consolidation, skipping clear-only promoted shadow passes, a lazy Metal command buffer in wgpu-hal, merging same-attachment passes) is the findings note's output. The wgpu-hal change is a dependency fork and an owner door.
- **Non-goal: glyphon staging churn.** `plans/done/per-frame-upload-batching` Decisions "Direct writes left" chose to leave glyphon's writes direct. The finding is recorded in `research.md` and sized in the findings note; fixing it is a sibling brief.
- **Measurement conditions pin to `plans/done/release-indirect-validation`** (same binary, eight 120-frame windows, median of the final five, foreground checks, idle `ioreg` snapshots, §12 confounders) plus the thermal record from `plans/done/per-frame-upload-batching`, on both maps, vsync on and off. Mac/Metal only; Windows is a handoff.

## Acceptance
### Automated
- [ ] With CPU timing on, a counted in-level window reports the frame's pass count as a count row under the render stage on the log line and in the capture measurement report.
- [ ] A frame whose surface acquire yields nothing reports the pass count absent, not zero; with timing off nothing is counted or allocated.
- [ ] On the same map and pose, a frame with the bloom switch off reports fewer passes than with it on by exactly the bloom chain's length; a map without fog volumes reports no fog passes.
- [ ] The `dev-tools` overlay's submit changes no pass count.
- [ ] A render or compute pass begun anywhere in renderer non-test source that is not counted fails a source-derived guard.
### Manual
- [ ] Honesty gate: on both maps the reported pass count per frame matches the Metal System Trace's unique labelled encoders per camera frame for the traced pass groups, within trace-boundary frames.
- [ ] Measured finding, release at the current head: `work`, `render_submit`, `render_record`, `render_prep` and pass-count medians on both maps, spawn pose, warm shadow cache, vsync on; the first post-`visible-span-draws` record.
- [ ] Measured finding: the same with vsync off plus total frame time, and the ratio of `work` to frame time, on both maps. Report whether either map is CPU-bound.
- [ ] Measured finding: `render_submit` with bloom on versus off on both maps; the delta divided by the chain's pass count, reported as µs per pass with window spread.
- [ ] Measured finding: how many per-slot entity shadow passes per frame draw no occluder on each map.
- [ ] Each run records `ioreg` idle VRAM and utilization, CPU speed limit and GPU temperature at window start and end.
- [ ] Findings note in the plan of record: the stage split by bucket (per pass, per command buffer, staging free, per draw, remainder), the candidate levers from `research.md` with their measured stakes, and a recommendation on which brief to draft next or that none is warranted.

## Path
Non-binding.
- Count precedent: `RenderStage::MeshPoseSamples` (`StageKind::Count`) in `cpu_stages.rs`; the count is read from the `[CpuTiming]` line and `cpu_stages` in the capture report. Pass sites: `record_scene_passes`, `record_pre_scene_compute`, `record_direct_sh_pre_scene_compute`, `record_spot_shadow_depth`, `record_cube_shadow_depth`, `record_depth_and_sdf_passes`, `Bloom::record`, `UiPass::encode`, `ScreenEffects::encode_resolve`, the fog, smoke, wireframe and debug-line passes, and the cull, animated-lightmap, SH, direct-SH, scatter and SDF dispatch helpers.
- Shape: count at each site through the frame's CPU-stage handle, with the guard pinning coverage. Strongest rival: a renderer-owned encoder wrapper that owns `begin_render_pass` and counts at one chokepoint. Better long term and the seam a pass-merging brief would want, but it touches every pass signature; too wide for a spike.
- First slice: the count row and its guard, then bloom on/off on the hallway. That falsifies the riskiest assumption cheaply: that the per-pass cost is a flat ~35–40 µs rather than dominated by a few heavy passes.
- Metal trace method, parser and summaries: `measurements/release-indirect-validation/runtime` (`run-runtime.py`, `parse-trace.py`); reuse rather than rewrite.
- `renderer_render_frame.rs` is past the file-size guidance; the spike adds only count calls, so no split unless the chokepoint shape is chosen.

## Open questions
- Should the spike include a locally patched wgpu-hal build (lazy `MTLCommandBuffer`) to measure the transition-buffer half directly? — owner: Dan — **blocks build**. Recommendation: exclude; the profile already sizes it at ≈ 0.4–0.5 ms, and carrying a dependency patch is a decision for after the findings.
- Should a sibling seed be filed for glyphon staging churn (≈ 0.8 ms of `render_submit` plus ≈ 0.3 ms of `rec_ui` on both maps)? — owner: Dan. Recommendation: yes; it is the largest single bucket on campaign and independent of pass count.
- Method for counting draw-less entity shadow passes (existing occluder tallies versus a trace query) — **delegated**.
- Whether Vulkan or DX12 on the Windows machine shows the same per-pass share — **delegated**: reported as a handoff in the findings note, not measured here.
