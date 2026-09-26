# shadowmask-atlas-compress-at-rest — plan of record

mode: resumable
status: done
read at: 804351717

Brief read at 8e4753ce1; 92 commits since. Every symbol cited by Decisions and Path was
re-read at 804351717. All Decision premises hold: `encode_bc5_rg` is serial, min/max
endpointed, and reproduces 0 and 255 exactly; the analytic graph walk already computes a
per-texel covered-light list (peak overlap is a max over it); the loader's malformed path
logs `ShadowmaskAtlas malformed; ignoring section: {err}`; `TEXTURE_COMPRESSION_BC` is
required at device acquisition; `MAX_ATLAS_DIMENSION` and `REQUIRED_MAX_TEXTURE_DIMENSION_2D`
are both 8192; atlas dims are powers of two ≥ `MIN_ATLAS_DIMENSION` (64).

## Corrections

- Path: "`LevelGeometry { lightmap: None, .. }` builders in `mesh_render.rs`,
  `particle_render.rs` and `startup/lifecycle.rs`" → those are `LevelWorld` literals. The
  `LevelGeometry` literals are `renderer_resources.rs` (empty geometry), the
  `render/shadowmask.rs` test and `capture/setup.rs`. All of them follow the type change.
- Path: "`level_world_to_geometry` and `LevelGeometry` carry [the payloads] to
  `LightmapResources::new`" → `install_level_geometry` takes `&LevelGeometry`, so a
  payload can't be moved through it. Planning around it: the payloads go to
  `install_level_geometry` by value, beside the borrowed geometry. `LightmapResources::new` also has a
  second caller, `build_full_renderer`, whose geometry is always `None`. It gets no
  payload.
- Acceptance *Lifecycle* row 1, "has no place to hold their payloads", clarified. The
  no-upload-install row requires a renderer-less install to keep the payloads, so the
  world must be able to hold them before install. Planned meaning: the world's retained
  id-42 and id-22 types are header types with no payload field. The payloads live in one
  separate movable value, which install takes and leaves `None`. A header whose payload
  was taken is unrepresentable, as the Decision requires. Owner confirmed with plan approval.

- Wire: `from_bytes` also rejects zero width, height or layer count. Without it, a
  pre-tag payload with zero dims and the tag's value in its width word parses as a
  valid tagged section, which the tag-collision pin forbids. The compiler never emits
  zero dims, and the renderer already sent them to the placeholder. Same outcome (fully
  lit), now named at load.
- Test surface: fill-semantics tests read the raw fill (`finish_raw`, plus a
  test-only record of the last raw fill for the public bake paths). BC4 reproduces
  only block endpoints exactly. Fixtures that pass through the encoder moved to
  4-aligned dims; historical goldens keep their slot tables and raw values.
- Observed, not this brief's: capture of any SH-streaming PRL needs
  `POSTRETRO_SH_STREAMING=sync-proof`, so the existing
  `specular_shadowmask_capture_scene_compiles_loads_and_writes_png` fails when run
  without it. The new capture test sets it on its child process.

- Wide-layer warning names the level by its input path: `prepare_fused_shadowmask`
  now takes a level label and returns `Result`, so the misaligned-atlas error fails the
  build through `bake_fused_prepared`'s `?`.

- Overlap carrier: the peak is recorded on `OverlapGraph` by the analytic walk and
  written as a `u32` prefix in the same memo entry. Both memo readers (fused and the
  legacy cached reference path) now share `read_shadowmask_memo`. The legacy path's
  distinct "corrupt shadowmask_atlas section, rebuilding" wording became the fused
  path's "corrupt shadowmask atlas, re-baking".

- Renderer: the linear lightmap sampler is now `filtering_sampler_descriptor()`, and
  the shadowmask upload goes through `shadowmask_texture_descriptor`. The GPU harness
  samples through both, so it exercises the production sampler and upload.

