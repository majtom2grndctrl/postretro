# spatial-residency--lightmap-bounded-pool — plan of record

mode: resumable
status: proposed
read at: 3d66934ce

No crate source changed between the brief's `read at` (6a813bdcd) and this commit. Every symbol cited by Decisions and Path was re-read at this commit. No Decision premise is false, and no Acceptance row needs restatement.

## Corrections

Each keeps the meaning of its Decision or Acceptance row. Any the owner reads differently goes back to the owner before task 1.

- **Manual route and shot tooling** — the brief treats the 2 km route as available; `spike/memory-measure-tour` and `measure/sh-bounded-pool*` were deleted, and `done/.../research.md` keeps only "2 km, 7 m/s". The commits survive unreferenced in the local object store. Pinned as local refs `measure/recovered-spike-tour` (95ac6b954: `measure_tour.rs`, `spike_shot.rs`, `imgdiff.rs`) and `measure/recovered-sh-harness` (6608e220a: `tools/measure-harness/`). Task 10 ports them to a throwaway branch that never merges. Neither ref is pushed, and both go at landing.
- **"Id 51 exists" and "usable id 51"** — the Wire format and the Acceptance row use different words for one thing. `lightmap_pool_gate.rs` records that id 51 is emitted exactly when the compiler's loader-backed portal test passes. The compiler writes id 58 on id 22 blocks plus an emitted id 51. The loader requires it on that same test, whatever `POSTRETRO_LIGHTMAP_STREAMING` says.
- **Id 57 is unregistered on this base** — `section_registry.rs` pins `from_u32(57) == None`, because `feat/mover-light-flow--probe-channel` owns it. Id 58 leaves a gap. That test keeps its pin on 57 and adds one on 59.
- **`place_all_resident` runs in block-id order** — the static plan, `all_resident_layers`, the compile gate and `targeted_demand_layers` (after its own pre-sort) all call it. It becomes tallest first, placements mapped back to block id, and the pre-sort folds in. All-resident placements move as a result. Sampling is placement-independent by §4's contract, so this is R24 as written. The pre-bake gate's count can change by a layer either way.
- **Band headroom is one `u64`** — `LightmapPoolReport.band_headroom_texels`, `BlockFacts.texels`, and `refused_headroom` in `drain.rs` become per-pool figures. R17 and R29 cannot hold on one number.
- **A level without id 42 cannot reach the white fallback through plane addresses** — absent now reads zero, and the fallback lives in the same sampler. The table entry needs a third state: no mask pool at all, reading all-visible. This preserves §4's no-id-42 rule and R27's distinction. `textureNumLayers` clamping and the half-width UV retire. `shadowmask_width_fits` retires too, since the mask texture is 2048² per layer.
- **A bound of 0** — "stored bound plus margin" gives zero layers for a pool whose level holds data no camera cell mandates. A pool with any all-resident layers takes at least one, so band pairs can land. Row 42's equality holds for every bound above 0.
- **R27's pool** — a level with id 42 and no present plane has no mask pool. It binds the existing placeholder texture, reports mask-present, and counts 0 bytes in the Build Summary and first generation, as a no-id-42 level counts its placeholder today. The meter shows it as a placeholder row.
- **Naming the camera cell in the growth warning** — `LightmapPoolModel` has no cell. The outcome reports which pool grew, and the controller, which holds the camera cell, logs the warning.
- **Tracked PRL** — `content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl` is committed. It rebakes in task 3 if it carries id 42 or needs id 58.
- **Grep rows on context docs** — row 47 must pass before the results report, so its doc edits land in task 9, ahead of the land step.

## Delegated answers

- **Block table layout** — one `vec4<u32>` per block, no fragment binding. `block_alignment()` is at least 4, so an offset stores as texels / 4.

  ```text
  x  lightmap address   offset x/4 (9) | offset y/4 (9) << 9 | layer (14) << 18
  y  plane A address    same
  z  plane B address    same
  w  extent w (12) | extent h (12) << 12 | flags (8) << 24
  ```

  Flags add plane A present, plane B present and no-mask-pool to RESIDENT and NONE. 14 layer bits exceed any device's array limit. The vertex stage decodes x and w into today's `lightmap_rect` and `lightmap_layer_flags`, and passes y and z raw through one new flat `vec2<u32>` at location 10, which is 11 of the 16 inter-stage locations. The fragment decodes them only on the mask path.

## AC-to-proof

