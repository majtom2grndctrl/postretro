# reflection-probe-fluid-pass-performance — plan of record

mode: compact
status: done
read at: e761fb131

## Corrections
- Source changed since `e36ea2d92` only in `crates/sim/src/sim/tick.rs.orig` (deleted). No cited symbol moved; Decision reads reused.
- "Against the build before" (GPU rows) → the oracle is the reflecting pipeline rebuilt from the same composed source with `fluid_reflection_ratio_from_sums(` textually restored to the kept per-fragment `fluid_reflection_ratio(`, installed through a test-only seam. That is the build before's fragment exactly: the only fragment change this brief makes. Same frame, same pose, same clock, so no clock-phase confound. A second binary from the merge-base stays the manual rows' method.
- `fluid_reflection_light_value` and its helpers move from `fluid_reflection.wgsl` into a new `fluid_reflection_lights.wgsl`, concatenated into both the fluid module and the sums module, so the sums pass reuses them by text. The kept per-fragment `fluid_reflection_ratio` stays in `fluid_reflection.wgsl`, unreachable. Existing pins retarget to the file each function now lives in; bodies unchanged.

- Row 24 (owner ruling, 2026-11-22) → "No view is slower beyond run-to-run spread" now judges whole-frame GPU time, and the report adds the sums pass's overlap. Fluid + sums double-counts the sums pass where it runs beside other GPU work (e24, which stores no animated light); see Results.

