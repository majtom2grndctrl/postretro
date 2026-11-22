# reflection-probe-fluid-pass-performance — plan of record

mode: compact
status: active
read at: e761fb131

## Corrections
- Source changed since `e36ea2d92` only in `crates/sim/src/sim/tick.rs.orig` (deleted). No cited symbol moved; Decision reads reused.
- "Against the build before" (GPU rows) → the oracle is the reflecting pipeline rebuilt from the same composed source with `fluid_reflection_ratio_from_sums(` textually restored to the kept per-fragment `fluid_reflection_ratio(`, installed through a test-only seam. That is the build before's fragment exactly: the only fragment change this brief makes. Same frame, same pose, same clock, so no clock-phase confound. A second binary from the merge-base stays the manual rows' method.
- `fluid_reflection_light_value` and its helpers move from `fluid_reflection.wgsl` into a new `fluid_reflection_lights.wgsl`, concatenated into both the fluid module and the sums module, so the sums pass reuses them by text. The kept per-fragment `fluid_reflection_ratio` stays in `fluid_reflection.wgsl`, unreachable. Existing pins retarget to the file each function now lives in; bodies unchanged.

## Delegated answers
- Non-finite live values (review finding) — the sums pass caps each light's change at 65504 (f16 max, the capture and target format). An infinite change would otherwise turn a zero share or lobe weight into NaN and blank the probe, where the per-fragment form blew out only shared directions. The coefficient stays `current / ref − 1`; the per-fragment oracle is unchanged.
- "Behind a wall" (row 15) — the face is absent from the previous frame's draw list, which is what the collector hands the renderer for a hidden face; no wall geometry is needed for the renderer-side contract.
- "Forced while unpublished" (row 21) — written into A's resident descriptor mirror directly; scripts reach only the active level, so no bridge path exists for an unpublished one.
- Dispatch placement — inside the fluid block, after the scene-input upload and before the SSR pass, gated by the fluid pass's own reflecting draws under the frame's mask snapshot and tier. The condition is the fluid pass's own (P6), and the span to the fluid pass then holds only the SSR pass, which writes no descriptor or sample buffer.
- One buffer or a copy — one buffer, `STORAGE | UNIFORM`, written `read_write` by the sums pass and bound as a uniform in fluid group 4. WebGPU allows the combined usage; wgpu tracks the storage-write → uniform-read transition between passes on every backend. DX12 binds uniforms as CBVs at 256-byte granularity: 20 480 B = 80 × 256. Metal proven by this machine's GPU gates; Vulkan by the 1660 run; DX12 by the FXC compile plus the Windows GPU gates (outstanding here).
- Workgroup width — 64; one invocation per probe, `ceil(probe_count / 64)` workgroups. Test probe counts: 1, 65, 256.
- Sums pass layout — group 0 the shared camera layout (already COMPUTE-visible; SH compose binds it), group 1 the shared SH layout (group 3 in the fluid pass; its descriptor and sample entries already carry COMPUTE), group 2 its own: binding 0 sums (`read_write` storage), binding 1 share table (uniform, over the install buffer). The pass binds the level's existing camera and SH groups, so it builds no group over the descriptor buffers.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 parity of blended sums vs per-fragment ratio | `ratio_blended_from_probe_sums_matches_the_per_fragment_ratio` (render-cpu) | achievable as stated |
| 2 frame ordering (P1) | `the_sums_pass_records_after_the_flush_and_before_the_fluid_pass` + `the_frames_one_descriptor_flush_precedes_the_fluid_pass` | achievable as stated |
| 3 entry globals | `the_reflection_free_entry_declares_none_of_the_reflection_globals`, `the_reflection_free_variant_compiles_no_reflection_into_its_executable_path` | achievable as stated |
| 4 dispatch condition snapshot (P6) | `the_sums_dispatch_reads_the_fluid_passs_own_reflecting_draws`, `fluid_pass_picks_variants_by_the_frames_group_0_mask` + GPU frame (worker) | achievable as stated |
| 5 IP fences | `the_sums_ip_fences_hold` | achievable as stated |
| 6 light-evaluation source gates retargeted | `the_sums_pass_light_evaluation_reads_no_promotion_mask_or_sh` | achievable as stated |
| 7 GPU-free references + text pins | `the_sums_pass_and_blend_wgsl_match_the_cpu_references`, `fluid_reflection_wgsl_bodies_match_the_cpu_reference` | achievable as stated |
| 8 no-history inventory | `fluid_pass_declares_no_history_or_precomputed_intersection_binding` | achievable as stated |
| 9 budget pins incl. compute | `fluid_sums_pipeline_compute_bindings_stay_within_limits`, `fluid_pipeline_fragment_uniforms_and_samplers_stay_within_limits` | achievable as stated |
| 10 labelled timing pair | `the_sums_pass_owns_a_labelled_timing_pair` | achievable as stated |
| 11 lifetime/creation source gates | `the_sums_live_with_the_level_and_their_pipeline_builds_at_full_init` | achievable as stated |
| 12 dispatch counts (P2) | `sums_pass_dispatches_once_per_reflecting_frame_and_never_without_a_reflecting_draw` | achievable as stated |
| 13 P6 low tier / planar plane / compose-only | `sums_pass_follows_the_fluid_pass_reflecting_draws_at_low_and_on_the_selected_plane`, `a_mask_set_after_the_uniform_upload_moves_neither_variants_nor_dispatch_until_the_next` | achievable as stated |
| 14 script switch on N (P1) | `a_script_switching_a_stored_light_off_drops_it_from_the_reflection_on_that_frame` | achievable as stated |
| 15 face first drawn / undrawn probe readback (P3) | `a_reflecting_face_first_drawn_this_frame_shows_its_forced_light_and_undrawn_sums_are_current` | achievable as stated |
| 16 all probes readback × 2 frames, counts 1/65/256 | `every_installed_probes_sums_match_the_cpu_reference_on_consecutive_frames` | achievable as stated |
| 17 no creations on reflecting frames | `a_reflecting_frame_creates_no_gpu_object_beyond_a_reflection_free_frame` | achievable as stated |
| 18 vs build before, pixel/smooth, animated curve (P4) | `the_reflection_matches_the_build_before_in_both_styles_and_on_an_animated_curve` | achievable as stated |
| 19 poison prefill | `sums_prefilled_with_poison_render_like_cleared_sums_on_the_first_published_frame` | achievable as stated |
| 20 level B publish (P5) | `a_level_built_beside_the_drawing_one_reads_its_own_probes_on_its_first_published_frame` | achievable as stated |
| 21 level A re-publish (P7) | `a_level_published_again_reads_its_lights_changed_while_resident_and_builds_nothing` | achievable as stated |
| 22 reflection-free and Off unchanged | `the_reflection_free_variant_and_every_off_case_draw_exactly_as_the_build_before` | achievable as stated |
| 23 DX12 FXC | `dx12_fxc_compiles_every_full_init_pipeline` on a Windows DX12 adapter | Windows machine; outstanding here |
| 24 5300M A/B | owner machine, Metal System Trace kit | manual |
| 25 GTX 1660 A/B | owner, Windows | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Behaviour-preserving splits: the frame's fluid block out of `renderer_render_frame.rs`; the level-install half out of `fluid_surface.rs`; retarget source gates | integrating executor | — | done: `renderer_fluid_frame.rs`, `fluid_surface/install.rs`; renderer lib 1440 pass |
| 2 | Thin slice: CPU refs (sums, blend) + parity test; shaders; sums buffer, layouts, pipeline at full init; dispatch in the fluid block; existing GPU live-lighting case passes | integrating executor | 1 | done: parity test (12k cases), fluid suite 105 pass on Metal incl. `a_probe_reads_darker_by_its_lights_share_when_its_descriptor_switches_off` |
| 3 | Source gates and pins (AC 2–11) | integrating executor | 2 | done |
| 4 | GPU gates (AC 12–22) | worker (delegated), integrated by executor | 2 | done: 13 GPU tests pass on Metal; a negative control (sums dispatch disabled) failed 8 of them |
| 5 | Register screen of `fs_main_reflecting` before/after; 5300M A/B at probes 4K first, then near/mid and Auto | integrating executor + owner | 2 | register screen done: 72 VGPRs before and after (SGPRs 102 → 98); A/B after review |
| 6 | Preflight, review panel, fixes | integrating executor | 3, 4 | review panel (7 lenses, no 🔴) and fixes done; focused gate and clippy clean; owner preflight outstanding |
