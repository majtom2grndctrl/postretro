# spatial-residency--lightmap-bounded-pool — plan of record

mode: resumable
status: approved
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

- **Row 28 and the compaction Decision, owner ruling 2026-11-06** — the brief claimed any set that packs from scratch fits at runtime. With no layer spare, the planner misses about 0.08% of real-scale stress compactions at 90–95% fill (25 of 31,382), and a hand-checked miss has an order, so these are search misses. The owner restated both: the guarantee covers sets with a layer to spare, by measurement rather than proof, and a no-spare miss takes logged growth. The larger sample in task 1's row backs the line.
- **Compaction shape** — the Path's dependency graph with blockers staged in the spare became target layers built one at a time into a rolling empty layer, because the graph search missed the hallway at 27 layers. The spare's index stays fixed. What is proved: every returned plan is valid, a failed plan changes nothing, building into an empty layer cannot fail, a park fails only by fragmentation, and a single-layer pool always succeeds.
- **Whole-map levels at no spare** — a level capped at all-resident with every block mandatory has no layer spare, so it can rarely grow past its cap and warn. The owner kept the cap; such levels are fixtures.

- **Demand, for the compaction gate** — the targeted set as it stands. A compaction that does not seat all of it, or finds no layout, spends the demand until targets change into a set that packs from scratch or the pool grows. A compaction that seats everything spends nothing, which row 28 needs. Attempts and the warning are once per demand.
- **Reservations** — made at target change, tallest first, from free space only, never by eviction. Past the pool, a later pair may take an unready target's room through the pairs-only repack, because row 49 wants growth last. Row 30 is "at most one repack": a demand that fits as the pool stands needs none.

- **Pool cap against the bound** — shelf packing is not monotone, so a cell's set could pack worse than the whole level. `first_generation_layers` caps at the larger of all-resident and the bound, per R24's "the pool never sits below the bound". No case is known.
- **Absent planes until the split** — `planes_with_absent_as_zero` in `level-loader/src/prl_lightmap.rs` expands absent planes to zeros for today's single pool, so a drain can upload more than its budget says. Task 5 removes it and teaches `static_plan`, `stream.rs` and `pool_mirror` presence.
- **On-disk PRLs** — 26 of 30 in `content/` carry id 42 and are refused until rebaked, the hallway among them. No routine test loads one. Task 8's dry run still charges every block both planes and needs presence.

- **Worst cell's bytes** — the cell's own layers priced across both pools, with no margin, cap or spare: those add nothing at cell level, and a cap would tie unequal cells. Lowest id wins a tie.
- **Row 11's floor** — it cannot be reached in a real build, so failing is proved at the gate, and row 7 proves the pipeline reaches the gate cold and warm.
- **Compiler details** — the mask pool places by the block's `block_alignment()`. `CellBlocks` refuses non-contiguous block runs, as `LevelBlockMap` does. A level with no camera cell stores 0 and 0 and names no cell. Mask gate failures read "Shadowmask runtime all-resident pool". `sh-streaming-hinted-door.prl` has 0 blocks and needs no id 58.