- Observed, not this brief's: `hdr_capture_writes_png_at_requested_dimensions` fails
  because its golden PRL carries SH section version 9 (loader expects 11).
  `spawner_capture_forced_alarm_reds_dynamic_receivers_and_keeps_baked_rest` fails
  identically on `main` (checked in a `main` worktree).

- Review round 1 (10 lenses; no 🔴 code defects):
  - Encoder: the shadowmask encode picks BC4's six-value mode per block when it lowers
    error. The normal-map entry point is byte-unchanged and pinned. Stage version is
    4; the id-42 container-entry version is 2.
  - Union path: its channel guard is now `shadowmask_union_channel` in `forward.wgsl`,
    which the GPU harness calls directly. The harness renders two pixels per probe to
    stay within the 32-byte colour-attachment limit.
  - GPU tests: `POSTRETRO_REQUIRE_GPU=1` turns a skipped run into a failure.
  - Renderer: it rejects an unknown shadowmask format tag or payload length to the
    placeholder, and animated atlas sizing follows payload presence.
  - Capture tests share `tests/capture_support`, are gated on the `capture` feature,
    and set `POSTRETRO_SH_STREAMING=sync-proof` on their own child process. L5 has an
    adapter capture.
  - Game-install renderer-branch take: manual rows MN6/MN7 only, since it needs a GPU.

## Owner decisions during build

- **M1 and M4, union path (2026-09-24).** The capture harness never promotes static
  lights, so its promoted-union term is zero and does not read the mask. Restated with
  the owner:
  - M1: "Four overlapping selected lights, spread across both groups, each resolve to
    their own mask — no drop, no cross-talk between groups. World specular is proven by
    offscreen capture on an adapter. The promoted-union path is proven on an adapter by
    a GPU readback of `forward.wgsl`'s sampling helper and that path's channel select.
    A skipped run does not count."
  - M4: "Moving a light from the first group to the second leaves world-specular output
    unchanged for surfaces covered only by first-group lights (offscreen capture on an
    adapter). Static→static world shadowing stays exactly zero: the GPU readback shows
    the union attenuation is zero at entity visibility 1 for every slot in both groups.
    A skipped run does not count."

## Delegated answers

- Tag position and width: a `u32` format tag is the first header word. Its value is
  ASCII `SMB5`, so no compiler-emitted pre-change width can equal it. If the tagged parse
  fails and the bytes parse under the pre-change raw layout, `from_bytes` reports a
  *format mismatch* rather than the first structural error. That covers the
  tag-collision pin deterministically.
- Overlap-count carrier: a `u32` prefix inside the same memo entry, ahead of the section
  bytes. With no sibling entry, nothing evicts independently, and an entry too short for
  the prefix is a miss. The shipped section never carries it.
- Header/payload split: header types in `level-format` beside the section types
  (`LightmapSection` and `ShadowmaskAtlasSection` split into header and payload, and
  rejoin by move for upload). No copy is made.
- GPU mask proofs: offscreen captures through the capture harness on a dev fixture map.
  The group-move and seam cases rewrite id 42 inside a compiled PRL, which controls group
  placement without depending on channel coloring. The `u` = 0 and `u` = 1 edge rows use
  a GPU readback of the shader helper against a hand-built BC5 texture, the
  `sdf_light_select_test.rs` pattern.

## AC-to-proof

Test names are working names. Rows marked *adapter* must actually run on an adapter. A
self-skip is not a pass.