## Delegated answers
- Non-finite live values (review finding) — the sums pass caps each light's change at 65504 (f16 max, the capture and target format). An infinite change would otherwise turn a zero share or lobe weight into NaN and blank the probe, where the per-fragment form blew out only shared directions. The coefficient stays `current / ref − 1`; the per-fragment oracle is unchanged.
- "Behind a wall" (row 15) — the face is absent from the previous frame's draw list, which is what the collector hands the renderer for a hidden face; no wall geometry is needed for the renderer-side contract.
- "Forced while unpublished" (row 21) — written into A's resident descriptor mirror directly; scripts reach only the active level, so no bridge path exists for an unpublished one.
- Dispatch placement — inside the fluid block, after the scene-input upload and before the SSR pass, gated by the fluid pass's own reflecting draws under the frame's mask snapshot and tier. The condition is the fluid pass's own (P6), and the span to the fluid pass then holds only the SSR pass, which writes no descriptor or sample buffer.
- One buffer or a copy — one buffer, `STORAGE | UNIFORM`, written `read_write` by the sums pass and bound as a uniform in fluid group 4. WebGPU allows the combined usage; wgpu tracks the storage-write → uniform-read transition between passes on every backend. DX12 binds uniforms as CBVs at 256-byte granularity: 20 480 B = 80 × 256. Metal proven by this machine's GPU gates; Vulkan by the 1660 run; DX12 by the FXC compile plus the Windows GPU gates (outstanding here).
- Workgroup width — 64; one invocation per probe, `ceil(probe_count / 64)` workgroups. Test probe counts: 1, 65, 256.
- Sums pass layout — group 0 the shared camera layout (already COMPUTE-visible; SH compose binds it), group 1 the shared SH layout (group 3 in the fluid pass; its descriptor and sample entries already carry COMPUTE), group 2 its own: binding 0 sums (`read_write` storage), binding 1 share table (uniform, over the install buffer). The pass binds the level's existing camera and SH groups, so it builds no group over the descriptor buffers.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 parity of blended sums vs per-fragment ratio | `ratio_blended_from_probe_sums_matches_the_per_fragment_ratio` (render-cpu) | achievable as stated | pass: render-cpu `reflection` 89 passed |
| 2 frame ordering (P1) | `the_sums_pass_records_after_the_flush_and_before_the_fluid_pass` + `the_frames_one_descriptor_flush_precedes_the_fluid_pass` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 3 entry globals | `the_reflection_free_entry_declares_none_of_the_reflection_globals`, `the_reflection_free_variant_compiles_no_reflection_into_its_executable_path` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 4 dispatch condition snapshot (P6) | `the_sums_dispatch_reads_the_fluid_passs_own_reflecting_draws`, `fluid_pass_picks_variants_by_the_frames_group_0_mask` + GPU frame (worker) | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 5 IP fences | `the_sums_ip_fences_hold` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 6 light-evaluation source gates retargeted | `the_sums_pass_light_evaluation_reads_no_promotion_mask_or_sh` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 7 GPU-free references + text pins | `the_sums_pass_and_blend_wgsl_match_the_cpu_references`, `fluid_reflection_wgsl_bodies_match_the_cpu_reference` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 8 no-history inventory | `fluid_pass_declares_no_history_or_precomputed_intersection_binding` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 9 budget pins incl. compute | `fluid_sums_pipeline_compute_bindings_stay_within_limits`, `fluid_pipeline_fragment_uniforms_and_samplers_stay_within_limits` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 10 labelled timing pair | `the_sums_pass_owns_a_labelled_timing_pair` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 11 lifetime/creation source gates | `the_sums_live_with_the_level_and_their_pipeline_builds_at_full_init` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 12 dispatch counts (P2) | `sums_pass_dispatches_once_per_reflecting_frame_and_never_without_a_reflecting_draw` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 13 P6 low tier / planar plane / compose-only | `sums_pass_follows_the_fluid_pass_reflecting_draws_at_low_and_on_the_selected_plane`, `a_mask_set_after_the_uniform_upload_moves_neither_variants_nor_dispatch_until_the_next` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 14 script switch on N (P1) | `a_script_switching_a_stored_light_off_drops_it_from_the_reflection_on_that_frame` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 15 face first drawn / undrawn probe readback (P3) | `a_reflecting_face_first_drawn_this_frame_shows_its_forced_light_and_undrawn_sums_are_current` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 16 all probes readback × 2 frames, counts 1/65/256 | `every_installed_probes_sums_match_the_cpu_reference_on_consecutive_frames` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 17 no creations on reflecting frames | `a_reflecting_frame_creates_no_gpu_object_beyond_a_reflection_free_frame` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 18 vs build before, pixel/smooth, animated curve (P4) | `the_reflection_matches_the_build_before_in_both_styles_and_on_an_animated_curve` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 19 poison prefill | `sums_prefilled_with_poison_render_like_cleared_sums_on_the_first_published_frame` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 20 level B publish (P5) | `a_level_built_beside_the_drawing_one_reads_its_own_probes_on_its_first_published_frame` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 21 level A re-publish (P7) | `a_level_published_again_reads_its_lights_changed_while_resident_and_builds_nothing` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 22 reflection-free and Off unchanged | `the_reflection_free_variant_and_every_off_case_draw_exactly_as_the_build_before` | achievable as stated | pass on Metal (5300M): renderer `fluid` filter 126 passed with POSTRETRO_REQUIRE_GPU=1; render-cpu `reflection` 89 passed |
| 23 DX12 FXC | `dx12_fxc_compiles_every_full_init_pipeline` on a Windows DX12 adapter | Windows machine; outstanding here | outstanding: needs a Windows DX12 adapter |
| 24 5300M A/B | owner machine, Metal System Trace kit | manual | pass (owner-reworded, see Corrections): whole-frame GPU time −3.0 ms probes 4K, −0.47 ms probes Auto, e24 views within run-to-run spread; see Results |
| 25 GTX 1660 A/B | owner, Windows | manual | outstanding: owner, Windows |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Behaviour-preserving splits: the frame's fluid block out of `renderer_render_frame.rs`; the level-install half out of `fluid_surface.rs`; retarget source gates | integrating executor | — | done: `renderer_fluid_frame.rs`, `fluid_surface/install.rs`; renderer lib 1440 pass |
| 2 | Thin slice: CPU refs (sums, blend) + parity test; shaders; sums buffer, layouts, pipeline at full init; dispatch in the fluid block; existing GPU live-lighting case passes | integrating executor | 1 | done: parity test (12k cases), fluid suite 105 pass on Metal incl. `a_probe_reads_darker_by_its_lights_share_when_its_descriptor_switches_off` |
| 3 | Source gates and pins (AC 2–11) | integrating executor | 2 | done |
| 4 | GPU gates (AC 12–22) | worker (delegated), integrated by executor | 2 | done: 13 GPU tests pass on Metal; a negative control (sums dispatch disabled) failed 8 of them |
| 5 | Register screen of `fs_main_reflecting` before/after; 5300M A/B at probes 4K first, then near/mid and Auto | integrating executor + owner | 2 | done: register screen 72 VGPRs before and after (SGPRs 102 → 98); A/B 24/24 runs valid (`measurements/fluid-base-pass-cost/results-sums-ab.md`) |
| 6 | Preflight, review panel, fixes | integrating executor | 3, 4 | review panel (7 lenses, no 🔴) and fixes done; focused gate and clippy clean; owner preflight outstanding |

