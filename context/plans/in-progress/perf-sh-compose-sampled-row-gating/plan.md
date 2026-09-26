# perf-sh-compose-sampled-row-gating — plan of record

mode: resumable
status: test-ready
read at: b3548b603

## Corrections

- `sh_streaming/frame.rs` was only 297 lines at the current read, not near the brief's
  conditional ~800-line split threshold; keep frame orchestration there and place the new pure
  gather, gate, and staleness planners in responsibility-specific sibling modules.
- The gather precedent's current symbol is `AnimatedLightmapResources::dispatch`, not
  `AnimatedLightmapCompose::dispatch`; its balanced 2D/padded `master_tiles` list remains the
  applicable precedent.
- `VisibleRenderPreparation::for_level` already materializes fog-reachable world AABBs as
  `reachable_cell_aabbs`; add visible-cell AABBs and drawn receiver regions without duplicating
  that work. The empty fog-reach DrawAll sentinel remains unchanged.
- Mesh visibility is now implemented in `postretro_render_cpu::mesh_pass::mesh_visible`, while
  the app collector remains in `postretro/src/scripting/frame_systems/mesh_render.rs`. Derive
  gate bounds from forward-visible emitted instances at their interpolated transforms, including
  the separately planned viewmodels. For movers, derive the interpolated AABB only for a
  beauty-visible draw; the existing `occluder_aabbs` includes shadow-only movers and uses the
  wrong population for this gate.
- Pass B consumes both animated descriptor activity and the uploaded `promoted_animated_states`;
  its trigger must notice changes to those effective scale values. Pass A compares the
  post-cache-zeroed `promoted_static_weights` that are actually uploaded. This is the current
  source expression of the Decision that each pass tracks only its own inputs.
- Binding 18 currently carries an 80-byte dynamic range record, and every shared compose shader
  still indexes `range_start + workgroup.x`. Replace it with a bounded gather form for streamed
  and legacy whole-load callers; all callers of the shared shaders will use the same enlarged
  binding-18 layout, but no binding number or storage-buffer count changes.

## Delegated answers

- Footprint dilation — expand each input AABB independently by `1.1 * cell_size` on every axis
  before clamping and resolving its 8-corner probe footprint to affinity rows. One spacing covers
  the adjacent trilinear corner and the extra 0.1 covers the largest component of the shader's
  `SH_NORMAL_OFFSET_M = 0.1` bias. Resolve per region into caller-owned scratch, then deduplicate;
  this is conservative for billboard and fog consumers that do not apply the normal offset.
- Gather carrier and encoding — keep binding 18 and its dynamic offset. Extend its uniform record
  with packed `vec4<u32>` flattened affinity-row IDs and store the row count in the existing
  tail. At the renderer-requested 64 KiB uniform-binding floor, an
  80-byte header leaves 4,091 vectors, or 16,364 row IDs per chunk. Effective chunk
  capacity is `min(16_364, max_compute_workgroups_per_dimension)`; each aligned record in one
  upload owns its row IDs, so every dispatch reads only its chunk. Legacy whole-load callers use
  generated `0..affinity_count` row IDs and cover every affinity row exactly once through the
  shared shaders.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| G1 fragmented rows, exact-capacity/overflow chunking, exact decode, one visit, device dispatch bound | `gather_plan_chunks_fragmented_rows_without_duplication` and `gather_plan_splits_only_above_capacity` | proved |