- **Row 19, read with R9** — a pair defers on retirement only when a pool that must grow is retiring. R13's ordering has the mask half deferred because it cannot grow, and R9 says a later growth waits only on its own pool. So a lightmap half that needs growth, beside a retiring mask pool whose half has room, grows and lands. The owner sees this at results.
- **Two-pool model** — `PerPool<PoolState>` under one journal. A lightmap item is the block id, a mask item is `block*2+plane`, and an absent plane has no item. A repack seats pending items as reserved room and pairs land after it. `LightmapRefusal.short` names the refusing pool. Band reads skip plane pairs when the mask pool is full but continue for planeless pairs. `PoolDemand`, `PerPool` and `PoolKind` live in `level-format`.
- **Renderer pools** — `PoolTextures` is one enum per pool under `PoolGeneration` (live plus retiring), stored as `PerPool<Option<PoolGeneration>>`; `None` means no id 42 or no present plane. Counters, live diagnostics and the Streaming tab are per pool. `shadowmask_width_fits` is gone, so a streamed level has no mask refusal path and defers past the device instead. Capture JSON adds `mask_pool_layers`, `mask_pool_bytes`, `mask_peak_pool_layers`, `mask_repacks` and `mask_growths`.
- **Mask placeholder** — 1×1, because flags answer every read and it is never sampled. An R27 level shows a 4-byte placeholder row, with 0 pool bytes.
- **Bindings** — no bind group layout changed. Forward fragment samples 16 textures with cube arrays and 15 without, budget 16. Inter-stage is 11 of 16 locations.
- **Row 41** stays model-side: `execute` targets the new or live generation only, and the model test needs sizing private to `render-cpu`.
- **Sizing names** — `StreamedPoolSizing { bound, first_generation_override }`, `PoolDemand::first_generation`, `LightmapPoolBoundSection::layers()`. `first_generation_layers` replaces `pool_cap_layers` on the plan. `EvictionReason::OverCap` and `levers.rs` are gone. Policy identity: `"bound-sized split pools, shared margin 5/4"`.
- **First-generation figure** — literal in the model, bounded only by the device, floored at 1, and 0 when the pool holds nothing. `first_generation_layers` does the capping, so a harness override may exceed all-resident.
- **Capture scene fields** — `lightmap_pool_layers` and `mask_pool_layers`, each optional, 0 refused. `lightmap_pool_cap_layers` now fails as an unknown field.
- **Growth warning text, now contract** — "The {pool} pool grew to {n} layers at camera cell {c}". An aborted drain restores the compaction gate, so its retry can warn again (rare).
- **Row 43's fixture** — grew 5→7 on the pre-brief model at 6a813bdcd with cap = bound = 5. A `#[cfg(test)]` `downward_only` switch in tree runs the same walk with 2 growths and the test asserts more than 0, so the fixture cannot go stale silently.
- **`forced_missing_lightmap_block_drops_exactly_its_static_direct_and_specular`** — fails before this branch too: a `--no-cache` bake with the 6a813bdcd compiler also reports 0 portals for `specular-shadowmask-capture.map` and skips id 51. Test left untouched and red. It needs a map change or another fixture; `switch-demo`, `trap-wave` and `shadowmask-groups-capture` also have 0 portals, and `closet-reveal` streams but has 0 mask layers. Goes to the owner at results.
- **`--all-features`** fails on `observability/driver.rs` (missing `assist_held`, `swim`, `vault`), code no commit on this branch touches.
- **`large-map-spatial-residency.md`** lives in `context/plans/`, not `context/lib/`; the gate reads it there. Its "Today" bullet still names `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES` and `DEFAULT_GPU_FLOOR_BYTES`, which no longer exist; left alone as out of scope.
- **Row 64 positions** (hallway, engine metres x, y up, z; spawn at (42.3, 2.4, 63.4)) — closet reveal: `warren_closet_trigger_0`, walk into it at (53.3, 2.4, -105.7), spawn level, 169 m away, touch-fired once, slides the door and spawns 2 enemies; closets 1 and 2 are on the top level at (10.2, 35.0, -63.4) and (-73.7, 35.0, 105.7). Door: `warren_door_3` at (10.6, 21.1, 79.7), about 40 m from spawn, one level up, press use inside its volume, rises 6.5 m and closes after 2.5 s. No dev teleport found; a capture cannot fire either. A copy of the map with `player_spawn` moved beside a trigger is the shortcut, untried.
- **Walk sizes** — `POSTRETRO_LIGHTMAP_WALK_POOL_LAYERS` replaces the caps variable: comma-separated, one run per entry. `34/15` both pools, `30` or `30/` lightmap only, `/15` mask only, `/` both from id 58, `bound` both at id 58 with no margin; 0 refused; a size may exceed all-resident. No margin variable: pass sizes directly. `POSTRETRO_LIGHTMAP_WALK_RUNS=all` is default, `bound`, leads 0 and 32. Run labels read `pools 34/15`, so old trip records do not pair.
- **Row 43 limit** — a real walk draws cells outside the bound's set, so a real walk at `bound` can legitimately grow; the harness does not assert zero growth on real walks. Row 43's claim is the model test plus `walk_harness_at_the_bound_with_no_margin_never_grows` through the real controller.
- **Dry run on the hallway** — `lightmap_residency_dry_run_from_prl` panics at `layouts.rs:165` with `LeafTooLarge` in the rival cluster-ordered layout, which assumes every cell fits one 8192 layer. The worker only renamed in that function, so it reads as pre-existing, unconfirmed by a run. It blocks only the full dry run report, not the walks. Goes to the owner at results.
- **Dry run limits** — rival whole-layer and tile layouts charge both planes per layer, and `block_pool_sim` models the lightmap pool only; each report says so. `band_pool_sim` runs both pools through the real model. Plane presence maps through each chart's stored block, because derived cell blocks do not line up with stored blocks by index.

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
| 28 | Any history: set that packs with a layer spare places with no growth, copies complete through one spare; no-spare misses reported | real-scale test in `compaction_scale_tests`, plus small proptest `compaction_places_any_set_that_packs_from_scratch` | restated by the owner, see Corrections |
| 29 | Downward pass enough: nothing moves up; drain-only fit gives way (R10, R18) | `downward_repack_must_cover_the_whole_demand` | achievable as stated |
| 30 | Demand over one drain's budget, shortest first: one repack, no growth (R11, R20) | `demand_arriving_over_drains_repacks_once` | achievable as stated |
| 31 | Demand past the pool compacts at most once before growing (R32) | `demand_past_the_pool_compacts_once_then_grows` | achievable as stated |
| 32 | Reserved room: stays a miss, no band lands there (R28) | `reserved_room_stays_a_miss_and_holds_off_band` | achievable as stated |
| 33 | Reservation returns on leaving demand, by removal or to band (R21) | `reservation_returns_when_its_target_leaves` | achievable as stated |
| 34 | Headroom leaves out reservations, counts a pair in hand once (R29) | `band_headroom_tracks_reservations` | achievable as stated |
| 35 | No copy order: layout stands, growth, warned by name (R22) | `no_copy_order_rolls_back_to_growth`, on a real search miss with a whole spare | achievable as stated |
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
| 64 | Theatrical moments | owner, in-engine, on the hallway; task 10 finds a closet reveal and a door from the `.map` and gives the owner positions | manual |
| 65 | Planar mirrors | executor, no-GPU harness, task 10; any growth goes to the owner | manual |
| 66 | Bound cost | executor, warm and cold, task 10 | manual |
| 67 | Cleanup | executor records driver memory; owner confirms | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Riskiest slice.** Compaction alone on today's single pool: dependency-graph copy order through the spare, planned before any copy records, rollback when no order exists. Property test on near-full mixed-size pools, with a deliberate hunt for stuck layouts. Walk harness at 28 and 30 layers. Metal timing of a whole-pool compaction. Rows 28, 35, 36 on one pool. Stops per Risks | integrating executor | — | done: 27, 28, 30 and 34 layers walk with 0 growths and 0 rollbacks; 0 misses in 262,818 real-scale compactions with a layer spare (131,650 at 90%+ pool fill, 95% upper bound about 0.0023%), against 0.10% and 0.18% at no spare on 27 and 34 layers. Rows 28, 35, 36 on one pool; Metal 5.85 ms median, 16 ms every block moved |
| 2 | Whole-demand repack and reservations: room held for every target, downward pass judged on the whole demand, one compaction before growth. Rows 29–34 on one pool | integrating executor | 1 | done: rows 29–34 pass on one pool, rows 28, 35, 36 still pass; 681 render-cpu tests. R32's warning by name waits on task 7 |
| 3 | Format: split `shadowmask_atlas.rs` tests first. `SMB8`, plane absence, id 58 section, tallest-first shared placement, shared margin and pool pricing in level-format, loader refusals, memo version bump, tracked fixture rebake. Rows 1–3 | worker, level-format + level-loader + compiler bake | — | done: rows 1, 3 and row 2's tag and version halves pass; row 2's absence half moves to task 4, when the compiler emits id 58. Fixture needed no rebake |
| 4 | Compiler: bound through one cell input, Build Summary, post-bake mask gate. Rows 4–11, 55 compiler side | worker, compiler | 3 | done: rows 2, 4–11 and 55's compiler side pass; `campaign-test` baked cold reports 128.0 MiB, bound 10 and 9, 13 and 10 layers. Bound about 145 ms on the hallway's cells |
| 5 | Two-pool model: one placement type per pool under one journal, atomic pairs, per-pool repack, growth, headroom and retirement, block table layout, payload presence, block facts and split reads. Rows 18–27, 37–41 model side | integrating executor | 2, 3 | done on the model: rows 18–27, 38, 41 and the model halves of 37, 39, 40 pass; rows 28–36 run per pool. 13 renderer GPU tests stay red until task 6 |
| 6 | Renderer: split `stream/tests.rs` first. Separate pool generations, one-submission execution, all-resident planes, mask placeholder, meter, shader and varying. Rows 12–17, 37, 39, 40, 51, 54 | worker, renderer | 5 | done: rows pass with a GPU required, 1,246 renderer tests. Streamed hallway capture is byte-identical to all-resident at 2 poses |
| 7 | Sizing: pools from id 58, cap retired from batch, levers, slider and capture, shared margin, growth warning, policy identity. Rows 42–46, 48–50, 52, 53, 57 | worker, app + renderer seam | 4, 5, 6 | done: rows 42–46, 48–50, 52, 53, 57 pass, GPU required for 48. Hallway first generation 34 and 15 layers, 274 MiB with spares, equal to the Build Summary; growths 0. `campaign-test` 3 streamed configs byte-identical to all-resident at 2 poses on a fresh copy |
| 8 | Harnesses: `pool_mirror`, the compiler dry run, walk sizing with no size passed. Rows 43, 56 | worker, measurement | 7 | done: rows 43, 56 pass, 56 with a GPU required against the renderer's own counters. Hallway walk with no size passed: 34 and 15 layers, 274.0 MiB, the runtime's figure. `bound` (27 and 12) took 0 growths on 300 steps of both walks |
| 9 | Docs: `rendering_pipeline.md` §4 and §7.8, `build_pipeline.md`, `large-map-spatial-residency.md`, with the grep gates. Row 47 | worker, docs | 7 | done: row 47 passes as `docs_describe_no_lightmap_pool_cap` in the cap gate, shown failing on reintroduced wording in both docs. Also touched `resource_management.md` and `rendering_pipeline.md` §10 |
| 10 | Cold rebake at this commit, old-compiler baselines baked fresh, harness ported to a throwaway branch, rows 59–67 | integrating executor; owner for 61, 63 review, 64, 67 | 8, 9 | |
| 11 | `/preflight`, `/review-panel`, `/fix-review-findings` loop, results table | integrating executor | 10 | |

