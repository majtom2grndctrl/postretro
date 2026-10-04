# render-submit-pass-overhead-spike

Spike · compact · reads: `context/lib/rendering_pipeline.md` §1, §7, §10 (Target hardware), §12 · `context/lib/experimental_spikes.md` · `context/plans/done/emissive-surfaces-bloom/optimization-research.md` · `context/plans/done/mod-bloom-render-profile/index.md` · read at b26d929bc · evidence: `../render-submit-pass-overhead/research.md`

## Problem
`render_submit` is the largest CPU stage on the compatibility-floor Mac, and `../render-submit-pass-overhead/research.md` attributes a large share to per-pass wgpu/Metal work. That attribution rests on a pre-`visible-span-draws` profile, a pass count estimated by hand, and an untested assumption that one per-pass cost applies to every pass type. This spike answers: **after `visible-span-draws`, how much CPU does each pass cost, how many passes and Metal command buffers does a frame record, is the hallway CPU-bound with vsync off, and which pass-reduction levers are worth a brief?** It needs no engine code. Done means a findings note carries those numbers and a go/no-go for the sibling brief.

## Decisions
- **No engine change.** Per-pass cost comes from the bloom on/off `render_submit` delta divided by bloom's compile-time pass count. Pass and command-buffer counts come from the Metal System Trace. A throwaway counter or toggle is scope.
- **Counts come from a widened `parse-trace.py`.** It counts every labelled encoder and every Metal command buffer per camera frame, including empty "(wgpu internal) Pre Pass" buffers, instead of the world/shadow allowlist. Dedupe by command-buffer id and encoder id, as today. A script change, not engine code.
- **Per-pass cost does not transfer across pass types until checked.** Bloom passes are one-draw colour passes; promoted entity shadow passes clear depth and often draw nothing. The findings report a bloom-derived figure and a shadow-pass figure separately, or mark the shadow figure unverified.
- **Every lever is classified Metal-only or all-backends.** wgpu-hal 29 Metal creates a fresh `MTLCommandBuffer` per `begin_encoding`; Vulkan reuses pooled buffers. Mac CPU is a real performance target (owner ruling), so Metal-only levers count fully, labelled.
- **Bloom lever order follows `optimization-research.md`.** Fewer levels first (each dropped level removes four passes); an algorithm change goes through a Windows GPU-timing handoff. Mod-authored bloom resolution and pixelated profiles must survive any bloom change.
- **Non-goal: a pass counter in the engine.** A follow-on pass-reduction brief owns a free-function `begin_*_pass` chokepoint with a Count row and a one-rule drift guard, the shape `plans/done/per-frame-upload-batching` used for its submit chokepoint. The findings recommend it; the spike does not build it. Warrant: the spike's question is answered by the trace.
- **Non-goal: a patched or forked wgpu-hal.** Owner ruling; the findings size the stake and stop.
- **Non-goal: glyphon staging churn.** A sibling brief owns it; the findings only keep it as its own bucket.
- **Conditions pin to `plans/done/release-indirect-validation`:** same release binary, no `dev-tools`, window in front, `POSTRETRO_CPU_TIMING=1`, 120-frame windows, median of the final five, at least five windows per run. The timing run and the trace run are separate (no profiler during timing).

## Acceptance
### Honesty gates (pass/fail)
- [ ] The widened `parse-trace.py` run on a recorded trace export reports labelled-encoder and command-buffer counts per camera frame, and its labelled-encoder totals match the old allowlist output on the same export for the labels both cover.
- [ ] The bloom pass count used as divisor equals `BLOOM_LEVEL_COUNT` (read from `bloom.rs` at the measured head) in the formula `1 + (L-1) + 2L + (L-1) + 1`, and the trace's bloom-labelled encoder count per frame confirms it.
- [ ] Every run's bloom-on and bloom-off windows share one binary digest, map, pose, vsync state and shadow-cache state (warm: no world-cache refresh encoders in the trace).
- [ ] Every window is screened for the intermittent 150-260 ms slow-frame regime and discarded and re-run if present; the note records how many were discarded.
- [ ] Each run records `ioreg -c IOAccelerator` idle VRAM and utilisation, CPU speed limit and GPU temperature at window start and end (§12).
- [ ] Release was built at the head after `visible-span-draws`; the note records the commit.

### Measured findings (measure-and-report)
- [ ] `work`, `render_submit`, `render_record`, `render_prep` and total frame medians on stress-warren-hallway-inspection and campaign-test, vsync on and vsync off, with window spread. Report whether each map is CPU-bound with vsync off (`work` against frame time).
- [ ] Bloom-derived µs per pass: `render_submit` delta (bloom on minus off) divided by the pass count, per map, with spread.
- [ ] Passes per frame and Metal command buffers per frame, per map, from the widened trace.
- [ ] Draw-less promoted entity shadow passes per frame per map, and a per-pass cost estimate for them from a second discriminator (see Path), or an explicit "unverified" with the reason.
- [ ] Post-`visible-span-draws` `render_submit` bucket split (per pass, per command buffer, staging free, per draw, remainder) from a separate `sample` run, set against the sibling research's table.
- [ ] Whether upstream wgpu has changed eager Metal command-buffer creation (release notes or source); report only.

### Findings note (last deliverable)
- [ ] A findings note in this plan folder, with: per-pass µs (bloom-derived and shadow-pass, labelled as in Decisions); pass and command-buffer counts per map; vsync-off boundness per map; the bucket split; each candidate lever from the sibling research classified Metal-only or all-backends with an estimated stake in ms; the chokepoint-plus-drift-guard recommendation for the follow-on; and a go/no-go for the sibling brief, with the Windows GPU-timing handoff named for any bloom algorithm change.

## Path
Non-binding.
- Harness: `RUNTIME_PROFILE=0 python3 measurements/release-indirect-validation/runtime/run-runtime.py <label> release <map>.prl <on|unset>` for timing (bloom off via `POSTRETRO_BLOOM=0` in the environment); `RUNTIME_TRACE=1` for the trace; `profile-cost.py` for `sample` attribution. Vsync toggles live at runtime through the diagnostics `ToggleVsync` action (Alt+Shift+V) and discard the partial window; confirm the present mode from the log.
- Trace: capture and export per §12 (`xctrace export` of `metal-gpu-intervals`), then delete the `.trace` bundle and the `instruments*.ktrace` temp file. Disk is tight; one trace at a time.
- Second discriminator for shadow passes: `--start-pose` poses on one map with different promoted-slot counts (slot occupancy visible with `POSTRETRO_SHADOW_DEBUG=1`), regressing `render_submit` on trace pass count. Draw-less count: the existing occluder tallies if a dev-tools run (never timed) can surface them, cross-checked against trace GPU interval length (a draw-less depth clear is short).
- Machine state: close the engine, snapshot `ioreg`, A/B an older run if a window looks wrong.

## Open questions
- Which poses give differing promoted-slot counts on each map? **delegated**: found during measurement with `POSTRETRO_SHADOW_DEBUG=1`; if none exist, report the shadow-pass figure unverified.
- Does the dev-tools surface the per-frame entity-occluder tally in a form a run can read? **delegated**: otherwise fall back to the trace-duration discriminator.
- Windows (Vulkan/DX12) per-pass share. **delegated**: handoff in the findings note, not measured here.
- File a sibling seed for glyphon staging churn? **blocks build**: owner Dan; recommendation: yes. The spike reports the bucket either way; the answer only decides whether the findings note names that brief.