## Results

5300M A/B, 2026-11-22 (`measurements/fluid-base-pass-cost/results-sums-ab.md`). Before: merge-base `e761fb131`; after: `19f7d6377`. Mask `0x17F`, 2 interleaved rounds, 120 + 120 frames, maps rebaked with the current compiler. All 24 runs valid, complete-frame basis. Fluid + sums in ms (per-run values), GPU frame in ms.

| View | Fluid + sums before | Fluid + sums after | Δ | Sums pass | GPU frame before → after |
|---|---|---|---|---|---|
| probes 4K | 15.33, 15.37 | 12.32, 12.34 | −3.02 (−19.7%) | 0.032 | 60.79 → 57.80 |
| probes Auto | 2.84, 2.90 | 2.36, 2.36 | −0.51 (−17.9%) | 0.024 | 12.30 → 11.83 |
| near 4K | 12.82, 12.81 | 12.92, 12.92 | +0.10 (+0.8%) | 0.068 | 60.80 → 60.83 |
| near Auto | 2.47, 2.46 | 2.51, 2.51 | +0.05 (+1.9%) | 0.040 | 11.52 → 11.53 |
| mid 4K | 6.79, 6.76 | 6.85, 6.86 | +0.08 (+1.2%) | 0.067 | 48.86 → 48.77 |
| mid Auto | 1.43, 1.42 | 1.47, 1.47 | +0.05 (+3.2%) | 0.043 | 9.88 → 9.88 |

SSR and forward passes unchanged within spread on every view.

- The saving is the probes map's: it stores animated lights per probe, so its fragments ran the light loop. e24 stores none (its bake skips the animated direct SH bake), so its per-fragment loop already found every slot empty and the sums pass is added cost with nothing to remove.
- Row 24's "No view is slower beyond run-to-run spread" fails for fluid + sums on the three e24 views: +0.05 to +0.10 ms against a two-run spread of at most 0.03 ms. Whole-frame GPU time on those views moves −0.09 to +0.03 ms, within its own run-to-run spread (up to 0.28 ms).

### Where the e24 cost goes

Overlap = sum of labelled pass times − union frame time (both per complete frame, two-run means). On e24 the overlap grows by the sums pass's own time, so the sums dispatch runs beside other labelled GPU work and adds nothing to the frame's busy span. The residual is the fluid pass's sums read, the same order as the drift between builds in passes this brief did not touch.

| View | Sums pass | Δ overlap | Δ fluid pass | Largest Δ of an untouched pass | Δ GPU frame |
|---|---|---|---|---|---|
| near 4K | 0.068 | +0.069 | +0.036 | 0.029 | +0.026 |
| mid 4K | 0.067 | +0.066 | +0.014 | 0.086 | −0.090 |
| near Auto | 0.040 | +0.039 | +0.006 | 0.038 | +0.015 |
| mid Auto | 0.043 | +0.042 | +0.002 | 0.007 | −0.004 |
| probes 4K | 0.032 | +0.024 | −3.056 | 0.091 | −2.995 |
| probes Auto | 0.024 | +0.008 | −0.537 | 0.106 | −0.472 |

- e24's probes (13) and the probes map's (3) both fit one workgroup, so the dispatch's work is the same; the sums pass's interval tracks resolution and scene instead, as a concurrent span would.
- Skipping the dispatch on a level that stores no light would save about nothing in frame time; the fluid pass's read (up to +0.036 ms, 0.3% at near 4K) would stay.
