# E25--landmark-level-change — plan of record

mode: compact
status: active
read at: e743823

## Corrections
- Source changed since the brief's `8489a1f` only in streaming, capture and debug-UI code; no symbol the Decisions or Path cite moved. Decision reads reused.
- Path "Respawn seeds seam latches beside its `settle_arrival` call" → respawn's `settle_arrival` runs inside `sim::finish_tick_start`, which has no seam state; the App frame loop latches each `RespawnTurn` pawn right after `finish_tick_start`, before the trigger stage (same tick, same ordering).
- Path "`UnbackedSpawn` carries no velocity, movement state or latch" → `UnbackedSpawn` gains `arrival: Option<SeamSpawnArrival>`; the resolver holds per-seat seam offers (`set_seam_offers`), tests the fit at resolve and again at promotion (`SpawnPlacements::with_arrival_world`), and participation entry applies the motion, latch and `arrive` queue.
- Brief "movement-state payload" → a `Swim` payload names the source level's fluid table, so it arrives as `Normal` (the next tick's immersion re-enters swim); crouch and slide carry their live (shrunk) capsule with the state.
- Seat 0 is the host's or single player's seat (`Seat(0)`), as `CarriedState` already assumes; the seam checkpoint pose reads seat 0's harvested arrival.
- Brief §Wire format "brushes (`u32` count, each a `u32` plane count then planes as four `f32`)" → each brush also carries its AABB min and max (six `f32`) before the plane count: id 61 uses the id-52 brush encoding, as the same paragraph's "mirrors `TeleportersSection` (id 55)" and `build_pipeline.md` require. Clarification; same meaning.
- Seam checkpoint pose → saves `LandmarkRelative::rest_offset`: a crouched or sliding crossing's centre raised to standing height with the feet kept, since Continue arrives standing at rest (review finding).
- Rename scope → the classname and the compiler's `streaming_split_regions`; runtime SH streaming identifiers (`SeamWarm`, "seam portal" strings) keep their names, out of this brief's scope.

## Delegated answers
- None: the brief has no open questions.

## AC-to-proof

Test homes: compile rows in `level-compiler` (`prl-build` bin, `tests/`), wire rows in `level-format`, arrival maths in `physics` (`movement/seam_arrival_tests.rs`), activation, latch, occupancy and events in `sim` (`seam_stage/tests.rs`, `seam_system/tests.rs`, `seam_commands/tests.rs`), level-change, spawn, co-op and save rows in the `postretro` binary (`startup/seam_*_tests.rs`, `seam_crossing_tests.rs`), all CPU-only. No row needs a GPU test.