The owner directed the session to orchestrate: tasks marked integrating executor may go to a dispatched worker, and the executor still owns shared contracts, integration review, `plan.md` and commits. Workers run sequentially on the feature branch's warm target, with no worktrees. Task 3 and task 1 touch disjoint files, but concurrent Cargo on one target is not worth the disk or the owner's machine. Each worker gets the full brief, `research.md`, routed docs, the full Acceptance list, named files, upstream contracts and downstream consumers. Workers self-review once, do not edit `plan.md`, and do not commit. Cargo stays capped at 6 jobs under `nice` while the owner is working.

## Risks

- **Task 1 walks** used a throwaway read shim with SH streaming off, because the hallway PRL fails `ClusterDirectory VersionMismatch` at this commit. With compaction off, that setup reproduced `research.md`. Task 10 rebakes.
- **The downward repack is not cheap.** At 34 layers it moves about 250 copies, p95 360 MiB, about 15–16 ms of GPU on the Mac, and makes up 92% of tour repacks (3.55% of tour steps). Order is unchanged, per the Decision. Task 2's whole-demand planning should cut the count; row 62 measures what remains.
- **Repack count after task 2.** Down 30% on the tour at 27 layers, flat at 34 (65+6 to 62+10), where every repack is one late pair no hole fits, at least 60 frames apart, so reservations cannot remove them. Compactions rose slightly on two walks, because judging on whole demand fails the downward pass more often (R18). Row 62 records what remains.
- **Device-limit starvation, existing and unchanged.** At the device limit, a pair that can never fit, sharing a drain with one that needs compaction, starves that pair too.
- **Compaction cost.** Up to about 200 MiB of copies in one submission on the hallway. If the task 1 timing is a visible hitch, the plan goes `blocked` before any format work, with budgeted incremental moves as the proposed next shape.
- **Row 43's fixture must grow today at that size.** Task 1's walk may show the first such fixture is the hallway itself, which cannot run in routine tests, so a synthetic one is needed.
- **Row 64's map.** The owner confirms the hallway has closet reveals, location unknown. Task 10 pulls `trigger_volume`, `entity_spawner` and mover entities from `stress-warren-hallway-inspection.map`. If none is a door, the row goes back to the owner with another map or a narrower check.
- **Disk.** 71 GB free. Task 10 needs about 25 GB for `xctrace`, plus baseline PRLs and a comparison binary. Delete raw traces after each run.
- **Mac confounders in task 10.** Temperature-gate runs, keep the display awake across gaps, pilot one run per config, and compare frames at equal clock.

## Hot paths

- **`plan_drain` runs per frame.** No new steady-state allocation: reservation and compaction buffers join `Scratch`, and `scratch_capacities` keeps proving it. Reservation work runs on target change, not per frame.
- **Compaction is rare and bounded.** One per demand at most, planned on the CPU before any copy records, and counted by kind for row 62.
- **Forward pass.** One more flat varying and no new binding. The mask path adds decoding and skips sampling for absent planes. Forward pass timing on the 5300M is checked in task 10.
- **Read path.** Fewer bytes by construction. `payload_from_pair_bytes` stays on the frame thread, per the brief's non-goal.
- **Compile.** The bound loop runs in Pack on every build. Row 66 measures it.
