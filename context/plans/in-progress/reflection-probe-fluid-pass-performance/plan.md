# reflection-probe-fluid-pass-performance — plan of record

mode: compact
status: active
read at: e761fb131

## Corrections
- Source changed since `e36ea2d92` only in `crates/sim/src/sim/tick.rs.orig` (deleted). No cited symbol moved; Decision reads reused.
- "Against the build before" (GPU rows) → the oracle is the reflecting pipeline rebuilt from the same composed source with `fluid_reflection_ratio_from_sums(` textually restored to the kept per-fragment `fluid_reflection_ratio(`, installed through a test-only seam. That is the build before's fragment exactly: the only fragment change this brief makes. Same frame, same pose, same clock, so no clock-phase confound. A second binary from the merge-base stays the manual rows' method.
- `fluid_reflection_light_value` and its helpers move from `fluid_reflection.wgsl` into a new `fluid_reflection_lights.wgsl`, concatenated into both the fluid module and the sums module, so the sums pass reuses them by text. The kept per-fragment `fluid_reflection_ratio` stays in `fluid_reflection.wgsl`, unreachable. Existing pins retarget to the file each function now lives in; bodies unchanged.

## Delegated answers
- Dispatch placement — inside the fluid block, after the scene-input upload and before the SSR pass, gated by the fluid pass's own reflecting draws under the frame's mask snapshot and tier. The condition is the fluid pass's own (P6), and the span to the fluid pass then holds only the SSR pass, which writes no descriptor or sample buffer.
- One buffer or a copy — one buffer, `STORAGE | UNIFORM`, written `read_write` by the sums pass and bound as a uniform in fluid group 4. WebGPU allows the combined usage; wgpu tracks the storage-write → uniform-read transition between passes on every backend. DX12 binds uniforms as CBVs at 256-byte granularity: 20 480 B = 80 × 256. Metal proven by this machine's GPU gates; Vulkan by the 1660 run; DX12 by the FXC compile plus the Windows GPU gates (outstanding here).
- Workgroup width — 64; one invocation per probe, `ceil(probe_count / 64)` workgroups. Test probe counts: 1, 65, 256.
- Sums pass layout — group 0 the shared camera layout (already COMPUTE-visible; SH compose binds it), group 1 the shared SH layout (group 3 in the fluid pass; its descriptor and sample entries already carry COMPUTE), group 2 its own: binding 0 sums (`read_write` storage), binding 1 share table (uniform, over the install buffer). The pass binds the level's existing camera and SH groups, so it builds no group over the descriptor buffers.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 parity of blended sums vs per-fragment ratio | `ratio_blended_from_probe_sums_matches_the_per_fragment_ratio` (render-cpu) | achievable as stated |
| 2 frame ordering (P1) | source-order gates: app frame loop and capture setup; renderer `record_scene_passes` timing prefill < sums dispatch < fluid pass, nothing between writes descriptors/samples | achievable as stated |
| 3 entry globals | naga `global_uses` test: no entry uses `anim_descriptors` or reaches `fluid_reflection_ratio`; reflecting entry uses `fluid_probe_sums`; reflection-free uses none | achievable as stated |
| 4 dispatch condition snapshot (P6) | source gate on the frame's one mask/tier snapshot + GPU frame with a mask set after upload | achievable as stated |
| 5 IP fences | grep gates over layouts, usages, consumers | achievable as stated |
| 6 light-evaluation source gates retargeted | existing gates extended to `fluid_reflection_sums.wgsl` | achievable as stated |
| 7 GPU-free references + text pins | render-cpu refs; text pins for sums shader and fragment blend | achievable as stated |
| 8 no-history inventory | sums shader binding inventory test | achievable as stated |
| 9 budget pins incl. compute | `pipeline_budget_tests.rs` | achievable as stated |
| 10 labelled timing pair | timing label gate + pair recorded in the sums pass | achievable as stated |
| 11 lifetime/creation source gates | grep gates | achievable as stated |
| 12 dispatch counts (P2) | GPU gate, dispatch counter | achievable as stated |
| 13 P6 low tier / planar plane / compose-only | GPU gate | achievable as stated |
| 14 script switch on N (P1) | GPU gate | achievable as stated |
| 15 face first drawn / undrawn probe readback (P3) | GPU gate | achievable as stated |
| 16 all probes readback × 2 frames, counts 1/65/256 | GPU gate | achievable as stated |
| 17 no creations on reflecting frames | GPU gate, `gpu_creation_counts` | achievable as stated |
| 18 vs build before, pixel/smooth, animated curve (P4) | GPU gate, per-fragment oracle pipeline (Corrections) | achievable as stated |
| 19 poison prefill | GPU gate | achievable as stated |
| 20 level B publish (P5) | GPU gate | achievable as stated |
| 21 level A re-publish (P7) | GPU gate | achievable as stated |
| 22 reflection-free and Off unchanged | GPU gate | achievable as stated |
| 23 DX12 FXC | `dx12_fxc_compiles_every_full_init_pipeline` on a Windows DX12 adapter | Windows machine; outstanding here |
| 24 5300M A/B | owner machine, Metal System Trace kit | manual |
| 25 GTX 1660 A/B | owner, Windows | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Behaviour-preserving splits: the frame's fluid block out of `renderer_render_frame.rs`; the level-install half out of `fluid_surface.rs`; retarget source gates | integrating executor | — | done: `renderer_fluid_frame.rs`, `fluid_surface/install.rs`; renderer lib 1440 pass |
| 2 | Thin slice: CPU refs (sums, blend) + parity test; shaders; sums buffer, layouts, pipeline at full init; dispatch in the fluid block; existing GPU live-lighting case passes | integrating executor | 1 | done: parity test (12k cases), fluid suite 105 pass on Metal incl. `a_probe_reads_darker_by_its_lights_share_when_its_descriptor_switches_off` |
| 3 | Source gates and pins (AC 2–11) | integrating executor | 2 | |
| 4 | GPU gates (AC 12–22) | integrating executor | 2 | |
| 5 | Register screen of `fs_main_reflecting` before/after; 5300M A/B at probes 4K first, then near/mid and Auto | integrating executor + owner | 2 | |
| 6 | Preflight, review panel, fixes | integrating executor | 3, 4 | |
