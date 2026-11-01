# E18 Ordering proof ledger

Rows refer to the binding Orderings in `index.md`. Tasks 1–6 results were
executed and recorded in `plan.md`; new Task 7 integration tests passed their
focused runs on 2026-10-30. Review and final preflight remain pending. This ledger does not certify
external visual proof (AC17/AC18).

| Row | Concrete proof | Result |
|-----|----------------|--------|
| O1 | `life_policy_o1_two_zeroed_subjects_fire_own_edges_and_one_legacy_event` | PASS, Task 7 (2026-10-30) |
| O2 | `life_policy_o2_heal_before_sweep_suppresses_edges_and_post_sweep_heal_rearms_latch` | PASS, Task 7 (2026-10-30) |
| O3 | `respawn_player_event_coalesces_last_request_and_preserves_immediate_state_write`; `respawn_last_request_wins_and_adjacent_state_write_does_not_resurrect_early` | PASS, Task 5 |
| O4 | Same two listed-order tests as O3; immediate state write precedes next-tick drain | PASS, Task 5 |
| O5 | `life_state_player_event_applies_only_to_subject_and_same_state_preserves_elapsed`; `life_policy_o5_same_state_set_continues_through_next_real_tick_and_condition` | PASS, Task 7 (2026-10-30) |
| O6 | `life_state_o6_down_disconnect_reclaims_rejoin_state_until_hold_expiry_then_starts_initial`; `rejoin_spawn_routes_fall_back_for_missing_map_removed_name_or_expired_hold` | PASS, Task 6 |
| O7 | `rejoin_level_unload_clears_bound_and_held_names_before_next_level_materialization` | PASS, Task 6 |
| O8 | `life_state_o8_kept_name_remaps_shifted_id_and_new_profile_without_reset`; `life_state_o8_removed_name_enters_initial_then_undeclared_with_one_warning_each`; `life_state_reload_kept_movement_installs_safe_crouch_and_preserves_reload_continuity`; `life_state_reload_skipped_movement_adds_initial_forced_crouch_before_next_physics_tick`; `life_state_host_remap_runs_when_movement_refresh_keeps_old_or_skips_missing_snapshot` | PASS, Task 4 |
| O9 | `life_state_relay_150ms_transition_reconciles_without_a_camera_cut`; both transition directions and the k/k+1 correction counts against the no-transition baseline are asserted inside the shared `transition_run` helper via `assert_one_correction` | PASS, Task 2 |
| O10 | `respawn_conditioned_o10_o28_coalesced_observer_cuts_once_without_replaying_last_teleporter_cue` | PASS, Task 5 |
| O11 | `conditioned_player_event_down_refuses_remote_fire_and_reaches_its_owner` | PASS, Task 1 |
| O12 | `life_state_o12_owner_refuses_locally_until_host_fire_permission_arrives` | PASS, Task 4 |
| O13 | `life_state_relay_150ms_transition_reconciles_without_a_camera_cut`; `life_state_relay_150ms_above_teleport_floor_still_glides`; the k+1 baseline bound is asserted in the shared `transition_run` helper (`assert_one_correction`) | PASS, Task 2 |
| O14 | `life_state_o14_profile_only_payload_restamps_rebuild_before_or_after_snapshots_without_reset`; `life_state_o14_profile_reload_with_backlog_corrects_once_after_payload_and_each_old_profile_merge` | PASS, Task 7 (2026-10-30) |
| O15 | `life_state_o15_trigger_write_reads_zero_before_next_host_tick`; `life_state_o15_player_event_first_next_pass_reads_16_and_threshold_first_at_900`; `respawn_remote_without_command_uses_transform_yaw_and_first_elapsed_pass`; `rejoin_remote_first_tick_without_command_reaches_sixteen_ms_and_keeps_other_pawn`; `respawn_conditioned_o33_admits_start_in_new_state_and_turns_its_aim_before_one_movement_step` | PASS, Tasks 1/4/5/6 |
| O16 | `life_state_o16_o35_trigger_transition_blocks_weapons_but_keeps_stage_pickup_profile`; same test asserts movement at T under the old profile and T+1 under speedScale 0 for local/remote pawns; look sampling supplemented by O38 | PASS, Tasks 2/3/7 (integrated O38 passes) |
| O17 | `respawn_full_tick_turns_forward_input_and_integrates_once_from_rest`; O33 conditioned command-100 start/one-step check | PASS, Task 5 |
| O18 | `respawn_seat_order_counts_later_queued_pawns_then_earlier_relocations` | PASS, Task 5 |
| O19 | `respawn_named_state_resolves_after_reload_or_falls_back_once`; `respawn_queued_name_survives_table_relower_and_removal_until_drain` | PASS, Task 5 |
| O20 | `respawn_o20_actual_level_unload_clears_requests_and_placements_while_suspend_keeps_them`; O7 actual next-level materialization | PASS, Tasks 5/6 |
| O21 | `life_state_o21_app_demotion_harvests_pre_respawn_state_and_missing_pawn_drain_warns_once` | PASS, Task 6 |
| O22 | `life_state_o22_held_dead_survives_app_suspend_reclaims_as_spectating_and_unload_clears_name`; `life_state_harvest_hold_expiry_discards_named_record_before_fresh_admission` | PASS, Task 6 |
| O23 | `life_state_o23_actual_reload_digest_demotion_harvests_down_at_ten_seconds_then_rejoins` | PASS, Task 6 |
| O24 | `life_state_clock_o24_trim_and_delay_corrections_use_positive_bounded_rates`; `life_state_ui_projection_advances_frozen_value_without_mutating_gameplay_store`; `life_state_elapsed_witness_retains_accepted_decrease_across_same_frame_samples`; `life_state_o24_actual_backlog_trim_ack_replay_keeps_240hz_display_advancing_and_converges` | PASS, Task 7 (2026-10-30) |
| O25 | `life_state_clock_o25_rendered_value_advances_above_tick_rate_for_fifteen_seconds`; `life_state_ui_projection_advances_frozen_value_without_mutating_gameplay_store`; `life_state_clock_positive_long_frames_remain_bounded_and_converge`; `life_state_clock_reset_serial_reseeds_but_kept_name_remaps_preserve_continuity` | PASS, Task 4 |
| O26 | `o26_snapshot_before_control_keeps_profile_and_rebases_replay_elapsed`; `o26_reconcile_missing_table_preserves_life_stamps_and_counts_replay_once` | PASS, Task 1; Control installation deferred to represent loss/resend |
| O27 | `life_state_o27_short_state_is_seen_for_exactly_one_host_pass` | PASS, Task 4; no client observation guarantee asserted |
| O28 | `respawn_conditioned_o10_o28_coalesced_observer_cuts_once_without_replaying_last_teleporter_cue` | PASS, Task 5; production cue requests, no audio-device playback claim |
| O29 | `respawn_latches_teleporter_armed_in_the_same_tick_start_until_reentry`; `respawn_arrival_latches_newly_armed_regions_until_leave_and_reenter` | PASS, Task 5 |
| O30 | `life_state_o30_app_suspend_keeps_host_clock_and_demotes_remote_before_rejoin` | PASS, Task 6 |
| O31 | `rejoin_remote_first_tick_without_command_reaches_sixteen_ms_and_keeps_other_pawn` | PASS, Task 6 |
| O32 | `respawn_remote_without_command_uses_transform_yaw_and_first_elapsed_pass` | PASS, Task 5 |
| O33 | `respawn_conditioned_o33_admits_start_in_new_state_and_turns_its_aim_before_one_movement_step` | PASS, Task 5 |
| O34 | `life_state_o34_new_declaration_enters_initial_and_o37_client_remaps_nothing`; `life_state_host_hud_publishes_name_and_exact_tick_milliseconds` | PASS, Task 4 |
| O35 | `life_state_o16_o35_trigger_transition_blocks_weapons_but_keeps_stage_pickup_profile` | PASS, Task 3 |
| O36 | `o36_two_payloads_before_merge_resolve_each_pawns_own_revision` | PASS, Task 1 |
| O37 | `life_state_o37_actual_movement_refresh_carries_fields_and_stamps_before_client_no_remap`; `life_state_identity_keeps_name_continuity_and_resets_on_real_witnesses` | PASS, Task 4 |
| O38 | `life_look_input_freezes_yaw_pitch_and_undeclared_view_still_turns`; `life_state_o38_received_look_gate_freezes_sent_facing_and_host_accepts_in_flight_turns` | PASS, Task 7 (2026-10-30) |
| O39 | `life_state_o39_wait_tail_enters_at_drain_and_reload_drops_pending_tail`; `life_state_o39_actual_staged_app_reload_discards_a_parked_life_state_wait_tail` | PASS, Task 7 (2026-10-30) |
| O40 | `life_state_touch_controls_teleport_presence_independently_of_vault_bar`; `life_state_refused_teleport_still_prunes_arrival_latches`; `life_state_use_touch_and_occupancy_gate_independently_without_false_exit` | PASS, Tasks 2/3 |
| O41 | `life_state_o41_actual_reload_remaps_removed_down_before_digest_harvest_and_initial_rejoin` | PASS, Task 6 |
| O42 | `o42_same_revision_profile_removal_keeps_stamps_until_host_remap` | PASS, Task 1 |
| O43 | `life_state_vault_event_write_clears_t_record_and_next_snapshot_matches_baseline` | PASS, Task 2 |
| O44 | `host_seat_cuts_a_respawn_on_the_delayed_timeline_without_blending_from_the_death_spot` and `host_seat_cuts_a_teleporter_arrival_on_the_delayed_timeline_without_blending` (netcode `host_presentation::discontinuity_tests`, production `host_present_client_pawns`); registry pose `respawn_tick_start_resets_crouched_geometry_motion_budgets_and_registry_pose_at_every_alpha`; O10/O28 observer cut | PASS pending gate — O44 restated by owner 2026-11-01 (B03-F1 resolved) |
