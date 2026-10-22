# E18--projectile-traps — plan of record

mode: compact
status: active
read at: 473185e8

## Corrections
- Brief `read at 035816e3` → source changed since (level-compiler reflection probes, `net`, `netcode`, `postretro/main.rs`); `sim`, `ai`, `scripting-core`, `entities` and `level-format` are unchanged, so the Decision reads stand. Re-read cited symbols: `AiWeaponControllers::advance_actor`, `advance_weapon_activation`, `shot_scale_inputs`, `spawn_projectile`, `projectile_stage::advance`/`finish_spawn_tick`, `emit_splash_damage`, `TriggerSystem::run_authoritative_tick_with_dispatch`, `trigger_volume_bridge.rs`, `TriggerVolumesSection::from_bytes`, `resolve_trigger_volume`, `bind_command_at`, `partition.rs`, `is_trigger_consequential_primitive`, `SpawnContext`, `mirror_gameplay_projectile_with_descriptor_class`, `publish_observer_weapon_cues`. All match the brief.
- Path "trap-private `WeaponComponent` … like the enemy controller" → the enemy controller keeps its components in a sim-side map; there is no session-owned home a trap stage, a trigger tick and every reaction site already share. Planning around it by keeping each trap's runtime (stripped weapon, schedule, issue-ordered request queue, private tick clock) inside the engine-closed `ProjectileTrap` component itself, outside the registry's `Weapon` column. Every issuing site already holds the registry, the registry's weapon sweeps (resource tick, cancel, descriptor refresh) never see it, a mod hot reload leaves it, and a level reinstall drops it with the entity. The component never replicates (reserved discriminant only).
- Path "Feed an enemy-shaped spawn list" → kept; trap spawns ride a separate `TickEvents::trap_projectile_spawns` of the same `EnemyProjectilePresentationSpawn` type, mirrored by the same netcode call.
- Brief co-op "observer shot id": trap shots publish as weapon `activate` emissions on the trap entity, the route local and remote fire already take; mirrors stamp the trap's network id into the shot id at registration (`observer_shot_id`).

## Delegated answers
- `TRIGGER_VOLUMES_VERSION` — stays 2. Field order, widths and entry layout are unchanged and every existing version-2 file decodes identically; value 4 only widens the closed command vocabulary, which a decoder that predates it already rejects as an invalid command.
- Command-to-kind dispatch shape — one `TriggerCommand` enum on the trigger component (`Start`, `Stop`, `Reverse`, `GoToPathNode`, `Fire`) with an exhaustive acceptance table per brainless member kind (mover, trap, spawner). On Enter the trigger resolves `target_tag` once, skips members carrying a brain, and applies the command per accepting kind in registry order; the install pass warns once per trigger whose non-empty tag some member carries but none accepts.

## AC-to-proof