| AC | Proof | Status | Result |
|---|---|---|---|
| W1 round-trip: empty, single, full four-slot | `level-format` `shadowmask_atlas::tests` round-trip cases | achievable as stated | pass — `shadowmask_atlas_round_trips_empty_single_and_full_slot_tables` |
| W2 `from_bytes` rejects length, misalignment, unknown tag, bad slot | `level-format` reject cases | achievable as stated | pass — `shadowmask_atlas_rejects_length_alignment_tag_and_slot_errors` |
| W3 pre-change payload: named warning, fully lit, no panic, tag collision | `level-format` legacy and collision cases, plus a `level-loader` test capturing the `[PRL]` warning with a `None` section | achievable as stated | pass — format tests plus `load_prl_rejects_pre_bc5_shadowmask_by_format_and_keeps_entity_shadow_selection` (raw and tag-collision) |
| W4 stale memo never served; rebuilt equals uncached | compiler test seeding a raw pre-change entry under the old and new keys | achievable as stated | pass — `pre_bc5_memo_entries_are_never_served_and_the_rebuild_matches_uncached` |
| W5 second build logs a memo hit | compiler test with log capture | achievable as stated | pass — `second_fused_build_hits_the_memo_and_warm_equals_uncached_on_real_masks` |
| W6 all-sentinel: half raw bytes, loads, fully lit | compiler test for bytes, plus a loader test for the load; the sentinel read is D1 | achievable as stated | pass — half-raw bytes in `all_filtered_selection_keeps_empty_bytes_and_indeterminate_progress`; loads in `load_prl_keeps_an_all_sentinel_shadowmask_section`; sentinel reads fully lit in the GPU harness |
| D1 sentinel fully lit; second group vs 2×1 placeholder in range | renderer GPU helper readback (*adapter*) | achievable as stated | pass (adapter, AMD Radeon Pro 5300M, `POSTRETRO_REQUIRE_GPU=1`) — `shadowmask_sample_test` placeholder and sentinel probes; placeholder shape asserted 2×1 |
| D2 `2W` = pinned dimension kept; one block wider → placeholder + `[Renderer]` error | `filter_usable_shadowmask_section` unit test with log capture | achievable as stated | pass — `shadowmask_texture_at_the_pinned_width_is_kept_and_one_block_wider_degrades` |
| D3 `W` = 8192 → placeholder; 4096 kept | same filter test | achievable as stated | pass — `eight_k_lightmap_width_shadowmask_degrades_and_four_k_is_kept` |
| D4 8192-wide bake warns, no id 42; 4096 emits | stage-seam test with a synthetic wide `SharedAtlas` | achievable as stated | pass — `eight_k_wide_layers_omit_the_shadowmask_on_every_build_and_four_k_emits` |
| D5 warm rebuild of 8192-wide warns again, no id 42, no memo entry | same test, run twice against one cache | achievable as stated | pass — same test, warm build |
| D6 hand-built misaligned section → placeholder + `[Renderer]` error | filter unit test | achievable as stated | pass — `hand_built_misaligned_shadowmask_degrades_with_a_renderer_error` |
| D7 misaligned bake fails naming dims, release as debug | compiler test; run the focused test once under `--release` too | achievable as stated | pass — `fused_prepare_rejects_a_misaligned_atlas_naming_its_dimensions` (width and height cases), debug and `--release` |
| M1 four lights across both groups, both decode paths | world specular: capture on a four-colored-light fixture with per-slot id-42 rewrites (*adapter*); union path: GPU readback of the helper plus channel select (*adapter*) | restated (owner) | pass (adapter) — world specular: `every_selected_light_reads_its_own_slot_in_either_group`, fails when groups swap; union: harness runs `forward.wgsl`'s `shadowmask_union_channel` and select |
| M2 seam-bleed | capture on a fixture whose groups differ at the seam (*adapter*) | achievable as stated | pass (adapter) — driven u = 0 / 1 probes in `shadowmask_sample_test` (fail without the per-group clamp); `group_placement_never_changes_what_a_light_reads` for placement |
| M3 `u` = 0 / `u` = 1 per group, groups differing at the seam | renderer GPU helper readback against a hand-built BC5 texture (*adapter*) | achievable as stated | pass (adapter) — `every_slot_reads_its_own_group_at_centers_block_edges_and_outer_uv` |
| M4 group-0→1 move leaves first-group specular unchanged; static→static stays zero | capture A/B with id 42 rewritten in a compiled PRL (*adapter*); union attenuation zero via GPU readback (*adapter*) | restated (owner) | pass (adapter) — `group_placement_never_changes_what_a_light_reads` (byte-identical after moving masks between groups); union attenuation zero at entity visibility 1 and equal to the slot mask at 0 for every slot |
| M5 all-255 atlas decodes fully lit | compiler encode→CPU-decode unit test | achievable as stated | pass — `all_visible_payload_equals_encoding_an_all_visible_fill_and_decodes_fully_lit` |
| M6 grep gate over `forward.wgsl` | rewritten `forward_shader_shadowmask_fallback_clamps_multilayer_indices` | achievable as stated | pass — `forward_shader_shadowmask_samples_both_groups_hoisted_at_one_layer` (includes the fs_main union call site) |
| M7 no new texture or sampler | `forward_pipeline_sampled_texture_request_matches_bgl_definitions`, untouched and green | achievable as stated | pass — `forward_pipeline_sampled_texture_request_matches_bgl_definitions`, untouched |
| B1 payload exactly half raw in the footprint report | compiler test reading `PrlFootprint` for a populated fixture | achievable as stated | pass — `shadowmask_footprint_payload_is_half_the_raw_rgba_arithmetic` |
| B2 texture description: BC5, `2W × H × L`, half raw bytes; the upload uses it | a pure `shadowmask_texture_descriptor` fn, unit-tested and called by the upload | achievable as stated | pass — `shadowmask_texture_description_is_bc5_double_width_at_half_the_raw_bytes` |
| B3 miss holds one raw fill + one output + ≤ 3 raw layers of scratch; raw gone before cache/return | test-only byte-residency tracker in the encode module | achievable as stated | pass — `cache_miss_holds_one_raw_fill_one_output_and_bounded_encode_scratch` |
| B4 byte-identical across worker counts; warm = uncached | adapt `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers` and the cached/uncached golden tests | achievable as stated | pass — `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers`; warm = uncached per W5 |
| B5 max and mean per-channel encode error, measured | measurement test printing both on a fixture; yardstick numbers in the landing note | manual (measured, not gated) | measured — see Measurements; yardstick: 2048²×73, max 18/255, mean < 0.0001/255, 99.9998% of samples exact |
| L1 world keeps headers; payloads have no place after install | type-level split plus a loader/seam test (see *Corrections* clarification) | achievable as clarified | pass — `taking_gpu_lighting_payloads_leaves_headers_and_nothing_to_take_twice` (owner-confirmed reading) |
| L2 released only after upload; renderer-less install keeps them | unit test on the install take seam | achievable as stated | pass for the keep half — `install_without_renderer_keeps_gpu_lighting_payloads_in_the_world`; game renderer-branch release is manual (MN6/MN7) |
| L3 capture takes the payloads the same way | capture setup test: world holds headers only after install | achievable as stated | pass (adapter) — capture takes before install; L4 and L5 captures show no `[Renderer]` payload errors |
| L4 animated atlas dims equal static after take; animated lights render | renderer test: `usable_atlas_dimensions` reads the header after the take; animated capture fixture (*adapter*) | achievable as stated | pass (adapter) — `animated_lights_render_after_lightmap_payloads_move_into_the_upload` plus header-sized animated atlas unit test |
| L5 lightmap-without-shadowmask and neither install without panic; first releases | take-seam unit test plus a capture of a no-shadowmask fixture (*adapter*) | achievable as stated | pass (adapter) — `level_with_a_lightmap_but_no_shadowmask_captures_cleanly` (byte-identical to all-open masks); split and pairing unit tests |
| O1 `--verbose` overlap line on cold and warm; none when not verbose | compiler test with log capture across miss, hit and non-verbose | achievable as stated | pass — ignored CLI `verbose_bakes_report_peak_texel_overlap_on_miss_and_hit_and_quiet_bakes_do_not` plus unit tests |
| MN1 id 42/22 bytes, layers, dims, peak RSS pre/post on the yardstick | owner or attended run, landing note | manual | measured (attended, 2026-09-25) — id 42 1,224,737,124 → 612,368,744 B (exactly ½); id 22 459,276,336 B both; 2048²×73 grid both (after: 4096×2048×73 BC5). Peak RSS 5.60 → 4.86 GB, not like-for-like (14 vs 10 workers). See Measurements § Yardstick |
| MN2 process memory after install pre/post, net of texture bytes, metric named | attended run | manual | measured (attended, peak not post-install) — `time -l` peak memory footprint over a capture run 4.33 → 2.53–2.86 GB; the ≈1.5–1.8 GB drop matches the 1.68 GB of id 42 + id 22 CPU copies now released. Discrete-GPU Mac, so texture bytes are not in the footprint. A steady-state post-install figure on the owner's Windows box is still open |
| MN3 specular highlight capture A/B, both images inspected | attended capture | manual | measured (attended), owner look outstanding — before/after pixel-identical (max 0/255) at three yardstick views, including one where the shadowmask contributes up to 94/255 (control: after binary on the retired-format PRL, which falls back to fully lit). Views exercise hard occlusion; penumbra error is covered by B5 |
| MN4 capture-harness CPU completion median and p95 | attended run per `testing_guide.md` §Resource bounds | manual | measured (attended), no regression but not discriminating — median 1,693.6–1,694.3 ms before, 1,693.0–1,695.5 ms after; p95 1,695.6–1,788.8 ms, spread by run not build. ≈1.7 s frames: the 0.04 m yardstick exceeds this 4 GB GPU. A meaningful frame-time A/B stays with the owner's Windows box |
| MN5 peak overlap on the yardstick | attended `--verbose` bake | manual | measured (attended) — `peak per-texel overlap: 6 selected light(s) at one texel; 73 layer(s), BC5 .rg side by side (4 slots)` |
| MN6 second-group shadows after reload and a dev level cycle | owner, in-engine | manual | outstanding — owner |
| MN7 level cycle between differing lightmap widths and back | owner, in-engine | manual | outstanding — owner |