| G2 empty pass emits no dispatch and zero counters | `empty_gather_plan_has_no_dispatch` | proved |
| G3 list contains only distinct resident rows | `planned_rows_are_unique_resident_rows` property test | proved |
| G4 compose storage-buffer budgets and uniform chunk size stay within requested limits | GPU-free layout-builder tests `streamed_indirect_compose_layout_fits_requested_compute_limits` and `direct_compose_keeps_existing_binding_numbers_and_dynamic_grid` | proved |
| G5 legacy whole-load upload covers every affinity row exactly once | `legacy_gather_compose_covers_every_affinity_row_once` | proved |
| R1 trigger selects only gated contributing rows | `trigger_selects_only_gated_contributing_rows_per_pass` | proved |
| R2 idle pass selects only residency-change rows | `idle_plan_contains_only_pending_residency_rows` | proved |
| R3 promotion change compares effective uploaded weights after cache zeroing | `effective_uploaded_promotion_weights_drive_static_trigger` | proved |
| R4 animated-only activity leaves static idle; promotion rewrite orders static then animated | `direct_plan_splits_activity_and_follows_static_rewrite` | proved |
| R5 deactivation tail survives a skipped compose frame | `deactivation_tail_waits_for_next_recorded_compose` | proved |
| R6 mask or dev-override change forces all resident rows, static before animated | `mask_or_override_change_forces_all_passes_in_order` | proved |
| R7 force-full-resident bypasses gate and triggers every pass every frame | `force_full_resident_plans_all_resident_rows` | proved |
| S1 mixed frame sequences leave no gated resident row lagging after recorded compose | state-machine property test `recorded_compose_clears_all_gated_lag` | proved |
| S2 lagging re-entry composes; current re-entry stays idle | `only_lagging_rows_compose_on_idle_reentry` | proved |
| S3 active pass with empty useful gate dispatches nothing and later entry composes | `empty_contributing_gate_advances_generation_without_dispatch` | proved |
| S4 deactivation while exiting gate composes off state on re-entry | `deactivation_outside_gate_is_repaired_on_reentry` | proved |
| S5 off-gate promotion change repairs both direct passes in order | `promotion_change_outside_gate_repairs_both_direct_passes` | proved |
| S6 camera cut repairs newly gated rows in the same frame | `camera_cut_rows_compose_before_sampling` | proved |
| S7 full eviction drops row; partial eviction composes; new generation starts current | `eviction_and_generation_reset_preserve_lag_contract` | proved |
| S8 install or reuse outside gate composes before promotion and does not recompose on clean entry | `install_and_slot_reuse_bypass_gate_before_promotion` | proved |
| S9 install/evict adds foreign scaled-node writer row | `residency_change_closes_over_foreign_writer_row` | proved |
| S10 post-drain resolution excludes evicted rows and includes promoted writer closure | `gate_resolution_observes_post_drain_residency` | proved |
| S11 encode failure retains lag and residency work for retry | `failed_encode_commits_no_compose_state` | proved |
| S12 1.1-spacing dilation includes footprint edge and excludes beyond edge | `region_dilation_matches_sampler_footprint` | proved |
| S13 scaled-node sample adds at most one origin writer row | `scaled_node_gate_adds_single_writer_row` property test | proved |
| S14 skinned body, interpolated mover sweep, and viewmodel contribute world bounds | app-side collector tests `drawn_sh_consumers_emit_complete_world_bounds` | proved |
| S15 empty fog reach gates all resident rows; non-empty stays scoped plus other consumers | `fog_reachability_sentinel_controls_gate_scope` | proved |
| S16 frame input exposes world bounds and no row indices | review plus compile-time construction tests around `ShSampleRegion` | proved |
| C1 counters equal planned row/dispatch/lag sets and capture report includes planning time | planner counter assertions plus capture report serialization test | proved |
| C2 measurement mode advances fixed animation time, stays VM-free, and retains force-active pulses | capture driver tests `measurement_frames_advance_animation_without_vm` | proved |
| C3 exactness fixture changes across stepped frames and records nonzero compose rows with authored curve visible | capture integration test, GPU/fixture gated | compiled; execution covered by M1 |
| C4 warm planning and gathered-upload encoding retain caller-owned capacity | capacity-watermark tests `compose_planning_reuses_warmed_scratch`, `compose_planner_reuses_warmed_pass_vectors`, and `gathered_upload_encoding_retains_warmed_capacity` cover region resolution, per-pass plan ownership, and binding-18 record/dispatch scratch | proved |
| M1 shipped vs force-full-resident capture PNGs are byte-identical at pinned poses and stepped times | owner, GPU runbook in `research.md` plus generated exactness scenes | manual-blocking |
| M2 1 m/3 m capture and live measurement against spike baseline with compose counters | owner, hardware measurement protocol in `research.md` | manual-blocking |
| M3 live pulses, turning, off-state, dynamic receivers, fog, mask and boundary crossings have no visual stale artifact | owner, in-engine visual runbook | manual-blocking |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Prove and implement the binding-18 gather carrier, bounded chunk planner, shared-shader row-list indexing, legacy coverage, and pipeline budget guards (G1–G5). This is the thinnest test of the highest-risk no-new-binding premise. | integrating executor | — | complete — `cargo check -p postretro-renderer`; 7 `sh_compose_dispatch` tests; 10 `compose_shader` tests; legacy/streamed layout guards; render-cpu gather record test |
| 2 | Add per-pass compose counters, planning-time accounting, caller-owned scratch, and the dev force-full-resident switch over today's resident sets; expose them through renderer snapshots, periodic diagnostics, Streaming UI, and capture reporting (G2, R7, C1, C4). | integrating executor | 1 | complete — force-full-resident planner test; warmed-scratch test; counter assertion; snapshot/UI/log/capture-report serialization tests |
| 3 | Build the GPU-free per-pass trigger and staleness state machine, including effective promotion-weight snapshots, tails across skipped frames, residency-change pending work, direct-pass ordering, commit-on-success, and generation reset (R1–R6, S1–S8, S11). | integrating executor | 1 | complete — 19 focused planner/state tests, including property sequences and failed Pass-B retry |
| 4 | Add the app-owned `ShSampleRegion` collection for visible/fog cells, drawn movers, accepted skinned meshes, and viewmodels; add renderer-owned 1.1-spacing region-to-row resolution and scaled-node writer closure against post-drain residency (S9–S10, S12–S16). | integrating executor | 3 | complete — 6 resolver tests plus mesh/viewmodel and interpolated-mover collector tests; renderer/app checks |
| 5 | Integrate gather, split triggers, gate, staleness, residency/promotion ordering, and retry semantics into the streamed indirect/static-direct/animated-direct compose passes; run focused state-sequence and renderer crate tests. | integrating executor | 2, 3, 4 | complete — normal/dev-tools/capture checks; renderer lib 654 passed, 1 ignored; focused app collectors green |
| 6 | Add capture measurement animation stepping, exactness fixtures/assertions, and complete measurement counters while preserving default single-instant capture and VM-free execution (C1–C3). | integrating executor | 5 | complete — 1/60 s VM-free stepping test; 16 scene parser tests; paired 0.5 s/1.0 s gated/oracle scenes; default/preload remain time zero; manual GPU pixel/counter assertion remains landing-blocking |
| 7 | Update `context/lib/rendering_pipeline.md` from planned to built, run review-readiness checks, `/review-panel` → `/fix-review-findings` loops with focused retests, then `/preflight` once. Record all automated results and prepare the three owner GPU/manual runbooks; because manual proof blocks landing, finish at `status: test-ready`. | integrating executor | 6 | complete — review panel clean after repair loops; `cargo fmt --check`, workspace clippy with `-D warnings`, and full workspace `cargo test` pass; M1–M3 runbooks recorded and remain landing-blocking |

## Landing policy

The brief does not authorize landing before its manual exactness, performance, and visual rows
pass. After automated review and preflight, set `status: test-ready` and wait for the owner's
blocking results. On pass, record every result, update durable context, move the brief to `done/`,
and commit the landing state. On a miss, record its counters and return to the owning task.
