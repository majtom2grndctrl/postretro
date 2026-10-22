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

| AC | Proof | Status |
|---|---|---|
| F1 fire spawns at origin along `angles`/`mangle`/`angle` (−1/−2), descriptor speed/damage | `traps::tests::fire_launches_from_origin_along_map_facing` + `trap_facing_follows_angles_mangle_and_angle_sentinels` | achievable as stated |
| F2 multi-shot sequence count/spacing | `traps::tests::fire_runs_the_authored_shot_sequence` | achievable as stated |
| F3 fire during execution/recovery dropped; two fires one tick → one (P1) | `traps::tests::fire_during_execution_or_recovery_is_dropped` + `sim::trap_tick_tests::two_fire_sources_on_one_tick_give_one_execution` | achievable as stated |
| F4 trap vs spawner duplicate fire (P2) | `trap_tick_tests::duplicate_fire_dedupes_on_traps_but_not_spawners` | achievable as stated |
| F5 no resource limits | `traps::tests::resource_weapons_never_run_dry_overheat_or_reload` | achievable as stated |
| F6 inert trap: unknown/hitscan/charged, one warning; inert ignores all (P13) | `traps::install::tests::inert_weapons_warn_once_and_valid_projectile_arms` + `traps::tests::inert_trap_ignores_every_request` | achievable as stated |
| T1 in-tick vs latched timing (P7, P21, P14, P15) | `trap_tick_tests::*` (trigger-bound, teleport-bound, latched, multi-landing) | achievable as stated |
| T2 `start_on_spawn` (P20) | `traps::install::tests::start_on_spawn_*` + surface first slice | achievable as stated |
| T3 latched before in-tick (P17) | `trap_tick_tests::latched_requests_apply_before_trigger_requests` | achievable as stated |
| T4 N intervals → N executions, phase, rounding | `traps::tests::schedule_*` | achievable as stated |
| T5 start/stop rules, option fallback both runtimes (P5) | `traps::tests::start_stop_*` + SDK parity fixture | achievable as stated |
| T6 fire + phase-0 start (P3) | `traps::tests::fire_and_phase_zero_start_share_one_execution` | achievable as stated |
| T7 fire + stop (P23) | `traps::tests::fire_and_stop_on_one_tick_fire_once_and_stay_stopped` | achievable as stated |
| T8 direct command before bound step (P24) | `trap_tick_tests::direct_command_precedes_bound_trap_steps` | achievable as stated |
| T9 fire between repeats; fire on due tick (P4, P19) | `traps::tests::off_schedule_fire_never_moves_the_schedule` | achievable as stated |
| T10 short interval drops overlaps (P16) | `traps::tests::interval_shorter_than_execution_drops_overlapping_repeats` | achievable as stated |
| T11 multi-tick frame (P18) | `trap_tick_tests::catch_up_frame_matches_one_tick_per_frame` | achievable as stated |
| T12 defaults: 1000 ms rounded | `traps::tests::authored_defaults_repeat_every_rounded_second` | achievable as stated |
| T13 `hold` primary | `traps::tests::hold_primary_runs_one_execution_per_request` | achievable as stated |
| T14 invalid timing: KVP inert + warning; literal rejects reaction | builtin parse test + `data_descriptors` js/lua rejection tests | achievable as stated |
| T15 reinstall clears (P10) | surface test `reinstall_drops_old_trap_work` | achievable as stated |
| T16 hot reload (P9) | surface test `mod_hot_reload_keeps_started_traps` | achievable as stated |
| D1 direct damage, no attacker, ledger credit | `traps::tests::trap_projectile_damages_without_an_attacker` | achievable as stated |
| D2 splash, no attacker, occlusion | `traps::tests::trap_splash_damages_without_an_attacker_and_respects_walls` | achievable as stated |
| W1 Enter fires every tagged trap | `trigger_system` direct-command tests + `trap-volley` surface | achievable as stated |
| W2 mover+trap start/stop | `trigger_system::tests::direct_start_stop_reach_movers_and_traps` | achievable as stated |
| W3 kind acceptance, no warning (P12) | `trigger_system::tests::direct_commands_reach_only_accepting_kinds` | achievable as stated |
| W4 spawner `fire` = one batch per spawner | `trigger_system::tests::direct_fire_spawns_one_batch_per_spawner` | achievable as stated |
| W5 brain skip; spawned enemy untouched | `trigger_system::tests::direct_commands_skip_brained_members` | achievable as stated |
| W6 install warning rules; mover regression | `trigger_commands_install::tests::*` + existing mover tests | achievable as stated |
| W7 `multiple` + `rearm_ms` | `trigger_system::tests::rearmed_trigger_refires_traps_only_after_rearm` | achievable as stated |
| W8 level file round trip; out-of-range fails | `level-format` + compiler + bridge tests | achievable as stated |
| C1 client never fires (P11) | `traps::tests::client_requests_never_latch` + surface client install | achievable as stated |
| C2 mirror + shot ids | netcode mirror test for trap spawns | achievable as stated |
| C3 co-op sweep plate dedupe (P8) | `trap_tick_tests::two_activators_on_a_zero_rearm_plate_fire_one_sweep` | achievable as stated |
| C4 no trap state on wire; versions unchanged | netcode snapshot test + version diff gate | achievable as stated |
| S1 trap-wave sweep in TS and Luau | `traps_surface_tests::wave_sweep_*` | achievable as stated |
| S2 loop zone | `traps_surface_tests::loop_zone_*` | achievable as stated |
| S3 trap-volley | `traps_surface_tests::volley_*` | achievable as stated |
| S4 typedefs, drift, parity | typedef fixtures, parity fixtures | achievable as stated |
| M1 fixture maps play | owner, in-engine | manual |
| M2 co-op client on trap-volley | owner, two machines | manual |
| M3 TrenchBroom FGD | owner, TrenchBroom | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: `ProjectileTrap` component + `projectile_trap` builtin + install resolution + trap stage (fire, schedule, no-resource weapon) + attackerless projectiles + presentation spawns/emissions; `start_on_spawn` fixture through the compiled-level harness | integrating executor | — | done: `traps::tests` (22), `traps_surface_tests` volley (3) |
| 2 | Trigger command: `TriggerCommand`, section 44 value 4, compiler, bridge, loader counts, Enter dispatch over brainless kinds, install warning | integrating executor | 1 | done: `trigger_direct_commands::tests` (8), format/compiler/bridge tests |
| 3 | (in progress) Script surface: `fireTrap`/`startTrap`/`stopTrap` member steps, bound commands, partition lists, literal validation, SDK TS/Luau handle, typedefs, drift and parity fixtures, light-membership kind lists | integrating executor | 1 | |
| 4 | Co-op: host mirror wiring in the frame loop, client gating, wire/snapshot diff gate | integrating executor | 1 | |
| 5 | Fixtures (`trap-volley`, `trap-wave`, `trap-wave-luau`), FGD, surface tests | integrating executor | 2, 3 | |
| 6 | Preflight, review loop, context docs | integrating executor | 1–5 | |