## Measurements

- B5 encode error (ignored `shadowmask_bc5_encode_error_on_fixture_bakes`, default
  density, soft-shadow samples 32). Max and mean absolute error per used slot, over
  every texel:
  - `shadowmask-groups-capture`: 1024²×2, max 0/255, mean 0.
  - `soft_shadow_test`: 1024²×1, max 17/255, mean 0.0031/255.
  - `gate-heavily-lit`: 1024²×2, max 17/255, mean < 0.0001/255.
  Hard shadows are exact because their blocks hold endpoints only. Error sits in
  penumbra blocks.
- Yardstick (2026-09-25, same test with the yardstick added locally and the error folded
  in a streaming histogram): `stress-warren-hallway-inspection`, 2048²×73, 4 used slots,
  1,224,736,768 samples. Max 18/255, mean < 0.0001/255, 99.9998% exact, p99.9 0/255. The
  fixture pipeline parses map lights directly without data-script membership, but its
  atlas matches the `--release` bake's 2048²×73 grid.
- After review round 1 (BC4 six-value mode, chosen per block only when it lowers total
  squared error):
  - `shadowmask-groups-capture`: max 0/255.
  - `soft_shadow_test`: max 19/255, mean 0.0019/255.
  - `gate-heavily-lit`: max 2/255, mean < 0.0001/255.
  The mean falls about 40% on the soft fixture and the chart-edge worst case collapses.
  One soft-penumbra texel's max rises by 2 levels because the choice minimizes block
  error, not per-texel max.