Test names are planned. "Req GPU" means the test runs under `POSTRETRO_REQUIRE_GPU=1`, per row 58.

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Nonzero plane stored unchanged, zero plane has no blob; A only, B only, both, neither | `zero_planes_have_no_blob_in_every_presence_combination` | achievable as stated |
| 2 | Old tag refused by name; id 58 missing or unknown version refused by name | `rejects_retired_smb7_with_a_rebake_hint`, `level_needing_the_bound_refuses_absent_or_unknown_id_58` | achievable as stated |
| 3 | Bad length, offset on an absent plane, non-adjacent planes refused by name | `rejects_malformed_plane_records_by_name` | achievable as stated |
| 4 | Hand-computed bound: pools differ, entry at lead counts, past it does not, pinned once | `bound_matches_hand_computed_fixture` | achievable as stated |
| 5 | Added cell raises the bound, id 58 layout unchanged | `added_cell_raises_the_bound_through_the_cell_input` | achievable as stated |
| 6 | Cell input reads no water-plane data | grep gate `bound_cell_input_reads_no_water_plane_data` | achievable as stated |
| 7 | Id 58 byte-identical warm and cold | `id_58_is_byte_identical_warm_and_cold` | achievable as stated |
| 8 | Compiler and runtime agree on layers, both pools | `compiler_and_runtime_count_the_same_layers` (dev-dep edge) | achievable as stated |
| 9 | No usable id 51: no section, all-resident | `level_without_id_51_has_no_bound_and_runs_all_resident` | achievable as stated |
| 10 | Build Summary bytes and worst cell, tie to lowest id; no-bound form; warning both sides | `build_summary_reports_bound_and_warns_past_threshold` | achievable as stated |
| 11 | Mask-pool floor: fails cold and warm, passes under, warns when streaming | `mask_pool_floor_gate_*` | achievable as stated |
| 12 | All-resident mask over device layers drops the mask pool alone, by name | `all_resident_mask_past_device_layers_drops_alone` | achievable as stated |
| 13 | Absent plane samples as stored zero: union, specular, unowned, owned, channels 0 and 1 | `absent_plane_samples_as_stored_zero` in `shadowmask_sample_test`, `shadowmask_union_test`, req GPU | achievable as stated |
| 14 | Three addresses each sampled from their own | `block_samples_three_addresses` req GPU | achievable as stated |
| 15 | No-lightmap, miss, and no-id-42 reads hold | `shadowmask_block_states_read_as_before` req GPU | achievable as stated |
| 16 | All-resident and streamed place planes alike and sample alike | `all_resident_and_streamed_sample_identically` req GPU | achievable as stated |
| 17 | Id 42 with no plane loads both modes, reads zero (R27) | `id_42_with_no_plane_reads_zero_in_both_modes` req GPU | achievable, see Corrections (R27's pool) |
| 18 | Pair short in one pool installs nothing, retries whole; band refused whole (R1) | `pair_short_in_either_pool_installs_nothing` | achievable as stated |
| 19 | Half needs growth while other retires: defers whole (R13) | `pair_defers_whole_when_other_pool_retires` | achievable as stated |
| 20 | Evicting a band block frees both halves (R2) | `eviction_frees_both_halves` | achievable as stated |
| 21 | Victim returns whole or stays out whole (R16) | `band_victim_returns_only_whole` | achievable as stated |
| 22 | Band pair read again only when the refusing pool has room (R17) | `refused_band_pair_waits_on_the_refusing_pool` | achievable as stated |
| 23 | No planes: no mask placement or read; one plane: one placement (R3) | `mask_placements_follow_plane_presence` | achievable as stated |
| 24 | Payload disagreeing with presence fails whole | `payload_disagreeing_with_presence_fails_whole` | achievable as stated |
| 25 | A-absent reads exactly B's bytes, and the reverse | `streamed_block_reads_exactly_its_present_plane` | achievable as stated |
| 26 | Abort restores both pools and table; failed install frees both halves (R4) | `abort_restores_both_pools`, `failed_install_frees_both_halves` | achievable as stated |
| 27 | Level change and first drain leave nothing resident in either pool (R5) | `new_generation_empties_both_pools` | achievable as stated |
| 28 | Any history: set that packs from scratch places with no growth, copies complete through one spare | proptest `compaction_places_any_set_that_packs_from_scratch`, reports compaction and cycle counts | achievable if task 1 finds no stuck case, see Risks |
| 29 | Downward pass enough: nothing moves up; drain-only fit gives way (R10, R18) | `downward_repack_must_cover_the_whole_demand` | achievable as stated |
| 30 | Demand over one drain's budget, shortest first: one repack, no growth (R11, R20) | `demand_arriving_over_drains_repacks_once` | achievable as stated |
| 31 | Demand past the pool compacts at most once before growing (R32) | `demand_past_the_pool_compacts_once_then_grows` | achievable as stated |
| 32 | Reserved room: stays a miss, no band lands there (R28) | `reserved_room_stays_a_miss_and_holds_off_band` | achievable as stated |
| 33 | Reservation returns on leaving demand, by removal or to band (R21) | `reservation_returns_when_its_target_leaves` | achievable as stated |
| 34 | Headroom leaves out reservations, counts a pair in hand once (R29) | `band_headroom_tracks_reservations` | achievable as stated |
| 35 | No copy order: layout stands, growth, warned by name (R22) | `no_copy_order_rolls_back_to_growth`, on a hand-built stuck layout | achievable as stated |
| 36 | Up, down, same-layer, swap land intact; planes told apart (R6) | `every_move_shape_lands_intact` on `gpu_mirror` | achievable as stated |
| 37 | One pool's repack leaves the other, band drop leaves both; one submission (R7, R15) | `one_pool_repack_leaves_the_other`, queue-counted, req GPU | achievable as stated |
| 38 | Mask-only repack rewrites only moved entries (R14) | `mask_repack_rewrites_moved_entries_only` | achievable as stated |
| 39 | One repacks, one grows: one submission, only the grown retires (R23) | `repack_beside_growth_lands_in_one_submission` req GPU | achievable as stated |
| 40 | Both repack: one submission, one table write per block (R31) | `both_pools_repack_with_one_table_write` req GPU | achievable as stated |
| 41 | Repack while retiring touches the live generation only (R8) | `repack_while_retiring_touches_live_generation` | achievable as stated |
| 42 | First generation is bound plus margin, capped | `first_generation_is_bound_plus_margin_capped` | achievable, see Corrections (bound of 0) |
| 43 | Walk within the bound never grows at no margin, on a fixture that grows today | `walk_within_bound_never_grows` | achievable as stated |
| 44 | At the all-resident cap, holds every block; R24's set; wholly mandatory from one cell | `all_resident_cap_holds_every_block` | achievable as stated |
| 45 | Band places in any layer, past the old default, after growth, under a replaced controller (R25) | `band_uses_every_layer_under_any_controller` | achievable as stated |
| 46 | No non-test source sizes from the old default, carries a drain cap, or offers a lever | grep gate `no_lightmap_pool_cap_survives` | achievable as stated |
| 47 | Docs describe no cap, slider or default cap | grep gate over both docs, task 9 | achievable, see Corrections |
| 48 | Capture override per pool, report names no cap, full pool matches all-resident | `capture_lightmap_streaming`, req GPU | achievable as stated |
| 49 | Grows once and warns, only when a ready pair cannot place; band stays; device defers (R19) | `demand_past_pool_grows_once_and_warns`, log capture | achievable as stated |
| 50 | Growth reserves targeted pack times margin, capped; no separate headroom | `growth_reserves_demand_times_the_shared_margin` | achievable as stated |
| 51 | After either pool grows alone, next draw samples the grown layer (R30) | `next_draw_samples_a_grown_layer` req GPU | achievable as stated |
| 52 | Second demand grows and warns again (R26) | `second_demand_grows_and_warns_again` | achievable as stated |
| 53 | Settling past the pool grows once, reaches reveal, keeps size (R12) | `settling_past_the_pool_grows_once_and_keeps_size` | achievable as stated |
| 54 | Pools grow and retire apart; meter reports both with spares and retiring (R9) | `pools_retire_independently_and_meter_counts_both` req GPU | achievable as stated |
| 55 | Summary bytes equal first-generation bytes, below the cap, at it, and with no id 51 | `summary_bytes_equal_first_generation_bytes` | achievable, see Corrections (R27's pool) |
| 56 | Walk harness first layers and bytes equal the runtime's, no size passed | `walk_harness_first_generation_matches_runtime` | achievable as stated |
| 57 | Policy identity differs from the old string | grep gate | achievable as stated |
| 58 | GPU rows run with an adapter required | preflight run under `POSTRETRO_REQUIRE_GPU=1` | achievable as stated |
| 59 | Hallway ceiling | executor records on the 5300M, task 10; finding, not a gate | manual |
| 60 | Walks at 5/4 and one step either side | executor, no-GPU harness, task 10 | manual |
| 61 | Frames on five maps | executor captures and diffs at equal clock; owner reviews | manual |
| 62 | Repack cost | executor records, task 10; a visible hitch goes to the owner | manual |
| 63 | Mask-heavy map | executor records; owner reviews frames | manual |
| 64 | Theatrical moments | owner, in-engine | manual |
| 65 | Planar mirrors | executor, no-GPU harness, task 10; any growth goes to the owner | manual |
| 66 | Bound cost | executor, warm and cold, task 10 | manual |
| 67 | Cleanup | executor records driver memory; owner confirms | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Riskiest slice.** Compaction alone on today's single pool: dependency-graph copy order through the spare, planned before any copy records, rollback when no order exists. Property test on near-full mixed-size pools, with a deliberate hunt for stuck layouts. Walk harness at 28 and 30 layers. Metal timing of a whole-pool compaction. Rows 28, 35, 36 on one pool. Stops per Risks | integrating executor | — | |
| 2 | Whole-demand repack and reservations: room held for every target, downward pass judged on the whole demand, one compaction before growth. Rows 29–34 on one pool | integrating executor | 1 | |
| 3 | Format: split `shadowmask_atlas.rs` tests first. `SMB8`, plane absence, id 58 section, tallest-first shared placement, shared margin and pool pricing in level-format, loader refusals, memo version bump, tracked fixture rebake. Rows 1–3 | worker, level-format + level-loader + compiler bake | — | |
| 4 | Compiler: bound through one cell input, Build Summary, post-bake mask gate. Rows 4–11, 55 compiler side | worker, compiler | 3 | |
| 5 | Two-pool model: one placement type per pool under one journal, atomic pairs, per-pool repack, growth, headroom and retirement, block table layout, payload presence, block facts and split reads. Rows 18–27, 37–41 model side | integrating executor | 2, 3 | |
| 6 | Renderer: split `stream/tests.rs` first. Separate pool generations, one-submission execution, all-resident planes, mask placeholder, meter, shader and varying. Rows 12–17, 37, 39, 40, 51, 54 | worker, renderer | 5 | |
| 7 | Sizing: pools from id 58, cap retired from batch, levers, slider and capture, shared margin, growth warning, policy identity. Rows 42–46, 48–50, 52, 53, 57 | worker, app + renderer seam | 4, 5, 6 | |
| 8 | Harnesses: `pool_mirror`, the compiler dry run, walk sizing with no size passed. Rows 43, 56 | worker, measurement | 7 | |
| 9 | Docs: `rendering_pipeline.md` §4 and §7.8, `build_pipeline.md`, `large-map-spatial-residency.md`, with the grep gates. Row 47 | integrating executor | 7 | |
| 10 | Cold rebake at this commit, old-compiler baselines baked fresh, harness ported to a throwaway branch, rows 59–67 | integrating executor; owner for 61, 63 review, 64, 67 | 8, 9 | |
| 11 | `/preflight`, `/review-panel`, `/fix-review-findings` loop, results table | integrating executor | 10 | |

Workers run sequentially on the feature branch's warm target, with no worktrees. Task 3 and task 1 touch disjoint files, but concurrent Cargo on one target is not worth the disk or the owner's machine. Each worker gets the full brief, `research.md`, routed docs, the full Acceptance list, named files, upstream contracts and downstream consumers. Workers self-review once, do not edit `plan.md`, and do not commit. Cargo stays capped at 6 jobs under `nice` while the owner is working.

## Risks

- **One spare may not always be enough.** A fully tangled move graph across three layers can need two blockers staged at once, and two large blockers may not share the spare. I have no real layout showing this, and no proof it cannot happen. Task 1 hunts for it directly.
  - If the generator reaches a stuck layout, the Decision that any packable set fits at runtime is false as written. The plan goes `blocked` with the layout, because row 28 and "the bound is provable" both rest on it. R22's fallback would keep the engine correct, but only by growing.
- **Compaction cost.** Up to about 200 MiB of copies in one submission on the hallway. If the task 1 timing is a visible hitch, the plan goes `blocked` before any format work, with budgeted incremental moves as the proposed next shape.
- **Row 43's fixture must grow today at that size.** Task 1's walk may show the first such fixture is the hallway itself, which cannot run in routine tests, so a synthetic one is needed.
- **Disk.** 71 GB free. Task 10 needs about 25 GB for `xctrace`, plus baseline PRLs and a comparison binary. Delete raw traces after each run.
- **Mac confounders in task 10.** Temperature-gate runs, keep the display awake across gaps, pilot one run per config, and compare frames at equal clock.

## Hot paths

- **`plan_drain` runs per frame.** No new steady-state allocation: reservation and compaction buffers join `Scratch`, and `scratch_capacities` keeps proving it. Reservation work runs on target change, not per frame.
- **Compaction is rare and bounded.** One per demand at most, planned on the CPU before any copy records, and counted by kind for row 62.
- **Forward pass.** One more flat varying and no new binding. The mask path adds decoding and skips sampling for absent planes. Forward pass timing on the 5300M is checked in task 10.
- **Read path.** Fewer bytes by construction. `payload_from_pair_bytes` stays on the frame thread, per the brief's non-goal.
- **Compile.** The bound loop runs in Pack on every build. Row 66 measures it.