| AC | Proof | Result |
|---|---|---|
| Arr 1 offset, yaw/pitch, velocity, boost, vault; equals seamless teleport | `seam_arrival_equals_a_seamless_teleport_between_the_same_landmarks`, `seam_arrival_aborts_a_vault_to_normal_and_keeps_its_velocity` | pass |
| Arr 2 facing and velocity read before unload (host, client) | `seam_crossing_arrives_host_at_mapped_pose_view_and_motion`, `seam_harvest_reads_client_facing_in_its_teleport_frame_before_unload`, `seam_harvest_reads_seats_inside_with_their_own_view_source` | pass |
| Arr 3 no landmark → bounds centre yaw 0; authored yaw rotates | compiler `parse/seams/tests.rs` (landmark rows), `seam_arrival_without_a_landmark_rotation_keeps_world_motion` | pass |
| Arr 4 missing seam / blocked → start, one warning | `seam_arrival_missing_seam_or_blocked_places_host_at_start_with_one_warning`, `seam_arrival_maps_through_the_destination_landmark_and_falls_back_with_one_warning` | pass |
| Arr 5 latched against armed teleporter | `seam_arrival_latches_against_an_armed_teleporter`, `seam_arrival_latches_armed_teleporters_and_settles_at_rest` | pass |
| Arr 6 host settles/reveals at arrival pose | `seam_crossing_arrives_host_at_mapped_pose_view_and_motion` | pass |
| Arr 7 zero-health pawn arrives landmark-relative | `seam_crossing_carries_seat_state_as_load_level_does` (0 HP case) | pass |
| Arr 8 disarmed destination places, latches, fires `arrive` | `seam_disarmed_destination_still_places_latches_and_queues_arrive` | pass |
| Arr 9 client pre-teleport stamp (P8) | `seam_harvest_reads_client_facing_in_its_teleport_frame_before_unload` | pass |
| Carry 1 carried state as `loadLevel` | `seam_crossing_carries_seat_state_as_load_level_does` | pass |
| Carry 2 first spawn consumes; later levels silent (P2) | `seam_arrival_consumed_then_next_level_starts_silently`, `seam_crossing_arrives_host_at_mapped_pose_view_and_motion`; respawn half structural (only install and participation entry read arrivals) | pass |
| Carry 3 outside at harvest → start (P1) | `seam_activator_outside_at_harvest_arrives_at_player_start` | pass |
| Carry 4 failed / cancelled / superseded → no arrival (P12) | `seam_load_failure_cancel_or_supersede_leaves_no_arrival` | pass |
| Carry 5 suspend at three points (P6) | `seam_suspend_before_drain_or_during_load_leaves_no_arrival`, `seam_suspend_while_a_crossing_client_loads_clears_its_arrival` | pass |
| Ckpt 1 saves seam + pose; Continue twice (P22) | `seam_crossing_saves_its_pose_and_continue_arrives_there_twice`, `seam_crouched_crossing_saves_a_standing_pose_continue_can_take` | pass |
| Ckpt 2 blocked / missing on Continue → start, one warning | `seam_continue_blocked_or_missing_seam_places_host_at_start_with_one_warning` | pass |
| Ckpt 3 failed load keeps checkpoint (P22) | `seam_load_failing_after_its_save_keeps_the_seam_checkpoint` | pass |
| Ckpt 4 host outside → mark, no pose; client writes no pose | `seam_host_outside_saves_a_seam_mark_without_pose_and_continue_uses_the_set` | pass |
| Ckpt 5 seam spawn set; later checkpoint replaces (P23) | `seam_spawn_set_serves_until_a_later_checkpoint_or_level_start` | pass |
| Ckpt 6 `LevelRecord` round-trips; old file loads | sim `run_level_record_round_trips_with_and_without_seam_and_pose` | pass |
| Ckpt 7 Continue over a backdrop (P24) | `seam_continue_over_a_backdrop_keeps_the_saved_pose` | pass |
| Ckpt 8 no run / unsaveable store (P25) | `seam_crossing_without_a_run_still_selects_the_seam_set` | pass |
| Ckpt 9 typo warns; no-pose Continue silent (P26) | `seam_silent_fallback_ends_at_a_later_unknown_checkpoint`, `seam_host_outside_saves_a_seam_mark_without_pose_and_continue_uses_the_set` | pass |
| Ckpt 10 later checkpoint / restart outlives pose (P27) | `seam_pose_is_outlived_by_a_later_checkpoint_or_restart` | pass |
| Ckpt 11 suspend then Continue (P28) | `seam_suspend_then_continue_resumes_where_the_run_saved` (a, b); (c) by P28 (b) plus `seam_suspend_while_a_crossing_client_loads_clears_its_arrival` | pass |
| Ckpt 12 Continue settles at pose, `arrive` once | `seam_crossing_saves_its_pose_and_continue_arrives_there_twice` | pass |
| Ckpt 13 hosted Continue: clients in set, no reservation (P29) | `seam_hosted_continue_places_clients_in_the_seam_set_never_at_the_pose` | pass |
| Ckpt 14 checkpoint on activation frame (P32) | `seam_checkpoint_on_the_activation_frame_ends_at_the_seam` | pass |
| Act 1 touch/use; disarmed; backdrop | sim `seam_touch_fires_on_entry_but_not_beside_the_bend`, `seam_latch_blocks_use_until_the_capsule_leaves`, `seam_arming_a_touch_seam_fires_…`, `seam_in_a_backdrop_never_activates`; app `seam_in_a_frontend_backdrop_never_activates_and_its_unload_writes_no_arrival` | pass |
| Act 2 popped frontend root over backdrop inert (P30) | `seam_in_a_frontend_backdrop_never_activates_and_its_unload_writes_no_arrival` | pass |
| Act 3 clears no pushed tree (P31) | `seam_activation_clears_no_pushed_tree`, `seam_save_under_a_pushed_tree_follows_the_menu` | pass |
| Act 4 script verb same frame wins (P3) | `seam_activation_loses_to_a_script_level_verb_on_the_same_frame`, `seam_activation_loses_to_a_same_frame_quit_to_menu` | pass |
| Act 5 unknown target warns once | `seam_unknown_target_warns_once_and_a_valid_seam_still_loads` | pass |
| Act 6 first in map order (P11, P15) | `seam_pick_takes_first_in_map_order_whatever_the_activation_order`, sim `seam_activations_record_each_seam_once_in_activation_order` | pass |
| Act 7 disarmed/emptied after activation still loads (P21) | `seam_activated_then_disarmed_or_emptied_still_loads` | pass |
| Act 8 unknown first + valid (P16) | `seam_unknown_target_warns_once_and_a_valid_seam_still_loads` | pass |
| Act 9 AABB-only overlap is outside | sim `seam_inside_test_is_the_brush_union_not_its_bounding_box`, `seam_touch_fires_on_entry_but_not_beside_the_bend` | pass |
| Act 10 arming with player inside fires next tick | sim `seam_arming_a_touch_seam_fires_for_unlatched_standing_occupants_next_tick` | pass |
| Act 11 zero-HP undeclared barred; declared follows state | sim `seam_zero_health_undeclared_pawn_cannot_activate_and_a_declared_one_follows_its_state` | pass |
| Act 12 seam-only tag: no absent-tag warning, counts | `occupancy_volume_tags` includes seams; sim `seam_occupancy_counts_by_the_inside_test_armed_or_not` | pass (warning half by code read; no dedicated install test) |
| Act 13 bent-seam occupancy rules; all-players arm moves everyone | sim `seam_occupancy_counts_by_the_inside_test_armed_or_not`, `seam_arming_a_touch_seam_fires_…`; app harvest moves every seat inside (`seam_harvest_reads_seats_inside_with_their_own_view_source`) | pass |
| Latch 1 D11 table incl. P4 | sim `seam_latch_follows_the_d11_table_for_touch`, `seam_latch_blocks_use_until_the_capsule_leaves`, `seam_arming_keeps_latches`, `seam_disarming_keeps_latches_and_never_fires` | pass |
| Latch 2 `arrive` once, host-only, first tick; none for start-placed; brush + script | sim `seam_system/tests.rs` (brush and `seamEvents` both run); app install tests (queued only on a successful arrival) | pass |
| Latch 3 teleport into touch seam fires (P9) | sim `seam_teleport_arrival_inside_a_touch_seam_fires_on_that_tick` | pass |
| Latch 4 respawn onto start inside seam latched (P14, P19) | sim `seam_respawn_latch_holds_whatever_the_armed_state`; frame loop latches each `RespawnTurn` before the trigger stage | pass |
| Latch 5 latch in disarmed seam clears on leave (P18) | sim `seam_latch_in_a_disarmed_seam_clears_on_leave_so_arming_fires` | pass |
| Latch 6 spawn beside bend not latched | sim `seam_touch_fires_on_entry_but_not_beside_the_bend` | pass |
| Latch 7 self-target reload (P10) | `seam_targeting_its_own_map_reinstalls_the_host_where_it_stood` | pass |
| Latch 8 example compiles TS and Luau; disarms on arrival on dev pair | scripting-core `seam_surface_example_tests.rs`; dev pair compiles with both scripts (`prl-build`); in-engine disarm: manual | pass (compile); in-engine outstanding |
| Coop 1 client inside arrives at entry; outside at start | `seam_client_inside_arrives_at_participation_entry_and_outside_takes_its_start` | pass |
| Coop 2 client settles/reveals at arrival; no start held | same, hint asserted; reveal at hint by existing stand-in tests | pass |
| Coop 3 blocked at resolve / promotion (P13) | `seam_client_arrival_blocked_at_resolve_takes_a_start_for_good`, `seam_client_arrival_blocked_at_promotion_spawns_at_a_start_and_rehints` | pass |
| Coop 4 no resolved command → pawn yaw, pitch 0 | `seam_harvest_reads_seats_inside_with_their_own_view_source` | pass |
| Coop 5 host change before client spawns (P5, P20) | `seam_host_level_change_before_the_client_spawns_clears_its_arrival` | pass |
| Coop 6 demote/reconnect (P7) | `seam_client_leaving_participation_after_its_arrival_spawn_re_enters_at_a_start`, `seam_client_reconnecting_before_its_spawn_still_arrives` | pass |
| Coop 7 blocked then re-promote (P17) | `seam_client_arrival_blocked_at_resolve_takes_a_start_for_good`, `seam_client_arrival_blocked_at_promotion_spawns_at_a_start_and_rehints` | pass |
| Comp 1 build failures name entity; no-spawn warns; blanks default | compiler `parse/seams/tests.rs` | pass |
| Comp 2 arrival spawns as the start's class | `seam_arrival_spawns_as_the_class_of_the_start_it_replaces`, coop entry test | pass |
| Comp 3 FGD defaults equal compiler's | `fgd_level_seam_defaults_equal_the_compilers` | pass |
| Comp 4 rename; grep gate | `stream_split_volume` streaming hint tests; xtask `retired_streaming_split_classname_appears_only_in_plans` | pass |
| Comp 5 `seamless` crosses as classic, no warning | compiler `seamless` test; runtime ignores mode | pass |
| Manual 1 dev pair cross and back, turned and moving | owner, in-engine (GPU) | outstanding |
| Manual 2 co-op crossing, one in one out | owner, in-engine (GPU) | outstanding |
| Manual 3 run cross, quit, Continue | owner, in-engine (GPU) | outstanding |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Rename `streaming_seam_volume` → `stream_split_volume` (own commit) | integrating executor | — | done (rename commit, xtask `seam_gates`) |
| 2 | Behavior-preserving splits of `trigger_system.rs` and `parse.rs` (own commits) | integrating executor | 1 | done |
| 3 | Factor teleport arrival apply out of `resolve`; seam arrival mapping (riskiest premise: the chokepoint splits cleanly) | integrating executor | — | done (`rotate_arrival_motion`, `movement/seam_arrival.rs`) |
| 4 | Compile + PRL id 61 + loader + FGD + compiler tests + dev fixture pair | worker A (worktree) | 2 | done (worker A) |
| 5 | Script surface: component, `getMapEntities("seam")`, handles TS/Luau, arm/disarm, `seamEvents`, `@seam`, binding table | worker B (worktree) | — | done (worker B) |
| 6 | Runtime install, inside test, activation, latch, occupancy, `arrive` dispatch | integrating executor | 3, 4, 5 | done (`seam_stage.rs`, `runtime_seams.rs`) |
| 7 | Frame-end pick, seam `LevelRequest`, harvest at unload, arrival clears | integrating executor | 6 | done (`seam_level_change.rs`) |
| 8 | Arrival spawn (host / SP), respawn latch, blocked fallback, Settling view, `arrive` | integrating executor | 7 | done (`install_level_seams`) |
| 9 | Co-op client arrival through the resolver and participation entry | integrating executor | 8 | done (resolver offers, participation entry) |
| 10 | Seam checkpoint: run record, level-start save, Continue, spawn-set fallback | integrating executor | 8 | done (`LevelRecord.seam`, `RunCheckpoint::Seam`, `seam_spawn_set`) |
| 11 | Lifecycle, co-op and save AC tests in the app crate | worker C (main checkout) | 6–10 | done (40 tests; fixed: an arriving host kept its player start) |
| 12 | Docs, dev-pair data script, scripting example | integrating executor | 6–10 | done (`docs/level-seams.md`, `seam-pair.{ts,luau}`) |
| 13 | Preflight, review panel, fixes | integrating executor | 11, 12 | |