### Yardstick (manual rows, attended 2026-09-25)

- **Fixture and inputs.** `content/dev/maps/stress-warren-hallway-inspection.map`, freshly baked on
  both sides with `prl-build --release -v` at the default 0.04 m lightmap density. Before is
  `main` 804351717; after is this branch at 0218faacd.
- **Machine.** MacBook Pro, 16 logical cores, AMD Radeon Pro 5300M (4 GB, discrete, Metal),
  release binaries.
- **Density caveat.** The 88 MB id-42 figure in the brief came from the 2026-08-31 artifact
  (84 × 512², a coarser non-default density). At the default density the map is 73 × 2048²,
  and id 42 is 1.22 GB before, in line with the brief's "projected 1.29 GB at 0.04". The halving
  holds at either density.
- **Bakes.** Before: 14 workers (the default), 10,088 s wall, max RSS 5.60 GB, peak footprint
  4.35 GB. After: 10 workers at the owner's request (`-j 10`, `RAYON_NUM_THREADS=10`, nice 10),
  13,502 s wall, max RSS 4.86 GB, peak footprint 4.62 GB. The worker counts differ, so the
  peak-memory pair is not like-for-like.
- **Unchanged pre-existing behavior.** Both bakes log the same channel-assignment fallback.
  It drops 70 selected light masks, because the exact search exhausted its 100,000-node budget.