| AC | Proof | Result |
|---|---|---|
| F1 fire spawns at origin along `angles`/`mangle`/`angle` (−1/−2), descriptor speed/damage | `traps::tests::fire_launches_from_origin_along_map_facing`, `traps::tests::trap_facing_follows_angles_mangle_and_angle_sentinels`, `traps_surface_tests::volley_traps_fire_along_their_authored_facing`, builtin `projectile_trap_rotation_is_the_view_frame` | pass |
| F2 multi-shot sequence count/spacing | `traps::tests::fire_runs_the_authored_shot_sequence` | pass |
| F3 fire during execution/recovery dropped; two fires one tick → one (P1) | `traps::tests::fire_during_execution_or_recovery_is_dropped`, `traps::tests::two_fires_on_one_tick_give_one_execution`, `sim::trap_tick_tests::trigger_bound_trap_step_acts_on_the_enter_tick_and_dedupes_with_the_direct_fire` | pass |
| F4 trap vs spawner duplicate fire (P2) | `sim::trap_tick_tests::duplicate_fire_dedupes_on_traps_but_not_spawners` | pass |
| F5 no resource limits | `traps::tests::resource_weapons_never_run_dry_overheat_or_reload` | pass |
| F6 inert trap: unknown/hitscan/charged, one warning; inert ignores all (P13) | `traps::tests::inert_weapons_warn_once_and_valid_projectile_arms`, `traps::tests::inert_trap_ignores_every_request`, `traps::tests::a_command_on_a_non_trap_warns_and_skips` | pass |
| T1 in-tick vs latched timing (P7, P21, P14, P15) | `sim::trap_tick_tests::trigger_bound_trap_step_acts_on_the_enter_tick_and_dedupes_with_the_direct_fire`, `sim::trap_tick_tests::teleport_bound_trap_step_acts_on_the_teleport_tick`, `sim::trap_tick_tests::post_wait_steps_latch_to_the_next_tick_even_from_a_catch_up_frame` | pass |
| T2 `start_on_spawn` (P20) | `traps::tests::start_on_spawn_starts_only_on_an_authoritative_install`, `traps::tests::start_on_spawn_precedes_level_load_requests`, `traps_surface_tests::volley_sentry_starts_on_spawn_with_attackerless_mirrored_shots` | pass |
| T3 latched before in-tick (P17) | `sim::trap_tick_tests::latched_requests_apply_before_trigger_requests` | pass |
| T4 N intervals → N executions, phase, rounding | `traps::tests::schedule_fires_n_executions_over_n_intervals_after_its_phase` | pass |
| T5 start/stop rules, option fallback both runtimes (P5) | `traps::tests::start_stop_rules_follow_issue_order_and_per_run_options`, `traps::tests::stop_halts_repeats_and_lets_the_inflight_execution_finish`, `ir::parity_tests::member_builder_sdk::trap_member_builders_emit_identical_steps_in_both_runtimes` | pass |
| T6 fire + phase-0 start (P3) | `traps::tests::fire_and_phase_zero_start_share_one_execution` | pass |
| T7 fire + stop (P23) | `traps::tests::fire_and_stop_on_one_tick_fire_once_and_stay_stopped` | pass |
| T8 direct command before bound step (P24) | `sim::trap_tick_tests::direct_command_precedes_bound_trap_steps` | pass |
| T9 fire between repeats; fire on due tick (P4, P19) | `traps::tests::off_schedule_fire_never_moves_the_schedule` | pass |
| T10 short interval drops overlaps (P16) | `traps::tests::interval_shorter_than_execution_drops_overlapping_repeats` | pass |
| T11 multi-tick frame (P18) | `sim::trap_tick_tests::catch_up_frame_matches_one_tick_per_frame` | pass |
| T12 defaults: 1000 ms rounded | `traps::tests::authored_defaults_repeat_every_rounded_second`, builtin `projectile_trap_kvps_default_to_a_silent_one_second_trap` | pass |
| T13 `hold` primary | `traps::tests::hold_primary_runs_one_execution_per_request` | pass |
| T14 invalid timing: KVP inert + warning; literal rejects reaction | builtin `projectile_trap_invalid_timing_kvp_is_inert_with_one_warning`, `data_descriptors::tests::reactions::trap_start_literals_and_addressing_are_rejected_in_both_vms` | pass |
| T15 reinstall clears (P10) | `traps_surface_tests::reinstall_drops_old_trap_work` | pass |
| T16 hot reload (P9) | `traps_surface_tests::mod_hot_reload_keeps_started_traps_and_latched_requests` | pass |
| D1 direct damage, no attacker, ledger credit | `traps::tests::trap_projectile_damages_without_an_attacker` | pass |
| D2 splash, no attacker, occlusion | `traps::tests::trap_splash_damages_without_an_attacker_and_respects_walls` | pass |
| W1 Enter fires every tagged trap | `trigger_direct_commands::tests::enter_fires_every_trap_carrying_the_tag_on_that_tick`, `traps_surface_tests::volley_plates_fire_their_tagged_traps_on_the_enter_tick` | pass |
| W2 mover+trap start/stop | `trigger_direct_commands::tests::direct_start_and_stop_reach_movers_and_traps` | pass |
| W3 kind acceptance, no warning (P12) | `trigger_direct_commands::tests::direct_commands_reach_only_accepting_kinds` | pass |
| W4 spawner `fire` = one batch per spawner | `trigger_direct_commands::tests::direct_fire_spawns_one_batch_per_spawner_on_the_host_only` | pass |
| W5 brain skip; spawned enemy untouched | `trigger_direct_commands::tests::direct_commands_skip_brained_members` | pass |
| W6 install warning rules; mover regression | `trigger_direct_commands::tests::install_warns_once_for_a_tag_no_member_accepts`, `trigger_direct_commands::tests::direct_start_still_starts_a_tagged_mover`, existing mover tests | pass |
| W7 `multiple` + `rearm_ms` | `trigger_direct_commands::tests::rearmed_trigger_refires_traps_only_after_rearm` | pass |
| W8 level file round trip; out-of-range fails | `level-format` `fire_command_round_trips_and_larger_values_fail`, compiler `trigger_volume_fire_command_resolves_to_section_value_four`, bridge `level_records_preserve_trigger_event_names_and_commands_on_components` | pass |
| C1 client never fires (P11) | `sim::trap_tick_tests::client_trap_steps_issue_and_latch_nothing`, `traps_surface_tests::client_install_arms_traps_without_starting_them` | pass |
| C2 mirror + shot ids | netcode `same_tick_trap_shots_mirror_with_their_weapon_class_and_distinct_shot_ids` | pass |
| C3 co-op sweep plate dedupe (P8) | `sim::trap_tick_tests::two_activators_on_a_zero_rearm_plate_fire_one_sweep` | pass |
| C4 no trap state on wire; versions unchanged | netcode `started_trap_snapshot_carries_no_trap_state_and_versions_hold` | pass |
| S1 trap-wave sweep in TS and Luau | `traps_surface_tests::wave_sweep_fires_in_order_on_its_constant_ticks_in_both_runtimes` | pass |
| S2 loop zone | `traps_surface_tests::wave_loop_zone_staggers_repeats_and_stops_on_exit_in_both_runtimes` | pass |
| S3 trap-volley | `traps_surface_tests::volley_*` (3) | pass |
| S4 typedefs, drift, parity | typedef `trap_member_handle_and_step_types_are_emitted_in_both_sdk_surfaces` + committed drift fixtures, member-builder parity, light-membership `projectile_traps_join_the_member_table_at_their_origin` | pass |
| M1 fixture maps play | owner, in-engine | manual — outstanding |
| M2 co-op client on trap-volley | owner, two machines | manual — outstanding |
| M3 TrenchBroom FGD | owner, TrenchBroom | manual — outstanding |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: `ProjectileTrap` component + `projectile_trap` builtin + install resolution + trap stage (fire, schedule, no-resource weapon) + attackerless projectiles + presentation spawns/emissions; `start_on_spawn` fixture through the compiled-level harness | integrating executor | — | done: `traps::tests` (22), `traps_surface_tests` volley (3) |
| 2 | Trigger command: `TriggerCommand`, section 44 value 4, compiler, bridge, loader counts, Enter dispatch over brainless kinds, install warning | integrating executor | 1 | done: `trigger_direct_commands::tests` (8), format/compiler/bridge tests |
| 3 | Script surface: `fireTrap`/`startTrap`/`stopTrap` member steps, bound commands, partition lists, literal validation, SDK TS/Luau handle, typedefs, drift and parity fixtures, light-membership kind lists | integrating executor | 1 | done: typedef drift (45), `trap_start_literals…` both VMs, membership tests, `trap_tick_tests` (9) |
| 4 | Co-op: host mirror wiring in the frame loop, client gating, wire/snapshot diff gate | integrating executor | 1 | done: netcode `same_tick_trap_shots…`, `started_trap_snapshot…`, `client_trap_steps…` |
| 5 | Fixtures (`trap-volley`, `trap-wave`, `trap-wave-luau`), FGD, surface tests | integrating executor | 2, 3 | done: `traps_surface_tests` (7); FGD class + `fire` choice; author docs |
| 6 | Preflight, review loop, context docs | integrating executor | 1–5 | review round 1 fixed; preflight pending; context docs at landing |