## Review loop

Round 1: 9 reviewers (2 correctness tracers, 1 seam-pass tracer, 2 contract verifiers, 1 adversarial tester on Opus; 3 hygiene/drift on Sonnet), 3 slices plus a seam pass. No 🔴 in code.
- Fixed: a same-frame quit to menu (any `navigate_to_frontend`) lost to a seam (3 lenses); the script-request note now also clears at the redraw boundary; Continue from a crouched crossing fell back to a start (`rest_offset`); arrival fit also rejects a non-finite view; suspend clears the resolver's seam offers; an empty seam stage returns before any per-tick work; arrivals are taken only with a script context; comment drift (`settle_arrival`, `subject_token_carries`, trigger install, pick doc, `activated`, resolve doc, P29 pin, dev script headers, occupancy warning, V4a message); `scripting-reference` §Addressing gains seams.
- Left: the resolver's class probe through `assign_joiner_placement` can advance the overflow cursor and spend the once-per-level overflow warning when every start is taken (logs only); Luau `seamEvents` aborts a sparse table where QuickJS skips the entry, as `teleportEvents` does; typedef doc comments listing subject tokens omit `@seam` (the types include it); `context/lib` "not yet built" markers drop at landing, per the brief's Path.

Round 2 (fix commit only): 1 correctness tracer (Opus), 1 hygiene/drift (Sonnet).
- Fixed: "crouched" for the seam pose now reads the capsule size, not the state (a forced-crouch swim, a dash out of a crouch); the rest raise keeps the feet either way (no clamp); a resized capsule arriving in `Swim` or `Normal` lands in the posture state that retries the stand-up; split-region vs seam-portal prose in the SH streaming hints; doc wording for the level-request note. App-level test for a crouched crossing through Continue.
- Left: an airborne crouch pins the head, not the feet, so its Continue pose stands a little high (more likely blocked, falls back to a start with one warning; never breaks); a script request that later fails to resolve still dropped that frame's seams (the brief's literal rule).

## Verification

Final gate on `0f4d9be2` plus the plan: `cargo fmt --check`; `cargo clippy --workspace --all-targets --features postretro/dev-tools,postretro/observability -- -D warnings`; `cargo test --workspace --no-fail-fast` with the same features (107 test targets, 0 failed); earlier on this branch `cargo check --release`, `crate-graph --check` and the telemetry reducer unittest passed. This machine has no GPU adapter: adapter-gated renderer tests skipped, and no seam row depends on one. Pre-existing on `main`: the observability driver's movement descriptor fixture lacked `swim`/`vault` (fixed here); `postretro-combat-model` reports an unused `postretro-net` dependency (manifest warning, untouched).