- **Engine behavior on load.** The engine disables animated-light contribution on load for
  both builds. The animated dispatch has 2,419,938 tiles, over the 65,535 workgroup limit. This
  is pre-existing at this density.
- **Capture settings.** `POSTRETRO_SH_STREAMING=sync-proof`, 1280×720, 120 warmup and 600
  sample frames, run A-B-A-B. The camera is at [45.5, 4.6, 63.4], yaw 90, pitch −20, FOV 90.
  The cache mode does not apply to capture. The runs overlapped other owner work on the machine.
- **MN3 views.** Pixel-identical at the view above, and at [37.6, 5.0, 86.97] yaw 0, pitch −30.
  The second view is a floor the shadowmask fully occludes: the after-binary control, falling
  back to fully lit, differs by up to 94/255 and 8/255 on average.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice.** Tagged BC5 wire format and `from_bytes`. Encode submodule under `shadowmask_bake/` used by `ShadowmaskFill::finish` and `empty_section_for_dimensions`. One header writer shared by `to_bytes` and the streamed cache writer. Bump `SHADOWMASK_ATLAS_STAGE_VERSION`. Renderer `2W` filter, alignment guard, BC5 upload through the descriptor fn, 2×1 placeholder. Two-sample shader helper with per-group clamp; rewrite the shader guard. Move the 5×5 fixtures to 4-aligned dims; raw-byte tests decode the payload. Four-light fixture map with a mask edge at the seam, checked by capture on an adapter (M1 first pass). | integrating executor | — | done — `capture_shadowmask_groups` passes on this Mac's adapter and fails when the shader's group order is swapped; compiler, renderer, level-format and loader suites green |
| 2 | **Bake contract.** Wide-layer omit before the memo probe and fill (D4, D5). Misaligned-bake error in every profile (D7). Stale memo, logged hit and all-sentinel tests (W4–W6). Scratch-bound tracker (B3). Determinism and warm=cold (B4). All-255 decode (M5). Encode-error measurement (B5). Footprint half-raw (B1). | integrating executor | 1 | done — D4/D5 `eight_k_wide_layers_omit_the_shadowmask_on_every_build_and_four_k_emits`; D7 `fused_prepare_rejects_a_misaligned_atlas_naming_its_dimensions` (debug and `--release`); W4 `pre_bc5_memo_entries_are_never_served_and_the_rebuild_matches_uncached`; W5/B4 `second_fused_build_hits_the_memo_and_warm_equals_uncached_on_real_masks`; W6 bytes in `all_filtered_selection_keeps_empty_bytes_and_indeterminate_progress`; B1 `shadowmask_footprint_payload_is_half_the_raw_rgba_arithmetic`; B3 `cache_miss_holds_one_raw_fill_one_output_and_bounded_encode_scratch`; B4 workers `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers`; M5 `all_visible_payload_equals_encoding_an_all_visible_fill_and_decodes_fully_lit`; B5 measured by the ignored `shadowmask_bc5_encode_error_on_fixture_bakes` |
| 3 | **Overlap report.** Peak per-texel count from `build_analytic_overlap_graph_in_order`, carried as the memo-entry prefix. `--verbose` line on miss and hit; an omission line for wide layers (O1). | integrating executor | 2 | done — O1 end to end: ignored CLI `verbose_bakes_report_peak_texel_overlap_on_miss_and_hit_and_quiet_bakes_do_not` (passes: peak 4 cold and warm, no quiet line); unit tests `fused_overlap_report_is_the_graph_peak_on_miss_and_the_memo_value_on_hit`, `memo_entry_without_an_overlap_count_is_a_miss`, `overlap_report_logs_peak_with_layers_and_format_or_names_the_omission` |
| 4 | **Wire and renderer fan-out.** W1–W3 including the loader warning, D2, D3, D6, B2. GPU helper readback for D1 and M3. | integrating executor | 1 | done — W1–W3 `level-format` `shadowmask_atlas::tests`; W3/W6 loader `load_prl_rejects_pre_bc5_shadowmask_by_format_and_keeps_entity_shadow_selection`, `load_prl_keeps_an_all_sentinel_shadowmask_section`; D2/D3/D6/B2 renderer `lighting::lightmap` tests; D1/M3 and the M1/M4 union halves in `render::shadowmask_sample_test`, run on AMD Radeon Pro 5300M (failing when the per-group clamp is removed) |
| 5 | **Payload ownership.** Header/payload split for ids 22 and 42. `LevelWorld` keeps headers plus one movable payload value. `install_level_payload` and the capture install take it; `install_level_geometry` receives it by value; `LightmapResources::new` consumes and drops it. `LevelWorld` and `LevelGeometry` literals follow. Lifecycle tests L1–L5. | integrating executor (may delegate: loader, startup and capture files; no overlap with 2–4 once task 1 lands) | 1 | done — L1 `taking_gpu_lighting_payloads_leaves_headers_and_nothing_to_take_twice`; L2 `install_without_renderer_keeps_gpu_lighting_payloads_in_the_world`; L3 capture takes before install (the `capture_shadowmask_groups` masks render from the moved payloads; `install_level_geometry` requires owned payloads, so no path can borrow them); L4 `animated_lights_render_after_lightmap_payloads_move_into_the_upload` (adapter) plus the header-sized animated atlas test; L5 `splitting_partial_lighting_moves_only_the_present_payloads` plus `upload_pairs_usable_headers_with_their_moved_payloads_only` |
| 6 | **GPU mask proofs.** Capture tests for M1 (final), M2, M4 (id-42 rewrite A/B), L4 and L5 captures. All run on an adapter. | integrating executor | 4, 5 | done on AMD Radeon Pro 5300M — M1 `every_selected_light_reads_its_own_slot_in_either_group`; M2 and M4 `group_placement_never_changes_what_a_light_reads` (open group left or right renders identically; baked masks moved group 0↔1 render byte-identically); L4 `animated_lights_render_after_lightmap_payloads_move_into_the_upload`; driven-edge M3 and union halves in `render::shadowmask_sample_test` |
| 7 | **Land.** `/preflight`, then a `/review-panel` → `/fix-review-findings` loop. Update `build_pipeline.md` (id-42 row, §PRL ShadowmaskAtlas, §Build Cache) and `rendering_pipeline.md` §4 from decided to built. Result column. Hand manual rows MN1–MN7 to the owner with the exact commands. | integrating executor | 1–6 | done — preflight green (fmt, clippy -D warnings, full `cargo test`, `--release` check, crate graph); review panel round 1 (10 lenses, no 🔴 code defects, all 🟡/🟢 fixed) and round 2 on the fixes (3 lenses, nits only, fixed); context docs updated; manual rows handed to owner |
